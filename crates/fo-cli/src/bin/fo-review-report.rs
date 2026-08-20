#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

use clap::Parser;
use fo_core::{
    CompositeSearchResult, HybridOverlapEvidence, HybridSearchReport, NormalizationProfile,
    SearchResult, SemanticFusionReport, SemanticRelationshipClass, normalize_with_provenance,
};
use fo_corpus::{
    CorpusDocument, CorpusManifest, MANIFEST_FILENAME, atomic_write, sha256_hex, unix_timestamp,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const REVIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-review-report",
    version,
    about = "Render a standalone human review page with aligned source/specimen evidence"
)]
struct Cli {
    /// Root of a verified fo-corpus containing manifest.json and source documents.
    corpus_root: PathBuf,
    /// Original specimen text file used for the search.
    specimen: PathBuf,
    /// JSON from fo query, fo-composite, fo-search query, or fo-semantic-fuse.
    results: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    /// Stable target identifier used in downloaded review decisions.
    #[arg(long)]
    target_id: Option<String>,
    /// Optional JSON NormalizationProfile matching the index that produced the results.
    #[arg(long)]
    normalization_profile: Option<PathBuf>,
    #[arg(long, default_value_t = 50)]
    maximum_candidates: usize,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_source_bytes: u64,
    #[arg(long, default_value_t = 260)]
    context_characters: usize,
    #[arg(long, default_value = "FrankenOverlap evidence review")]
    title: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewReport {
    schema_version: u32,
    generated_at_unix: u64,
    title: String,
    corpus_id: String,
    corpus_provider: String,
    corpus_manifest_sha256: String,
    target_id: String,
    specimen_path: String,
    specimen_sha256: String,
    results_path: String,
    results_sha256: String,
    normalization_profile: NormalizationProfile,
    candidates: Vec<ReviewCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewCandidate {
    rank: usize,
    external_id: String,
    title: String,
    score: f32,
    relationship: String,
    textual_provenance_supported: bool,
    lexical_supported: bool,
    semantic_supported: bool,
    semantic_only: bool,
    source_relative_path: String,
    source_sha256: String,
    source_url: String,
    author_or_issuer: String,
    published_or_filed: Option<String>,
    metadata: BTreeMap<String, String>,
    blocks: Vec<ReviewBlock>,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewBlock {
    block_index: usize,
    query_token_start: usize,
    query_token_end: usize,
    source_token_start: usize,
    source_token_end: usize,
    query_byte_start: usize,
    query_byte_end: usize,
    source_byte_start: usize,
    source_byte_end: usize,
    edit_distance: usize,
    edit_similarity: f32,
    matched_tokens: usize,
    raw_score: f32,
    expected_false_matches: f64,
    query_excerpt: String,
    source_excerpt: String,
    query_context_html: String,
    source_context_html: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewDecisionTemplate {
    schema_version: u32,
    target_id: String,
    candidate_id: String,
    decision: ReviewDecision,
    reviewer: String,
    notes: String,
    corrected_source_id: Option<String>,
    accepted_block_indexes: Vec<usize>,
    reviewed_at_unix: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewDecision {
    Unreviewed,
}

#[derive(Debug, Clone, Serialize)]
struct ArtifactReceipt {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct ArtifactManifest {
    schema_version: u32,
    generated_at_unix: u64,
    files: Vec<ArtifactReceipt>,
}

#[derive(Debug)]
enum ParsedResults {
    Raw(Vec<SearchResult>),
    Composite(Vec<CompositeSearchResult>),
    Hybrid(HybridSearchReport),
    Semantic(SemanticFusionReport),
}

#[derive(Debug)]
struct CandidateEvidence {
    external_id: String,
    title: Option<String>,
    score: f32,
    relationship: String,
    textual_provenance_supported: bool,
    lexical_supported: bool,
    semantic_supported: bool,
    semantic_only: bool,
    metadata: BTreeMap<String, String>,
    blocks: Vec<BlockEvidence>,
}

#[derive(Debug)]
struct BlockEvidence {
    query_start: usize,
    query_end: usize,
    source_start: usize,
    source_end: usize,
    edit_distance: usize,
    edit_similarity: f32,
    matched_tokens: usize,
    raw_score: f32,
    expected_false_matches: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-review-report: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    let manifest = CorpusManifest::load(&command.corpus_root)?;
    let manifest_path = command.corpus_root.join(MANIFEST_FILENAME);
    let manifest_bytes = fs::read(&manifest_path)?;
    let specimen = fs::read_to_string(&command.specimen)?;
    let result_bytes = fs::read(&command.results)?;
    let parsed = parse_results(&result_bytes)?;
    let normalization_profile = load_profile(command.normalization_profile.as_deref())?;
    let target_id = command.target_id.clone().unwrap_or_else(|| {
        command
            .specimen
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("specimen")
            .to_owned()
    });
    if target_id.trim().is_empty() {
        return Err(invalid("target_id must not be empty"));
    }

    let specimen_provenance = normalize_with_provenance(&specimen, &normalization_profile);
    let candidates = candidate_evidence(parsed);
    let mut review_candidates = Vec::new();
    for (rank, mut candidate) in candidates
        .into_iter()
        .take(command.maximum_candidates)
        .enumerate()
    {
        let document = manifest.document(&candidate.external_id).ok_or_else(|| {
            invalid(format!(
                "result candidate {} is absent from corpus manifest",
                candidate.external_id
            ))
        })?;
        validate_relative_path(&document.relative_path)?;
        if document.bytes > command.maximum_source_bytes {
            return Err(invalid(format!(
                "source {} exceeds the configured byte limit",
                document.id
            )));
        }
        let source_path = command.corpus_root.join(&document.relative_path);
        let source_bytes = fs::read(&source_path)?;
        if source_bytes.len() as u64 != document.bytes
            || sha256_hex(&source_bytes) != document.sha256
        {
            return Err(invalid(format!(
                "source {} no longer matches its corpus manifest receipt",
                document.id
            )));
        }
        let source = String::from_utf8(source_bytes)
            .map_err(|error| invalid(format!("source {} is not UTF-8: {error}", document.id)))?;
        let source_provenance = normalize_with_provenance(&source, &normalization_profile);
        let mut blocks = Vec::new();
        for (block_index, block) in std::mem::take(&mut candidate.blocks)
            .into_iter()
            .enumerate()
        {
            if block.query_start >= block.query_end
                || block.source_start >= block.source_end
                || block.query_end > specimen_provenance.len()
                || block.source_end > source_provenance.len()
            {
                return Err(invalid(format!(
                    "candidate {} block {} uses out-of-range normalized coordinates",
                    candidate.external_id, block_index
                )));
            }
            let query_range = specimen_provenance
                .original_range_for_tokens(block.query_start, block.query_end)
                .ok_or_else(|| invalid("could not map query token span to original bytes"))?;
            let source_range = source_provenance
                .original_range_for_tokens(block.source_start, block.source_end)
                .ok_or_else(|| invalid("could not map source token span to original bytes"))?;
            let query_excerpt = specimen
                .get(query_range.start..query_range.end)
                .ok_or_else(|| invalid("mapped query byte range is not valid UTF-8"))?
                .to_owned();
            let source_excerpt = source
                .get(source_range.start..source_range.end)
                .ok_or_else(|| invalid("mapped source byte range is not valid UTF-8"))?
                .to_owned();
            blocks.push(ReviewBlock {
                block_index,
                query_token_start: block.query_start,
                query_token_end: block.query_end,
                source_token_start: block.source_start,
                source_token_end: block.source_end,
                query_byte_start: query_range.start,
                query_byte_end: query_range.end,
                source_byte_start: source_range.start,
                source_byte_end: source_range.end,
                edit_distance: block.edit_distance,
                edit_similarity: block.edit_similarity,
                matched_tokens: block.matched_tokens,
                raw_score: block.raw_score,
                expected_false_matches: block.expected_false_matches,
                query_excerpt,
                source_excerpt,
                query_context_html: highlighted_context(
                    &specimen,
                    query_range.start,
                    query_range.end,
                    command.context_characters,
                )?,
                source_context_html: highlighted_context(
                    &source,
                    source_range.start,
                    source_range.end,
                    command.context_characters,
                )?,
            });
        }
        review_candidates.push(review_candidate(rank + 1, candidate, document, blocks));
    }

    let report = ReviewReport {
        schema_version: REVIEW_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        title: command.title,
        corpus_id: manifest.corpus_id,
        corpus_provider: format!("{:?}", manifest.provider),
        corpus_manifest_sha256: sha256_hex(&manifest_bytes),
        target_id: target_id.clone(),
        specimen_path: command.specimen.display().to_string(),
        specimen_sha256: sha256_hex(specimen.as_bytes()),
        results_path: command.results.display().to_string(),
        results_sha256: sha256_hex(&result_bytes),
        normalization_profile,
        candidates: review_candidates,
    };

    fs::create_dir_all(&command.output)?;
    let review_path = command.output.join("review.json");
    let decisions_path = command.output.join("decisions.jsonl");
    let html_path = command.output.join("index.html");
    atomic_write(&review_path, &serde_json::to_vec_pretty(&report)?)?;
    atomic_write(&decisions_path, &decision_templates(&report, &target_id)?)?;
    atomic_write(&html_path, render_html(&report)?.as_bytes())?;
    let artifact_manifest = artifact_manifest(
        &command.output,
        &[&review_path, &decisions_path, &html_path],
    )?;
    let artifacts_path = command.output.join("artifacts.json");
    atomic_write(
        &artifacts_path,
        &serde_json::to_vec_pretty(&artifact_manifest)?,
    )?;

    if command.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "output": command.output,
                "candidates": report.candidates.len(),
                "review": review_path,
                "decisions": decisions_path,
                "html": html_path,
                "artifacts": artifacts_path,
            }))?
        );
    } else {
        println!("Review directory: {}", command.output.display());
        println!("Candidates:       {}", report.candidates.len());
        println!("Review JSON:      {}", review_path.display());
        println!("Decision template:{}", decisions_path.display());
        println!("Open in browser:  {}", html_path.display());
        println!("Artifact manifest:{}", artifacts_path.display());
    }
    Ok(())
}

fn validate_command(command: &Cli) -> CliResult<()> {
    if command.output.exists() {
        return Err(invalid(format!(
            "{} already exists; review outputs are immutable",
            command.output.display()
        )));
    }
    if command.maximum_candidates == 0
        || command.maximum_source_bytes == 0
        || command.context_characters == 0
    {
        return Err(invalid(
            "candidate, source-byte, and context limits must be positive",
        ));
    }
    if command.title.trim().is_empty() {
        return Err(invalid("title must not be empty"));
    }
    Ok(())
}

fn load_profile(path: Option<&Path>) -> CliResult<NormalizationProfile> {
    match path {
        Some(path) => Ok(serde_json::from_slice(&fs::read(path)?)?),
        None => Ok(NormalizationProfile::default()),
    }
}

fn parse_results(bytes: &[u8]) -> CliResult<ParsedResults> {
    if let Ok(report) = serde_json::from_slice::<SemanticFusionReport>(bytes) {
        return Ok(ParsedResults::Semantic(report));
    }
    if let Ok(report) = serde_json::from_slice::<HybridSearchReport>(bytes) {
        return Ok(ParsedResults::Hybrid(report));
    }
    if let Ok(results) = serde_json::from_slice::<Vec<CompositeSearchResult>>(bytes) {
        return Ok(ParsedResults::Composite(results));
    }
    if let Ok(results) = serde_json::from_slice::<Vec<SearchResult>>(bytes) {
        return Ok(ParsedResults::Raw(results));
    }
    Err(invalid(
        "results file is not a recognized raw, composite, hybrid, or semantic report",
    ))
}

fn candidate_evidence(results: ParsedResults) -> Vec<CandidateEvidence> {
    match results {
        ParsedResults::Raw(results) => results.into_iter().map(candidate_from_raw).collect(),
        ParsedResults::Composite(results) => {
            results.into_iter().map(candidate_from_composite).collect()
        }
        ParsedResults::Hybrid(report) => report
            .results
            .into_iter()
            .map(candidate_from_hybrid)
            .collect(),
        ParsedResults::Semantic(report) => report
            .results
            .into_iter()
            .map(|result| {
                let semantic_supported = !result.semantic.is_empty();
                match result.hybrid {
                    Some(hybrid) => {
                        let mut candidate = candidate_from_hybrid(hybrid);
                        candidate.score = result.score;
                        candidate.relationship = relationship_name(result.relationship).to_owned();
                        candidate.semantic_supported = semantic_supported;
                        candidate.semantic_only = result.semantic_only;
                        candidate.textual_provenance_supported =
                            result.textual_provenance_supported;
                        candidate.lexical_supported = result.lexical_supported;
                        candidate
                    }
                    None => CandidateEvidence {
                        external_id: result.external_id,
                        title: Some(result.title),
                        score: result.score,
                        relationship: relationship_name(result.relationship).to_owned(),
                        textual_provenance_supported: false,
                        lexical_supported: false,
                        semantic_supported,
                        semantic_only: true,
                        metadata: result
                            .semantic
                            .first()
                            .map_or_else(BTreeMap::new, |evidence| evidence.metadata.clone()),
                        blocks: Vec::new(),
                    },
                }
            })
            .collect(),
    }
}

fn candidate_from_raw(result: SearchResult) -> CandidateEvidence {
    CandidateEvidence {
        external_id: result.path.clone(),
        title: None,
        score: result.combined_score,
        relationship: "textual_provenance".to_owned(),
        textual_provenance_supported: true,
        lexical_supported: false,
        semantic_supported: false,
        semantic_only: false,
        metadata: BTreeMap::new(),
        blocks: vec![block_from_raw(&result)],
    }
}

fn candidate_from_composite(result: CompositeSearchResult) -> CandidateEvidence {
    CandidateEvidence {
        external_id: result.path,
        title: None,
        score: result.aggregate_score,
        relationship: if result.reordered_blocks {
            "textual_provenance_reordered"
        } else {
            "textual_provenance_fragmented"
        }
        .to_owned(),
        textual_provenance_supported: true,
        lexical_supported: false,
        semantic_supported: false,
        semantic_only: false,
        metadata: BTreeMap::new(),
        blocks: result
            .blocks
            .into_iter()
            .map(|block| BlockEvidence {
                query_start: block.query_start,
                query_end: block.query_end,
                source_start: block.corpus_start,
                source_end: block.corpus_end,
                edit_distance: block.edit_distance,
                edit_similarity: block.edit_similarity,
                matched_tokens: block.matched_tokens,
                raw_score: block.raw_score,
                expected_false_matches: block.expected_false_matches,
            })
            .collect(),
    }
}

fn candidate_from_hybrid(result: fo_core::HybridSearchResult) -> CandidateEvidence {
    let blocks = match result.overlap.as_ref() {
        Some(HybridOverlapEvidence::Passage(passage)) => vec![block_from_raw(passage)],
        Some(HybridOverlapEvidence::Composite(composite)) => composite
            .blocks
            .iter()
            .map(|block| BlockEvidence {
                query_start: block.query_start,
                query_end: block.query_end,
                source_start: block.corpus_start,
                source_end: block.corpus_end,
                edit_distance: block.edit_distance,
                edit_similarity: block.edit_similarity,
                matched_tokens: block.matched_tokens,
                raw_score: block.raw_score,
                expected_false_matches: block.expected_false_matches,
            })
            .collect(),
        None => Vec::new(),
    };
    let textual_provenance_supported = result.overlap.is_some();
    CandidateEvidence {
        external_id: result.external_id,
        title: Some(result.title),
        score: result.score,
        relationship: if textual_provenance_supported {
            "textual_provenance"
        } else {
            "lexical_only"
        }
        .to_owned(),
        textual_provenance_supported,
        lexical_supported: result.lexical.is_some(),
        semantic_supported: false,
        semantic_only: false,
        metadata: result.metadata,
        blocks,
    }
}

fn block_from_raw(result: &SearchResult) -> BlockEvidence {
    BlockEvidence {
        query_start: result.query_start,
        query_end: result.query_end,
        source_start: result.corpus_start,
        source_end: result.corpus_end,
        edit_distance: result.edit_distance,
        edit_similarity: result.edit_similarity,
        matched_tokens: result.matched_tokens,
        raw_score: result.combined_score,
        expected_false_matches: result.estimated_false_matches,
    }
}

fn relationship_name(value: SemanticRelationshipClass) -> &'static str {
    match value {
        SemanticRelationshipClass::TextualProvenance => "textual_provenance",
        SemanticRelationshipClass::TextualAndSemantic => "textual_and_semantic",
        SemanticRelationshipClass::LexicalOnly => "lexical_only",
        SemanticRelationshipClass::LexicalAndSemantic => "lexical_and_semantic",
        SemanticRelationshipClass::SemanticOnly => "semantic_only",
    }
}

