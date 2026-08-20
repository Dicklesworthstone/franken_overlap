#![forbid(unsafe_code)]

#[path = "../adjudication.rs"]
mod adjudication;

use std::error::Error;
use std::path::{Path, PathBuf};

use adjudication::{QueueOptions, apply_decisions, create_review_queue, validate_gold};
use clap::{Args, Parser, Subcommand};
use fo_corpus::atomic_write;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-adjudicate",
    version,
    about = "Create review queues and convert ambiguous real-corpus labels into validated gold queries"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create review tasks from scenario queries and pair-level benchmark scores.
    Queue(QueueCommand),
    /// Apply human decisions and emit benchmark-compatible gold query JSONL.
    Apply(ApplyCommand),
    /// Validate gold positives, relevance grades, and normalized token spans.
    Validate(ValidateCommand),
}

#[derive(Debug, Args)]
struct QueueCommand {
    corpus_root: PathBuf,
    queries: PathBuf,
    scores: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
    #[arg(long)]
    corpus_size: Option<usize>,
    #[arg(long, default_value_t = 12)]
    top_candidates: usize,
    #[arg(long, default_value_t = 5)]
    hybrid_top_k: usize,
    #[arg(long, default_value_t = 0.05)]
    low_margin: f64,
    #[arg(long, default_value_t = 160)]
    snippet_tokens: usize,
    #[arg(long)]
    include_all: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ApplyCommand {
    queries: PathBuf,
    decisions: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
    /// Retain generated natural-relation labels when no human decision exists.
    #[arg(long)]
    allow_unreviewed_natural: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ValidateCommand {
    corpus_root: PathBuf,
    gold_queries: PathBuf,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-adjudicate: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Queue(command) => run_queue(command),
        Command::Apply(command) => run_apply(command),
        Command::Validate(command) => run_validate(command),
    }
}

fn run_queue(command: QueueCommand) -> CliResult<()> {
    let report = create_review_queue(
        &command.corpus_root,
        &command.queries,
        &command.scores,
        &command.output,
        &QueueOptions {
            corpus_size: command.corpus_size,
            top_candidates: command.top_candidates,
            hybrid_top_k: command.hybrid_top_k,
            low_margin: command.low_margin,
            snippet_tokens: command.snippet_tokens,
            include_all: command.include_all,
        },
    )?;
    write_optional_report(command.report.as_deref(), &report)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:                 {}", report.corpus_id);
        println!("Corpus size:            {}", report.corpus_size);
        println!("Input queries:          {}", report.input_queries);
        println!("Queued queries:         {}", report.queued_queries);
        println!(
            "Natural relations:      {}",
            report.natural_relation_queries
        );
        println!("Top-one disagreements:  {}", report.disagreement_queries);
        println!("Queue:                  {}", command.output.display());
        println!("Queue SHA-256:          {}", report.output_sha256);
    }
    Ok(())
}

fn run_apply(command: ApplyCommand) -> CliResult<()> {
    let report = apply_decisions(
        &command.queries,
        &command.decisions,
        &command.output,
        command.allow_unreviewed_natural,
    )?;
    write_optional_report(command.report.as_deref(), &report)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Input queries:          {}", report.input_queries);
        println!("Decisions:              {}", report.decisions);
        println!("Accepted generated:     {}", report.accepted_generated);
        println!("Replaced:               {}", report.replaced);
        println!("Excluded:               {}", report.excluded);
        println!(
            "Unreviewed controlled:  {}",
            report.retained_controlled_without_review
        );
        println!("Output queries:         {}", report.output_queries);
        println!("Gold queries:           {}", command.output.display());
        println!("Gold SHA-256:           {}", report.output_sha256);
    }
    Ok(())
}

fn run_validate(command: ValidateCommand) -> CliResult<()> {
    let report = validate_gold(&command.corpus_root, &command.gold_queries)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:                 {}", report.corpus_id);
        println!("Queries:                {}", report.queries);
        println!("Controlled:             {}", report.controlled_queries);
        println!("Natural:                {}", report.natural_queries);
        println!("Multi-positive:         {}", report.multi_positive_queries);
        println!("Relevance labels:       {}", report.relevance_labels);
        println!("Acceptable spans:       {}", report.acceptable_spans);
        println!("Gold SHA-256:           {}", report.query_sha256);
    }
    Ok(())
}

fn write_optional_report<T: serde::Serialize>(path: Option<&Path>, report: &T) -> CliResult<()> {
    if let Some(path) = path {
        atomic_write(path, &serde_json::to_vec_pretty(report)?)?;
    }
    Ok(())
}
