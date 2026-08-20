#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    LexicalDocumentInput, LexicalIndex, LexicalIndexBuilder, LexicalIndexConfig,
    LexicalSearchOptions,
};
use fo_corpus::{CorpusManifest, MANIFEST_FILENAME};
use rayon::prelude::*;
use serde::Serialize;

const DEFAULT_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-lexical",
    version,
    about = "Fielded BM25, phrase, and proximity search for FrankenOverlap corpora"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build a fielded positional lexical index.
    Build(BuildCommand),
    /// Query a lexical index.
    Query(QueryCommand),
    /// Inspect lexical index statistics.
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct BuildCommand {
    /// Directory tree, fo-corpus root, UTF-8 file, or LexicalDocumentInput JSONL.
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    input_format: InputFormatArg,
    #[arg(long)]
    all_files: bool,
    #[arg(long, default_value_t = DEFAULT_MAX_FILE_BYTES)]
    max_file_bytes: u64,
    #[arg(long, default_value_t = 1.2)]
    k1: f32,
    #[arg(long, default_value_t = 0.75)]
    length_normalization: f32,
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
    /// Query syntax supports +required, -excluded, quoted phrases, and title:/body:/tag:.
    query: Option<String>,
    #[arg(long, conflicts_with = "query")]
    query_file: Option<PathBuf>,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 50_000)]
    candidates: usize,
    #[arg(long, default_value_t = 8)]
    candidate_terms: usize,
    #[arg(long, default_value_t = 1_000_000)]
    maximum_postings_per_term: usize,
    #[arg(long, default_value_t = 0.0)]
    minimum_should_match: f32,
    #[arg(long, default_value_t = 2.0)]
    phrase_boost: f32,
    #[arg(long, default_value_t = 1.25)]
    proximity_boost: f32,
    #[arg(long, default_value_t = 0.75)]
    coverage_boost: f32,
    #[arg(long, default_value_t = 64)]
    proximity_window: usize,
    #[arg(long, default_value_t = 48)]
    snippet_words: usize,
    #[arg(long)]
    require_phrases: bool,
    #[arg(long, default_value_t = 0.0)]
    minimum_score: f32,
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

#[derive(Debug, Serialize)]
struct BuildReport {
    output: String,
    documents: usize,
    distinct_terms: usize,
    postings: usize,
    title_tokens: u64,
    body_tokens: u64,
    tag_tokens: u64,
    source_format: String,
    skipped: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-lexical: {error}");
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
    let config = LexicalIndexConfig {
        k1: command.k1,
        length_normalization: command.length_normalization,
        title_weight: command.title_weight,
        body_weight: command.body_weight,
        tags_weight: command.tags_weight,
    };
    config.validate()?;
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
        return Err(invalid_input("no lexical documents were loaded"));
    }
    let mut builder = LexicalIndexBuilder::new(config)?;
    for document in documents {
        builder.add_document(document)?;
    }
    let index = builder.build()?;
    index.save(&command.output)?;
    let stats = index.stats();
    let report = BuildReport {
        output: command.output.display().to_string(),
        documents: stats.documents,
        distinct_terms: stats.distinct_terms,
        postings: stats.postings,
        title_tokens: stats.title_tokens,
        body_tokens: stats.body_tokens,
        tag_tokens: stats.tag_tokens,
        source_format: format_name(format).to_owned(),
        skipped,
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Wrote {}", report.output);
        println!("  source format:  {}", report.source_format);
        println!("  documents:      {}", report.documents);
        println!("  distinct terms: {}", report.distinct_terms);
        println!("  postings:       {}", report.postings);
        println!("  title tokens:   {}", report.title_tokens);
        println!("  body tokens:    {}", report.body_tokens);
        println!("  tag tokens:     {}", report.tag_tokens);
        println!("  skipped:        {}", report.skipped);
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
    let index = LexicalIndex::load(&command.index)?;
    let results = index.search_text(
        query.trim(),
        &LexicalSearchOptions {
            max_results: command.limit,
            max_candidate_documents: command.candidates,
            candidate_term_limit: command.candidate_terms,
            maximum_postings_per_term: command.maximum_postings_per_term,
            minimum_should_match: command.minimum_should_match,
            phrase_boost: command.phrase_boost,
            proximity_boost: command.proximity_boost,
            coverage_boost: command.coverage_boost,
            proximity_window: command.proximity_window,
            snippet_words: command.snippet_words,
            require_phrases: command.require_phrases,
            minimum_score: command.minimum_score,
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("No lexical results met the requested constraints.");
    } else {
        for (rank, result) in results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} fields={:?} terms={} phrase={} proximity={:.4}",
                rank + 1,
                result.title,
                result.score,
                result.matched_fields,
                result.matched_terms.join(","),
                result.explanation.exact_phrase_matches,
                result.explanation.proximity_boost,
            );
            println!("   id={}", result.external_id);
            println!("   {}", result.snippet);
            println!(
                "   bm25(title={:.4}, body={:.4}, tags={:.4}) coverage={:.4}",
                result.explanation.title_bm25,
                result.explanation.body_bm25,
                result.explanation.tags_bm25,
                result.explanation.coverage_boost,
            );
        }
    }
    Ok(())
}

