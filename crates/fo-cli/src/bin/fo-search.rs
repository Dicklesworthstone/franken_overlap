#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    CompositeSearchOptions, HybridDocumentInput, HybridIndex, HybridIndexBuilder,
    HybridIndexConfig, HybridSearchMode, HybridSearchOptions, LexicalSearchOptions,
    SearchIntent, SearchOptions,
};
use fo_corpus::{CorpusManifest, MANIFEST_FILENAME};
use rayon::prelude::*;
use serde::Serialize;

const DEFAULT_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-search",
    version,
    about = "Unified lexical, overlap, composite, and hybrid search"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Build(BuildCommand),
    Query(QueryCommand),
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct BuildCommand {
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    input_format: InputFormatArg,
    #[arg(long)]
    all_files: bool,
    #[arg(long, default_value_t = DEFAULT_MAX_FILE_BYTES)]
    max_file_bytes: u64,
    #[arg(long, default_value_t = 7)]
    qgram: usize,
    #[arg(long, default_value_t = 12)]
    winnow_window: usize,
    #[arg(long, default_value_t = 2.5)]
    title_weight: f32,
    #[arg(long, default_value_t = 1.0)]
    body_weight: f32,
    #[arg(long, default_value_t = 3.0)]
    tags_weight: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct QueryCommand {
    index: PathBuf,
    query: Option<String>,
    #[arg(long, conflicts_with = "query")]
    query_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "auto")]
    mode: SearchModeArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 8)]
    candidate_multiplier: usize,
    #[arg(long, default_value_t = 1.0)]
    lexical_weight: f32,
    #[arg(long, default_value_t = 1.0)]
    overlap_weight: f32,
    #[arg(long, default_value_t = 60.0)]
    rrf_k: f32,
    #[arg(long, default_value_t = 12)]
    auto_lexical_max_words: usize,
    #[arg(long, default_value_t = 48)]
    auto_overlap_min_words: usize,
    #[arg(long, default_value_t = 180)]
    auto_composite_min_words: usize,
    #[arg(long, default_value_t = 0.0)]
    minimum_final_score: f32,
    #[arg(long, default_value_t = 0.0)]
    lexical_minimum_score: f32,
    #[arg(long, default_value_t = 0.10)]
    overlap_minimum_score: f32,
    #[arg(long, default_value_t = 2.0)]
    phrase_boost: f32,
    #[arg(long, default_value_t = 1.25)]
    proximity_boost: f32,
    #[arg(long, default_value_t = 0.75)]
    coverage_boost: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long, value_parser = parse_key_value)]
    metadata: Vec<(String, String)>,
    #[arg(long = "require-tag")]
    required_tags: Vec<String>,
    #[arg(long = "exclude-tag")]
    excluded_tags: Vec<String>,
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
enum InputFormatArg {
    Auto,
    Directory,
    Corpus,
    Jsonl,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchModeArg {
    Auto,
    Lexical,
    Overlap,
    Composite,
    Hybrid,
}

impl From<SearchModeArg> for HybridSearchMode {
    fn from(value: SearchModeArg) -> Self {
        match value {
            SearchModeArg::Auto => Self::Auto,
            SearchModeArg::Lexical => Self::Lexical,
            SearchModeArg::Overlap => Self::Overlap,
            SearchModeArg::Composite => Self::Composite,
            SearchModeArg::Hybrid => Self::Hybrid,
        }
    }
}

#[derive(Debug, Serialize)]
struct BuildReport {
    output: String,
    documents: usize,
    overlap_tokens: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
    skipped: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-search: {error}");
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
    let format = resolve_input_format(&command.input, command.input_format)?;
    let (documents, skipped) = match format {
        InputFormatArg::Directory => {
            load_directory(&command.input, command.all_files, command.max_file_bytes)?
        }
        InputFormatArg::Corpus => load_corpus(&command.input, command.max_file_bytes)?,
        InputFormatArg::Jsonl => (load_jsonl(&command.input)?, 0),
        InputFormatArg::Auto => unreachable!("auto input format must be resolved"),
    };
    if documents.is_empty() {
        return Err(invalid_input("no hybrid documents were loaded"));
    }
    let mut config = HybridIndexConfig::default();
    config.overlap.qgram_size = command.qgram;
    config.overlap.winnow_window = command.winnow_window;
    config.lexical.title_weight = command.title_weight;
    config.lexical.body_weight = command.body_weight;
    config.lexical.tags_weight = command.tags_weight;
    config.validate()?;
    let mut builder = HybridIndexBuilder::new(config)?;
    for document in documents {
        builder.add_document(document)?;
    }
    let index = builder.build()?;
    index.save(&command.output)?;
    let stats = index.stats();
    let report = BuildReport {
        output: command.output.display().to_string(),
        documents: stats.documents,
        overlap_tokens: stats.overlap.normalized_tokens,
        overlap_postings: stats.overlap.postings,
        lexical_terms: stats.lexical.distinct_terms,
        lexical_postings: stats.lexical.postings,
        skipped,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Wrote {}", report.output);
        println!("  documents:         {}", report.documents);
        println!("  overlap tokens:    {}", report.overlap_tokens);
        println!("  overlap postings:  {}", report.overlap_postings);
        println!("  lexical terms:     {}", report.lexical_terms);
        println!("  lexical postings:  {}", report.lexical_postings);
        println!("  skipped:           {}", report.skipped);
    }
    Ok(())
}

fn run_query(command: QueryCommand) -> CliResult<()> {
    let query = match (command.query, command.query_file) {
        (Some(query), None) => query,
        (None, Some(path)) => fs::read_to_string(path)?,
        (None, None) => return Err(invalid_input("provide QUERY or --query-file")),
        (Some(_), Some(_)) => {
            return Err(invalid_input(
                "QUERY and --query-file are mutually exclusive",
            ));
        }
    };
    let required_metadata = command.metadata.into_iter().collect::<BTreeMap<_, _>>();
    let index = HybridIndex::load(&command.index)?;
    let report = index.search_text(
        query.trim(),
        &HybridSearchOptions {
            mode: command.mode.into(),
            max_results: command.limit,
            candidate_multiplier: command.candidate_multiplier,
            lexical_weight: command.lexical_weight,
            overlap_weight: command.overlap_weight,
            reciprocal_rank_k: command.rrf_k,
            auto_lexical_max_words: command.auto_lexical_max_words,
            auto_overlap_min_words: command.auto_overlap_min_words,
            auto_composite_min_words: command.auto_composite_min_words,
            final_minimum_score: command.minimum_final_score,
            required_metadata,
            required_tags: command.required_tags,
            excluded_tags: command.excluded_tags,
            lexical: LexicalSearchOptions {
                minimum_score: command.lexical_minimum_score,
                phrase_boost: command.phrase_boost,
                proximity_boost: command.proximity_boost,
                coverage_boost: command.coverage_boost,
                ..LexicalSearchOptions::default()
            },
            overlap: SearchOptions {
                intent: SearchIntent::SourceAttribution,
                minimum_similarity: command.overlap_minimum_score,
                minimum_matched_tokens: command.minimum_matched_tokens,
                minimum_query_coverage: command.minimum_query_coverage,
                minimum_source_coverage: command.minimum_source_coverage,
                ..SearchOptions::default()
            },
            composite: CompositeSearchOptions {
                minimum_block_tokens: command.minimum_matched_tokens.min(20).max(1),
                minimum_aggregate_score: command.overlap_minimum_score,
                ..CompositeSearchOptions::default()
            },
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Route: {:?} words={} unique={:.3} paragraphs={} explicit_syntax={}",
            report.analysis.route,
            report.analysis.word_count,
            report.analysis.unique_word_fraction,
            report.analysis.paragraph_count,
            report.analysis.explicit_lexical_syntax,
        );
        if report.results.is_empty() {
            println!("No result met the requested constraints.");
        }
        for (rank, result) in report.results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} route={:?} lexical={:.4} overlap={:.4} cross_lane={}",
                rank + 1,
                result.title,
                result.score,
                result.route,
                result.explanation.lexical_normalized_score,
                result.explanation.overlap_score,
                result.explanation.cross_lane_support,
            );
            println!("   id={}", result.external_id);
            println!("   {}", result.snippet);
        }
    }
    Ok(())
}

