#![forbid(unsafe_code)]

#[path = "../naive_proof.rs"]
mod naive_proof;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use fo_corpus::{MANIFEST_FILENAME, atomic_write, sha256_hex, unix_timestamp};
use naive_proof::{NaiveProofOptions, render_naive_proof, run_naive_proof};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-naive-proof",
    version,
    about = "Compare full-corpus FrankenOverlap with bounded naive semi-global Levenshtein scans"
)]
struct Cli {
    /// Existing fo-corpus root; sectioned Gutenberg/SEC corpora are recommended.
    corpus_root: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 48)]
    maximum_documents: usize,
    #[arg(long, default_value_t = 14)]
    queries: usize,
    #[arg(long, default_value_t = 64)]
    passage_words: usize,
    #[arg(long, default_value_t = 7)]
    hard_negatives: usize,
    #[arg(long, default_value_t = 2_000_000_000u128)]
    maximum_total_cells: u128,
    #[arg(long, default_value_t = 1)]
    repetitions: usize,
    #[arg(long, default_value_t = 0x6e_61_69_76_65_2d_64_70)]
    seed: u64,
    #[arg(long)]
    minimum_p95_speedup: Option<f64>,
    #[arg(long)]
    minimum_macro_auprc_delta: Option<f64>,
    #[arg(long, default_value_t = 0.75)]
    minimum_completed_fraction: f64,
    #[arg(long)]
    no_replace: bool,
}

#[derive(Debug, Serialize)]
struct EnvironmentReport {
    generated_at_unix: u64,
    operating_system: String,
    architecture: String,
    logical_parallelism: usize,
    rustc: Option<String>,
    git_commit: Option<String>,
    uname: Option<String>,
    corpus_manifest_sha256: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-naive-proof: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_gates(&command)?;
    if command.output.exists() {
        if command.no_replace {
            return Err(invalid(format!(
                "output directory {} already exists",
                command.output.display()
            )));
        }
        fs::remove_dir_all(&command.output)?;
    }
    fs::create_dir_all(&command.output)?;
    let options = NaiveProofOptions {
        maximum_documents: command.maximum_documents,
        query_count: command.queries,
        passage_words: command.passage_words,
        hard_negatives: command.hard_negatives,
        maximum_total_cells: command.maximum_total_cells,
        repetitions: command.repetitions,
        seed: command.seed,
    };
    let report = run_naive_proof(&command.corpus_root, &options)?;
    let completed_fraction = report.completed_queries as f64 / report.requested_queries as f64;
    if completed_fraction < command.minimum_completed_fraction {
        return Err(invalid(format!(
            "completed fraction {completed_fraction:.4} is below required {:.4}",
            command.minimum_completed_fraction
        )));
    }
    if command
        .minimum_p95_speedup
        .is_some_and(|minimum| report.p95_speedup_over_naive < minimum)
    {
        return Err(invalid(format!(
            "p95 speedup {:.4} is below required {:.4}",
            report.p95_speedup_over_naive,
            command.minimum_p95_speedup.unwrap_or_default()
        )));
    }
    if command
        .minimum_macro_auprc_delta
        .is_some_and(|minimum| report.quality.macro_auprc_delta < minimum)
    {
        return Err(invalid(format!(
            "macro AUPRC delta {:+.6} is below required {:+.6}",
            report.quality.macro_auprc_delta,
            command.minimum_macro_auprc_delta.unwrap_or_default()
        )));
    }

    let manifest_bytes = fs::read(command.corpus_root.join(MANIFEST_FILENAME))?;
    let environment = EnvironmentReport {
        generated_at_unix: unix_timestamp(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        logical_parallelism: std::thread::available_parallelism().map_or(1, |value| value.get()),
        rustc: command_output("rustc", &["-Vv"]),
        git_commit: command_output("git", &["rev-parse", "HEAD"]),
        uname: command_output("uname", &["-a"]),
        corpus_manifest_sha256: sha256_hex(&manifest_bytes),
    };
    let report_path = command.output.join("report.json");
    let examples_path = command.output.join("EXAMPLES.md");
    let environment_path = command.output.join("environment.json");
    atomic_write(&report_path, &serde_json::to_vec_pretty(&report)?)?;
    atomic_write(&examples_path, render_naive_proof(&report).as_bytes())?;
    atomic_write(&environment_path, &serde_json::to_vec_pretty(&environment)?)?;
    write_checksums(
        &command.output,
        &[&report_path, &examples_path, &environment_path],
    )?;

    println!("Naive proof bundle: {}", command.output.display());
    println!("Completed queries:  {}", report.completed_queries);
    println!("Skipped queries:    {}", report.skipped_queries);
    println!(
        "DP cells:           {}",
        report.total_dynamic_programming_cells
    );
    println!("p95 speedup:        {:.3}×", report.p95_speedup_over_naive);
    println!(
        "Macro AUPRC delta: {:+.6}",
        report.quality.macro_auprc_delta
    );
    Ok(())
}

fn validate_gates(command: &Cli) -> CliResult<()> {
    if !command.minimum_completed_fraction.is_finite()
        || !(0.0..=1.0).contains(&command.minimum_completed_fraction)
        || command
            .minimum_p95_speedup
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        || command
            .minimum_macro_auprc_delta
            .is_some_and(|value| !value.is_finite())
    {
        return Err(invalid("proof gates are invalid"));
    }
    Ok(())
}

fn write_checksums(root: &Path, paths: &[&Path]) -> CliResult<()> {
    let mut entries = Vec::new();
    for path in paths {
        let bytes = fs::read(path)?;
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push((relative, sha256_hex(&bytes)));
    }
    entries.sort_unstable();
    let text = entries
        .into_iter()
        .map(|(path, digest)| format!("{digest}  {path}\n"))
        .collect::<String>();
    atomic_write(&root.join("SHA256SUMS"), text.as_bytes())?;
    Ok(())
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
