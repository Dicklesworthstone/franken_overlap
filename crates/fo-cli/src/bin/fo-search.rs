#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    HybridDocumentInput, HybridFilter, HybridIndex, HybridIndexBuilder, HybridIndexConfig,
    HybridQueryMode, HybridSearchOptions, IndexConfig, LexicalIndexConfig, LexicalSearchOptions,
    NormalizationProfile, SearchOptions,
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
    about = "Unified lexical, phrase, proximity, and edited-overlap search"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build a persisted hybrid index directory.
    Build(BuildCommand),
    /// Query with automatic or explicit lexical/overlap routing.
    Query(QueryCommand),
    /// Inspect hybrid component statistics and configuration.
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct BuildCommand {
    /// Directory tree, fo-corpus root, UTF-8 file, or HybridDocumentInput JSONL.
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
    window: usize,
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
    /// Query text. Omit when using --query-file.
    query: Option<String>,
    #[arg(long, conflicts_with = "query")]
    query_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "auto")]
    mode: HybridModeArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 8)]
    candidate_multiplier: usize,
    #[arg(long, default_value_t = 0.40)]
    lexical_weight: f32,
    #[arg(long, default_value_t = 0.45)]
    overlap_weight: f32,
    #[arg(long, default_value_t = 0.15)]
    rrf_weight: f32,
    #[arg(long, default_value_t = 60.0)]
    rrf_constant: f32,
    #[arg(long, default_value_t = 4.0)]
    lexical_saturation: f32,
    #[arg(long, default_value_t = 0.12)]
    agreement_bonus: f32,
    #[arg(long, default_value_t = 0.08)]
    phrase_bonus: f32,
    #[arg(long, default_value_t = 0.0)]
    minimum_score: f32,
    #[arg(long, default_value_t = 0.10)]
    overlap_candidate_floor: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long, default_value_t = 50_000)]
    maximum_postings_per_feature: usize,
    #[arg(long, default_value_t = 50_000)]
    lexical_candidates: usize,
    #[arg(long, default_value_t = 8)]
    lexical_candidate_terms: usize,
    #[arg(long, default_value_t = 1_000_000)]
    maximum_postings_per_term: usize,
    #[arg(long, default_value_t = 0.0)]
    minimum_should_match: f32,
    #[arg(long)]
    require_phrases: bool,
    #[arg(long)]
    external_id_prefix: Option<String>,
    #[arg(long = "require-tag")]
    required_tags: Vec<String>,
    /// Exact metadata filter expressed as KEY=VALUE.
    #[arg(long = "metadata")]
    metadata_filters: Vec<String>,
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
enum HybridModeArg {
    Auto,
    Hybrid,
    Overlap,
    Lexical,
}

impl From<HybridModeArg> for HybridQueryMode {
    fn from(value: HybridModeArg) -> Self {
        match value {
            HybridModeArg::Auto => Self::Auto,
            HybridModeArg::Hybrid => Self::Hybrid,
            HybridModeArg::Overlap => Self::Overlap,
            HybridModeArg::Lexical => Self::Lexical,
        }
    }
}

