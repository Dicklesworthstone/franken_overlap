#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use fo_core::{
    GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore, HybridDocumentInput,
    HybridIndexBuilder, HybridIndexConfig, HybridQueryMode, HybridSearchOptions,
    LexicalSearchOptions, NormalizationProfile, SearchOptions, grouped_evaluation_report, normalize,
};
use fo_corpus::{CorpusManifest, atomic_write, unix_timestamp};
use serde::{Deserialize, Serialize};

type BenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "fo-exhaustive-bench",
    version,
    about = "Compare bounded exhaustive semi-global Levenshtein retrieval with indexed FrankenOverlap search"
)]
struct Cli {
    /// A fo-corpus root containing manifest.json and the referenced documents.
    corpus_root: PathBuf,
    /// JSONL query specifications with explicit positive document IDs.
    queries: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Optional per-query/document score JSONL.
    #[arg(long)]
    scores_output: Option<PathBuf>,
    #[arg(long, default_value_t = 250)]
    maximum_documents: usize,
    /// Maximum dynamic-programming cells evaluated for one query.
    #[arg(long, default_value_t = 2_000_000_000)]
    maximum_cells_per_query: u64,
    /// Maximum dynamic-programming cells evaluated across the complete run.
    #[arg(long, default_value_t = 20_000_000_000)]
    maximum_total_cells: u64,
    #[arg(long, default_value_t = 4096)]
    maximum_query_tokens: usize,
    #[arg(long, default_value_t = 4_000_000)]
    maximum_document_tokens: usize,
    /// Fail instead of reporting partial exhaustive coverage when a work budget is reached.
    #[arg(long)]
    require_complete_exhaustive: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySpec {
    id: String,
    #[serde(default = "default_profile")]
    profile: String,
    text: String,
    positive_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct BenchmarkDocument {
    external_id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    cost: u32,
    start: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ExhaustiveAlignment {
    distance: usize,
    similarity: f64,
    text_start: usize,
    text_end: usize,
    cells: u64,
}

#[derive(Debug, Clone, Serialize)]
struct PairScoreRow {
    query_id: String,
    profile: String,
    query_text: String,
    source_ids: Vec<String>,
    candidate_id: String,
    candidate_title: String,
    label: bool,
    scores: BTreeMap<String, f64>,
    exhaustive: Option<ExhaustiveAlignment>,
}

#[derive(Debug, Clone, Serialize)]
struct QueryExhaustiveReport {
    query_id: String,
    profile: String,
    query_tokens: usize,
    evaluated_documents: usize,
    skipped_documents: usize,
    cells: u64,
    elapsed_ms: f64,
    complete: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ExhaustiveCoverageReport {
    complete: bool,
    complete_queries: usize,
    partial_queries: usize,
    evaluated_pairs: usize,
    skipped_pairs: usize,
    cells: u64,
    maximum_cells_per_query: u64,
    maximum_total_cells: u64,
    query_reports: Vec<QueryExhaustiveReport>,
}

#[derive(Debug, Clone, Serialize)]
struct MethodReport {
    name: String,
    complete_queries: usize,
    evaluated_pairs: usize,
    elapsed_ms: f64,
    queries_per_second: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    quality: Option<GroupedEvaluationReport>,
}

#[derive(Debug, Clone, Serialize)]
struct BuildReport {
    build_ms: f64,
    documents: usize,
    overlap_fingerprints: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
}

#[derive(Debug, Clone, Serialize)]
struct BreakEvenReport {
    indexed_method: String,
    exhaustive_complete_queries: usize,
    exhaustive_p95_ms: Option<f64>,
    indexed_p95_ms: f64,
    build_ms: f64,
    saved_ms_per_query_at_p95: Option<f64>,
    break_even_queries: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    generated_at_unix: u64,
    corpus_id: String,
    indexed_documents: usize,
    queries: usize,
    pairs: usize,
    build: BuildReport,
    exhaustive_coverage: ExhaustiveCoverageReport,
    methods: Vec<MethodReport>,
    break_even: BreakEvenReport,
}

#[derive(Default)]
struct MethodAccumulator {
    examples: Vec<GroupedLabeledScore>,
    latencies_ms: Vec<f64>,
    elapsed: Duration,
    complete_queries: usize,
    evaluated_pairs: usize,
}

impl MethodAccumulator {
    fn observe(
        &mut self,
        query: &QuerySpec,
        scores: &[f64],
        labels: &[bool],
        elapsed: Duration,
    ) {
        self.elapsed += elapsed;
        self.latencies_ms.push(elapsed.as_secs_f64() * 1_000.0);
        self.complete_queries += 1;
        self.evaluated_pairs += scores.len();
        for (&score, &label) in scores.iter().zip(labels) {
            self.examples.push(GroupedLabeledScore {
                query_id: query.id.clone(),
                score: score.clamp(0.0, 1.0),
                label,
            });
        }
    }

    fn report(mut self, name: &str) -> BenchResult<MethodReport> {
        self.latencies_ms.sort_by(f64::total_cmp);
        let quality = if self.examples.is_empty() {
            None
        } else {
            Some(grouped_evaluation_report(
                &self.examples,
                evaluation_options(),
            )?)
        };
        let seconds = self.elapsed.as_secs_f64();
        Ok(MethodReport {
            name: name.to_owned(),
            complete_queries: self.complete_queries,
            evaluated_pairs: self.evaluated_pairs,
            elapsed_ms: seconds * 1_000.0,
            queries_per_second: self.complete_queries as f64 / seconds.max(1.0e-12),
            p50_ms: percentile(&self.latencies_ms, 0.50),
            p95_ms: percentile(&self.latencies_ms, 0.95),
            p99_ms: percentile(&self.latencies_ms, 0.99),
            quality,
        })
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-exhaustive-bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> BenchResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    let manifest = CorpusManifest::load(&command.corpus_root)?;
    let queries = read_queries(&command.queries)?;
    let documents = load_documents(
        &command.corpus_root,
        &manifest,
        command.maximum_documents,
        command.maximum_document_tokens,
    )?;
    validate_queries(&queries, &documents, command.maximum_query_tokens)?;

    let build_started = Instant::now();
    let mut builder = HybridIndexBuilder::new(HybridIndexConfig::default())?;
    for document in &documents {
        builder.add_document(HybridDocumentInput {
            external_id: document.external_id.clone(),
            title: document.title.clone(),
            body: document.body.clone(),
            tags: document.tags.clone(),
            metadata: document.metadata.clone(),
        })?;
    }
    let index = builder.build()?;
    let build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
    let stats = index.stats();
    let build = BuildReport {
        build_ms,
        documents: documents.len(),
        overlap_fingerprints: stats.overlap.distinct_fingerprints,
        overlap_postings: stats.overlap.postings,
        lexical_terms: stats.lexical.distinct_terms,
        lexical_postings: stats.lexical.postings,
    };

    let normalization = NormalizationProfile::default();
    let normalized_documents = documents
        .iter()
        .map(|document| normalize(&document.body, &normalization))
        .collect::<Vec<_>>();
    let document_lookup = documents
        .iter()
        .enumerate()
        .map(|(index, document)| (document.external_id.as_str(), index))
        .collect::<BTreeMap<_, _>>();

    let mut exact_accumulator = MethodAccumulator::default();
    let mut exhaustive_accumulator = MethodAccumulator::default();
    let mut lexical_accumulator = MethodAccumulator::default();
    let mut overlap_accumulator = MethodAccumulator::default();
    let mut hybrid_accumulator = MethodAccumulator::default();
    let mut rows = Vec::with_capacity(queries.len().saturating_mul(documents.len()));
    let mut query_reports = Vec::with_capacity(queries.len());
    let mut total_cells = 0u64;
    let mut complete_queries = 0usize;
    let mut partial_queries = 0usize;
    let mut evaluated_pairs = 0usize;
    let mut skipped_pairs = 0usize;

    for query in &queries {
        let labels = documents
            .iter()
            .map(|document| query.positive_ids.iter().any(|id| id == &document.external_id))
            .collect::<Vec<_>>();
        let normalized_query = normalize(&query.text, &normalization);

        let exact_started = Instant::now();
        let exact_scores = normalized_documents
            .iter()
            .map(|document| {
                if document.text.contains(&normalized_query.text) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        exact_accumulator.observe(query, &exact_scores, &labels, exact_started.elapsed());

        let lexical_started = Instant::now();
        let lexical_results = index.lexical_index().search_text(
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
        let mut lexical_scores = vec![0.0; documents.len()];
        for result in lexical_results {
            if let Some(&document_index) = document_lookup.get(result.external_id.as_str()) {
                lexical_scores[document_index] =
                    1.0 - (-f64::from(result.score.max(0.0)) / 4.0).exp();
            }
        }
        lexical_accumulator.observe(query, &lexical_scores, &labels, lexical_started.elapsed());

        let search_options = SearchOptions {
            max_results: documents.len(),
            max_candidates: documents.len().saturating_mul(32).max(200),
            max_postings_per_feature: 10_000_000,
            minimum_matched_tokens: 8.min(normalized_query.len().max(1)),
            minimum_query_coverage: 0.0,
            minimum_source_coverage: 0.0,
            direct_fallback_work_limit: 500_000_000,
            short_query_candidates: documents.len().min(4_096).max(8),
            minimum_similarity: 0.0,
            ..SearchOptions::default()
        };
        let overlap_started = Instant::now();
        let overlap_results = index.overlap_index().search(&query.text, &search_options)?;
        let mut overlap_scores = vec![0.0; documents.len()];
        for result in overlap_results {
            if let Some(&document_index) = document_lookup.get(result.path.as_str()) {
                overlap_scores[document_index] = f64::from(result.combined_score.clamp(0.0, 1.0));
            }
        }
        overlap_accumulator.observe(query, &overlap_scores, &labels, overlap_started.elapsed());

        let hybrid_started = Instant::now();
        let hybrid_results = index.search(
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
                overlap: search_options,
                overlap_candidate_floor: 0.0,
                minimum_score: 0.0,
                ..HybridSearchOptions::default()
            },
        )?;
        let mut hybrid_scores = vec![0.0; documents.len()];
        for result in hybrid_results.results {
            if let Some(&document_index) = document_lookup.get(result.external_id.as_str()) {
                hybrid_scores[document_index] = f64::from(result.score.clamp(0.0, 1.0));
            }
        }
        hybrid_accumulator.observe(query, &hybrid_scores, &labels, hybrid_started.elapsed());

        let exhaustive_started = Instant::now();
        let mut exhaustive_scores = vec![None; documents.len()];
        let mut query_cells = 0u64;
        let mut query_evaluated = 0usize;
        let mut query_skipped = 0usize;
        for (document_index, document) in normalized_documents.iter().enumerate() {
            let cells = checked_cells(normalized_query.len(), document.len())?;
            let fits_query = query_cells.saturating_add(cells) <= command.maximum_cells_per_query;
            let fits_total = total_cells.saturating_add(cells) <= command.maximum_total_cells;
            if !fits_query || !fits_total {
                query_skipped += 1;
                skipped_pairs += 1;
                continue;
            }
            let alignment = exhaustive_semi_global(&normalized_query.tokens, &document.tokens)?;
            debug_assert_eq!(alignment.cells, cells);
            query_cells = query_cells.saturating_add(cells);
            total_cells = total_cells.saturating_add(cells);
            query_evaluated += 1;
            evaluated_pairs += 1;
            exhaustive_scores[document_index] = Some(alignment);
        }
        let exhaustive_elapsed = exhaustive_started.elapsed();
        let query_complete = query_skipped == 0;
        if query_complete {
            complete_queries += 1;
            exhaustive_accumulator.observe(
                query,
                &exhaustive_scores
                    .iter()
                    .map(|alignment| alignment.map_or(0.0, |alignment| alignment.similarity))
                    .collect::<Vec<_>>(),
                &labels,
                exhaustive_elapsed,
            );
        } else {
            partial_queries += 1;
        }
        query_reports.push(QueryExhaustiveReport {
            query_id: query.id.clone(),
            profile: query.profile.clone(),
            query_tokens: normalized_query.len(),
            evaluated_documents: query_evaluated,
            skipped_documents: query_skipped,
            cells: query_cells,
            elapsed_ms: exhaustive_elapsed.as_secs_f64() * 1_000.0,
            complete: query_complete,
        });

        for document_index in 0..documents.len() {
            let mut scores = BTreeMap::new();
            scores.insert(
                "normalized_exact_substring".to_owned(),
                exact_scores[document_index],
            );
            scores.insert(
                "fielded_bm25_phrase_proximity".to_owned(),
                lexical_scores[document_index],
            );
            scores.insert("franken_overlap".to_owned(), overlap_scores[document_index]);
            scores.insert("franken_hybrid".to_owned(), hybrid_scores[document_index]);
            if let Some(alignment) = exhaustive_scores[document_index] {
                scores.insert("exhaustive_levenshtein".to_owned(), alignment.similarity);
            }
            rows.push(PairScoreRow {
                query_id: query.id.clone(),
                profile: query.profile.clone(),
                query_text: query.text.clone(),
                source_ids: query.positive_ids.clone(),
                candidate_id: documents[document_index].external_id.clone(),
                candidate_title: documents[document_index].title.clone(),
                label: labels[document_index],
                scores,
                exhaustive: exhaustive_scores[document_index],
            });
        }
    }

    let exhaustive_complete = partial_queries == 0;
    if command.require_complete_exhaustive && !exhaustive_complete {
        return Err(invalid(format!(
            "exhaustive baseline was partial: {partial_queries} of {} queries exceeded a cell budget",
            queries.len()
        )));
    }

    let exact_report = exact_accumulator.report("normalized_exact_substring")?;
    let exhaustive_report = exhaustive_accumulator.report("exhaustive_levenshtein")?;
    let lexical_report = lexical_accumulator.report("fielded_bm25_phrase_proximity")?;
    let overlap_report = overlap_accumulator.report("franken_overlap")?;
    let hybrid_report = hybrid_accumulator.report("franken_hybrid")?;
    let break_even = break_even_report(build_ms, &exhaustive_report, &hybrid_report);
    let report = BenchmarkReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id,
        indexed_documents: documents.len(),
        queries: queries.len(),
        pairs: queries.len().saturating_mul(documents.len()),
        build,
        exhaustive_coverage: ExhaustiveCoverageReport {
            complete: exhaustive_complete,
            complete_queries,
            partial_queries,
            evaluated_pairs,
            skipped_pairs,
            cells: total_cells,
            maximum_cells_per_query: command.maximum_cells_per_query,
            maximum_total_cells: command.maximum_total_cells,
            query_reports,
        },
        methods: vec![
            exact_report,
            exhaustive_report,
            lexical_report,
            overlap_report,
            hybrid_report,
        ],
        break_even,
    };

    if let Some(path) = &command.output {
        atomic_write(path, &serde_json::to_vec_pretty(&report)?)?;
    }
    if let Some(path) = &command.scores_output {
        write_jsonl(path, &rows)?;
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn validate_command(command: &Cli) -> BenchResult<()> {
    if command.maximum_documents < 2
        || command.maximum_cells_per_query == 0
        || command.maximum_total_cells == 0
        || command.maximum_query_tokens == 0
        || command.maximum_document_tokens == 0
    {
        return Err(invalid("document, token, and work limits must be positive"));
    }
    Ok(())
}

fn read_queries(path: &Path) -> BenchResult<Vec<QuerySpec>> {
    let file = File::open(path)?;
    let mut queries = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        queries.push(serde_json::from_str::<QuerySpec>(value).map_err(|error| {
            invalid(format!("{}:{}: {error}", path.display(), line_index + 1))
        })?);
    }
    if queries.is_empty() {
        return Err(invalid(format!("{} contains no queries", path.display())));
    }
    let mut ids = BTreeSet::new();
    for query in &queries {
        if query.id.trim().is_empty()
            || query.text.trim().is_empty()
            || query.positive_ids.is_empty()
            || !ids.insert(query.id.as_str())
        {
            return Err(invalid(format!(
                "query {:?} has an empty/duplicate ID, empty text, or no positives",
                query.id
            )));
        }
    }
    Ok(queries)
}

fn load_documents(
    root: &Path,
    manifest: &CorpusManifest,
    maximum_documents: usize,
    maximum_document_tokens: usize,
) -> BenchResult<Vec<BenchmarkDocument>> {
    let mut records = manifest.documents.iter().collect::<Vec<_>>();
    records.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    let mut documents = Vec::new();
    for record in records {
        if documents.len() >= maximum_documents {
            break;
        }
        validate_relative_path(&record.relative_path)?;
        let path = root.join(&record.relative_path);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(_) => continue,
        };
        if normalize(&body, &NormalizationProfile::default()).len() > maximum_document_tokens {
            continue;
        }
        let mut tags = Vec::new();
        if let Some(language) = &record.language {
            tags.push(language.clone());
        }
        for key in ["form", "tickers", "subjects", "section_title", "section_origin"] {
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
        let mut metadata = record.metadata.clone();
        metadata.insert("source_url".to_owned(), record.source_url.clone());
        if let Some(date) = &record.published_or_filed {
            metadata.insert("date".to_owned(), date.clone());
        }
        documents.push(BenchmarkDocument {
            external_id: record.id.clone(),
            title: record.title.clone(),
            body,
            tags,
            metadata,
        });
    }
    if documents.len() < 2 {
        return Err(invalid("fewer than two eligible documents were loaded"));
    }
    Ok(documents)
}

fn validate_queries(
    queries: &[QuerySpec],
    documents: &[BenchmarkDocument],
    maximum_query_tokens: usize,
) -> BenchResult<()> {
    let document_ids = documents
        .iter()
        .map(|document| document.external_id.as_str())
        .collect::<BTreeSet<_>>();
    for query in queries {
        let tokens = normalize(&query.text, &NormalizationProfile::default()).len();
        if tokens == 0 || tokens > maximum_query_tokens {
            return Err(invalid(format!(
                "query {} has {tokens} normalized tokens; expected 1..={maximum_query_tokens}",
                query.id
            )));
        }
        for positive in &query.positive_ids {
            if !document_ids.contains(positive.as_str()) {
                return Err(invalid(format!(
                    "query {} references positive document {positive:?}, which is absent from the loaded candidate set",
                    query.id
                )));
            }
        }
    }
    Ok(())
}

fn exhaustive_semi_global(pattern: &[u32], text: &[u32]) -> BenchResult<ExhaustiveAlignment> {
    if pattern.is_empty() {
        return Err(invalid("exhaustive alignment pattern must not be empty"));
    }
    if text.len() > u32::MAX as usize {
        return Err(invalid("exhaustive alignment text exceeds u32 coordinates"));
    }
    let cells = checked_cells(pattern.len(), text.len())?;
    let mut previous = (0..=text.len())
        .map(|position| Cell {
            cost: 0,
            start: position as u32,
        })
        .collect::<Vec<_>>();
    let mut current = vec![Cell { cost: 0, start: 0 }; text.len() + 1];

    for (pattern_index, &pattern_token) in pattern.iter().enumerate() {
        current[0] = Cell {
            cost: u32::try_from(pattern_index + 1)
                .map_err(|_| invalid("query length exceeds u32 edit distance"))?,
            start: 0,
        };
        for (text_index, &text_token) in text.iter().enumerate() {
            let column = text_index + 1;
            let diagonal = Cell {
                cost: previous[column - 1]
                    .cost
                    .saturating_add(u32::from(pattern_token != text_token)),
                start: previous[column - 1].start,
            };
            let delete_pattern = Cell {
                cost: previous[column].cost.saturating_add(1),
                start: previous[column].start,
            };
            let insert_text = Cell {
                cost: current[column - 1].cost.saturating_add(1),
                start: current[column - 1].start,
            };
            current[column] = better_transition(
                better_transition(diagonal, delete_pattern, column),
                insert_text,
                column,
            );
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let mut best = previous[0];
    let mut best_end = 0usize;
    for (end, &cell) in previous.iter().enumerate().skip(1) {
        if better_final(cell, end, best, best_end) {
            best = cell;
            best_end = end;
        }
    }
    let text_start = best.start as usize;
    let text_end = best_end.max(text_start);
    let denominator = pattern.len().max(text_end.saturating_sub(text_start)).max(1);
    let similarity = (1.0 - f64::from(best.cost) / denominator as f64).clamp(0.0, 1.0);
    Ok(ExhaustiveAlignment {
        distance: best.cost as usize,
        similarity,
        text_start,
        text_end,
        cells,
    })
}

fn better_transition(left: Cell, right: Cell, end: usize) -> Cell {
    if left.cost < right.cost
        || left.cost == right.cost
            && (span_length(left, end) > span_length(right, end)
                || span_length(left, end) == span_length(right, end)
                    && left.start < right.start)
    {
        left
    } else {
        right
    }
}

fn better_final(candidate: Cell, candidate_end: usize, best: Cell, best_end: usize) -> bool {
    candidate.cost < best.cost
        || candidate.cost == best.cost
            && (span_length(candidate, candidate_end) > span_length(best, best_end)
                || span_length(candidate, candidate_end) == span_length(best, best_end)
                    && candidate.start < best.start)
}

fn span_length(cell: Cell, end: usize) -> usize {
    end.saturating_sub(cell.start as usize)
}

fn checked_cells(pattern: usize, text: usize) -> BenchResult<u64> {
    let cells = (pattern as u128).saturating_mul(text as u128);
    u64::try_from(cells).map_err(|_| invalid("dynamic-programming cell count exceeds u64"))
}

fn break_even_report(
    build_ms: f64,
    exhaustive: &MethodReport,
    indexed: &MethodReport,
) -> BreakEvenReport {
    let exhaustive_p95_ms = (exhaustive.complete_queries > 0).then_some(exhaustive.p95_ms);
    let saved = exhaustive_p95_ms
        .map(|baseline| baseline - indexed.p95_ms)
        .filter(|value| *value > 0.0);
    BreakEvenReport {
        indexed_method: indexed.name.clone(),
        exhaustive_complete_queries: exhaustive.complete_queries,
        exhaustive_p95_ms,
        indexed_p95_ms: indexed.p95_ms,
        build_ms,
        saved_ms_per_query_at_p95: saved,
        break_even_queries: saved.map(|saved| build_ms / saved),
    }
}

fn evaluation_options() -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    }
}

fn percentile(sorted: &[f64], probability: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = ((sorted.len() - 1) as f64 * probability).round() as usize;
    sorted[position.min(sorted.len() - 1)]
}

fn write_jsonl(path: &Path, rows: &[PairScoreRow]) -> BenchResult<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let file = File::create(&temporary)?;
    let mut writer = BufWriter::new(file);
    for row in rows {
        serde_json::to_writer(&mut writer, row)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    replace_file(&temporary, path)?;
    Ok(())
}

fn print_report(report: &BenchmarkReport) {
    println!("Corpus:                  {}", report.corpus_id);
    println!(
        "Documents / queries:     {} / {}",
        report.indexed_documents, report.queries
    );
    println!("Index build ms:          {:.3}", report.build.build_ms);
    println!(
        "Exhaustive complete:     {}",
        report.exhaustive_coverage.complete
    );
    println!(
        "Exhaustive queries:      {} complete, {} partial",
        report.exhaustive_coverage.complete_queries,
        report.exhaustive_coverage.partial_queries
    );
    println!(
        "Exhaustive DP cells:     {}",
        report.exhaustive_coverage.cells
    );
    for method in &report.methods {
        let quality = method
            .quality
            .as_ref()
            .map(|quality| {
                format!(
                    "micro={:.6} macro={:.6} r@1={:.6}",
                    quality.micro.average_precision,
                    quality.macro_average_precision,
                    quality
                        .recall_at_k
                        .iter()
                        .find(|metric| metric.k == 1)
                        .map_or(0.0, |metric| metric.value)
                )
            })
            .unwrap_or_else(|| "quality=n/a".to_owned());
        println!(
            "{:<34} p95={:>10.3}ms qps={:>10.3} {}",
            method.name, method.p95_ms, method.queries_per_second, quality
        );
    }
    if let Some(queries) = report.break_even.break_even_queries {
        println!("Hybrid/exhaustive break-even: {:.2} queries", queries);
    } else {
        println!("Hybrid/exhaustive break-even: not established");
    }
}

fn validate_relative_path(path: &str) -> BenchResult<()> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid(format!("unsafe corpus path {}", path.display())));
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut filename = path
        .file_name()
        .map_or_else(|| "scores".into(), |name| name.to_os_string());
    filename.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(filename)
}

fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(_) if destination.exists() => {
            fs::remove_file(destination)?;
            fs::rename(temporary, destination)
        }
        Err(error) => Err(error),
    }
}

fn default_profile() -> String {
    "unspecified".to_owned()
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Cell, better_final, better_transition, exhaustive_semi_global};

    #[test]
    fn exhaustive_alignment_finds_exact_infix() {
        let alignment = exhaustive_semi_global(&[2, 3, 4], &[0, 1, 2, 3, 4, 5])
            .expect("alignment");
        assert_eq!(alignment.distance, 0);
        assert_eq!((alignment.text_start, alignment.text_end), (2, 5));
        assert_eq!(alignment.similarity, 1.0);
    }

    #[test]
    fn exhaustive_alignment_handles_one_substitution() {
        let alignment = exhaustive_semi_global(&[2, 9, 4], &[0, 2, 3, 4, 8])
            .expect("alignment");
        assert_eq!(alignment.distance, 1);
        assert!(alignment.similarity > 0.60);
    }

    #[test]
    fn transition_tie_breaker_prefers_longer_then_earlier_spans() {
        let longer = Cell { cost: 1, start: 2 };
        let shorter = Cell { cost: 1, start: 4 };
        assert_eq!(better_transition(longer, shorter, 7), longer);
        let early = Cell { cost: 1, start: 1 };
        let late = Cell { cost: 1, start: 2 };
        assert_eq!(better_transition(early, late, 5), early);
    }

    #[test]
    fn final_tie_breaker_uses_each_candidates_actual_end() {
        let candidate = Cell { cost: 1, start: 4 };
        let best = Cell { cost: 1, start: 1 };
        assert!(better_final(candidate, 10, best, 5));
        assert!(!better_final(best, 5, candidate, 10));
    }
}
