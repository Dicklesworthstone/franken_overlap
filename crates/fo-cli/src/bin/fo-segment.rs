#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{IndexConfig, SearchIntent, SearchOptions, SegmentDocumentInput, SegmentedIndex};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-segment",
    version,
    about = "Append, delete, search, verify, and compact FrankenOverlap index segments"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Create(CreateCommand),
    Append(AppendCommand),
    Delete(DeleteCommand),
    Search(SearchCommand),
    Compact(IndexCommand),
    Inspect(IndexCommand),
    Verify(IndexCommand),
}

#[derive(Debug, Args)]
struct CreateCommand {
    index: PathBuf,
    #[arg(long, default_value_t = 7)]
    qgram: usize,
    #[arg(long, default_value_t = 12)]
    window: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AppendCommand {
    index: PathBuf,
    /// JSONL records containing {"path":"...","contents":"..."}.
    documents: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DeleteCommand {
    index: PathBuf,
    /// Active document paths to tombstone.
    paths: Vec<String>,
    /// Optional newline-delimited path file.
    #[arg(long)]
    paths_file: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SearchCommand {
    index: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    max_postings: usize,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct IndexCommand {
    index: PathBuf,
    #[arg(long)]
    json: bool,
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

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-segment: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Create(command) => run_create(command),
        Command::Append(command) => run_append(command),
        Command::Delete(command) => run_delete(command),
        Command::Search(command) => run_search(command),
        Command::Compact(command) => run_compact(command),
        Command::Inspect(command) => run_inspect(command),
        Command::Verify(command) => run_verify(command),
    }
}

fn run_create(command: CreateCommand) -> CliResult<()> {
    let index = SegmentedIndex::create(
        &command.index,
        IndexConfig {
            qgram_size: command.qgram,
            winnow_window: command.window,
            ..IndexConfig::default()
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&index.stats())?);
    } else {
        println!("Created {}", command.index.display());
        print_stats(&index.stats());
    }
    Ok(())
}

fn run_append(command: AppendCommand) -> CliResult<()> {
    let documents = read_documents(&command.documents)?;
    let mut index = SegmentedIndex::open(&command.index)?;
    let report = index.append_documents(documents)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Appended segment {}", report.segment_id);
        println!("  generation:     {}", report.generation);
        println!("  documents:      {}", report.added_documents);
        println!(
            "  global ids:     {}..={}",
            report.first_global_document_id, report.last_global_document_id
        );
        println!("  postings:       {}", report.stats.postings);
    }
    Ok(())
}

fn run_delete(command: DeleteCommand) -> CliResult<()> {
    let mut paths = command.paths;
    if let Some(path) = command.paths_file {
        paths.extend(read_paths(&path)?);
    }
    if paths.is_empty() {
        return Err(invalid_input(
            "provide at least one path or a nonempty --paths-file",
        ));
    }
    let mut index = SegmentedIndex::open(&command.index)?;
    let report = index.delete_paths(&paths)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Generation: {}", report.generation);
        println!("Deleted:    {}", report.deleted_document_ids.len());
        println!("Missing:    {}", report.missing_paths.len());
        for path in report.missing_paths {
            println!("  missing {path}");
        }
    }
    Ok(())
}

fn run_search(command: SearchCommand) -> CliResult<()> {
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = SegmentedIndex::open(&command.index)?;
    let results = index.search(
        &specimen,
        &SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            max_postings_per_feature: command.max_postings,
            minimum_matched_tokens: command.minimum_matched_tokens,
            minimum_similarity: command.minimum_similarity,
            ..SearchOptions::default()
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("No active segmented-index match met the thresholds.");
    } else {
        for (rank, result) in results.iter().enumerate() {
            println!(
                "{}. global={} segment={} {} [{}..{}] fused={:.4} raw={:.4} edit={:.4} query={:.4}",
                rank + 1,
                result.global_document_id,
                result.segment_id,
                result.result.path,
                result.result.corpus_start,
                result.result.corpus_end,
                result.fused_score,
                result.result.combined_score,
                result.result.edit_similarity,
                result.result.query_coverage,
            );
        }
    }
    Ok(())
}

fn run_compact(command: IndexCommand) -> CliResult<()> {
    let mut index = SegmentedIndex::open(&command.index)?;
    let report = index.compact()?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Generation:       {}", report.generation);
        println!("Active documents: {}", report.active_documents);
        println!("Deleted records:  {}", report.deleted_documents);
        println!(
            "Segments:          {} -> {}",
            report.old_segments, report.new_segments
        );
        for failure in report.cleanup_failures {
            println!("Cleanup warning: {failure}");
        }
    }
    Ok(())
}

fn run_inspect(command: IndexCommand) -> CliResult<()> {
    let index = SegmentedIndex::open(&command.index)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(index.manifest())?);
    } else {
        print_stats(&index.stats());
        for segment in &index.manifest().segments {
            println!(
                "  segment={} file={} documents={} tokens={} postings={} bytes={}",
                segment.id,
                segment.filename,
                segment.stats.documents,
                segment.stats.normalized_tokens,
                segment.stats.postings,
                segment.file_bytes,
            );
        }
    }
    Ok(())
}

fn run_verify(command: IndexCommand) -> CliResult<()> {
    let index = SegmentedIndex::open(&command.index)?;
    let report = index.verify_storage()?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Generation:        {}", report.generation);
        println!("Verified segments: {}", report.verified_segments);
        println!("Active documents:  {}", report.verified_active_documents);
        println!("Segment bytes:     {}", report.file_bytes);
    }
    Ok(())
}

fn read_documents(path: &Path) -> CliResult<Vec<SegmentDocumentInput>> {
    let file = File::open(path)?;
    let mut documents = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let document = serde_json::from_str::<SegmentDocumentInput>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
        })?;
        documents.push(document);
    }
    if documents.is_empty() {
        return Err(invalid_input(format!(
            "{} contains no documents",
            path.display()
        )));
    }
    Ok(documents)
}

fn read_paths(path: &Path) -> CliResult<Vec<String>> {
    let input = fs::read_to_string(path)?;
    Ok(input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
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

fn print_stats(stats: &fo_core::SegmentedIndexStats) {
    println!("Generation:      {}", stats.generation);
    println!("Segments:        {}", stats.segments);
    println!("Active docs:     {}", stats.active_documents);
    println!("Deleted docs:    {}", stats.deleted_documents);
    println!("Physical docs:   {}", stats.physical_documents);
    println!("Physical tokens: {}", stats.physical_normalized_tokens);
    println!("Physical posts:  {}", stats.physical_postings);
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
