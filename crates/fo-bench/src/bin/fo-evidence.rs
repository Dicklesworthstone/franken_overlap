#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use fo_core::{
    GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore,
    grouped_evaluation_report,
};
use fo_corpus::{atomic_write, sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const EVIDENCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-evidence",
    version,
    about = "Turn fo-real-bench reports and score streams into auditable evidence bundles"
)]
struct Cli {
    /// JSON report emitted by fo-real-bench.
    report: PathBuf,
    /// Per-query-document JSONL emitted by fo-real-bench --scores-output.
    scores: PathBuf,
    /// Evidence bundle destination.
    #[arg(short, long)]
    output: PathBuf,
    /// Method whose quality claims are being evaluated.
    #[arg(long, default_value = "franken_hybrid")]
    method: String,
    /// Baseline methods. When omitted, every other method in the report is used.
    #[arg(long = "baseline")]
    baselines: Vec<String>,
    /// Query-group paired bootstrap resamples. Zero disables confidence intervals.
    #[arg(long, default_value_t = 2_000)]
    bootstrap_samples: usize,
    #[arg(long, default_value_t = 0.95)]
    confidence_level: f64,
    #[arg(long, default_value_t = 0x65_76_69_64_65_6e_63_65)]
    seed: u64,
    /// Number of top candidates retained per method in illustrative examples.
    #[arg(long, default_value_t = 5)]
    top_candidates: usize,
    /// Required lower confidence bound for macro query AUPRC improvement.
    #[arg(long, default_value_t = 0.0)]
    minimum_macro_delta_lower_bound: f64,
    /// Maximum permitted point-estimate Recall@1 regression.
    #[arg(long, default_value_t = 0.0)]
    maximum_recall_at_1_regression: f64,
    /// Refuse to replace a pre-existing evidence directory.
    #[arg(long)]
    no_replace: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkReport {
    schema_version: u32,
    generated_at_unix: u64,
    corpus_id: String,
    corpus_provider: String,
    corpus_manifest_documents: usize,
    indexed_documents: usize,
    source_documents: usize,
    queries: usize,
    pairs: usize,
    seed: u64,
    profiles: Vec<String>,
    build: BuildReport,
    methods: Vec<BenchmarkMethodReport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BuildReport {
    build_ms: f64,
    serialization_ms: f64,
    index_bytes: u64,
    overlap_fingerprints: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkMethodReport {
    name: String,
    elapsed_ms: f64,
    queries_per_second: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    false_positives_per_query_at_best_f1: f64,
    quality: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Deserialize)]
struct ScoreRow {
    query_id: String,
    profile: String,
    source_id: String,
    candidate_id: String,
    label: bool,
    scores: BTreeMap<String, f64>,
    #[serde(default)]
    query_text: Option<String>,
    #[serde(default)]
    source_title: Option<String>,
    #[serde(default)]
    candidate_title: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct MetricSnapshot {
    micro_auprc: f64,
    macro_auprc: f64,
    mean_reciprocal_rank: f64,
    recall_at_1: f64,
    recall_at_5: f64,
    recall_at_10: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct DeltaInterval {
    lower: f64,
    upper: f64,
    confidence_level: f64,
    samples: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct MetricDelta {
    point: f64,
    interval: Option<DeltaInterval>,
}

#[derive(Debug, Clone, Serialize)]
struct TimingComparison {
    selected_p50_ms: f64,
    selected_p95_ms: f64,
    selected_p99_ms: f64,
    baseline_p50_ms: f64,
    baseline_p95_ms: f64,
    baseline_p99_ms: f64,
    p95_speed_ratio: f64,
    selected_queries_per_second: f64,
    baseline_queries_per_second: f64,
    throughput_ratio: f64,
    selected_is_faster_at_p95: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MethodComparison {
    selected_method: String,
    baseline_method: String,
    selected: MetricSnapshot,
    baseline: MetricSnapshot,
    micro_auprc_delta: MetricDelta,
    macro_auprc_delta: MetricDelta,
    mean_reciprocal_rank_delta: MetricDelta,
    recall_at_1_delta: MetricDelta,
    timing: TimingComparison,
    quality_gate_passed: bool,
    strict_quality_and_p95_dominance: bool,
}

#[derive(Debug, Clone, Serialize)]
struct RankedCandidate {
    candidate_id: String,
    title: Option<String>,
    score: f64,
    label: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MethodRankSummary {
    method: String,
    positive_best_rank: usize,
    positive_worst_rank: usize,
    positive_expected_rank: f64,
    positive_expected_reciprocal_rank: f64,
    top_candidates: Vec<RankedCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct IllustrativeExample {
    profile: String,
    kind: String,
    query_id: String,
    query_text: Option<String>,
    source_id: String,
    source_title: Option<String>,
    selected: MethodRankSummary,
    baselines: Vec<MethodRankSummary>,
    expected_rank_improvement_over_best_baseline: f64,
}

#[derive(Debug, Clone, Serialize)]
struct EnvironmentReport {
    generated_at_unix: u64,
    operating_system: String,
    architecture: String,
    logical_parallelism: usize,
    rustc: Option<String>,
    git_commit: Option<String>,
    uname: Option<String>,
    report_path: String,
    report_sha256: String,
    scores_path: String,
    scores_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct EvidenceVerdict {
    best_baseline: String,
    quality_better_than_best_baseline: bool,
    faster_than_best_baseline_at_p95: bool,
    strictly_dominates_best_baseline: bool,
    faster_than_at_least_one_baseline_at_p95: bool,
    claim: String,
}

#[derive(Debug, Clone, Serialize)]
struct EvidenceReport {
    schema_version: u32,
    generated_at_unix: u64,
    benchmark_schema_version: u32,
    benchmark_generated_at_unix: u64,
    corpus_id: String,
    corpus_provider: String,
    corpus_manifest_documents: usize,
    indexed_documents: usize,
    source_documents: usize,
    queries: usize,
    pairs: usize,
    minimum_candidates_per_query: usize,
    maximum_candidates_per_query: usize,
    positives: usize,
    seed: u64,
    profiles: Vec<String>,
    selected_method: String,
    baselines: Vec<String>,
    build: BuildReport,
    comparisons: Vec<MethodComparison>,
    examples: Vec<IllustrativeExample>,
    verdict: EvidenceVerdict,
}

#[derive(Debug, Clone, Serialize)]
struct BundleFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct BundleManifest {
    schema_version: u32,
    generated_at_unix: u64,
    files: Vec<BundleFile>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-evidence: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    if command.output.exists() {
        if command.no_replace {
            return Err(invalid_input(format!(
                "output directory {} already exists",
                command.output.display()
            )));
        }
        fs::remove_dir_all(&command.output)?;
    }
    fs::create_dir_all(&command.output)?;

    let report_bytes = fs::read(&command.report)?;
    let scores_bytes = fs::read(&command.scores)?;
    let benchmark = serde_json::from_slice::<BenchmarkReport>(&report_bytes)?;
    let rows = read_score_rows(&command.scores)?;
    let grouped = validate_inputs(&benchmark, &rows)?;

    let report_methods = benchmark
        .methods
        .iter()
        .map(|method| method.name.clone())
        .collect::<BTreeSet<_>>();
    if !report_methods.contains(&command.method) {
        return Err(invalid_input(format!(
            "selected method {:?} is absent from the benchmark report",
            command.method
        )));
    }
    let baselines = if command.baselines.is_empty() {
        report_methods
            .iter()
            .filter(|method| method.as_str() != command.method)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let mut values = command.baselines.clone();
        values.sort_unstable();
        values.dedup();
        values
    };
    if baselines.is_empty() {
        return Err(invalid_input("at least one baseline method is required"));
    }
    for baseline in &baselines {
        if baseline == &command.method {
            return Err(invalid_input("selected method cannot also be a baseline"));
        }
        if !report_methods.contains(baseline) {
            return Err(invalid_input(format!(
                "baseline method {baseline:?} is absent from the report"
            )));
        }
    }

    let selected_report = evaluate_rows(&rows, &command.method)?;
    verify_report_metric_consistency(&benchmark, &command.method, &selected_report)?;
    let selected_snapshot = metric_snapshot(&selected_report);
    let mut comparisons = Vec::with_capacity(baselines.len());
    for baseline in &baselines {
        let baseline_report = evaluate_rows(&rows, baseline)?;
        verify_report_metric_consistency(&benchmark, baseline, &baseline_report)?;
        let baseline_snapshot = metric_snapshot(&baseline_report);
        let intervals = bootstrap_deltas(
            &grouped,
            &command.method,
            baseline,
            command.bootstrap_samples,
            command.confidence_level,
            command.seed ^ stable_hash(baseline),
        )?;
        let selected_timing = benchmark_method(&benchmark, &command.method)?;
        let baseline_timing = benchmark_method(&benchmark, baseline)?;
        let timing = TimingComparison {
            selected_p50_ms: selected_timing.p50_ms,
            selected_p95_ms: selected_timing.p95_ms,
            selected_p99_ms: selected_timing.p99_ms,
            baseline_p50_ms: baseline_timing.p50_ms,
            baseline_p95_ms: baseline_timing.p95_ms,
            baseline_p99_ms: baseline_timing.p99_ms,
            p95_speed_ratio: ratio(baseline_timing.p95_ms, selected_timing.p95_ms),
            selected_queries_per_second: selected_timing.queries_per_second,
            baseline_queries_per_second: baseline_timing.queries_per_second,
            throughput_ratio: ratio(
                selected_timing.queries_per_second,
                baseline_timing.queries_per_second,
            ),
            selected_is_faster_at_p95: selected_timing.p95_ms < baseline_timing.p95_ms,
        };
        let macro_delta = selected_snapshot.macro_auprc - baseline_snapshot.macro_auprc;
        let recall_delta = selected_snapshot.recall_at_1 - baseline_snapshot.recall_at_1;
        let macro_lower = intervals
            .macro_auprc
            .map_or(macro_delta, |interval| interval.lower);
        let quality_gate_passed = macro_lower >= command.minimum_macro_delta_lower_bound
            && recall_delta >= -command.maximum_recall_at_1_regression;
        comparisons.push(MethodComparison {
            selected_method: command.method.clone(),
            baseline_method: baseline.clone(),
            selected: selected_snapshot,
            baseline: baseline_snapshot,
            micro_auprc_delta: MetricDelta {
                point: selected_snapshot.micro_auprc - baseline_snapshot.micro_auprc,
                interval: intervals.micro_auprc,
            },
            macro_auprc_delta: MetricDelta {
                point: macro_delta,
                interval: intervals.macro_auprc,
            },
            mean_reciprocal_rank_delta: MetricDelta {
                point: selected_snapshot.mean_reciprocal_rank
                    - baseline_snapshot.mean_reciprocal_rank,
                interval: intervals.mean_reciprocal_rank,
            },
            recall_at_1_delta: MetricDelta {
                point: recall_delta,
                interval: intervals.recall_at_1,
            },
            strict_quality_and_p95_dominance: quality_gate_passed
                && timing.selected_is_faster_at_p95,
            quality_gate_passed,
            timing,
        });
    }
    comparisons.sort_unstable_by(|left, right| left.baseline_method.cmp(&right.baseline_method));

    let examples = select_examples(
        &grouped,
        &command.method,
        &baselines,
        command.top_candidates,
    );
    let best_baseline = comparisons
        .iter()
        .max_by(|left, right| {
            left.baseline
                .macro_auprc
                .total_cmp(&right.baseline.macro_auprc)
                .then_with(|| left.baseline.micro_auprc.total_cmp(&right.baseline.micro_auprc))
        })
        .ok_or_else(|| invalid_input("no baseline comparisons were produced"))?;
    let quality_better_than_best_baseline = best_baseline.quality_gate_passed;
    let faster_than_best_baseline_at_p95 = best_baseline.timing.selected_is_faster_at_p95;
    let strictly_dominates_best_baseline = best_baseline.strict_quality_and_p95_dominance;
    let faster_than_at_least_one_baseline_at_p95 = comparisons
        .iter()
        .any(|comparison| comparison.timing.selected_is_faster_at_p95);
    let claim = if strictly_dominates_best_baseline {
        "quality_and_wall_time_superiority_supported"
    } else if quality_better_than_best_baseline {
        "quality_superiority_supported_wall_time_not_dominant"
    } else {
        "superiority_not_established"
    };

    let candidate_counts = grouped.values().map(Vec::len).collect::<Vec<_>>();
    let evidence = EvidenceReport {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        benchmark_schema_version: benchmark.schema_version,
        benchmark_generated_at_unix: benchmark.generated_at_unix,
        corpus_id: benchmark.corpus_id.clone(),
        corpus_provider: benchmark.corpus_provider.clone(),
        corpus_manifest_documents: benchmark.corpus_manifest_documents,
        indexed_documents: benchmark.indexed_documents,
        source_documents: benchmark.source_documents,
        queries: grouped.len(),
        pairs: rows.len(),
        minimum_candidates_per_query: candidate_counts.iter().copied().min().unwrap_or(0),
        maximum_candidates_per_query: candidate_counts.iter().copied().max().unwrap_or(0),
        positives: rows.iter().filter(|row| row.label).count(),
        seed: benchmark.seed,
        profiles: benchmark.profiles.clone(),
        selected_method: command.method.clone(),
        baselines: baselines.clone(),
        build: benchmark.build.clone(),
        comparisons,
        examples,
        verdict: EvidenceVerdict {
            best_baseline: best_baseline.baseline_method.clone(),
            quality_better_than_best_baseline,
            faster_than_best_baseline_at_p95,
            strictly_dominates_best_baseline,
            faster_than_at_least_one_baseline_at_p95,
            claim: claim.to_owned(),
        },
    };
    let environment = EnvironmentReport {
        generated_at_unix: unix_timestamp(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        logical_parallelism: std::thread::available_parallelism().map_or(1, usize::from),
        rustc: command_output("rustc", &["-Vv"]),
        git_commit: command_output("git", &["rev-parse", "HEAD"]),
        uname: command_output("uname", &["-a"]),
        report_path: command.report.display().to_string(),
        report_sha256: sha256_hex(&report_bytes),
        scores_path: command.scores.display().to_string(),
        scores_sha256: sha256_hex(&scores_bytes),
    };

    let evidence_path = command.output.join("evidence.json");
    let environment_path = command.output.join("environment.json");
    let examples_path = command.output.join("EXAMPLES.md");
    atomic_write(&evidence_path, &serde_json::to_vec_pretty(&evidence)?)?;
    atomic_write(
        &environment_path,
        &serde_json::to_vec_pretty(&environment)?,
    )?;
    atomic_write(&examples_path, render_examples(&evidence).as_bytes())?;
    let bundle = write_bundle_manifest(&command.output, &[&evidence_path, &environment_path, &examples_path])?;

    println!("Evidence bundle: {}", command.output.display());
    println!("Corpus:          {}", evidence.corpus_id);
    println!("Queries/pairs:   {} / {}", evidence.queries, evidence.pairs);
    println!("Selected method: {}", evidence.selected_method);
    println!("Best baseline:   {}", evidence.verdict.best_baseline);
    println!("Verdict:         {}", evidence.verdict.claim);
    println!("Bundle files:    {}", bundle.files.len());
    Ok(())
}

fn validate_command(command: &Cli) -> CliResult<()> {
    if command.method.trim().is_empty() || command.top_candidates == 0 {
        return Err(invalid_input(
            "method names must be nonempty and top-candidates must be positive",
        ));
    }
    if command.bootstrap_samples > 100_000 {
        return Err(invalid_input("bootstrap-samples must not exceed 100000"));
    }
    if !command.confidence_level.is_finite()
        || command.confidence_level <= 0.0
        || command.confidence_level >= 1.0
        || !command.minimum_macro_delta_lower_bound.is_finite()
        || !command.maximum_recall_at_1_regression.is_finite()
        || command.maximum_recall_at_1_regression < 0.0
    {
        return Err(invalid_input("evidence thresholds are invalid"));
    }
    Ok(())
}

fn read_score_rows(path: &Path) -> CliResult<Vec<ScoreRow>> {
    let file = File::open(path)?;
    let mut rows = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        rows.push(serde_json::from_str::<ScoreRow>(value).map_err(|error| {
            invalid_input(format!("{}:{}: {error}", path.display(), line_index + 1))
        })?);
    }
    if rows.is_empty() {
        return Err(invalid_input(format!(
            "{} contains no score rows",
            path.display()
        )));
    }
    Ok(rows)
}

fn validate_inputs<'a>(
    benchmark: &BenchmarkReport,
    rows: &'a [ScoreRow],
) -> CliResult<BTreeMap<&'a str, Vec<&'a ScoreRow>>> {
    if benchmark.queries == 0 || benchmark.pairs == 0 || benchmark.methods.is_empty() {
        return Err(invalid_input("benchmark report is empty"));
    }
    if benchmark.pairs != rows.len() {
        return Err(invalid_input(format!(
            "report declares {} pairs but score stream contains {}",
            benchmark.pairs,
            rows.len()
        )));
    }
    let method_names = benchmark
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut grouped = BTreeMap::<&str, Vec<&ScoreRow>>::new();
    let mut seen = BTreeSet::<(&str, &str)>::new();
    for (row_index, row) in rows.iter().enumerate() {
        if row.query_id.trim().is_empty()
            || row.profile.trim().is_empty()
            || row.source_id.trim().is_empty()
            || row.candidate_id.trim().is_empty()
        {
            return Err(invalid_input(format!(
                "score row {row_index} has an empty identifier"
            )));
        }
        if !seen.insert((&row.query_id, &row.candidate_id)) {
            return Err(invalid_input(format!(
                "duplicate query/candidate pair {}/{}",
                row.query_id, row.candidate_id
            )));
        }
        if row.scores.keys().map(String::as_str).collect::<BTreeSet<_>>() != method_names {
            return Err(invalid_input(format!(
                "score row {row_index} method set disagrees with the report"
            )));
        }
        if row
            .scores
            .values()
            .any(|score| !score.is_finite() || !(0.0..=1.0).contains(score))
        {
            return Err(invalid_input(format!(
                "score row {row_index} contains a non-finite or out-of-range score"
            )));
        }
        grouped.entry(&row.query_id).or_default().push(row);
    }
    if grouped.len() != benchmark.queries {
        return Err(invalid_input(format!(
            "report declares {} queries but score stream contains {}",
            benchmark.queries,
            grouped.len()
        )));
    }
    for (query_id, group) in &grouped {
        let first = group[0];
        if group.iter().any(|row| {
            row.profile != first.profile || row.source_id != first.source_id
        }) {
            return Err(invalid_input(format!(
                "query group {query_id} has inconsistent profile or source metadata"
            )));
        }
        if group.iter().filter(|row| row.label).count() == 0 {
            return Err(invalid_input(format!(
                "query group {query_id} contains no positive candidate"
            )));
        }
    }
    Ok(grouped)
}

fn evaluate_rows(rows: &[ScoreRow], method: &str) -> CliResult<GroupedEvaluationReport> {
    let examples = rows
        .iter()
        .map(|row| {
            Ok(GroupedLabeledScore {
                query_id: row.query_id.clone(),
                score: *row.scores.get(method).ok_or_else(|| {
                    invalid_input(format!("score row lacks method {method}"))
                })?,
                label: row.label,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    Ok(grouped_evaluation_report(&examples, evaluation_options())?)
}

fn metric_snapshot(report: &GroupedEvaluationReport) -> MetricSnapshot {
    MetricSnapshot {
        micro_auprc: report.micro.average_precision,
        macro_auprc: report.macro_average_precision,
        mean_reciprocal_rank: report.mean_reciprocal_rank,
        recall_at_1: metric_at(report, 1),
        recall_at_5: metric_at(report, 5),
        recall_at_10: metric_at(report, 10),
    }
}

fn evaluation_options() -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    }
}

fn metric_at(report: &GroupedEvaluationReport, k: usize) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == k)
        .map_or(0.0, |metric| metric.value)
}

fn verify_report_metric_consistency(
    benchmark: &BenchmarkReport,
    method: &str,
    observed: &GroupedEvaluationReport,
) -> CliResult<()> {
    let expected = benchmark_method(benchmark, method)?;
    for (name, left, right) in [
        (
            "micro AUPRC",
            observed.micro.average_precision,
            expected.quality.micro.average_precision,
        ),
        (
            "macro AUPRC",
            observed.macro_average_precision,
            expected.quality.macro_average_precision,
        ),
        (
            "MRR",
            observed.mean_reciprocal_rank,
            expected.quality.mean_reciprocal_rank,
        ),
    ] {
        if (left - right).abs() > 1.0e-9 {
            return Err(invalid_input(format!(
                "recomputed {name} for {method} ({left:.12}) disagrees with report ({right:.12})"
            )));
        }
    }
    Ok(())
}

fn benchmark_method<'a>(
    benchmark: &'a BenchmarkReport,
    method: &str,
) -> CliResult<&'a BenchmarkMethodReport> {
    benchmark
        .methods
        .iter()
        .find(|candidate| candidate.name == method)
        .ok_or_else(|| invalid_input(format!("benchmark has no method {method}")))
}

#[derive(Debug, Clone, Copy)]
struct BootstrapIntervals {
    micro_auprc: Option<DeltaInterval>,
    macro_auprc: Option<DeltaInterval>,
    mean_reciprocal_rank: Option<DeltaInterval>,
    recall_at_1: Option<DeltaInterval>,
}

fn bootstrap_deltas(
    grouped: &BTreeMap<&str, Vec<&ScoreRow>>,
    selected: &str,
    baseline: &str,
    samples: usize,
    confidence_level: f64,
    seed: u64,
) -> CliResult<BootstrapIntervals> {
    if samples == 0 {
        return Ok(BootstrapIntervals {
            micro_auprc: None,
            macro_auprc: None,
            mean_reciprocal_rank: None,
            recall_at_1: None,
        });
    }
    let groups = grouped.values().collect::<Vec<_>>();
    let mut rng = DeterministicRng::new(seed);
    let mut micro = Vec::with_capacity(samples);
    let mut macro_values = Vec::with_capacity(samples);
    let mut mrr = Vec::with_capacity(samples);
    let mut recall = Vec::with_capacity(samples);
    for sample in 0..samples {
        let mut selected_examples = Vec::new();
        let mut baseline_examples = Vec::new();
        for draw in 0..groups.len() {
            let group = groups[rng.range(groups.len())];
            let query_id = format!("bootstrap-{sample}-{draw}-{}", group[0].query_id);
            for row in group {
                selected_examples.push(GroupedLabeledScore {
                    query_id: query_id.clone(),
                    score: row.scores[selected],
                    label: row.label,
                });
                baseline_examples.push(GroupedLabeledScore {
                    query_id: query_id.clone(),
                    score: row.scores[baseline],
                    label: row.label,
                });
            }
        }
        let selected_report = grouped_evaluation_report(&selected_examples, evaluation_options())?;
        let baseline_report = grouped_evaluation_report(&baseline_examples, evaluation_options())?;
        micro.push(
            selected_report.micro.average_precision - baseline_report.micro.average_precision,
        );
        macro_values.push(
            selected_report.macro_average_precision - baseline_report.macro_average_precision,
        );
        mrr.push(
            selected_report.mean_reciprocal_rank - baseline_report.mean_reciprocal_rank,
        );
        recall.push(metric_at(&selected_report, 1) - metric_at(&baseline_report, 1));
    }
    Ok(BootstrapIntervals {
        micro_auprc: percentile_interval(micro, confidence_level),
        macro_auprc: percentile_interval(macro_values, confidence_level),
        mean_reciprocal_rank: percentile_interval(mrr, confidence_level),
        recall_at_1: percentile_interval(recall, confidence_level),
    })
}

fn percentile_interval(
    mut values: Vec<f64>,
    confidence_level: f64,
) -> Option<DeltaInterval> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let tail = (1.0 - confidence_level) / 2.0;
    Some(DeltaInterval {
        lower: percentile(&values, tail),
        upper: percentile(&values, 1.0 - tail),
        confidence_level,
        samples: values.len(),
    })
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let fraction = position - lower as f64;
        sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
    }
}

fn select_examples(
    grouped: &BTreeMap<&str, Vec<&ScoreRow>>,
    selected_method: &str,
    baselines: &[String],
    top_candidates: usize,
) -> Vec<IllustrativeExample> {
    let mut by_profile = BTreeMap::<String, Vec<IllustrativeExample>>::new();
    for group in grouped.values() {
        let selected = summarize_rank(group, selected_method, top_candidates);
        let baseline_summaries = baselines
            .iter()
            .map(|method| summarize_rank(group, method, top_candidates))
            .collect::<Vec<_>>();
        let best_baseline_rank = baseline_summaries
            .iter()
            .map(|summary| summary.positive_expected_rank)
            .fold(f64::INFINITY, f64::min);
        let first = group[0];
        by_profile
            .entry(first.profile.clone())
            .or_default()
            .push(IllustrativeExample {
                profile: first.profile.clone(),
                kind: String::new(),
                query_id: first.query_id.clone(),
                query_text: first.query_text.clone(),
                source_id: first.source_id.clone(),
                source_title: first.source_title.clone(),
                selected,
                baselines: baseline_summaries,
                expected_rank_improvement_over_best_baseline: best_baseline_rank
                    - summarize_rank(group, selected_method, 1).positive_expected_rank,
            });
    }
    let mut selected = Vec::new();
    for examples in by_profile.values_mut() {
        examples.sort_unstable_by(|left, right| {
            right
                .expected_rank_improvement_over_best_baseline
                .total_cmp(&left.expected_rank_improvement_over_best_baseline)
                .then_with(|| left.query_id.cmp(&right.query_id))
        });
        if let Some(mut improvement) = examples.first().cloned() {
            improvement.kind = "largest_rank_improvement".to_owned();
            selected.push(improvement);
        }
        examples.sort_unstable_by(|left, right| {
            right
                .selected
                .positive_expected_rank
                .total_cmp(&left.selected.positive_expected_rank)
                .then_with(|| left.query_id.cmp(&right.query_id))
        });
        if let Some(mut hardest) = examples.first().cloned()
            && !selected
                .iter()
                .any(|existing| existing.query_id == hardest.query_id)
        {
            hardest.kind = "hardest_selected_method_case".to_owned();
            selected.push(hardest);
        }
    }
    selected.sort_unstable_by(|left, right| {
        left.profile
            .cmp(&right.profile)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.query_id.cmp(&right.query_id))
    });
    selected
}

fn summarize_rank(
    group: &[&ScoreRow],
    method: &str,
    top_candidates: usize,
) -> MethodRankSummary {
    let positive_score = group
        .iter()
        .filter(|row| row.label)
        .map(|row| row.scores[method])
        .fold(f64::NEG_INFINITY, f64::max);
    let better = group
        .iter()
        .filter(|row| row.scores[method] > positive_score)
        .count();
    let tied = group
        .iter()
        .filter(|row| row.scores[method].total_cmp(&positive_score).is_eq())
        .count();
    let best_rank = better + 1;
    let worst_rank = better + tied.max(1);
    let expected_rank = (best_rank + worst_rank) as f64 / 2.0;
    let expected_reciprocal_rank = (best_rank..=worst_rank)
        .map(|rank| 1.0 / rank as f64)
        .sum::<f64>()
        / (worst_rank - best_rank + 1) as f64;
    let mut ranked = group.to_vec();
    ranked.sort_unstable_by(|left, right| {
        right.scores[method]
            .total_cmp(&left.scores[method])
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    MethodRankSummary {
        method: method.to_owned(),
        positive_best_rank: best_rank,
        positive_worst_rank: worst_rank,
        positive_expected_rank: expected_rank,
        positive_expected_reciprocal_rank: expected_reciprocal_rank,
        top_candidates: ranked
            .into_iter()
            .take(top_candidates)
            .map(|row| RankedCandidate {
                candidate_id: row.candidate_id.clone(),
                title: row.candidate_title.clone(),
                score: row.scores[method],
                label: row.label,
            })
            .collect(),
    }
}

fn render_examples(report: &EvidenceReport) -> String {
    let mut output = String::new();
    output.push_str("# FrankenOverlap empirical evidence\n\n");
    output.push_str(&format!("Corpus: `{}`  \n", report.corpus_id));
    output.push_str(&format!("Selected method: `{}`  \n", report.selected_method));
    output.push_str(&format!("Queries: {}  \n", report.queries));
    output.push_str(&format!("Candidate pairs: {}  \n", report.pairs));
    output.push_str(&format!("Verdict: **{}**\n\n", report.verdict.claim));
    output.push_str("## Method comparisons\n\n");
    output.push_str("| Baseline | Δ micro AUPRC | Δ macro AUPRC | Δ Recall@1 | p95 ratio | Quality gate |\n");
    output.push_str("|---|---:|---:|---:|---:|:---:|\n");
    for comparison in &report.comparisons {
        output.push_str(&format!(
            "| `{}` | {:+.6} | {:+.6} | {:+.6} | {:.3}× | {} |\n",
            comparison.baseline_method,
            comparison.micro_auprc_delta.point,
            comparison.macro_auprc_delta.point,
            comparison.recall_at_1_delta.point,
            comparison.timing.p95_speed_ratio,
            if comparison.quality_gate_passed { "yes" } else { "no" },
        ));
    }
    output.push_str("\nA p95 ratio above 1 means the selected method was faster. Quality and wall-time claims are intentionally reported separately.\n\n");
    output.push_str("## Illustrative query outcomes\n\n");
    for example in &report.examples {
        output.push_str(&format!(
            "### {} — {}\n\n",
            example.profile, example.kind
        ));
        output.push_str(&format!("Query ID: `{}`  \n", example.query_id));
        output.push_str(&format!("True source: `{}`", example.source_id));
        if let Some(title) = &example.source_title {
            output.push_str(&format!(" — {}", title));
        }
        output.push_str("  \n");
        if let Some(query_text) = &example.query_text {
            output.push_str("\n> ");
            output.push_str(&one_line(query_text, 600));
            output.push_str("\n\n");
        }
        output.push_str(&format!(
            "Selected-method expected source rank: **{:.2}** (range {}–{}).  \n",
            example.selected.positive_expected_rank,
            example.selected.positive_best_rank,
            example.selected.positive_worst_rank,
        ));
        output.push_str(&format!(
            "Improvement over the best baseline expected rank: **{:+.2}**.\n\n",
            example.expected_rank_improvement_over_best_baseline,
        ));
        render_rank_table(&mut output, &example.selected);
        for baseline in &example.baselines {
            render_rank_table(&mut output, baseline);
        }
    }
    output.push_str("## Interpretation\n\n");
    output.push_str("This bundle proves only what its recorded corpus snapshot, query generation, score stream, compiler, and host measurements support. It does not convert an architectural expectation into an empirical claim. Re-run the benchmark and regenerate this bundle after every material retrieval or ranking change.\n");
    output
}

fn render_rank_table(output: &mut String, summary: &MethodRankSummary) {
    output.push_str(&format!("#### `{}`\n\n", summary.method));
    output.push_str("| Rank | Candidate | Score | Relevant |\n");
    output.push_str("|---:|---|---:|:---:|\n");
    for (rank, candidate) in summary.top_candidates.iter().enumerate() {
        let display = candidate.title.as_deref().unwrap_or(&candidate.candidate_id);
        output.push_str(&format!(
            "| {} | {} | {:.6} | {} |\n",
            rank + 1,
            markdown_cell(display),
            candidate.score,
            if candidate.label { "yes" } else { "" },
        ));
    }
    output.push('\n');
}

fn write_bundle_manifest(root: &Path, paths: &[&Path]) -> CliResult<BundleManifest> {
    let mut files = Vec::new();
    for path in paths {
        let bytes = fs::read(path)?;
        files.push(BundleFile {
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
    let manifest = BundleManifest {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        files,
    };
    let path = root.join("manifest.json");
    atomic_write(&path, &serde_json::to_vec_pretty(&manifest)?)?;
    let mut sums = String::new();
    for file in &manifest.files {
        sums.push_str(&format!("{}  {}\n", file.sha256, file.path));
    }
    sums.push_str(&format!(
        "{}  manifest.json\n",
        sha256_hex(&fs::read(&path)?)
    ));
    atomic_write(&root.join("SHA256SUMS"), sums.as_bytes())?;
    Ok(manifest)
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        if numerator > 0.0 { f64::INFINITY } else { 1.0 }
    } else {
        numerator / denominator
    }
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn one_line(value: &str, maximum_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_chars {
        compact
    } else {
        compact.chars().take(maximum_chars).collect::<String>() + "…"
    }
}

fn markdown_cell(value: &str) -> String {
    one_line(value, 120).replace('|', "\\|")
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[derive(Debug, Clone, Copy)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next() % upper as u64) as usize
        }
    }
}
