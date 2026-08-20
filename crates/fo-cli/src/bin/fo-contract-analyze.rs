#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_core::{
    ClauseKind, ContractAnalysis, ContractAnalysisOptions, ContractProfile, analyze_contract,
};
use fo_corpus::{CollectionManifest, atomic_write, verify_collection};
use rayon::prelude::*;
use serde::Serialize;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-contract-analyze",
    version,
    about = "Extract clauses, definitions, obligations, and economic terms from agreements"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze one UTF-8 agreement.
    Document(DocumentCommand),
    /// Analyze every document in a verified related-document collection.
    Collection(CollectionCommand),
}

#[derive(Debug, Args)]
struct DocumentCommand {
    input: PathBuf,
    #[arg(long, value_enum, default_value = "general")]
    profile: ProfileArg,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[command(flatten)]
    limits: LimitArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct CollectionCommand {
    root: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, value_enum)]
    profile: Option<ProfileArg>,
    #[arg(long)]
    family: Option<String>,
    #[arg(long, default_value_t = 10_000)]
    maximum_documents: usize,
    #[arg(long, default_value_t = 0)]
    threads: usize,
    #[command(flatten)]
    limits: LimitArgs,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct LimitArgs {
    #[arg(long, default_value_t = 24)]
    minimum_clause_characters: usize,
    #[arg(long, default_value_t = 64_000)]
    maximum_clause_characters: usize,
    #[arg(long, default_value_t = 10_000)]
    maximum_clauses: usize,
    #[arg(long, default_value_t = 20_000)]
    maximum_definitions: usize,
    #[arg(long, default_value_t = 100_000)]
    maximum_obligations: usize,
    #[arg(long, default_value_t = 100_000)]
    maximum_economic_terms: usize,
}

