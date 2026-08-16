#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_core::{
    benchmark_contract_portfolio, compare_contracts, ContractAnalysis, ContractComparison,
    ContractComparisonOptions, ContractPortfolioBenchmark, ContractPortfolioOptions,
    PortfolioDocumentAnalysis,
};
use fo_corpus::{
    atomic_write, verify_collection, CollectionDocumentRecord, CollectionManifest,
    CollectionRelationKind,
};
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-contract-compare",
    version,
    about = "Compare agreement versions and benchmark contract portfolios"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compare two ContractAnalysis JSON files.
    Pair(PairCommand),
    /// Compare adjacent family versions and benchmark a complete collection.
    Portfolio(PortfolioCommand),
}

#[derive(Debug, Args)]
struct PairCommand {
    previous: PathBuf,
    current: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[command(flatten)]
    comparison: ComparisonArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PortfolioCommand {
    collection_root: PathBuf,
    analysis_root: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    family: Option<String>,
    #[arg(long, default_value_t = 10_000)]
    maximum_documents: usize,
    #[command(flatten)]
    comparison: ComparisonArgs,
    #[command(flatten)]
    portfolio: PortfolioArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct ComparisonArgs {
    #[arg(long, default_value_t = 0.42)]
    minimum_clause_alignment_score: f32,
    #[arg(long, default_value_t = 0.975)]
    unchanged_similarity: f32,
    #[arg(long, default_value_t = 0.82)]
    minor_revision_similarity: f32,
    #[arg(long, default_value_t = 1_000_000)]
    maximum_clause_pair_evaluations: usize,
    #[arg(long, default_value_t = 4_096)]
    maximum_exact_tokens: usize,
    #[arg(long, default_value_t = 1_000)]
    maximum_alerts: usize,
}

impl From<ComparisonArgs> for ContractComparisonOptions {
    fn from(value: ComparisonArgs) -> Self {
        Self {
            minimum_clause_alignment_score: value.minimum_clause_alignment_score,
            unchanged_similarity: value.unchanged_similarity,
            minor_revision_similarity: value.minor_revision_similarity,
            maximum_clause_pair_evaluations: value.maximum_clause_pair_evaluations,
            maximum_exact_tokens: value.maximum_exact_tokens,
            maximum_alerts: value.maximum_alerts,
            ..ContractComparisonOptions::default()
        }
    }
}

#[derive(Debug, Clone, Args)]
struct PortfolioArgs {
    #[arg(long, default_value_t = 0.10)]
    rare_clause_prevalence: f32,
    #[arg(long, default_value_t = 0.80)]
    common_clause_prevalence: f32,
    #[arg(long, default_value_t = 3.5)]
    outlier_mad_multiplier: f64,
    #[arg(long, default_value_t = 5)]
    minimum_distribution_observations: usize,
    #[arg(long, default_value_t = 10_000)]
    maximum_outliers: usize,
}

impl From<PortfolioArgs> for ContractPortfolioOptions {
    fn from(value: PortfolioArgs) -> Self {
        Self {
            rare_clause_prevalence: value.rare_clause_prevalence,
            common_clause_prevalence: value.common_clause_prevalence,
            outlier_mad_multiplier: value.outlier_mad_multiplier,
            minimum_distribution_observations: value.minimum_distribution_observations,
            maximum_outliers: value.maximum_outliers,
        }
    }
}

#[derive(Debug, Serialize)]
struct PortfolioComparisonSummary {
    collection_id: String,
    documents: usize,
    families: usize,
    comparisons: usize,
    alerts: usize,
    benchmark_path: String,
    alerts_path: String,
    comparison_files: Vec<ComparisonFile>,
}

#[derive(Debug, Serialize)]
struct ComparisonFile {
    family_id: String,
    previous_id: String,
    current_id: String,
    path: String,
    overall_similarity: f32,
    alerts: usize,
    materially_revised_clauses: usize,
}

#[derive(Debug, Serialize)]
struct AlertRow<'a> {
    family_id: &'a str,
    previous_id: &'a str,
    current_id: &'a str,
    alert: &'a fo_core::ContractChangeAlert,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-contract-compare: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Pair(command) => run_pair(command),
        Command::Portfolio(command) => run_portfolio(command),
    }
}

fn run_pair(command: PairCommand) -> CliResult<()> {
    let previous = read_analysis(&command.previous)?;
    let current = read_analysis(&command.current)?;
    let comparison = compare_contracts(&previous, &current, &command.comparison.into())?;
    let bytes = serde_json::to_vec_pretty(&comparison)?;
    if let Some(output) = command.output.as_ref() {
        atomic_write(output, &bytes)?;
    }
    if command.json || command.output.is_none() {
        println!("{}", String::from_utf8(bytes)?);
    } else {
        print_pair(&comparison);
    }
    Ok(())
}

