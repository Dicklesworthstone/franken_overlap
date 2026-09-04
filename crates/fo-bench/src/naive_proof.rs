use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

use fo_core::{
    CompositeSearchOptions, Fingerprint, GroupedEvaluationOptions, GroupedEvaluationReport,
    GroupedLabeledScore, Index, IndexBuilder, IndexConfig, SearchIntent, SearchOptions,
    grouped_evaluation_report, normalize, qgram_hashes,
};
use fo_corpus::{CorpusManifest, unix_timestamp};
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

pub type ProofResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const NAIVE_PROOF_SCHEMA_VERSION: u32 = 1;
const NOISE_WORDS: &[&str] = &[
    "lantern", "railway", "orchard", "ceramic", "violet", "meadow", "saffron", "cabinet", "marble",
    "festival", "chimney", "harbor", "compass", "velvet",
];

#[derive(Debug, Clone)]
pub struct NaiveProofOptions {
    pub maximum_documents: usize,
    pub query_count: usize,
    pub passage_words: usize,
    pub hard_negatives: usize,
    pub maximum_total_cells: u128,
    pub repetitions: usize,
    pub seed: u64,
}

impl Default for NaiveProofOptions {
    fn default() -> Self {
        Self {
            maximum_documents: 48,
            query_count: 14,
            passage_words: 64,
            hard_negatives: 7,
            maximum_total_cells: 2_000_000_000,
            repetitions: 1,
            seed: 0x6e_61_69_76_65_2d_64_70,
        }
    }
}

