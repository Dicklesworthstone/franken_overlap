#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use fo_core::{
    ActiveLearningCandidate, ActiveLearningOptions, ActiveLearningSelection,
    select_active_learning_queue,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-active",
    version,
    about = "Select uncertain, disputed, hard, and evidence-diverse examples for labeling"
)]
struct Cli {
    /// JSONL ActiveLearningCandidate records.
    input: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 200)]
    maximum_examples: usize,
    #[arg(long, default_value_t = 100_000)]
    maximum_input_candidates: usize,
    #[arg(long, default_value_t = 3)]
    maximum_per_query: usize,
    #[arg(long, default_value_t = 12)]
    maximum_per_document: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_priority: f64,
    #[arg(long, default_value_t = 0.35)]
    uncertainty_weight: f64,
    #[arg(long, default_value_t = 0.25)]
    disagreement_weight: f64,
    #[arg(long, default_value_t = 0.25)]
    hard_negative_weight: f64,
    #[arg(long, default_value_t = 0.15)]
    novelty_weight: f64,
    #[arg(long)]
    include_labeled: bool,
    /// Emit one JSON array rather than JSONL.
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-active: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let candidates = read_candidates(&command.input)?;
    let selections = select_active_learning_queue(
        &candidates,
        ActiveLearningOptions {
            maximum_examples: command.maximum_examples,
            maximum_input_candidates: command.maximum_input_candidates,
            maximum_per_query: command.maximum_per_query,
            maximum_per_document: command.maximum_per_document,
            minimum_priority: command.minimum_priority,
            uncertainty_weight: command.uncertainty_weight,
            disagreement_weight: command.disagreement_weight,
            hard_negative_weight: command.hard_negative_weight,
            novelty_weight: command.novelty_weight,
            include_labeled: command.include_labeled,
        },
    )?;
    match (&command.output, command.json) {
        (Some(path), true) => atomic_write(path, &serde_json::to_vec_pretty(&selections)?)?,
        (Some(path), false) => write_jsonl(path, &selections)?,
        (None, true) => println!("{}", serde_json::to_string_pretty(&selections)?),
        (None, false) => {
            let stdout = io::stdout();
            let mut writer = BufWriter::new(stdout.lock());
            write_jsonl_to(&mut writer, &selections)?;
        }
    }
    eprintln!(
        "selected {} of {} active-learning candidates",
        selections.len(),
        candidates.len()
    );
    Ok(())
}

fn read_candidates(path: &Path) -> CliResult<Vec<ActiveLearningCandidate>> {
    let file = File::open(path)?;
    let mut candidates = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        candidates.push(
            serde_json::from_str::<ActiveLearningCandidate>(value).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}:{}: {error}", path.display(), line_index + 1),
                )
            })?,
        );
    }
    if candidates.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} contains no active-learning candidates", path.display()),
        )
        .into());
    }
    Ok(candidates)
}

fn write_jsonl(path: &Path, values: &[ActiveLearningSelection]) -> CliResult<()> {
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

fn write_jsonl_to(writer: &mut impl Write, values: &[ActiveLearningSelection]) -> CliResult<()> {
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
        .map_or_else(|| "active-learning".into(), |name| name.to_os_string());
    filename.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(filename)
}
