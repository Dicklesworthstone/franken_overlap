use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fo_core::{
    GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore, HybridDocumentInput,
    HybridIndex, HybridIndexBuilder, HybridIndexConfig, HybridOverlapEvidence, HybridQueryMode,
    HybridSearchOptions, LexicalSearchOptions, NormalizationProfile, SearchOptions,
    grouped_evaluation_report,
};
use fo_corpus::{CorpusDocument, CorpusManifest, atomic_write, sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize};

use super::retrieval_baselines::{
    ExhaustiveAlignment, PreparedBaselines, SpanAccuracy, TokenSpan, expected_token_span,
    span_accuracy,
};

pub type ScenarioBenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const SCENARIO_BENCHMARK_SCHEMA_VERSION: u32 = 1;

const METHOD_EXACT: &str = "normalized_exact_substring";
const METHOD_JACCARD: &str = "character_qgram_jaccard";
const METHOD_SIMHASH: &str = "character_qgram_simhash";
const METHOD_LEXICAL: &str = "fielded_bm25_phrase_proximity";
const METHOD_EXHAUSTIVE: &str = "exhaustive_levenshtein";
const METHOD_OVERLAP: &str = "franken_overlap";
const METHOD_HYBRID: &str = "franken_hybrid";

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkQuery {
    pub id: String,
    pub profile: String,
    pub text: String,
    pub positive_ids: Vec<String>,
    pub source_id: String,
    #[serde(default)]
    pub source_title: String,
    #[serde(default)]
    pub relation_key: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ScenarioBenchmarkOptions {
    pub requested_corpus_sizes: Vec<usize>,
    pub maximum_documents: usize,
    pub maximum_queries: usize,
    pub profiles: BTreeSet<String>,
    pub warmup_runs: usize,
    pub measurement_repetitions: usize,
    pub qgram_size: usize,
    pub maximum_document_bytes: u64,
    pub maximum_exhaustive_cells_per_query: u64,
    pub maximum_exhaustive_cells_per_scale: u64,
    pub seed: u64,
    pub index_root: Option<PathBuf>,
    pub retain_indexes: bool,
}

impl Default for ScenarioBenchmarkOptions {
    fn default() -> Self {
        Self {
            requested_corpus_sizes: Vec::new(),
            maximum_documents: 250,
            maximum_queries: usize::MAX,
            profiles: BTreeSet::new(),
            warmup_runs: 1,
            measurement_repetitions: 3,
            qgram_size: 5,
            maximum_document_bytes: 128 * 1024 * 1024,
            maximum_exhaustive_cells_per_query: 2_000_000_000,
            maximum_exhaustive_cells_per_scale: 20_000_000_000,
            seed: 0x70_72_6f_6f_66_2d_62_65,
            index_root: None,
            retain_indexes: false,
        }
    }
}

impl ScenarioBenchmarkOptions {
    pub fn validate(&self) -> ScenarioBenchResult<()> {
        if self.maximum_documents < 2
            || self.maximum_queries == 0
            || self.measurement_repetitions == 0
            || self.qgram_size == 0
            || self.maximum_document_bytes == 0
            || self.maximum_exhaustive_cells_per_query == 0
            || self.maximum_exhaustive_cells_per_scale == 0
        {
            return Err(invalid(
                "document, query, repetition, q-gram, byte, and DP-cell limits must be positive",
            ));
        }
        if self.requested_corpus_sizes.iter().any(|&size| size < 2) {
            return Err(invalid("requested corpus sizes must be at least two"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkBuildReport {
    pub build_ms: f64,
    pub serialization_ms: f64,
    pub cold_load_ms: f64,
    pub index_bytes: u64,
    pub source_bytes: u64,
    pub overlap_fingerprints: usize,
    pub overlap_postings: usize,
    pub lexical_terms: usize,
    pub lexical_postings: usize,
    pub peak_rss_kib: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodTimingReport {
    pub first_execution_samples: usize,
    pub repeat_samples: usize,
    pub first_p50_ms: f64,
    pub first_p95_ms: f64,
    pub repeat_p50_ms: f64,
    pub repeat_p95_ms: f64,
    pub repeat_p99_ms: f64,
    pub measured_operations: usize,
    pub measured_elapsed_ms: f64,
    pub operations_per_second: f64,
    pub one_shot_total_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodProfileQuality {
    pub profile: String,
    pub queries: usize,
    pub quality: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodSpanReport {
    pub eligible_queries: usize,
    pub predicted_queries: usize,
    pub mean_iou: f64,
    pub median_iou: f64,
    pub mean_expected_coverage: f64,
    pub mean_predicted_coverage: f64,
    pub mean_start_absolute_error: f64,
    pub mean_end_absolute_error: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMethodReport {
    pub name: String,
    pub complete_quality_queries: usize,
    pub evaluated_pairs: usize,
    pub nonzero_scores: usize,
    pub quality: Option<GroupedEvaluationReport>,
    pub profiles: Vec<MethodProfileQuality>,
    pub timing: MethodTimingReport,
    pub span: Option<MethodSpanReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustiveQueryCoverage {
    pub query_id: String,
    pub profile: String,
    pub complete: bool,
    pub evaluated_documents: usize,
    pub skipped_documents: usize,
    pub cells: u64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustiveScaleCoverage {
    pub complete: bool,
    pub complete_queries: usize,
    pub partial_queries: usize,
    pub evaluated_pairs: usize,
    pub skipped_pairs: usize,
    pub cells: u64,
    pub maximum_cells_per_query: u64,
    pub maximum_cells_per_scale: u64,
    pub queries: Vec<ExhaustiveQueryCoverage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakEvenComparison {
    pub baseline_method: String,
    pub indexed_method: String,
    pub baseline_p95_ms: Option<f64>,
    pub indexed_p95_ms: f64,
    pub index_build_serialization_load_ms: f64,
    pub saved_ms_per_query_at_p95: Option<f64>,
    pub break_even_queries: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusScaleReport {
    pub corpus_size: usize,
    pub required_positive_documents: usize,
    pub candidate_ids_sha256: String,
    pub build: BenchmarkBuildReport,
    pub methods: Vec<ScenarioMethodReport>,
    pub exhaustive: ExhaustiveScaleCoverage,
    pub break_even: Vec<BreakEvenComparison>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioBenchmarkReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_provider: String,
    pub corpus_manifest_sha256: String,
    pub query_file_sha256: String,
    pub available_documents: usize,
    pub required_positive_documents: usize,
    pub queries: usize,
    pub profiles: Vec<String>,
    pub requested_corpus_sizes: Vec<usize>,
    pub skipped_corpus_sizes: Vec<usize>,
    pub evaluated_corpus_sizes: Vec<usize>,
    pub warmup_runs: usize,
    pub measurement_repetitions: usize,
    pub seed: u64,
    pub scales: Vec<CorpusScaleReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioScoreRow {
    pub corpus_size: usize,
    pub query_id: String,
    pub profile: String,
    pub query_text: String,
    pub source_id: String,
    pub source_title: String,
    pub positive_ids: Vec<String>,
    pub candidate_id: String,
    pub candidate_title: String,
    pub label: bool,
    pub scores: BTreeMap<String, f64>,
    pub expected_source_span: Option<TokenSpan>,
    pub predicted_spans: BTreeMap<String, Vec<TokenSpan>>,
    pub exhaustive_alignment: Option<ExhaustiveAlignment>,
}

#[derive(Debug, Clone)]
struct LoadedDocument {
    record: CorpusDocument,
    body: String,
}

#[derive(Debug, Clone)]
struct ScoringOutput {
    scores: Vec<f64>,
    spans: Vec<Vec<TokenSpan>>,
}

impl ScoringOutput {
    fn scores_only(scores: Vec<f64>) -> Self {
        let spans = vec![Vec::new(); scores.len()];
        Self { scores, spans }
    }
}

#[derive(Debug, Clone)]
struct MeasuredOutput {
    output: ScoringOutput,
    first_ms: f64,
    repeats_ms: Vec<f64>,
}

#[derive(Default)]
struct SpanAccumulator {
    eligible_queries: usize,
    predicted_queries: usize,
    observations: Vec<SpanAccuracy>,
}

impl SpanAccumulator {
    fn observe(&mut self, expected: Option<TokenSpan>, predicted: &[TokenSpan]) {
        let Some(expected) = expected else {
            return;
        };
        self.eligible_queries += 1;
        if let Some(accuracy) = span_accuracy(expected, predicted) {
            self.predicted_queries += 1;
            self.observations.push(accuracy);
        }
    }

    fn report(mut self) -> MethodSpanReport {
        self.observations.sort_by(|left, right| {
            left.intersection_over_union
                .total_cmp(&right.intersection_over_union)
        });
        MethodSpanReport {
            eligible_queries: self.eligible_queries,
            predicted_queries: self.predicted_queries,
            mean_iou: mean(
                self.observations
                    .iter()
                    .map(|value| value.intersection_over_union),
            ),
            median_iou: median(
                &self
                    .observations
                    .iter()
                    .map(|value| value.intersection_over_union)
                    .collect::<Vec<_>>(),
            ),
            mean_expected_coverage: mean(
                self.observations
                    .iter()
                    .map(|value| value.expected_coverage),
            ),
            mean_predicted_coverage: mean(
                self.observations
                    .iter()
                    .map(|value| value.predicted_coverage),
            ),
            mean_start_absolute_error: mean(
                self.observations
                    .iter()
                    .map(|value| value.start_absolute_error as f64),
            ),
            mean_end_absolute_error: mean(
                self.observations
                    .iter()
                    .map(|value| value.end_absolute_error as f64),
            ),
        }
    }
}

#[derive(Default)]
struct MethodAccumulator {
    examples: Vec<GroupedLabeledScore>,
    profile_examples: BTreeMap<String, Vec<GroupedLabeledScore>>,
    first_samples_ms: Vec<f64>,
    repeat_samples_ms: Vec<f64>,
    measured_elapsed: Duration,
    measured_operations: usize,
    evaluated_pairs: usize,
    nonzero_scores: usize,
    complete_quality_queries: usize,
    span: SpanAccumulator,
}

impl MethodAccumulator {
    fn observe(
        &mut self,
        query: &BenchmarkQuery,
        labels: &[bool],
        measured: &MeasuredOutput,
        expected_span: Option<TokenSpan>,
        source_index: usize,
    ) {
        self.first_samples_ms.push(measured.first_ms);
        self.repeat_samples_ms.extend(&measured.repeats_ms);
        let elapsed_ms = measured.first_ms + measured.repeats_ms.iter().sum::<f64>();
        self.measured_elapsed += Duration::from_secs_f64(elapsed_ms / 1_000.0);
        self.measured_operations += 1 + measured.repeats_ms.len();
        self.evaluated_pairs += measured.output.scores.len();
        self.nonzero_scores += measured
            .output
            .scores
            .iter()
            .filter(|&&score| score > 0.0)
            .count();
        self.complete_quality_queries += 1;
        let profile = self
            .profile_examples
            .entry(query.profile.clone())
            .or_default();
        for (&score, &label) in measured.output.scores.iter().zip(labels) {
            let example = GroupedLabeledScore {
                query_id: query.id.clone(),
                score: score.clamp(0.0, 1.0),
                label,
            };
            self.examples.push(example.clone());
            profile.push(example);
        }
        self.span.observe(
            expected_span,
            measured
                .output
                .spans
                .get(source_index)
                .map_or(&[], Vec::as_slice),
        );
    }

    fn observe_exhaustive(
        &mut self,
        query: &BenchmarkQuery,
        labels: &[bool],
        scores: &[f64],
        first_ms: f64,
        expected_span: Option<TokenSpan>,
        source_index: usize,
        alignments: &[Option<ExhaustiveAlignment>],
    ) {
        self.first_samples_ms.push(first_ms);
        self.measured_elapsed += Duration::from_secs_f64(first_ms / 1_000.0);
        self.measured_operations += 1;
        self.evaluated_pairs += scores.len();
        self.nonzero_scores += scores.iter().filter(|&&score| score > 0.0).count();
        self.complete_quality_queries += 1;
        let profile = self
            .profile_examples
            .entry(query.profile.clone())
            .or_default();
        for (&score, &label) in scores.iter().zip(labels) {
            let example = GroupedLabeledScore {
                query_id: query.id.clone(),
                score: score.clamp(0.0, 1.0),
                label,
            };
            self.examples.push(example.clone());
            profile.push(example);
        }
        let predicted = alignments
            .get(source_index)
            .and_then(|alignment| *alignment)
            .map(|alignment| vec![alignment.span()])
            .unwrap_or_default();
        self.span.observe(expected_span, &predicted);
    }

    fn report(
        mut self,
        name: &str,
        indexed: bool,
        build: &BenchmarkBuildReport,
    ) -> ScenarioBenchResult<ScenarioMethodReport> {
        self.first_samples_ms.sort_by(f64::total_cmp);
        self.repeat_samples_ms.sort_by(f64::total_cmp);
        let quality = if self.examples.is_empty() {
            None
        } else {
            Some(grouped_evaluation_report(
                &self.examples,
                evaluation_options(),
            )?)
        };
        let mut profiles = Vec::new();
        for (profile, examples) in self.profile_examples {
            let query_count = examples
                .iter()
                .map(|example| example.query_id.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            profiles.push(MethodProfileQuality {
                profile,
                queries: query_count,
                quality: grouped_evaluation_report(&examples, evaluation_options())?,
            });
        }
        profiles.sort_unstable_by(|left, right| left.profile.cmp(&right.profile));
        let repeat_p95 = if self.repeat_samples_ms.is_empty() {
            percentile(&self.first_samples_ms, 0.95)
        } else {
            percentile(&self.repeat_samples_ms, 0.95)
        };
        let setup = if indexed {
            build.build_ms + build.serialization_ms + build.cold_load_ms
        } else {
            0.0
        };
        let seconds = self.measured_elapsed.as_secs_f64();
        let timing = MethodTimingReport {
            first_execution_samples: self.first_samples_ms.len(),
            repeat_samples: self.repeat_samples_ms.len(),
            first_p50_ms: percentile(&self.first_samples_ms, 0.50),
            first_p95_ms: percentile(&self.first_samples_ms, 0.95),
            repeat_p50_ms: if self.repeat_samples_ms.is_empty() {
                percentile(&self.first_samples_ms, 0.50)
            } else {
                percentile(&self.repeat_samples_ms, 0.50)
            },
            repeat_p95_ms: repeat_p95,
            repeat_p99_ms: if self.repeat_samples_ms.is_empty() {
                percentile(&self.first_samples_ms, 0.99)
            } else {
                percentile(&self.repeat_samples_ms, 0.99)
            },
            measured_operations: self.measured_operations,
            measured_elapsed_ms: seconds * 1_000.0,
            operations_per_second: self.measured_operations as f64 / seconds.max(1.0e-12),
            one_shot_total_ms: setup + percentile(&self.first_samples_ms, 0.50),
        };
        let span = (self.span.eligible_queries > 0).then(|| self.span.report());
        Ok(ScenarioMethodReport {
            name: name.to_owned(),
            complete_quality_queries: self.complete_quality_queries,
            evaluated_pairs: self.evaluated_pairs,
            nonzero_scores: self.nonzero_scores,
            quality,
            profiles,
            timing,
            span,
        })
    }
}

pub fn read_queries(path: &Path) -> ScenarioBenchResult<Vec<BenchmarkQuery>> {
    let input = fs::read_to_string(path)?;
    let mut queries = Vec::new();
    let mut ids = BTreeSet::new();
    for (line_index, line) in input.lines().enumerate() {
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let query = serde_json::from_str::<BenchmarkQuery>(value)
            .map_err(|error| invalid(format!("{}:{}: {error}", path.display(), line_index + 1)))?;
        if query.id.trim().is_empty()
            || query.profile.trim().is_empty()
            || query.text.trim().is_empty()
            || query.source_id.trim().is_empty()
            || query.positive_ids.is_empty()
            || !ids.insert(query.id.clone())
        {
            return Err(invalid(format!(
                "query {:?} has empty required fields, no positives, or a duplicate ID",
                query.id
            )));
        }
        if !query.positive_ids.iter().any(|id| id == &query.source_id) {
            return Err(invalid(format!(
                "query {} source_id is not present in positive_ids",
                query.id
            )));
        }
        queries.push(query);
    }
    if queries.is_empty() {
        return Err(invalid(format!("{} contains no queries", path.display())));
    }
    Ok(queries)
}

pub fn run_scenario_benchmark(
    corpus_root: &Path,
    query_path: &Path,
    mut queries: Vec<BenchmarkQuery>,
    options: &ScenarioBenchmarkOptions,
) -> ScenarioBenchResult<(ScenarioBenchmarkReport, Vec<ScenarioScoreRow>)> {
    options.validate()?;
    if !options.profiles.is_empty() {
        queries.retain(|query| options.profiles.contains(&query.profile));
    }
    queries.truncate(options.maximum_queries.min(queries.len()));
    if queries.is_empty() {
        return Err(invalid("no queries remain after profile/limit filtering"));
    }
    let manifest = CorpusManifest::load(corpus_root)?;
    let manifest_bytes = fs::read(corpus_root.join(fo_corpus::MANIFEST_FILENAME))?;
    let query_bytes = fs::read(query_path)?;
    let required_ids = queries
        .iter()
        .flat_map(|query| query.positive_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let pool = load_document_pool(corpus_root, &manifest, &required_ids, options)?;
    let required_count = required_ids.len();
    let (sizes, skipped_sizes) =
        resolve_sizes(&options.requested_corpus_sizes, required_count, pool.len())?;
    let mut scales = Vec::new();
    let mut rows = Vec::new();
    for size in &sizes {
        let selected = select_scale_documents(&pool, &required_ids, *size);
        let (scale, mut scale_rows) =
            run_scale(corpus_root, &queries, selected, required_count, options)?;
        scales.push(scale);
        rows.append(&mut scale_rows);
    }
    let profiles = queries
        .iter()
        .map(|query| query.profile.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok((
        ScenarioBenchmarkReport {
            schema_version: SCENARIO_BENCHMARK_SCHEMA_VERSION,
            generated_at_unix: unix_timestamp(),
            corpus_id: manifest.corpus_id,
            corpus_provider: format!("{:?}", manifest.provider),
            corpus_manifest_sha256: sha256_hex(&manifest_bytes),
            query_file_sha256: sha256_hex(&query_bytes),
            available_documents: pool.len(),
            required_positive_documents: required_count,
            queries: queries.len(),
            profiles,
            requested_corpus_sizes: options.requested_corpus_sizes.clone(),
            skipped_corpus_sizes: skipped_sizes,
            evaluated_corpus_sizes: sizes,
            warmup_runs: options.warmup_runs,
            measurement_repetitions: options.measurement_repetitions,
            seed: options.seed,
            scales,
        },
        rows,
    ))
}

pub fn write_score_rows(path: &Path, rows: &[ScenarioScoreRow]) -> ScenarioBenchResult<()> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row)?;
        bytes.push(b'\n');
    }
    atomic_write(path, &bytes)?;
    Ok(())
}

fn run_scale(
    _corpus_root: &Path,
    queries: &[BenchmarkQuery],
    documents: Vec<LoadedDocument>,
    required_positive_documents: usize,
    options: &ScenarioBenchmarkOptions,
) -> ScenarioBenchResult<(CorpusScaleReport, Vec<ScenarioScoreRow>)> {
    let corpus_size = documents.len();
    let candidate_ids = documents
        .iter()
        .map(|document| document.record.id.as_str())
        .collect::<Vec<_>>();
    let candidate_ids_sha256 = sha256_hex(candidate_ids.join("\n").as_bytes());
    let source_bytes = documents
        .iter()
        .map(|document| document.body.len() as u64)
        .sum::<u64>();
    let bodies = documents
        .iter()
        .map(|document| document.body.clone())
        .collect::<Vec<_>>();
    let profile = NormalizationProfile::default();
    let baselines = PreparedBaselines::new(&bodies, profile.clone(), options.qgram_size)?;

    let build_started = Instant::now();
    let mut builder = HybridIndexBuilder::new(HybridIndexConfig::default())?;
    for document in &documents {
        builder.add_document(HybridDocumentInput {
            external_id: document.record.id.clone(),
            title: document.record.title.clone(),
            body: document.body.clone(),
            tags: document_tags(&document.record),
            metadata: document.record.metadata.clone(),
        })?;
    }
    let index = builder.build()?;
    let build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
    let index_directory = index_directory(options, corpus_size);
    if index_directory.exists() {
        fs::remove_dir_all(&index_directory)?;
    }
    let serialization_started = Instant::now();
    index.save(&index_directory)?;
    let serialization_ms = serialization_started.elapsed().as_secs_f64() * 1_000.0;
    let index_bytes = directory_bytes(&index_directory)?;
    let load_started = Instant::now();
    let index = HybridIndex::load(&index_directory)?;
    let cold_load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
    let stats = index.stats();
    let build = BenchmarkBuildReport {
        build_ms,
        serialization_ms,
        cold_load_ms,
        index_bytes,
        source_bytes,
        overlap_fingerprints: stats.overlap.distinct_fingerprints,
        overlap_postings: stats.overlap.postings,
        lexical_terms: stats.lexical.distinct_terms,
        lexical_postings: stats.lexical.postings,
        peak_rss_kib: peak_rss_kib(),
    };

    let document_lookup = documents
        .iter()
        .enumerate()
        .map(|(index, document)| (document.record.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut exact = MethodAccumulator::default();
    let mut jaccard = MethodAccumulator::default();
    let mut simhash = MethodAccumulator::default();
    let mut lexical = MethodAccumulator::default();
    let mut overlap = MethodAccumulator::default();
    let mut hybrid = MethodAccumulator::default();
    let mut exhaustive = MethodAccumulator::default();
    let mut rows = Vec::with_capacity(queries.len().saturating_mul(documents.len()));
    let mut exhaustive_queries = Vec::with_capacity(queries.len());
    let mut exhaustive_cells = 0u64;
    let mut exhaustive_complete_queries = 0usize;
    let mut exhaustive_partial_queries = 0usize;
    let mut exhaustive_evaluated_pairs = 0usize;
    let mut exhaustive_skipped_pairs = 0usize;

    for query in queries {
        let labels = documents
            .iter()
            .map(|document| {
                query
                    .positive_ids
                    .iter()
                    .any(|id| id == &document.record.id)
            })
            .collect::<Vec<_>>();
        let source_index = *document_lookup
            .get(query.source_id.as_str())
            .ok_or_else(|| {
                invalid(format!(
                    "query {} source {} is absent at corpus size {corpus_size}",
                    query.id, query.source_id
                ))
            })?;
        let expected_span = expected_span(query, &documents[source_index], &profile);
        let normalized_query = baselines.normalize_query(&query.text);

        let exact_measured = measure_operation(options, || {
            Ok(ScoringOutput::scores_only(
                baselines.exact_scores(&normalized_query),
            ))
        })?;
        exact.observe(query, &labels, &exact_measured, expected_span, source_index);

        let jaccard_measured = measure_operation(options, || {
            Ok(ScoringOutput::scores_only(
                baselines.jaccard_scores(&normalized_query)?,
            ))
        })?;
        jaccard.observe(
            query,
            &labels,
            &jaccard_measured,
            expected_span,
            source_index,
        );

        let simhash_measured = measure_operation(options, || {
            Ok(ScoringOutput::scores_only(
                baselines.simhash_scores(&normalized_query)?,
            ))
        })?;
        simhash.observe(
            query,
            &labels,
            &simhash_measured,
            expected_span,
            source_index,
        );

        let lexical_measured = measure_operation(options, || {
            score_lexical(&index, query, &documents, &document_lookup)
        })?;
        lexical.observe(
            query,
            &labels,
            &lexical_measured,
            expected_span,
            source_index,
        );

        let search_options = search_options(corpus_size, normalized_query.len());
        let overlap_measured = measure_operation(options, || {
            score_overlap(&index, query, &documents, &document_lookup, &search_options)
        })?;
        overlap.observe(
            query,
            &labels,
            &overlap_measured,
            expected_span,
            source_index,
        );

        let hybrid_measured = measure_operation(options, || {
            score_hybrid(&index, query, &documents, &document_lookup, &search_options)
        })?;
        hybrid.observe(
            query,
            &labels,
            &hybrid_measured,
            expected_span,
            source_index,
        );

        let remaining = options
            .maximum_exhaustive_cells_per_scale
            .saturating_sub(exhaustive_cells)
            .min(options.maximum_exhaustive_cells_per_query);
        let exhaustive_started = Instant::now();
        let exhaustive_output = baselines.exhaustive_scores(&normalized_query, remaining)?;
        let exhaustive_ms = exhaustive_started.elapsed().as_secs_f64() * 1_000.0;
        exhaustive_cells = exhaustive_cells.saturating_add(exhaustive_output.cells);
        exhaustive_evaluated_pairs =
            exhaustive_evaluated_pairs.saturating_add(exhaustive_output.evaluated_documents);
        exhaustive_skipped_pairs =
            exhaustive_skipped_pairs.saturating_add(exhaustive_output.skipped_documents);
        if exhaustive_output.complete {
            exhaustive_complete_queries += 1;
            let scores = exhaustive_output
                .scores
                .iter()
                .map(|score| score.unwrap_or(0.0))
                .collect::<Vec<_>>();
            exhaustive.observe_exhaustive(
                query,
                &labels,
                &scores,
                exhaustive_ms,
                expected_span,
                source_index,
                &exhaustive_output.alignments,
            );
        } else {
            exhaustive_partial_queries += 1;
        }
        exhaustive_queries.push(ExhaustiveQueryCoverage {
            query_id: query.id.clone(),
            profile: query.profile.clone(),
            complete: exhaustive_output.complete,
            evaluated_documents: exhaustive_output.evaluated_documents,
            skipped_documents: exhaustive_output.skipped_documents,
            cells: exhaustive_output.cells,
            elapsed_ms: exhaustive_ms,
        });

        for document_index in 0..documents.len() {
            let mut scores = BTreeMap::new();
            scores.insert(
                METHOD_EXACT.to_owned(),
                exact_measured.output.scores[document_index],
            );
            scores.insert(
                METHOD_JACCARD.to_owned(),
                jaccard_measured.output.scores[document_index],
            );
            scores.insert(
                METHOD_SIMHASH.to_owned(),
                simhash_measured.output.scores[document_index],
            );
            scores.insert(
                METHOD_LEXICAL.to_owned(),
                lexical_measured.output.scores[document_index],
            );
            scores.insert(
                METHOD_OVERLAP.to_owned(),
                overlap_measured.output.scores[document_index],
            );
            scores.insert(
                METHOD_HYBRID.to_owned(),
                hybrid_measured.output.scores[document_index],
            );
            if let Some(score) = exhaustive_output.scores[document_index] {
                scores.insert(METHOD_EXHAUSTIVE.to_owned(), score);
            }
            let mut predicted_spans = BTreeMap::new();
            if !overlap_measured.output.spans[document_index].is_empty() {
                predicted_spans.insert(
                    METHOD_OVERLAP.to_owned(),
                    overlap_measured.output.spans[document_index].clone(),
                );
            }
            if !hybrid_measured.output.spans[document_index].is_empty() {
                predicted_spans.insert(
                    METHOD_HYBRID.to_owned(),
                    hybrid_measured.output.spans[document_index].clone(),
                );
            }
            if let Some(alignment) = exhaustive_output.alignments[document_index] {
                predicted_spans.insert(METHOD_EXHAUSTIVE.to_owned(), vec![alignment.span()]);
            }
            rows.push(ScenarioScoreRow {
                corpus_size,
                query_id: query.id.clone(),
                profile: query.profile.clone(),
                query_text: query.text.clone(),
                source_id: query.source_id.clone(),
                source_title: query.source_title.clone(),
                positive_ids: query.positive_ids.clone(),
                candidate_id: documents[document_index].record.id.clone(),
                candidate_title: documents[document_index].record.title.clone(),
                label: labels[document_index],
                scores,
                expected_source_span: (document_index == source_index)
                    .then_some(expected_span)
                    .flatten(),
                predicted_spans,
                exhaustive_alignment: exhaustive_output.alignments[document_index],
            });
        }
    }

    let exact = exact.report(METHOD_EXACT, false, &build)?;
    let jaccard = jaccard.report(METHOD_JACCARD, false, &build)?;
    let simhash = simhash.report(METHOD_SIMHASH, false, &build)?;
    let lexical = lexical.report(METHOD_LEXICAL, true, &build)?;
    let exhaustive = exhaustive.report(METHOD_EXHAUSTIVE, false, &build)?;
    let overlap = overlap.report(METHOD_OVERLAP, true, &build)?;
    let hybrid = hybrid.report(METHOD_HYBRID, true, &build)?;
    let methods = vec![
        exact, jaccard, simhash, lexical, exhaustive, overlap, hybrid,
    ];
    let break_even = break_even_comparisons(&methods, &build);
    let exhaustive = ExhaustiveScaleCoverage {
        complete: exhaustive_partial_queries == 0,
        complete_queries: exhaustive_complete_queries,
        partial_queries: exhaustive_partial_queries,
        evaluated_pairs: exhaustive_evaluated_pairs,
        skipped_pairs: exhaustive_skipped_pairs,
        cells: exhaustive_cells,
        maximum_cells_per_query: options.maximum_exhaustive_cells_per_query,
        maximum_cells_per_scale: options.maximum_exhaustive_cells_per_scale,
        queries: exhaustive_queries,
    };
    if !options.retain_indexes {
        fs::remove_dir_all(&index_directory).ok();
    }
    Ok((
        CorpusScaleReport {
            corpus_size,
            required_positive_documents,
            candidate_ids_sha256,
            build,
            methods,
            exhaustive,
            break_even,
        },
        rows,
    ))
}

fn score_lexical(
    index: &HybridIndex,
    query: &BenchmarkQuery,
    documents: &[LoadedDocument],
    document_lookup: &BTreeMap<&str, usize>,
) -> ScenarioBenchResult<ScoringOutput> {
    let results = index.lexical_index().search_text(
        &query.text,
        &LexicalSearchOptions {
            max_results: documents.len(),
            max_candidate_documents: documents.len(),
            candidate_term_limit: 16,
            maximum_postings_per_term: 10_000_000,
            minimum_score: 0.0,
            ..LexicalSearchOptions::default()
        },
    )?;
    let mut scores = vec![0.0; documents.len()];
    for result in results {
        if let Some(&document_index) = document_lookup.get(result.external_id.as_str()) {
            scores[document_index] = 1.0 - (-f64::from(result.score.max(0.0)) / 4.0).exp();
        }
    }
    Ok(ScoringOutput::scores_only(scores))
}

fn score_overlap(
    index: &HybridIndex,
    query: &BenchmarkQuery,
    documents: &[LoadedDocument],
    document_lookup: &BTreeMap<&str, usize>,
    options: &SearchOptions,
) -> ScenarioBenchResult<ScoringOutput> {
    let results = index.overlap_index().search(&query.text, options)?;
    let mut output = ScoringOutput {
        scores: vec![0.0; documents.len()],
        spans: vec![Vec::new(); documents.len()],
    };
    for result in results {
        if let Some(&document_index) = document_lookup.get(result.path.as_str()) {
            output.scores[document_index] = f64::from(result.combined_score.clamp(0.0, 1.0));
            output.spans[document_index] = vec![TokenSpan {
                start: result.corpus_start,
                end: result.corpus_end,
            }];
        }
    }
    Ok(output)
}

fn score_hybrid(
    index: &HybridIndex,
    query: &BenchmarkQuery,
    documents: &[LoadedDocument],
    document_lookup: &BTreeMap<&str, usize>,
    search_options: &SearchOptions,
) -> ScenarioBenchResult<ScoringOutput> {
    let results = index.search(
        &query.text,
        &HybridSearchOptions {
            mode: HybridQueryMode::Auto,
            max_results: documents.len(),
            candidate_multiplier: 4,
            lexical: LexicalSearchOptions {
                max_results: documents.len(),
                max_candidate_documents: documents.len(),
                candidate_term_limit: 16,
                maximum_postings_per_term: 10_000_000,
                minimum_score: 0.0,
                ..LexicalSearchOptions::default()
            },
            overlap: search_options.clone(),
            overlap_candidate_floor: 0.0,
            minimum_score: 0.0,
            ..HybridSearchOptions::default()
        },
    )?;
    let mut output = ScoringOutput {
        scores: vec![0.0; documents.len()],
        spans: vec![Vec::new(); documents.len()],
    };
    for result in results.results {
        if let Some(&document_index) = document_lookup.get(result.external_id.as_str()) {
            output.scores[document_index] = f64::from(result.score.clamp(0.0, 1.0));
            output.spans[document_index] = result
                .overlap
                .as_ref()
                .map(overlap_spans)
                .unwrap_or_default();
        }
    }
    Ok(output)
}

fn overlap_spans(evidence: &HybridOverlapEvidence) -> Vec<TokenSpan> {
    match evidence {
        HybridOverlapEvidence::Passage(result) => vec![TokenSpan {
            start: result.corpus_start,
            end: result.corpus_end,
        }],
        HybridOverlapEvidence::Composite(result) => result
            .blocks
            .iter()
            .map(|block| TokenSpan {
                start: block.corpus_start,
                end: block.corpus_end,
            })
            .collect(),
    }
}

fn measure_operation<F>(
    options: &ScenarioBenchmarkOptions,
    mut operation: F,
) -> ScenarioBenchResult<MeasuredOutput>
where
    F: FnMut() -> ScenarioBenchResult<ScoringOutput>,
{
    let first_started = Instant::now();
    let output = operation()?;
    let first_ms = first_started.elapsed().as_secs_f64() * 1_000.0;
    for _ in 0..options.warmup_runs {
        let _ = operation()?;
    }
    let mut repeats_ms = Vec::with_capacity(options.measurement_repetitions);
    for _ in 0..options.measurement_repetitions {
        let started = Instant::now();
        let repeated = operation()?;
        repeats_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        debug_assert_eq!(repeated.scores.len(), output.scores.len());
    }
    Ok(MeasuredOutput {
        output,
        first_ms,
        repeats_ms,
    })
}

fn expected_span(
    query: &BenchmarkQuery,
    source: &LoadedDocument,
    profile: &NormalizationProfile,
) -> Option<TokenSpan> {
    if query.profile == "natural_relation" {
        return None;
    }
    let start = query.metadata.get("passage_start_word")?.parse().ok()?;
    let words = query.metadata.get("passage_words")?.parse().ok()?;
    expected_token_span(&source.body, start, words, profile)
}

fn search_options(corpus_size: usize, query_tokens: usize) -> SearchOptions {
    SearchOptions {
        max_results: corpus_size,
        max_candidates: corpus_size.saturating_mul(32).max(200),
        max_postings_per_feature: 10_000_000,
        minimum_matched_tokens: 8.min(query_tokens.max(1)),
        minimum_query_coverage: 0.0,
        minimum_source_coverage: 0.0,
        direct_fallback_work_limit: 500_000_000,
        short_query_candidates: corpus_size.clamp(8, 4_096),
        minimum_similarity: 0.0,
        ..SearchOptions::default()
    }
}

fn load_document_pool(
    corpus_root: &Path,
    manifest: &CorpusManifest,
    required_ids: &BTreeSet<String>,
    options: &ScenarioBenchmarkOptions,
) -> ScenarioBenchResult<Vec<LoadedDocument>> {
    let by_id = manifest
        .documents
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    for id in required_ids {
        if !by_id.contains_key(id.as_str()) {
            return Err(invalid(format!(
                "required positive document {id:?} is absent from the corpus manifest"
            )));
        }
    }
    let maximum = options
        .maximum_documents
        .max(required_ids.len())
        .min(manifest.documents.len());
    let mut selected_ids = required_ids.clone();
    let mut distractors = manifest
        .documents
        .iter()
        .filter(|record| !required_ids.contains(&record.id))
        .collect::<Vec<_>>();
    distractors.sort_unstable_by_key(|record| stable_hash(&record.id, options.seed));
    for record in distractors {
        if selected_ids.len() >= maximum {
            break;
        }
        selected_ids.insert(record.id.clone());
    }
    let mut documents = Vec::new();
    for id in selected_ids {
        let record = by_id[id.as_str()];
        validate_relative_path(&record.relative_path)?;
        if record.bytes > options.maximum_document_bytes {
            if required_ids.contains(&record.id) {
                return Err(invalid(format!(
                    "required positive document {} exceeds the byte limit",
                    record.id
                )));
            }
            continue;
        }
        let path = corpus_root.join(&record.relative_path);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) if required_ids.contains(&record.id) => return Err(error.into()),
            Err(_) => continue,
        };
        documents.push(LoadedDocument {
            record: record.clone(),
            body,
        });
    }
    let loaded_ids = documents
        .iter()
        .map(|document| document.record.id.as_str())
        .collect::<BTreeSet<_>>();
    for id in required_ids {
        if !loaded_ids.contains(id.as_str()) {
            return Err(invalid(format!(
                "required positive document {id:?} could not be loaded"
            )));
        }
    }
    documents.sort_unstable_by(|left, right| {
        let left_required = required_ids.contains(&left.record.id);
        let right_required = required_ids.contains(&right.record.id);
        right_required
            .cmp(&left_required)
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    Ok(documents)
}

fn resolve_sizes(
    requested: &[usize],
    required: usize,
    available: usize,
) -> ScenarioBenchResult<(Vec<usize>, Vec<usize>)> {
    if required > available {
        return Err(invalid(
            "required positive documents exceed the loaded pool",
        ));
    }
    let mut candidates = if requested.is_empty() {
        vec![required, 10, 25, 50, 100, 250, available]
    } else {
        requested.to_vec()
    };
    candidates.sort_unstable();
    candidates.dedup();
    let mut evaluated = Vec::new();
    let mut skipped = Vec::new();
    for size in candidates {
        if size < required || size > available {
            skipped.push(size);
        } else {
            evaluated.push(size);
        }
    }
    if evaluated.is_empty() {
        evaluated.push(available);
    }
    Ok((evaluated, skipped))
}

fn select_scale_documents(
    pool: &[LoadedDocument],
    required_ids: &BTreeSet<String>,
    size: usize,
) -> Vec<LoadedDocument> {
    let mut selected = pool
        .iter()
        .filter(|document| required_ids.contains(&document.record.id))
        .cloned()
        .collect::<Vec<_>>();
    selected.extend(
        pool.iter()
            .filter(|document| !required_ids.contains(&document.record.id))
            .take(size.saturating_sub(selected.len()))
            .cloned(),
    );
    selected.sort_unstable_by(|left, right| left.record.id.cmp(&right.record.id));
    selected
}

fn document_tags(record: &CorpusDocument) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(language) = &record.language {
        tags.push(language.clone());
    }
    for key in [
        "form",
        "tickers",
        "subjects",
        "section_title",
        "section_origin",
    ] {
        if let Some(value) = record.metadata.get(key) {
            tags.extend(
                value
                    .split([',', ';', '|'])
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    tags.sort_unstable();
    tags.dedup();
    tags
}

fn break_even_comparisons(
    methods: &[ScenarioMethodReport],
    build: &BenchmarkBuildReport,
) -> Vec<BreakEvenComparison> {
    let setup = build.build_ms + build.serialization_ms + build.cold_load_ms;
    let indexed = methods
        .iter()
        .filter(|method| {
            matches!(
                method.name.as_str(),
                METHOD_LEXICAL | METHOD_OVERLAP | METHOD_HYBRID
            )
        })
        .collect::<Vec<_>>();
    let baselines = methods
        .iter()
        .filter(|method| matches!(method.name.as_str(), METHOD_EXACT | METHOD_EXHAUSTIVE))
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for baseline in baselines {
        for method in &indexed {
            let baseline_p95 = baseline
                .quality
                .as_ref()
                .map(|_| baseline.timing.repeat_p95_ms);
            let saved = baseline_p95
                .map(|value| value - method.timing.repeat_p95_ms)
                .filter(|value| *value > 0.0);
            output.push(BreakEvenComparison {
                baseline_method: baseline.name.clone(),
                indexed_method: method.name.clone(),
                baseline_p95_ms: baseline_p95,
                indexed_p95_ms: method.timing.repeat_p95_ms,
                index_build_serialization_load_ms: setup,
                saved_ms_per_query_at_p95: saved,
                break_even_queries: saved.map(|saved| setup / saved),
            });
        }
    }
    output
}

fn evaluation_options() -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    }
}

fn index_directory(options: &ScenarioBenchmarkOptions, size: usize) -> PathBuf {
    options
        .index_root
        .clone()
        .unwrap_or_else(|| {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            std::env::temp_dir().join(format!(
                "franken-overlap-proof-index-{}-{nonce}",
                std::process::id()
            ))
        })
        .join(format!("size-{size}"))
}

fn directory_bytes(path: &Path) -> ScenarioBenchResult<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_bytes(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn peak_rss_kib() -> Option<u64> {
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        if let Some(value) = status.lines().find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        }) {
            return Some(value);
        }
    }
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn percentile(sorted: &[f64], probability: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = ((sorted.len() - 1) as f64 * probability).round() as usize;
    sorted[position.min(sorted.len() - 1)]
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    percentile(&values, 0.50)
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let mut total = 0.0;
    let mut count = 0usize;
    for value in values {
        total += value;
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

fn stable_hash(value: &str, seed: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn validate_relative_path(value: &str) -> ScenarioBenchResult<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid(format!("unsafe corpus path {value:?}")));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
