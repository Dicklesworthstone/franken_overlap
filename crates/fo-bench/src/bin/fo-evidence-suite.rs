#![forbid(unsafe_code)]
#![allow(clippy::too_many_arguments)]

#[allow(dead_code, unused_imports)]
#[path = "../claim_gates.rs"]
mod claim_gates;
#[allow(dead_code, unused_imports)]
#[path = "../evidence_bundle.rs"]
mod evidence_bundle;
#[allow(dead_code, unused_imports)]
#[path = "../retrieval_baselines.rs"]
mod retrieval_baselines;
#[allow(dead_code, unused_imports)]
#[path = "../scenario_benchmark.rs"]
mod scenario_benchmark;

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use claim_gates::{ClaimGateReport, ClaimVerdict, evaluate_claims};
use clap::Parser;
use evidence_bundle::{BundleOptions, BundleReport, render_evidence_bundle};
use fo_corpus::{atomic_write, sha256_hex, unix_timestamp};
use scenario_benchmark::{
    ScenarioBenchmarkOptions, read_queries, run_scenario_benchmark, write_score_rows,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const SUITE_SCHEMA_VERSION: u32 = 1;
const STATUS_FILE: &str = "suite-status.json";
const SUITE_FILE: &str = "suite.json";
const PROOF_FILE: &str = "proof.json";
const SCORES_FILE: &str = "scores.jsonl";
const CLAIMS_FILE: &str = "claims.json";
const BUNDLE_DIRECTORY: &str = "bundle";

#[derive(Debug, Parser)]
#[command(
    name = "fo-evidence-suite",
    version,
    about = "Run the complete benchmark, claim-gate, and immutable evidence-bundle workflow"
)]
struct Cli {
    /// Searchable fo-corpus root, normally created by fo-showcase.
    corpus_root: PathBuf,
    /// Generated or adjudicated query JSONL.
    queries: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    /// Optional preregistered claim-gate manifest.
    #[arg(long)]
    claim_manifest: Option<PathBuf>,
    /// Optional fo-adjudicate validation report included in the bundle receipts.
    #[arg(long)]
    gold_validation: Option<PathBuf>,
    #[arg(long = "corpus-size")]
    corpus_sizes: Vec<usize>,
    #[arg(long, default_value_t = 250)]
    maximum_documents: usize,
    #[arg(long, default_value_t = usize::MAX)]
    maximum_queries: usize,
    #[arg(long = "profile")]
    profiles: Vec<String>,
    #[arg(long, default_value_t = 1)]
    warmup_runs: usize,
    #[arg(long, default_value_t = 3)]
    measurement_repetitions: usize,
    #[arg(long, default_value_t = 5)]
    qgram_size: usize,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_document_bytes: u64,
    #[arg(long, default_value_t = 2_000_000_000)]
    maximum_exhaustive_cells_per_query: u64,
    #[arg(long, default_value_t = 20_000_000_000)]
    maximum_exhaustive_cells_per_scale: u64,
    #[arg(long, default_value_t = 0x65_76_69_64_65_6e_63_65)]
    seed: u64,
    #[arg(long)]
    retain_indexes: bool,
    #[arg(long, default_value_t = 3)]
    examples_per_profile: usize,
    #[arg(long, default_value_t = 5)]
    top_candidates_per_method: usize,
    #[arg(long, default_value_t = 180)]
    snippet_tokens: usize,
    #[arg(long, default_value = "FrankenOverlap evidence report")]
    title: String,
    /// Return an error after producing the suite unless every claim is supported.
    #[arg(long)]
    require_supported: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct StatusReport {
    schema_version: u32,
    status: String,
    started_at_unix: u64,
    completed_at_unix: Option<u64>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct FileReceipt {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
struct SuiteReport {
    schema_version: u32,
    generated_at_unix: u64,
    status: String,
    claim_status: String,
    corpus_root: String,
    query_path: String,
    output_directory: String,
    proof: FileReceipt,
    scores: FileReceipt,
    claims: Option<FileReceipt>,
    claim_manifest: Option<FileReceipt>,
    gold_validation: Option<FileReceipt>,
    bundle: BundleReport,
    suite_file: String,
    profiles: Vec<String>,
    evaluated_corpus_sizes: Vec<usize>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-evidence-suite: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    if command.output.exists() {
        return Err(invalid(format!(
            "{} already exists; evidence suites are immutable",
            command.output.display()
        )));
    }
    fs::create_dir_all(&command.output)?;
    let started_at = unix_timestamp();
    let status_path = command.output.join(STATUS_FILE);
    write_status(
        &status_path,
        &StatusReport {
            schema_version: SUITE_SCHEMA_VERSION,
            status: "running".to_owned(),
            started_at_unix: started_at,
            completed_at_unix: None,
            message: "benchmark and evidence generation in progress".to_owned(),
        },
    )?;

    let result = run_suite(&command);
    match result {
        Ok(report) => {
            let suite_path = command.output.join(SUITE_FILE);
            atomic_write(&suite_path, &serde_json::to_vec_pretty(&report)?)?;
            write_status(
                &status_path,
                &StatusReport {
                    schema_version: SUITE_SCHEMA_VERSION,
                    status: "complete".to_owned(),
                    started_at_unix: started_at,
                    completed_at_unix: Some(unix_timestamp()),
                    message: format!("claim status: {}", report.claim_status),
                },
            )?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
            if command.require_supported && report.claim_status != "supported" {
                return Err(invalid(format!(
                    "evidence suite completed with claim status {}; supported was required",
                    report.claim_status
                )));
            }
            Ok(())
        }
        Err(error) => {
            write_status(
                &status_path,
                &StatusReport {
                    schema_version: SUITE_SCHEMA_VERSION,
                    status: "failed".to_owned(),
                    started_at_unix: started_at,
                    completed_at_unix: Some(unix_timestamp()),
                    message: error.to_string(),
                },
            )?;
            Err(error)
        }
    }
}

fn run_suite(command: &Cli) -> CliResult<SuiteReport> {
    let queries = read_queries(&command.queries)?;
    let proof_path = command.output.join(PROOF_FILE);
    let scores_path = command.output.join(SCORES_FILE);
    let claims_path = command.output.join(CLAIMS_FILE);
    let bundle_path = command.output.join(BUNDLE_DIRECTORY);
    let index_root = command
        .retain_indexes
        .then(|| command.output.join("indexes"));
    let options = ScenarioBenchmarkOptions {
        requested_corpus_sizes: command.corpus_sizes.clone(),
        maximum_documents: command.maximum_documents,
        maximum_queries: command.maximum_queries,
        profiles: command.profiles.iter().cloned().collect::<BTreeSet<_>>(),
        warmup_runs: command.warmup_runs,
        measurement_repetitions: command.measurement_repetitions,
        qgram_size: command.qgram_size,
        maximum_document_bytes: command.maximum_document_bytes,
        maximum_exhaustive_cells_per_query: command.maximum_exhaustive_cells_per_query,
        maximum_exhaustive_cells_per_scale: command.maximum_exhaustive_cells_per_scale,
        seed: command.seed,
        index_root,
        retain_indexes: command.retain_indexes,
    };
    let (proof, rows) = run_scenario_benchmark(
        &command.corpus_root,
        &command.queries,
        queries,
        &options,
    )?;
    atomic_write(&proof_path, &serde_json::to_vec_pretty(&proof)?)?;
    write_score_rows(&scores_path, &rows)?;

    let claim_report = command
        .claim_manifest
        .as_ref()
        .map(|manifest| evaluate_claims(&proof_path, &scores_path, manifest))
        .transpose()?;
    if let Some(report) = &claim_report {
        atomic_write(&claims_path, &serde_json::to_vec_pretty(report)?)?;
    }
    let claim_status = claim_status(claim_report.as_ref());
    let bundle = render_evidence_bundle(
        &command.corpus_root,
        &command.queries,
        &proof_path,
        &scores_path,
        claim_report.as_ref().map(|_| claims_path.as_path()),
        command.gold_validation.as_deref(),
        &bundle_path,
        &BundleOptions {
            examples_per_profile: command.examples_per_profile,
            top_candidates_per_method: command.top_candidates_per_method,
            snippet_tokens: command.snippet_tokens,
            title: command.title.clone(),
        },
    )?;
    Ok(SuiteReport {
        schema_version: SUITE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        status: "complete".to_owned(),
        claim_status,
        corpus_root: command.corpus_root.display().to_string(),
        query_path: command.queries.display().to_string(),
        output_directory: command.output.display().to_string(),
        proof: receipt(&proof_path)?,
        scores: receipt(&scores_path)?,
        claims: claim_report.as_ref().map(|_| receipt(&claims_path)).transpose()?,
        claim_manifest: command
            .claim_manifest
            .as_deref()
            .map(receipt)
            .transpose()?,
        gold_validation: command
            .gold_validation
            .as_deref()
            .map(receipt)
            .transpose()?,
        bundle,
        suite_file: command.output.join(SUITE_FILE).display().to_string(),
        profiles: proof.profiles,
        evaluated_corpus_sizes: proof.evaluated_corpus_sizes,
    })
}

fn claim_status(report: Option<&ClaimGateReport>) -> String {
    match report {
        None => "not_evaluated".to_owned(),
        Some(report) if report.all_supported => "supported".to_owned(),
        Some(report)
            if report
                .comparisons
                .iter()
                .any(|comparison| comparison.verdict == ClaimVerdict::Unsupported) =>
        {
            "unsupported".to_owned()
        }
        Some(_) => "inconclusive".to_owned(),
    }
}

fn receipt(path: &Path) -> CliResult<FileReceipt> {
    let bytes = fs::read(path)?;
    Ok(FileReceipt {
        path: path.display().to_string(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    })
}

fn write_status(path: &Path, status: &StatusReport) -> CliResult<()> {
    atomic_write(path, &serde_json::to_vec_pretty(status)?)?;
    Ok(())
}

fn print_report(report: &SuiteReport) {
    println!("Evidence suite:          {}", report.output_directory);
    println!("Status:                  {}", report.status);
    println!("Claim status:            {}", report.claim_status);
    println!("Profiles:                {}", report.profiles.join(", "));
    println!("Corpus sizes:            {:?}", report.evaluated_corpus_sizes);
    println!("Proof report:            {}", report.proof.path);
    println!("Pair scores:             {}", report.scores.path);
    if let Some(claims) = &report.claims {
        println!("Claim report:            {}", claims.path);
    }
    println!("Markdown:                {}", report.bundle.results_markdown);
    println!("HTML:                    {}", report.bundle.results_html);
    println!("Suite manifest:          {}", report.suite_file);
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
