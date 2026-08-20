#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use fo_core::{GroupedEvaluationReport, HybridFusionProfile};
use serde::{Deserialize, Serialize};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const EXPERIMENT_SCHEMA_VERSION: u32 = 1;
const REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-experiment",
    version,
    about = "Append benchmark evidence and promote only quality-gated search profiles"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract one method from a fo-real-bench report and append it to the ledger.
    Record(RecordCommand),
    /// List ledger records, optionally restricted to one corpus.
    List(ListCommand),
    /// Select the best record satisfying explicit quality and latency constraints.
    Best(BestCommand),
    /// Promote a profile-bearing record into a corpus-keyed deployment registry.
    Promote(PromoteCommand),
    /// Show a promotion registry.
    Registry(RegistryCommand),
}

#[derive(Debug, Args)]
struct RecordCommand {
    ledger: PathBuf,
    report: PathBuf,
    #[arg(long, default_value = "franken_hybrid")]
    method: String,
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    commit: Option<String>,
    #[arg(long)]
    compiler: Option<String>,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    notes: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListCommand {
    ledger: PathBuf,
    #[arg(long)]
    corpus_id: Option<String>,
    #[arg(long)]
    method: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct BestCommand {
    ledger: PathBuf,
    #[arg(long)]
    corpus_id: String,
    #[arg(long)]
    method: Option<String>,
    #[arg(long, default_value_t = 0.0)]
    minimum_micro_auprc: f64,
    #[arg(long, default_value_t = 0.0)]
    minimum_macro_auprc: f64,
    #[arg(long, default_value_t = 0.0)]
    minimum_recall_at_1: f64,
    #[arg(long)]
    maximum_p95_ms: Option<f64>,
    #[arg(long)]
    require_profile: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PromoteCommand {
    ledger: PathBuf,
    registry: PathBuf,
    #[arg(long)]
    corpus_id: String,
    #[arg(long)]
    method: Option<String>,
    #[arg(long, default_value_t = 0.0)]
    minimum_micro_auprc: f64,
    #[arg(long, default_value_t = 0.0)]
    minimum_macro_auprc: f64,
    #[arg(long, default_value_t = 0.0)]
    minimum_recall_at_1: f64,
    #[arg(long)]
    maximum_p95_ms: Option<f64>,
    #[arg(long, default_value_t = 0.0)]
    minimum_macro_delta: f64,
    #[arg(long, default_value_t = 0.0)]
    maximum_micro_regression: f64,
    #[arg(long, default_value_t = 0.0)]
    maximum_recall_at_1_regression: f64,
    #[arg(long, default_value_t = 0.10)]
    maximum_p95_regression_fraction: f64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct RegistryCommand {
    registry: PathBuf,
    #[arg(long)]
    corpus_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentMetrics {
    pub micro_auprc: f64,
    pub macro_auprc: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub false_positives_per_query: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub queries_per_second: f64,
    pub build_ms: f64,
    pub serialization_ms: f64,
    pub index_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentRecord {
    pub schema_version: u32,
    pub run_id: String,
    pub recorded_at_unix: u64,
    pub corpus_id: String,
    pub corpus_provider: String,
    pub indexed_documents: usize,
    pub source_documents: usize,
    pub queries: usize,
    pub pairs: usize,
    pub seed: u64,
    pub method: String,
    pub commit: String,
    pub compiler: Option<String>,
    pub host: Option<String>,
    pub notes: Option<String>,
    pub report_path: String,
    pub report_fingerprint: String,
    pub profile_fingerprint: Option<String>,
    pub metrics: ExperimentMetrics,
    pub profile: Option<HybridFusionProfile>,
}

impl ExperimentRecord {
    fn validate(&self) -> CliResult<()> {
        if self.schema_version != EXPERIMENT_SCHEMA_VERSION {
            return Err(invalid_input(format!(
                "unsupported experiment schema {}",
                self.schema_version
            )));
        }
        for (name, value) in [
            ("run_id", self.run_id.as_str()),
            ("corpus_id", self.corpus_id.as_str()),
            ("method", self.method.as_str()),
            ("commit", self.commit.as_str()),
            ("report_fingerprint", self.report_fingerprint.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(invalid_input(format!(
                    "experiment {name} must not be empty"
                )));
            }
        }
        validate_metrics(&self.metrics)?;
        if let Some(profile) = &self.profile {
            profile.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionEntry {
    pub promoted_at_unix: u64,
    pub run_id: String,
    pub corpus_id: String,
    pub method: String,
    pub commit: String,
    pub report_fingerprint: String,
    pub metrics: ExperimentMetrics,
    pub profile: HybridFusionProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionRegistry {
    pub schema_version: u32,
    pub updated_at_unix: u64,
    pub promotions: BTreeMap<String, PromotionEntry>,
}

impl Default for PromotionRegistry {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            updated_at_unix: unix_timestamp(),
            promotions: BTreeMap::new(),
        }
    }
}

impl PromotionRegistry {
    fn load(path: &Path) -> CliResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let registry = serde_json::from_slice::<Self>(&fs::read(path)?)?;
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> CliResult<()> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(invalid_input(format!(
                "unsupported promotion registry schema {}",
                self.schema_version
            )));
        }
        for (corpus_id, entry) in &self.promotions {
            if corpus_id != &entry.corpus_id {
                return Err(invalid_input(format!(
                    "registry key {corpus_id} disagrees with embedded corpus {}",
                    entry.corpus_id
                )));
            }
            entry.profile.validate()?;
            validate_metrics(&entry.metrics)?;
        }
        Ok(())
    }

    fn save(&self, path: &Path) -> CliResult<()> {
        self.validate()?;
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }
}

#[derive(Debug, Deserialize)]
struct RealBenchmarkInput {
    corpus_id: String,
    corpus_provider: String,
    indexed_documents: usize,
    source_documents: usize,
    queries: usize,
    pairs: usize,
    seed: u64,
    build: BuildInput,
    methods: Vec<MethodInput>,
}

#[derive(Debug, Deserialize)]
struct BuildInput {
    build_ms: f64,
    serialization_ms: f64,
    index_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct MethodInput {
    name: String,
    queries_per_second: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    false_positives_per_query_at_best_f1: f64,
    quality: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Copy)]
struct SelectionConstraints {
    minimum_micro_auprc: f64,
    minimum_macro_auprc: f64,
    minimum_recall_at_1: f64,
    maximum_p95_ms: Option<f64>,
    require_profile: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-experiment: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    match Cli::parse().command {
        Command::Record(command) => run_record(command),
        Command::List(command) => run_list(command),
        Command::Best(command) => run_best(command),
        Command::Promote(command) => run_promote(command),
        Command::Registry(command) => run_registry(command),
    }
}

fn run_record(command: RecordCommand) -> CliResult<()> {
    let report_bytes = fs::read(&command.report)?;
    let report = serde_json::from_slice::<RealBenchmarkInput>(&report_bytes)?;
    let method = report
        .methods
        .iter()
        .find(|method| method.name == command.method)
        .ok_or_else(|| {
            invalid_input(format!(
                "benchmark report contains no method named {}",
                command.method
            ))
        })?;
    let profile = command
        .profile
        .as_ref()
        .map(|path| read_profile(path))
        .transpose()?;
    let profile_fingerprint = command
        .profile
        .as_ref()
        .map(|path| fs::read(path).map(|bytes| hex_fingerprint(&bytes)))
        .transpose()?;
    let report_fingerprint = hex_fingerprint(&report_bytes);
    let recorded_at_unix = unix_timestamp();
    let run_id = command.run_id.unwrap_or_else(|| {
        format!(
            "{}-{}-{}-{:016x}",
            sanitize_id(&report.corpus_id),
            sanitize_id(&command.method),
            recorded_at_unix,
            fnv1a64(report_fingerprint.as_bytes())
        )
    });
    let record = ExperimentRecord {
        schema_version: EXPERIMENT_SCHEMA_VERSION,
        run_id,
        recorded_at_unix,
        corpus_id: report.corpus_id,
        corpus_provider: report.corpus_provider,
        indexed_documents: report.indexed_documents,
        source_documents: report.source_documents,
        queries: report.queries,
        pairs: report.pairs,
        seed: report.seed,
        method: method.name.clone(),
        commit: command
            .commit
            .or_else(|| std::env::var("GIT_COMMIT").ok())
            .unwrap_or_else(|| "unknown".to_owned()),
        compiler: command.compiler,
        host: command.host,
        notes: command.notes,
        report_path: command.report.display().to_string(),
        report_fingerprint,
        profile_fingerprint,
        metrics: metrics_from_report(&report.build, method),
        profile,
    };
    record.validate()?;
    append_record(&command.ledger, &record)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!("Recorded {}", record.run_id);
        print_record(&record);
    }
    Ok(())
}

fn run_list(command: ListCommand) -> CliResult<()> {
    let mut records = read_ledger(&command.ledger)?;
    records.retain(|record| {
        command
            .corpus_id
            .as_ref()
            .is_none_or(|corpus| &record.corpus_id == corpus)
            && command
                .method
                .as_ref()
                .is_none_or(|method| &record.method == method)
    });
    records.sort_unstable_by(|left, right| {
        left.recorded_at_unix
            .cmp(&right.recorded_at_unix)
            .then_with(|| left.run_id.cmp(&right.run_id))
    });
    if command.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else if records.is_empty() {
        println!("No experiment records matched.");
    } else {
        for record in &records {
            print_record(record);
        }
    }
    Ok(())
}

fn run_best(command: BestCommand) -> CliResult<()> {
    let constraints = selection_constraints(
        command.minimum_micro_auprc,
        command.minimum_macro_auprc,
        command.minimum_recall_at_1,
        command.maximum_p95_ms,
        command.require_profile,
    )?;
    let records = read_ledger(&command.ledger)?;
    let best = select_best(
        &records,
        &command.corpus_id,
        command.method.as_deref(),
        constraints,
    )
    .ok_or_else(|| invalid_input("no experiment satisfies the requested constraints"))?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(best)?);
    } else {
        print_record(best);
    }
    Ok(())
}

fn run_promote(command: PromoteCommand) -> CliResult<()> {
    let constraints = selection_constraints(
        command.minimum_micro_auprc,
        command.minimum_macro_auprc,
        command.minimum_recall_at_1,
        command.maximum_p95_ms,
        true,
    )?;
    validate_non_negative(
        "--maximum-micro-regression",
        command.maximum_micro_regression,
    )?;
    validate_non_negative(
        "--maximum-recall-at-1-regression",
        command.maximum_recall_at_1_regression,
    )?;
    validate_non_negative(
        "--maximum-p95-regression-fraction",
        command.maximum_p95_regression_fraction,
    )?;
    if !command.minimum_macro_delta.is_finite() {
        return Err(invalid_input("--minimum-macro-delta must be finite"));
    }

    let records = read_ledger(&command.ledger)?;
    let candidate = select_best(
        &records,
        &command.corpus_id,
        command.method.as_deref(),
        constraints,
    )
    .ok_or_else(|| invalid_input("no profile-bearing experiment satisfies the base constraints"))?;
    let profile = candidate
        .profile
        .clone()
        .ok_or_else(|| invalid_input("selected experiment has no fusion profile"))?;
    let mut registry = PromotionRegistry::load(&command.registry)?;
    if let Some(current) = registry.promotions.get(&command.corpus_id) {
        enforce_promotion_delta(
            current,
            candidate,
            command.minimum_macro_delta,
            command.maximum_micro_regression,
            command.maximum_recall_at_1_regression,
            command.maximum_p95_regression_fraction,
        )?;
    }
    let entry = PromotionEntry {
        promoted_at_unix: unix_timestamp(),
        run_id: candidate.run_id.clone(),
        corpus_id: candidate.corpus_id.clone(),
        method: candidate.method.clone(),
        commit: candidate.commit.clone(),
        report_fingerprint: candidate.report_fingerprint.clone(),
        metrics: candidate.metrics.clone(),
        profile,
    };
    registry.updated_at_unix = entry.promoted_at_unix;
    registry
        .promotions
        .insert(command.corpus_id.clone(), entry.clone());
    registry.save(&command.registry)?;
    if command.json {
        println!("{}", serde_json::to_string_pretty(&entry)?);
    } else {
        println!("Promoted {} for {}", entry.run_id, entry.corpus_id);
        print_metrics(&entry.metrics);
    }
    Ok(())
}

fn run_registry(command: RegistryCommand) -> CliResult<()> {
    let registry = PromotionRegistry::load(&command.registry)?;
    if let Some(corpus_id) = command.corpus_id {
        let entry = registry.promotions.get(&corpus_id).ok_or_else(|| {
            invalid_input(format!("registry contains no promotion for {corpus_id}"))
        })?;
        if command.json {
            println!("{}", serde_json::to_string_pretty(entry)?);
        } else {
            println!("{}", entry.corpus_id);
            println!("  run:     {}", entry.run_id);
            println!("  method:  {}", entry.method);
            println!("  commit:  {}", entry.commit);
            println!("  profile: {}", entry.profile.name);
            print_metrics(&entry.metrics);
        }
    } else if command.json {
        println!("{}", serde_json::to_string_pretty(&registry)?);
    } else if registry.promotions.is_empty() {
        println!("Promotion registry is empty.");
    } else {
        for entry in registry.promotions.values() {
            println!(
                "{} -> {} macro={:.6} micro={:.6} recall@1={:.6} p95={:.3}ms",
                entry.corpus_id,
                entry.run_id,
                entry.metrics.macro_auprc,
                entry.metrics.micro_auprc,
                entry.metrics.recall_at_1,
                entry.metrics.p95_ms,
            );
        }
    }
    Ok(())
}

fn metrics_from_report(build: &BuildInput, method: &MethodInput) -> ExperimentMetrics {
    ExperimentMetrics {
        micro_auprc: method.quality.micro.average_precision,
        macro_auprc: method.quality.macro_average_precision,
        mean_reciprocal_rank: method.quality.mean_reciprocal_rank,
        recall_at_1: recall_at(&method.quality, 1),
        recall_at_5: recall_at(&method.quality, 5),
        recall_at_10: recall_at(&method.quality, 10),
        false_positives_per_query: method.false_positives_per_query_at_best_f1,
        p50_ms: method.p50_ms,
        p95_ms: method.p95_ms,
        p99_ms: method.p99_ms,
        queries_per_second: method.queries_per_second,
        build_ms: build.build_ms,
        serialization_ms: build.serialization_ms,
        index_bytes: build.index_bytes,
    }
}

fn recall_at(report: &GroupedEvaluationReport, k: usize) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == k)
        .map_or(0.0, |metric| metric.value)
}

fn validate_metrics(metrics: &ExperimentMetrics) -> CliResult<()> {
    for (name, value) in [
        ("micro_auprc", metrics.micro_auprc),
        ("macro_auprc", metrics.macro_auprc),
        ("mean_reciprocal_rank", metrics.mean_reciprocal_rank),
        ("recall_at_1", metrics.recall_at_1),
        ("recall_at_5", metrics.recall_at_5),
        ("recall_at_10", metrics.recall_at_10),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid_input(format!(
                "experiment metric {name} must lie in [0, 1]"
            )));
        }
    }
    for (name, value) in [
        (
            "false_positives_per_query",
            metrics.false_positives_per_query,
        ),
        ("p50_ms", metrics.p50_ms),
        ("p95_ms", metrics.p95_ms),
        ("p99_ms", metrics.p99_ms),
        ("queries_per_second", metrics.queries_per_second),
        ("build_ms", metrics.build_ms),
        ("serialization_ms", metrics.serialization_ms),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(invalid_input(format!(
                "experiment metric {name} must be finite and non-negative"
            )));
        }
    }
    Ok(())
}

