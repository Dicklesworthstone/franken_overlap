#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use fo_corpus::{
    CollectionImportOptions, CollectionManifest, CollectionProfile, import_collection,
    verify_collection,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-collection",
    version,
    about = "Import, validate, and inspect collections of related documents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Copy a directory of UTF-8 documents into a versioned fo-corpus collection.
    Import(ImportCommand),
    /// Verify collection metadata, relations, document bytes, and SHA-256 receipts.
    Verify(VerifyCommand),
    /// Inspect document families and explicit/inferred version relations.
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct ImportCommand {
    source: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    collection_id: String,
    #[arg(long, value_enum, default_value = "general")]
    profile: ProfileArg,
    /// Optional JSONL metadata keyed by source_path.
    #[arg(long)]
    metadata: Option<PathBuf>,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_document_bytes: u64,
    #[arg(long)]
    all_files: bool,
    #[arg(long)]
    replace_output: bool,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    infer_previous_versions: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct VerifyCommand {
    root: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct InspectCommand {
    root: PathBuf,
    #[arg(long)]
    family: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProfileArg {
    General,
    SecFilings,
    RetailLease,
    ProfessionalServices,
    Nda,
    Contract,
    Policy,
    SourceCode,
    Research,
}

impl From<ProfileArg> for CollectionProfile {
    fn from(value: ProfileArg) -> Self {
        match value {
            ProfileArg::General => Self::General,
            ProfileArg::SecFilings => Self::SecFilings,
            ProfileArg::RetailLease => Self::RetailLease,
            ProfileArg::ProfessionalServices => Self::ProfessionalServices,
            ProfileArg::Nda => Self::Nda,
            ProfileArg::Contract => Self::Contract,
            ProfileArg::Policy => Self::Policy,
            ProfileArg::SourceCode => Self::SourceCode,
            ProfileArg::Research => Self::Research,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-collection: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Import(command) => run_import(command),
        Command::Verify(command) => run_verify(command),
        Command::Inspect(command) => run_inspect(command),
    }
}

fn run_import(command: ImportCommand) -> CliResult<()> {
    let report = import_collection(CollectionImportOptions {
        source_dir: command.source,
        output_dir: command.output,
        collection_id: command.collection_id,
        profile: command.profile.into(),
        metadata_jsonl: command.metadata,
        maximum_document_bytes: command.maximum_document_bytes,
        all_files: command.all_files,
        replace_output: command.replace_output,
        infer_previous_versions: command.infer_previous_versions,
    })?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Collection:          {}", report.collection_id);
        println!("Profile:             {:?}", report.profile);
        println!("Documents:           {}", report.documents);
        println!("Families:            {}", report.families);
        println!("Relations:           {}", report.relations);
        println!("Total bytes:         {}", report.total_bytes);
        println!("Skipped binary:      {}", report.skipped_binary);
        println!("Skipped oversized:   {}", report.skipped_oversized);
        println!("Collection manifest: {}", report.collection_manifest);
        println!("Corpus manifest:     {}", report.corpus_manifest);
        if !report.unused_metadata_rows.is_empty() {
            println!("Unused metadata rows:");
            for path in report.unused_metadata_rows {
                println!("  {path}");
            }
        }
    }
    Ok(())
}

fn run_verify(command: VerifyCommand) -> CliResult<()> {
    let report = verify_collection(&command.root)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Collection: {}", report.collection_id);
        println!("Profile:    {:?}", report.profile);
        println!("Documents:  {}", report.documents);
        println!("Families:   {}", report.families);
        println!("Relations:  {}", report.relations);
        println!("Bytes:      {}", report.corpus.total_bytes);
        println!("Status:     verified");
    }
    Ok(())
}

fn run_inspect(command: InspectCommand) -> CliResult<()> {
    let manifest = CollectionManifest::load(&command.root)?;
    if command.json {
        if let Some(family) = command.family {
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest.family(&family))?
            );
        } else {
            println!("{}", serde_json::to_string_pretty(&manifest)?);
        }
        return Ok(());
    }
    println!("Collection: {}", manifest.collection_id);
    println!("Profile:    {:?}", manifest.profile);
    println!("Documents:  {}", manifest.documents.len());
    println!("Relations:  {}", manifest.relations.len());
    let mut families = manifest
        .documents
        .iter()
        .map(|document| document.family_id.as_str())
        .collect::<Vec<_>>();
    families.sort_unstable();
    families.dedup();
    for family in families {
        if command
            .family
            .as_deref()
            .is_some_and(|expected| expected != family)
        {
            continue;
        }
        println!("\n{family}");
        for document in manifest.family(family) {
            println!(
                "  {}  {}  {}  {}",
                document.effective_date.as_deref().unwrap_or("----------"),
                document.version_id,
                document.document_type,
                document.id,
            );
        }
    }
    if command.family.is_none() {
        println!("\nRelations:");
        for relation in &manifest.relations {
            println!(
                "  {:?}: {} -> {}",
                relation.kind, relation.from_id, relation.to_id
            );
        }
    }
    Ok(())
}
