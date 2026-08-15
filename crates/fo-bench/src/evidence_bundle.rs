use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use fo_core::{NormalizationProfile, normalize};
use fo_corpus::{CorpusManifest, atomic_write, sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize};

pub type BundleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const EVIDENCE_BUNDLE_SCHEMA_VERSION: u32 = 1;

const RESULTS_MARKDOWN: &str = "RESULTS.md";
const RESULTS_HTML: &str = "RESULTS.html";
const ENVIRONMENT_JSON: &str = "environment.json";
const EXAMPLES_JSON: &str = "examples.json";
const MANIFEST_JSON: &str = "artifacts.json";
const METHOD_HYBRID: &str = "franken_hybrid";
const BASELINE_METHODS: &[&str] = &[
    "normalized_exact_substring",
    "character_qgram_jaccard",
    "character_qgram_simhash",
    "fielded_bm25_phrase_proximity",
];

#[derive(Debug, Clone)]
pub struct BundleOptions {
    pub examples_per_profile: usize,
    pub top_candidates_per_method: usize,
    pub snippet_tokens: usize,
    pub title: String,
}

impl Default for BundleOptions {
    fn default() -> Self {
        Self {
            examples_per_profile: 3,
            top_candidates_per_method: 5,
            snippet_tokens: 180,
            title: "FrankenOverlap evidence report".to_owned(),
        }
    }
}

