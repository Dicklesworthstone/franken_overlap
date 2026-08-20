#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use clap::Parser;
use fo_core::{
    CompositeMatchBlock, CompositeSearchResult, FeedbackExample, GroupedFeedbackExample,
    HybridOverlapEvidence, HybridSearchReport, LineageEvidence, LineageGraph, LineageNode,
    LineageRelation, ReviewDecisionKind, ReviewDecisionRecord, SearchResult, SemanticFusionReport,
    validate_review_decisions,
};
use fo_corpus::{sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const APPLICATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-review-apply",
    version,
    about = "Apply human review decisions to durable ranking feedback and textual lineage"
)]
struct Cli {
    /// Original result JSON reviewed by fo-review-report.
    results: PathBuf,
    /// Completed decisions.jsonl downloaded from or based on fo-review-report.
    decisions: PathBuf,
    /// Append/merge query-grouped feedback for pairwise and AP-delta ranking.
    #[arg(long)]
    feedback_output: Option<PathBuf>,
    /// Append/merge ungrouped feedback for probability calibration.
    #[arg(long)]
    calibration_output: Option<PathBuf>,
    /// Create or update a durable lineage graph with accepted localized evidence.
    #[arg(long)]
    lineage: Option<PathBuf>,
    /// Append/merge an auditable application ledger.
    #[arg(long)]
    decision_ledger: Option<PathBuf>,
    #[arg(long)]
    target_title: Option<String>,
    #[arg(long)]
    target_observed_at_unix: Option<u64>,
    #[arg(long = "target-metadata")]
    target_metadata: Vec<String>,
    #[arg(long, default_value_t = 0.30)]
    minimum_lineage_score: f32,
    #[arg(long, default_value_t = 0.15)]
    minimum_lineage_query_coverage: f32,
    #[arg(long, default_value_t = 24)]
    minimum_lineage_matched_tokens: usize,
    /// Validate and report what would change without writing outputs.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug)]
struct CandidateEvidence {
    title: String,
    metadata: BTreeMap<String, String>,
    results: Vec<SearchResult>,
}

#[derive(Debug)]
enum ParsedResults {
    Raw(Vec<SearchResult>),
    Composite(Vec<CompositeSearchResult>),
    Hybrid(HybridSearchReport),
    Semantic(SemanticFusionReport),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppliedDecision {
    schema_version: u32,
    application_id: String,
    applied_at_unix: u64,
    results_sha256: String,
    decisions_sha256: String,
    decision: ReviewDecisionRecord,
    localized_result_records: usize,
    ranking_feedback_records: usize,
    calibration_feedback_records: usize,
    lineage_edges_changed: usize,
    corrected_source_recorded: bool,
}

#[derive(Debug, Serialize)]
struct ApplicationReport {
    schema_version: u32,
    applied_at_unix: u64,
    target_id: String,
    results_sha256: String,
    decisions_sha256: String,
    decisions: usize,
    accepted: usize,
    rejected: usize,
    uncertain: usize,
    unreviewed: usize,
    corrected_source: usize,
    accepted_without_localized_evidence: usize,
    ranking_feedback_records_added: usize,
    calibration_feedback_records_added: usize,
    lineage_nodes_changed: usize,
    lineage_edges_changed: usize,
    decision_ledger_records_added: usize,
    dry_run: bool,
    lineage_summary: Option<fo_core::LineageSummary>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-review-apply: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    let results_bytes = fs::read(&command.results)?;
    let decisions_bytes = fs::read(&command.decisions)?;
    let results_sha256 = sha256_hex(&results_bytes);
    let decisions_sha256 = sha256_hex(&decisions_bytes);
    let decisions = read_jsonl::<ReviewDecisionRecord>(&command.decisions)?;
    validate_review_decisions(&decisions)?;
    let target_ids = decisions
        .iter()
        .map(|decision| decision.target_id.as_str())
        .collect::<BTreeSet<_>>();
    if target_ids.len() != 1 {
        return Err(invalid(
            "one review application must contain exactly one target_id",
        ));
    }
    let target_id = (*target_ids.first().expect("one target id")).to_owned();
    let evidence = candidate_evidence(parse_results(&results_bytes)?);
    for decision in &decisions {
        if !evidence.contains_key(&decision.candidate_id) {
            return Err(invalid(format!(
                "decision references candidate {} absent from the reviewed results",
                decision.candidate_id
            )));
        }
    }