fn run_portfolio(command: PortfolioCommand) -> CliResult<()> {
    if command.maximum_documents == 0 {
        return Err(invalid("--maximum-documents must be positive"));
    }
    verify_collection(&command.collection_root)?;
    let manifest = CollectionManifest::load(&command.collection_root)?;
    if command.output.exists() {
        return Err(invalid(format!(
            "output directory already exists: {}",
            command.output.display()
        )));
    }
    fs::create_dir_all(command.output.join("comparisons"))?;

    let documents = manifest
        .documents
        .iter()
        .filter(|document| {
            command
                .family
                .as_deref()
                .is_none_or(|family| document.family_id == family)
        })
        .take(command.maximum_documents)
        .collect::<Vec<_>>();
    if documents.is_empty() {
        return Err(invalid("no documents matched the requested portfolio scope"));
    }

    let mut analyses = BTreeMap::<String, ContractAnalysis>::new();
    for document in &documents {
        let path = analysis_path(&command.analysis_root, &document.id);
        analyses.insert(document.id.clone(), read_analysis(&path).map_err(|error| {
            invalid(format!("could not load analysis for {}: {error}", document.id))
        })?);
    }
    let portfolio_input = documents
        .iter()
        .map(|document| PortfolioDocumentAnalysis {
            document_id: document.id.clone(),
            analysis: analyses[&document.id].clone(),
        })
        .collect::<Vec<_>>();
    let benchmark = benchmark_contract_portfolio(&portfolio_input, &command.portfolio.into())?;
    let benchmark_path = command.output.join("benchmark.json");
    atomic_write(&benchmark_path, &serde_json::to_vec_pretty(&benchmark)?)?;

    let comparison_options = ContractComparisonOptions::from(command.comparison);
    let pairs = comparison_pairs(&manifest, &documents);
    let alerts_path = command.output.join("alerts.jsonl");
    let alerts_file = File::create(&alerts_path)?;
    let mut alerts_writer = BufWriter::new(alerts_file);
    let mut comparison_files = Vec::new();
    let mut total_alerts = 0usize;

    for (family_id, previous, current) in pairs {
        let comparison = compare_contracts(
            &analyses[&previous.id],
            &analyses[&current.id],
            &comparison_options,
        )?;
        let filename = format!(
            "{}--{}--{}.json",
            safe_name(&family_id),
            safe_name(&previous.id),
            safe_name(&current.id)
        );
        let relative = format!("comparisons/{filename}");
        atomic_write(
            &command.output.join(&relative),
            &serde_json::to_vec_pretty(&comparison)?,
        )?;
        for alert in &comparison.alerts {
            serde_json::to_writer(
                &mut alerts_writer,
                &AlertRow {
                    family_id: &family_id,
                    previous_id: &previous.id,
                    current_id: &current.id,
                    alert,
                },
            )?;
            alerts_writer.write_all(b"\n")?;
        }
        total_alerts += comparison.alerts.len();
        comparison_files.push(ComparisonFile {
            family_id,
            previous_id: previous.id.clone(),
            current_id: current.id.clone(),
            path: relative,
            overall_similarity: comparison.overall_similarity,
            alerts: comparison.alerts.len(),
            materially_revised_clauses: comparison.materially_revised_clauses,
        });
    }
    alerts_writer.flush()?;
    comparison_files.sort_unstable_by(|left, right| {
        left.family_id
            .cmp(&right.family_id)
            .then_with(|| left.current_id.cmp(&right.current_id))
    });
    let families = documents
        .iter()
        .map(|document| document.family_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let summary = PortfolioComparisonSummary {
        collection_id: manifest.collection_id,
        documents: documents.len(),
        families,
        comparisons: comparison_files.len(),
        alerts: total_alerts,
        benchmark_path: "benchmark.json".to_owned(),
        alerts_path: "alerts.jsonl".to_owned(),
        comparison_files,
    };
    atomic_write(
        &command.output.join("summary.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    atomic_write(
        &command.output.join("SUMMARY.md"),
        render_summary(&summary, &benchmark).as_bytes(),
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("Collection:  {}", summary.collection_id);
        println!("Documents:   {}", summary.documents);
        println!("Families:    {}", summary.families);
        println!("Comparisons: {}", summary.comparisons);
        println!("Alerts:      {}", summary.alerts);
        println!("Outliers:    {}", benchmark.outliers.len());
        println!("Output:      {}", command.output.display());
    }
    Ok(())
}

fn comparison_pairs<'a>(
    manifest: &'a CollectionManifest,
    scoped: &[&'a CollectionDocumentRecord],
) -> Vec<(String, &'a CollectionDocumentRecord, &'a CollectionDocumentRecord)> {
    let scope_ids = scoped
        .iter()
        .map(|document| document.id.as_str())
        .collect::<BTreeSet<_>>();
    let by_id = scoped
        .iter()
        .map(|document| (document.id.as_str(), *document))
        .collect::<BTreeMap<_, _>>();
    let mut pairs = Vec::new();
    let mut seen = BTreeSet::new();
    for relation in &manifest.relations {
        if relation.kind != CollectionRelationKind::PreviousVersion
            || !scope_ids.contains(relation.from_id.as_str())
            || !scope_ids.contains(relation.to_id.as_str())
        {
            continue;
        }
        let current = by_id[relation.from_id.as_str()];
        let previous = by_id[relation.to_id.as_str()];
        if seen.insert((previous.id.as_str(), current.id.as_str())) {
            pairs.push((current.family_id.clone(), previous, current));
        }
    }
    let families = scoped
        .iter()
        .map(|document| document.family_id.as_str())
        .collect::<BTreeSet<_>>();
    for family in families {
        let versions = manifest
            .family(family)
            .into_iter()
            .filter(|document| scope_ids.contains(document.id.as_str()))
            .collect::<Vec<_>>();
        for window in versions.windows(2) {
            let previous = window[0];
            let current = window[1];
            if seen.insert((previous.id.as_str(), current.id.as_str())) {
                pairs.push((family.to_owned(), previous, current));
            }
        }
    }
    pairs.sort_unstable_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.2.version_id.cmp(&right.2.version_id))
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    pairs
}

fn read_analysis(path: &Path) -> CliResult<ContractAnalysis> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn analysis_path(root: &Path, id: &str) -> PathBuf {
    root.join("documents").join(format!("{}.json", safe_name(id)))
}

fn print_pair(comparison: &ContractComparison) {
    println!("Overall similarity:          {:.4}", comparison.overall_similarity);
    println!("Matched clauses:             {}", comparison.matched_clauses);
    println!("Added clauses:               {}", comparison.added_clauses);
    println!("Removed clauses:             {}", comparison.removed_clauses);
    println!(
        "Materially revised clauses: {}",
        comparison.materially_revised_clauses
    );
    println!("Definition changes:          {}", comparison.definition_changes.len());
    println!("Obligation changes:          {}", comparison.obligation_changes.len());
    println!("Economic changes:            {}", comparison.economic_term_changes.len());
    println!("Alerts:                      {}", comparison.alerts.len());
    for alert in &comparison.alerts {
        println!(
            "  {:.3} {:?} {:?}: {}",
            alert.severity, alert.impact, alert.direction, alert.title
        );
    }
}

fn render_summary(
    summary: &PortfolioComparisonSummary,
    benchmark: &ContractPortfolioBenchmark,
) -> String {
    let mut output = format!(
        "# Contract portfolio intelligence\n\n- Collection: `{}`\n- Documents: {}\n- Families: {}\n- Version comparisons: {}\n- Change alerts: {}\n- Portfolio outliers: {}\n\n## Version comparisons\n\n| Family | Previous | Current | Similarity | Material revisions | Alerts |\n|---|---|---|---:|---:|---:|\n",
        summary.collection_id,
        summary.documents,
        summary.families,
        summary.comparisons,
        summary.alerts,
        benchmark.outliers.len(),
    );
    for comparison in &summary.comparison_files {
        output.push_str(&format!(
            "| `{}` | `{}` | `{}` | {:.3} | {} | {} |\n",
            comparison.family_id,
            comparison.previous_id,
            comparison.current_id,
            comparison.overall_similarity,
            comparison.materially_revised_clauses,
            comparison.alerts,
        ));
    }
    output.push_str("\n## Highest portfolio outliers\n\n| Severity | Document | Code | Description |\n|---:|---|---|---|\n");
    for outlier in benchmark.outliers.iter().take(100) {
        output.push_str(&format!(
            "| {:.3} | `{}` | `{}` | {} |\n",
            outlier.severity,
            outlier.document_id,
            outlier.code,
            outlier.description.replace('|', "\\|"),
        ));
    }
    output
}

fn safe_name(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "document".to_owned()
    } else {
        output
    }
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