impl NaiveProofOptions {
    pub fn validate(&self) -> ProofResult<()> {
        if self.maximum_documents < 2
            || self.query_count == 0
            || self.passage_words < 16
            || self.hard_negatives == 0
            || self.maximum_total_cells == 0
            || self.repetitions == 0
            || self.repetitions > 100
        {
            return Err(invalid("naive-proof limits are outside safe bounds"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NaiveProofReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_provider: String,
    pub indexed_documents: usize,
    pub requested_queries: usize,
    pub completed_queries: usize,
    pub skipped_queries: usize,
    pub hard_negatives_per_query: usize,
    pub candidate_subset_size: usize,
    pub passage_words: usize,
    pub repetitions: usize,
    pub seed: u64,
    pub total_dynamic_programming_cells: u128,
    pub naive: MethodTiming,
    pub franken_overlap_full_corpus: MethodTiming,
    pub p95_speedup_over_naive: f64,
    pub quality: QualityComparison,
    pub queries: Vec<QueryProof>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MethodTiming {
    pub measurements: usize,
    pub total_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub queries_per_second: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityComparison {
    pub naive: GroupedEvaluationReport,
    pub franken_overlap: GroupedEvaluationReport,
    pub micro_auprc_delta: f64,
    pub macro_auprc_delta: f64,
    pub recall_at_1_delta: f64,
    pub mean_reciprocal_rank_delta: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryProof {
    pub query_id: String,
    pub profile: String,
    pub query_text: String,
    pub source_id: String,
    pub source_title: String,
    pub query_tokens: usize,
    pub candidate_documents: usize,
    pub dynamic_programming_cells: u128,
    pub naive_ms: f64,
    pub franken_overlap_full_corpus_ms: f64,
    pub speedup: f64,
    pub naive_source_rank: RankInterval,
    pub franken_source_rank: RankInterval,
    pub candidates: Vec<CandidateProof>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RankInterval {
    pub best: usize,
    pub worst: usize,
    pub expected: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateProof {
    pub candidate_id: String,
    pub candidate_title: String,
    pub relevant: bool,
    pub qgram_jaccard: f64,
    pub naive_similarity: f64,
    pub franken_overlap_score: f64,
}

#[derive(Debug, Clone)]
struct Document {
    id: String,
    title: String,
    body: String,
    words: Vec<String>,
    normalized: fo_core::NormalizedText,
    qgrams: HashSet<Fingerprint>,
}

#[derive(Debug, Clone)]
struct GeneratedQuery {
    id: String,
    profile: Profile,
    source: usize,
    text: String,
}

#[derive(Debug, Clone, Copy)]
enum Profile {
    Exact,
    Formatting,
    Substitution,
    InsertDelete,
    Ocr,
    Fragmented,
    Reordered,
}

impl Profile {
    const ALL: [Self; 7] = [
        Self::Exact,
        Self::Formatting,
        Self::Substitution,
        Self::InsertDelete,
        Self::Ocr,
        Self::Fragmented,
        Self::Reordered,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Exact => "exact_passage",
            Self::Formatting => "formatting_case",
            Self::Substitution => "word_substitution",
            Self::InsertDelete => "insertion_deletion",
            Self::Ocr => "ocr_noise",
            Self::Fragmented => "fragmented_context",
            Self::Reordered => "reordered_blocks",
        }
    }

    const fn uses_composite(self) -> bool {
        matches!(self, Self::Fragmented | Self::Reordered)
    }
}

pub fn run_naive_proof(
    corpus_root: &Path,
    options: &NaiveProofOptions,
) -> ProofResult<NaiveProofReport> {
    options.validate()?;
    let manifest = CorpusManifest::load(corpus_root)?;
    let documents = load_documents(corpus_root, &manifest, options)?;
    if documents.len() < 2 {
        return Err(invalid(
            "corpus contains fewer than two sufficiently long documents",
        ));
    }
    let index = build_index(&documents)?;
    let queries = generate_queries(&documents, options);
    let id_to_index = documents
        .iter()
        .enumerate()
        .map(|(index, document)| (document.id.as_str(), index))
        .collect::<HashMap<_, _>>();

    let mut remaining_cells = options.maximum_total_cells;
    let mut completed = Vec::new();
    let mut naive_examples = Vec::new();
    let mut franken_examples = Vec::new();
    let mut naive_latencies = Vec::new();
    let mut franken_latencies = Vec::new();
    let mut total_cells = 0u128;
    let mut skipped = 0usize;

    for query in queries {
        let normalized_query = normalize(&query.text, &IndexConfig::default().normalization);
        if normalized_query.is_empty() {
            skipped += 1;
            continue;
        }
        let query_qgrams = fingerprint_set(&normalized_query.tokens, 5)?;
        let candidates = hard_candidate_subset(
            &documents,
            query.source,
            &query_qgrams,
            options.hard_negatives,
        );
        let cells_per_repetition = candidates.iter().fold(0u128, |total, &candidate| {
            total.saturating_add(
                (normalized_query.len() as u128)
                    .saturating_mul(documents[candidate].normalized.len() as u128),
            )
        });
        let required_cells = cells_per_repetition.saturating_mul(options.repetitions as u128);
        if required_cells > remaining_cells {
            skipped += 1;
            continue;
        }
        remaining_cells -= required_cells;
        total_cells = total_cells.saturating_add(required_cells);

        let mut naive_scores = Vec::new();
        let mut naive_total_ms = 0.0;
        for repetition in 0..options.repetitions {
            let started = Instant::now();
            let scores = candidates
                .iter()
                .map(|&candidate| {
                    naive_similarity(
                        &normalized_query.tokens,
                        &documents[candidate].normalized.tokens,
                    )
                })
                .collect::<Vec<_>>();
            let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
            naive_latencies.push(elapsed);
            naive_total_ms += elapsed;
            if repetition == 0 {
                naive_scores = scores;
            }
        }

        let mut franken_scores = Vec::new();
        let mut franken_total_ms = 0.0;
        for repetition in 0..options.repetitions {
            let started = Instant::now();
            let all_scores = franken_scores_for_query(&index, &query, &documents, &id_to_index)?;
            let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
            franken_latencies.push(elapsed);
            franken_total_ms += elapsed;
            if repetition == 0 {
                franken_scores = candidates
                    .iter()
                    .map(|&candidate| all_scores[candidate])
                    .collect();
            }
        }

        let naive_ms = naive_total_ms / options.repetitions as f64;
        let franken_ms = franken_total_ms / options.repetitions as f64;
        let query_id = query.id.clone();
        for (position, &candidate) in candidates.iter().enumerate() {
            let label = candidate == query.source;
            naive_examples.push(GroupedLabeledScore {
                query_id: query_id.clone(),
                score: naive_scores[position],
                label,
            });
            franken_examples.push(GroupedLabeledScore {
                query_id: query_id.clone(),
                score: franken_scores[position],
                label,
            });
        }
        let candidate_proofs = candidates
            .iter()
            .enumerate()
            .map(|(position, &candidate)| CandidateProof {
                candidate_id: documents[candidate].id.clone(),
                candidate_title: documents[candidate].title.clone(),
                relevant: candidate == query.source,
                qgram_jaccard: jaccard(&query_qgrams, &documents[candidate].qgrams),
                naive_similarity: naive_scores[position],
                franken_overlap_score: franken_scores[position],
            })
            .collect::<Vec<_>>();
        completed.push(QueryProof {
            query_id,
            profile: query.profile.name().to_owned(),
            query_text: query.text,
            source_id: documents[query.source].id.clone(),
            source_title: documents[query.source].title.clone(),
            query_tokens: normalized_query.len(),
            candidate_documents: candidates.len(),
            dynamic_programming_cells: required_cells,
            naive_ms,
            franken_overlap_full_corpus_ms: franken_ms,
            speedup: safe_ratio(naive_ms, franken_ms),
            naive_source_rank: rank_interval(
                &naive_scores,
                candidates
                    .iter()
                    .position(|&value| value == query.source)
                    .unwrap_or(0),
            ),
            franken_source_rank: rank_interval(
                &franken_scores,
                candidates
                    .iter()
                    .position(|&value| value == query.source)
                    .unwrap_or(0),
            ),
            candidates: candidate_proofs,
        });
    }

    if completed.is_empty() {
        return Err(invalid(
            "naive cell budget was insufficient for every generated query",
        ));
    }
    let evaluation = GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    };
    let naive_quality = grouped_evaluation_report(&naive_examples, evaluation.clone())?;
    let franken_quality = grouped_evaluation_report(&franken_examples, evaluation)?;
    let naive_timing = timing_report(&naive_latencies, completed.len());
    let franken_timing = timing_report(&franken_latencies, completed.len());
    Ok(NaiveProofReport {
        schema_version: NAIVE_PROOF_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id,
        corpus_provider: format!("{:?}", manifest.provider),
        indexed_documents: documents.len(),
        requested_queries: options.query_count,
        completed_queries: completed.len(),
        skipped_queries: skipped,
        hard_negatives_per_query: options.hard_negatives,
        candidate_subset_size: options
            .hard_negatives
            .saturating_add(1)
            .min(documents.len()),
        passage_words: options.passage_words,
        repetitions: options.repetitions,
        seed: options.seed,
        total_dynamic_programming_cells: total_cells,
        p95_speedup_over_naive: safe_ratio(naive_timing.p95_ms, franken_timing.p95_ms),
        quality: QualityComparison {
            micro_auprc_delta: franken_quality.micro.average_precision
                - naive_quality.micro.average_precision,
            macro_auprc_delta: franken_quality.macro_average_precision
                - naive_quality.macro_average_precision,
            recall_at_1_delta: metric_at(&franken_quality, 1) - metric_at(&naive_quality, 1),
            mean_reciprocal_rank_delta: franken_quality.mean_reciprocal_rank
                - naive_quality.mean_reciprocal_rank,
            naive: naive_quality,
            franken_overlap: franken_quality,
        },
        naive: naive_timing,
        franken_overlap_full_corpus: franken_timing,
        queries: completed,
    })
}

pub fn render_naive_proof(report: &NaiveProofReport) -> String {
    let mut output = String::new();
    output.push_str("# FrankenOverlap versus naïve semi-global Levenshtein\n\n");
    output.push_str(&format!("Corpus: `{}`  \n", report.corpus_id));
    output.push_str(&format!(
        "Indexed documents: {}  \n",
        report.indexed_documents
    ));
    output.push_str(&format!(
        "Completed / skipped queries: {} / {}  \n",
        report.completed_queries, report.skipped_queries
    ));
    output.push_str(&format!(
        "Dynamic-programming cells evaluated: {}  \n\n",
        report.total_dynamic_programming_cells
    ));
    output.push_str("## Aggregate comparison\n\n");
    output.push_str("| Metric | Naïve Levenshtein | FrankenOverlap | Delta / ratio |\n");
    output.push_str("|---|---:|---:|---:|\n");
    output.push_str(&format!(
        "| Micro AUPRC | {:.6} | {:.6} | {:+.6} |\n",
        report.quality.naive.micro.average_precision,
        report.quality.franken_overlap.micro.average_precision,
        report.quality.micro_auprc_delta,
    ));
    output.push_str(&format!(
        "| Macro query AUPRC | {:.6} | {:.6} | {:+.6} |\n",
        report.quality.naive.macro_average_precision,
        report.quality.franken_overlap.macro_average_precision,
        report.quality.macro_auprc_delta,
    ));
    output.push_str(&format!(
        "| Recall@1 | {:.6} | {:.6} | {:+.6} |\n",
        metric_at(&report.quality.naive, 1),
        metric_at(&report.quality.franken_overlap, 1),
        report.quality.recall_at_1_delta,
    ));
    output.push_str(&format!(
        "| p95 query time | {:.3} ms | {:.3} ms | {:.3}× speedup |\n\n",
        report.naive.p95_ms,
        report.franken_overlap_full_corpus.p95_ms,
        report.p95_speedup_over_naive,
    ));
    output.push_str("The naïve method scans only the source plus selected hard negatives. FrankenOverlap searches the complete indexed corpus, so a speedup above one is deliberately conservative.\n\n");
    output.push_str("## Real query examples\n\n");
    for query in &report.queries {
        output.push_str(&format!(
            "### {} — {}\n\n",
            query.profile, query.source_title
        ));
        output.push_str(&format!("> {}\n\n", one_line(&query.query_text, 700)));
        output.push_str(&format!(
            "Naïve: {:.3} ms, source rank {:.2} ({}–{}).  \n",
            query.naive_ms,
            query.naive_source_rank.expected,
            query.naive_source_rank.best,
            query.naive_source_rank.worst,
        ));
        output.push_str(&format!(
            "FrankenOverlap full corpus: {:.3} ms, source rank {:.2} ({}–{}), speedup {:.3}×.\n\n",
            query.franken_overlap_full_corpus_ms,
            query.franken_source_rank.expected,
            query.franken_source_rank.best,
            query.franken_source_rank.worst,
            query.speedup,
        ));
        output.push_str("| Candidate | Relevant | Jaccard | Naïve | FrankenOverlap |\n");
        output.push_str("|---|:---:|---:|---:|---:|\n");
        let mut candidates = query.candidates.clone();
        candidates.sort_unstable_by(|left, right| {
            right
                .franken_overlap_score
                .total_cmp(&left.franken_overlap_score)
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        for candidate in candidates {
            output.push_str(&format!(
                "| {} | {} | {:.4} | {:.4} | {:.4} |\n",
                markdown_cell(&candidate.candidate_title),
                if candidate.relevant { "yes" } else { "" },
                candidate.qgram_jaccard,
                candidate.naive_similarity,
                candidate.franken_overlap_score,
            ));
        }
        output.push('\n');
    }
    output
}

fn load_documents(
    root: &Path,
    manifest: &CorpusManifest,
    options: &NaiveProofOptions,
) -> ProofResult<Vec<Document>> {
    let normalization = IndexConfig::default().normalization;
    let mut candidates = manifest.documents.iter().collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|document| stable_hash(&document.id, options.seed));
    let mut documents = Vec::new();
    for document in candidates {
        if documents.len() >= options.maximum_documents {
            break;
        }
        let path = root.join(&document.relative_path);
        let body = match fs::read_to_string(path) {
            Ok(body) => body,
            Err(_) => continue,
        };
        let words = tokenize_words(&body);
        if words.len() < options.passage_words.saturating_mul(2) {
            continue;
        }
        let normalized = normalize(&body, &normalization);
        let qgrams = fingerprint_set(&normalized.tokens, 5)?;
        documents.push(Document {
            id: document.id.clone(),
            title: document.title.clone(),
            body,
            words,
            normalized,
            qgrams,
        });
    }
    documents.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Ok(documents)
}

fn build_index(documents: &[Document]) -> ProofResult<Index> {
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    for document in documents {
        builder.add_document(document.id.clone(), &document.body)?;
    }
    Ok(builder.build()?)
}

fn generate_queries(documents: &[Document], options: &NaiveProofOptions) -> Vec<GeneratedQuery> {
    let mut sources = (0..documents.len()).collect::<Vec<_>>();
    sources.sort_unstable_by_key(|&index| {
        stable_hash(&documents[index].id, options.seed ^ 0xa5a5_a5a5_a5a5_a5a5)
    });
    let mut queries = Vec::with_capacity(options.query_count);
    for query_index in 0..options.query_count {
        let source = sources[query_index % sources.len()];
        let profile = Profile::ALL[query_index % Profile::ALL.len()];
        let document = &documents[source];
        let mut rng =
            DeterministicRng::new(stable_hash(&document.id, options.seed ^ query_index as u64));
        let maximum_start = document.words.len().saturating_sub(options.passage_words);
        let start = if maximum_start == 0 {
            0
        } else {
            rng.range(maximum_start + 1)
        };
        let passage = &document.words[start..start + options.passage_words];
        queries.push(GeneratedQuery {
            id: format!("proof-{query_index:04}-{}", profile.name()),
            profile,
            source,
            text: mutate(passage, profile, &mut rng),
        });
    }
    queries
}

fn hard_candidate_subset(
    documents: &[Document],
    source: usize,
    query_qgrams: &HashSet<Fingerprint>,
    hard_negatives: usize,
) -> Vec<usize> {
    let mut negatives = documents
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != source)
        .map(|(index, document)| (index, jaccard(query_qgrams, &document.qgrams)))
        .collect::<Vec<_>>();
    negatives.sort_unstable_by(|(left_index, left), (right_index, right)| {
        right
            .total_cmp(left)
            .then_with(|| documents[*left_index].id.cmp(&documents[*right_index].id))
    });
    let mut output = vec![source];
    output.extend(
        negatives
            .into_iter()
            .take(hard_negatives)
            .map(|(index, _)| index),
    );
    output
}

fn franken_scores_for_query(
    index: &Index,
    query: &GeneratedQuery,
    documents: &[Document],
    id_to_index: &HashMap<&str, usize>,
) -> ProofResult<Vec<f64>> {
    let search = SearchOptions {
        intent: SearchIntent::SourceAttribution,
        max_results: documents.len(),
        max_candidates: documents.len().saturating_mul(64).max(200),
        max_postings_per_feature: 10_000_000,
        minimum_matched_tokens: 8,
        minimum_query_coverage: 0.0,
        minimum_source_coverage: 0.0,
        direct_fallback_work_limit: 1_000_000_000,
        short_query_candidates: documents.len().max(8),
        minimum_similarity: 0.0,
        ..SearchOptions::default()
    };
    let mut scores = vec![0.0; documents.len()];
    if query.profile.uses_composite() {
        let results = index.search_composite(
            &query.text,
            &search,
            CompositeSearchOptions {
                maximum_blocks_per_document: 12,
                minimum_block_tokens: 8,
                minimum_incremental_query_tokens: 6,
                maximum_overlap_fraction: 0.80,
                minimum_aggregate_score: 0.0,
            },
        )?;
        for result in results {
            if let Some(&position) = id_to_index.get(result.path.as_str()) {
                scores[position] = f64::from(result.aggregate_score.clamp(0.0, 1.0));
            }
        }
    } else {
        for result in index.search(&query.text, &search)? {
            if let Some(&position) = id_to_index.get(result.path.as_str()) {
                scores[position] = f64::from(result.combined_score.clamp(0.0, 1.0));
            }
        }
    }
    Ok(scores)
}

fn naive_similarity(pattern: &[u32], text: &[u32]) -> f64 {
    if pattern.is_empty() {
        return 0.0;
    }
    let distance = semi_global_levenshtein(pattern, text);
    (1.0 - distance as f64 / pattern.len() as f64).clamp(0.0, 1.0)
}

fn semi_global_levenshtein(pattern: &[u32], text: &[u32]) -> usize {
    if pattern.is_empty() {
        return 0;
    }
    if text.is_empty() {
        return pattern.len();
    }
    let mut previous = vec![0usize; text.len() + 1];
    let mut current = vec![0usize; text.len() + 1];
    for (pattern_index, &pattern_token) in pattern.iter().enumerate() {
        current[0] = pattern_index + 1;
        for (text_index, &text_token) in text.iter().enumerate() {
            let substitution = previous[text_index] + usize::from(pattern_token != text_token);
            let deletion = previous[text_index + 1] + 1;
            let insertion = current[text_index] + 1;
            current[text_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.into_iter().min().unwrap_or(pattern.len())
}

fn timing_report(latencies: &[f64], completed_queries: usize) -> MethodTiming {
    let mut sorted = latencies.to_vec();
    sorted.sort_by(f64::total_cmp);
    let total_ms = sorted.iter().sum::<f64>();
    MethodTiming {
        measurements: sorted.len(),
        total_ms,
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: percentile(&sorted, 0.99),
        queries_per_second: completed_queries as f64 / (total_ms / 1_000.0).max(1.0e-12),
    }
}

fn rank_interval(scores: &[f64], positive_index: usize) -> RankInterval {
    let positive = scores.get(positive_index).copied().unwrap_or(0.0);
    let better = scores.iter().filter(|&&score| score > positive).count();
    let tied = scores
        .iter()
        .filter(|&&score| score.total_cmp(&positive).is_eq())
        .count()
        .max(1);
    let best = better + 1;
    let worst = better + tied;
    RankInterval {
        best,
        worst,
        expected: (best + worst) as f64 / 2.0,
    }
}

fn metric_at(report: &GroupedEvaluationReport, k: usize) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == k)
        .map_or(0.0, |metric| metric.value)
}

fn fingerprint_set(tokens: &[u32], qgram: usize) -> ProofResult<HashSet<Fingerprint>> {
    Ok(qgram_hashes(tokens, qgram)?
        .into_iter()
        .map(|feature| feature.fingerprint)
        .collect())
}

fn jaccard(left: &HashSet<Fingerprint>, right: &HashSet<Fingerprint>) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(right).count();
    let union = left.len() + right.len() - intersection;
    intersection as f64 / union.max(1) as f64
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.unicode_words().map(str::to_owned).collect()
}

fn mutate(passage: &[String], profile: Profile, rng: &mut DeterministicRng) -> String {
    match profile {
        Profile::Exact => passage.join(" "),
        Profile::Formatting => passage
            .iter()
            .enumerate()
            .map(|(index, word)| {
                let word = if index % 9 == 0 {
                    word.to_ascii_uppercase()
                } else {
                    word.clone()
                };
                if index % 11 == 0 {
                    format!("{word},\n")
                } else if index % 7 == 0 {
                    format!("{word};")
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        Profile::Substitution => passage
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if index % 7 == 3 {
                    NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned()
                } else {
                    word.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        Profile::InsertDelete => {
            let mut output = Vec::new();
            for (index, word) in passage.iter().enumerate() {
                if index % 11 == 5 {
                    continue;
                }
                output.push(word.clone());
                if index % 13 == 7 {
                    output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
                    output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
                }
            }
            output.join(" ")
        }
        Profile::Ocr => passage
            .join(" ")
            .chars()
            .enumerate()
            .map(|(index, character)| {
                if index % 29 != 11 {
                    character
                } else {
                    match character {
                        'o' | 'O' => '0',
                        'l' | 'I' => '1',
                        'e' | 'E' => 'c',
                        'm' | 'M' => 'n',
                        _ => character,
                    }
                }
            })
            .collect(),
        Profile::Fragmented => {
            let third = passage.len() / 3;
            let mut output = passage[..third].to_vec();
            for _ in 0..24 {
                output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
            }
            output.extend_from_slice(&passage[passage.len() - third..]);
            output.join(" ")
        }
        Profile::Reordered => {
            let third = passage.len() / 3;
            let mut output = passage[third..third * 2].to_vec();
            output.extend_from_slice(&passage[..third]);
            output.extend_from_slice(&passage[third * 2..]);
            output.join(" ")
        }
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
}

fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    numerator.max(0.0) / denominator.max(1.0e-12)
}

fn stable_hash(value: &str, seed: u64) -> u64 {
    value
        .bytes()
        .fold(seed ^ 0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn one_line(value: &str, maximum: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum {
        compact
    } else {
        compact.chars().take(maximum).collect::<String>() + "…"
    }
}

fn markdown_cell(value: &str) -> String {
    one_line(value, 120).replace('|', "\\|")
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[derive(Debug, Clone, Copy)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next() % upper as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{naive_similarity, semi_global_levenshtein};

    #[test]
    fn semi_global_distance_finds_an_infix() {
        let pattern = [2, 3, 4];
        let text = [0, 1, 2, 3, 4, 5];
        assert_eq!(semi_global_levenshtein(&pattern, &text), 0);
        assert_eq!(naive_similarity(&pattern, &text), 1.0);
    }

    #[test]
    fn semi_global_distance_counts_substitutions() {
        let pattern = [2, 9, 4];
        let text = [0, 1, 2, 3, 4, 5];
        assert_eq!(semi_global_levenshtein(&pattern, &text), 1);
    }
}
