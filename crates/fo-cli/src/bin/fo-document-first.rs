#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    DocumentFirstOptions, Index, PreparedOverlapQuery, SearchIntent, SearchOptions,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-document-first",
    version,
    about = "Prepare one specimen, rank plausible documents, then run positional alignment only on the retained set"
)]
struct Cli {
    index: PathBuf,
    /// Specimen text file. Omit when using --text or --prepared-query.
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with_all = ["specimen", "prepared_query"])]
    text: Option<String>,
    /// Load a previously serialized PreparedOverlapQuery.
    #[arg(long, conflicts_with_all = ["specimen", "text"])]
    prepared_query: Option<PathBuf>,
    /// Persist the prepared query for reuse by later searches.
    #[arg(long)]
    prepared_output: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 128)]
    maximum_documents: usize,
    #[arg(long, default_value_t = 0.08)]
    minimum_document_score_fraction: f32,
    #[arg(long, default_value_t = 2)]
    minimum_distinct_features: usize,
    #[arg(long, default_value_t = 50_000)]
    maximum_postings_per_feature: usize,
    #[arg(long, default_value_t = 10_000_000)]
    maximum_posting_pairs: u64,
    #[arg(long, default_value_t = 0.50)]
    maximum_selected_document_fraction: f32,
    #[arg(long)]
    no_full_index_fallback: bool,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    #[arg(long)]
    plan_only: bool,
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
        eprintln!("fo-document-first: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let index = Index::load_auto(&command.index)?;
    let prepared = match command.prepared_query.as_deref() {
        Some(path) => serde_json::from_slice::<PreparedOverlapQuery>(&fs::read(path)?)?,
        None => index.prepare_overlap_query(&specimen_text(
            command.specimen.as_deref(),
            command.text,
        )?)?,
    };
    prepared.validate_for(&index)?;
    if let Some(path) = command.prepared_output.as_deref() {
        atomic_write(path, &serde_json::to_vec_pretty(&prepared)?)?;
    }
    let mut report = index.search_document_first(
        &prepared,
        &DocumentFirstOptions {
            maximum_documents: command.maximum_documents,
            minimum_document_score_fraction: command.minimum_document_score_fraction,
            minimum_distinct_features: command.minimum_distinct_features,
            maximum_postings_per_feature: command.maximum_postings_per_feature,
            maximum_posting_pairs: command.maximum_posting_pairs,
            maximum_selected_document_fraction: command.maximum_selected_document_fraction,
            fallback_to_full_index: !command.no_full_index_fallback,
        },
        &SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            max_postings_per_feature: command.maximum_postings_per_feature,
            minimum_matched_tokens: command.minimum_matched_tokens,
            minimum_query_coverage: command.minimum_query_coverage,
            minimum_source_coverage: command.minimum_source_coverage,
            minimum_similarity: command.minimum_similarity,
            ..SearchOptions::default()
        },
    )?;
    if command.plan_only {
        report.results.clear();
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Status:                  {:?}", report.status);
        println!("Corpus documents:        {}", report.corpus_documents);
        println!("Selected documents:      {}", report.selected_documents);
        println!("Selected fraction:       {:.4}", report.selected_fraction);
        println!(
            "Prepared feature occurrences: {}",
            report.prepared_feature_occurrences
        );
        println!("Retained distinct features: {}", report.retained_distinct_features);
        println!("Postings scanned:        {}", report.postings_scanned);
        println!("Posting pairs:           {}", report.posting_pairs);
        println!(
            "Suppressed cap / work:  {} / {}",
            report.suppressed_features_by_posting_cap,
            report.suppressed_features_by_work_budget
        );
        println!("\nDocument candidates:");
        for (rank, candidate) in report.document_candidates.iter().take(20).enumerate() {
            println!(
                "{}. {} score={:.3} features={} occurrences={}",
                rank + 1,
                candidate.path,
                candidate.score,
                candidate.distinct_features,
                candidate.matched_query_feature_occurrences,
            );
        }
        if !command.plan_only {
            println!("\nVerified results:");
            for (rank, result) in report.results.iter().enumerate() {
                println!(
                    "{}. {} score={:.4} edit={:.4} query={:.4} source={:.4} tokens={}",
                    rank + 1,
                    result.path,
                    result.combined_score,
                    result.edit_similarity,
                    result.query_coverage,
                    result.source_coverage,
                    result.matched_tokens,
                );
            }
        }
    }
    Ok(())
}

fn specimen_text(path: Option<&Path>, inline: Option<String>) -> CliResult<String> {
    match (path, inline) {
        (Some(path), None) => Ok(fs::read_to_string(path)?),
        (None, Some(text)) => Ok(text),
        (None, None) => Err(invalid(
            "provide a specimen file, --text, or --prepared-query",
        )),
        (Some(_), Some(_)) => Err(invalid(
            "specimen file and --text are mutually exclusive",
        )),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension().and_then(|value| value.to_str()).unwrap_or("json"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