fn review_candidate(
    rank: usize,
    candidate: CandidateEvidence,
    document: &CorpusDocument,
    blocks: Vec<ReviewBlock>,
) -> ReviewCandidate {
    ReviewCandidate {
        rank,
        external_id: candidate.external_id,
        title: candidate.title.unwrap_or_else(|| document.title.clone()),
        score: candidate.score,
        relationship: candidate.relationship,
        textual_provenance_supported: candidate.textual_provenance_supported,
        lexical_supported: candidate.lexical_supported,
        semantic_supported: candidate.semantic_supported,
        semantic_only: candidate.semantic_only,
        source_relative_path: document.relative_path.clone(),
        source_sha256: document.sha256.clone(),
        source_url: document.source_url.clone(),
        author_or_issuer: document.author_or_issuer.clone(),
        published_or_filed: document.published_or_filed.clone(),
        metadata: if candidate.metadata.is_empty() {
            document.metadata.clone()
        } else {
            candidate.metadata
        },
        blocks,
    }
}

fn highlighted_context(
    text: &str,
    start: usize,
    end: usize,
    context_characters: usize,
) -> CliResult<String> {
    if start >= end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err(invalid("highlight byte range is invalid"));
    }
    let context_start = move_back_characters(text, start, context_characters);
    let context_end = move_forward_characters(text, end, context_characters);
    let prefix = if context_start > 0 { "…" } else { "" };
    let suffix = if context_end < text.len() { "…" } else { "" };
    Ok(format!(
        "{}{}<mark>{}</mark>{}{}",
        prefix,
        escape_html(&text[context_start..start]),
        escape_html(&text[start..end]),
        escape_html(&text[end..context_end]),
        suffix,
    ))
}