fn run_inspect(command: InspectCommand) -> CliResult<()> {
    let index = LexicalIndex::load(&command.index)?;
    let stats = index.stats();
    if command.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Documents:       {}", stats.documents);
        println!("Distinct terms:  {}", stats.distinct_terms);
        println!("Postings:        {}", stats.postings);
        println!("Title tokens:    {}", stats.title_tokens);
        println!("Body tokens:     {}", stats.body_tokens);
        println!("Tag tokens:      {}", stats.tag_tokens);
        println!("Average title:   {:.2}", stats.average_title_length);
        println!("Average body:    {:.2}", stats.average_body_length);
        println!("Average tags:    {:.2}", stats.average_tags_length);
    }
    Ok(())
}

fn resolve_input_format(path: &Path, requested: InputFormatArg) -> CliResult<InputFormatArg> {
    if !matches!(requested, InputFormatArg::Auto) {
        return Ok(requested);
    }
    if path.is_dir() && path.join(MANIFEST_FILENAME).is_file() {
        return Ok(InputFormatArg::Corpus);
    }
    if path.is_dir() || path.is_file() && !is_jsonl(path) {
        return Ok(InputFormatArg::Directory);
    }
    if path.is_file() && is_jsonl(path) {
        return Ok(InputFormatArg::Jsonl);
    }
    Err(invalid_input(format!(
        "could not infer input format for {}",
        path.display()
    )))
}

fn load_corpus(root: &Path, maximum_bytes: u64) -> CliResult<(Vec<LexicalDocumentInput>, usize)> {
    let manifest = CorpusManifest::load(root)?;
    let loaded = manifest
        .documents
        .par_iter()
        .map(|document| {
            validate_relative_path(&document.relative_path)?;
            let path = root.join(&document.relative_path);
            if document.bytes > maximum_bytes {
                return Ok(None);
            }
            let body = fs::read_to_string(&path)?;
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
            tags.extend(provider_tags(&manifest, document));
            Ok(Some(LexicalDocumentInput {
                external_id: document.id.clone(),
                title: document.title.clone(),
                body,
                tags,
                metadata,
            }))
        })
        .collect::<Vec<CliResult<Option<LexicalDocumentInput>>>>();
    collect_loaded(loaded)
}

fn provider_tags(manifest: &CorpusManifest, document: &fo_corpus::CorpusDocument) -> Vec<String> {
    let mut tags = vec![format!("{:?}", manifest.provider).to_ascii_lowercase()];
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
    tags
}

fn load_directory(
    input: &Path,
    all_files: bool,
    maximum_bytes: u64,
) -> CliResult<(Vec<LexicalDocumentInput>, usize)> {
    let mut paths = Vec::new();
    collect_files(input, all_files, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err(invalid_input("no eligible UTF-8 files were found"));
    }
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
) -> CliResult<Option<LexicalDocumentInput>> {
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
        .find(|line| !line.is_empty())
        .filter(|line| line.chars().count() <= 240)
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
    for component in Path::new(&relative).components() {
        if let Component::Normal(value) = component
            && let Some(value) = value.to_str()
            && value
                != path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
        {
            tags.push(value.to_owned());
        }
    }
    tags.sort_unstable();
    tags.dedup();
    Ok(Some(LexicalDocumentInput {
        external_id: relative.clone(),
        title,
        body,
        tags,
        metadata: BTreeMap::from([("path".to_owned(), relative)]),
    }))
}

fn load_jsonl(path: &Path) -> CliResult<Vec<LexicalDocumentInput>> {
    let file = File::open(path)?;
    let mut documents = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        documents.push(
            serde_json::from_str::<LexicalDocumentInput>(value).map_err(|error| {
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
    loaded: Vec<CliResult<Option<LexicalDocumentInput>>>,
) -> CliResult<(Vec<LexicalDocumentInput>, usize)> {
    let mut documents = Vec::new();
    let mut skipped = 0usize;
    for result in loaded {
        match result? {
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

const fn format_name(format: InputFormatArg) -> &'static str {
    match format {
        InputFormatArg::Auto => "auto",
        InputFormatArg::Directory => "directory",
        InputFormatArg::Corpus => "corpus",
        InputFormatArg::Jsonl => "jsonl",
    }
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
