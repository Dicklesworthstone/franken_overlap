use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Args;
use fo_core::{
    EvaluationOptions, Fingerprint, IndexBuilder, IndexConfig, LabeledScore, NormalizationProfile,
    PrecisionRecallReport, SearchIntent, SearchOptions, normalize, precision_recall_report,
    qgram_hashes,
};
use serde::Serialize;

type BenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const COMMON_WORDS: &[&str] = &[
    "analysis",
    "system",
    "evidence",
    "method",
    "result",
    "process",
    "model",
    "signal",
    "record",
    "change",
    "measure",
    "structure",
    "value",
    "context",
    "source",
    "pattern",
    "review",
    "compare",
    "observe",
    "estimate",
    "document",
    "sequence",
    "sample",
    "feature",
    "robust",
    "careful",
    "repeat",
    "local",
    "global",
    "dynamic",
    "precise",
    "public",
];

const NOISE_WORDS: &[&str] = &[
    "lantern", "harbor", "violet", "railway", "ceramic", "orchard", "copper", "winter", "festival",
    "cabinet", "marble", "compass", "velvet", "meadow", "chimney", "saffron",
];

const TOPICS: &[&[&str]] = &[
    &[
        "telescope",
        "observatory",
        "spectrum",
        "photon",
        "orbit",
        "galaxy",
        "stellar",
        "aperture",
        "detector",
        "calibration",
        "exposure",
        "cosmic",
    ],
    &[
        "enzyme",
        "genome",
        "protein",
        "cellular",
        "assay",
        "mutation",
        "receptor",
        "pathway",
        "tissue",
        "molecule",
        "clinical",
        "biological",
    ],
    &[
        "contract",
        "statute",
        "clause",
        "court",
        "filing",
        "issuer",
        "covenant",
        "liability",
        "regulatory",
        "disclosure",
        "agreement",
        "precedent",
    ],
    &[
        "compiler",
        "kernel",
        "memory",
        "vector",
        "thread",
        "runtime",
        "dispatch",
        "cache",
        "buffer",
        "instruction",
        "latency",
        "throughput",
    ],
    &[
        "market",
        "portfolio",
        "return",
        "liquidity",
        "issuer",
        "yield",
        "spread",
        "valuation",
        "risk",
        "position",
        "capital",
        "transaction",
    ],
    &[
        "weather",
        "pressure",
        "rainfall",
        "climate",
        "temperature",
        "station",
        "forecast",
        "humidity",
        "storm",
        "atmosphere",
        "seasonal",
        "sensor",
    ],
    &[
        "building",
        "plumbing",
        "ventilation",
        "electrical",
        "foundation",
        "contractor",
        "equipment",
        "inspection",
        "material",
        "installation",
        "geometry",
        "site",
    ],
    &[
        "language",
        "sentence",
        "token",
        "corpus",
        "semantic",
        "lexical",
        "paragraph",
        "translation",
        "grammar",
        "phrase",
        "document",
        "alignment",
    ],
];

#[derive(Debug, Args)]
pub struct SyntheticCommand {
    /// Number of corpus documents.
    #[arg(long, default_value_t = 32)]
    documents: usize,
    /// Number of mutation profiles generated per source document.
    #[arg(long, default_value_t = 4)]
    queries_per_document: usize,
    #[arg(long, default_value_t = 0x5eed_f00d_cafe_babe)]
    seed: u64,
    /// Pretty-print the complete report as JSON.
    #[arg(long)]
    json: bool,
    /// Optional JSON report destination.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Optional JSONL destination for FrankenOverlap pair scores.
    #[arg(long)]
    labeled_scores: Option<PathBuf>,
    /// Fail if FrankenOverlap AUPRC is below this floor.
    #[arg(long)]
    minimum_auprc: Option<f64>,
    /// Fail if FrankenOverlap Recall@1 is below this floor.
    #[arg(long)]
    minimum_recall_at_1: Option<f64>,
}

#[derive(Debug, Clone)]
struct GeneratedDocument {
    id: usize,
    words: Vec<String>,
    text: String,
}