fn move_back_characters(text: &str, index: usize, count: usize) -> usize {
    text[..index]
        .char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(0, |(offset, _)| offset)
}

fn move_forward_characters(text: &str, index: usize, count: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(count)
        .map_or(text.len(), |(offset, _)| index + offset)
}

fn decision_templates(report: &ReviewReport, target_id: &str) -> CliResult<Vec<u8>> {
    let mut output = Vec::new();
    for candidate in &report.candidates {
        serde_json::to_writer(
            &mut output,
            &ReviewDecisionTemplate {
                schema_version: REVIEW_SCHEMA_VERSION,
                target_id: target_id.to_owned(),
                candidate_id: candidate.external_id.clone(),
                decision: ReviewDecision::Unreviewed,
                reviewer: String::new(),
                notes: String::new(),
                corrected_source_id: None,
                accepted_block_indexes: Vec::new(),
                reviewed_at_unix: 0,
            },
        )?;
        output.push(b'\n');
    }
    Ok(output)
}

fn render_html(report: &ReviewReport) -> CliResult<String> {
    let data = serde_json::to_string(report)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let mut cards = String::new();
    for (index, candidate) in report.candidates.iter().enumerate() {
        cards.push_str(&format!(
            "<article class=\"candidate\"><header><div><span class=\"rank\">#{}</span> <h2>{}</h2><code>{}</code></div><div class=\"score\">{:.4}</div></header>",
            candidate.rank,
            escape_html(&candidate.title),
            escape_html(&candidate.external_id),
            candidate.score,
        ));
        cards.push_str("<div class=\"badges\">");
        cards.push_str(&badge(&candidate.relationship, "relationship"));
        cards.push_str(&badge(
            if candidate.textual_provenance_supported {
                "localized textual evidence"
            } else {
                "no localized textual evidence"
            },
            if candidate.textual_provenance_supported {
                "good"
            } else {
                "warn"
            },
        ));
        if candidate.lexical_supported {
            cards.push_str(&badge("lexical", "neutral"));
        }
        if candidate.semantic_supported {
            cards.push_str(&badge("semantic", "neutral"));
        }
        cards.push_str("</div>");
        cards.push_str(&format!(
            "<p class=\"meta\">{} · {} · <a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">source</a></p>",
            escape_html(&candidate.author_or_issuer),
            escape_html(candidate.published_or_filed.as_deref().unwrap_or("date unknown")),
            escape_attribute(&candidate.source_url),
        ));
        if candidate.blocks.is_empty() {
            cards.push_str("<div class=\"notice\">This candidate has no localized textual block. It may be lexical or semantic evidence only and must not be treated as provenance.</div>");
        }
        for block in &candidate.blocks {
            cards.push_str(&format!(
                "<section class=\"block\"><h3>Block {} <span>edit {:.3} · {} tokens · score {:.3}</span></h3><div class=\"columns\"><div><h4>Specimen</h4><pre>{}</pre></div><div><h4>Proposed source</h4><pre>{}</pre></div></div><details><summary>Coordinates and evidence</summary><dl><dt>Specimen bytes</dt><dd>{}..{}</dd><dt>Source bytes</dt><dd>{}..{}</dd><dt>Edit distance</dt><dd>{}</dd><dt>Expected false matches</dt><dd>{:.6}</dd></dl></details></section>",
                block.block_index + 1,
                block.edit_similarity,
                block.matched_tokens,
                block.raw_score,
                block.query_context_html,
                block.source_context_html,
                block.query_byte_start,
                block.query_byte_end,
                block.source_byte_start,
                block.source_byte_end,
                block.edit_distance,
                block.expected_false_matches,
            ));
        }
        cards.push_str(&format!(
            "<section class=\"decision\"><label>Decision<select data-field=\"decision\" data-index=\"{}\"><option value=\"unreviewed\">Unreviewed</option><option value=\"accept\">Accept source</option><option value=\"reject\">Reject source</option><option value=\"uncertain\">Uncertain</option><option value=\"correct_source\">Correct source ID</option></select></label><label>Corrected source ID<input data-field=\"corrected_source_id\" data-index=\"{}\" placeholder=\"optional\"></label><label>Accepted blocks<input data-field=\"accepted_blocks\" data-index=\"{}\" placeholder=\"e.g. 0,1\"></label><label>Notes<textarea data-field=\"notes\" data-index=\"{}\"></textarea></label></section></article>",
            index, index, index, index,
        ));
    }
    Ok(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style></head><body><main><section class=\"hero\"><div><p class=\"eyebrow\">FrankenOverlap review workbench</p><h1>{}</h1><p>Target <code>{}</code> · corpus <code>{}</code> · {} candidates</p></div><div class=\"controls\"><label>Reviewer<input id=\"reviewer\" placeholder=\"name or handle\"></label><button id=\"download\">Download decisions.jsonl</button></div></section><section class=\"legend\"><strong>Trust boundary:</strong> only candidates labeled “localized textual evidence” support a textual-provenance claim. Lexical or semantic-only candidates may still be relevant, but they are not evidence of descent.</section>{}</main><script>const DATA={};{}</script></body></html>",
        escape_html(&report.title),
        css(),
        escape_html(&report.title),
        escape_html(&report.target_id),
        escape_html(&report.corpus_id),
        report.candidates.len(),
        cards,
        data,
        javascript(),
    ))
}

