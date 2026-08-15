#![forbid(unsafe_code)]

#[allow(dead_code, unused_imports)]
#[path = "../evidence_bundle.rs"]
mod evidence_bundle;

use std::error::Error;
use std::path::PathBuf;

use clap::Parser;
use evidence_bundle::{BundleOptions, render_evidence_bundle};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-proof-report",
    version,
    about = "Render immutable Markdown/HTML evidence bundles from proof-benchmark receipts"
)]
struct Cli {
    corpus_root: PathBuf,
    queries: PathBuf,
    proof_report: PathBuf,
    scores: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    claim_report: Option<PathBuf>,
    #[arg(long)]
    gold_validation: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    examples_per_profile: usize,
    #[arg(long, default_value_t = 5)]
    top_candidates_per_method: usize,
    #[arg(long, default_value_t = 180)]
    snippet_tokens: usize,
    #[arg(long, default_value = "FrankenOverlap evidence report")]
    title: String,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-proof-report: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let report = render_evidence_bundle(
        &command.corpus_root,
        &command.queries,
        &command.proof_report,
        &command.scores,
        command.claim_report.as_deref(),
        command.gold_validation.as_deref(),
        &command.output,
        &BundleOptions {
            examples_per_profile: command.examples_per_profile,
            top_candidates_per_method: command.top_candidates_per_method,
            snippet_tokens: command.snippet_tokens,
            title: command.title,
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Evidence directory:       {}", report.output_directory);
        println!("Claim status:             {}", report.claim_status);
        println!("Representative examples: {}", report.examples);
        println!("Markdown:                 {}", report.results_markdown);
        println!("HTML:                     {}", report.results_html);
        println!("Environment:              {}", report.environment_json);
        println!("Examples:                 {}", report.examples_json);
        println!("Artifact manifest:        {}", report.artifacts_json);
    }
    Ok(())
}
