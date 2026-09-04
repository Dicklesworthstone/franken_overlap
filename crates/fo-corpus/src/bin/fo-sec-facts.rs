#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use fo_corpus::{
    analyze_sec_companyfacts, atomic_write, fetch_sec_companyfacts, verify_sec_companyfacts,
    SecCompanyFacts, SecFactAnalysisOptions, SecFactsFetchOptions, SecFactsManifest,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-sec-facts",
    version,
    about = "Acquire, verify, and analyze SEC Company Facts/XBRL data"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download raw Company Facts and normalize every observation.
    Fetch(FetchCommand),
    /// Verify raw and normalized byte receipts.
    Verify(VerifyCommand),
    /// Build investor-oriented metric series, deltas, restatements, and alerts.
    Analyze(AnalyzeCommand),
}

#[derive(Debug, Args)]
struct FetchCommand {
    #[arg(short, long, default_value = "corpora/sec-companyfacts")]
    output: PathBuf,
    #[arg(long = "ticker")]
    tickers: Vec<String>,
    #[arg(long = "cik")]
    ciks: Vec<u64>,
    #[arg(long)]
    sampled_companies: Option<usize>,
    #[arg(long, default_value_t = 0x78_62_72_6c_2d_66_61_63)]
    seed: u64,
    /// Required SEC identity, e.g. "Example Research research@example.com".
    #[arg(long)]
    user_agent: Option<String>,
    #[arg(long, default_value_t = 5.0)]
    requests_per_second: f64,
    #[arg(long, default_value_t = 5)]
    maximum_attempts: usize,
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    maximum_ticker_json_bytes: u64,
    #[arg(long, default_value_t = 512 * 1024 * 1024)]
    maximum_companyfacts_bytes: u64,
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

#[derive(Debug, Args)]
struct AnalyzeCommand {
    root: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    /// Restrict analysis to one or more CIKs. Every manifest company is analyzed when omitted.
    #[arg(long = "cik")]
    ciks: Vec<u64>,
    #[arg(long = "form")]
    allowed_forms: Vec<String>,
    #[arg(long)]
    earliest_filed_date: Option<String>,
    #[arg(long, default_value_t = 256)]
    maximum_points_per_metric: usize,
    #[arg(long, default_value_t = 5)]
    minimum_points_for_anomaly: usize,
    #[arg(long, default_value_t = 0.15)]
    material_change_fraction: f64,
    #[arg(long, default_value_t = 3.5)]
    anomaly_mad_threshold: f64,
    #[arg(long, default_value_t = 0.02)]
    margin_alert_points: f64,
    #[arg(long, default_value_t = 0.03)]
    dilution_alert_fraction: f64,
    #[arg(long, default_value_t = 0.15)]
    working_capital_spread_fraction: f64,
    #[arg(long, default_value_t = 1_000)]
    maximum_alerts: usize,
    #[arg(long)]
    replace_output: bool,
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-sec-facts: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Fetch(command) => run_fetch(command),
        Command::Verify(command) => run_verify(command),
        Command::Analyze(command) => run_analyze(command),
    }
}

fn run_fetch(command: FetchCommand) -> CliResult<()> {
    let user_agent = command
        .user_agent
        .or_else(|| std::env::var("SEC_USER_AGENT").ok())
        .unwrap_or_default();
    let report = fetch_sec_companyfacts(SecFactsFetchOptions {
        output_dir: command.output,
        tickers: command.tickers,
        ciks: command.ciks,
        sampled_companies: command.sampled_companies,
        seed: command.seed,
        user_agent,
        requests_per_second: command.requests_per_second,
        maximum_attempts: command.maximum_attempts,
        maximum_ticker_json_bytes: command.maximum_ticker_json_bytes,
        maximum_companyfacts_bytes: command.maximum_companyfacts_bytes,
        overwrite: command.overwrite,
    })?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:       {}", report.manifest.corpus_id);
        println!("Companies:    {}", report.selected_companies);
        println!("Downloaded:   {}", report.downloaded);
        println!("Reused:       {}", report.reused);
        println!("Failed:       {}", report.failed);
        println!("Observations: {}", report.observations);
    }
    Ok(())
}

fn run_verify(command: VerifyCommand) -> CliResult<()> {
    let report = verify_sec_companyfacts(&command.root)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Corpus:           {}", report.corpus_id);
        println!("Companies:        {}", report.companies);
        println!("Verified:         {}", report.verified);
        println!("Raw bytes:        {}", report.raw_bytes);
        println!("Normalized bytes: {}", report.normalized_bytes);
        println!("Observations:     {}", report.observations);
    }
    Ok(())
}

