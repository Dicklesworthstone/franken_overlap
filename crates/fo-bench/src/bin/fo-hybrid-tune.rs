#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_core::{
    GroupedEvaluationOptions, GroupedLabeledScore, HybridFusionProfile, HybridMetricSnapshot,
    HYBRID_FUSION_PROFILE_SCHEMA_VERSION, grouped_evaluation_report,
};
use serde::{Deserialize, Serialize};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const DEFAULT_LEXICAL_METHOD: &str = "fielded_bm25_phrase_proximity";
const DEFAULT_OVERLAP_METHOD: &str = "franken_overlap";
const DEFAULT_BASELINE_METHOD: &str = "franken_hybrid";

#[derive(Debug, Parser)]
#[command(
    name = "fo-hybrid-tune",
    version,
    about = "Tune and apply hybrid lexical/overlap/RRF fusion profiles"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Fit a profile using deterministic query-group train/validation/test splits.
    Fit(FitCommand),
    /// Apply a fitted profile to a score stream and emit grouped tuned scores.
    Apply(ApplyCommand),
    /// Evaluate a fitted profile against a named baseline on one score stream.
    Compare(CompareCommand),
}

#[derive(Debug, Args)]
struct FitCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_LEXICAL_METHOD)]
    lexical_method: String,
    #[arg(long, default_value = DEFAULT_OVERLAP_METHOD)]
    overlap_method: String,
    #[arg(long, default_value = DEFAULT_BASELINE_METHOD)]
    baseline_method: String,
    #[arg(long, default_value_t = 0x74_75_6e_65_2d_68_79_62)]
    seed: u64,
    #[arg(long, default_value_t = 32)]
    shortlist: usize,
    #[arg(long, default_value_t = 0.0)]
    require_test_micro_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    require_test_macro_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    require_test_recall_at_1_delta: f64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ApplyCommand {
    input: PathBuf,
    profile: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value = DEFAULT_LEXICAL_METHOD)]
    lexical_method: String,
    #[arg(long, default_value = DEFAULT_OVERLAP_METHOD)]
    overlap_method: String,
}