#[derive(Debug, Serialize)]
struct BuildReport {
    output: String,
    source_format: String,
    skipped: usize,
    stats: fo_core::HybridIndexStats,
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
        InputFormatArg::Auto => unreachable!("auto format must be resolved"),
    };
    if documents.is_empty() {
        return Err(invalid_input("no hybrid documents were loaded"));
    }
    let config = HybridIndexConfig {
        overlap: IndexConfig {
            normalization: NormalizationProfile::default(),
            qgram_size: command.qgram,
            winnow_window: command.window,
        },
        lexical: LexicalIndexConfig {
            k1: command.k1,
            length_normalization: command.length_normalization,
            title_weight: command.title_weight,
            body_weight: command.body_weight,
            tags_weight: command.tags_weight,
        },
    };
    let mut builder = HybridIndexBuilder::new(config)?;
    for document in documents {
        builder.add_document(document)?;
    }
    let index = builder.build()?;
    index.save(&command.output)?;
    let report = BuildReport {
        output: command.output.display().to_string(),
        source_format: format_name(format).to_owned(),
        skipped,
        stats: index.stats(),
    };
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Wrote {}", report.output);
        println!("  source format:       {}", report.source_format);
        println!("  documents:           {}", report.stats.documents);
        println!(
            "  overlap fingerprints:{}",
            report.stats.overlap.distinct_fingerprints
        );
        println!("  overlap postings:    {}", report.stats.overlap.postings);
        println!("  lexical terms:       {}", report.stats.lexical.distinct_terms);
        println!("  lexical postings:    {}", report.stats.lexical.postings);
        println!("  skipped:             {}", report.skipped);
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
    let metadata_equals = parse_metadata_filters(&command.metadata_filters)?;
    let index = HybridIndex::load(&command.index)?;
    let report = index.search(
        query.trim(),
        &HybridSearchOptions {
            mode: command.mode.into(),
            max_results: command.limit,
            candidate_multiplier: command.candidate_multiplier,
            lexical: LexicalSearchOptions {
                max_results: command.limit,
                max_candidate_documents: command.lexical_candidates,
                candidate_term_limit: command.lexical_candidate_terms,
                maximum_postings_per_term: command.maximum_postings_per_term,
                minimum_should_match: command.minimum_should_match,
                require_phrases: command.require_phrases,
                ..LexicalSearchOptions::default()
            },
            overlap: SearchOptions {
                max_results: command.limit,
                max_postings_per_feature: command.maximum_postings_per_feature,
                minimum_matched_tokens: command.minimum_matched_tokens,
                minimum_query_coverage: command.minimum_query_coverage,
                minimum_source_coverage: command.minimum_source_coverage,
                minimum_similarity: command.overlap_candidate_floor,
                ..SearchOptions::default()
            },
            lexical_weight: command.lexical_weight,
            overlap_weight: command.overlap_weight,
            rrf_weight: command.rrf_weight,
            rrf_constant: command.rrf_constant,
            lexical_saturation: command.lexical_saturation,
            agreement_bonus: command.agreement_bonus,
            phrase_bonus: command.phrase_bonus,
            overlap_candidate_floor: command.overlap_candidate_floor,
            minimum_score: command.minimum_score,
            filter: HybridFilter {
                external_id_prefix: command.external_id_prefix,
                required_tags: command.required_tags,
                metadata_equals,
            },
            ..HybridSearchOptions::default()
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Mode: {:?} ({} unique terms, {} occurrences; lexical candidates={}, overlap candidates={})",
            report.selected_mode,
            report.positive_terms,
            report.positive_term_occurrences,
            report.lexical_candidates,
            report.overlap_candidates,
        );
        if let Some(plan) = &report.overlap_plan {
            println!(
                "Overlap route: {:?}; posting pairs={}; retained={:.3}",
                plan.route, plan.estimated_posting_pairs, plan.retained_fraction
            );
        }
        if report.results.is_empty() {
            println!("No results met the requested constraints.");
        }
        for (rank, result) in report.results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} lexical={:.4} overlap={:.4} rrf={:.4} agreement={}",
                rank + 1,
                result.title,
                result.score,
                result.explanation.lexical_score,
                result.explanation.overlap_score,
                result.explanation.reciprocal_rank_score,
                result.explanation.agreement,
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
        println!("Schema:               {}", stats.schema_version);
        println!("Documents:            {}", stats.documents);
        println!(
            "Overlap fingerprints: {}",
            stats.overlap.distinct_fingerprints
        );
        println!("Overlap postings:     {}", stats.overlap.postings);
        println!("Lexical terms:        {}", stats.lexical.distinct_terms);
        println!("Lexical postings:     {}", stats.lexical.postings);
        println!("Average body tokens:  {:.2}", stats.lexical.average_body_length);
    }
    Ok(())
}

fn parse_metadata_filters(values: &[String]) -> CliResult<BTreeMap<String, String>> {
    let mut filters = BTreeMap::new();
    for value in values {
        let Some((key, expected)) = value.split_once('=') else {
            return Err(invalid_input(format!(
                "metadata filter {value:?} must use KEY=VALUE"
            )));
        };
        if key.trim().is_empty() {
            return Err(invalid_input("metadata filter key must not be empty"));
        }
        filters.insert(key.trim().to_owned(), expected.to_owned());
    }
    Ok(filters)
}

fn resolve_input_format(path: &Path, requested: InputFormatArg) -> CliResult<InputFormatArg> {
    if !matches!(requested, InputFormatArg::Auto) {
        return Ok(requested);
    }
    if path.is_dir() && path.join(MANIFEST_FILENAME).is_file() {
        return Ok(InputFormatArg::Corpus);
    }
    if path.is_file() && is_jsonl(path) {
        return Ok(InputFormatArg::Jsonl);
    }
    if path.is_dir() || path.is_file() {
        return Ok(InputFormatArg::Directory);
    }
    Err(invalid_input(format!(
        "could not infer input format for {}",
        path.display()
    )))
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
            let path = root.join(&document.relative_path);
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

fn provider_tags(
    manifest: &CorpusManifest,
    document: &fo_corpus::CorpusDocument,
) -> Vec<String> {
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
) -> CliResult<(Vec<HybridDocumentInput>, usize)> {
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
            && value != relative.as_str()
        {
            tags.push(value.to_owned());
        }
    }
    tags.sort_unstable();
    tags.dedup();
    Ok(Some(HybridDocumentInput {
        external_id: relative,
        title,
        body,
        tags,
        metadata: BTreeMap::new(),
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
    values: Vec<CliResult<Option<HybridDocumentInput>>>,
) -> CliResult<(Vec<HybridDocumentInput>, usize)> {
    let mut documents = Vec::new();
    let mut skipped = 0usize;
    for value in values {
        match value? {
            Some(document) => documents.push(document),
            None => skipped += 1,
        }
    }
    documents.sort_unstable_by(|left, right| left.external_id.cmp(&right.external_id));
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
        })
}

fn validate_relative_path(value: &str) -> CliResult<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid_input(format!(
            "unsafe corpus relative path {value:?}"
        )));
    }
    Ok(())
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
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