fn selection_constraints(
    minimum_micro_auprc: f64,
    minimum_macro_auprc: f64,
    minimum_recall_at_1: f64,
    maximum_p95_ms: Option<f64>,
    require_profile: bool,
) -> CliResult<SelectionConstraints> {
    for (name, value) in [
        ("minimum_micro_auprc", minimum_micro_auprc),
        ("minimum_macro_auprc", minimum_macro_auprc),
        ("minimum_recall_at_1", minimum_recall_at_1),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid_input(format!("{name} must lie in [0, 1]")));
        }
    }
    if maximum_p95_ms.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(invalid_input(
            "maximum_p95_ms must be finite and non-negative",
        ));
    }
    Ok(SelectionConstraints {
        minimum_micro_auprc,
        minimum_macro_auprc,
        minimum_recall_at_1,
        maximum_p95_ms,
        require_profile,
    })
}

fn select_best<'a>(
    records: &'a [ExperimentRecord],
    corpus_id: &str,
    method: Option<&str>,
    constraints: SelectionConstraints,
) -> Option<&'a ExperimentRecord> {
    records
        .iter()
        .filter(|record| record.corpus_id == corpus_id)
        .filter(|record| method.is_none_or(|method| record.method == method))
        .filter(|record| !constraints.require_profile || record.profile.is_some())
        .filter(|record| record.metrics.micro_auprc >= constraints.minimum_micro_auprc)
        .filter(|record| record.metrics.macro_auprc >= constraints.minimum_macro_auprc)
        .filter(|record| record.metrics.recall_at_1 >= constraints.minimum_recall_at_1)
        .filter(|record| {
            constraints
                .maximum_p95_ms
                .is_none_or(|maximum| record.metrics.p95_ms <= maximum)
        })
        .max_by(|left, right| {
            left.metrics
                .macro_auprc
                .total_cmp(&right.metrics.macro_auprc)
                .then_with(|| {
                    left.metrics
                        .micro_auprc
                        .total_cmp(&right.metrics.micro_auprc)
                })
                .then_with(|| {
                    left.metrics
                        .recall_at_1
                        .total_cmp(&right.metrics.recall_at_1)
                })
                .then_with(|| right.metrics.p95_ms.total_cmp(&left.metrics.p95_ms))
                .then_with(|| left.recorded_at_unix.cmp(&right.recorded_at_unix))
                .then_with(|| left.run_id.cmp(&right.run_id))
        })
}

