#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{CompositeSearchOptions, Index, SearchIntent, SearchOptions};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-composite",
    version,
    about = "Find fragmented and reordered textual reuse from one source document"
)]
struct Cli {
    /// FrankenOverlap index.
    index: PathBuf,
    /// Specimen text file. Omit when using --text.
    specimen: Option<PathBuf>,
    /// Supply the specimen inline.
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 400)]
    candidates: usize,
    #[arg(long, default_value_t = 8)]
    maximum_blocks: usize,
    #[arg(long, default_value_t = 20)]
    minimum_block_tokens: usize,
    #[arg(long, default_value_t = 12)]
    minimum_incremental_query_tokens: usize,
    #[arg(long, default_value_t = 0.70)]
    maximum_overlap_fraction: f32,
    #[arg(long, default_value_t = 0.30)]
    minimum_score: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum IntentArg {
    SourceAttribution,
    NearDuplicate,
}

impl From<IntentArg> for SearchIntent {
    fn from(value: IntentArg) -> Self {
        match value {
            IntentArg::SourceAttribution => Self::SourceAttribution,
            IntentArg::NearDuplicate => Self::NearDuplicate,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-composite: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let specimen = specimen_text(command.specimen.as_deref(), command.text)?;
    let index = Index::load_auto(&command.index)?;
    let results = index.search_composite(
        &specimen,
        &SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            minimum_similarity: command.minimum_score,
            minimum_query_coverage: command.minimum_query_coverage,
            minimum_source_coverage: command.minimum_source_coverage,
            minimum_matched_tokens: command.minimum_block_tokens,
            ..SearchOptions::default()
        },
        CompositeSearchOptions {
            maximum_blocks_per_document: command.maximum_blocks,
            minimum_block_tokens: command.minimum_block_tokens,
            minimum_incremental_query_tokens: command.minimum_incremental_query_tokens,
            maximum_overlap_fraction: command.maximum_overlap_fraction,
            minimum_aggregate_score: command.minimum_score,
        },
    )?;

    if command.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("No composite source met the requested thresholds.");
    } else {
        for (rank, result) in results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} query={:.4} source={:.4} edit={:.4} blocks={} reordered={} tokens={} expected_fp={:.3e}",
                rank + 1,
                result.path,
                result.aggregate_score,
                result.query_coverage,
                result.source_coverage,
                result.weighted_edit_similarity,
                result.blocks.len(),
                result.reordered_blocks,
                result.matched_tokens,
                result.expected_false_matches,
            );
            for (block_index, block) in result.blocks.iter().enumerate() {
                println!(
                    "   block {} query=[{}..{}] corpus=[{}..{}] edit={:.4} score={:.4} {}",
                    block_index + 1,
                    block.query_start,
                    block.query_end,
                    block.corpus_start,
                    block.corpus_end,
                    block.edit_similarity,
                    block.raw_score,
                    one_line(&block.matched_text, 180),
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
        (None, None) => Err(invalid_input("provide a specimen file or --text")),
        (Some(_), Some(_)) => Err(invalid_input(
            "specimen file and --text are mutually exclusive",
        )),
    }
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
