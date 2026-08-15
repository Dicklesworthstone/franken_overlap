#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    CalibratedResult, CalibrationModel, CalibrationOptions, EvaluationOptions, FeedbackExample,
    LabeledScore, PrecisionRecallReport, SearchResult, precision_recall_report,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-bench",
    version,
    about = "Quality, calibration, and benchmark evaluation for FrankenOverlap"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Evaluate labeled probability scores and emit AUPRC/calibration metrics.
    Evaluate(EvaluateCommand),
    /// Append a labeled search result to an accretive feedback ledger.
    RecordFeedback(RecordFeedbackCommand),
    /// Fit a deterministic logistic calibration model from feedback JSONL.
    FitCalibration(FitCalibrationCommand),
    /// Compare raw and calibrated AUPRC on a feedback corpus.
    CompareCalibration(CompareCalibrationCommand),
    /// Rerank a JSON result set with a fitted calibration model.
    Rerank(RerankCommand),
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

#[derive(Debug, Args)]
struct RecordFeedbackCommand {
    /// JSON file containing one SearchResult or a JSON array of SearchResult.
    results: PathBuf,
    /// One-based result rank to label.
    #[arg(long, default_value_t = 1)]
    rank: usize,
    #[arg(long, value_enum)]
    label: LabelArg,
    /// Append-only feedback JSONL destination.
    #[arg(short, long)]
    output: PathBuf,
    /// Optional training weight for this judgment.
    #[arg(long, default_value_t = 1.0)]
    weight: f64,
}

#[derive(Debug, Args)]
struct FitCalibrationCommand {
    /// Feedback JSONL produced by record-feedback.
    input: PathBuf,
    /// Destination model JSON.
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 600)]
    epochs: usize,
    #[arg(long, default_value_t = 0.12)]
    learning_rate: f64,
    #[arg(long, default_value_t = 0.01)]
    l2: f64,
    #[arg(long, default_value_t = 1.0e-9)]
    convergence_tolerance: f64,
    #[arg(long, default_value_t = 256)]
    max_curve_points: usize,
    #[arg(long, default_value_t = 15)]
    calibration_bins: usize,
}

#[derive(Debug, Args)]
struct CompareCalibrationCommand {
    input: PathBuf,
    model: PathBuf,
    #[arg(long, default_value_t = 256)]
    max_curve_points: usize,
    #[arg(long, default_value_t = 15)]
    calibration_bins: usize,
    #[arg(long)]
    json: bool,
    /// Fail when calibrated AUPRC improves by less than this amount.
    #[arg(long)]
    require_auprc_delta: Option<f64>,
    /// Fail when calibrated Brier score regresses by more than this amount.
    #[arg(long)]
    maximum_brier_regression: Option<f64>,
}

#[derive(Debug, Args)]
struct RerankCommand {
    results: PathBuf,
    model: PathBuf,
    /// Optional destination; stdout is used when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LabelArg {
    Positive,
    Negative,
}

impl LabelArg {
    fn value(self) -> bool {
        matches!(self, Self::Positive)
    }
}

#[derive(Debug, Serialize)]
struct CalibrationComparison {
    raw: PrecisionRecallReport,
    calibrated: PrecisionRecallReport,
    average_precision_delta: f64,
    brier_delta: f64,
    log_loss_delta: f64,
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
        Command::RecordFeedback(command) => run_record_feedback(command),
        Command::FitCalibration(command) => run_fit_calibration(command),
        Command::CompareCalibration(command) => run_compare_calibration(command),
        Command::Rerank(command) => run_rerank(command),
    }
}