fn enforce_promotion_delta(
    current: &PromotionEntry,
    candidate: &ExperimentRecord,
    minimum_macro_delta: f64,
    maximum_micro_regression: f64,
    maximum_recall_regression: f64,
    maximum_p95_regression_fraction: f64,
) -> CliResult<()> {
    let macro_delta = candidate.metrics.macro_auprc - current.metrics.macro_auprc;
    let micro_regression = current.metrics.micro_auprc - candidate.metrics.micro_auprc;
    let recall_regression = current.metrics.recall_at_1 - candidate.metrics.recall_at_1;
    let p95_regression = if current.metrics.p95_ms > 0.0 {
        (candidate.metrics.p95_ms - current.metrics.p95_ms) / current.metrics.p95_ms
    } else if candidate.metrics.p95_ms > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    if macro_delta < minimum_macro_delta {
        return Err(invalid_input(format!(
            "candidate macro-AUPRC delta {macro_delta:+.6} is below required {minimum_macro_delta:+.6}"
        )));
    }
    if micro_regression > maximum_micro_regression {
        return Err(invalid_input(format!(
            "candidate micro-AUPRC regression {micro_regression:.6} exceeds allowed {maximum_micro_regression:.6}"
        )));
    }
    if recall_regression > maximum_recall_regression {
        return Err(invalid_input(format!(
            "candidate Recall@1 regression {recall_regression:.6} exceeds allowed {maximum_recall_regression:.6}"
        )));
    }
    if p95_regression > maximum_p95_regression_fraction {
        return Err(invalid_input(format!(
            "candidate p95 regression fraction {p95_regression:.6} exceeds allowed {maximum_p95_regression_fraction:.6}"
        )));
    }
    Ok(())
}