fn css() -> &'static str {
    r#"
:root{font-family:Inter,ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color:#17202a;background:#f4f7fb;line-height:1.5}*{box-sizing:border-box}body{margin:0}main{max-width:1500px;margin:auto;padding:28px}.hero{display:flex;justify-content:space-between;gap:24px;align-items:end;background:white;border:1px solid #dfe7f1;border-radius:18px;padding:28px;box-shadow:0 8px 30px rgba(22,34,51,.06)}h1{margin:.15rem 0;font-size:clamp(2rem,5vw,4.2rem);line-height:1}.eyebrow{text-transform:uppercase;letter-spacing:.12em;font-weight:750;color:#486581}.controls{display:grid;gap:12px;min-width:280px}input,select,textarea,button{font:inherit;border:1px solid #b9c7d8;border-radius:9px;padding:10px;background:white}button{background:#0b63ce;color:white;border:0;font-weight:750;cursor:pointer}.legend,.notice{margin:20px 0;padding:16px 18px;border-radius:12px;background:#fff7d6;border:1px solid #ead17a}.candidate{margin:22px 0;background:white;border:1px solid #dfe7f1;border-radius:18px;padding:24px;box-shadow:0 8px 24px rgba(22,34,51,.05)}.candidate>header{display:flex;justify-content:space-between;gap:20px;align-items:start}.candidate h2{display:inline;margin:0 .4rem}.rank{font-weight:800;color:#486581}.score{font:800 1.6rem ui-monospace,SFMono-Regular,Menlo,monospace}.badges{display:flex;gap:8px;flex-wrap:wrap;margin:14px 0}.badge{display:inline-block;border-radius:999px;padding:4px 9px;font-size:.82rem;font-weight:750;background:#edf2f7}.badge.good{background:#dff5e7;color:#146c36}.badge.warn{background:#ffe7d6;color:#993c0d}.badge.relationship{background:#e5edff;color:#244a9b}.meta{color:#52667a}.block{border-top:1px solid #e5ebf2;padding-top:16px;margin-top:18px}.block h3 span{font-size:.85rem;font-weight:500;color:#66788a}.columns{display:grid;grid-template-columns:1fr 1fr;gap:16px}.columns>div{min-width:0}pre{white-space:pre-wrap;word-break:break-word;background:#f7f9fc;border:1px solid #e1e8f0;border-radius:12px;padding:16px;max-height:360px;overflow:auto;font:500 .94rem/1.65 ui-monospace,SFMono-Regular,Menlo,monospace}mark{background:#ffe56d;padding:.08em .02em;border-radius:2px}dl{display:grid;grid-template-columns:max-content 1fr;gap:6px 14px}dt{font-weight:750}.decision{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin-top:20px;padding:16px;border-radius:12px;background:#f2f7ff}.decision label,.controls label{display:grid;gap:5px;font-weight:700}.decision label:last-child{grid-column:1/-1}textarea{min-height:80px}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}@media(max-width:850px){.hero,.candidate>header{display:block}.controls{margin-top:18px}.columns,.decision{grid-template-columns:1fr}.decision label:last-child{grid-column:auto}}
"#
}

