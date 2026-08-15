#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    CalibratedResult, CalibrationModel, Index, IndexBuilder, IndexConfig, NormalizationProfile,
    PunctuationMode, SearchIntent, SearchOptions, SearchResult, SpectralOptions, normalize,
    spectral_scan,
};
use rayon::prelude::*;
use serde::Serialize;

const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo",
    version,
    about = "FrankenOverlap: sparse-spectral textual overlap and approximate alignment"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build an immutable .foidx corpus index.
    Index(IndexCommand),
    /// Search an index for edited or partially reused text.
    Query(QueryCommand),
    /// Inspect index metadata and corpus statistics.
    Inspect(InspectCommand),
    /// Run dense CountSketch cross-correlation over one corpus text.
    Scan(ScanCommand),
}

#[derive(Debug, Args)]
struct IndexCommand {
    /// A UTF-8 text file or directory tree.
    input: PathBuf,
    /// Destination .foidx file.
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 7)]
    qgram: usize,
    #[arg(long, default_value_t = 12)]
    window: usize,
    #[arg(long, value_enum, default_value = "to-space")]
    punctuation: PunctuationArg,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    nfkc: bool,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    lowercase: bool,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    collapse_whitespace: bool,
    /// Index files regardless of extension.
    #[arg(long)]
    all_files: bool,
    /// Skip files larger than this many bytes.
    #[arg(long, default_value_t = DEFAULT_MAX_FILE_BYTES)]
    max_file_bytes: u64,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct QueryCommand {
    index: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    /// Supply the specimen inline.
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    /// Select passage discovery, source attribution, or near-duplicate ranking.
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: SearchIntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    max_postings: usize,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long, default_value_t = 50_000_000)]
    direct_fallback_work_limit: u64,
    #[arg(long, default_value_t = 8)]
    short_query_candidates: usize,
    /// Minimum hand-designed score retained before optional calibration.
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    /// Optional fitted CalibrationModel JSON used to rerank the retained hits.
    #[arg(long)]
    calibration_model: Option<PathBuf>,
    /// Minimum calibrated probability retained when --calibration-model is used.
    #[arg(long, default_value_t = 0.0)]
    minimum_probability: f64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct InspectCommand {
    index: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ScanCommand {
    corpus: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "to-space")]
    punctuation: PunctuationArg,
    #[arg(long, default_value_t = 4)]
    repetitions: usize,
    #[arg(long, default_value_t = 8)]
    buckets: usize,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 0.25)]
    minimum_score: f32,
    #[arg(long, default_value_t = 16)]
    radius: usize,
    #[arg(long, default_value_t = 250_000_000)]
    direct_work_limit: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PunctuationArg {
    Keep,
    ToSpace,
    Drop,
}

impl From<PunctuationArg> for PunctuationMode {
    fn from(value: PunctuationArg) -> Self {
        match value {
            PunctuationArg::Keep => Self::Keep,
            PunctuationArg::ToSpace => Self::ToSpace,
            PunctuationArg::Drop => Self::Drop,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchIntentArg {
    AnyPassage,
    SourceAttribution,
    NearDuplicate,
}

impl From<SearchIntentArg> for SearchIntent {
    fn from(value: SearchIntentArg) -> Self {
        match value {
            SearchIntentArg::AnyPassage => Self::AnyPassage,
            SearchIntentArg::SourceAttribution => Self::SourceAttribution,
            SearchIntentArg::NearDuplicate => Self::NearDuplicate,
        }
    }
}

#[derive(Debug, Serialize)]
struct IndexReport {
    output: String,
    documents: usize,
    normalized_tokens: usize,
    distinct_fingerprints: usize,
    postings: usize,
    skipped_files: usize,
}

#[derive(Debug, Serialize)]
struct InspectReport<'a> {
    config: &'a IndexConfig,
    stats: fo_core::IndexStats,
    documents: Vec<DocumentReport<'a>>,
}

#[derive(Debug, Serialize)]
struct DocumentReport<'a> {
    id: u32,
    path: &'a str,
    normalized_tokens: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Index(command) => run_index(command),
        Command::Query(command) => run_query(command),
        Command::Inspect(command) => run_inspect(command),
        Command::Scan(command) => run_scan(command),
    }
}