#[derive(Debug, Clone)]
struct GeneratedQuery {
    source_document: usize,
    profile: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
pub struct SyntheticBenchmarkReport {
    schema_version: u32,
    seed: u64,
    documents: usize,
    queries: usize,
    pairs: usize,
    index_build_ms: f64,
    baseline_preparation_ms: f64,
    profiles: Vec<String>,
    methods: Vec<MethodReport>,
}

#[derive(Debug, Serialize)]
struct MethodReport {
    name: String,
    elapsed_ms: f64,
    pairs_per_second: f64,
    recall_at_1: f64,
    mean_reciprocal_rank: f64,
    false_positives_per_query_at_best_f1: f64,
    quality: PrecisionRecallReport,
}

#[derive(Debug, Default)]
struct MethodAccumulator {
    labeled_scores: Vec<LabeledScore>,
    top_one: usize,
    reciprocal_rank_sum: f64,
    elapsed: Duration,
}

impl MethodAccumulator {
    fn observe(&mut self, scores: &[f64], source_document: usize, elapsed: Duration) {
        self.elapsed += elapsed;
        for (document_id, &score) in scores.iter().enumerate() {
            self.labeled_scores.push(LabeledScore {
                score: score.clamp(0.0, 1.0),
                label: document_id == source_document,
            });
        }
        let mut ranking = (0..scores.len()).collect::<Vec<_>>();
        ranking.sort_unstable_by(|&left, &right| {
            scores[right]
                .total_cmp(&scores[left])
                .then_with(|| left.cmp(&right))
        });
        if ranking.first().copied() == Some(source_document) {
            self.top_one += 1;
        }
        if let Some(rank) = ranking
            .iter()
            .position(|&document_id| document_id == source_document)
        {
            self.reciprocal_rank_sum += 1.0 / (rank + 1) as f64;
        }
    }