fn run_inspect(command: InspectCommand) -> CliResult<()> {
    let index = HybridIndex::load(&command.index)?;
    let stats = index.stats();
    if command.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Documents:            {}", stats.documents);
        println!("Overlap tokens:       {}", stats.overlap.normalized_tokens);
        println!("Overlap fingerprints: {}", stats.overlap.distinct_fingerprints);
        println!("Overlap postings:     {}", stats.overlap.postings);
        println!("Lexical terms:        {}", stats.lexical.distinct_terms);
        println!("Lexical postings:     {}", stats.lexical.postings);
    }
    Ok(())
}

fn resolve_input_format(path: &Path, requested: InputFormatArg) -> CliResult<InputFormatArg> {
    if !matches!(requested, InputFormatArg::Auto) {
        return Ok(requested);
    }
    if path.is_dir() && path.join(MANIFEST_FILENAME).is_file() {
        Ok(InputFormatArg::Corpus)
    } else if path.is_file() && is_jsonl(path) {
        Ok(InputFormatArg::Jsonl)
    } else if path.is_dir() || path.is_file() {
        Ok(InputFormatArg::Directory)
    } else {
        Err(invalid_input(format!(
            "could not infer input format for {}",
            path.display()
        )))
    }
}

fn load_corpus(root: &Path, maximum_bytes: u64) -> CliResult<(Vec<HybridDocumentInput>, usize)> {
    let manifest = CorpusManifest::load(root)?;
    let loaded = manifest
        .documents
        .par_iter()
        .map(|document| {
            validate_relative_path(&document.relative_path)?;
            if document.bytes > maximum_bytes {
                return Ok(None);
            }
            let body = fs::read_to_string(root.join(&document.relative_path))?;
            let mut metadata = document.metadata.clone();
            metadata.insert("source_url".to_owned(), document.source_url.clone());
            metadata.insert("sha256".to_owned(), document.sha256.clone());
            if let Some(date) = &document.published_or_filed {
                metadata.insert("date".to_owned(), date.clone());
            }
            let mut tags = Vec::new();
            if let Some(language) = &document.language {
                tags.push(language.clone());
            }
            for key in ["form", "tickers", "subjects"] {
                if let Some(value) = document.metadata.get(key) {
                    tags.extend(
                        value
                            .split([',', ';', '|'])
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned),
                    );
                }
            }
            tags.sort_unstable();
            tags.dedup();
            Ok(Some(HybridDocumentInput {
                external_id: document.id.clone(),
                title: document.title.clone(),
                body,
                tags,
                metadata,
            }))
        })
        .collect::<Vec<CliResult<Option<HybridDocumentInput>>>>();
    collect_loaded(loaded)
}

