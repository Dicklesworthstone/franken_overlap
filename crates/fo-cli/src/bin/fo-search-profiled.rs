#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    HybridFilter, HybridFusionProfile, HybridIndex, HybridQueryMode, HybridSearchOptions,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-search-profiled",
    version,
    about = "Query a hybrid index using a held-out tuned fusion profile"
)]
struct Cli {
    index: PathBuf,
    profile: PathBuf,
    query: Option<String>,
    #[arg(long, conflicts_with = "query")]
    query_file: Option<PathBuf>,
    #[arg(long, value_enum)]
    mode: Option<ModeArg>,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    minimum_score: Option<f32>,
    #[arg(long)]
    external_id_prefix: Option<String>,
    #[arg(long = "require-tag")]
    required_tags: Vec<String>,
    #[arg(long = "metadata")]
    metadata_filters: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
    Auto,
    Hybrid,
    Overlap,
    Lexical,
}

impl From<ModeArg> for HybridQueryMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Auto => Self::Auto,
            ModeArg::Hybrid => Self::Hybrid,
            ModeArg::Overlap => Self::Overlap,
            ModeArg::Lexical => Self::Lexical,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-search-profiled: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    if command.limit == 0 {
        return Err(invalid_input("--limit must be positive"));
    }
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
    let profile = read_profile(&command.profile)?;
    let mut options = HybridSearchOptions::default();
    profile.apply(&mut options)?;
    options.max_results = command.limit;
    if let Some(mode) = command.mode {
        options.mode = mode.into();
    }
    if let Some(minimum_score) = command.minimum_score {
        if !minimum_score.is_finite() || !(0.0..=1.0).contains(&minimum_score) {
            return Err(invalid_input("--minimum-score must lie in [0, 1]"));
        }
        options.minimum_score = minimum_score;
    }
    options.filter = HybridFilter {
        external_id_prefix: command.external_id_prefix,
        required_tags: command.required_tags,
        metadata_equals: parse_metadata_filters(&command.metadata_filters)?,
    };
    options.validate()?;

    let index = HybridIndex::load(&command.index)?;
    let report = index.search(query.trim(), &options)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Profile:        {}", profile.name);
        println!("Trained from:   {:?}", profile.trained_from);
        println!("Requested mode: {:?}", report.requested_mode);
        println!("Selected mode:  {:?}", report.selected_mode);
        println!(
            "Weights L/O/R: {:.4}/{:.4}/{:.4}",
            profile.lexical_weight, profile.overlap_weight, profile.rrf_weight
        );
        if report.results.is_empty() {
            println!("No results met the profile and query constraints.");
        }
        for (rank, result) in report.results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} lexical={:.4} overlap={:.4} rrf={:.4}",
                rank + 1,
                result.title,
                result.score,
                result.explanation.lexical_score,
                result.explanation.overlap_score,
                result.explanation.reciprocal_rank_score,
            );
            println!("   {}", result.external_id);
            println!("   {}", result.snippet);
        }
    }
    Ok(())
}

fn read_profile(path: &Path) -> CliResult<HybridFusionProfile> {
    let profile = serde_json::from_slice::<HybridFusionProfile>(&fs::read(path)?)?;
    profile.validate()?;
    Ok(profile)
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

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