    let target_metadata = parse_metadata(&command.target_metadata)?;
    let applied_at_unix = unix_timestamp();
    let mut ranking_feedback = Vec::new();
    let mut calibration_feedback = Vec::new();
    let mut ledger = Vec::new();
    let mut graph = match command.lineage.as_deref() {
        Some(path) if path.exists() => {
            let graph = serde_json::from_slice::<LineageGraph>(&fs::read(path)?)?;
            graph.validate()?;
            Some(graph)
        }
        Some(_) => Some(LineageGraph::new()),
        None => None,
    };
    let mut lineage_nodes_changed = 0usize;
    if let Some(graph) = graph.as_mut() {
        lineage_nodes_changed += usize::from(
            graph.upsert_node(LineageNode {
                id: target_id.clone(),
                title: command
                    .target_title
                    .clone()
                    .unwrap_or_else(|| target_id.clone()),
                observed_at_unix: command.target_observed_at_unix,
                metadata: target_metadata,
            })?,
        );
    }

    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut uncertain = 0usize;
    let mut unreviewed = 0usize;
    let mut corrected_source = 0usize;
    let mut accepted_without_localized_evidence = 0usize;
    let mut lineage_edges_changed = 0usize;

    for decision in decisions {
        let candidate = &evidence[&decision.candidate_id];
        let selected_results = select_results(candidate, &decision)?;
        let label = match decision.decision {
            ReviewDecisionKind::Accept => {
                accepted += 1;
                Some(true)
            }
            ReviewDecisionKind::Reject => {
                rejected += 1;
                Some(false)
            }
            ReviewDecisionKind::CorrectSource => {
                corrected_source += 1;
                Some(false)
            }
            ReviewDecisionKind::Uncertain => {
                uncertain += 1;
                None
            }
            ReviewDecisionKind::Unreviewed => {
                unreviewed += 1;
                None
            }
        };
        let feedback_weight = if selected_results.is_empty() {
            1.0
        } else {
            1.0 / selected_results.len() as f64
        };
        let ranking_before = ranking_feedback.len();
        let calibration_before = calibration_feedback.len();
        if let Some(label) = label {
            for result in &selected_results {
                ranking_feedback.push(GroupedFeedbackExample {
                    query_id: target_id.clone(),
                    result: result.clone(),
                    label,
                    weight: feedback_weight,
                });
                calibration_feedback.push(FeedbackExample {
                    result: result.clone(),
                    label,
                    weight: feedback_weight,
                });
            }
        }

        let mut changed_for_decision = 0usize;
        if decision.decision == ReviewDecisionKind::Accept {
            if selected_results.is_empty() {
                accepted_without_localized_evidence += 1;
            }
            if let Some(graph) = graph.as_mut() {
                lineage_nodes_changed += usize::from(graph.upsert_node(LineageNode {
                    id: decision.candidate_id.clone(),
                    title: if candidate.title.trim().is_empty() {
                        decision.candidate_id.clone()
                    } else {
                        candidate.title.clone()
                    },
                    observed_at_unix: metadata_timestamp(&candidate.metadata),
                    metadata: candidate.metadata.clone(),
                })?);
                for result in selected_results.iter().filter(|result| {
                    result.combined_score >= command.minimum_lineage_score
                        && result.query_coverage >= command.minimum_lineage_query_coverage
                        && result.matched_tokens >= command.minimum_lineage_matched_tokens
                }) {
                    let mut lineage_evidence =
                        LineageEvidence::from_search_result(result, decision.reviewed_at_unix);
                    lineage_evidence
                        .metadata
                        .insert("reviewer".to_owned(), decision.reviewer.clone());
                    lineage_evidence
                        .metadata
                        .insert("review_notes".to_owned(), decision.notes.clone());
                    lineage_evidence
                        .metadata
                        .insert("review_decision".to_owned(), "accept".to_owned());
                    changed_for_decision += usize::from(graph.add_evidence(
                        &decision.candidate_id,
                        &target_id,
                        LineageRelation::DerivedFrom,
                        lineage_evidence,
                    )?);
                }
            }
        }
        lineage_edges_changed += changed_for_decision;
        let application_id = application_id(&results_sha256, &decisions_sha256, &decision);
        ledger.push(AppliedDecision {
            schema_version: APPLICATION_SCHEMA_VERSION,
            application_id,
            applied_at_unix,
            results_sha256: results_sha256.clone(),
            decisions_sha256: decisions_sha256.clone(),
            corrected_source_recorded: decision.decision == ReviewDecisionKind::CorrectSource,
            localized_result_records: selected_results.len(),
            ranking_feedback_records: ranking_feedback.len() - ranking_before,
            calibration_feedback_records: calibration_feedback.len() - calibration_before,
            lineage_edges_changed: changed_for_decision,
            decision,
        });
    }

