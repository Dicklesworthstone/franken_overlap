#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use fo_corpus::{
    fetch_sec_filings, verify_manifest, SecFilingsOptions, COMMENT_LETTER_FORMS,
    INVESTOR_CORE_FORMS, REGISTRATION_FORMS,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-sec-fetch",
    version,
    about = "Acquire a verified multi-form SEC EDGAR filing corpus"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download selected filing forms for explicit or sampled companies.
    Fetch(FetchCommand),
    /// Verify the manifest, byte lengths, and SHA-256 receipts.
    Verify(VerifyCommand),
}

#[derive(Debug, Args)]
struct FetchCommand {
    #[arg(short, long, default_value = "corpora/sec-filings")]
    output: PathBuf,
    #[arg(long = "ticker")]
    tickers: Vec<String>,
    #[arg(long = "cik")]
    ciks: Vec<u64>,
    #[arg(long)]
    sampled_companies: Option<usize>,
    #[arg(long, default_value_t = 0x73_65_63_2d_66_69_6c_65)]
    seed: u64,
    /// Exact EDGAR form names. Investor-core forms are used when no preset/form is supplied.
    #[arg(long = "form")]
    forms: Vec<String>,
    #[arg(long)]
    investor_core: bool,
    #[arg(long)]
    registration: bool,
    #[arg(long)]
    comment_letters: bool,
    #[arg(long, default_value_t = 40)]
    filings_per_company: usize,
    #[arg(long, default_value = "2018-01-01")]
    from_date: String,
    #[arg(long)]
    to_date: Option<String>,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    include_historical_submission_files: bool,
    #[arg(long, default_value_t = 32)]
    maximum_historical_files_per_company: usize,
    /// Required SEC identity, e.g. "Example Research research@example.com".
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long, default_value_t = 5.0)]
    requests_per_second: f64,
    #[arg(long, default_value_t = 5)]
    maximum_attempts: usize,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_json_bytes: u64,
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    maximum_document_bytes: u64,
    #[arg(long, default_value_t = 500)]
    minimum_characters: usize,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct VerifyCommand {
    root: PathBuf,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-sec-fetch: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Fetch(command) => run_fetch(command),
        Command::Verify(command) => run_verify(command),
    }
}

fn run_fetch(command: FetchCommand) -> CliResult<()> {
    let mut forms = BTreeSet::new();
    if command.investor_core {
        forms.extend(INVESTOR_CORE_FORMS.iter().map(|form| (*form).to_owned()));
    }
    if command.registration {
        forms.extend(REGISTRATION_FORMS.iter().map(|form| (*form).to_owned()));
    }
    if command.comment_letters {
        forms.extend(COMMENT_LETTER_FORMS.iter().map(|form| (*form).to_owned()));
    }
    forms.extend(command.forms);
    if forms.is_empty() {
        forms.extend(INVESTOR_CORE_FORMS.iter().map(|form| (*form).to_owned()));
    }
    let user_agent = command
        .user_agent
        .or_else(|| std::env::var("SEC_USER_AGENT").ok())
        .unwrap_or_default();
    let report = fetch_sec_filings(SecFilingsOptions {
        output_dir: command.output,
        tickers: command.tickers,
        ciks: command.ciks,
        sampled_companies: command.sampled_companies,
        seed: command.seed,
        forms: forms.into_iter().collect(),
        filings_per_company: command.filings_per_company,
        from_date: Some(command.from_date),
        to_date: command.to_date,
        include_historical_submission_files: command.include_historical_submission_files,
        maximum_historical_files_per_company: command.maximum_historical_files_per_company,
        user_agent,
        requests_per_second: command.requests_per_second,
        maximum_attempts: command.maximum_attempts,
        maximum_json_bytes: command.maximum_json_bytes,
        maximum_document_bytes: command.maximum_document_bytes,
        minimum_characters: command.minimum_characters,
        overwrite: command.overwrite,
    })?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:                    {}", report.manifest.corpus_id);
        println!("Companies:                 {}", report.companies);
        println!("Requested forms:           {}", report.requested_forms.join(", "));
        println!("Candidate filings:         {}", report.candidate_filings);
        println!("Downloaded:                {}", report.downloaded);
        println!("Reused:                    {}", report.reused);
        println!("Too short:                 {}", report.rejected_too_short);
        println!("Binary/unsupported:        {}", report.rejected_binary);
        println!(
            "Historical submission files: {}",
            report.historical_submission_files_read
        );
        println!("Failed:                    {}", report.failed);
        println!("Manifest documents:        {}", report.manifest.documents.len());
        if !report.counts_by_form.is_empty() {
            println!("Forms:");
            for (form, count) in &report.counts_by_form {
                println!("  {form:<12} {count}");
            }
        }
    }
    Ok(())
}

fn run_verify(command: VerifyCommand) -> CliResult<()> {
    let report = verify_manifest(&command.root)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:    {}", report.corpus_id);
        println!("Documents: {}", report.documents);
        println!("Verified:  {}", report.verified);
        println!("Bytes:     {}", report.total_bytes);
        println!("Status:    verified");
    }
    Ok(())
}
