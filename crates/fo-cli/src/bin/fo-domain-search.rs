#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    DomainFeaturePolicy, DomainSearchOptions, Index, SearchIntent, SearchOptions, TextDomain,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-domain-search",
    version,
    about = "Search with domain-aware boilerplate suppression and an explicit posting-pair budget"
)]
struct Cli {
    index: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "general")]
    domain: DomainArg,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    maximum_postings_per_feature: usize,
    /// Override the domain profile's maximum document-frequency fraction.
    #[arg(long)]
    maximum_document_frequency_fraction: Option<f32>,
    /// Override the domain profile's minimum feature IDF.
    #[arg(long)]
    minimum_feature_idf: Option<f32>,
    /// Override the domain profile's total posting/query-position pair budget.
    #[arg(long)]
    maximum_query_posting_pairs: Option<u64>,
    /// Override the minimum fraction of selected query features that must survive.
    #[arg(long)]
    minimum_informative_feature_fraction: Option<f32>,
    /// Permit a corpus-wide direct fallback when too little informative evidence survives.
    #[arg(long)]
    allow_direct_fallback_on_thin_evidence: bool,
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
enum DomainArg {
    General,
    SecFiling,
    Contract,
    Ocr,
    SourceCode,
}

impl From<DomainArg> for TextDomain {
    fn from(value: DomainArg) -> Self {
        match value {
            DomainArg::General => Self::General,
            DomainArg::SecFiling => Self::SecFiling,
            DomainArg::Contract => Self::Contract,
            DomainArg::Ocr => Self::Ocr,
            DomainArg::SourceCode => Self::SourceCode,
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

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-domain-search: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = Index::load_auto(&command.index)?;
    let domain = command.domain.into();
    let mut policy = DomainFeaturePolicy::for_domain(domain);
    if let Some(value) = command.maximum_document_frequency_fraction {
        policy.maximum_document_frequency_fraction = value;
    }
    if let Some(value) = command.minimum_feature_idf {
        policy.minimum_feature_idf = value;
    }
    if let Some(value) = command.maximum_query_posting_pairs {
        policy.maximum_query_posting_pairs = value;
    }
    if let Some(value) = command.minimum_informative_feature_fraction {
        policy.minimum_informative_feature_fraction = value;
    }
    if command.allow_direct_fallback_on_thin_evidence {
        policy.allow_direct_fallback_on_thin_evidence = true;
    }
    let options = DomainSearchOptions {
        domain,
        policy,
        search: SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            max_postings_per_feature: command.maximum_postings_per_feature,
            maximum_document_frequency_fraction: policy.maximum_document_frequency_fraction,
            minimum_feature_idf: policy.minimum_feature_idf,
            maximum_query_posting_pairs: policy.maximum_query_posting_pairs,
            minimum_informative_feature_fraction: policy.minimum_informative_feature_fraction,
            minimum_matched_tokens: command.minimum_matched_tokens,
            minimum_query_coverage: command.minimum_query_coverage,
            minimum_source_coverage: command.minimum_source_coverage,
            minimum_similarity: command.minimum_similarity,
            ..SearchOptions::default()
        },
    };
    let mut report = index.search_domain(&specimen, &options)?;
    if command.plan_only {
        report.results.clear();
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Domain:                 {:?}", report.domain);
        println!("Status:                 {:?}", report.status);
        println!(
            "Selected / retained:    {} / {} feature occurrences ({:.3})",
            report.analysis.selected_feature_occurrences,
            report.analysis.retained_feature_occurrences,
            report.analysis.informative_feature_fraction,
        );
        println!(
            "Posting pairs:          {} -> {}",
            report.analysis.predicted_posting_pairs_before_policy,
            report.analysis.predicted_posting_pairs_after_policy,
        );
        println!(
            "Suppressed df/idf/cap/work: {}/{}/{}/{}",
            report.analysis.suppressed_by_document_frequency_occurrences,
            report.analysis.suppressed_by_idf_occurrences,
            report.analysis.suppressed_by_posting_cap_occurrences,
            report.analysis.suppressed_by_work_budget_occurrences,
        );
        println!("Mean retained IDF:      {:.3}", report.analysis.mean_retained_idf);
        if !command.plan_only {
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
                println!("   {}", one_line(&result.matched_text, 240));
            }
        }
    }
    Ok(())
}

fn specimen_text(path: Option<&Path>, inline: Option<String>) -> CliResult<String> {
    match (path, inline) {
        (Some(path), None) => Ok(fs::read_to_string(path)?),
        (None, Some(text)) => Ok(text),
        (None, None) => Err(invalid("provide a specimen file or --text")),
        (Some(_), Some(_)) => Err(invalid(
            "specimen file and --text are mutually exclusive",
        )),
    }
}

fn one_line(value: &str, maximum: usize) -> String {
    let mut output = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if output.chars().count() > maximum {
        output = output.chars().take(maximum).collect::<String>();
        output.push('…');
    }
    output
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