    if let Some(graph) = graph.as_ref() {
        graph.validate()?;
    }
    let ranking_added = merge_feedback(
        command.feedback_output.as_deref(),
        &ranking_feedback,
        command.dry_run,
    )?;
    let calibration_added = merge_calibration(
        command.calibration_output.as_deref(),
        &calibration_feedback,
        command.dry_run,
    )?;
    let ledger_added = merge_ledger(command.decision_ledger.as_deref(), &ledger, command.dry_run)?;
    if !command.dry_run {
        if let (Some(path), Some(graph)) = (command.lineage.as_deref(), graph.as_ref()) {
            atomic_write(path, &serde_json::to_vec_pretty(graph)?)?;
        }
    }

    let report = ApplicationReport {
        schema_version: APPLICATION_SCHEMA_VERSION,
        applied_at_unix,
        target_id,
        results_sha256,
        decisions_sha256,
        decisions: ledger.len(),
        accepted,
        rejected,
        uncertain,
        unreviewed,
        corrected_source,
        accepted_without_localized_evidence,
        ranking_feedback_records_added: ranking_added,
        calibration_feedback_records_added: calibration_added,
        lineage_nodes_changed,
        lineage_edges_changed,
        decision_ledger_records_added: ledger_added,
        dry_run: command.dry_run,
        lineage_summary: graph.as_ref().map(LineageGraph::summary),
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Target:                       {}", report.target_id);
        println!("Decisions:                    {}", report.decisions);
        println!(
            "Accept / reject:              {} / {}",
            report.accepted, report.rejected
        );
        println!(
            "Uncertain / unreviewed:       {} / {}",
            report.uncertain, report.unreviewed
        );
        println!("Corrected source:             {}", report.corrected_source);
        println!(
            "Accepts without text evidence:{}",
            report.accepted_without_localized_evidence
        );
        println!(
            "Ranking feedback added:        {}",
            report.ranking_feedback_records_added
        );
        println!(
            "Calibration feedback added:    {}",
            report.calibration_feedback_records_added
        );
        println!(
            "Lineage nodes / edges changed:{} / {}",
            lineage_nodes_changed, lineage_edges_changed
        );
        println!(
            "Decision ledger added:        {}",
            report.decision_ledger_records_added
        );
        println!("Dry run:                      {}", report.dry_run);
    }
    Ok(())
}

fn validate_command(command: &Cli) -> CliResult<()> {
    if command.feedback_output.is_none()
        && command.calibration_output.is_none()
        && command.lineage.is_none()
        && command.decision_ledger.is_none()
    {
        return Err(invalid(
            "at least one feedback, calibration, lineage, or decision-ledger output is required",
        ));
    }
    for (name, value) in [
        ("minimum_lineage_score", command.minimum_lineage_score),
        (
            "minimum_lineage_query_coverage",
            command.minimum_lineage_query_coverage,
        ),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid(format!("{name} must lie in [0, 1]")));
        }
    }
    if command.minimum_lineage_matched_tokens == 0 {
        return Err(invalid("minimum_lineage_matched_tokens must be positive"));
    }
    Ok(())
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

fn candidate_evidence(results: ParsedResults) -> BTreeMap<String, CandidateEvidence> {
    let mut output = BTreeMap::new();
    match results {
        ParsedResults::Raw(results) => {
            for result in results {
                output.insert(
                    result.path.clone(),
                    CandidateEvidence {
                        title: result.path.clone(),
                        metadata: BTreeMap::new(),
                        results: vec![result],
                    },
                );
            }
        }
        ParsedResults::Composite(results) => {
            for result in results {
                output.insert(
                    result.path.clone(),
                    CandidateEvidence {
                        title: result.path.clone(),
                        metadata: BTreeMap::new(),
                        results: result
                            .blocks
                            .iter()
                            .map(|block| block_result(&result, block))
                            .collect(),
                    },
                );
            }
        }
        ParsedResults::Hybrid(report) => {
            for result in report.results {
                let localized = match result.overlap {
                    Some(HybridOverlapEvidence::Passage(passage)) => vec![passage],
                    Some(HybridOverlapEvidence::Composite(composite)) => composite
                        .blocks
                        .iter()
                        .map(|block| block_result(&composite, block))
                        .collect(),
                    None => Vec::new(),
                };
                output.insert(
                    result.external_id,
                    CandidateEvidence {
                        title: result.title,
                        metadata: result.metadata,
                        results: localized,
                    },
                );
            }
        }
        ParsedResults::Semantic(report) => {
            for result in report.results {
                let (title, metadata, localized) = match result.hybrid {
                    Some(hybrid) => {
                        let localized = match hybrid.overlap {
                            Some(HybridOverlapEvidence::Passage(passage)) => vec![passage],
                            Some(HybridOverlapEvidence::Composite(composite)) => composite
                                .blocks
                                .iter()
                                .map(|block| block_result(&composite, block))
                                .collect(),
                            None => Vec::new(),
                        };
                        (hybrid.title, hybrid.metadata, localized)
                    }
                    None => (
                        result.title,
                        result
                            .semantic
                            .first()
                            .map_or_else(BTreeMap::new, |value| value.metadata.clone()),
                        Vec::new(),
                    ),
                };
                output.insert(
                    result.external_id,
                    CandidateEvidence {
                        title,
                        metadata,
                        results: localized,
                    },
                );
            }
        }
    }
    output
}

fn block_result(composite: &CompositeSearchResult, block: &CompositeMatchBlock) -> SearchResult {
    let block_fraction = block.matched_tokens as f32 / composite.matched_tokens.max(1) as f32;
    SearchResult {
        document_id: composite.document_id,
        path: composite.path.clone(),
        intent: composite.intent,
        corpus_start: block.corpus_start,
        corpus_end: block.corpus_end,
        query_start: block.query_start,
        query_end: block.query_end,
        edit_distance: block.edit_distance,
        edit_similarity: block.edit_similarity,
        anchor_coverage: 0.0,
        query_coverage: (composite.query_coverage * block_fraction).clamp(0.0, 1.0),
        source_coverage: (composite.source_coverage * block_fraction).clamp(0.0, 1.0),
        anchor_score: block.raw_score,
        vote_support: 0.0,
        chain_consistency: 1.0,
        matched_tokens: block.matched_tokens,
        distinct_anchor_count: 1,
        estimated_false_matches: block.expected_false_matches,
        combined_score: block.raw_score,
        matched_text: block.matched_text.clone(),
    }
}

fn select_results(
    candidate: &CandidateEvidence,
    decision: &ReviewDecisionRecord,
) -> CliResult<Vec<SearchResult>> {
    if decision.accepted_block_indexes.is_empty() {
        return Ok(candidate.results.clone());
    }
    let mut output = Vec::new();
    for &index in &decision.accepted_block_indexes {
        output.push(candidate.results.get(index).cloned().ok_or_else(|| {
            invalid(format!(
                "candidate {} has {} localized blocks, but decision selected block {}",
                decision.candidate_id,
                candidate.results.len(),
                index
            ))
        })?);
    }
    Ok(output)
}

fn merge_feedback(
    path: Option<&Path>,
    new_values: &[GroupedFeedbackExample],
    dry_run: bool,
) -> CliResult<usize> {
    let Some(path) = path else {
        return Ok(0);
    };
    let mut values = if path.exists() {
        read_jsonl::<GroupedFeedbackExample>(path)?
    } else {
        Vec::new()
    };
    let before = values.len();
    values.extend_from_slice(new_values);
    values.sort_unstable_by(feedback_order);
    values.dedup_by(feedback_same);
    let added = values.len().saturating_sub(before);
    if !dry_run {
        atomic_write(path, &jsonl_bytes(&values)?)?;
    }
    Ok(added)
}

fn merge_calibration(
    path: Option<&Path>,
    new_values: &[FeedbackExample],
    dry_run: bool,
) -> CliResult<usize> {
    let Some(path) = path else {
        return Ok(0);
    };
    let mut values = if path.exists() {
        read_jsonl::<FeedbackExample>(path)?
    } else {
        Vec::new()
    };
    let before = values.len();
    values.extend_from_slice(new_values);
    values.sort_unstable_by(|left, right| {
        left.result
            .path
            .cmp(&right.result.path)
            .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
            .then_with(|| left.result.query_start.cmp(&right.result.query_start))
            .then_with(|| left.label.cmp(&right.label))
    });
    values.dedup_by(|left, right| {
        left.result.path == right.result.path
            && left.result.corpus_start == right.result.corpus_start
            && left.result.corpus_end == right.result.corpus_end
            && left.result.query_start == right.result.query_start
            && left.result.query_end == right.result.query_end
            && left.label == right.label
    });
    let added = values.len().saturating_sub(before);
    if !dry_run {
        atomic_write(path, &jsonl_bytes(&values)?)?;
    }
    Ok(added)
}

fn merge_ledger(
    path: Option<&Path>,
    new_values: &[AppliedDecision],
    dry_run: bool,
) -> CliResult<usize> {
    let Some(path) = path else {
        return Ok(0);
    };
    let mut values = if path.exists() {
        read_jsonl::<AppliedDecision>(path)?
    } else {
        Vec::new()
    };
    let before = values.len();
    values.extend_from_slice(new_values);
    values.sort_unstable_by(|left, right| left.application_id.cmp(&right.application_id));
    values.dedup_by(|left, right| left.application_id == right.application_id);
    let added = values.len().saturating_sub(before);
    if !dry_run {
        atomic_write(path, &jsonl_bytes(&values)?)?;
    }
    Ok(added)
}

fn feedback_order(
    left: &GroupedFeedbackExample,
    right: &GroupedFeedbackExample,
) -> std::cmp::Ordering {
    left.query_id
        .cmp(&right.query_id)
        .then_with(|| left.result.path.cmp(&right.result.path))
        .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
        .then_with(|| left.result.query_start.cmp(&right.result.query_start))
        .then_with(|| left.label.cmp(&right.label))
}

fn feedback_same(left: &mut GroupedFeedbackExample, right: &mut GroupedFeedbackExample) -> bool {
    left.query_id == right.query_id
        && left.result.path == right.result.path
        && left.result.corpus_start == right.result.corpus_start
        && left.result.corpus_end == right.result.corpus_end
        && left.result.query_start == right.result.query_start
        && left.result.query_end == right.result.query_end
        && left.label == right.label
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> CliResult<Vec<T>> {
    let file = fs::File::open(path)?;
    let mut output = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        output.push(
            serde_json::from_str(value).map_err(|error| {
                invalid(format!("{}:{}: {error}", path.display(), line_index + 1))
            })?,
        );
    }
    Ok(output)
}

fn jsonl_bytes<T: Serialize>(values: &[T]) -> CliResult<Vec<u8>> {
    let mut output = Vec::new();
    for value in values {
        serde_json::to_writer(&mut output, value)?;
        output.push(b'\n');
    }
    Ok(output)
}

fn parse_metadata(values: &[String]) -> CliResult<BTreeMap<String, String>> {
    let mut metadata = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            return Err(invalid(format!("metadata must use KEY=VALUE: {value:?}")));
        };
        if key.trim().is_empty() {
            return Err(invalid("metadata keys must not be empty"));
        }
        metadata.insert(key.trim().to_owned(), value.to_owned());
    }
    Ok(metadata)
}

fn metadata_timestamp(metadata: &BTreeMap<String, String>) -> Option<u64> {
    [
        "observed_at_unix",
        "filed_at_unix",
        "published_at_unix",
        "timestamp",
    ]
    .into_iter()
    .find_map(|key| metadata.get(key).and_then(|value| value.parse().ok()))
}

fn application_id(
    results_sha256: &str,
    decisions_sha256: &str,
    decision: &ReviewDecisionRecord,
) -> String {
    let value = format!(
        "{results_sha256}\0{decisions_sha256}\0{}\0{}\0{:?}",
        decision.target_id, decision.candidate_id, decision.decision
    );
    format!("review-{:016x}", stable_hash(&value))
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
