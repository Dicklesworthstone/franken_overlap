#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_core::{
    ApRankingComparison, ApRankingModel, ApRankingOptions, GroupedEvaluationOptions,
    GroupedFeedbackExample, SearchResult,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-ap-rank",
    version,
    about = "Train and validate query-balanced AP-delta ranking models"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Fit an AP-delta-weighted linear ranker from grouped feedback JSONL.
    Fit(FitCommand),
    /// Compare raw and ranked held-out query groups and enforce quality gates.
    Compare(CompareCommand),
    /// Rerank one SearchResult JSON array.
    Rerank(RerankCommand),
}

#[derive(Debug, Args)]
struct FitCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 500)]
    epochs: usize,
    #[arg(long, default_value_t = 0.08)]
    learning_rate: f64,
    #[arg(long, default_value_t = 0.01)]
    l2: f64,
    #[arg(long, default_value_t = 24)]
    maximum_negatives_per_query: usize,
    #[arg(long, default_value_t = 1.0e-8)]
    minimum_ap_delta: f64,
    #[arg(long, default_value_t = 1.0e-9)]
    convergence_tolerance: f64,
    #[arg(long, default_value_t = 0)]
    bootstrap_samples: usize,
    #[arg(long, default_value_t = 0x4150_5241_4e4b_4552)]
    bootstrap_seed: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct CompareCommand {
    input: PathBuf,
    model: PathBuf,
    #[arg(long, default_value_t = 0.0)]
    require_micro_auprc_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    require_macro_auprc_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    require_mrr_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    require_recall_at_1_delta: f64,
    #[arg(long, default_value_t = 1000)]
    bootstrap_samples: usize,
    #[arg(long, default_value_t = 0x4150_5241_4e4b_4552)]
    bootstrap_seed: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RerankCommand {
    input: PathBuf,
    model: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-ap-rank: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Fit(command) => run_fit(command),
        Command::Compare(command) => run_compare(command),
        Command::Rerank(command) => run_rerank(command),
    }
}

fn run_fit(command: FitCommand) -> CliResult<()> {
    let examples = read_grouped_feedback(&command.input)?;
    let model = ApRankingModel::fit(
        &examples,
        ApRankingOptions {
            epochs: command.epochs,
            learning_rate: command.learning_rate,
            l2: command.l2,
            maximum_negatives_per_query: command.maximum_negatives_per_query,
            minimum_ap_delta: command.minimum_ap_delta,
            convergence_tolerance: command.convergence_tolerance,
        },
        evaluation_options(command.bootstrap_samples, command.bootstrap_seed),
    )?;
    atomic_write_json(&command.output, &model)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&model)?);
    } else {
        println!("Wrote {}", command.output.display());
        println!("Examples:          {}", model.training_examples);
        println!("Queries:           {}", model.training_queries);
        println!("Trainable queries: {}", model.trainable_queries);
        println!("Last epoch pairs:  {}", model.last_epoch_pairs);
        println!("Completed epochs:  {}", model.completed_epochs);
        println!(
            "Raw micro AUPRC:   {:.6}",
            model.raw_training_report.micro.average_precision
        );
        println!(
            "Ranked micro AUPRC:{:.6}",
            model.ranked_training_report.micro.average_precision
        );
        println!(
            "Raw macro AUPRC:   {:.6}",
            model.raw_training_report.macro_average_precision
        );
        println!(
            "Ranked macro AUPRC:{:.6}",
            model.ranked_training_report.macro_average_precision
        );
    }
    Ok(())
}