impl BundleOptions {
    pub fn validate(&self) -> BundleResult<()> {
        if self.examples_per_profile == 0
            || self.top_candidates_per_method == 0
            || self.snippet_tokens == 0
            || self.title.trim().is_empty()
        {
            return Err(invalid(
                "example, candidate, snippet, and title settings must be nonempty and positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProofReport {
    schema_version: u32,
    corpus_id: String,
    corpus_provider: String,
    corpus_manifest_sha256: String,
    query_file_sha256: String,
    available_documents: usize,
    required_positive_documents: usize,
    queries: usize,
    profiles: Vec<String>,
    evaluated_corpus_sizes: Vec<usize>,
    warmup_runs: usize,
    measurement_repetitions: usize,
    seed: u64,
    scales: Vec<ScaleReport>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScaleReport {
    corpus_size: usize,
    build: BuildReport,
    methods: Vec<MethodReport>,
    exhaustive: ExhaustiveReport,
    break_even: Vec<BreakEven>,
}

#[derive(Debug, Clone, Deserialize)]
struct BuildReport {
    build_ms: f64,
    serialization_ms: f64,
    cold_load_ms: f64,
    index_bytes: u64,
    source_bytes: u64,
    overlap_fingerprints: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
    peak_rss_kib: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct MethodReport {
    name: String,
    complete_quality_queries: usize,
    evaluated_pairs: usize,
    nonzero_scores: usize,
    quality: Option<QualityReport>,
    profiles: Vec<ProfileReport>,
    timing: TimingReport,
    span: Option<SpanReport>,
}

#[derive(Debug, Clone, Deserialize)]
struct QualityReport {
    micro: MicroReport,
    macro_average_precision: f64,
    mean_reciprocal_rank: f64,
    recall_at_k: Vec<AtK>,
    ndcg_at_k: Vec<AtK>,
}

#[derive(Debug, Clone, Deserialize)]
struct MicroReport {
    average_precision: f64,
    best_f1: f64,
    best_threshold: f64,
    brier_score: f64,
    expected_calibration_error: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct AtK {
    k: usize,
    value: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileReport {
    profile: String,
    queries: usize,
    quality: QualityReport,
}

#[derive(Debug, Clone, Deserialize)]
struct TimingReport {
    first_execution_samples: usize,
    repeat_samples: usize,
    first_p50_ms: f64,
    first_p95_ms: f64,
    repeat_p50_ms: f64,
    repeat_p95_ms: f64,
    repeat_p99_ms: f64,
    measured_operations: usize,
    measured_elapsed_ms: f64,
    operations_per_second: f64,
    one_shot_total_ms: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct SpanReport {
    eligible_queries: usize,
    predicted_queries: usize,
    mean_iou: f64,
    median_iou: f64,
    mean_expected_coverage: f64,
    mean_predicted_coverage: f64,
    mean_start_absolute_error: f64,
    mean_end_absolute_error: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ExhaustiveReport {
    complete: bool,
    complete_queries: usize,
    partial_queries: usize,
    evaluated_pairs: usize,
    skipped_pairs: usize,
    cells: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct BreakEven {
    baseline_method: String,
    indexed_method: String,
    baseline_p95_ms: Option<f64>,
    indexed_p95_ms: f64,
    index_build_serialization_load_ms: f64,
    saved_ms_per_query_at_p95: Option<f64>,
    break_even_queries: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScenarioQuery {
    id: String,
    profile: String,
    text: String,
    positive_ids: Vec<String>,
    source_id: String,
    #[serde(default)]
    source_title: String,
    #[serde(default)]
    relation_key: String,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
struct TokenSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ScoreRow {
    corpus_size: usize,
    query_id: String,
    profile: String,
    query_text: String,
    source_id: String,
    source_title: String,
    positive_ids: Vec<String>,
    candidate_id: String,
    candidate_title: String,
    label: bool,
    scores: BTreeMap<String, f64>,
    expected_source_span: Option<TokenSpan>,
    #[serde(default)]
    predicted_spans: BTreeMap<String, Vec<TokenSpan>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClaimReport {
    all_supported: bool,
    nominal_confidence_level: f64,
    familywise_confidence_level: f64,
    comparisons: Vec<ClaimComparison>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClaimComparison {
    id: String,
    verdict: String,
    baseline_method: String,
    challenger_method: String,
    eligible_queries: usize,
    excluded_incomplete_queries: usize,
    baseline: MetricSnapshot,
    challenger: MetricSnapshot,
    delta: MetricDelta,
    bootstrap: BootstrapIntervals,
    worst_profile: Option<String>,
    worst_profile_macro_delta: Option<f64>,
    baseline_p95_ms: f64,
    challenger_p95_ms: f64,
    p95_ratio: Option<f64>,
    failures: Vec<String>,
    uncertainties: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MetricSnapshot {
    micro_auprc: f64,
    macro_auprc: f64,
    mean_reciprocal_rank: f64,
    recall_at_1: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct MetricDelta {
    micro_auprc: f64,
    macro_auprc: f64,
    mean_reciprocal_rank: f64,
    recall_at_1: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct BootstrapIntervals {
    micro_auprc: DeltaInterval,
    macro_auprc: DeltaInterval,
    recall_at_1: DeltaInterval,
}

#[derive(Debug, Clone, Deserialize)]
struct DeltaInterval {
    lower: f64,
    median: f64,
    upper: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentReport {
    pub generated_at_unix: u64,
    pub command: Vec<String>,
    pub repository_commit: Option<String>,
    pub repository_dirty: Option<bool>,
    pub rustc_verbose: Option<String>,
    pub cargo_version: Option<String>,
    pub operating_system: String,
    pub architecture: String,
    pub cpu_model: Option<String>,
    pub logical_cores: usize,
    pub memory_bytes: Option<u64>,
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RankInterval {
    pub best_rank: usize,
    pub worst_rank: usize,
    pub score: f64,
}

impl RankInterval {
    fn midpoint(&self) -> f64 {
        (self.best_rank + self.worst_rank) as f64 / 2.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateExample {
    pub candidate_id: String,
    pub candidate_title: String,
    pub score: f64,
    pub label: bool,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodExample {
    pub method: String,
    pub positive_rank: Option<RankInterval>,
    pub top_candidates: Vec<CandidateExample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryExample {
    pub query_id: String,
    pub profile: String,
    pub selection_reason: String,
    pub query_text: String,
    pub source_id: String,
    pub source_title: String,
    pub positive_ids: Vec<String>,
    pub relation_key: String,
    pub source_snippet: String,
    pub methods: Vec<MethodExample>,
    pub hybrid_rank_advantage_over_best_baseline: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExampleReport {
    pub corpus_size: usize,
    pub profiles: Vec<String>,
    pub examples: Vec<QueryExample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputReceipt {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactReceipt {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub claim_status: String,
    pub inputs: Vec<InputReceipt>,
    pub artifacts: Vec<ArtifactReceipt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleReport {
    pub output_directory: String,
    pub results_markdown: String,
    pub results_html: String,
    pub environment_json: String,
    pub examples_json: String,
    pub artifacts_json: String,
    pub claim_status: String,
    pub examples: usize,
}

pub fn render_evidence_bundle(
    corpus_root: &Path,
    query_path: &Path,
    proof_report_path: &Path,
    score_path: &Path,
    claim_report_path: Option<&Path>,
    gold_validation_path: Option<&Path>,
    output_directory: &Path,
    options: &BundleOptions,
) -> BundleResult<BundleReport> {
    options.validate()?;
    if output_directory.exists() {
        return Err(invalid(format!(
            "{} already exists; evidence bundles are immutable",
            output_directory.display()
        )));
    }
    fs::create_dir_all(output_directory)?;
    let manifest = CorpusManifest::load(corpus_root)?;
    let proof_bytes = fs::read(proof_report_path)?;
    let query_bytes = fs::read(query_path)?;
    let score_bytes = fs::read(score_path)?;
    let proof = serde_json::from_slice::<ProofReport>(&proof_bytes)?;
    if proof.corpus_id != manifest.corpus_id {
        return Err(invalid(format!(
            "proof corpus {} does not match manifest corpus {}",
            proof.corpus_id, manifest.corpus_id
        )));
    }
    let queries = read_jsonl::<ScenarioQuery>(&query_bytes)?;
    let rows = read_jsonl::<ScoreRow>(&score_bytes)?;
    let claim = claim_report_path
        .map(fs::read)
        .transpose()?
        .map(|bytes| serde_json::from_slice::<ClaimReport>(&bytes))
        .transpose()?;
    let claim_status = claim_status(claim.as_ref());
    let largest_scale = proof
        .scales
        .iter()
        .max_by_key(|scale| scale.corpus_size)
        .ok_or_else(|| invalid("proof report contains no corpus scales"))?;
    let examples = build_examples(
        corpus_root,
        &manifest,
        &queries,
        &rows,
        largest_scale,
        options,
    )?;
    let environment = environment_report();
    let markdown = render_markdown(&options.title, &proof, claim.as_ref(), &examples);
    let html = render_html(&options.title, &proof, claim.as_ref(), &examples);

    let markdown_path = output_directory.join(RESULTS_MARKDOWN);
    let html_path = output_directory.join(RESULTS_HTML);
    let environment_path = output_directory.join(ENVIRONMENT_JSON);
    let examples_path = output_directory.join(EXAMPLES_JSON);
    atomic_write(&markdown_path, markdown.as_bytes())?;
    atomic_write(&html_path, html.as_bytes())?;
    atomic_write(&environment_path, &serde_json::to_vec_pretty(&environment)?)?;
    atomic_write(&examples_path, &serde_json::to_vec_pretty(&examples)?)?;

    let mut inputs = vec![
        receipt("corpus_manifest", &corpus_root.join(fo_corpus::MANIFEST_FILENAME))?,
        receipt("queries", query_path)?,
        receipt("proof_report", proof_report_path)?,
        receipt("pair_scores", score_path)?,
    ];
    if let Some(path) = claim_report_path {
        inputs.push(receipt("claim_report", path)?);
    }
    if let Some(path) = gold_validation_path {
        inputs.push(receipt("gold_validation", path)?);
    }
    inputs.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let artifacts = vec![
        artifact(RESULTS_MARKDOWN, &markdown_path)?,
        artifact(RESULTS_HTML, &html_path)?,
        artifact(ENVIRONMENT_JSON, &environment_path)?,
        artifact(EXAMPLES_JSON, &examples_path)?,
    ];
    let bundle_manifest = BundleManifest {
        schema_version: EVIDENCE_BUNDLE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: proof.corpus_id.clone(),
        claim_status: claim_status.clone(),
        inputs,
        artifacts,
    };
    let manifest_path = output_directory.join(MANIFEST_JSON);
    atomic_write(&manifest_path, &serde_json::to_vec_pretty(&bundle_manifest)?)?;
    Ok(BundleReport {
        output_directory: output_directory.display().to_string(),
        results_markdown: markdown_path.display().to_string(),
        results_html: html_path.display().to_string(),
        environment_json: environment_path.display().to_string(),
        examples_json: examples_path.display().to_string(),
        artifacts_json: manifest_path.display().to_string(),
        claim_status,
        examples: examples.examples.len(),
    })
}

fn build_examples(
    corpus_root: &Path,
    manifest: &CorpusManifest,
    queries: &[ScenarioQuery],
    rows: &[ScoreRow],
    scale: &ScaleReport,
    options: &BundleOptions,
) -> BundleResult<ExampleReport> {
    let rows = rows
        .iter()
        .filter(|row| row.corpus_size == scale.corpus_size)
        .collect::<Vec<_>>();
    let grouped_rows = group_rows(&rows)?;
    let query_map = queries
        .iter()
        .map(|query| (query.id.as_str(), query))
        .collect::<BTreeMap<_, _>>();
    let mut by_profile = BTreeMap::<String, Vec<QuerySelection>>::new();
    for (query_id, query_rows) in &grouped_rows {
        let query = query_map.get(query_id.as_str()).ok_or_else(|| {
            invalid(format!("score rows reference missing query {query_id:?}"))
        })?;
        let hybrid_rank = positive_rank(query_rows, METHOD_HYBRID, &query.positive_ids);
        let baseline_rank = BASELINE_METHODS
            .iter()
            .filter_map(|method| positive_rank(query_rows, method, &query.positive_ids))
            .min_by(|left, right| left.midpoint().total_cmp(&right.midpoint()));
        let advantage = hybrid_rank
            .as_ref()
            .zip(baseline_rank.as_ref())
            .map(|(hybrid, baseline)| baseline.midpoint() - hybrid.midpoint());
        by_profile
            .entry(query.profile.clone())
            .or_default()
            .push(QuerySelection {
                query_id: query.id.clone(),
                advantage,
            });
    }
    let mut selected = Vec::<(String, String)>::new();
    for (profile, mut selections) in by_profile {
        selections.sort_unstable_by(|left, right| left.query_id.cmp(&right.query_id));
        if let Some(first) = selections.first() {
            selected.push((first.query_id.clone(), "deterministic_first".to_owned()));
        }
        if options.examples_per_profile >= 2 {
            if let Some(best) = selections.iter().max_by(|left, right| {
                left.advantage
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&right.advantage.unwrap_or(f64::NEG_INFINITY))
                    .then_with(|| right.query_id.cmp(&left.query_id))
            }) {
                selected.push((best.query_id.clone(), "largest_hybrid_rank_gain".to_owned()));
            }
        }
        if options.examples_per_profile >= 3 {
            if let Some(worst) = selections.iter().min_by(|left, right| {
                left.advantage
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&right.advantage.unwrap_or(f64::INFINITY))
                    .then_with(|| left.query_id.cmp(&right.query_id))
            }) {
                selected.push((worst.query_id.clone(), "largest_hybrid_rank_loss".to_owned()));
            }
        }
        selected.sort_unstable();
        selected.dedup();
        let _ = profile;
    }
    selected.sort_unstable();
    selected.dedup();

    let document_map = manifest
        .documents
        .iter()
        .map(|document| (document.id.as_str(), document))
        .collect::<BTreeMap<_, _>>();
    let method_names = scale
        .methods
        .iter()
        .map(|method| method.name.clone())
        .collect::<Vec<_>>();
    let mut examples = Vec::new();
    for (query_id, reason) in selected {
        let query = query_map[query_id.as_str()];
        let query_rows = grouped_rows[query_id.as_str()].as_slice();
        let source_document = document_map.get(query.source_id.as_str()).ok_or_else(|| {
            invalid(format!("source {} is absent from corpus manifest", query.source_id))
        })?;
        let source_row = query_rows
            .iter()
            .find(|row| row.candidate_id == query.source_id)
            .copied();
        let source_snippet = document_snippet(
            corpus_root,
            source_document,
            source_row.and_then(|row| row.expected_source_span),
            options.snippet_tokens,
        )?;
        let mut methods = Vec::new();
        for method in &method_names {
            let mut ranked = query_rows.to_vec();
            ranked.sort_unstable_by(|left, right| {
                score_for(right, method)
                    .total_cmp(&score_for(left, method))
                    .then_with(|| left.candidate_id.cmp(&right.candidate_id))
            });
            let positive_rank = positive_rank(query_rows, method, &query.positive_ids);
            let mut top_candidates = Vec::new();
            for row in ranked.into_iter().take(options.top_candidates_per_method) {
                let document = document_map.get(row.candidate_id.as_str()).ok_or_else(|| {
                    invalid(format!(
                        "candidate {} is absent from corpus manifest",
                        row.candidate_id
                    ))
                })?;
                let span = row
                    .predicted_spans
                    .get(method)
                    .and_then(|spans| spans.first())
                    .copied();
                let snippet = span
                    .map(|span| {
                        document_snippet(
                            corpus_root,
                            document,
                            Some(span),
                            options.snippet_tokens,
                        )
                    })
                    .transpose()?;
                top_candidates.push(CandidateExample {
                    candidate_id: row.candidate_id.clone(),
                    candidate_title: row.candidate_title.clone(),
                    score: score_for(row, method),
                    label: row.label,
                    snippet,
                });
            }
            methods.push(MethodExample {
                method: method.clone(),
                positive_rank,
                top_candidates,
            });
        }
        let hybrid_rank = positive_rank(query_rows, METHOD_HYBRID, &query.positive_ids);
        let baseline_rank = BASELINE_METHODS
            .iter()
            .filter_map(|method| positive_rank(query_rows, method, &query.positive_ids))
            .min_by(|left, right| left.midpoint().total_cmp(&right.midpoint()));
        examples.push(QueryExample {
            query_id: query.id.clone(),
            profile: query.profile.clone(),
            selection_reason: reason,
            query_text: query.text.clone(),
            source_id: query.source_id.clone(),
            source_title: if query.source_title.is_empty() {
                source_document.title.clone()
            } else {
                query.source_title.clone()
            },
            positive_ids: query.positive_ids.clone(),
            relation_key: query.relation_key.clone(),
            source_snippet,
            methods,
            hybrid_rank_advantage_over_best_baseline: hybrid_rank
                .as_ref()
                .zip(baseline_rank.as_ref())
                .map(|(hybrid, baseline)| baseline.midpoint() - hybrid.midpoint()),
        });
    }
    examples.sort_unstable_by(|left, right| {
        left.profile
            .cmp(&right.profile)
            .then_with(|| left.query_id.cmp(&right.query_id))
            .then_with(|| left.selection_reason.cmp(&right.selection_reason))
    });
    let profiles = examples
        .iter()
        .map(|example| example.profile.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ExampleReport {
        corpus_size: scale.corpus_size,
        profiles,
        examples,
    })
}

#[derive(Debug)]
struct QuerySelection {
    query_id: String,
    advantage: Option<f64>,
}

fn positive_rank(
    rows: &[&ScoreRow],
    method: &str,
    positive_ids: &[String],
) -> Option<RankInterval> {
    let positive_set = positive_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut ranked = rows.to_vec();
    ranked.sort_unstable_by(|left, right| {
        score_for(right, method)
            .total_cmp(&score_for(left, method))
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    let best_positive = ranked
        .iter()
        .filter(|row| positive_set.contains(row.candidate_id.as_str()))
        .map(|row| score_for(row, method))
        .max_by(f64::total_cmp)?;
    let better = ranked
        .iter()
        .filter(|row| score_for(row, method) > best_positive)
        .count();
    let tied = ranked
        .iter()
        .filter(|row| score_for(row, method).total_cmp(&best_positive).is_eq())
        .count();
    Some(RankInterval {
        best_rank: better + 1,
        worst_rank: better + tied,
        score: best_positive,
    })
}

fn document_snippet(
    corpus_root: &Path,
    document: &fo_corpus::CorpusDocument,
    span: Option<TokenSpan>,
    snippet_tokens: usize,
) -> BundleResult<String> {
    validate_relative_path(&document.relative_path)?;
    let body = fs::read_to_string(corpus_root.join(&document.relative_path))?;
    let normalized = normalize(&body, &NormalizationProfile::default());
    let text = match span {
        Some(span) if span.start < span.end => {
            let context = snippet_tokens / 2;
            normalized
                .slice_tokens(
                    span.start.saturating_sub(context),
                    span.end.saturating_add(context).min(normalized.len()),
                )
                .to_owned()
        }
        _ => normalized
            .slice_tokens(0, snippet_tokens.min(normalized.len()))
            .to_owned(),
    };
    Ok(one_line(&text, 1_400))
}

fn render_markdown(
    title: &str,
    proof: &ProofReport,
    claims: Option<&ClaimReport>,
    examples: &ExampleReport,
) -> String {
    let mut output = String::new();
    output.push_str(&format!("# {title}\n\n"));
    output.push_str(&format!(
        "**Corpus:** `{}`  \n**Provider:** `{}`  \n**Queries:** {}  \n**Largest measured corpus:** {} documents  \n**Claim status:** **{}**\n\n",
        proof.corpus_id,
        proof.corpus_provider,
        proof.queries,
        examples.corpus_size,
        claim_status(claims)
    ));
    output.push_str(
        "> This report is generated from machine-readable receipts. A method is not described as superior unless its preregistered claim gate is supported. Exact substring and BM25 are expected to win workloads suited to them.\n\n",
    );
    output.push_str("## Experimental contract\n\n");
    output.push_str(&format!(
        "- Proof schema: `{}`\n- Corpus manifest SHA-256: `{}`\n- Query SHA-256: `{}`\n- Available documents: {}\n- Required positive documents: {}\n- Profiles: {}\n- Evaluated sizes: {:?}\n- Warmups excluded: {}\n- Repeat measurements: {}\n- Seed: `{}`\n\n",
        proof.schema_version,
        proof.corpus_manifest_sha256,
        proof.query_file_sha256,
        proof.available_documents,
        proof.required_positive_documents,
        proof.profiles.join(", "),
        proof.evaluated_corpus_sizes,
        proof.warmup_runs,
        proof.measurement_repetitions,
        proof.seed
    ));
    output.push_str("## Quality and latency\n\n");
    for scale in &proof.scales {
        output.push_str(&format!("### {} documents\n\n", scale.corpus_size));
        output.push_str(&format!(
            "Build {:.3} ms; serialize {:.3} ms; cold load {:.3} ms; index {} bytes; source {} bytes; peak RSS {}.\n\n",
            scale.build.build_ms,
            scale.build.serialization_ms,
            scale.build.cold_load_ms,
            scale.build.index_bytes,
            scale.build.source_bytes,
            scale
                .build
                .peak_rss_kib
                .map_or_else(|| "not measured".to_owned(), |value| format!("{value} KiB"))
        ));
        output.push_str("| Method | Micro AUPRC | Macro AUPRC | Recall@1 | MRR | Repeat p95 | QPS | Span IoU |\n");
        output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
        for method in &scale.methods {
            let quality = method.quality.as_ref();
            output.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {:.3} ms | {:.2} | {} |\n",
                method.name,
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.micro.average_precision)),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.macro_average_precision)),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", metric_at(&value.recall_at_k, 1))),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.mean_reciprocal_rank)),
                method.timing.repeat_p95_ms,
                method.timing.operations_per_second,
                method.span.as_ref().map_or_else(|| "n/a".to_owned(), |span| format!("{:.4}", span.mean_iou))
            ));
        }
        output.push('\n');
        output.push_str(&format!(
            "Exhaustive Levenshtein complete: **{}**; complete queries {}; partial queries {}; evaluated pairs {}; skipped pairs {}; DP cells {}.\n\n",
            scale.exhaustive.complete,
            scale.exhaustive.complete_queries,
            scale.exhaustive.partial_queries,
            scale.exhaustive.evaluated_pairs,
            scale.exhaustive.skipped_pairs,
            scale.exhaustive.cells
        ));
        if !scale.break_even.is_empty() {
            output.push_str("Break-even comparisons:\n\n");
            for comparison in &scale.break_even {
                output.push_str(&format!(
                    "- `{}` → `{}`: baseline p95 {}; indexed p95 {:.3} ms; setup {:.3} ms; break-even {}.\n",
                    comparison.baseline_method,
                    comparison.indexed_method,
                    comparison.baseline_p95_ms.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.3} ms")),
                    comparison.indexed_p95_ms,
                    comparison.index_build_serialization_load_ms,
                    comparison.break_even_queries.map_or_else(|| "not established".to_owned(), |value| format!("{value:.2} queries"))
                ));
            }
            output.push('\n');
        }
    }
    output.push_str("## Predeclared claim verdicts\n\n");
    match claims {
        None => output.push_str("No claim report was supplied. No superiority claim is established.\n\n"),
        Some(claims) => {
            output.push_str(&format!(
                "Nominal confidence {:.4}; family-wise confidence {:.4}; all comparisons supported: **{}**.\n\n",
                claims.nominal_confidence_level,
                claims.familywise_confidence_level,
                claims.all_supported
            ));
            for comparison in &claims.comparisons {
                output.push_str(&format!(
                    "### `{}` — **{}**\n\n`{}` versus `{}` over {} paired queries ({} incomplete excluded).\n\n",
                    comparison.id,
                    comparison.verdict,
                    comparison.challenger_method,
                    comparison.baseline_method,
                    comparison.eligible_queries,
                    comparison.excluded_incomplete_queries
                ));
                output.push_str(&format!(
                    "- Micro AUPRC: {:.4} → {:.4} ({:+.4}; family-wise lower {:+.4})\n- Macro AUPRC: {:.4} → {:.4} ({:+.4}; family-wise lower {:+.4})\n- Recall@1: {:.4} → {:.4} ({:+.4}; family-wise lower {:+.4})\n- MRR delta: {:+.4}\n- Repeat p95: {:.3} ms → {:.3} ms; ratio {}\n",
                    comparison.baseline.micro_auprc,
                    comparison.challenger.micro_auprc,
                    comparison.delta.micro_auprc,
                    comparison.bootstrap.micro_auprc.lower,
                    comparison.baseline.macro_auprc,
                    comparison.challenger.macro_auprc,
                    comparison.delta.macro_auprc,
                    comparison.bootstrap.macro_auprc.lower,
                    comparison.baseline.recall_at_1,
                    comparison.challenger.recall_at_1,
                    comparison.delta.recall_at_1,
                    comparison.bootstrap.recall_at_1.lower,
                    comparison.delta.mean_reciprocal_rank,
                    comparison.baseline_p95_ms,
                    comparison.challenger_p95_ms,
                    comparison.p95_ratio.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.3}"))
                ));
                if let Some(profile) = &comparison.worst_profile {
                    output.push_str(&format!(
                        "- Worst profile: `{profile}` macro delta {:+.4}\n",
                        comparison.worst_profile_macro_delta.unwrap_or(0.0)
                    ));
                }
                for failure in &comparison.failures {
                    output.push_str(&format!("- **Failure:** {failure}\n"));
                }
                for uncertainty in &comparison.uncertainties {
                    output.push_str(&format!("- **Inconclusive:** {uncertainty}\n"));
                }
                output.push('\n');
            }
        }
    }
    output.push_str("## Representative real-world cases\n\n");
    output.push_str(
        "For each profile the renderer selects the lexicographically first query, the largest hybrid rank gain, and the largest hybrid rank loss when distinct. Losses are shown deliberately. Rank intervals represent score ties.\n\n",
    );
    for example in &examples.examples {
        output.push_str(&format!(
            "### `{}` — {}\n\n**Selection:** `{}`  \n**Source:** `{}` — {}  \n**Acceptable positives:** {}  \n**Hybrid rank advantage over best baseline:** {}\n\n",
            example.profile,
            example.query_id,
            example.selection_reason,
            example.source_id,
            example.source_title,
            example.positive_ids.join(", "),
            example.hybrid_rank_advantage_over_best_baseline.map_or_else(|| "n/a".to_owned(), |value| format!("{value:+.2}"))
        ));
        output.push_str("**Query**\n\n");
        output.push_str(&format!("> {}\n\n", markdown_quote(&example.query_text)));
        output.push_str("**Source passage**\n\n");
        output.push_str(&format!("> {}\n\n", markdown_quote(&example.source_snippet)));
        output.push_str("| Method | Positive rank interval | Top candidate | Score |\n");
        output.push_str("|---|---:|---|---:|\n");
        for method in &example.methods {
            let leader = method.top_candidates.first();
            output.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                method.method,
                method.positive_rank.as_ref().map_or_else(|| "n/a".to_owned(), |rank| format!("{}–{}", rank.best_rank, rank.worst_rank)),
                leader.map_or_else(|| "n/a".to_owned(), |candidate| format!("`{}` {}", candidate.candidate_id, candidate.candidate_title)),
                leader.map_or_else(|| "n/a".to_owned(), |candidate| format!("{:.4}", candidate.score))
            ));
        }
        output.push('\n');
    }
    output.push_str("## Interpretation boundary\n\n");
    output.push_str(
        "This bundle establishes only the comparisons marked **supported** by the supplied claim report. `inconclusive` is not evidence of no effect, and `unsupported` is evidence that the preregistered statement failed. Exact substring and BM25 results remain first-class controls rather than straw men.\n",
    );
    output
}

fn render_html(
    title: &str,
    proof: &ProofReport,
    claims: Option<&ClaimReport>,
    examples: &ExampleReport,
) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<header><h1>{}</h1><p class=lead>Corpus <code>{}</code> · {} queries · largest scale {} documents</p><p class=status>Claim status: <strong>{}</strong></p></header>",
        html_escape(title),
        html_escape(&proof.corpus_id),
        proof.queries,
        examples.corpus_size,
        html_escape(&claim_status(claims))
    ));
    body.push_str("<section><h2>Quality and latency</h2>");
    for scale in &proof.scales {
        body.push_str(&format!("<h3>{} documents</h3>", scale.corpus_size));
        body.push_str(&format!(
            "<p>Build {:.3} ms · serialize {:.3} ms · cold load {:.3} ms · index {} bytes · source {} bytes</p>",
            scale.build.build_ms,
            scale.build.serialization_ms,
            scale.build.cold_load_ms,
            scale.build.index_bytes,
            scale.build.source_bytes
        ));
        body.push_str("<div class=tablewrap><table><thead><tr><th>Method</th><th>Micro AUPRC</th><th>Macro AUPRC</th><th>Recall@1</th><th>MRR</th><th>p95</th><th>QPS</th><th>Span IoU</th></tr></thead><tbody>");
        for method in &scale.methods {
            let quality = method.quality.as_ref();
            body.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.3} ms</td><td>{:.2}</td><td>{}</td></tr>",
                html_escape(&method.name),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.micro.average_precision)),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.macro_average_precision)),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", metric_at(&value.recall_at_k, 1))),
                quality.map_or_else(|| "n/a".to_owned(), |value| format!("{:.4}", value.mean_reciprocal_rank)),
                method.timing.repeat_p95_ms,
                method.timing.operations_per_second,
                method.span.as_ref().map_or_else(|| "n/a".to_owned(), |span| format!("{:.4}", span.mean_iou))
            ));
        }
        body.push_str("</tbody></table></div>");
        body.push_str(&format!(
            "<p>Exhaustive Levenshtein complete: <strong>{}</strong>; {} complete queries, {} partial queries, {} DP cells.</p>",
            scale.exhaustive.complete,
            scale.exhaustive.complete_queries,
            scale.exhaustive.partial_queries,
            scale.exhaustive.cells
        ));
    }
    body.push_str("</section><section><h2>Predeclared claim verdicts</h2>");
    match claims {
        None => body.push_str("<p>No claim report was supplied. No superiority claim is established.</p>"),
        Some(claims) => {
            body.push_str(&format!(
                "<p>Nominal confidence {:.4}; family-wise confidence {:.4}; all supported: <strong>{}</strong>.</p>",
                claims.nominal_confidence_level,
                claims.familywise_confidence_level,
                claims.all_supported
            ));
            for comparison in &claims.comparisons {
                let class = match comparison.verdict.as_str() {
                    "supported" => "supported",
                    "unsupported" => "unsupported",
                    _ => "inconclusive",
                };
                body.push_str(&format!(
                    "<article class=claim><h3><span class=badge-{class}>{}</span> {}</h3><p><code>{}</code> versus <code>{}</code> over {} paired queries.</p><ul><li>Micro AUPRC {:.4} → {:.4}, delta {:+.4}, lower {:+.4}</li><li>Macro AUPRC {:.4} → {:.4}, delta {:+.4}, lower {:+.4}</li><li>Recall@1 {:.4} → {:.4}, delta {:+.4}, lower {:+.4}</li><li>p95 {:.3} ms → {:.3} ms</li></ul>",
                    html_escape(&comparison.verdict),
                    html_escape(&comparison.id),
                    html_escape(&comparison.challenger_method),
                    html_escape(&comparison.baseline_method),
                    comparison.eligible_queries,
                    comparison.baseline.micro_auprc,
                    comparison.challenger.micro_auprc,
                    comparison.delta.micro_auprc,
                    comparison.bootstrap.micro_auprc.lower,
                    comparison.baseline.macro_auprc,
                    comparison.challenger.macro_auprc,
                    comparison.delta.macro_auprc,
                    comparison.bootstrap.macro_auprc.lower,
                    comparison.baseline.recall_at_1,
                    comparison.challenger.recall_at_1,
                    comparison.delta.recall_at_1,
                    comparison.bootstrap.recall_at_1.lower,
                    comparison.baseline_p95_ms,
                    comparison.challenger_p95_ms
                ));
                for failure in &comparison.failures {
                    body.push_str(&format!(
                        "<p class=failure><strong>Failure:</strong> {}</p>",
                        html_escape(failure)
                    ));
                }
                for uncertainty in &comparison.uncertainties {
                    body.push_str(&format!(
                        "<p class=uncertainty><strong>Inconclusive:</strong> {}</p>",
                        html_escape(uncertainty)
                    ));
                }
                body.push_str("</article>");
            }
        }
    }
    body.push_str("</section><section><h2>Representative real-world cases</h2><p>Selection is deterministic and includes both the largest hybrid gain and largest hybrid loss per profile when distinct.</p>");
    for example in &examples.examples {
        body.push_str(&format!(
            "<article class=example><h3>{} · {}</h3><p><strong>Selection:</strong> <code>{}</code><br><strong>Source:</strong> <code>{}</code> {}<br><strong>Acceptable positives:</strong> {}<br><strong>Hybrid rank advantage:</strong> {}</p><details><summary>Query</summary><blockquote>{}</blockquote></details><details><summary>Source passage</summary><blockquote>{}</blockquote></details>",
            html_escape(&example.profile),
            html_escape(&example.query_id),
            html_escape(&example.selection_reason),
            html_escape(&example.source_id),
            html_escape(&example.source_title),
            html_escape(&example.positive_ids.join(", ")),
            example.hybrid_rank_advantage_over_best_baseline.map_or_else(|| "n/a".to_owned(), |value| format!("{value:+.2}")),
            html_escape(&example.query_text),
            html_escape(&example.source_snippet)
        ));
        body.push_str("<div class=tablewrap><table><thead><tr><th>Method</th><th>Positive rank</th><th>Top candidate</th><th>Score</th></tr></thead><tbody>");
        for method in &example.methods {
            let leader = method.top_candidates.first();
            body.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                html_escape(&method.method),
                method.positive_rank.as_ref().map_or_else(|| "n/a".to_owned(), |rank| format!("{}–{}", rank.best_rank, rank.worst_rank)),
                leader.map_or_else(|| "n/a".to_owned(), |candidate| format!("<code>{}</code> {}", html_escape(&candidate.candidate_id), html_escape(&candidate.candidate_title))),
                leader.map_or_else(|| "n/a".to_owned(), |candidate| format!("{:.4}", candidate.score))
            ));
        }
        body.push_str("</tbody></table></div></article>");
    }
    body.push_str("</section><footer>This report establishes only comparisons marked supported by the supplied preregistered claim gate.</footer>");
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style></head><body><main>{}</main></body></html>",
        html_escape(title),
        html_styles(),
        body
    )
}

fn group_rows<'a>(rows: &[&'a ScoreRow]) -> BundleResult<BTreeMap<String, Vec<&'a ScoreRow>>> {
    let mut grouped = BTreeMap::<String, Vec<&ScoreRow>>::new();
    let mut keys = BTreeSet::new();
    for row in rows {
        if row.query_id.trim().is_empty()
            || row.profile.trim().is_empty()
            || row.candidate_id.trim().is_empty()
            || row.scores.values().any(|score| !score.is_finite())
            || !keys.insert((row.query_id.clone(), row.candidate_id.clone()))
        {
            return Err(invalid(
                "score rows contain empty fields, non-finite scores, or duplicate query/candidate pairs",
            ));
        }
        grouped.entry(row.query_id.clone()).or_default().push(*row);
    }
    Ok(grouped)
}

fn score_for(row: &ScoreRow, method: &str) -> f64 {
    row.scores.get(method).copied().unwrap_or(0.0)
}

fn metric_at(metrics: &[AtK], k: usize) -> f64 {
    metrics
        .iter()
        .find(|metric| metric.k == k)
        .map_or(0.0, |metric| metric.value)
}

fn claim_status(claims: Option<&ClaimReport>) -> String {
    match claims {
        None => "not_evaluated".to_owned(),
        Some(claims) if claims.all_supported => "supported".to_owned(),
        Some(claims)
            if claims
                .comparisons
                .iter()
                .any(|comparison| comparison.verdict == "unsupported") =>
        {
            "unsupported".to_owned()
        }
        Some(_) => "inconclusive".to_owned(),
    }
}

fn environment_report() -> EnvironmentReport {
    EnvironmentReport {
        generated_at_unix: unix_timestamp(),
        command: std::env::args().collect(),
        repository_commit: command_output("git", &["rev-parse", "HEAD"]),
        repository_dirty: Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| !output.stdout.is_empty()),
        rustc_verbose: command_output("rustc", &["-Vv"]),
        cargo_version: command_output("cargo", &["-V"]),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        cpu_model: cpu_model(),
        logical_cores: std::thread::available_parallelism().map_or(1, usize::from),
        memory_bytes: memory_bytes(),
        hostname: command_output("hostname", &[]),
    }
}

fn cpu_model() -> Option<String> {
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if let Some(model) = cpuinfo.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| *name == "model name")
                .map(|(_, value)| value.trim().to_owned())
        }) {
            return Some(model);
        }
    }
    command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
}

