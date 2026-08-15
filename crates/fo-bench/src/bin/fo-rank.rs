#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_core::{
    EvaluationOptions, GroupedFeedbackExample, PairwiseRankingOptions, RankingModel,
    SearchResult, mine_hard_negatives,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-rank",
    version,
    about = "Query-grouped hard-negative ranking for FrankenOverlap"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Fit a pairwise ranker from grouped feedback JSONL.
    Fit(FitCommand),
    /// Compare raw and ranked held-out AUPRC.
    Compare(CompareCommand),
    /// Retain every positive and the hardest negatives per query.
    MineHardNegatives(MineCommand),
    /// Rerank a SearchResult JSON array.
    Rerank(RerankCommand),
}

#[derive(Debug, Args)]
struct FitCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 600)]
    epochs: usize,
    #[arg(long, default_value_t = 0.08)]
    learning_rate: f64,
    #[arg(long, default_value_t = 0.01)]
    l2: f64,
    #[arg(long, default_value_t = 16)]
    max_negatives_per_positive: usize,
}

#[derive(Debug, Args)]
struct CompareCommand {
    input: PathBuf,
    model: PathBuf,
    #[arg(long, default_value_t = 0.0)]
    require_auprc_delta: f64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct MineCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 16)]
    maximum_negatives_per_query: usize,
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
        eprintln!("fo-rank: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Fit(command) => run_fit(command),
        Command::Compare(command) => run_compare(command),
        Command::MineHardNegatives(command) => run_mine(command),
        Command::Rerank(command) => run_rerank(command),
    }
}

fn run_fit(command: FitCommand) -> CliResult<()> {
    let examples = read_grouped_feedback(&command.input)?;
    let model = RankingModel::fit(
        &examples,
        PairwiseRankingOptions {
            epochs: command.epochs,
            learning_rate: command.learning_rate,
            l2: command.l2,
            max_negatives_per_positive: command.max_negatives_per_positive,
            ..PairwiseRankingOptions::default()
        },
        EvaluationOptions::default(),
    )?;
    write_pretty_json(&command.output, &model)?;
    println!("Wrote {}", command.output.display());
    println!("  examples:       {}", model.training_examples);
    println!("  queries:        {}", model.training_queries);
    println!("  training pairs: {}", model.training_pairs);
    println!(
        "  raw AUPRC:      {:.6}",
        model.raw_training_report.average_precision
    );
    println!(
        "  ranked AUPRC:   {:.6}",
        model.ranked_training_report.average_precision
    );
    Ok(())
}

fn run_compare(command: CompareCommand) -> CliResult<()> {
    if !command.require_auprc_delta.is_finite() {
        return Err(invalid_input("--require-auprc-delta must be finite"));
    }
    let examples = read_grouped_feedback(&command.input)?;
    let model = read_model(&command.model)?;
    let comparison = model.compare(&examples, EvaluationOptions::default())?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
    } else {
        println!("Raw AUPRC:      {:.6}", comparison.raw.average_precision);
        println!("Ranked AUPRC:   {:.6}", comparison.ranked.average_precision);
        println!("AUPRC delta:    {:+.6}", comparison.auprc_delta);
        println!("Brier delta:    {:+.6}", comparison.brier_delta);
    }
    if comparison.auprc_delta < command.require_auprc_delta {
        return Err(invalid_input(format!(
            "ranked AUPRC delta {:+.6} is below required {:+.6}",
            comparison.auprc_delta, command.require_auprc_delta
        )));
    }
    Ok(())
}

fn run_mine(command: MineCommand) -> CliResult<()> {
    let examples = read_grouped_feedback(&command.input)?;
    let mined = mine_hard_negatives(&examples, command.maximum_negatives_per_query)?;
    write_grouped_feedback(&command.output, &mined)?;
    println!(
        "Wrote {} examples to {} (from {})",
        mined.len(),
        command.output.display(),
        examples.len()
    );
    Ok(())
}

fn run_rerank(command: RerankCommand) -> CliResult<()> {
    let results = serde_json::from_slice::<Vec<SearchResult>>(&fs::read(&command.input)?)?;
    let model = read_model(&command.model)?;
    let ranked = model.rerank(&results)?;
    let json = serde_json::to_string_pretty(&ranked)?;
    if let Some(path) = command.output {
        atomic_write(&path, json.as_bytes())?;
        println!("Wrote {}", path.display());
    } else {
        println!("{json}");
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
            "{} contains no grouped feedback",
            path.display()
        )));
    }
    Ok(examples)
}

fn write_grouped_feedback(path: &Path, examples: &[GroupedFeedbackExample]) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let mut writer = BufWriter::new(file);
    for example in examples {
        serde_json::to_writer(&mut writer, example)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

fn read_model(path: &Path) -> CliResult<RankingModel> {
    let model = serde_json::from_slice::<RankingModel>(&fs::read(path)?)?;
    model.validate()?;
    Ok(model)
}

fn write_pretty_json(path: &Path, value: &impl serde::Serialize) -> CliResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("json"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
