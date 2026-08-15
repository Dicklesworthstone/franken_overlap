#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    MultiViewConfig, MultiViewIndex, MultiViewIndexBuilder, MultiViewSearchResult, SearchIntent,
    SearchOptions,
};
use rayon::prelude::*;
use serde::Serialize;

const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-multiview",
    version,
    about = "Build and query consensus indexes across several q-gram scales"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build a persisted multi-view index directory.
    Build(BuildCommand),
    /// Query a persisted multi-view index.
    Query(QueryCommand),
    /// Inspect its view configurations and sizes.
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct BuildCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "balanced")]
    preset: PresetArg,
    #[arg(long)]
    all_files: bool,
    #[arg(long, default_value_t = DEFAULT_MAX_FILE_BYTES)]
    max_file_bytes: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct QueryCommand {
    index: PathBuf,
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 400)]
    candidates: usize,
    #[arg(long, default_value_t = 0.30)]
    minimum_score: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct InspectCommand {
    index: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PresetArg {
    Balanced,
    HighRecall,
    HighPrecision,
}

impl PresetArg {
    fn config(self) -> MultiViewConfig {
        match self {
            Self::Balanced => MultiViewConfig::balanced(),
            Self::HighRecall => MultiViewConfig::high_recall(),
            Self::HighPrecision => MultiViewConfig::high_precision(),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum IntentArg {
    AnyPassage,
    SourceAttribution,
    NearDuplicate,
}

impl From<IntentArg> for SearchIntent {
    fn from(value: IntentArg) -> Self {
        match value {
            IntentArg::AnyPassage => Self::AnyPassage,
            IntentArg::SourceAttribution => Self::SourceAttribution,
            IntentArg::NearDuplicate => Self::NearDuplicate,
        }
    }
}

#[derive(Debug, Serialize)]
struct BuildReport {
    output: String,
    documents: usize,
    views: usize,
    total_postings: usize,
    skipped_files: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-multiview: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Build(command) => run_build(command),
        Command::Query(command) => run_query(command),
        Command::Inspect(command) => run_inspect(command),
    }
}

fn run_build(command: BuildCommand) -> CliResult<()> {
    if command.max_file_bytes == 0 {
        return Err(invalid_input("--max-file-bytes must be positive"));
    }
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
        .map(|path| load_document(path, root, command.max_file_bytes))
        .collect::<Vec<_>>();

    let mut builder = MultiViewIndexBuilder::new(command.preset.config())?;
    let mut skipped_files = 0usize;
    for document in loaded {
        match document? {
            Some((path, contents)) if !contents.trim().is_empty() => {
                builder.add_document(path, contents)?;
            }
            _ => skipped_files += 1,
        }
    }
    let index = builder.build()?;
    let stats = index.stats();
    if stats.documents == 0 {
        return Err(invalid_input(
            "all eligible files were empty, binary, oversized, or invalid UTF-8",
        ));
    }
    index.save(&command.output)?;
    let report = BuildReport {
        output: command.output.display().to_string(),
        documents: stats.documents,
        views: stats.views,
        total_postings: stats.total_postings,
        skipped_files,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Wrote {}", report.output);
        println!("  documents:      {}", report.documents);
        println!("  views:          {}", report.views);
        println!("  total postings: {}", report.total_postings);
        println!("  skipped files:  {}", report.skipped_files);
    }
    Ok(())
}

fn run_query(command: QueryCommand) -> CliResult<()> {
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = MultiViewIndex::load(&command.index)?;
    let results = index.search(
        &specimen,
        &SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            minimum_similarity: command.minimum_score,
            minimum_matched_tokens: command.minimum_matched_tokens,
            minimum_query_coverage: command.minimum_query_coverage,
            minimum_source_coverage: command.minimum_source_coverage,
            ..SearchOptions::default()
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_results(&results);
    }
    Ok(())
}

fn run_inspect(command: InspectCommand) -> CliResult<()> {
    let index = MultiViewIndex::load(&command.index)?;
    let stats = index.stats();
    if command.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Views: {}", stats.views);
        println!("Documents: {}", stats.documents);
        println!("Total postings: {}", stats.total_postings);
        for view in stats.view_stats {
            println!(
                "  {} q={} w={} weight={:.2} fingerprints={} postings={}",
                view.name,
                view.config.qgram_size,
                view.config.winnow_window,
                view.weight,
                view.stats.distinct_fingerprints,
                view.stats.postings,
            );
        }
    }
    Ok(())
}

fn print_results(results: &[MultiViewSearchResult]) {
    if results.is_empty() {
        println!("No consensus match met the requested thresholds.");
        return;
    }
    for (rank, result) in results.iter().enumerate() {
        let representative = &result.representative;
        println!(
            "{}. {} [{}..{}] fused={:.4} support={}/{} disagreement={:.4} edit={:.4} query={:.4} source={:.4}",
            rank + 1,
            representative.path,
            representative.corpus_start,
            representative.corpus_end,
            result.fused_score,
            result.view_support,
            result.evidence.len().max(result.view_support),
            result.score_disagreement,
            result.weighted_edit_similarity,
            result.weighted_query_coverage,
            result.weighted_source_coverage,
        );
        let views = result
            .evidence
            .iter()
            .map(|evidence| format!("{}:{:.3}", evidence.view_name, evidence.raw_score))
            .collect::<Vec<_>>()
            .join(", ");
        println!("   views: {views}");
        println!("   {}", one_line(&representative.matched_text, 220));
    }
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
) -> CliResult<Option<(String, String)>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_file_bytes {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    let Ok(contents) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let display_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Some((display_path, contents)))
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