fn append_record(path: &Path, record: &ExperimentRecord) -> CliResult<()> {
    let _lock = LedgerLock::acquire(path)?;
    let records = if path.exists() {
        read_ledger(path)?
    } else {
        Vec::new()
    };
    if records
        .iter()
        .any(|existing| existing.run_id == record.run_id)
    {
        return Err(invalid_input(format!(
            "ledger already contains run ID {}",
            record.run_id
        )));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, record)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_ledger(path: &Path) -> CliResult<Vec<ExperimentRecord>> {
    let file = File::open(path)?;
    let mut records = Vec::new();
    let mut run_ids = BTreeSet::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let record = serde_json::from_str::<ExperimentRecord>(value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: {error}", path.display(), line_index + 1),
            )
        })?;
        record.validate()?;
        if !run_ids.insert(record.run_id.clone()) {
            return Err(invalid_input(format!(
                "ledger contains duplicate run ID {}",
                record.run_id
            )));
        }
        records.push(record);
    }
    Ok(records)
}

struct LedgerLock {
    path: PathBuf,
}

impl LedgerLock {
    fn acquire(ledger: &Path) -> CliResult<Self> {
        let lock_path = ledger.with_extension(format!(
            "{}.lock",
            ledger
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("jsonl")
        ));
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "could not acquire experiment-ledger lock {}: {error}",
                        lock_path.display()
                    ),
                )
            })?;
        Ok(Self { path: lock_path })
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

