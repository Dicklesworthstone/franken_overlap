use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use fo_core::{
    GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore,
    grouped_evaluation_report,
};
use serde::{Deserialize, Serialize};

pub type EvidenceResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_provider: String,
    pub corpus_manifest_documents: usize,
    pub indexed_documents: usize,
    pub source_documents: usize,
    pub queries: usize,
    pub pairs: usize,
    pub seed: u64,
    pub profiles: Vec<String>,
    pub build: BuildReport,
    pub methods: Vec<BenchmarkMethodReport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BuildReport {
    pub build_ms: f64,
    pub serialization_ms: f64,
    pub index_bytes: u64,
    pub overlap_fingerprints: usize,
    pub overlap_postings: usize,
    pub lexical_terms: usize,
    pub lexical_postings: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkMethodReport {
    pub name: String,
    pub queries_per_second: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub quality: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScoreRow {
    pub query_id: String,
    pub profile: String,
    pub source_id: String,
    pub candidate_id: String,
    pub label: bool,
    pub scores: BTreeMap<String, f64>,
    #[serde(default)]
    pub query_text: Option<String>,
    #[serde(default)]
    pub source_title: Option<String>,
    #[serde(default)]
    pub candidate_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EvidenceOptions {
    pub selected_method: String,
    pub baselines: Vec<String>,
    pub bootstrap_samples: usize,
    pub confidence_level: f64,
    pub seed: u64,
    pub top_candidates: usize,
    pub minimum_macro_delta_lower_bound: f64,
    pub maximum_recall_at_1_regression: f64,
}

impl EvidenceOptions {
    pub fn validate(&self) -> EvidenceResult<()> {
        if self.selected_method.trim().is_empty() || self.top_candidates == 0 {
            return Err(invalid(
                "selected method and top-candidates must be nonempty",
            ));
        }
        if self.bootstrap_samples > 100_000 {
            return Err(invalid("bootstrap-samples must not exceed 100000"));
        }
        if !self.confidence_level.is_finite()
            || self.confidence_level <= 0.0
            || self.confidence_level >= 1.0
            || !self.minimum_macro_delta_lower_bound.is_finite()
            || !self.maximum_recall_at_1_regression.is_finite()
            || self.maximum_recall_at_1_regression < 0.0
        {
            return Err(invalid("evidence thresholds are invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MetricSnapshot {
    pub micro_auprc: f64,
    pub macro_auprc: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DeltaInterval {
    pub lower: f64,
    pub upper: f64,
    pub confidence_level: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MetricDelta {
    pub point: f64,
    pub interval: Option<DeltaInterval>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimingComparison {
    pub selected_p50_ms: f64,
    pub selected_p95_ms: f64,
    pub selected_p99_ms: f64,
    pub baseline_p50_ms: f64,
    pub baseline_p95_ms: f64,
    pub baseline_p99_ms: f64,
    pub p95_speed_ratio: f64,
    pub selected_queries_per_second: f64,
    pub baseline_queries_per_second: f64,
    pub throughput_ratio: f64,
    pub selected_is_faster_at_p95: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodComparison {
    pub baseline_method: String,
    pub selected: MetricSnapshot,
    pub baseline: MetricSnapshot,
    pub micro_auprc_delta: MetricDelta,
    pub macro_auprc_delta: MetricDelta,
    pub mean_reciprocal_rank_delta: MetricDelta,
    pub recall_at_1_delta: MetricDelta,
    pub timing: TimingComparison,
    pub quality_gate_passed: bool,
    pub strict_quality_and_p95_dominance: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RankedCandidate {
    pub candidate_id: String,
    pub title: Option<String>,
    pub score: f64,
    pub relevant: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodRankSummary {
    pub method: String,
    pub positive_best_rank: usize,
    pub positive_worst_rank: usize,
    pub positive_expected_rank: f64,
    pub positive_expected_reciprocal_rank: f64,
    pub top_candidates: Vec<RankedCandidate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IllustrativeExample {
    pub profile: String,
    pub kind: String,
    pub query_id: String,
    pub query_text: Option<String>,
    pub source_id: String,
    pub source_title: Option<String>,
    pub selected: MethodRankSummary,
    pub baselines: Vec<MethodRankSummary>,
    pub expected_rank_improvement_over_best_baseline: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceVerdict {
    pub best_baseline: String,
    pub quality_better_than_best_baseline: bool,
    pub faster_than_best_baseline_at_p95: bool,
    pub strictly_dominates_best_baseline: bool,
    pub faster_than_at_least_one_baseline_at_p95: bool,
    pub claim: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub benchmark_schema_version: u32,
    pub benchmark_generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_provider: String,
    pub corpus_manifest_documents: usize,
    pub indexed_documents: usize,
    pub source_documents: usize,
    pub queries: usize,
    pub pairs: usize,
    pub minimum_candidates_per_query: usize,
    pub maximum_candidates_per_query: usize,
    pub positives: usize,
    pub seed: u64,
    pub profiles: Vec<String>,
    pub selected_method: String,
    pub baselines: Vec<String>,
    pub build: BuildReport,
    pub comparisons: Vec<MethodComparison>,
    pub examples: Vec<IllustrativeExample>,
    pub verdict: EvidenceVerdict,
}

pub fn read_score_rows(path: &Path) -> EvidenceResult<Vec<ScoreRow>> {
    let file = File::open(path)?;
    let mut rows = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        rows.push(
            serde_json::from_str::<ScoreRow>(value).map_err(|error| {
                invalid(format!("{}:{}: {error}", path.display(), line_index + 1))
            })?,
        );
    }
    if rows.is_empty() {
        return Err(invalid(format!(
            "{} contains no score rows",
            path.display()
        )));
    }
    Ok(rows)
}

pub fn build_evidence(
    benchmark: &BenchmarkReport,
    rows: &[ScoreRow],
    options: &EvidenceOptions,
    generated_at_unix: u64,
) -> EvidenceResult<EvidenceReport> {
    options.validate()?;
    let grouped = validate_inputs(benchmark, rows)?;
    let method_names = benchmark
        .methods
        .iter()
        .map(|method| method.name.clone())
        .collect::<BTreeSet<_>>();
    if !method_names.contains(&options.selected_method) {
        return Err(invalid(format!(
            "selected method {:?} is absent from the report",
            options.selected_method
        )));
    }
    let baselines = resolved_baselines(options, &method_names)?;
    let selected_report = evaluate(rows, &options.selected_method)?;
    verify_report_consistency(benchmark, &options.selected_method, &selected_report)?;
    let selected = snapshot(&selected_report);
    let mut comparisons = Vec::with_capacity(baselines.len());
    for baseline_name in &baselines {
        let baseline_report = evaluate(rows, baseline_name)?;
        verify_report_consistency(benchmark, baseline_name, &baseline_report)?;
        let baseline = snapshot(&baseline_report);
        let intervals = bootstrap(
            &grouped,
            &options.selected_method,
            baseline_name,
            options.bootstrap_samples,
            options.confidence_level,
            options.seed ^ stable_hash(baseline_name),
        )?;
        let selected_timing = method_report(benchmark, &options.selected_method)?;
        let baseline_timing = method_report(benchmark, baseline_name)?;
        let timing = TimingComparison {
            selected_p50_ms: selected_timing.p50_ms,
            selected_p95_ms: selected_timing.p95_ms,
            selected_p99_ms: selected_timing.p99_ms,
            baseline_p50_ms: baseline_timing.p50_ms,
            baseline_p95_ms: baseline_timing.p95_ms,
            baseline_p99_ms: baseline_timing.p99_ms,
            p95_speed_ratio: safe_ratio(baseline_timing.p95_ms, selected_timing.p95_ms),
            selected_queries_per_second: selected_timing.queries_per_second,
            baseline_queries_per_second: baseline_timing.queries_per_second,
            throughput_ratio: safe_ratio(
                selected_timing.queries_per_second,
                baseline_timing.queries_per_second,
            ),
            selected_is_faster_at_p95: selected_timing.p95_ms < baseline_timing.p95_ms,
        };
        let macro_point = selected.macro_auprc - baseline.macro_auprc;
        let recall_point = selected.recall_at_1 - baseline.recall_at_1;
        let macro_lower = intervals
            .macro_auprc
            .map_or(macro_point, |interval| interval.lower);
        let quality_gate_passed = macro_lower >= options.minimum_macro_delta_lower_bound
            && recall_point >= -options.maximum_recall_at_1_regression;
        comparisons.push(MethodComparison {
            baseline_method: baseline_name.clone(),
            selected,
            baseline,
            micro_auprc_delta: MetricDelta {
                point: selected.micro_auprc - baseline.micro_auprc,
                interval: intervals.micro_auprc,
            },
            macro_auprc_delta: MetricDelta {
                point: macro_point,
                interval: intervals.macro_auprc,
            },
            mean_reciprocal_rank_delta: MetricDelta {
                point: selected.mean_reciprocal_rank - baseline.mean_reciprocal_rank,
                interval: intervals.mean_reciprocal_rank,
            },
            recall_at_1_delta: MetricDelta {
                point: recall_point,
                interval: intervals.recall_at_1,
            },
            strict_quality_and_p95_dominance: quality_gate_passed
                && timing.selected_is_faster_at_p95,
            quality_gate_passed,
            timing,
        });
    }
    comparisons.sort_unstable_by(|left, right| left.baseline_method.cmp(&right.baseline_method));
    let examples = choose_examples(
        &grouped,
        &options.selected_method,
        &baselines,
        options.top_candidates,
    );
    let best = comparisons
        .iter()
        .max_by(|left, right| {
            left.baseline
                .macro_auprc
                .total_cmp(&right.baseline.macro_auprc)
                .then_with(|| {
                    left.baseline
                        .micro_auprc
                        .total_cmp(&right.baseline.micro_auprc)
                })
        })
        .ok_or_else(|| invalid("no baseline comparison was produced"))?;
    let best_baseline = best.baseline_method.clone();
    let quality_better = best.quality_gate_passed;
    let faster_best = best.timing.selected_is_faster_at_p95;
    let dominates = best.strict_quality_and_p95_dominance;
    let faster_any = comparisons
        .iter()
        .any(|comparison| comparison.timing.selected_is_faster_at_p95);
    let claim = if dominates {
        "quality_and_wall_time_superiority_supported"
    } else if quality_better {
        "quality_superiority_supported_wall_time_not_dominant"
    } else {
        "superiority_not_established"
    };
    let candidate_counts = grouped.values().map(Vec::len).collect::<Vec<_>>();
    Ok(EvidenceReport {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        generated_at_unix,
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
        selected_method: options.selected_method.clone(),
        baselines,
        build: benchmark.build.clone(),
        comparisons,
        examples,
        verdict: EvidenceVerdict {
            best_baseline,
            quality_better_than_best_baseline: quality_better,
            faster_than_best_baseline_at_p95: faster_best,
            strictly_dominates_best_baseline: dominates,
            faster_than_at_least_one_baseline_at_p95: faster_any,
            claim: claim.to_owned(),
        },
    })
}

pub fn render_markdown(report: &EvidenceReport) -> String {
    let mut out = String::new();
    out.push_str("# FrankenOverlap empirical evidence\n\n");
    out.push_str(&format!("Corpus: `{}`  \n", report.corpus_id));
    out.push_str(&format!(
        "Selected method: `{}`  \n",
        report.selected_method
    ));
    out.push_str(&format!(
        "Queries / pairs: {} / {}  \n",
        report.queries, report.pairs
    ));
    out.push_str(&format!("Verdict: **{}**\n\n", report.verdict.claim));
    out.push_str("## Method comparisons\n\n");
    out.push_str("| Baseline | Δ micro AUPRC | Δ macro AUPRC | Δ Recall@1 | p95 speed ratio | Quality gate |\n");
    out.push_str("|---|---:|---:|---:|---:|:---:|\n");
    for comparison in &report.comparisons {
        out.push_str(&format!(
            "| `{}` | {:+.6} | {:+.6} | {:+.6} | {:.3}× | {} |\n",
            comparison.baseline_method,
            comparison.micro_auprc_delta.point,
            comparison.macro_auprc_delta.point,
            comparison.recall_at_1_delta.point,
            comparison.timing.p95_speed_ratio,
            if comparison.quality_gate_passed {
                "yes"
            } else {
                "no"
            },
        ));
    }
    out.push_str("\nA p95 ratio above 1 means the selected method was faster. Quality and wall time are deliberately separate claims.\n\n");
    out.push_str("## Illustrative outcomes\n\n");
    for example in &report.examples {
        out.push_str(&format!("### {} — {}\n\n", example.profile, example.kind));
        out.push_str(&format!("Query: `{}`  \n", example.query_id));
        out.push_str(&format!("Source: `{}`", example.source_id));
        if let Some(title) = &example.source_title {
            out.push_str(&format!(" — {}", title));
        }
        out.push_str("  \n");
        if let Some(text) = &example.query_text {
            out.push_str(&format!("\n> {}\n\n", one_line(text, 600)));
        }
        out.push_str(&format!(
            "Selected expected rank: **{:.2}** ({}–{}); improvement over best baseline: **{:+.2}**.\n\n",
            example.selected.positive_expected_rank,
            example.selected.positive_best_rank,
            example.selected.positive_worst_rank,
            example.expected_rank_improvement_over_best_baseline,
        ));
        render_ranks(&mut out, &example.selected);
        for baseline in &example.baselines {
            render_ranks(&mut out, baseline);
        }
    }
    out.push_str("## Scope of the claim\n\n");
    out.push_str("This document proves only what the recorded corpus, score stream, query groups, compiler, host, and thresholds support. It must be regenerated after material retrieval or ranking changes.\n");
    out
}

fn resolved_baselines(
    options: &EvidenceOptions,
    methods: &BTreeSet<String>,
) -> EvidenceResult<Vec<String>> {
    let mut values = if options.baselines.is_empty() {
        methods
            .iter()
            .filter(|method| method.as_str() != options.selected_method.as_str())
            .cloned()
            .collect::<Vec<_>>()
    } else {
        options.baselines.clone()
    };
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        return Err(invalid("at least one baseline is required"));
    }
    for value in &values {
        if value == &options.selected_method || !methods.contains(value) {
            return Err(invalid(format!("invalid baseline method {value:?}")));
        }
    }
    Ok(values)
}

fn validate_inputs<'a>(
    benchmark: &BenchmarkReport,
    rows: &'a [ScoreRow],
) -> EvidenceResult<BTreeMap<&'a str, Vec<&'a ScoreRow>>> {
    if benchmark.queries == 0 || benchmark.pairs == 0 || benchmark.methods.is_empty() {
        return Err(invalid("benchmark report is empty"));
    }
    if benchmark.pairs != rows.len() {
        return Err(invalid(format!(
            "report declares {} pairs but score stream has {}",
            benchmark.pairs,
            rows.len()
        )));
    }
    let methods = benchmark
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut grouped = BTreeMap::<&str, Vec<&ScoreRow>>::new();
    let mut seen = BTreeSet::<(&str, &str)>::new();
    for (index, row) in rows.iter().enumerate() {
        if row.query_id.trim().is_empty()
            || row.profile.trim().is_empty()
            || row.source_id.trim().is_empty()
            || row.candidate_id.trim().is_empty()
        {
            return Err(invalid(format!(
                "score row {index} has an empty identifier"
            )));
        }
        if !seen.insert((&row.query_id, &row.candidate_id)) {
            return Err(invalid(format!(
                "duplicate pair {}/{}",
                row.query_id, row.candidate_id
            )));
        }
        if row
            .scores
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != methods
        {
            return Err(invalid(format!(
                "score row {index} method set disagrees with report"
            )));
        }
        if row
            .scores
            .values()
            .any(|score| !score.is_finite() || !(0.0..=1.0).contains(score))
        {
            return Err(invalid(format!(
                "score row {index} contains an invalid score"
            )));
        }
        grouped.entry(&row.query_id).or_default().push(row);
    }
    if grouped.len() != benchmark.queries {
        return Err(invalid(format!(
            "report declares {} queries but score stream has {}",
            benchmark.queries,
            grouped.len()
        )));
    }
    for (query, group) in &grouped {
        let first = group[0];
        if group
            .iter()
            .any(|row| row.profile != first.profile || row.source_id != first.source_id)
            || !group.iter().any(|row| row.label)
        {
            return Err(invalid(format!(
                "query group {query} is inconsistent or has no positive"
            )));
        }
    }
    Ok(grouped)
}

fn evaluate(rows: &[ScoreRow], method: &str) -> EvidenceResult<GroupedEvaluationReport> {
    let examples = rows
        .iter()
        .map(|row| {
            Ok(GroupedLabeledScore {
                query_id: row.query_id.clone(),
                score: *row
                    .scores
                    .get(method)
                    .ok_or_else(|| invalid(format!("missing method {method}")))?,
                label: row.label,
            })
        })
        .collect::<EvidenceResult<Vec<_>>>()?;
    Ok(grouped_evaluation_report(&examples, evaluation_options())?)
}

fn verify_report_consistency(
    benchmark: &BenchmarkReport,
    method: &str,
    observed: &GroupedEvaluationReport,
) -> EvidenceResult<()> {
    let expected = method_report(benchmark, method)?;
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
            return Err(invalid(format!(
                "recomputed {name} for {method} ({left:.12}) disagrees with report ({right:.12})"
            )));
        }
    }
    Ok(())
}

fn method_report<'a>(
    benchmark: &'a BenchmarkReport,
    method: &str,
) -> EvidenceResult<&'a BenchmarkMethodReport> {
    benchmark
        .methods
        .iter()
        .find(|candidate| candidate.name == method)
        .ok_or_else(|| invalid(format!("benchmark has no method {method}")))
}

fn snapshot(report: &GroupedEvaluationReport) -> MetricSnapshot {
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

#[derive(Debug, Clone, Copy)]
struct BootstrapIntervals {
    micro_auprc: Option<DeltaInterval>,
    macro_auprc: Option<DeltaInterval>,
    mean_reciprocal_rank: Option<DeltaInterval>,
    recall_at_1: Option<DeltaInterval>,
}

fn bootstrap(
    grouped: &BTreeMap<&str, Vec<&ScoreRow>>,
    selected: &str,
    baseline: &str,
    samples: usize,
    confidence: f64,
    seed: u64,
) -> EvidenceResult<BootstrapIntervals> {
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
        let mut selected_rows = Vec::new();
        let mut baseline_rows = Vec::new();
        for draw in 0..groups.len() {
            let group = groups[rng.range(groups.len())];
            let query_id = format!("bootstrap-{sample}-{draw}");
            for row in group {
                selected_rows.push(GroupedLabeledScore {
                    query_id: query_id.clone(),
                    score: row.scores[selected],
                    label: row.label,
                });
                baseline_rows.push(GroupedLabeledScore {
                    query_id: query_id.clone(),
                    score: row.scores[baseline],
                    label: row.label,
                });
            }
        }
        let selected_report = grouped_evaluation_report(&selected_rows, evaluation_options())?;
        let baseline_report = grouped_evaluation_report(&baseline_rows, evaluation_options())?;
        micro.push(
            selected_report.micro.average_precision - baseline_report.micro.average_precision,
        );
        macro_values.push(
            selected_report.macro_average_precision - baseline_report.macro_average_precision,
        );
        mrr.push(selected_report.mean_reciprocal_rank - baseline_report.mean_reciprocal_rank);
        recall.push(metric_at(&selected_report, 1) - metric_at(&baseline_report, 1));
    }
    Ok(BootstrapIntervals {
        micro_auprc: interval(micro, confidence),
        macro_auprc: interval(macro_values, confidence),
        mean_reciprocal_rank: interval(mrr, confidence),
        recall_at_1: interval(recall, confidence),
    })
}

fn interval(mut values: Vec<f64>, confidence: f64) -> Option<DeltaInterval> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let tail = (1.0 - confidence) / 2.0;
    Some(DeltaInterval {
        lower: percentile(&values, tail),
        upper: percentile(&values, 1.0 - tail),
        confidence_level: confidence,
        samples: values.len(),
    })
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
}

fn choose_examples(
    grouped: &BTreeMap<&str, Vec<&ScoreRow>>,
    selected_method: &str,
    baselines: &[String],
    top_candidates: usize,
) -> Vec<IllustrativeExample> {
    let mut profiles = BTreeMap::<String, Vec<IllustrativeExample>>::new();
    for group in grouped.values() {
        let selected = rank_summary(group, selected_method, top_candidates);
        let baseline_summaries = baselines
            .iter()
            .map(|method| rank_summary(group, method, top_candidates))
            .collect::<Vec<_>>();
        let best_baseline_rank = baseline_summaries
            .iter()
            .map(|summary| summary.positive_expected_rank)
            .fold(f64::INFINITY, f64::min);
        let first = group[0];
        profiles
            .entry(first.profile.clone())
            .or_default()
            .push(IllustrativeExample {
                profile: first.profile.clone(),
                kind: String::new(),
                query_id: first.query_id.clone(),
                query_text: first.query_text.clone(),
                source_id: first.source_id.clone(),
                source_title: first.source_title.clone(),
                expected_rank_improvement_over_best_baseline: best_baseline_rank
                    - selected.positive_expected_rank,
                selected,
                baselines: baseline_summaries,
            });
    }
    let mut output = Vec::new();
    for examples in profiles.values_mut() {
        examples.sort_unstable_by(|left, right| {
            right
                .expected_rank_improvement_over_best_baseline
                .total_cmp(&left.expected_rank_improvement_over_best_baseline)
                .then_with(|| left.query_id.cmp(&right.query_id))
        });
        if let Some(mut value) = examples.first().cloned() {
            value.kind = "largest_rank_improvement".to_owned();
            output.push(value);
        }
        examples.sort_unstable_by(|left, right| {
            right
                .selected
                .positive_expected_rank
                .total_cmp(&left.selected.positive_expected_rank)
                .then_with(|| left.query_id.cmp(&right.query_id))
        });
        if let Some(mut value) = examples.first().cloned()
            && !output
                .iter()
                .any(|existing| existing.query_id == value.query_id)
        {
            value.kind = "hardest_selected_method_case".to_owned();
            output.push(value);
        }
    }
    output.sort_unstable_by(|left, right| {
        left.profile
            .cmp(&right.profile)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    output
}

fn rank_summary(group: &[&ScoreRow], method: &str, top_candidates: usize) -> MethodRankSummary {
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
        .count()
        .max(1);
    let best = better + 1;
    let worst = better + tied;
    let expected_reciprocal_rank =
        (best..=worst).map(|rank| 1.0 / rank as f64).sum::<f64>() / (worst - best + 1) as f64;
    let mut ranked = group.to_vec();
    ranked.sort_unstable_by(|left, right| {
        right.scores[method]
            .total_cmp(&left.scores[method])
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    MethodRankSummary {
        method: method.to_owned(),
        positive_best_rank: best,
        positive_worst_rank: worst,
        positive_expected_rank: (best + worst) as f64 / 2.0,
        positive_expected_reciprocal_rank: expected_reciprocal_rank,
        top_candidates: ranked
            .into_iter()
            .take(top_candidates)
            .map(|row| RankedCandidate {
                candidate_id: row.candidate_id.clone(),
                title: row.candidate_title.clone(),
                score: row.scores[method],
                relevant: row.label,
            })
            .collect(),
    }
}

fn render_ranks(out: &mut String, summary: &MethodRankSummary) {
    out.push_str(&format!("#### `{}`\n\n", summary.method));
    out.push_str("| Rank | Candidate | Score | Relevant |\n");
    out.push_str("|---:|---|---:|:---:|\n");
    for (rank, candidate) in summary.top_candidates.iter().enumerate() {
        let name = candidate
            .title
            .as_deref()
            .unwrap_or(&candidate.candidate_id);
        out.push_str(&format!(
            "| {} | {} | {:.6} | {} |\n",
            rank + 1,
            markdown_cell(name),
            candidate.score,
            if candidate.relevant { "yes" } else { "" },
        ));
    }
    out.push('\n');
}

fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    numerator.max(0.0) / denominator.max(1.0e-12)
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn one_line(value: &str, maximum: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum {
        compact
    } else {
        compact.chars().take(maximum).collect::<String>() + "…"
    }
}

fn markdown_cell(value: &str) -> String {
    one_line(value, 120).replace('|', "\\|")
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
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

#[cfg(test)]
mod tests {
    use super::{interval, safe_ratio};

    #[test]
    fn percentile_interval_is_order_independent() {
        let interval = interval(vec![3.0, 1.0, 2.0, 4.0], 0.50).expect("interval");
        assert!(interval.lower <= interval.upper);
        assert_eq!(interval.samples, 4);
    }

    #[test]
    fn ratios_remain_finite_for_zero_duration() {
        assert!(safe_ratio(1.0, 0.0).is_finite());
    }
}
