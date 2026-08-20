#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    BatchQuery, BatchSearchOptions, BatchSearchReport, BatchSearchResult, Index, SearchIntent,
    SearchOptions,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-batch",
    version,
    about = "Parallel, stable-order NDJSON search over one resident FrankenOverlap index"
)]
struct Cli {
    index: PathBuf,
    /// JSONL containing {"id":"query-1","specimen":"..."}.
    input: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: IntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    /// Zero uses Rayon's configured global worker count.
    #[arg(long, default_value_t = 0)]
    threads: usize,
    #[arg(long, default_value_t = 1_000_000)]
    maximum_queries: usize,
    #[arg(long, default_value_t = 1024 * 1024 * 1024)]
    maximum_total_specimen_bytes: usize,
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    deduplicate_identical_specimens: bool,
    #[arg(long)]
    fail_fast: bool,
    /// Emit the complete BatchSearchReport as one JSON value instead of result JSONL.
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
        eprintln!("fo-batch: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let index = Index::load_auto(&command.index)?;
    let queries = read_queries(&command.input)?;
    let report = index.search_batch(
        &queries,
        &SearchOptions {
            intent: command.intent.into(),
            max_results: command.limit,
            max_candidates: command.candidates,
            minimum_similarity: command.minimum_similarity,
            minimum_matched_tokens: command.minimum_matched_tokens,
            minimum_query_coverage: command.minimum_query_coverage,
            minimum_source_coverage: command.minimum_source_coverage,
            ..SearchOptions::default()
        },
        BatchSearchOptions {
            threads: command.threads,
            maximum_queries: command.maximum_queries,
            maximum_total_specimen_bytes: command.maximum_total_specimen_bytes,
            deduplicate_identical_specimens: command.deduplicate_identical_specimens,
            fail_fast: command.fail_fast,
        },
    )?;

    match (&command.output, command.json) {
        (Some(path), true) => atomic_write(path, &serde_json::to_vec_pretty(&report)?)?,
        (Some(path), false) => write_jsonl(path, &report.results)?,
        (None, true) => println!("{}", serde_json::to_string_pretty(&report)?),
        (None, false) => {
            let stdout = io::stdout();
            let mut writer = BufWriter::new(stdout.lock());
            write_jsonl_to(&mut writer, &report.results)?;
        }
    }
    print_summary(&report);
    if report.failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}

fn read_queries(path: &Path) -> CliResult<Vec<BatchQuery>> {
    let file = File::open(path)?;
    let mut queries = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        queries.push(serde_json::from_str::<BatchQuery>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
        })?);
    }
    if queries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} contains no batch queries", path.display()),
        )
        .into());
    }
    Ok(queries)
}

fn write_jsonl(path: &Path, values: &[BatchSearchResult]) -> CliResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let file = File::create(&temporary)?;
    let mut writer = BufWriter::new(file);
    write_jsonl_to(&mut writer, values)?;
    writer.flush()?;
    replace_file(&temporary, path)
}

fn write_jsonl_to(writer: &mut impl Write, values: &[BatchSearchResult]) -> CliResult<()> {
    for value in values {
        serde_json::to_writer(&mut *writer, value)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    fs::write(&temporary, bytes)?;
    replace_file(&temporary, path)
}

fn replace_file(temporary: &Path, destination: &Path) -> CliResult<()> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.exists() => {
            fs::remove_file(destination)?;
            fs::rename(temporary, destination)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut filename = path
        .file_name()
        .map_or_else(|| "batch".into(), |name| name.to_os_string());
    filename.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(filename)
}

fn print_summary(report: &BatchSearchReport) {
    eprintln!("queries:             {}", report.queries);
    eprintln!("unique specimens:    {}", report.unique_specimens);
    eprintln!("deduplicated queries:{}", report.deduplicated_queries);
    eprintln!("succeeded:           {}", report.succeeded);
    eprintln!("failed:              {}", report.failed);
    eprintln!("total hits:          {}", report.total_hits);
}
