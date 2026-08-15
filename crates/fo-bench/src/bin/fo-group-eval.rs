#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use clap::Parser;
use fo_core::{
    EvaluationOptions, GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore,
    PrecisionRecallPoint, ThresholdConstraints, grouped_evaluation_report,
    select_operating_point,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-group-eval",
    version,
    about = "Query-grouped AUPRC, ranking metrics, confidence intervals, and threshold selection"
)]
struct Cli {
    /// JSONL containing {"query_id":"q1","score":0.93,"label":true}.
    input: PathBuf,
    #[arg(long, value_delimiter = ',', default_value = "1,5,10")]
    recall_k: Vec<usize>,
    #[arg(long, default_value_t = 500)]
    bootstrap_samples: usize,
    #[arg(long, default_value_t = 0.95)]
    confidence_level: f64,
    #[arg(long, default_value_t = 0x8f3c_21d7_4a9b_65e1)]
    seed: u64,
    #[arg(long, default_value_t = 256)]
    max_curve_points: usize,
    #[arg(long, default_value_t = 15)]
    calibration_bins: usize,
    /// Require this precision when selecting an operating threshold.
    #[arg(long)]
    minimum_precision: Option<f64>,
    /// Require this recall when selecting an operating threshold.
    #[arg(long)]
    minimum_recall: Option<f64>,
    /// Maximum false positives per query at the selected threshold.
    #[arg(long)]
    maximum_false_positives_per_query: Option<f64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct Output {
    report: GroupedEvaluationReport,
    operating_point: Option<PrecisionRecallPoint>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-group-eval: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let examples = read_examples(&command.input)?;
    let evaluation = EvaluationOptions {
        max_curve_points: command.max_curve_points,
        calibration_bins: command.calibration_bins,
    };
    let report = grouped_evaluation_report(
        &examples,
        GroupedEvaluationOptions {
            evaluation,
            recall_ks: command.recall_k,
            bootstrap_samples: command.bootstrap_samples,
            confidence_level: command.confidence_level,
            seed: command.seed,
        },
    )?;
    let has_constraints = command.minimum_precision.is_some()
        || command.minimum_recall.is_some()
        || command.maximum_false_positives_per_query.is_some();
    let operating_point = if has_constraints {
        Some(select_operating_point(
            &examples,
            evaluation,
            ThresholdConstraints {
                minimum_precision: command.minimum_precision.unwrap_or(0.0),
                minimum_recall: command.minimum_recall.unwrap_or(0.0),
                maximum_false_positives_per_query: command
                    .maximum_false_positives_per_query,
            },
        )?)
    } else {
        None
    };
    let output = Output {
        report,
        operating_point,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_output(&output);
    }
    Ok(())
}

fn read_examples(path: &Path) -> CliResult<Vec<GroupedLabeledScore>> {
    let file = File::open(path)?;
    let mut examples = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        examples.push(
            serde_json::from_str::<GroupedLabeledScore>(value).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}:{}: {error}", path.display(), line_index + 1),
                )
            })?,
        );
    }
    if examples.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} contains no grouped labeled scores", path.display()),
        )
        .into());
    }
    Ok(examples)
}

fn print_output(output: &Output) {
    let report = &output.report;
    println!("Queries:                    {}", report.queries);
    println!(
        "Queries with positives:     {}",
        report.queries_with_positives
    );
    println!("Examples:                   {}", report.examples);
    println!("Micro AUPRC:                {:.6}", report.micro.average_precision);
    println!(
        "Macro query AUPRC:          {:.6}",
        report.macro_average_precision
    );
    println!(
        "Mean reciprocal rank:       {:.6}",
        report.mean_reciprocal_rank
    );
    for metric in &report.recall_at_k {
        println!("Recall@{:<3}                  {:.6}", metric.k, metric.value);
    }
    for metric in &report.ndcg_at_k {
        println!("nDCG@{:<3}                    {:.6}", metric.k, metric.value);
    }
    if let Some(interval) = report.micro_average_precision_interval {
        println!(
            "Micro AUPRC {:.1}% CI:     [{:.6}, {:.6}]",
            interval.confidence_level * 100.0,
            interval.lower,
            interval.upper
        );
    }
    if let Some(interval) = report.macro_average_precision_interval {
        println!(
            "Macro AUPRC {:.1}% CI:     [{:.6}, {:.6}]",
            interval.confidence_level * 100.0,
            interval.lower,
            interval.upper
        );
    }
    if let Some(point) = output.operating_point {
        println!("Selected threshold:          {:.6}", point.threshold);
        println!("  precision:                 {:.6}", point.precision);
        println!("  recall:                    {:.6}", point.recall);
        println!("  false positives:           {}", point.false_positives);
    }
}