fn read_profile(path: &Path) -> CliResult<HybridFusionProfile> {
    let profile = serde_json::from_slice::<HybridFusionProfile>(&fs::read(path)?)?;
    profile.validate()?;
    Ok(profile)
}

fn print_record(record: &ExperimentRecord) {
    println!(
        "{} corpus={} method={} commit={} profile={}",
        record.run_id,
        record.corpus_id,
        record.method,
        record.commit,
        record
            .profile
            .as_ref()
            .map_or("none", |profile| profile.name.as_str())
    );
    print_metrics(&record.metrics);
}

fn print_metrics(metrics: &ExperimentMetrics) {
    println!(
        "  macro={:.6} micro={:.6} recall@1={:.6} mrr={:.6} p95={:.3}ms qps={:.3}",
        metrics.macro_auprc,
        metrics.micro_auprc,
        metrics.recall_at_1,
        metrics.mean_reciprocal_rank,
        metrics.p95_ms,
        metrics.queries_per_second,
    );
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
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "registry".into(), |name| name.to_os_string());
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn validate_non_negative(name: &str, value: f64) -> CliResult<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(invalid_input(format!(
            "{name} must be finite and non-negative"
        )));
    }
    Ok(())
}

fn sanitize_id(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
        } else if !output.ends_with('-') && !output.is_empty() {
            output.push('-');
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "run".to_owned()
    } else {
        output
    }
}

