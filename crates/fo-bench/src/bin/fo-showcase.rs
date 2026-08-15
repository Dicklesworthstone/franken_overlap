#![forbid(unsafe_code)]

#[path = "../showcase.rs"]
mod showcase;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_corpus::{
    CorpusManifest, GutenbergOptions, GutenbergPreset, Sec10KOptions, SecPreset,
    SectionCorpusOptions, SectionStrategy, atomic_write, fetch_gutenberg, fetch_sec_10k,
    section_corpus, sha256_hex, unix_timestamp, verify_manifest,
};
use serde::Serialize;
use showcase::{ScenarioGenerationOptions, ScenarioGenerationReport, generate_scenarios};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const SHOWCASE_REPORT_SCHEMA_VERSION: u32 = 1;
const DEFAULT_GUTENBERG_IDS: &[u64] = &[84, 41445, 42324, 1661, 2701, 11, 1342, 98, 2600, 345];
const DEFAULT_SEC_TICKERS: &[&str] = &["AAPL", "MSFT", "NVDA", "JPM", "WMT"];

#[derive(Debug, Parser)]
#[command(
    name = "fo-showcase",
    version,
    about = "Prepare reproducible Gutenberg and SEC showcase corpora and labeled query scenarios"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download fixed public-domain books, derive chapters, and generate scenarios.
    Gutenberg(GutenbergCommand),
    /// Download real 10-Ks, derive filing items, and generate scenarios.
    Sec10k(Sec10KCommand),
    /// Generate scenarios from an already prepared fo-corpus, optionally sectioning it first.
    Existing(ExistingCommand),
}

#[derive(Debug, Args)]
struct CommonScenarioArgs {
    #[arg(long, default_value_t = 24)]
    source_documents: usize,
    #[arg(long, default_value_t = 8)]
    queries_per_source: usize,
    #[arg(long, default_value_t = 96)]
    passage_words: usize,
    #[arg(long, default_value_t = 0x73_68_6f_77_63_61_73_65)]
    seed: u64,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_document_bytes: u64,
}

#[derive(Debug, Args)]
struct GutenbergCommand {
    #[arg(short, long, default_value = "showcase/gutenberg")]
    output: PathBuf,
    /// Explicit Gutenberg IDs. The curated edition/diversity set is used when omitted.
    #[arg(long = "id")]
    ids: Vec<u64>,
    /// Project Gutenberg mirror base ending at cache/epub.
    #[arg(long)]
    mirror_base: Option<String>,
    #[arg(long, default_value_t = 2_000)]
    request_interval_ms: u64,
    #[arg(long, default_value_t = 4)]
    maximum_attempts: usize,
    #[arg(long)]
    refresh_downloads: bool,
    #[arg(long)]
    rebuild_sections: bool,
    #[arg(long)]
    replace_output: bool,
    #[command(flatten)]
    scenario: CommonScenarioArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct Sec10KCommand {
    #[arg(short, long, default_value = "showcase/sec-10k")]
    output: PathBuf,
    /// Tickers to acquire. AAPL, MSFT, NVDA, JPM, and WMT are used when omitted.
    #[arg(long = "ticker")]
    tickers: Vec<String>,
    #[arg(long, default_value_t = 5)]
    filings_per_company: usize,
    #[arg(long, default_value = "2018-01-01")]
    from_date: String,
    #[arg(long)]
    to_date: Option<String>,
    #[arg(long)]
    include_amendments: bool,
    /// Required SEC identity, e.g. "Example Research research@example.com".
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long, default_value_t = 5.0)]
    requests_per_second: f64,
    #[arg(long, default_value_t = 5)]
    maximum_attempts: usize,
    #[arg(long)]
    refresh_downloads: bool,
    #[arg(long)]
    rebuild_sections: bool,
    #[arg(long)]
    replace_output: bool,
    #[command(flatten)]
    scenario: CommonScenarioArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ExistingCommand {
    corpus_root: PathBuf,
    #[arg(short, long, default_value = "showcase/existing")]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    strategy: SectionStrategyArg,
    /// Use the input corpus directly rather than deriving sections/windows.
    #[arg(long)]
    no_sectioning: bool,
    #[arg(long)]
    rebuild_sections: bool,
    #[arg(long)]
    replace_output: bool,
    #[command(flatten)]
    scenario: CommonScenarioArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SectionStrategyArg {
    Auto,
    Gutenberg,
    Sec10k,
    ParagraphWindows,
}

impl From<SectionStrategyArg> for SectionStrategy {
    fn from(value: SectionStrategyArg) -> Self {
        match value {
            SectionStrategyArg::Auto => Self::Auto,
            SectionStrategyArg::Gutenberg => Self::Gutenberg,
            SectionStrategyArg::Sec10k => Self::Sec10K,
            SectionStrategyArg::ParagraphWindows => Self::ParagraphWindows,
        }
    }
}

#[derive(Debug, Serialize)]
struct ManifestReceipt {
    root: String,
    manifest_path: String,
    manifest_sha256: String,
    corpus_id: String,
    documents: usize,
    total_bytes: u64,
}

#[derive(Debug, Serialize)]
struct ShowcasePreparationReport {
    schema_version: u32,
    generated_at_unix: u64,
    provider: String,
    output_root: String,
    raw: ManifestReceipt,
    searchable: ManifestReceipt,
    scenarios: ScenarioGenerationReport,
    report_path: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-showcase: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Gutenberg(command) => run_gutenberg(command),
        Command::Sec10k(command) => run_sec(command),
        Command::Existing(command) => run_existing(command),
    }
}

