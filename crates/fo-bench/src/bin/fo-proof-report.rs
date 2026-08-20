#![forbid(unsafe_code)]

#[allow(dead_code, unused_imports)]
#[path = "../evidence_bundle.rs"]
mod evidence_bundle;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use evidence_bundle::{BundleOptions, render_evidence_bundle};
use fo_corpus::sha256_hex;

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
    validate_proof_receipts(
        &command.corpus_root,
        &command.queries,
        &command.proof_report,
    )?;
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

fn validate_proof_receipts(
    corpus_root: &Path,
    queries: &Path,
    proof_report: &Path,
) -> CliResult<()> {
    let proof = serde_json::from_slice::<serde_json::Value>(&fs::read(proof_report)?)?;
    let expected_manifest = proof
        .get("corpus_manifest_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("proof report has no corpus_manifest_sha256"))?;
    let expected_queries = proof
        .get("query_file_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("proof report has no query_file_sha256"))?;
    let manifest_path = corpus_root.join(fo_corpus::MANIFEST_FILENAME);
    let observed_manifest = sha256_hex(&fs::read(&manifest_path)?);
    let observed_queries = sha256_hex(&fs::read(queries)?);
    if observed_manifest != expected_manifest {
        return Err(invalid(format!(
            "corpus manifest digest mismatch: proof expects {expected_manifest}, observed {observed_manifest}"
        )));
    }
    if observed_queries != expected_queries {
        return Err(invalid(format!(
            "query digest mismatch: proof expects {expected_queries}, observed {observed_queries}"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
