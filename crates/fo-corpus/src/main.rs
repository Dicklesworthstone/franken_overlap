#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_corpus::{
    fetch_gutenberg, fetch_sec_10k, verify_manifest, CorpusManifest, GutenbergOptions,
    GutenbergPreset, Sec10KOptions, SecPreset, DEFAULT_GUTENBERG_MIRROR,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-corpus",
    version,
    about = "Reproducible corpus acquisition for FrankenOverlap"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List built-in corpus providers and benchmark tiers.
    List,
    /// Download a deterministic Project Gutenberg text corpus.
    Gutenberg(GutenbergCommand),
    /// Download recent SEC Form 10-K primary documents.
    Sec10k(Sec10KCommand),
    /// Verify every document against manifest sizes and SHA-256 digests.
    Verify {
        root: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Print one corpus manifest.
    Show {
        root: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct GutenbergCommand {
    #[arg(short, long, default_value = "corpora/gutenberg")]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "smoke")]
    preset: GutenbergPresetArg,
    /// Override the preset document count.
    #[arg(long)]
    limit: Option<usize>,
    /// Download explicit Project Gutenberg IDs instead of sampling the catalog.
    #[arg(long = "id")]
    ids: Vec<u64>,
    #[arg(long, default_value = "en")]
    language: String,
    #[arg(long, default_value_t = 10_000)]
    minimum_characters: usize,
    #[arg(long, default_value_t = 32 * 1024 * 1024)]
    maximum_document_bytes: u64,
    /// Project Gutenberg mirror base ending at cache/epub.
    #[arg(long)]
    mirror_base: Option<String>,
    #[arg(long, default_value_t = 0x67_75_74_65_6e_62_65_72)]
    seed: u64,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    refresh_catalog: bool,
    #[arg(long, default_value_t = 2_000)]
    request_interval_ms: u64,
    #[arg(long, default_value_t = 4)]
    maximum_attempts: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct Sec10KCommand {
    #[arg(short, long, default_value = "corpora/sec-10k")]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "smoke")]
    preset: SecPresetArg,
    #[arg(long = "ticker")]
    tickers: Vec<String>,
    #[arg(long = "cik")]
    ciks: Vec<u64>,
    /// Deterministically sample this many current SEC ticker associations.
    #[arg(long)]
    sampled_companies: Option<usize>,
    #[arg(long, default_value_t = 3)]
    filings_per_company: usize,
    #[arg(long)]
    from_date: Option<String>,
    #[arg(long)]
    to_date: Option<String>,
    #[arg(long)]
    include_amendments: bool,
    #[arg(long, default_value_t = 25_000)]
    minimum_characters: usize,
    #[arg(long, default_value_t = 96 * 1024 * 1024)]
    maximum_document_bytes: u64,
    /// Required SEC bot identity, e.g. "Example Research research@example.com".
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long, default_value_t = 5.0)]
    requests_per_second: f64,
    #[arg(long, default_value_t = 5)]
    maximum_attempts: usize,
    #[arg(long)]
    overwrite: bool,
    #[arg(long, default_value_t = 0x53_45_43_2d_31_30_4b)]
    seed: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GutenbergPresetArg {
    Smoke,
    Standard,
    Large,
}

impl From<GutenbergPresetArg> for GutenbergPreset {
    fn from(value: GutenbergPresetArg) -> Self {
        match value {
            GutenbergPresetArg::Smoke => Self::Smoke,
            GutenbergPresetArg::Standard => Self::Standard,
            GutenbergPresetArg::Large => Self::Large,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SecPresetArg {
    Smoke,
    Standard,
    Large,
}

impl From<SecPresetArg> for SecPreset {
    fn from(value: SecPresetArg) -> Self {
        match value {
            SecPresetArg::Smoke => Self::Smoke,
            SecPresetArg::Standard => Self::Standard,
            SecPresetArg::Large => Self::Large,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-corpus: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::List => print_registry(),
        Command::Gutenberg(command) => run_gutenberg(command)?,
        Command::Sec10k(command) => run_sec(command)?,
        Command::Verify { root, json } => {
            let report = verify_manifest(root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Corpus:      {}", report.corpus_id);
                println!("Documents:   {}", report.documents);
                println!("Verified:    {}", report.verified);
                println!("Total bytes: {}", report.total_bytes);
            }
        }
        Command::Show { root, json } => {
            let manifest = CorpusManifest::load(root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                print_manifest_summary(&manifest);
            }
        }
    }
    Ok(())
}

fn run_gutenberg(command: GutenbergCommand) -> CliResult<()> {
    let mirror_base = command
        .mirror_base
        .or_else(|| std::env::var("GUTENBERG_MIRROR").ok())
        .unwrap_or_else(|| DEFAULT_GUTENBERG_MIRROR.to_owned());
    let report = fetch_gutenberg(GutenbergOptions {
        output_dir: command.output,
        preset: command.preset.into(),
        document_limit: command.limit,
        explicit_ids: command.ids,
        language: command.language,
        minimum_characters: command.minimum_characters,
        maximum_document_bytes: command.maximum_document_bytes,
        mirror_base,
        seed: command.seed,
        overwrite: command.overwrite,
        refresh_catalog: command.refresh_catalog,
        request_interval: Duration::from_millis(command.request_interval_ms),
        maximum_attempts: command.maximum_attempts,
        ..GutenbergOptions::default()
    })?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_manifest_summary(&report.manifest);
        println!("Catalog candidates: {}", report.catalog_candidates);
        println!("Requested:          {}", report.requested);
        println!("Downloaded:         {}", report.downloaded);
        println!("Reused:             {}", report.reused);
        println!("Rejected too short: {}", report.rejected_too_short);
        println!("Failed:             {}", report.failed);
    }
    Ok(())
}

fn run_sec(command: Sec10KCommand) -> CliResult<()> {
    let user_agent = command
        .user_agent
        .or_else(|| std::env::var("SEC_USER_AGENT").ok())
        .unwrap_or_default();
    let report = fetch_sec_10k(Sec10KOptions {
        output_dir: command.output,
        preset: command.preset.into(),
        tickers: command.tickers,
        ciks: command.ciks,
        sampled_companies: command.sampled_companies,
        filings_per_company: command.filings_per_company,
        from_date: command.from_date,
        to_date: command.to_date,
        include_amendments: command.include_amendments,
        minimum_characters: command.minimum_characters,
        maximum_document_bytes: command.maximum_document_bytes,
        user_agent,
        requests_per_second: command.requests_per_second,
        maximum_attempts: command.maximum_attempts,
        overwrite: command.overwrite,
        seed: command.seed,
    })?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_manifest_summary(&report.manifest);
        println!("Companies:          {}", report.companies);
        println!("Candidate filings:  {}", report.candidate_filings);
        println!("Downloaded:         {}", report.downloaded);
        println!("Reused:             {}", report.reused);
        println!("Rejected too short: {}", report.rejected_too_short);
        println!("Failed:             {}", report.failed);
    }
    Ok(())
}

fn print_registry() {
    println!("project-gutenberg");
    println!("  smoke:    25 English texts");
    println!("  standard: 250 English texts; requires an explicit mirror");
    println!("  large:    2500 English texts; requires an explicit mirror");
    println!("sec-edgar-10k");
    println!("  smoke:    3 companies × 3 recent 10-K filings");
    println!("  standard: 25 companies × 3 recent 10-K filings");
    println!("  large:    100 companies × 3 recent 10-K filings");
}

fn print_manifest_summary(manifest: &CorpusManifest) {
    let bytes = manifest
        .documents
        .iter()
        .map(|document| document.bytes)
        .sum::<u64>();
    println!("Corpus:    {}", manifest.corpus_id);
    println!("Provider:  {:?}", manifest.provider);
    println!("Documents: {}", manifest.documents.len());
    println!("Bytes:     {bytes}");
    println!("Failures:  {}", manifest.failures.len());
}