impl From<LimitArgs> for ContractAnalysisOptions {
    fn from(value: LimitArgs) -> Self {
        Self {
            minimum_clause_characters: value.minimum_clause_characters,
            maximum_clause_characters: value.maximum_clause_characters,
            maximum_clauses: value.maximum_clauses,
            maximum_definitions: value.maximum_definitions,
            maximum_obligations: value.maximum_obligations,
            maximum_economic_terms: value.maximum_economic_terms,
            ..ContractAnalysisOptions::default()
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProfileArg {
    General,
    RetailLease,
    ProfessionalServices,
    Nda,
}

impl From<ProfileArg> for ContractProfile {
    fn from(value: ProfileArg) -> Self {
        match value {
            ProfileArg::General => Self::General,
            ProfileArg::RetailLease => Self::RetailLease,
            ProfileArg::ProfessionalServices => Self::ProfessionalServices,
            ProfileArg::Nda => Self::Nda,
        }
    }
}

#[derive(Debug, Serialize)]
struct CollectionAnalysisSummary {
    collection_id: String,
    profile: ContractProfile,
    analyzed_documents: usize,
    failed_documents: usize,
    total_clauses: usize,
    total_definitions: usize,
    total_obligations: usize,
    total_economic_terms: usize,
    clause_counts: BTreeMap<ClauseKind, usize>,
    warning_counts: BTreeMap<String, usize>,
    documents: Vec<DocumentSummary>,
}

#[derive(Debug, Serialize)]
struct DocumentSummary {
    id: String,
    family_id: String,
    version_id: String,
    title: String,
    analysis_path: Option<String>,
    clauses: usize,
    definitions: usize,
    obligations: usize,
    economic_terms: usize,
    warnings: usize,
    error: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-contract-analyze: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Document(command) => run_document(command),
        Command::Collection(command) => run_collection(command),
    }
}

fn run_document(command: DocumentCommand) -> CliResult<()> {
    let text = fs::read_to_string(&command.input)?;
    let analysis = analyze_contract(&text, command.profile.into(), &command.limits.into())?;
    let bytes = serde_json::to_vec_pretty(&analysis)?;
    if let Some(path) = command.output.as_ref() {
        atomic_write(path, &bytes)?;
    }
    if command.json || command.output.is_none() {
        println!("{}", String::from_utf8(bytes)?);
    } else {
        print_analysis(&command.input, &analysis);
    }
    Ok(())
}

fn run_collection(command: CollectionCommand) -> CliResult<()> {
    if command.maximum_documents == 0 {
        return Err(invalid("--maximum-documents must be positive"));
    }
    verify_collection(&command.root)?;
    let manifest = CollectionManifest::load(&command.root)?;
    let profile = command
        .profile
        .map(ContractProfile::from)
        .unwrap_or_else(|| profile_from_collection(manifest.profile));
    let options = ContractAnalysisOptions::from(command.limits);
    options.validate()?;
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
        .cloned()
        .collect::<Vec<_>>();
    if documents.is_empty() {
        return Err(invalid(
            "no collection documents matched the requested scope",
        ));
    }
    if command.output.exists() {
        return Err(invalid(format!(
            "output directory already exists: {}",
            command.output.display()
        )));
    }
    fs::create_dir_all(command.output.join("documents"))?;

    let analyze = || {
        documents
            .par_iter()
            .map(|document| {
                let source_path = command.root.join(&document.stored_path);
                let result = fs::read_to_string(&source_path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        analyze_contract(&text, profile, &options)
                            .map_err(|error| error.to_string())
                    });
                (document, result)
            })
            .collect::<Vec<_>>()
    };
    let analyzed = if command.threads == 0 {
        analyze()
    } else {
        rayon::ThreadPoolBuilder::new()
            .num_threads(command.threads)
            .build()?
            .install(analyze)
    };

    let mut summaries = Vec::with_capacity(analyzed.len());
    let mut clause_counts = BTreeMap::new();
    let mut warning_counts = BTreeMap::new();
    let mut total_clauses = 0usize;
    let mut total_definitions = 0usize;
    let mut total_obligations = 0usize;
    let mut total_economic_terms = 0usize;
    let mut failed = 0usize;

    for (document, result) in analyzed {
        match result {
            Ok(analysis) => {
                let relative = format!("documents/{}.json", safe_name(&document.id));
                atomic_write(
                    &command.output.join(&relative),
                    &serde_json::to_vec_pretty(&analysis)?,
                )?;
                total_clauses += analysis.clauses.len();
                total_definitions += analysis.definitions.len();
                total_obligations += analysis.obligations.len();
                total_economic_terms += analysis.economic_terms.len();
                for (kind, count) in &analysis.clause_counts {
                    *clause_counts.entry(*kind).or_insert(0usize) += *count;
                }
                for warning in &analysis.warnings {
                    *warning_counts.entry(warning.code.clone()).or_insert(0usize) += 1;
                }
                summaries.push(DocumentSummary {
                    id: document.id.clone(),
                    family_id: document.family_id.clone(),
                    version_id: document.version_id.clone(),
                    title: document.title.clone(),
                    analysis_path: Some(relative),
                    clauses: analysis.clauses.len(),
                    definitions: analysis.definitions.len(),
                    obligations: analysis.obligations.len(),
                    economic_terms: analysis.economic_terms.len(),
                    warnings: analysis.warnings.len(),
                    error: None,
                });
            }
            Err(error) => {
                failed += 1;
                summaries.push(DocumentSummary {
                    id: document.id.clone(),
                    family_id: document.family_id.clone(),
                    version_id: document.version_id.clone(),
                    title: document.title.clone(),
                    analysis_path: None,
                    clauses: 0,
                    definitions: 0,
                    obligations: 0,
                    economic_terms: 0,
                    warnings: 0,
                    error: Some(error),
                });
            }
        }
    }
    summaries.sort_unstable_by(|left, right| {
        left.family_id
            .cmp(&right.family_id)
            .then_with(|| left.version_id.cmp(&right.version_id))
            .then_with(|| left.id.cmp(&right.id))
    });
    let summary = CollectionAnalysisSummary {
        collection_id: manifest.collection_id,
        profile,
        analyzed_documents: summaries.len() - failed,
        failed_documents: failed,
        total_clauses,
        total_definitions,
        total_obligations,
        total_economic_terms,
        clause_counts,
        warning_counts,
        documents: summaries,
    };
    atomic_write(
        &command.output.join("summary.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    atomic_write(
        &command.output.join("SUMMARY.md"),
        render_markdown(&summary).as_bytes(),
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("Analyzed documents: {}", summary.analyzed_documents);
        println!("Failed documents:   {}", summary.failed_documents);
        println!("Clauses:            {}", summary.total_clauses);
        println!("Definitions:        {}", summary.total_definitions);
        println!("Obligations:        {}", summary.total_obligations);
        println!("Economic terms:     {}", summary.total_economic_terms);
        println!("Output:             {}", command.output.display());
    }
    Ok(())
}

fn profile_from_collection(profile: fo_corpus::CollectionProfile) -> ContractProfile {
    match profile {
        fo_corpus::CollectionProfile::RetailLease => ContractProfile::RetailLease,
        fo_corpus::CollectionProfile::ProfessionalServices => ContractProfile::ProfessionalServices,
        fo_corpus::CollectionProfile::Nda => ContractProfile::Nda,
        _ => ContractProfile::General,
    }
}

fn print_analysis(path: &Path, analysis: &ContractAnalysis) {
    println!("Document:       {}", path.display());
    println!("Profile:        {:?}", analysis.profile);
    println!("Clauses:        {}", analysis.clauses.len());
    println!("Definitions:    {}", analysis.definitions.len());
    println!("Obligations:    {}", analysis.obligations.len());
    println!("Economic terms: {}", analysis.economic_terms.len());
    println!("Warnings:       {}", analysis.warnings.len());
    for clause in &analysis.clauses {
        let primary = clause
            .classifications
            .first()
            .map_or(ClauseKind::Unclassified, |classification| {
                classification.kind
            });
        println!(
            "  {:>4} {:?} [{:>8}..{:>8}] {}",
            clause.index, primary, clause.start_byte, clause.end_byte, clause.heading,
        );
    }
}

fn render_markdown(summary: &CollectionAnalysisSummary) -> String {
    let mut output = format!(
        "# Contract collection analysis\n\n- Collection: `{}`\n- Profile: `{:?}`\n- Analyzed documents: {}\n- Failed documents: {}\n- Clauses: {}\n- Definitions: {}\n- Obligations: {}\n- Economic terms: {}\n\n## Clause inventory\n\n| Clause kind | Count |\n|---|---:|\n",
        summary.collection_id,
        summary.profile,
        summary.analyzed_documents,
        summary.failed_documents,
        summary.total_clauses,
        summary.total_definitions,
        summary.total_obligations,
        summary.total_economic_terms,
    );
    for (kind, count) in &summary.clause_counts {
        output.push_str(&format!("| `{:?}` | {} |\n", kind, count));
    }
    output.push_str("\n## Documents\n\n| Family | Version | Document | Clauses | Obligations | Terms | Warnings |\n|---|---|---|---:|---:|---:|---:|\n");
    for document in &summary.documents {
        output.push_str(&format!(
            "| `{}` | `{}` | `{}` | {} | {} | {} | {} |\n",
            document.family_id,
            document.version_id,
            document.id,
            document.clauses,
            document.obligations,
            document.economic_terms,
            document.warnings,
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