fn run_gutenberg(command: GutenbergCommand) -> CliResult<()> {
    prepare_output(&command.output, command.replace_output)?;
    let raw_root = command.output.join("raw");
    let section_root = command.output.join("sections");
    let ids = if command.ids.is_empty() {
        DEFAULT_GUTENBERG_IDS.to_vec()
    } else {
        command.ids
    };
    let mirror_base = command
        .mirror_base
        .or_else(|| std::env::var("GUTENBERG_MIRROR").ok())
        .unwrap_or_else(|| fo_corpus::DEFAULT_GUTENBERG_MIRROR.to_owned());
    fetch_gutenberg(GutenbergOptions {
        output_dir: raw_root.clone(),
        preset: GutenbergPreset::Smoke,
        document_limit: Some(ids.len()),
        explicit_ids: ids,
        mirror_base,
        overwrite: command.refresh_downloads,
        refresh_catalog: command.refresh_downloads,
        request_interval: Duration::from_millis(command.request_interval_ms),
        maximum_attempts: command.maximum_attempts,
        ..GutenbergOptions::default()
    })?;
    let searchable = load_or_section(
        &raw_root,
        &section_root,
        SectionStrategy::Gutenberg,
        command.rebuild_sections,
    )?;
    finalize_showcase(
        "project_gutenberg",
        &command.output,
        &raw_root,
        searchable,
        command.scenario,
        command.json,
    )
}

fn run_sec(command: Sec10KCommand) -> CliResult<()> {
    prepare_output(&command.output, command.replace_output)?;
    let raw_root = command.output.join("raw");
    let section_root = command.output.join("sections");
    let tickers = if command.tickers.is_empty() {
        DEFAULT_SEC_TICKERS
            .iter()
            .map(|ticker| (*ticker).to_owned())
            .collect()
    } else {
        command.tickers
    };
    let user_agent = command
        .user_agent
        .or_else(|| std::env::var("SEC_USER_AGENT").ok())
        .unwrap_or_default();
    fetch_sec_10k(Sec10KOptions {
        output_dir: raw_root.clone(),
        preset: SecPreset::Smoke,
        tickers,
        filings_per_company: command.filings_per_company,
        from_date: Some(command.from_date),
        to_date: command.to_date,
        include_amendments: command.include_amendments,
        user_agent,
        requests_per_second: command.requests_per_second,
        maximum_attempts: command.maximum_attempts,
        overwrite: command.refresh_downloads,
        ..Sec10KOptions::default()
    })?;
    let searchable = load_or_section(
        &raw_root,
        &section_root,
        SectionStrategy::Sec10K,
        command.rebuild_sections,
    )?;
    finalize_showcase(
        "sec_edgar_10k",
        &command.output,
        &raw_root,
        searchable,
        command.scenario,
        command.json,
    )
}