fn load_directory(
    input: &Path,
    all_files: bool,
    maximum_bytes: u64,
) -> CliResult<(Vec<HybridDocumentInput>, usize)> {
    let mut paths = Vec::new();
    collect_files(input, all_files, &mut paths)?;
    paths.sort();
    let root = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or_else(|| Path::new("."))
    };
    let loaded = paths
        .par_iter()
        .map(|path| load_directory_document(path, root, maximum_bytes))
        .collect::<Vec<_>>();
    collect_loaded(loaded)
}

fn load_directory_document(
    path: &Path,
    root: &Path,
    maximum_bytes: u64,
) -> CliResult<Option<HybridDocumentInput>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > maximum_bytes {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    let Ok(body) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let title = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && line.chars().count() <= 240)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&relative)
                .to_owned()
        });
    let mut tags = Vec::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        tags.push(extension.to_ascii_lowercase());
    }
    Ok(Some(HybridDocumentInput {
        external_id: relative.clone(),
        title,
        body,
        tags,
        metadata: BTreeMap::from([("path".to_owned(), relative)]),
    }))
}

fn load_jsonl(path: &Path) -> CliResult<Vec<HybridDocumentInput>> {
    let file = File::open(path)?;
    let mut documents = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        documents.push(
            serde_json::from_str::<HybridDocumentInput>(value).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}:{}: {error}", path.display(), line_index + 1),
                )
            })?,
        );
    }
    Ok(documents)
}

fn collect_loaded(
    loaded: Vec<CliResult<Option<HybridDocumentInput>>>,
) -> CliResult<(Vec<HybridDocumentInput>, usize)> {
    let mut documents = Vec::new();
    let mut skipped = 0usize;
    for item in loaded {
        match item? {
            Some(document) => documents.push(document),
            None => skipped += 1,
        }
    }
    Ok((documents, skipped))
}

fn collect_files(path: &Path, all_files: bool, output: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if all_files || eligible_extension(path) {
            output.push(path.to_owned());
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
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
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
        })
}

fn parse_key_value(value: &str) -> Result<(String, String), String> {
    let Some((key, value)) = value.split_once('=') else {
        return Err("metadata filters must use KEY=VALUE".to_owned());
    };
    if key.trim().is_empty() {
        return Err("metadata filter key must not be empty".to_owned());
    }
    Ok((key.trim().to_owned(), value.to_owned()))
}

fn validate_relative_path(path: &str) -> CliResult<()> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(invalid_input("corpus manifest contains an unsafe path"));
    }
    Ok(())
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("jsonl") || extension.eq_ignore_ascii_case("ndjson")
        })
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