    fn report(
        self,
        name: &str,
        query_count: usize,
        pair_count: usize,
    ) -> BenchResult<MethodReport> {
        let quality = precision_recall_report(&self.labeled_scores, EvaluationOptions::default())?;
        let false_positives = self
            .labeled_scores
            .iter()
            .filter(|example| !example.label && example.score >= quality.best_threshold)
            .count();
        let seconds = self.elapsed.as_secs_f64();
        Ok(MethodReport {
            name: name.to_owned(),
            elapsed_ms: seconds * 1_000.0,
            pairs_per_second: pair_count as f64 / seconds.max(1.0e-12),
            recall_at_1: self.top_one as f64 / query_count.max(1) as f64,
            mean_reciprocal_rank: self.reciprocal_rank_sum / query_count.max(1) as f64,
            false_positives_per_query_at_best_f1: false_positives as f64
                / query_count.max(1) as f64,
            quality,
        })
    }
}

pub fn run(command: SyntheticCommand) -> BenchResult<()> {
    if command.documents < 2 {
        return Err(invalid_input("--documents must be at least 2"));
    }
    if command.queries_per_document == 0 || command.queries_per_document > 64 {
        return Err(invalid_input(
            "--queries-per-document must be between 1 and 64",
        ));
    }
    for (name, value) in [
        ("--minimum-auprc", command.minimum_auprc),
        ("--minimum-recall-at-1", command.minimum_recall_at_1),
    ] {
        if let Some(value) = value
            && (!value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(invalid_input(format!("{name} must lie in [0, 1]")));
        }
    }

    let mut rng = DeterministicRng::new(command.seed);
    let documents = generate_documents(command.documents, &mut rng);
    let queries = generate_queries(&documents, command.queries_per_document, &mut rng);
    let pair_count = documents.len().saturating_mul(queries.len());

    let index_start = Instant::now();
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    for document in &documents {
        builder.add_document(format!("synthetic/{:04}.txt", document.id), &document.text)?;
    }
    let index = builder.build()?;
    let index_build_ms = index_start.elapsed().as_secs_f64() * 1_000.0;

    let preparation_start = Instant::now();
    let normalized_documents = documents
        .iter()
        .map(|document| normalize(&document.text, &NormalizationProfile::default()))
        .collect::<Vec<_>>();
    let document_qgrams = normalized_documents
        .iter()
        .map(|document| fingerprint_set(&document.tokens, 5))
        .collect::<Result<Vec<_>, _>>()?;
    let document_simhashes = normalized_documents
        .iter()
        .map(|document| simhash(&document.tokens, 5))
        .collect::<Result<Vec<_>, _>>()?;
    let baseline_preparation_ms = preparation_start.elapsed().as_secs_f64() * 1_000.0;

    let mut overlap = MethodAccumulator::default();
    let mut exact = MethodAccumulator::default();
    let mut jaccard = MethodAccumulator::default();
    let mut simhash_method = MethodAccumulator::default();

    for query in &queries {
        let normalized_query = normalize(&query.text, &NormalizationProfile::default());

        let started = Instant::now();
        let hits = index.search(
            &query.text,
            &SearchOptions {
                intent: SearchIntent::SourceAttribution,
                max_results: documents.len(),
                max_candidates: documents.len().saturating_mul(16).max(200),
                minimum_similarity: 0.0,
                minimum_matched_tokens: 8,
                minimum_query_coverage: 0.05,
                ..SearchOptions::default()
            },
        )?;
        let mut overlap_scores = vec![0.0f64; documents.len()];
        for hit in hits {
            if let Some(score) = overlap_scores.get_mut(hit.document_id as usize) {
                *score = (*score).max(f64::from(hit.combined_score));
            }
        }
        overlap.observe(&overlap_scores, query.source_document, started.elapsed());

        let started = Instant::now();
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
        exact.observe(&exact_scores, query.source_document, started.elapsed());

        let started = Instant::now();
        let query_qgrams = fingerprint_set(&normalized_query.tokens, 5)?;
        let jaccard_scores = document_qgrams
            .iter()
            .map(|document| jaccard_similarity(&query_qgrams, document))
            .collect::<Vec<_>>();
        jaccard.observe(&jaccard_scores, query.source_document, started.elapsed());

        let started = Instant::now();
        let query_simhash = simhash(&normalized_query.tokens, 5)?;
        let simhash_scores = document_simhashes
            .iter()
            .map(|&document| 1.0 - f64::from((query_simhash ^ document).count_ones()) / 64.0)
            .collect::<Vec<_>>();
        simhash_method.observe(&simhash_scores, query.source_document, started.elapsed());
    }

    if let Some(path) = &command.labeled_scores {
        write_labeled_scores(path, &overlap.labeled_scores)?;
    }

    let query_count = queries.len();
    let overlap_report = overlap.report("franken_overlap", query_count, pair_count)?;
    let methods = vec![
        overlap_report,
        exact.report("normalized_exact_substring", query_count, pair_count)?,
        jaccard.report("character_qgram_jaccard", query_count, pair_count)?,
        simhash_method.report("character_qgram_simhash", query_count, pair_count)?,
    ];
    let report = SyntheticBenchmarkReport {
        schema_version: 1,
        seed: command.seed,
        documents: documents.len(),
        queries: query_count,
        pairs: pair_count,
        index_build_ms,
        baseline_preparation_ms,
        profiles: {
            let mut profiles = queries
                .iter()
                .map(|query| query.profile.to_owned())
                .collect::<Vec<_>>();
            profiles.sort_unstable();
            profiles.dedup();
            profiles
        },
        methods,
    };

    if let Some(path) = &command.output {
        write_report(path, &report)?;
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_summary(&report);
    }

    let overlap = report
        .methods
        .first()
        .ok_or_else(|| invalid_input("benchmark produced no methods"))?;
    if let Some(minimum) = command.minimum_auprc
        && overlap.quality.average_precision < minimum
    {
        return Err(invalid_input(format!(
            "FrankenOverlap AUPRC {:.6} is below required {:.6}",
            overlap.quality.average_precision, minimum
        )));
    }
    if let Some(minimum) = command.minimum_recall_at_1
        && overlap.recall_at_1 < minimum
    {
        return Err(invalid_input(format!(
            "FrankenOverlap Recall@1 {:.6} is below required {:.6}",
            overlap.recall_at_1, minimum
        )));
    }
    Ok(())
}

fn generate_documents(count: usize, rng: &mut DeterministicRng) -> Vec<GeneratedDocument> {
    (0..count)
        .map(|id| {
            let topic = TOPICS[id % TOPICS.len()];
            let mut words = Vec::with_capacity(240);
            for position in 0..240 {
                let word = if position % 17 == 0 {
                    topic[(position + id) % topic.len()]
                } else if rng.range(100) < 58 {
                    topic[rng.range(topic.len())]
                } else {
                    COMMON_WORDS[rng.range(COMMON_WORDS.len())]
                };
                words.push(word.to_owned());
            }
            let text = render_words(&words, false);
            GeneratedDocument { id, words, text }
        })
        .collect()
}

fn generate_queries(
    documents: &[GeneratedDocument],
    per_document: usize,
    rng: &mut DeterministicRng,
) -> Vec<GeneratedQuery> {
    let mut queries = Vec::with_capacity(documents.len().saturating_mul(per_document));
    for document in documents {
        for query_index in 0..per_document {
            let length = 72usize.min(document.words.len().saturating_sub(24));
            let maximum_start = document.words.len().saturating_sub(length + 12);
            let start = 12 + rng.range(maximum_start.saturating_sub(11).max(1));
            let source = &document.words[start..start + length];
            let profile = query_index % 4;
            let (name, text) = match profile {
                0 => ("formatting_only", render_words(source, true)),
                1 => (
                    "word_substitutions",
                    render_words(&substitute_words(source, rng, 11), true),
                ),
                2 => (
                    "insertions_and_deletions",
                    render_words(&indel_words(source, rng), true),
                ),
                _ => (
                    "partial_reuse_with_noise",
                    render_words(&partial_noise_words(source, rng), true),
                ),
            };
            queries.push(GeneratedQuery {
                source_document: document.id,
                profile: name,
                text,
            });
        }
    }
    queries
}

fn substitute_words(
    source: &[String],
    rng: &mut DeterministicRng,
    percentage: usize,
) -> Vec<String> {
    source
        .iter()
        .map(|word| {
            if rng.range(100) < percentage {
                NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned()
            } else {
                word.clone()
            }
        })
        .collect()
}

fn indel_words(source: &[String], rng: &mut DeterministicRng) -> Vec<String> {
    let mut output = Vec::with_capacity(source.len() + source.len() / 8);
    for word in source {
        if rng.range(100) < 7 {
            continue;
        }
        output.push(word.clone());
        if rng.range(100) < 7 {
            output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
        }
    }
    output
}

fn partial_noise_words(source: &[String], rng: &mut DeterministicRng) -> Vec<String> {
    let retained_start = source.len() / 7;
    let retained_end = source.len() - source.len() / 7;
    let noise_count = source.len() / 6;
    let mut output = Vec::with_capacity(source.len() + noise_count * 2);
    for _ in 0..noise_count {
        output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
    }
    output.extend_from_slice(&source[retained_start..retained_end]);
    for _ in 0..noise_count {
        output.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
    }
    output
}

fn render_words(words: &[String], decorated: bool) -> String {
    let mut output = String::new();
    for (index, word) in words.iter().enumerate() {
        if decorated && index % 13 == 0 {
            output.push_str(&word.to_uppercase());
        } else {
            output.push_str(word);
        }
        if index + 1 == words.len() {
            break;
        }
        if decorated && index % 11 == 10 {
            output.push_str(",\n");
        } else if index % 23 == 22 {
            output.push_str(". ");
        } else {
            output.push(' ');
        }
    }
    output
}

fn fingerprint_set(tokens: &[u32], qgram: usize) -> fo_core::Result<HashSet<Fingerprint>> {
    Ok(qgram_hashes(tokens, qgram)?
        .into_iter()
        .map(|feature| feature.fingerprint)
        .collect())
}

fn jaccard_similarity(left: &HashSet<Fingerprint>, right: &HashSet<Fingerprint>) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(right).count();
    let union = left.len() + right.len() - intersection;
    intersection as f64 / union.max(1) as f64
}

