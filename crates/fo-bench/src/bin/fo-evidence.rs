#![forbid(unsafe_code)]

#[path = "../evidence.rs"]
mod evidence;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use evidence::{BenchmarkReport, EvidenceOptions, build_evidence, read_score_rows, render_markdown};
use fo_corpus::{atomic_write, sha256_hex, unix_timestamp};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-evidence",
    version,
    about = "Turn fo-real-bench reports and score streams into auditable evidence bundles"
)]
struct Cli {
    report: PathBuf,
    scores: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value = "franken_hybrid")]
    method: String,
    #[arg(long = "baseline")]
    baselines: Vec<String>,
    #[arg(long, default_value_t = 2_000)]
    bootstrap_samples: usize,
    #[arg(long, default_value_t = 0.95)]
    confidence_level: f64,
    #[arg(long, default_value_t = 0x65_76_69_64_65_6e_63_65)]
    seed: u64,
    #[arg(long, default_value_t = 5)]
    top_candidates: usize,
    #[arg(long, default_value_t = 0.0)]
    minimum_macro_delta_lower_bound: f64,
    #[arg(long, default_value_t = 0.0)]
    maximum_recall_at_1_regression: f64,
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
    report_path: String,
    report_sha256: String,
    scores_path: String,
    scores_sha256: String,
}

#[derive(Debug, Serialize)]
struct BundleFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct BundleManifest {
    schema_version: u32,
    generated_at_unix: u64,
    files: Vec<BundleFile>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-evidence: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
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

    let report_bytes = fs::read(&command.report)?;
    let scores_bytes = fs::read(&command.scores)?;
    let benchmark = serde_json::from_slice::<BenchmarkReport>(&report_bytes)?;
    let rows = read_score_rows(&command.scores)?;
    let options = EvidenceOptions {
        selected_method: command.method,
        baselines: command.baselines,
        bootstrap_samples: command.bootstrap_samples,
        confidence_level: command.confidence_level,
        seed: command.seed,
        top_candidates: command.top_candidates,
        minimum_macro_delta_lower_bound: command.minimum_macro_delta_lower_bound,
        maximum_recall_at_1_regression: command.maximum_recall_at_1_regression,
    };
    let generated_at_unix = unix_timestamp();
    let report = build_evidence(&benchmark, &rows, &options, generated_at_unix)?;
    let environment = EnvironmentReport {
        generated_at_unix,
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        logical_parallelism: std::thread::available_parallelism().map_or(1, |value| value.get()),
        rustc: command_output("rustc", &["-Vv"]),
        git_commit: command_output("git", &["rev-parse", "HEAD"]),
        uname: command_output("uname", &["-a"]),
        report_path: command.report.display().to_string(),
        report_sha256: sha256_hex(&report_bytes),
        scores_path: command.scores.display().to_string(),
        scores_sha256: sha256_hex(&scores_bytes),
    };

    let evidence_path = command.output.join("evidence.json");
    let examples_path = command.output.join("EXAMPLES.md");
    let environment_path = command.output.join("environment.json");
    atomic_write(&evidence_path, &serde_json::to_vec_pretty(&report)?)?;
    atomic_write(&examples_path, render_markdown(&report).as_bytes())?;
    atomic_write(
        &environment_path,
        &serde_json::to_vec_pretty(&environment)?,
    )?;
    let bundle = write_bundle(&command.output, &[&evidence_path, &examples_path, &environment_path])?;

    println!("Evidence bundle: {}", command.output.display());
    println!("Corpus:          {}", report.corpus_id);
    println!("Queries / pairs: {} / {}", report.queries, report.pairs);
    println!("Selected method: {}", report.selected_method);
    println!("Best baseline:   {}", report.verdict.best_baseline);
    println!("Verdict:         {}", report.verdict.claim);
    println!("Bundle files:    {}", bundle.files.len());
    Ok(())
}

fn write_bundle(root: &Path, paths: &[&Path]) -> CliResult<BundleManifest> {
    let mut files = Vec::new();
    for path in paths {
        let bytes = fs::read(path)?;
        files.push(BundleFile {
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/"),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let manifest = BundleManifest {
        schema_version: evidence::EVIDENCE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        files,
    };
    let manifest_path = root.join("manifest.json");
    atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    let mut sums = String::new();
    for file in &manifest.files {
        sums.push_str(&format!("{}  {}\n", file.sha256, file.path));
    }
    sums.push_str(&format!(
        "{}  manifest.json\n",
        sha256_hex(&fs::read(&manifest_path)?)
    ));
    atomic_write(&root.join("SHA256SUMS"), sums.as_bytes())?;
    Ok(manifest)
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
