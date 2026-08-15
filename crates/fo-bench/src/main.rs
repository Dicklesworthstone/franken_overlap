#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_core::{
    EvaluationOptions, LabeledScore, PrecisionRecallReport, precision_recall_report,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-bench",
    version,
    about = "Quality and performance evaluation for FrankenOverlap"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Evaluate labeled probability scores and emit AUPRC/calibration metrics.
    Evaluate(EvaluateCommand),
}

#[derive(Debug, Args)]
struct EvaluateCommand {
    /// JSONL containing {"score": 0.93, "label": true} records.
    input: PathBuf,
    /// Maximum number of threshold points retained in the output PR curve.
    #[arg(long, default_value_t = 256)]
    max_curve_points: usize,
    /// Number of equal-width probability bins used for calibration error.
    #[arg(long, default_value_t = 15)]
    calibration_bins: usize,
    /// Emit the complete report as pretty JSON.
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Evaluate(command) => run_evaluate(command),
    }
}

fn run_evaluate(command: EvaluateCommand) -> CliResult<()> {
    let examples = read_labeled_scores(&command.input)?;
    let report = precision_recall_report(
        &examples,
        EvaluationOptions {
            max_curve_points: command.max_curve_points,
            calibration_bins: command.calibration_bins,
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn read_labeled_scores(path: &Path) -> CliResult<Vec<LabeledScore>> {
    let file = File::open(path)?;
    let mut examples = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let example = serde_json::from_str::<LabeledScore>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
        })?;
        examples.push(example);
    }
    if examples.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} contains no labeled scores", path.display()),
        )
        .into());
    }
    Ok(examples)
}

fn print_report(report: &PrecisionRecallReport) {
    println!("Examples:              {}", report.examples);
    println!("Positives:             {}", report.positives);
    println!("Negatives:             {}", report.negatives);
    println!("Prevalence:            {:.6}", report.prevalence);
    println!("AUPRC / avg precision: {:.6}", report.average_precision);
    println!("Best F1:               {:.6}", report.best_f1);
    println!("Best threshold:        {:.6}", report.best_threshold);
    println!("Brier score:           {:.6}", report.brier_score);
    println!("Log loss:              {:.6}", report.log_loss);
    println!(
        "Expected cal. error:   {:.6}",
        report.expected_calibration_error
    );
    println!(
        "Maximum cal. error:    {:.6}",
        report.maximum_calibration_error
    );
    println!("PR curve points:       {}", report.curve.len());
}