#[derive(Debug, Args)]
struct CompareCommand {
    input: PathBuf,
    profile: PathBuf,
    #[arg(long, default_value = DEFAULT_LEXICAL_METHOD)]
    lexical_method: String,
    #[arg(long, default_value = DEFAULT_OVERLAP_METHOD)]
    overlap_method: String,
    #[arg(long, default_value = DEFAULT_BASELINE_METHOD)]
    baseline_method: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ScoreRow {
    query_id: String,
    #[serde(default)]
    candidate_id: String,
    label: bool,
    scores: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
struct PreparedRow {
    query_id: String,
    label: bool,
    lexical_score: f64,
    overlap_score: f64,
    lexical_rank_score: f64,
    overlap_rank_score: f64,
    baseline_score: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct FusionParameters {
    lexical_weight: f64,
    overlap_weight: f64,
    rrf_weight: f64,
    rrf_constant: f64,
}

#[derive(Debug, Clone)]
struct CandidateEvaluation {
    parameters: FusionParameters,
    metrics: HybridMetricSnapshot,
}

#[derive(Debug, Serialize)]
struct TuningReport {
    schema_version: u32,
    input: String,
    input_fingerprint: String,
    rows: usize,
    queries: usize,
    train_queries: usize,
    validation_queries: usize,
    test_queries: usize,
    lexical_method: String,
    overlap_method: String,
    baseline_method: String,
    evaluated_configurations: usize,
    shortlist: usize,
    selected: FusionParameters,
    train_metrics: HybridMetricSnapshot,
    validation_metrics: HybridMetricSnapshot,
    test_metrics: HybridMetricSnapshot,
    baseline_test_metrics: HybridMetricSnapshot,
    test_micro_delta: f64,
    test_macro_delta: f64,
    test_mrr_delta: f64,
    test_recall_at_1_delta: f64,
}

#[derive(Debug, Serialize)]
struct ComparisonReport {
    tuned: HybridMetricSnapshot,
    baseline: HybridMetricSnapshot,
    micro_delta: f64,
    macro_delta: f64,
    mrr_delta: f64,
    recall_at_1_delta: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-hybrid-tune: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Fit(command) => run_fit(command),
        Command::Apply(command) => run_apply(command),
        Command::Compare(command) => run_compare(command),
    }
}

fn run_fit(command: FitCommand) -> CliResult<()> {
    validate_fit_command(&command)?;
    let input_bytes = fs::read(&command.input)?;
    let rows = read_rows(&input_bytes, &command.input)?;
    let prepared = prepare_rows(
        &rows,
        &command.lexical_method,
        &command.overlap_method,
        Some(&command.baseline_method),
    )?;
    let splits = query_splits(&prepared, command.seed)?;
    let configurations = parameter_grid();
    let mut train_evaluations = configurations
        .iter()
        .copied()
        .map(|parameters| {
            Ok(CandidateEvaluation {
                parameters,
                metrics: evaluate(&prepared, &splits.train, parameters)?,
            })
        })
        .collect::<fo_core::Result<Vec<_>>>()?;
    train_evaluations.sort_unstable_by(|left, right| {
        compare_candidate_evaluations(right, left)
    });
    train_evaluations.truncate(command.shortlist.min(train_evaluations.len()));
    let selected = train_evaluations
        .iter()
        .map(|candidate| {
            Ok(CandidateEvaluation {
                parameters: candidate.parameters,
                metrics: evaluate(&prepared, &splits.validation, candidate.parameters)?,
            })
        })
        .collect::<fo_core::Result<Vec<_>>>()?
        .into_iter()
        .max_by(compare_candidate_evaluations)
        .ok_or_else(|| invalid_input("tuning produced no candidate configurations"))?;

    let train_metrics = evaluate(&prepared, &splits.train, selected.parameters)?;
    let validation_metrics = selected.metrics;
    let test_metrics = evaluate(&prepared, &splits.test, selected.parameters)?;
    let baseline_test_metrics = evaluate_baseline(&prepared, &splits.test)?;
    let fingerprint = format!("{:016x}", fnv1a64(&input_bytes));
    let profile = HybridFusionProfile {
        schema_version: HYBRID_FUSION_PROFILE_SCHEMA_VERSION,
        name: format!("tuned-{}-{}", command.lexical_method, command.overlap_method),
        lexical_weight: selected.parameters.lexical_weight as f32,
        overlap_weight: selected.parameters.overlap_weight as f32,
        rrf_weight: selected.parameters.rrf_weight as f32,
        rrf_constant: selected.parameters.rrf_constant as f32,
        lexical_saturation: 4.0,
        agreement_bonus: 0.0,
        phrase_bonus: 0.0,
        candidate_multiplier: 8,
        minimum_score: 0.0,
        trained_from: Some(fingerprint.clone()),
        train_queries: splits.train.len(),
        validation_queries: splits.validation.len(),
        test_queries: splits.test.len(),
        validation_metrics: Some(validation_metrics),
        test_metrics: Some(test_metrics),
        baseline_test_metrics: Some(baseline_test_metrics),
    };
    profile.validate()?;
    write_pretty_json(&command.output, &profile)?;

    let report = TuningReport {
        schema_version: 1,
        input: command.input.display().to_string(),
        input_fingerprint: fingerprint,
        rows: prepared.len(),
        queries: splits.train.len() + splits.validation.len() + splits.test.len(),
        train_queries: splits.train.len(),
        validation_queries: splits.validation.len(),
        test_queries: splits.test.len(),
        lexical_method: command.lexical_method,
        overlap_method: command.overlap_method,
        baseline_method: command.baseline_method,
        evaluated_configurations: configurations.len(),
        shortlist: train_evaluations.len(),
        selected: selected.parameters,
        train_metrics,
        validation_metrics,
        test_metrics,
        baseline_test_metrics,
        test_micro_delta: test_metrics.micro_auprc - baseline_test_metrics.micro_auprc,
        test_macro_delta: test_metrics.macro_auprc - baseline_test_metrics.macro_auprc,
        test_mrr_delta: test_metrics.mean_reciprocal_rank
            - baseline_test_metrics.mean_reciprocal_rank,
        test_recall_at_1_delta: test_metrics.recall_at_1 - baseline_test_metrics.recall_at_1,
    };
    if let Some(path) = &command.report {
        write_pretty_json(path, &report)?;
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_tuning_report(&report, &command.output);
    }
    enforce_fit_gates(&command, &report)
}

fn run_apply(command: ApplyCommand) -> CliResult<()> {
    let rows = read_rows(&fs::read(&command.input)?, &command.input)?;
    let prepared = prepare_rows(
        &rows,
        &command.lexical_method,
        &command.overlap_method,
        None,
    )?;
    let profile = read_profile(&command.profile)?;
    let scores = score_rows(&prepared, profile_parameters(&profile), None);
    write_grouped_scores(&command.output, &scores)?;
    println!(
        "Wrote {} tuned scores to {}",
        scores.len(),
        command.output.display()
    );
    Ok(())
}

fn run_compare(command: CompareCommand) -> CliResult<()> {
    let rows = read_rows(&fs::read(&command.input)?, &command.input)?;
    let prepared = prepare_rows(
        &rows,
        &command.lexical_method,
        &command.overlap_method,
        Some(&command.baseline_method),
    )?;
    let profile = read_profile(&command.profile)?;
    let all_queries = prepared
        .iter()
        .map(|row| row.query_id.clone())
        .collect::<BTreeSet<_>>();
    let tuned = evaluate(&prepared, &all_queries, profile_parameters(&profile))?;
    let baseline = evaluate_baseline(&prepared, &all_queries)?;
    let report = ComparisonReport {
        micro_delta: tuned.micro_auprc - baseline.micro_auprc,
        macro_delta: tuned.macro_auprc - baseline.macro_auprc,
        mrr_delta: tuned.mean_reciprocal_rank - baseline.mean_reciprocal_rank,
        recall_at_1_delta: tuned.recall_at_1 - baseline.recall_at_1,
        tuned,
        baseline,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Tuned micro AUPRC:    {:.6}", report.tuned.micro_auprc);
        println!("Baseline micro AUPRC: {:.6}", report.baseline.micro_auprc);
        println!("Micro delta:           {:+.6}", report.micro_delta);
        println!("Tuned macro AUPRC:    {:.6}", report.tuned.macro_auprc);
        println!("Baseline macro AUPRC: {:.6}", report.baseline.macro_auprc);
        println!("Macro delta:           {:+.6}", report.macro_delta);
        println!("MRR delta:             {:+.6}", report.mrr_delta);
        println!("Recall@1 delta:        {:+.6}", report.recall_at_1_delta);
    }
    Ok(())
}

fn validate_fit_command(command: &FitCommand) -> CliResult<()> {
    if command.shortlist == 0 || command.shortlist > 10_000 {
        return Err(invalid_input("--shortlist must lie in 1..=10,000"));
    }
    for (name, value) in [
        ("--require-test-micro-delta", command.require_test_micro_delta),
        ("--require-test-macro-delta", command.require_test_macro_delta),
        (
            "--require-test-recall-at-1-delta",
            command.require_test_recall_at_1_delta,
        ),
    ] {
        if !value.is_finite() {
            return Err(invalid_input(format!("{name} must be finite")));
        }
    }
    Ok(())
}

fn enforce_fit_gates(command: &FitCommand, report: &TuningReport) -> CliResult<()> {
    if report.test_micro_delta < command.require_test_micro_delta {
        return Err(invalid_input(format!(
            "test micro-AUPRC delta {:+.6} is below required {:+.6}",
            report.test_micro_delta, command.require_test_micro_delta
        )));
    }
    if report.test_macro_delta < command.require_test_macro_delta {
        return Err(invalid_input(format!(
            "test macro-AUPRC delta {:+.6} is below required {:+.6}",
            report.test_macro_delta, command.require_test_macro_delta
        )));
    }
    if report.test_recall_at_1_delta < command.require_test_recall_at_1_delta {
        return Err(invalid_input(format!(
            "test Recall@1 delta {:+.6} is below required {:+.6}",
            report.test_recall_at_1_delta, command.require_test_recall_at_1_delta
        )));
    }
    Ok(())
}

fn read_rows(bytes: &[u8], path: &Path) -> CliResult<Vec<ScoreRow>> {
    let mut rows = Vec::new();
    for (line_index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        let value = std::str::from_utf8(line)?.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        rows.push(serde_json::from_str::<ScoreRow>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
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

fn prepare_rows(
    rows: &[ScoreRow],
    lexical_method: &str,
    overlap_method: &str,
    baseline_method: Option<&str>,
) -> CliResult<Vec<PreparedRow>> {
    let mut grouped = BTreeMap::<String, Vec<&ScoreRow>>::new();
    for row in rows {
        if row.query_id.trim().is_empty() {
            return Err(invalid_input("score row query_id must not be empty"));
        }
        grouped.entry(row.query_id.clone()).or_default().push(row);
    }
    if grouped.len() < 5 {
        return Err(invalid_input(
            "hybrid tuning requires at least five independent query groups",
        ));
    }
    let mut prepared = Vec::with_capacity(rows.len());
    for (query_id, group) in grouped {
        if !group.iter().any(|row| row.label) {
            return Err(invalid_input(format!(
                "query {query_id} has no positive candidate"
            )));
        }
        let lexical = method_scores(&group, lexical_method)?;
        let overlap = method_scores(&group, overlap_method)?;
        let lexical_ranks = reciprocal_rank_scores(&group, &lexical);
        let overlap_ranks = reciprocal_rank_scores(&group, &overlap);
        for (index, row) in group.into_iter().enumerate() {
            let baseline_score = baseline_method
                .map(|method| required_score(row, method))
                .transpose()?;
            prepared.push(PreparedRow {
                query_id: row.query_id.clone(),
                label: row.label,
                lexical_score: lexical[index],
                overlap_score: overlap[index],
                lexical_rank_score: lexical_ranks[index],
                overlap_rank_score: overlap_ranks[index],
                baseline_score,
            });
        }
    }
    Ok(prepared)
}

fn method_scores(group: &[&ScoreRow], method: &str) -> CliResult<Vec<f64>> {
    group
        .iter()
        .map(|row| required_score(row, method))
        .collect()
}

fn required_score(row: &ScoreRow, method: &str) -> CliResult<f64> {
    let score = *row.scores.get(method).ok_or_else(|| {
        invalid_input(format!(
            "query {} candidate {} has no score for method {method}",
            row.query_id, row.candidate_id
        ))
    })?;
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(invalid_input(format!(
            "method {method} score {score} must lie in [0, 1]"
        )));
    }
    Ok(score)
}

fn reciprocal_rank_scores(group: &[&ScoreRow], scores: &[f64]) -> Vec<f64> {
    let mut order = (0..scores.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|&left, &right| {
        scores[right]
            .total_cmp(&scores[left])
            .then_with(|| group[left].candidate_id.cmp(&group[right].candidate_id))
            .then_with(|| left.cmp(&right))
    });
    let mut ranks = vec![0.0; scores.len()];
    for (position, index) in order.into_iter().enumerate() {
        if scores[index] > 0.0 {
            ranks[index] = (position + 1) as f64;
        }
    }
    ranks
}

struct QuerySplits {
    train: BTreeSet<String>,
    validation: BTreeSet<String>,
    test: BTreeSet<String>,
}

fn query_splits(rows: &[PreparedRow], seed: u64) -> CliResult<QuerySplits> {
    let mut queries = rows
        .iter()
        .map(|row| row.query_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    queries.sort_unstable_by_key(|query| stable_hash(query, seed));
    if queries.len() < 5 {
        return Err(invalid_input("at least five query groups are required"));
    }
    let train_end = (queries.len().saturating_mul(6) / 10)
        .max(1)
        .min(queries.len() - 2);
    let validation_end = (queries.len().saturating_mul(8) / 10)
        .max(train_end + 1)
        .min(queries.len() - 1);
    Ok(QuerySplits {
        train: queries[..train_end].iter().cloned().collect(),
        validation: queries[train_end..validation_end]
            .iter()
            .cloned()
            .collect(),
        test: queries[validation_end..].iter().cloned().collect(),
    })
}

fn parameter_grid() -> Vec<FusionParameters> {
    let mut configurations = Vec::new();
    for rrf_steps in 0..=4 {
        let rrf_weight = f64::from(rrf_steps) * 0.10;
        let raw_weight = 1.0 - rrf_weight;
        for lexical_steps in 0..=20 {
            let lexical_fraction = f64::from(lexical_steps) * 0.05;
            for rrf_constant in [10.0, 30.0, 60.0, 100.0] {
                configurations.push(FusionParameters {
                    lexical_weight: raw_weight * lexical_fraction,
                    overlap_weight: raw_weight * (1.0 - lexical_fraction),
                    rrf_weight,
                    rrf_constant,
                });
            }
        }
    }
    configurations
}

fn evaluate(
    rows: &[PreparedRow],
    queries: &BTreeSet<String>,
    parameters: FusionParameters,
) -> fo_core::Result<HybridMetricSnapshot> {
    metric_snapshot(&score_rows(rows, parameters, Some(queries)))
}

fn evaluate_baseline(
    rows: &[PreparedRow],
    queries: &BTreeSet<String>,
) -> fo_core::Result<HybridMetricSnapshot> {
    let examples = rows
        .iter()
        .filter(|row| queries.contains(&row.query_id))
        .map(|row| GroupedLabeledScore {
            query_id: row.query_id.clone(),
            score: row.baseline_score.unwrap_or(0.0),
            label: row.label,
        })
        .collect::<Vec<_>>();
    metric_snapshot(&examples)
}

fn score_rows(
    rows: &[PreparedRow],
    parameters: FusionParameters,
    queries: Option<&BTreeSet<String>>,
) -> Vec<GroupedLabeledScore> {
    rows.iter()
        .filter(|row| queries.is_none_or(|queries| queries.contains(&row.query_id)))
        .map(|row| {
            let score = parameters.lexical_weight * row.lexical_score
                + parameters.overlap_weight * row.overlap_score
                + parameters.rrf_weight * rrf_score(row, parameters.rrf_constant);
            GroupedLabeledScore {
                query_id: row.query_id.clone(),
                score: score.clamp(0.0, 1.0),
                label: row.label,
            }
        })
        .collect()
}

fn rrf_score(row: &PreparedRow, constant: f64) -> f64 {
    let mut score = 0.0;
    let mut lanes = 0usize;
    for rank in [row.lexical_rank_score, row.overlap_rank_score] {
        if rank > 0.0 {
            score += (constant + 1.0) / (constant + rank);
            lanes += 1;
        }
    }
    if lanes == 0 {
        0.0
    } else {
        score / lanes as f64
    }
}

fn metric_snapshot(examples: &[GroupedLabeledScore]) -> fo_core::Result<HybridMetricSnapshot> {
    let report = grouped_evaluation_report(
        examples,
        GroupedEvaluationOptions {
            recall_ks: vec![1],
            bootstrap_samples: 0,
            ..GroupedEvaluationOptions::default()
        },
    )?;
    Ok(HybridMetricSnapshot {
        micro_auprc: report.micro.average_precision,
        macro_auprc: report.macro_average_precision,
        mean_reciprocal_rank: report.mean_reciprocal_rank,
        recall_at_1: report
            .recall_at_k
            .first()
            .map_or(0.0, |metric| metric.value),
    })
}

fn compare_candidate_evaluations(
    left: &CandidateEvaluation,
    right: &CandidateEvaluation,
) -> std::cmp::Ordering {
    left.metrics
        .macro_auprc
        .total_cmp(&right.metrics.macro_auprc)
        .then_with(|| left.metrics.micro_auprc.total_cmp(&right.metrics.micro_auprc))
        .then_with(|| {
            left.metrics
                .mean_reciprocal_rank
                .total_cmp(&right.metrics.mean_reciprocal_rank)
        })
        .then_with(|| left.metrics.recall_at_1.total_cmp(&right.metrics.recall_at_1))
        .then_with(|| {
            right
                .parameters
                .lexical_weight
                .total_cmp(&left.parameters.lexical_weight)
        })
}

fn profile_parameters(profile: &HybridFusionProfile) -> FusionParameters {
    FusionParameters {
        lexical_weight: f64::from(profile.lexical_weight),
        overlap_weight: f64::from(profile.overlap_weight),
        rrf_weight: f64::from(profile.rrf_weight),
        rrf_constant: f64::from(profile.rrf_constant),
    }
}

fn read_profile(path: &Path) -> CliResult<HybridFusionProfile> {
    let profile = serde_json::from_slice::<HybridFusionProfile>(&fs::read(path)?)?;
    profile.validate()?;
    Ok(profile)
}

fn write_pretty_json(path: &Path, value: &impl Serialize) -> CliResult<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

fn write_grouped_scores(path: &Path, values: &[GroupedLabeledScore]) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let file = File::create(&temporary)?;
    let mut writer = BufWriter::new(file);
    for value in values {
        serde_json::to_writer(&mut writer, value)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    replace_file(&temporary, path)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    fs::write(&temporary, bytes)?;
    replace_file(&temporary, path)
}

fn replace_file(temporary: &Path, destination: &Path) -> CliResult<()> {
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "profile".into(), |name| name.to_os_string());
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn print_tuning_report(report: &TuningReport, output: &Path) {
    println!("Wrote profile:         {}", output.display());
    println!("Input fingerprint:     {}", report.input_fingerprint);
    println!(
        "Queries train/val/test:{}/{}/{}",
        report.train_queries, report.validation_queries, report.test_queries
    );
    println!("Configurations:        {}", report.evaluated_configurations);
    println!("Lexical weight:        {:.4}", report.selected.lexical_weight);
    println!("Overlap weight:        {:.4}", report.selected.overlap_weight);
    println!("RRF weight:            {:.4}", report.selected.rrf_weight);
    println!("RRF constant:          {:.1}", report.selected.rrf_constant);
    println!(
        "Validation macro AP:   {:.6}",
        report.validation_metrics.macro_auprc
    );
    println!("Test micro AP:         {:.6}", report.test_metrics.micro_auprc);
    println!("Test macro AP:         {:.6}", report.test_metrics.macro_auprc);
    println!("Test micro delta:      {:+.6}", report.test_micro_delta);
    println!("Test macro delta:      {:+.6}", report.test_macro_delta);
    println!(
        "Test Recall@1 delta:   {:+.6}",
        report.test_recall_at_1_delta
    );
}

fn stable_hash(value: &str, seed: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{parameter_grid, prepare_rows, query_splits, ScoreRow};

    #[test]
    fn parameter_grid_contains_pure_and_mixed_models() {
        let grid = parameter_grid();
        assert!(grid.iter().any(|parameters| parameters.lexical_weight == 1.0));
        assert!(grid.iter().any(|parameters| parameters.overlap_weight == 1.0));
        assert!(grid.iter().any(|parameters| parameters.rrf_weight > 0.0));
    }

    #[test]
    fn grouped_preparation_keeps_queries_intact() {
        let mut rows = Vec::new();
        for query in 0..6 {
            for candidate in 0..3 {
                rows.push(ScoreRow {
                    query_id: format!("q{query}"),
                    candidate_id: format!("d{candidate}"),
                    label: candidate == 0,
                    scores: BTreeMap::from([
                        ("lex".to_owned(), if candidate == 0 { 0.8 } else { 0.2 }),
                        (
                            "overlap".to_owned(),
                            if candidate == 0 { 0.9 } else { 0.1 },
                        ),
                        (
                            "baseline".to_owned(),
                            if candidate == 0 { 0.7 } else { 0.3 },
                        ),
                    ]),
                });
            }
        }
        let prepared =
            prepare_rows(&rows, "lex", "overlap", Some("baseline")).expect("prepare");
        let splits = query_splits(&prepared, 7).expect("splits");
        assert_eq!(
            splits.train.len() + splits.validation.len() + splits.test.len(),
            6
        );
        assert!(splits.train.is_disjoint(&splits.validation));
        assert!(splits.validation.is_disjoint(&splits.test));
    }
}
