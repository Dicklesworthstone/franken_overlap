#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use fo_corpus::{section_corpus, SectionCorpusOptions, SectionStrategy};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-section",
    version,
    about = "Derive chapter, 10-K item, or overlapping paragraph-window corpora"
)]
struct Cli {
    /// Existing fo-corpus root.
    input: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "auto")]
    strategy: StrategyArg,
    #[arg(long, default_value_t = 2_000)]
    minimum_characters: usize,
    #[arg(long, default_value_t = 18_000)]
    target_characters: usize,
    #[arg(long, default_value_t = 36_000)]
    maximum_characters: usize,
    #[arg(long, default_value_t = 1_000)]
    overlap_characters: usize,
    #[arg(long, default_value_t = 512)]
    maximum_sections_per_document: usize,
    #[arg(long)]
    replace_output: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StrategyArg {
    Auto,
    Gutenberg,
    Sec10k,
    ParagraphWindows,
}

impl From<StrategyArg> for SectionStrategy {
    fn from(value: StrategyArg) -> Self {
        match value {
            StrategyArg::Auto => Self::Auto,
            StrategyArg::Gutenberg => Self::Gutenberg,
            StrategyArg::Sec10k => Self::Sec10K,
            StrategyArg::ParagraphWindows => Self::ParagraphWindows,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-section: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let report = section_corpus(
        &command.input,
        SectionCorpusOptions {
            output_dir: command.output,
            strategy: command.strategy.into(),
            minimum_characters: command.minimum_characters,
            target_characters: command.target_characters,
            maximum_characters: command.maximum_characters,
            overlap_characters: command.overlap_characters,
            maximum_sections_per_document: command.maximum_sections_per_document,
            replace_output: command.replace_output,
        },
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:                   {}", report.manifest.corpus_id);
        println!("Parent documents:         {}", report.parent_documents);
        println!("Section documents:        {}", report.section_documents);
        println!("Heading sections:         {}", report.heading_sections);
        println!("Window sections:          {}", report.window_sections);
        println!(
            "Skipped parent documents:{}",
            report.skipped_parent_documents
        );
        println!("Source bytes:             {}", report.total_source_bytes);
        println!("Section bytes:            {}", report.total_section_bytes);
    }
    Ok(())
}