fn run_evaluate(command: EvaluateCommand) -> CliResult<()> {
    let examples = read_jsonl::<LabeledScore>(&command.input)?;
    let report = precision_recall_report(
        &examples,
        evaluation_options(command.max_curve_points, command.calibration_bins),
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn run_record_feedback(command: RecordFeedbackCommand) -> CliResult<()> {
    if command.rank == 0 {
        return Err(invalid_data("--rank is one-based and must be positive"));
    }
    if !command.weight.is_finite() || command.weight <= 0.0 {
        return Err(invalid_data("--weight must be finite and positive"));
    }
    let results = read_results(&command.results)?;
    let result = results.get(command.rank - 1).ok_or_else(|| {
        invalid_data(format!(
            "rank {} is unavailable; result file contains {} entries",
            command.rank,
            results.len()
        ))
    })?;
    if let Some(parent) = command
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&command.output)?;
    serde_json::to_writer(
        &mut output,
        &FeedbackExample {
            result: result.clone(),
            label: command.label.value(),
            weight: command.weight,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    println!(
        "Recorded {:?} feedback for rank {} in {}",
        command.label,
        command.rank,
        command.output.display()
    );
    Ok(())
}

fn run_fit_calibration(command: FitCalibrationCommand) -> CliResult<()> {
    let examples = read_jsonl::<FeedbackExample>(&command.input)?;
    let model = CalibrationModel::fit(
        &examples,
        CalibrationOptions {
            epochs: command.epochs,
            learning_rate: command.learning_rate,
            l2: command.l2,
            convergence_tolerance: command.convergence_tolerance,
        },
        evaluation_options(command.max_curve_points, command.calibration_bins),
    )?;
    if let Some(parent) = command
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&command.output, serde_json::to_vec_pretty(&model)?)?;
    println!("Wrote calibration model to {}", command.output.display());
    println!("Training examples: {}", model.training_examples);
    println!(
        "Training AUPRC:   {:.6}",
        model.training_report.average_precision
    );
    println!("Completed epochs: {}", model.completed_epochs);
    Ok(())
}

fn run_compare_calibration(command: CompareCalibrationCommand) -> CliResult<()> {
    let examples = read_jsonl::<FeedbackExample>(&command.input)?;
    let model = read_model(&command.model)?;
    let options = evaluation_options(command.max_curve_points, command.calibration_bins);
    let raw_scores = examples
        .iter()
        .map(|example| LabeledScore {
            score: f64::from(example.result.combined_score),
            label: example.label,
        })
        .collect::<Vec<_>>();
    let calibrated_scores = examples
        .iter()
        .map(|example| LabeledScore {
            score: model.predict(&example.result),
            label: example.label,
        })
        .collect::<Vec<_>>();
    let raw = precision_recall_report(&raw_scores, options)?;
    let calibrated = precision_recall_report(&calibrated_scores, options)?;
    let comparison = CalibrationComparison {
        average_precision_delta: calibrated.average_precision - raw.average_precision,
        brier_delta: calibrated.brier_score - raw.brier_score,
        log_loss_delta: calibrated.log_loss - raw.log_loss,
        raw,
        calibrated,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
    } else {
        println!("Raw evidence:");
        print_report(&comparison.raw);
        println!("\nCalibrated evidence:");
        print_report(&comparison.calibrated);
        println!(
            "\nAUPRC delta:   {:+.6}",
            comparison.average_precision_delta
        );
        println!("Brier delta:   {:+.6}", comparison.brier_delta);
        println!("Log-loss delta:{:+.6}", comparison.log_loss_delta);
    }
    if let Some(required) = command.require_auprc_delta
        && comparison.average_precision_delta < required
    {
        return Err(invalid_data(format!(
            "calibrated AUPRC delta {:+.6} is below required {:+.6}",
            comparison.average_precision_delta, required
        )));
    }
    if let Some(maximum) = command.maximum_brier_regression
        && comparison.brier_delta > maximum
    {
        return Err(invalid_data(format!(
            "calibrated Brier regression {:+.6} exceeds maximum {:+.6}",
            comparison.brier_delta, maximum
        )));
    }
    Ok(())
}

fn run_rerank(command: RerankCommand) -> CliResult<()> {
    let results = read_results(&command.results)?;
    let model = read_model(&command.model)?;
    let reranked = model.rerank(&results)?;
    write_calibrated_results(&reranked, command.output.as_deref())
}

fn read_jsonl<T>(path: &Path) -> CliResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let file = File::open(path)?;
    let mut examples = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let example = serde_json::from_str::<T>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
        })?;
        examples.push(example);
    }
    if examples.is_empty() {
        return Err(invalid_data(format!(
            "{} contains no usable records",
            path.display()
        )));
    }
    Ok(examples)
}

fn read_results(path: &Path) -> CliResult<Vec<SearchResult>> {
    let bytes = fs::read(path)?;
    if let Ok(results) = serde_json::from_slice::<Vec<SearchResult>>(&bytes) {
        if results.is_empty() {
            return Err(invalid_data(format!(
                "{} contains an empty result array",
                path.display()
            )));
        }
        return Ok(results);
    }
    let result = serde_json::from_slice::<SearchResult>(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not SearchResult JSON: {error}", path.display()),
        )
    })?;
    Ok(vec![result])
}

fn read_model(path: &Path) -> CliResult<CalibrationModel> {
    let model = serde_json::from_slice::<CalibrationModel>(&fs::read(path)?)?;
    model.validate()?;
    Ok(model)
}

fn write_calibrated_results(
    results: &[CalibratedResult],
    output: Option<&Path>,
) -> CliResult<()> {
    let bytes = serde_json::to_vec_pretty(results)?;
    if let Some(path) = output {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        println!("Wrote reranked results to {}", path.display());
    } else {
        io::stdout().write_all(&bytes)?;
        io::stdout().write_all(b"\n")?;
    }
    Ok(())
}

fn evaluation_options(max_curve_points: usize, calibration_bins: usize) -> EvaluationOptions {
    EvaluationOptions {
        max_curve_points,
        calibration_bins,
    }
}

fn invalid_data(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidData, message.into()))
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