fn run_compare(command: CompareCommand) -> CliResult<()> {
    for (name, value) in [
        ("--require-micro-auprc-delta", command.require_micro_auprc_delta),
        ("--require-macro-auprc-delta", command.require_macro_auprc_delta),
        ("--require-mrr-delta", command.require_mrr_delta),
        (
            "--require-recall-at-1-delta",
            command.require_recall_at_1_delta,
        ),
    ] {
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(invalid_input(format!("{name} must lie in [-1, 1]")));
        }
    }
    let examples = read_grouped_feedback(&command.input)?;
    let model = read_model(&command.model)?;
    let comparison = model.compare(
        &examples,
        evaluation_options(command.bootstrap_samples, command.bootstrap_seed),
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
    } else {
        print_comparison(&comparison);
    }
    let failures = [
        (
            "micro AUPRC",
            comparison.micro_auprc_delta,
            command.require_micro_auprc_delta,
        ),
        (
            "macro AUPRC",
            comparison.macro_auprc_delta,
            command.require_macro_auprc_delta,
        ),
        (
            "mean reciprocal rank",
            comparison.mean_reciprocal_rank_delta,
            command.require_mrr_delta,
        ),
        (
            "Recall@1",
            comparison.recall_at_1_delta,
            command.require_recall_at_1_delta,
        ),
    ]
    .into_iter()
    .filter(|(_, observed, required)| observed < required)
    .map(|(name, observed, required)| {
        format!("{name} delta {observed:.6} is below required {required:.6}")
    })
    .collect::<Vec<_>>();
    if !failures.is_empty() {
        return Err(invalid_input(failures.join("; ")));
    }
    Ok(())
}

fn run_rerank(command: RerankCommand) -> CliResult<()> {
    let model = read_model(&command.model)?;
    let results = serde_json::from_slice::<Vec<SearchResult>>(&fs::read(&command.input)?)?;
    let ranked = model.rerank(&results)?;
    let bytes = serde_json::to_vec_pretty(&ranked)?;
    if let Some(output) = command.output {
        atomic_write(&output, &bytes)?;
    } else {
        println!("{}", String::from_utf8(bytes)?);
    }
    Ok(())
}

fn read_grouped_feedback(path: &Path) -> CliResult<Vec<GroupedFeedbackExample>> {
    let file = File::open(path)?;
    let mut examples = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        examples.push(
            serde_json::from_str::<GroupedFeedbackExample>(value).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}:{}: {error}", path.display(), line_index + 1),
                )
            })?,
        );
    }
    if examples.is_empty() {
        return Err(invalid_input(format!(
            "{} contains no grouped feedback examples",
            path.display()
        )));
    }
    Ok(examples)
}

fn read_model(path: &Path) -> CliResult<ApRankingModel> {
    let model = serde_json::from_slice::<ApRankingModel>(&fs::read(path)?)?;
    model.validate()?;
    Ok(model)
}

fn evaluation_options(samples: usize, seed: u64) -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        bootstrap_samples: samples,
        bootstrap_seed: seed,
        ..GroupedEvaluationOptions::default()
    }
}

fn print_comparison(comparison: &ApRankingComparison) {
    println!(
        "Raw micro AUPRC:       {:.6}",
        comparison.raw.micro.average_precision
    );
    println!(
        "Ranked micro AUPRC:    {:.6}",
        comparison.ranked.micro.average_precision
    );
    println!("Micro AUPRC delta:      {:+.6}", comparison.micro_auprc_delta);
    println!(
        "Raw macro AUPRC:       {:.6}",
        comparison.raw.macro_average_precision
    );
    println!(
        "Ranked macro AUPRC:    {:.6}",
        comparison.ranked.macro_average_precision
    );
    println!("Macro AUPRC delta:      {:+.6}", comparison.macro_auprc_delta);
    println!(
        "MRR delta:              {:+.6}",
        comparison.mean_reciprocal_rank_delta
    );
    println!("Recall@1 delta:         {:+.6}", comparison.recall_at_1_delta);
}

fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> CliResult<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    replace_file(&temporary, path)
}

fn replace_file(temporary: &Path, destination: &Path) -> CliResult<()> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.exists() => {
            fs::remove_file(destination)?;
            fs::rename(temporary, destination)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut filename = path
        .file_name()
        .map_or_else(|| "ap-ranker".into(), |name| name.to_os_string());
    filename.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(filename)
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
