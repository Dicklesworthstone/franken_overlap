#![forbid(unsafe_code)]

#[path = "../claim_gates.rs"]
mod claim_gates;

use std::error::Error;
use std::path::{Path, PathBuf};

use claim_gates::{ClaimGateReport, evaluate_claims, write_default_manifest};
use clap::{Args, Parser, Subcommand};
use fo_corpus::atomic_write;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-claim-gate",
    version,
    about = "Evaluate predeclared paired quality and latency claims from proof-benchmark receipts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a documented starter gate manifest.
    Init(InitCommand),
    /// Evaluate every declared comparison and emit supported/inconclusive/unsupported verdicts.
    Evaluate(EvaluateCommand),
}

#[derive(Debug, Args)]
struct InitCommand {
    output: PathBuf,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct EvaluateCommand {
    proof_report: PathBuf,
    scores: PathBuf,
    manifest: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    require_supported: bool,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-claim-gate: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Init(command) => run_init(command),
        Command::Evaluate(command) => run_evaluate(command),
    }
}

fn run_init(command: InitCommand) -> CliResult<()> {
    if command.output.exists() && !command.force {
        return Err(invalid(format!(
            "{} already exists; pass --force to replace it",
            command.output.display()
        )));
    }
    write_default_manifest(&command.output)?;
    println!("Wrote {}", command.output.display());
    Ok(())
}

fn run_evaluate(command: EvaluateCommand) -> CliResult<()> {
    let report = evaluate_claims(&command.proof_report, &command.scores, &command.manifest)?;
    if let Some(path) = &command.output {
        atomic_write(path, &serde_json::to_vec_pretty(&report)?)?;
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report, command.output.as_deref());
    }
    if command.require_supported && !report.all_supported {
        return Err(invalid(
            "one or more predeclared comparisons were inconclusive or unsupported",
        ));
    }
    Ok(())
}

fn print_report(report: &ClaimGateReport, output: Option<&Path>) {
    println!("Corpus:                      {}", report.corpus_id);
    println!("Corpus size:                 {}", report.corpus_size);
    println!(
        "Confidence:                  nominal {:.4}, family-wise {:.4}",
        report.nominal_confidence_level, report.familywise_confidence_level
    );
    println!("All claims supported:        {}", report.all_supported);
    if let Some(path) = output {
        println!("Report:                      {}", path.display());
    }
    for comparison in &report.comparisons {
        println!();
        println!(
            "[{:?}] {}: {} versus {}",
            comparison.verdict,
            comparison.id,
            comparison.challenger_method,
            comparison.baseline_method
        );
        println!(
            "  paired queries:             {} ({} incomplete excluded)",
            comparison.eligible_queries, comparison.excluded_incomplete_queries
        );
        println!(
            "  micro AUPRC:                {:.6} -> {:.6} ({:+.6}; lower {:+.6})",
            comparison.baseline.micro_auprc,
            comparison.challenger.micro_auprc,
            comparison.delta.micro_auprc,
            comparison.bootstrap.micro_auprc.lower
        );
        println!(
            "  macro AUPRC:                {:.6} -> {:.6} ({:+.6}; lower {:+.6})",
            comparison.baseline.macro_auprc,
            comparison.challenger.macro_auprc,
            comparison.delta.macro_auprc,
            comparison.bootstrap.macro_auprc.lower
        );
        println!(
            "  Recall@1:                   {:.6} -> {:.6} ({:+.6}; lower {:+.6})",
            comparison.baseline.recall_at_1,
            comparison.challenger.recall_at_1,
            comparison.delta.recall_at_1,
            comparison.bootstrap.recall_at_1.lower
        );
        println!(
            "  repeat p95:                 {:.3} ms -> {:.3} ms (ratio {})",
            comparison.baseline_p95_ms,
            comparison.challenger_p95_ms,
            comparison
                .p95_ratio
                .map_or_else(|| "n/a".to_owned(), |ratio| format!("{ratio:.4}"))
        );
        if let Some(profile) = &comparison.worst_profile {
            println!(
                "  worst profile macro delta:  {} {:+.6}",
                profile,
                comparison.worst_profile_macro_delta.unwrap_or(0.0)
            );
        }
        for failure in &comparison.failures {
            println!("  FAILURE:                     {failure}");
        }
        for uncertainty in &comparison.uncertainties {
            println!("  INCONCLUSIVE:                {uncertainty}");
        }
    }
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
