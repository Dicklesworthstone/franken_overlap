#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use clap::Parser;
use fo_core::{
    HybridSearchReport, SemanticCandidateSet, SemanticEvidence, SemanticFusionOptions,
    fuse_semantic_candidates,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-semantic-fuse",
    version,
    about = "Fuse external semantic candidates with explainable lexical and textual-overlap evidence"
)]
struct Cli {
    /// HybridSearchReport JSON produced by fo-search query --json.
    hybrid_report: PathBuf,
    /// SemanticCandidateSet JSON, Vec<SemanticEvidence> JSON, or SemanticEvidence JSONL.
    semantic_candidates: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 0.0)]
    minimum_score: f32,
    #[arg(long, default_value_t = 0.0)]
    minimum_semantic_score: f32,
    #[arg(long, default_value_t = 0.70)]
    hybrid_weight: f32,
    #[arg(long, default_value_t = 0.20)]
    semantic_weight: f32,
    #[arg(long, default_value_t = 0.10)]
    rrf_weight: f32,
    #[arg(long, default_value_t = 60.0)]
    rrf_constant: f32,
    #[arg(long, default_value_t = 0.05)]
    agreement_bonus: f32,
    /// Permit candidates that have semantic evidence but no lexical or overlap evidence.
    #[arg(long)]
    allow_semantic_only: bool,
    #[arg(long, default_value_t = 5)]
    maximum_semantic_only: usize,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-semantic-fuse: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let hybrid = serde_json::from_slice::<HybridSearchReport>(&fs::read(&command.hybrid_report)?)?;
    let semantic = read_semantic_candidates(&command.semantic_candidates)?;
    let report = fuse_semantic_candidates(
        &hybrid,
        &semantic,
        &SemanticFusionOptions {
            max_results: command.limit,
            minimum_score: command.minimum_score,
            minimum_semantic_score: command.minimum_semantic_score,
            hybrid_weight: command.hybrid_weight,
            semantic_weight: command.semantic_weight,
            reciprocal_rank_weight: command.rrf_weight,
            reciprocal_rank_constant: command.rrf_constant,
            agreement_bonus: command.agreement_bonus,
            allow_semantic_only: command.allow_semantic_only,
            maximum_semantic_only: command.maximum_semantic_only,
        },
    )?;
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = command.output {
        atomic_write(&path, &bytes)?;
    }
    if command.json || command.output.is_none() {
        println!("{}", String::from_utf8(bytes)?);
    } else {
        println!("Hybrid candidates:         {}", report.hybrid_candidates);
        println!("Semantic candidates:       {}", report.semantic_candidates);
        println!("Cross-lane candidates:     {}", report.overlapping_candidates);
        println!(
            "Semantic-only retained:    {}",
            report.semantic_only_candidates_retained
        );
        for (rank, result) in report.results.iter().enumerate() {
            println!(
                "{}. {} score={:.4} relationship={:?} textual_provenance={} semantic_only={}",
                rank + 1,
                result.title,
                result.score,
                result.relationship,
                result.textual_provenance_supported,
                result.semantic_only,
            );
            println!("   id={}", result.external_id);
        }
    }
    Ok(())
}

fn read_semantic_candidates(path: &Path) -> CliResult<SemanticCandidateSet> {
    let bytes = fs::read(path)?;
    if let Ok(set) = serde_json::from_slice::<SemanticCandidateSet>(&bytes) {
        set.validate()?;
        return Ok(set);
    }
    if let Ok(candidates) = serde_json::from_slice::<Vec<SemanticEvidence>>(&bytes) {
        let set = SemanticCandidateSet {
            schema_version: 1,
            query_id: None,
            candidates,
        };
        set.validate()?;
        return Ok(set);
    }

    let file = fs::File::open(path)?;
    let mut candidates = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        candidates.push(serde_json::from_str::<SemanticEvidence>(value).map_err(|error| {
            invalid(format!("{}:{}: {error}", path.display(), line_index + 1))
        })?);
    }
    let set = SemanticCandidateSet {
        schema_version: 1,
        query_id: None,
        candidates,
    };
    set.validate()?;
    Ok(set)
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
