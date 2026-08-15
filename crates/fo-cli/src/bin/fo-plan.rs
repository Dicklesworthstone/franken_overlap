#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    AdaptiveMatch, CompositeSearchOptions, Index, QueryPlannerOptions, SearchIntent, SearchOptions,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-plan",
    version,
    about = "Inspect query difficulty and optionally execute FrankenOverlap's adaptive route"
)]
struct Cli {
    index: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    /// Execute the selected ordinary/composite route after planning.
    #[arg(long)]
    execute: bool,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    max_postings: usize,
    #[arg(long, default_value_t = 25_000_000)]
    sparse_posting_pair_budget: u64,
    #[arg(long, default_value_t = 256)]
    composite_minimum_tokens: usize,
    #[arg(long, default_value_t = 0.55)]
    composite_retained_fraction: f32,
    #[arg(long, default_value_t = 0.25)]
    composite_repetition_fraction: f32,
    #[arg(long, default_value_t = 0.38)]
    low_entropy_ratio: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    #[arg(long, default_value_t = 8)]
    maximum_blocks: usize,
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
        eprintln!("fo-plan: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = Index::load_auto(&command.index)?;
    let search = SearchOptions {
        intent: command.intent.into(),
        max_results: command.limit,
        max_candidates: command.candidates,
        max_postings_per_feature: command.max_postings,
        minimum_matched_tokens: command.minimum_matched_tokens,
        minimum_similarity: command.minimum_similarity,
        ..SearchOptions::default()
    };
    let planner = QueryPlannerOptions {
        maximum_sparse_posting_pairs: command.sparse_posting_pair_budget,
        composite_minimum_tokens: command.composite_minimum_tokens,
        composite_retained_fraction: command.composite_retained_fraction,
        composite_repetition_fraction: command.composite_repetition_fraction,
        low_entropy_ratio: command.low_entropy_ratio,
        ..QueryPlannerOptions::default()
    };

    if command.execute {
        let report = index.search_adaptive(
            &specimen,
            &search,
            planner,
            CompositeSearchOptions {
                maximum_blocks_per_document: command.maximum_blocks,
                minimum_block_tokens: command.minimum_matched_tokens.min(20).max(1),
                minimum_incremental_query_tokens: command.minimum_matched_tokens.min(12).max(1),
                minimum_aggregate_score: command.minimum_similarity,
                ..CompositeSearchOptions::default()
            },
        )?;
        if command.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_plan(&report.plan);
            println!(
                "Effective posting cap: {}",
                report.effective_max_postings_per_feature
            );
            println!("Matches: {}", report.matches.len());
            for (rank, item) in report.matches.iter().enumerate() {
                match item {
                    AdaptiveMatch::Passage(result) => println!(
                        "  {}. passage {} score={:.4} query={:.4} tokens={}",
                        rank + 1,
                        result.path,
                        result.combined_score,
                        result.query_coverage,
                        result.matched_tokens,
                    ),
                    AdaptiveMatch::Composite(result) => println!(
                        "  {}. composite {} score={:.4} query={:.4} blocks={}",
                        rank + 1,
                        result.path,
                        result.aggregate_score,
                        result.query_coverage,
                        result.blocks.len(),
                    ),
                }
            }
        }
    } else {
        let plan = index.plan_query(&specimen, &search, planner)?;
        if command.json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print_plan(&plan);
        }
    }
    Ok(())
}

fn print_plan(plan: &fo_core::QueryPlan) {
    println!("Route:                     {:?}", plan.route);
    println!("Advisories:                {:?}", plan.advisories);
    println!("Normalized tokens:         {}", plan.normalized_tokens);
    println!("Distinct tokens:           {}", plan.distinct_tokens);
    println!("Entropy ratio:             {:.4}", plan.token_entropy_ratio);
    println!("Repetition fraction:       {:.4}", plan.repetition_fraction);
    println!("Selected features:         {}", plan.selected_features);
    println!("Retained features:         {}", plan.retained_features);
    println!("Missing features:          {}", plan.missing_features);
    println!("Suppressed features:       {}", plan.suppressed_features);
    println!("Retained fraction:         {:.4}", plan.retained_fraction);
    println!("Estimated posting pairs:   {}", plan.estimated_posting_pairs);
    println!("Estimated diagonal votes:  {}", plan.estimated_diagonal_votes);
    println!(
        "Suggested posting cap:     {}",
        plan.suggested_max_postings_per_feature
    );
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

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