fn run_analyze(command: AnalyzeCommand) -> CliResult<()> {
    verify_sec_companyfacts(&command.root)?;
    if command.output.exists() {
        if command.replace_output {
            fs::remove_dir_all(&command.output)?;
        } else {
            return Err(invalid(format!(
                "analysis output already exists: {}",
                command.output.display()
            )));
        }
    }
    fs::create_dir_all(command.output.join("companies"))?;
    let manifest = SecFactsManifest::load(&command.root)?;
    let allowed_forms = if command.allowed_forms.is_empty() {
        SecFactAnalysisOptions::default().allowed_forms
    } else {
        command.allowed_forms
    };
    let options = SecFactAnalysisOptions {
        allowed_forms,
        earliest_filed_date: command.earliest_filed_date,
        maximum_points_per_metric: command.maximum_points_per_metric,
        minimum_points_for_anomaly: command.minimum_points_for_anomaly,
        material_change_fraction: command.material_change_fraction,
        anomaly_mad_threshold: command.anomaly_mad_threshold,
        margin_alert_points: command.margin_alert_points,
        dilution_alert_fraction: command.dilution_alert_fraction,
        working_capital_spread_fraction: command.working_capital_spread_fraction,
        maximum_alerts: command.maximum_alerts,
        ..SecFactAnalysisOptions::default()
    };
    options.validate()?;
    let selected = command.ciks.into_iter().collect::<std::collections::BTreeSet<_>>();
    let mut summaries = Vec::new();
    for company in &manifest.companies {
        if !selected.is_empty() && !selected.contains(&company.cik) {
            continue;
        }
        let facts = SecCompanyFacts::load(command.root.join(&company.normalized_path))?;
        let analysis = analyze_sec_companyfacts(&facts, &options)?;
        let relative = format!("companies/CIK{:010}.json", company.cik);
        atomic_write(
            &command.output.join(&relative),
            &serde_json::to_vec_pretty(&analysis)?,
        )?;
        summaries.push(serde_json::json!({
            "cik": company.cik,
            "entity_name": company.entity_name,
            "tickers": company.tickers,
            "analysis_path": relative,
            "metrics": analysis.metric_series.len(),
            "alerts": analysis.alerts.len(),
            "restatements": analysis.metric_series.iter().map(|series| series.restatements.len()).sum::<usize>(),
            "missing_metrics": analysis.missing_metrics,
        }));
    }
    summaries.sort_unstable_by_key(|summary| summary["cik"].as_u64().unwrap_or_default());
    let summary = serde_json::json!({
        "schema_version": 1,
        "corpus_id": manifest.corpus_id,
        "companies": summaries.len(),
        "options": options,
        "company_analyses": summaries,
    });
    atomic_write(
        &command.output.join("summary.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    atomic_write(
        &command.output.join("SUMMARY.md"),
        render_summary(&summary).as_bytes(),
    )?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("Companies analyzed: {}", summaries_len(&summary));
        println!("Output:             {}", command.output.display());
    }
    Ok(())
}

fn render_summary(summary: &serde_json::Value) -> String {
    let mut output = format!(
        "# SEC Company Facts investor analysis\n\n- Corpus: `{}`\n- Companies: {}\n\n| CIK | Issuer | Metrics | Alerts | Restatements |\n|---:|---|---:|---:|---:|\n",
        summary["corpus_id"].as_str().unwrap_or_default(),
        summaries_len(summary),
    );
    if let Some(companies) = summary["company_analyses"].as_array() {
        for company in companies {
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                company["cik"].as_u64().unwrap_or_default(),
                company["entity_name"]
                    .as_str()
                    .unwrap_or_default()
                    .replace('|', "\\|"),
                company["metrics"].as_u64().unwrap_or_default(),
                company["alerts"].as_u64().unwrap_or_default(),
                company["restatements"].as_u64().unwrap_or_default(),
            ));
        }
    }
    output
}

fn summaries_len(summary: &serde_json::Value) -> usize {
    summary["company_analyses"]
        .as_array()
        .map_or(0, Vec::len)
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[allow(dead_code)]
fn _safe_relative(path: &Path) -> bool {
    !path.is_absolute()
}