fn memory_bytes() -> Option<u64> {
    if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
        if let Some(kib) = meminfo.lines().find_map(|line| {
            line.strip_prefix("MemTotal:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
        }) {
            return kib.checked_mul(1024);
        }
    }
    command_output("sysctl", &["-n", "hw.memsize"])
        .and_then(|value| value.parse::<u64>().ok())
}

fn command_output(command: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(command).args(arguments).output().ok()?;
    output.status.success().then_some(())?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn receipt(name: &str, path: &Path) -> BundleResult<InputReceipt> {
    let bytes = fs::read(path)?;
    Ok(InputReceipt {
        name: name.to_owned(),
        path: path.display().to_string(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    })
}

fn artifact(name: &str, path: &Path) -> BundleResult<ArtifactReceipt> {
    let bytes = fs::read(path)?;
    Ok(ArtifactReceipt {
        name: name.to_owned(),
        path: path.display().to_string(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    })
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> BundleResult<Vec<T>> {
    let input = std::str::from_utf8(bytes)?;
    input
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let value = line.trim();
            (!value.is_empty() && !value.starts_with('#')).then_some((index, value))
        })
        .map(|(index, value)| {
            serde_json::from_str(value)
                .map_err(|error| invalid(format!("JSONL row {}: {error}", index + 1)))
        })
        .collect()
}

fn one_line(value: &str, maximum_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_chars {
        return compact;
    }
    let mut output = compact.chars().take(maximum_chars).collect::<String>();
    output.push('…');
    output
}

fn markdown_quote(value: &str) -> String {
    one_line(value, 1_800).replace('\n', " ")
}

fn html_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

fn html_styles() -> &'static str {
    "body{margin:0;background:#f6f4ef;color:#171717;font:16px/1.55 system-ui,-apple-system,sans-serif}main{max-width:1180px;margin:auto;padding:36px 24px 80px}header{background:white;border:1px solid #ddd6ca;border-radius:18px;padding:28px;margin-bottom:28px}h1{font-size:2.4rem;margin:.1em 0}.lead{font-size:1.1rem;color:#555}.status{font-size:1.2rem}section{background:white;border:1px solid #ddd6ca;border-radius:18px;padding:24px;margin:22px 0}.tablewrap{overflow-x:auto}table{border-collapse:collapse;width:100%;margin:14px 0}th,td{border-bottom:1px solid #e7e1d8;padding:9px;text-align:left;vertical-align:top}th{background:#faf8f4}code{background:#f2eee7;border-radius:5px;padding:2px 5px}.claim,.example{border-top:1px solid #e7e1d8;padding-top:16px;margin-top:18px}.badge-supported{background:#d8f5df;color:#155d27;padding:4px 8px;border-radius:999px}.badge-inconclusive{background:#fff0c2;color:#6b5000;padding:4px 8px;border-radius:999px}.badge-unsupported{background:#ffd8d8;color:#7c1717;padding:4px 8px;border-radius:999px}.failure{color:#8d1616}.uncertainty{color:#775800}blockquote{background:#faf8f4;border-left:4px solid #c6b79f;margin:12px 0;padding:12px 16px}details{margin:10px 0}footer{color:#666;margin-top:28px}"
}

fn validate_relative_path(value: &str) -> BundleResult<()> {
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

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