fn run_index(command: IndexCommand) -> CliResult<()> {
    if command.max_file_bytes == 0 {
        return Err(invalid_input("--max-file-bytes must be positive"));
    }
    let config = IndexConfig {
        normalization: NormalizationProfile {
            nfkc: command.nfkc,
            lowercase: command.lowercase,
            collapse_whitespace: command.collapse_whitespace,
            punctuation: command.punctuation.into(),
        },
        qgram_size: command.qgram,
        winnow_window: command.window,
    };
    config.validate()?;

    let mut paths = Vec::new();
    collect_files(&command.input, command.all_files, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err(invalid_input("no eligible files were found"));
    }
    let root = if command.input.is_dir() {
        command.input.as_path()
    } else {
        command.input.parent().unwrap_or_else(|| Path::new("."))
    };

    let loaded = paths
        .par_iter()
        .map(|path| load_document(path, root, command.max_file_bytes, &config.normalization))
        .collect::<Vec<_>>();

    let mut builder = IndexBuilder::new(config)?;
    let mut skipped_files = 0usize;
    for item in loaded {
        match item? {
            Some((path, normalized)) if !normalized.is_empty() => {
                builder.add_normalized_document(path, normalized)?;
            }
            _ => skipped_files += 1,
        }
    }
    let index = builder.build()?;
    if index.documents().is_empty() {
        return Err(invalid_input(
            "all eligible files were empty, binary, oversized, or invalid UTF-8",
        ));
    }
    index.save(&command.output)?;
    let stats = index.stats();
    let report = IndexReport {
        output: command.output.display().to_string(),
        documents: stats.documents,
        normalized_tokens: stats.normalized_tokens,
        distinct_fingerprints: stats.distinct_fingerprints,
        postings: stats.postings,
        skipped_files,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Wrote {}", report.output);
        println!("  documents:            {}", report.documents);
        println!("  normalized tokens:    {}", report.normalized_tokens);
        println!("  distinct fingerprints:{}", report.distinct_fingerprints);
        println!("  postings:             {}", report.postings);
        println!("  skipped files:        {}", report.skipped_files);
    }
    Ok(())
}

fn run_query(command: QueryCommand) -> CliResult<()> {
    if !command.minimum_probability.is_finite()
        || !(0.0..=1.0).contains(&command.minimum_probability)
    {
        return Err(invalid_input("--minimum-probability must lie in [0, 1]"));
    }
    if command.calibration_model.is_none() && command.minimum_probability > 0.0 {
        return Err(invalid_input(
            "--minimum-probability requires --calibration-model",
        ));
    }
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = Index::load(&command.index)?;
    let options = SearchOptions {
        intent: command.intent.into(),
        max_results: command.limit,
        max_candidates: command.candidates,
        max_postings_per_feature: command.max_postings,
        minimum_matched_tokens: command.minimum_matched_tokens,
        minimum_query_coverage: command.minimum_query_coverage,
        minimum_source_coverage: command.minimum_source_coverage,
        direct_fallback_work_limit: command.direct_fallback_work_limit,
        short_query_candidates: command.short_query_candidates,
        minimum_similarity: command.minimum_similarity,
        ..SearchOptions::default()
    };
    let results = index.search(&specimen, &options)?;
    if let Some(path) = command.calibration_model {
        let model = read_calibration_model(&path)?;
        let mut calibrated = model.rerank(&results)?;
        calibrated.retain(|result| result.probability >= command.minimum_probability);
        calibrated.truncate(command.limit);
        if command.json {
            println!("{}", serde_json::to_string_pretty(&calibrated)?);
        } else {
            print_calibrated_results(&calibrated);
        }
    } else if command.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_raw_results(&results);
    }
    Ok(())
}

fn read_calibration_model(path: &Path) -> CliResult<CalibrationModel> {
    let model = serde_json::from_slice::<CalibrationModel>(&fs::read(path)?)?;
    model.validate()?;
    Ok(model)
}

fn print_raw_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("No matches met the search thresholds.");
        return;
    }
    for (rank, result) in results.iter().enumerate() {
        println!(
            "{}. {} [{}..{}] score={:.4} edit={:.4} query={:.4} source={:.4} chain={:.4} tokens={} expected_fp={:.3e}",
            rank + 1,
            result.path,
            result.corpus_start,
            result.corpus_end,
            result.combined_score,
            result.edit_similarity,
            result.query_coverage,
            result.source_coverage,
            result.chain_consistency,
            result.matched_tokens,
            result.estimated_false_matches,
        );
        println!("   {}", one_line(&result.matched_text, 240));
    }
}