fn simhash(tokens: &[u32], qgram: usize) -> fo_core::Result<u64> {
    let mut counters = [0i64; 64];
    for feature in qgram_hashes(tokens, qgram)? {
        let value = feature.fingerprint.lo ^ feature.fingerprint.hi.rotate_left(17);
        for (bit, counter) in counters.iter_mut().enumerate() {
            if value & (1u64 << bit) == 0 {
                *counter -= 1;
            } else {
                *counter += 1;
            }
        }
    }
    let mut output = 0u64;
    for (bit, &counter) in counters.iter().enumerate() {
        if counter >= 0 {
            output |= 1u64 << bit;
        }
    }
    Ok(output)
}

fn write_labeled_scores(path: &Path, scores: &[LabeledScore]) -> BenchResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut output = fs::File::create(path)?;
    for score in scores {
        serde_json::to_writer(&mut output, score)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    Ok(())
}

fn write_report(path: &Path, report: &SyntheticBenchmarkReport) -> BenchResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

fn print_summary(report: &SyntheticBenchmarkReport) {
    println!(
        "Synthetic benchmark: {} documents, {} queries, {} pairs",
        report.documents, report.queries, report.pairs
    );
    println!("Index build: {:.3} ms", report.index_build_ms);
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>12}",
        "method", "AUPRC", "R@1", "MRR", "pairs/sec"
    );
    for method in &report.methods {
        println!(
            "{:<28} {:>9.5} {:>9.5} {:>9.5} {:>12.0}",
            method.name,
            method.quality.average_precision,
            method.recall_at_1,
            method.mean_reciprocal_rank,
            method.pairs_per_second
        );
    }
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[derive(Debug, Clone, Copy)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            return 0;
        }
        (self.next_u64() % upper as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeterministicRng, fingerprint_set, generate_documents, generate_queries,
        jaccard_similarity, simhash,
    };
    use fo_core::{NormalizationProfile, normalize};

    #[test]
    fn generation_is_deterministic() {
        let mut left = DeterministicRng::new(7);
        let mut right = DeterministicRng::new(7);
        let left_documents = generate_documents(4, &mut left);
        let right_documents = generate_documents(4, &mut right);
        assert_eq!(left_documents[2].text, right_documents[2].text);
        let left_queries = generate_queries(&left_documents, 4, &mut left);
        let right_queries = generate_queries(&right_documents, 4, &mut right);
        assert_eq!(left_queries[7].text, right_queries[7].text);
    }

    #[test]
    fn qgram_baselines_rank_identical_text_above_unrelated_text() {
        let profile = NormalizationProfile::default();
        let source = normalize("alpha beta gamma delta epsilon", &profile);
        let unrelated = normalize("winter railway ceramic orchard violet", &profile);
        let query = normalize("alpha beta gamma delta", &profile);
        let query_set = fingerprint_set(&query.tokens, 3).expect("query");
        let source_set = fingerprint_set(&source.tokens, 3).expect("source");
        let unrelated_set = fingerprint_set(&unrelated.tokens, 3).expect("unrelated");
        assert!(
            jaccard_similarity(&query_set, &source_set)
                > jaccard_similarity(&query_set, &unrelated_set)
        );
        let query_hash = simhash(&query.tokens, 3).expect("query hash");
        let source_hash = simhash(&source.tokens, 3).expect("source hash");
        let unrelated_hash = simhash(&unrelated.tokens, 3).expect("unrelated hash");
        assert!(
            (query_hash ^ source_hash).count_ones() < (query_hash ^ unrelated_hash).count_ones()
        );
    }
}