fn run_existing(command: ExistingCommand) -> CliResult<()> {
    prepare_output(&command.output, command.replace_output)?;
    let raw = CorpusManifest::load(&command.corpus_root)?;
    verify_manifest(&command.corpus_root)?;
    let searchable_root = if command.no_sectioning {
        command.corpus_root.clone()
    } else {
        let section_root = command.output.join("sections");
        load_or_section(
            &command.corpus_root,
            &section_root,
            command.strategy.into(),
            command.rebuild_sections,
        )?
    };
    let provider = format!("{:?}", raw.provider).to_ascii_lowercase();
    finalize_showcase(
        &provider,
        &command.output,
        &command.corpus_root,
        searchable_root,
        command.scenario,
        command.json,
    )
}

fn load_or_section(
    raw_root: &Path,
    section_root: &Path,
    strategy: SectionStrategy,
    rebuild: bool,
) -> CliResult<PathBuf> {
    let manifest_path = section_root.join(fo_corpus::MANIFEST_FILENAME);
    if manifest_path.is_file() && !rebuild {
        CorpusManifest::load(section_root)?;
        verify_manifest(section_root)?;
        return Ok(section_root.to_owned());
    }
    section_corpus(
        raw_root,
        SectionCorpusOptions {
            output_dir: section_root.to_owned(),
            strategy,
            minimum_characters: 1_500,
            target_characters: 16_000,
            maximum_characters: 32_000,
            overlap_characters: 800,
            maximum_sections_per_document: 512,
            replace_output: rebuild && section_root.exists(),
        },
    )?;
    verify_manifest(section_root)?;
    Ok(section_root.to_owned())
}

fn finalize_showcase(
    provider: &str,
    output_root: &Path,
    raw_root: &Path,
    searchable_root: PathBuf,
    scenario: CommonScenarioArgs,
    json: bool,
) -> CliResult<()> {
    verify_manifest(raw_root)?;
    verify_manifest(&searchable_root)?;
    let query_path = output_root.join("queries.jsonl");
    let scenarios = generate_scenarios(
        &searchable_root,
        &query_path,
        ScenarioGenerationOptions {
            source_documents: scenario.source_documents,
            queries_per_source: scenario.queries_per_source,
            passage_words: scenario.passage_words,
            seed: scenario.seed,
            maximum_document_bytes: scenario.maximum_document_bytes,
        },
    )?;
    let report_path = output_root.join("showcase.json");
    let report = ShowcasePreparationReport {
        schema_version: SHOWCASE_REPORT_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        provider: provider.to_owned(),
        output_root: output_root.display().to_string(),
        raw: manifest_receipt(raw_root)?,
        searchable: manifest_receipt(&searchable_root)?,
        scenarios,
        report_path: report_path.display().to_string(),
    };
    atomic_write(&report_path, &serde_json::to_vec_pretty(&report)?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Showcase root:         {}", output_root.display());
        println!("Provider:              {}", report.provider);
        println!("Raw documents:         {}", report.raw.documents);
        println!("Searchable documents:  {}", report.searchable.documents);
        println!("Queries:               {}", report.scenarios.queries);
        println!(
            "Multi-positive queries: {}",
            report.scenarios.multi_positive_queries
        );
        println!("Queries JSONL:          {}", report.scenarios.query_file);
        println!("Report:                 {}", report.report_path);
    }
    Ok(())
}

fn manifest_receipt(root: &Path) -> CliResult<ManifestReceipt> {
    let manifest = CorpusManifest::load(root)?;
    let manifest_path = root.join(fo_corpus::MANIFEST_FILENAME);
    let bytes = fs::read(&manifest_path)?;
    Ok(ManifestReceipt {
        root: root.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        manifest_sha256: sha256_hex(&bytes),
        corpus_id: manifest.corpus_id,
        documents: manifest.documents.len(),
        total_bytes: manifest
            .documents
            .iter()
            .map(|document| document.bytes)
            .sum(),
    })
}

fn prepare_output(output: &Path, replace: bool) -> CliResult<()> {
    if output.exists() && replace {
        fs::remove_dir_all(output)?;
    }
    fs::create_dir_all(output)?;
    Ok(())
}