fn print_calibrated_results(results: &[CalibratedResult]) {
    if results.is_empty() {
        println!("No matches met the calibrated probability threshold.");
        return;
    }
    for (rank, calibrated) in results.iter().enumerate() {
        let result = &calibrated.result;
        println!(
            "{}. {} [{}..{}] probability={:.4} raw={:.4} edit={:.4} query={:.4} source={:.4} tokens={}",
            rank + 1,
            result.path,
            result.corpus_start,
            result.corpus_end,
            calibrated.probability,
            result.combined_score,
            result.edit_similarity,
            result.query_coverage,
            result.source_coverage,
            result.matched_tokens,
        );
        println!("   {}", one_line(&result.matched_text, 240));
    }
}

fn run_inspect(command: InspectCommand) -> CliResult<()> {
    let index = Index::load(&command.index)?;
    let report = InspectReport {
        config: &index.config,
        stats: index.stats(),
        documents: index
            .documents()
            .iter()
            .map(|document| DocumentReport {
                id: document.id,
                path: &document.path,
                normalized_tokens: document.token_count(),
            })
            .collect(),
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Index: {}", command.index.display());
        println!("Documents: {}", report.stats.documents);
        println!("Normalized tokens: {}", report.stats.normalized_tokens);
        println!("Fingerprints: {}", report.stats.distinct_fingerprints);
        println!("Postings: {}", report.stats.postings);
        println!(
            "q={} w={} normalization={:?}",
            report.config.qgram_size,
            report.config.winnow_window,
            report.config.normalization
        );
        for document in &report.documents {
            println!(
                "  {:>6} {:>10} {}",
                document.id, document.normalized_tokens, document.path
            );
        }
    }
    Ok(())
}

fn run_scan(command: ScanCommand) -> CliResult<()> {
    let corpus = fs::read_to_string(&command.corpus)?;
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let options = SpectralOptions {
        repetitions: command.repetitions,
        buckets: command.buckets,
        max_results: command.limit,
        minimum_score: command.minimum_score,
        local_maximum_radius: command.radius,
        direct_work_limit: command.direct_work_limit,
    };
    let profile = NormalizationProfile {
        punctuation: command.punctuation.into(),
        ..NormalizationProfile::default()
    };
    let peaks = spectral_scan(&corpus, &specimen, &profile, &options)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&peaks)?);
    } else if peaks.is_empty() {
        println!("No dense-correlation peaks met the score threshold.");
    } else {
        for (rank, peak) in peaks.iter().enumerate() {
            println!(
                "{}. [{}..{}] score={:.4} {}",
                rank + 1,
                peak.offset,
                peak.end,
                peak.score,
                one_line(&peak.matched_text, 200)
            );
        }
    }
    Ok(())
}

fn specimen_text(path: Option<&Path>, inline: Option<String>) -> CliResult<String> {
    match (path, inline) {
        (Some(path), None) => Ok(fs::read_to_string(path)?),
        (None, Some(text)) => Ok(text),
        (None, None) => Err(invalid_input("provide a specimen file or --text")),
        (Some(_), Some(_)) => Err(invalid_input(
            "specimen file and --text are mutually exclusive",
        )),
    }
}

fn collect_files(path: &Path, all_files: bool, output: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if all_files || eligible_extension(path) {
            output.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut children = fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        collect_files(&child, all_files, output)?;
    }
    Ok(())
}

fn eligible_extension(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "rst"
            | "tex"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "xml"
            | "html"
            | "htm"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "java"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "toml"
            | "yaml"
            | "yml"
            | "sql"
            | "log"
    )
}

fn load_document(
    path: &Path,
    root: &Path,
    max_file_bytes: u64,
    profile: &NormalizationProfile,
) -> CliResult<Option<(String, fo_core::NormalizedText)>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_file_bytes {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let display_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Some((display_path, normalize(&text, profile))))
}

fn one_line(value: &str, maximum_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_chars {
        return compact;
    }
    let mut output = compact.chars().take(maximum_chars).collect::<String>();
    output.push('…');
    output
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