fn javascript() -> &'static str {
    r#"
const state=DATA.candidates.map(candidate=>({schema_version:1,target_id:DATA.target_id,candidate_id:candidate.external_id,decision:"unreviewed",reviewer:"",notes:"",corrected_source_id:null,accepted_block_indexes:[],reviewed_at_unix:0}));
document.querySelectorAll("[data-field]").forEach(element=>element.addEventListener("input",event=>{const index=Number(event.target.dataset.index);const field=event.target.dataset.field;const value=event.target.value;if(field==="accepted_blocks"){state[index].accepted_block_indexes=value.split(",").map(v=>Number(v.trim())).filter(Number.isInteger)}else if(field==="corrected_source_id"){state[index].corrected_source_id=value.trim()||null}else{state[index][field]=value}}));
document.getElementById("download").addEventListener("click",()=>{const reviewer=document.getElementById("reviewer").value.trim();const now=Math.floor(Date.now()/1000);const lines=state.map(item=>JSON.stringify({...item,reviewer,reviewed_at_unix:item.decision==="unreviewed"?0:now})).join("\n")+"\n";const url=URL.createObjectURL(new Blob([lines],{type:"application/x-ndjson"}));const link=document.createElement("a");link.href=url;link.download="decisions.jsonl";link.click();URL.revokeObjectURL(url)});
"#
}

fn badge(text: &str, class_name: &str) -> String {
    format!(
        "<span class=\"badge {}\">{}</span>",
        escape_attribute(class_name),
        escape_html(text)
    )
}

fn artifact_manifest(root: &Path, files: &[&Path]) -> CliResult<ArtifactManifest> {
    let mut receipts = Vec::new();
    for path in files {
        let bytes = fs::read(path)?;
        receipts.push(ArtifactReceipt {
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/"),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    receipts.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(ArtifactManifest {
        schema_version: REVIEW_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        files: receipts,
    })
}

fn validate_relative_path(value: &str) -> CliResult<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid(format!("unsafe corpus path {value:?}")));
    }
    Ok(())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attribute(value: &str) -> String {
    escape_html(value)
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
