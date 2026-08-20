#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

use clap::Parser;
use fo_core::{
    DomainQueryAnalysis, DomainSearchOptions, DomainSearchStatus, IndexBuilder, IndexConfig,
    LineageEvidence, LineageGraph, LineageNode, LineageRelation, SearchIntent, SearchOptions,
    SearchResult, TextDomain,
};
use fo_corpus::{
    CorpusDocument, CorpusManifest, CorpusProvider, MANIFEST_FILENAME, atomic_write, sha256_hex,
    unix_timestamp,
};
use rayon::prelude::*;
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-sec-lineage",
    version,
    about = "Analyze SEC filing-item histories, emit change alerts, and build a textual lineage graph"
)]
struct Cli {
    /// Sectioned SEC fo-corpus root produced by fo-section --strategy sec10k.
    corpus_root: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    /// Analyze every filing after the first rather than only the latest item in each history.
    #[arg(long)]
    all_targets: bool,
    /// Include sections that have no earlier same-issuer item.
    #[arg(long)]
    include_no_history: bool,
    /// Search earlier same-item sections from other issuers for language migration.
    #[arg(long)]
    include_peer_search: bool,
    /// Repeat to retain only section titles containing one of these case-insensitive fragments.
    #[arg(long = "section")]
    section_filters: Vec<String>,
    #[arg(long, default_value_t = 500)]
    maximum_targets: usize,
    #[arg(long, default_value_t = 8)]
    maximum_prior_filings: usize,
    #[arg(long, default_value_t = 250)]
    maximum_peer_candidates: usize,
    #[arg(long, default_value_t = 3)]
    maximum_sources_per_target: usize,
    #[arg(long, default_value_t = 20)]
    review_candidates: usize,
    #[arg(long, default_value_t = 1_000)]
    minimum_section_characters: usize,
    #[arg(long, default_value_t = 4)]
    threads: usize,
    #[arg(long, default_value_t = 0.30)]
    minimum_edge_score: f32,
    #[arg(long, default_value_t = 0.15)]
    minimum_edge_query_coverage: f32,
    #[arg(long, default_value_t = 32)]
    minimum_edge_matched_tokens: usize,
    #[arg(long, default_value_t = 0.20)]
    new_language_coverage: f32,
    #[arg(long, default_value_t = 0.55)]
    material_change_coverage: f32,
    #[arg(long, default_value_t = 0.72)]
    material_change_similarity: f32,
    #[arg(long, default_value_t = 0.80)]
    high_reuse_coverage: f32,
    #[arg(long, default_value_t = 0.88)]
    high_reuse_similarity: f32,
    #[arg(long, default_value_t = 0.08)]
    legacy_language_margin: f32,
    #[arg(long, default_value_t = 0.55)]
    peer_migration_score: f32,
    #[arg(long, default_value_t = 0.35)]
    peer_migration_coverage: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AnalysisOptions {
    all_targets: bool,
    include_no_history: bool,
    include_peer_search: bool,
    section_filters: Vec<String>,
    maximum_targets: usize,
    maximum_prior_filings: usize,
    maximum_peer_candidates: usize,
    maximum_sources_per_target: usize,
    review_candidates: usize,
    minimum_section_characters: usize,
    threads: usize,
    minimum_edge_score: f32,
    minimum_edge_query_coverage: f32,
    minimum_edge_matched_tokens: usize,
    new_language_coverage: f32,
    material_change_coverage: f32,
    material_change_similarity: f32,
    high_reuse_coverage: f32,
    high_reuse_similarity: f32,
    legacy_language_margin: f32,
    peer_migration_score: f32,
    peer_migration_coverage: f32,
}

impl From<&Cli> for AnalysisOptions {
    fn from(value: &Cli) -> Self {
        Self {
            all_targets: value.all_targets,
            include_no_history: value.include_no_history,
            include_peer_search: value.include_peer_search,
            section_filters: value
                .section_filters
                .iter()
                .map(|filter| filter.to_ascii_lowercase())
                .collect(),
            maximum_targets: value.maximum_targets,
            maximum_prior_filings: value.maximum_prior_filings,
            maximum_peer_candidates: value.maximum_peer_candidates,
            maximum_sources_per_target: value.maximum_sources_per_target,
            review_candidates: value.review_candidates,
            minimum_section_characters: value.minimum_section_characters,
            threads: value.threads,
            minimum_edge_score: value.minimum_edge_score,
            minimum_edge_query_coverage: value.minimum_edge_query_coverage,
            minimum_edge_matched_tokens: value.minimum_edge_matched_tokens,
            new_language_coverage: value.new_language_coverage,
            material_change_coverage: value.material_change_coverage,
            material_change_similarity: value.material_change_similarity,
            high_reuse_coverage: value.high_reuse_coverage,
            high_reuse_similarity: value.high_reuse_similarity,
            legacy_language_margin: value.legacy_language_margin,
            peer_migration_score: value.peer_migration_score,
            peer_migration_coverage: value.peer_migration_coverage,
        }
    }
}

#[derive(Debug, Clone)]
struct SecSection {
    record: CorpusDocument,
    body: String,
    cik: String,
    section_title: String,
    section_key: String,
    filing_date: String,
    observed_at_unix: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum AlertKind {
    NoHistory,
    InsufficientDistinctiveEvidence,
    NewLanguage,
    MaterialRevision,
    ModerateRevision,
    HighReuse,
    LegacyLanguageReintroduced,
    PeerLanguageMigration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum AlertSeverity {
    Info,
    Review,
    High,
}

#[derive(Debug, Clone, Serialize)]
struct SourceMatch {
    source_id: String,
    source_title: String,
    source_cik: String,
    source_filing_date: String,
    same_issuer: bool,
    score: f32,
    edit_similarity: f32,
    query_coverage: f32,
    source_coverage: f32,
    matched_tokens: usize,
    expected_false_matches: f64,
    corpus_start: usize,
    corpus_end: usize,
    query_start: usize,
    query_end: usize,
}

#[derive(Debug, Clone, Serialize)]
struct SecAlert {
    id: String,
    kind: AlertKind,
    severity: AlertSeverity,
    target_id: String,
    target_title: String,
    target_cik: String,
    target_filing_date: String,
    section_title: String,
    source: Option<SourceMatch>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct TargetAnalysis {
    target_id: String,
    target_title: String,
    target_cik: String,
    target_filing_date: String,
    section_title: String,
    section_key: String,
    prior_documents: usize,
    peer_documents: usize,
    same_issuer_status: Option<DomainSearchStatus>,
    peer_status: Option<DomainSearchStatus>,
    same_issuer_analysis: Option<DomainQueryAnalysis>,
    peer_analysis: Option<DomainQueryAnalysis>,
    best_previous: Option<SourceMatch>,
    best_peer: Option<SourceMatch>,
    accepted_sources: Vec<SourceMatch>,
    alerts: Vec<SecAlert>,
    results_file: String,
    specimen_relative_path: String,
    review_command: String,
}

#[derive(Debug, Clone)]
struct EdgeDraft {
    source_id: String,
    target_id: String,
    relation: LineageRelation,
    result: SearchResult,
    source_kind: &'static str,
}

#[derive(Debug)]
struct TargetOutcome {
    analysis: TargetAnalysis,
    review_results: Vec<SearchResult>,
    edges: Vec<EdgeDraft>,
}

#[derive(Debug, Serialize)]
struct SecLineageReport {
    schema_version: u32,
    generated_at_unix: u64,
    corpus_id: String,
    corpus_manifest_sha256: String,
    manifest_documents: usize,
    eligible_sections: usize,
    issuer_item_histories: usize,
    analyzed_targets: usize,
    alerts: usize,
    alert_counts: BTreeMap<AlertKind, usize>,
    lineage: fo_core::LineageSummary,
    options: AnalysisOptions,
    targets: Vec<TargetAnalysis>,
}

#[derive(Debug, Serialize)]
struct ArtifactReceipt {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct ArtifactManifest {
    schema_version: u32,
    generated_at_unix: u64,
    files: Vec<ArtifactReceipt>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-sec-lineage: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    let options = AnalysisOptions::from(&command);
    let manifest = CorpusManifest::load(&command.corpus_root)?;
    if manifest.provider != CorpusProvider::SecEdgar10K {
        return Err(invalid("fo-sec-lineage requires an SEC EDGAR 10-K corpus"));
    }
    let manifest_bytes = fs::read(command.corpus_root.join(MANIFEST_FILENAME))?;
    let sections = load_sections(&command.corpus_root, &manifest, &options)?;
    if sections.is_empty() {
        return Err(invalid("no eligible SEC sections were loaded"));
    }
    let (histories, item_groups) = build_groups(&sections);
    let targets = select_targets(&sections, &histories, &options);
    if targets.is_empty() {
        return Err(invalid(
            "no targets have sufficient history under the requested selection policy",
        ));
    }

    fs::create_dir_all(command.output.join("results"))?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .build()?;
    let outcomes = pool.install(|| {
        targets
            .par_iter()
            .map(|&target| {
                analyze_target(
                    target,
                    &sections,
                    &histories,
                    &item_groups,
                    &options,
                    &command.corpus_root,
                    &command.output,
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, String>>()
    });
    let mut outcomes = outcomes.map_err(invalid)?;
    outcomes.sort_unstable_by(|left, right| left.analysis.target_id.cmp(&right.analysis.target_id));

    let mut graph = LineageGraph::new();
    for section in &sections {
        graph.upsert_node(LineageNode {
            id: section.record.id.clone(),
            title: section.record.title.clone(),
            observed_at_unix: Some(section.observed_at_unix),
            metadata: node_metadata(section),
        })?;
    }

    let mut result_paths = Vec::new();
    let mut alerts = Vec::new();
    for outcome in &outcomes {
        let result_path = command.output.join(&outcome.analysis.results_file);
        atomic_write(
            &result_path,
            &serde_json::to_vec_pretty(&outcome.review_results)?,
        )?;
        result_paths.push(result_path);
        alerts.extend(outcome.analysis.alerts.iter().cloned());
        for edge in &outcome.edges {
            let mut evidence = LineageEvidence::from_search_result(&edge.result, unix_timestamp());
            evidence
                .metadata
                .insert("source_kind".to_owned(), edge.source_kind.to_owned());
            evidence
                .metadata
                .insert("analysis".to_owned(), "fo-sec-lineage".to_owned());
            graph.add_evidence(&edge.source_id, &edge.target_id, edge.relation, evidence)?;
        }
    }
    graph.validate()?;

    let mut alert_counts = BTreeMap::new();
    for alert in &alerts {
        *alert_counts.entry(alert.kind).or_insert(0usize) += 1;
    }
    let report = SecLineageReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id,
        corpus_manifest_sha256: sha256_hex(&manifest_bytes),
        manifest_documents: manifest.documents.len(),
        eligible_sections: sections.len(),
        issuer_item_histories: histories.len(),
        analyzed_targets: outcomes.len(),
        alerts: alerts.len(),
        alert_counts,
        lineage: graph.summary(),
        options,
        targets: outcomes
            .into_iter()
            .map(|outcome| outcome.analysis)
            .collect(),
    };

    let report_path = command.output.join("report.json");
    let alerts_path = command.output.join("alerts.jsonl");
    let lineage_path = command.output.join("lineage.json");
    let summary_path = command.output.join("SUMMARY.md");
    atomic_write(&report_path, &serde_json::to_vec_pretty(&report)?)?;
    atomic_write(&alerts_path, &jsonl_bytes(&alerts)?)?;
    atomic_write(&lineage_path, &serde_json::to_vec_pretty(&graph)?)?;
    atomic_write(&summary_path, render_summary(&report).as_bytes())?;

    let mut artifacts = vec![report_path, alerts_path, lineage_path, summary_path];
    artifacts.extend(result_paths);
    let artifact_manifest = artifact_manifest(&command.output, &artifacts)?;
    let artifact_path = command.output.join("artifacts.json");
    atomic_write(
        &artifact_path,
        &serde_json::to_vec_pretty(&artifact_manifest)?,
    )?;

    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:                 {}", report.corpus_id);
        println!("Eligible sections:      {}", report.eligible_sections);
        println!("Issuer/item histories:  {}", report.issuer_item_histories);
        println!("Analyzed targets:       {}", report.analyzed_targets);
        println!("Alerts:                 {}", report.alerts);
        println!(
            "Lineage nodes / edges:  {} / {}",
            report.lineage.nodes, report.lineage.edges
        );
        println!(
            "Report:                 {}",
            command.output.join("report.json").display()
        );
        println!(
            "Summary:                {}",
            command.output.join("SUMMARY.md").display()
        );
    }
    Ok(())
}

fn validate_command(command: &Cli) -> CliResult<()> {
    if command.output.exists() {
        return Err(invalid(format!(
            "{} already exists; SEC analysis outputs are immutable",
            command.output.display()
        )));
    }
    if command.maximum_targets == 0
        || command.maximum_prior_filings == 0
        || command.maximum_peer_candidates == 0
        || command.maximum_sources_per_target == 0
        || command.review_candidates == 0
        || command.minimum_section_characters == 0
        || command.threads == 0
        || command.threads > 256
        || command.minimum_edge_matched_tokens == 0
    {
        return Err(invalid("SEC analysis count and thread limits are invalid"));
    }
    for (name, value) in [
        ("minimum_edge_score", command.minimum_edge_score),
        (
            "minimum_edge_query_coverage",
            command.minimum_edge_query_coverage,
        ),
        ("new_language_coverage", command.new_language_coverage),
        ("material_change_coverage", command.material_change_coverage),
        (
            "material_change_similarity",
            command.material_change_similarity,
        ),
        ("high_reuse_coverage", command.high_reuse_coverage),
        ("high_reuse_similarity", command.high_reuse_similarity),
        ("legacy_language_margin", command.legacy_language_margin),
        ("peer_migration_score", command.peer_migration_score),
        ("peer_migration_coverage", command.peer_migration_coverage),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid(format!("{name} must lie in [0, 1]")));
        }
    }
    if command.new_language_coverage > command.material_change_coverage
        || command.material_change_coverage > command.high_reuse_coverage
        || command.material_change_similarity > command.high_reuse_similarity
    {
        return Err(invalid(
            "coverage and similarity thresholds must increase from new/material to high reuse",
        ));
    }
    Ok(())
}

fn load_sections(
    root: &Path,
    manifest: &CorpusManifest,
    options: &AnalysisOptions,
) -> CliResult<Vec<SecSection>> {
    let mut sections = Vec::new();
    for record in &manifest.documents {
        let Some(cik) = record.metadata.get("cik").cloned() else {
            continue;
        };
        let Some(section_title) = record.metadata.get("section_title").cloned() else {
            continue;
        };
        if !options.section_filters.is_empty()
            && !options
                .section_filters
                .iter()
                .any(|filter| section_title.to_ascii_lowercase().contains(filter))
        {
            continue;
        }
        let Some(filing_date) = record.published_or_filed.clone() else {
            continue;
        };
        let Some(observed_at_unix) = iso_date_to_unix(&filing_date) else {
            continue;
        };
        validate_relative_path(&record.relative_path)?;
        let path = root.join(&record.relative_path);
        let bytes = fs::read(&path)?;
        if bytes.len() as u64 != record.bytes || sha256_hex(&bytes) != record.sha256 {
            return Err(invalid(format!(
                "section {} no longer matches its corpus manifest receipt",
                record.id
            )));
        }
        let body = String::from_utf8(bytes)
            .map_err(|error| invalid(format!("section {} is not UTF-8: {error}", record.id)))?;
        if body.chars().count() < options.minimum_section_characters {
            continue;
        }
        sections.push(SecSection {
            record: record.clone(),
            body,
            cik,
            section_key: canonical_section(&section_title),
            section_title,
            filing_date,
            observed_at_unix,
        });
    }
    sections.sort_unstable_by(|left, right| left.record.id.cmp(&right.record.id));
    Ok(sections)
}

fn build_groups(
    sections: &[SecSection],
) -> (BTreeMap<String, Vec<usize>>, BTreeMap<String, Vec<usize>>) {
    let mut histories = BTreeMap::<String, Vec<usize>>::new();
    let mut items = BTreeMap::<String, Vec<usize>>::new();
    for (index, section) in sections.iter().enumerate() {
        histories
            .entry(history_key(section))
            .or_default()
            .push(index);
        items
            .entry(section.section_key.clone())
            .or_default()
            .push(index);
    }
    for values in histories.values_mut().chain(items.values_mut()) {
        values.sort_unstable_by(|left, right| {
            sections[*left]
                .filing_date
                .cmp(&sections[*right].filing_date)
                .then_with(|| sections[*left].record.id.cmp(&sections[*right].record.id))
        });
    }
    (histories, items)
}

fn select_targets(
    sections: &[SecSection],
    histories: &BTreeMap<String, Vec<usize>>,
    options: &AnalysisOptions,
) -> Vec<usize> {
    let mut targets = BTreeSet::new();
    for history in histories.values() {
        if history.len() == 1 {
            if options.include_no_history {
                targets.insert(history[0]);
            }
            continue;
        }
        if options.all_targets {
            targets.extend(history.iter().skip(1).copied());
        } else if let Some(&latest) = history.last() {
            targets.insert(latest);
        }
    }
    let mut targets = targets.into_iter().collect::<Vec<_>>();
    targets.sort_unstable_by(|left, right| {
        sections[*right]
            .filing_date
            .cmp(&sections[*left].filing_date)
            .then_with(|| sections[*left].record.id.cmp(&sections[*right].record.id))
    });
    targets.truncate(options.maximum_targets);
    targets
}

fn analyze_target(
    target_index: usize,
    sections: &[SecSection],
    histories: &BTreeMap<String, Vec<usize>>,
    item_groups: &BTreeMap<String, Vec<usize>>,
    options: &AnalysisOptions,
    corpus_root: &Path,
    output_root: &Path,
) -> CliResult<TargetOutcome> {
    let target = &sections[target_index];
    let history = &histories[&history_key(target)];
    let target_position = history
        .iter()
        .position(|&index| index == target_index)
        .ok_or_else(|| invalid("target is absent from its history"))?;
    let mut prior_indices = history[..target_position]
        .iter()
        .rev()
        .take(options.maximum_prior_filings)
        .copied()
        .collect::<Vec<_>>();
    prior_indices.reverse();

    let same_report = search_subset(target, &prior_indices, sections, options)?;
    let same_results = same_report
        .as_ref()
        .map_or_else(Vec::new, |report| report.results.clone());
    let latest_prior_id = prior_indices
        .last()
        .map(|&index| sections[index].record.id.as_str());
    let best_previous = same_results
        .first()
        .and_then(|result| match_from_result(result, sections, true));

    let peer_indices = if options.include_peer_search {
        item_groups
            .get(&target.section_key)
            .into_iter()
            .flatten()
            .filter(|&&index| {
                sections[index].cik != target.cik
                    && sections[index].filing_date < target.filing_date
            })
            .rev()
            .take(options.maximum_peer_candidates)
            .copied()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let peer_report = search_subset(target, &peer_indices, sections, options)?;
    let peer_results = peer_report
        .as_ref()
        .map_or_else(Vec::new, |report| report.results.clone());
    let best_peer = peer_results
        .first()
        .and_then(|result| match_from_result(result, sections, false));

    let mut alerts = classify_alerts(
        target,
        best_previous.as_ref(),
        best_peer.as_ref(),
        latest_prior_id,
        &same_results,
        same_report.as_ref().map(|report| report.status),
        options,
    );
    alerts.sort_unstable_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut accepted_sources = Vec::new();
    let mut edges = Vec::new();
    for result in same_results
        .iter()
        .filter(|result| retain_edge(result, options))
        .take(options.maximum_sources_per_target)
    {
        if let Some(source) = match_from_result(result, sections, true) {
            accepted_sources.push(source);
            edges.push(EdgeDraft {
                source_id: result.path.clone(),
                target_id: target.record.id.clone(),
                relation: LineageRelation::DerivedFrom,
                result: result.clone(),
                source_kind: "same_issuer_history",
            });
        }
    }
    if let Some(result) = peer_results
        .iter()
        .find(|result| {
            result.combined_score >= options.peer_migration_score
                && result.query_coverage >= options.peer_migration_coverage
                && result.matched_tokens >= options.minimum_edge_matched_tokens
        })
        .cloned()
    {
        if let Some(source) = match_from_result(&result, sections, false) {
            accepted_sources.push(source);
            edges.push(EdgeDraft {
                source_id: result.path.clone(),
                target_id: target.record.id.clone(),
                relation: LineageRelation::Reuses,
                result,
                source_kind: "cross_issuer_peer",
            });
        }
    }

    let mut review_results = same_results;
    review_results.extend(peer_results);
    review_results.sort_unstable_by(|left, right| {
        right
            .combined_score
            .total_cmp(&left.combined_score)
            .then_with(|| left.path.cmp(&right.path))
    });
    review_results.dedup_by(|left, right| left.path == right.path);
    review_results.truncate(options.review_candidates);

    let result_name = format!("results/{}.json", sanitize_component(&target.record.id));
    let specimen_relative_path = target.record.relative_path.clone();
    let review_command = format!(
        "fo-review-report {} {} {} --target-id {} --output reviews/{}",
        shell_quote(&corpus_root.display().to_string()),
        shell_quote(
            &corpus_root
                .join(&specimen_relative_path)
                .display()
                .to_string()
        ),
        shell_quote(&output_root.join(&result_name).display().to_string()),
        shell_quote(&target.record.id),
        shell_quote(&sanitize_component(&target.record.id)),
    );
    Ok(TargetOutcome {
        analysis: TargetAnalysis {
            target_id: target.record.id.clone(),
            target_title: target.record.title.clone(),
            target_cik: target.cik.clone(),
            target_filing_date: target.filing_date.clone(),
            section_title: target.section_title.clone(),
            section_key: target.section_key.clone(),
            prior_documents: prior_indices.len(),
            peer_documents: peer_indices.len(),
            same_issuer_status: same_report.as_ref().map(|report| report.status),
            peer_status: peer_report.as_ref().map(|report| report.status),
            same_issuer_analysis: same_report.map(|report| report.analysis),
            peer_analysis: peer_report.map(|report| report.analysis),
            best_previous,
            best_peer,
            accepted_sources,
            alerts,
            results_file: result_name,
            specimen_relative_path,
            review_command,
        },
        review_results,
        edges,
    })
}

fn search_subset(
    target: &SecSection,
    candidate_indices: &[usize],
    sections: &[SecSection],
    _options: &AnalysisOptions,
) -> CliResult<Option<fo_core::DomainSearchReport>> {
    if candidate_indices.is_empty() {
        return Ok(None);
    }
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    for &index in candidate_indices {
        builder.add_document(sections[index].record.id.clone(), &sections[index].body)?;
    }
    let index = builder.build()?;
    let report = index.search_domain_adaptive(
        &target.body,
        &DomainSearchOptions {
            search: SearchOptions {
                intent: SearchIntent::SourceAttribution,
                max_results: candidate_indices.len(),
                max_candidates: candidate_indices.len().saturating_mul(16).max(64),
                minimum_similarity: 0.10,
                minimum_query_coverage: 0.0,
                minimum_source_coverage: 0.0,
                minimum_matched_tokens: 12,
                ..SearchOptions::default()
            },
            ..DomainSearchOptions::for_domain(TextDomain::SecFiling)
        },
    )?;
    Ok(Some(report))
}

fn classify_alerts(
    target: &SecSection,
    best_previous: Option<&SourceMatch>,
    best_peer: Option<&SourceMatch>,
    latest_prior_id: Option<&str>,
    same_results: &[SearchResult],
    status: Option<DomainSearchStatus>,
    options: &AnalysisOptions,
) -> Vec<SecAlert> {
    let mut alerts = Vec::new();
    match best_previous {
        None if latest_prior_id.is_none() => alerts.push(alert(
            AlertKind::NoHistory,
            AlertSeverity::Info,
            target,
            None,
            "No earlier filing item exists in the loaded corpus.".to_owned(),
        )),
        None if status == Some(DomainSearchStatus::InsufficientInformativeEvidence) => {
            alerts.push(alert(
                AlertKind::InsufficientDistinctiveEvidence,
                AlertSeverity::Review,
                target,
                None,
                "The item is dominated by common language or novel wording; too little distinctive overlap survived the SEC policy.".to_owned(),
            ));
        }
        None => alerts.push(alert(
            AlertKind::NewLanguage,
            AlertSeverity::High,
            target,
            None,
            "No prior same-issuer item produced a verified overlap candidate.".to_owned(),
        )),
        Some(source)
            if source.query_coverage >= options.high_reuse_coverage
                && source.edit_similarity >= options.high_reuse_similarity =>
        {
            alerts.push(alert(
                AlertKind::HighReuse,
                AlertSeverity::Info,
                target,
                Some(source.clone()),
                format!(
                    "The filing retains {:.1}% of the target wording with {:.1}% local edit similarity.",
                    100.0 * source.query_coverage,
                    100.0 * source.edit_similarity,
                ),
            ));
        }
        Some(source) if source.query_coverage < options.new_language_coverage => {
            alerts.push(alert(
                AlertKind::NewLanguage,
                AlertSeverity::High,
                target,
                Some(source.clone()),
                format!(
                    "Only {:.1}% of the target item is explained by the strongest prior filing.",
                    100.0 * source.query_coverage,
                ),
            ));
        }
        Some(source)
            if source.query_coverage < options.material_change_coverage
                || source.edit_similarity < options.material_change_similarity =>
        {
            alerts.push(alert(
                AlertKind::MaterialRevision,
                AlertSeverity::High,
                target,
                Some(source.clone()),
                format!(
                    "The strongest prior item has {:.1}% target coverage and {:.1}% edit similarity.",
                    100.0 * source.query_coverage,
                    100.0 * source.edit_similarity,
                ),
            ));
        }
        Some(source) => alerts.push(alert(
            AlertKind::ModerateRevision,
            AlertSeverity::Review,
            target,
            Some(source.clone()),
            format!(
                "The item is related to the prior filing but retains only {:.1}% target coverage.",
                100.0 * source.query_coverage,
            ),
        )),
    }

    if let (Some(best), Some(latest_id)) = (best_previous, latest_prior_id)
        && best.source_id != latest_id
    {
        let latest_score = same_results
            .iter()
            .find(|result| result.path == latest_id)
            .map_or(0.0, |result| result.combined_score);
        if best.score >= latest_score + options.legacy_language_margin {
            alerts.push(alert(
                AlertKind::LegacyLanguageReintroduced,
                AlertSeverity::Review,
                target,
                Some(best.clone()),
                format!(
                    "An older filing outranks the immediately prior year by {:.3}, suggesting reintroduced legacy language.",
                    best.score - latest_score,
                ),
            ));
        }
    }

    if let Some(peer) = best_peer
        && peer.score >= options.peer_migration_score
        && peer.query_coverage >= options.peer_migration_coverage
    {
        alerts.push(alert(
            AlertKind::PeerLanguageMigration,
            AlertSeverity::High,
            target,
            Some(peer.clone()),
            format!(
                "An earlier filing from another issuer explains {:.1}% of the target item with score {:.3}.",
                100.0 * peer.query_coverage,
                peer.score,
            ),
        ));
    }
    alerts
}

fn alert(
    kind: AlertKind,
    severity: AlertSeverity,
    target: &SecSection,
    source: Option<SourceMatch>,
    message: String,
) -> SecAlert {
    let source_id = source
        .as_ref()
        .map_or("none", |source| source.source_id.as_str());
    SecAlert {
        id: format!(
            "alert-{:016x}",
            stable_hash(&format!("{kind:?}\0{}\0{source_id}", target.record.id))
        ),
        kind,
        severity,
        target_id: target.record.id.clone(),
        target_title: target.record.title.clone(),
        target_cik: target.cik.clone(),
        target_filing_date: target.filing_date.clone(),
        section_title: target.section_title.clone(),
        source,
        message,
    }
}

fn retain_edge(result: &SearchResult, options: &AnalysisOptions) -> bool {
    result.combined_score >= options.minimum_edge_score
        && result.query_coverage >= options.minimum_edge_query_coverage
        && result.matched_tokens >= options.minimum_edge_matched_tokens
}

fn match_from_result(
    result: &SearchResult,
    sections: &[SecSection],
    same_issuer: bool,
) -> Option<SourceMatch> {
    let source = sections
        .iter()
        .find(|section| section.record.id == result.path)?;
    Some(SourceMatch {
        source_id: source.record.id.clone(),
        source_title: source.record.title.clone(),
        source_cik: source.cik.clone(),
        source_filing_date: source.filing_date.clone(),
        same_issuer,
        score: result.combined_score,
        edit_similarity: result.edit_similarity,
        query_coverage: result.query_coverage,
        source_coverage: result.source_coverage,
        matched_tokens: result.matched_tokens,
        expected_false_matches: result.estimated_false_matches,
        corpus_start: result.corpus_start,
        corpus_end: result.corpus_end,
        query_start: result.query_start,
        query_end: result.query_end,
    })
}

fn node_metadata(section: &SecSection) -> BTreeMap<String, String> {
    let mut metadata = section.record.metadata.clone();
    metadata.insert("cik".to_owned(), section.cik.clone());
    metadata.insert("filing_date".to_owned(), section.filing_date.clone());
    metadata.insert("section_title".to_owned(), section.section_title.clone());
    metadata.insert("section_key".to_owned(), section.section_key.clone());
    metadata.insert(
        "relative_path".to_owned(),
        section.record.relative_path.clone(),
    );
    metadata
}

fn history_key(section: &SecSection) -> String {
    format!("{}\0{}", section.cik, section.section_key)
}

fn canonical_section(value: &str) -> String {
    let mut output = String::new();
    let mut space = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if space && !output.is_empty() {
                output.push(' ');
            }
            output.push(character);
            space = false;
        } else {
            space = true;
        }
    }
    output
}

fn iso_date_to_unix(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = value[0..4].parse::<i64>().ok()?;
    let month = value[5..7].parse::<i64>().ok()?;
    let day = value[8..10].parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    if days < 0 {
        None
    } else {
        u64::try_from(days).ok()?.checked_mul(86_400)
    }
}

fn render_summary(report: &SecLineageReport) -> String {
    let mut output = format!(
        "# SEC textual-lineage analysis\n\n- Corpus: `{}`\n- Eligible sections: {}\n- Issuer/item histories: {}\n- Analyzed targets: {}\n- Lineage nodes / edges: {} / {}\n- Alerts: {}\n\n## Alert counts\n\n| Kind | Count |\n|---|---:|\n",
        report.corpus_id,
        report.eligible_sections,
        report.issuer_item_histories,
        report.analyzed_targets,
        report.lineage.nodes,
        report.lineage.edges,
        report.alerts,
    );
    for (kind, count) in &report.alert_counts {
        output.push_str(&format!("| `{:?}` | {} |\n", kind, count));
    }
    output.push_str("\n## Highest-priority alerts\n\n| Severity | Kind | Filing | Source | Message |\n|---|---|---|---|---|\n");
    let mut alerts = report
        .targets
        .iter()
        .flat_map(|target| target.alerts.iter())
        .collect::<Vec<_>>();
    alerts.sort_unstable_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    for alert in alerts.into_iter().take(100) {
        output.push_str(&format!(
            "| `{:?}` | `{:?}` | `{}` | `{}` | {} |\n",
            alert.severity,
            alert.kind,
            markdown_escape(&alert.target_id),
            markdown_escape(
                alert
                    .source
                    .as_ref()
                    .map_or("", |source| source.source_id.as_str())
            ),
            markdown_escape(&alert.message),
        ));
    }
    output.push_str("\n## Review workflow\n\nEach target entry in `report.json` contains a ready-to-run `fo-review-report` command and a result JSON path under `results/`. Accepted localized sources can be merged into `lineage.json`; rejected or uncertain candidates should enter the feedback and active-learning ledgers.\n");
    output
}

fn jsonl_bytes<T: Serialize>(values: &[T]) -> CliResult<Vec<u8>> {
    let mut output = Vec::new();
    for value in values {
        serde_json::to_writer(&mut output, value)?;
        output.push(b'\n');
    }
    Ok(output)
}

fn artifact_manifest(root: &Path, paths: &[PathBuf]) -> CliResult<ArtifactManifest> {
    let mut files = Vec::new();
    for path in paths {
        let bytes = fs::read(path)?;
        files.push(ArtifactReceipt {
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/"),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(ArtifactManifest {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        files,
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

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(180)
        .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