fn hex_fingerprint(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::{
        ExperimentMetrics, ExperimentRecord, PromotionEntry, SelectionConstraints,
        enforce_promotion_delta, select_best,
    };
    use fo_core::HybridFusionProfile;

    #[test]
    fn best_record_prefers_macro_quality_then_latency() {
        let records = vec![
            record("a", 0.80, 0.85, 0.75, 20.0, false),
            record("b", 0.82, 0.83, 0.74, 12.0, true),
            record("c", 0.82, 0.84, 0.76, 18.0, true),
        ];
        let selected = select_best(
            &records,
            "corpus",
            None,
            SelectionConstraints {
                minimum_micro_auprc: 0.0,
                minimum_macro_auprc: 0.0,
                minimum_recall_at_1: 0.0,
                maximum_p95_ms: None,
                require_profile: true,
            },
        )
        .expect("selected");
        assert_eq!(selected.run_id, "c");
    }

    #[test]
    fn promotion_rejects_unacceptable_latency_regression() {
        let current_record = record("current", 0.80, 0.80, 0.80, 10.0, true);
        let current = PromotionEntry {
            promoted_at_unix: 0,
            run_id: current_record.run_id.clone(),
            corpus_id: current_record.corpus_id.clone(),
            method: current_record.method.clone(),
            commit: current_record.commit.clone(),
            report_fingerprint: current_record.report_fingerprint.clone(),
            metrics: current_record.metrics.clone(),
            profile: HybridFusionProfile::default(),
        };
        let candidate = record("candidate", 0.82, 0.82, 0.82, 15.0, true);
        assert!(enforce_promotion_delta(&current, &candidate, 0.0, 0.0, 0.0, 0.10).is_err());
    }

    fn record(
        id: &str,
        macro_auprc: f64,
        micro_auprc: f64,
        recall_at_1: f64,
        p95_ms: f64,
        with_profile: bool,
    ) -> ExperimentRecord {
        ExperimentRecord {
            schema_version: 1,
            run_id: id.to_owned(),
            recorded_at_unix: 0,
            corpus_id: "corpus".to_owned(),
            corpus_provider: "fixture".to_owned(),
            indexed_documents: 10,
            source_documents: 5,
            queries: 20,
            pairs: 200,
            seed: 7,
            method: "franken_hybrid".to_owned(),
            commit: "commit".to_owned(),
            compiler: None,
            host: None,
            notes: None,
            report_path: "report.json".to_owned(),
            report_fingerprint: id.to_owned(),
            profile_fingerprint: with_profile.then(|| id.to_owned()),
            metrics: ExperimentMetrics {
                micro_auprc,
                macro_auprc,
                mean_reciprocal_rank: recall_at_1,
                recall_at_1,
                recall_at_5: 1.0,
                recall_at_10: 1.0,
                false_positives_per_query: 0.0,
                p50_ms: p95_ms / 2.0,
                p95_ms,
                p99_ms: p95_ms * 1.2,
                queries_per_second: 100.0,
                build_ms: 1.0,
                serialization_ms: 1.0,
                index_bytes: 100,
            },
            profile: with_profile.then(HybridFusionProfile::default),
        }
    }
}
