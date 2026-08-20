#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use fo_core::{
    Fingerprint, GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore,
    HybridDocumentInput, HybridIndex, HybridIndexBuilder, HybridIndexConfig, HybridQueryMode,
    HybridSearchOptions, LexicalSearchOptions, NormalizationProfile, SearchOptions,
    grouped_evaluation_report, normalize, qgram_hashes,
};
use fo_corpus::{
    CorpusManifest, GutenbergOptions, GutenbergPreset, Sec10KOptions, SecPreset, atomic_write,
    fetch_gutenberg, fetch_sec_10k, unix_timestamp,
};
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

type BenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const SCHEMA_VERSION: u32 = 1;
const MAX_QUERY_PROFILES: usize = 8;
const NOISE_WORDS: &[&str] = &[
    "lantern", "railway", "orchard", "ceramic", "violet", "meadow", "saffron", "cabinet", "marble",
    "festival", "chimney", "harbor", "compass", "velvet", "kitchen", "weather",
];

#[derive(Debug, Parser)]
#[command(
    name = "fo-real-bench",
    version,
    about = "Download or load real corpora and benchmark exact, legacy, lexical, overlap, and hybrid retrieval"
)]
struct Cli {
    /// Corpus root containing fo-corpus manifest.json, or download destination.
    #[arg(long, default_value = "corpora/benchmark")]
    corpus_root: PathBuf,
    #[arg(long, value_enum, default_value = "existing")]
    provider: CorpusSourceArg,
    #[arg(long, value_enum, default_value = "smoke")]
    preset: CorpusPresetArg,
    /// Override provider preset size: books for Gutenberg, companies for SEC.
    #[arg(long)]
    provider_items: Option<usize>,
    /// Required for Gutenberg standard/large acquisition.
    #[arg(long)]
    gutenberg_mirror: Option<String>,
    /// SEC identity such as "Example Research research@example.com".
    #[arg(long)]
    sec_user_agent: Option<String>,
    #[arg(long, default_value_t = 3)]
    sec_filings_per_company: usize,
    #[arg(long)]
    refresh_downloads: bool,
    /// Maximum documents retained in the benchmark index.
    #[arg(long, default_value_t = 250)]
    maximum_documents: usize,
    /// Number of indexed documents used as query sources.
    #[arg(long, default_value_t = 32)]
    source_documents: usize,
    #[arg(long, default_value_t = 8)]
    queries_per_document: usize,
    #[arg(long, default_value_t = 96)]
    passage_words: usize,
    #[arg(long, default_value_t = 0x72_65_61_6c_2d_62_65_6e)]
    seed: u64,
    /// Persist the built hybrid index here; otherwise a temporary index is measured and removed.
    #[arg(long)]
    index_output: Option<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
    /// Optional JSONL containing every query-document label and every method score.
    #[arg(long)]
    scores_output: Option<PathBuf>,
    #[arg(long)]
    minimum_hybrid_auprc: Option<f64>,
    #[arg(long)]
    minimum_hybrid_recall_at_1: Option<f64>,
    #[arg(long)]
    require_hybrid_auprc_delta: Option<f64>,
    #[arg(long)]
    maximum_hybrid_p95_ms: Option<f64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CorpusSourceArg {
    Existing,
    Gutenberg,
    Sec10k,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CorpusPresetArg {
    Smoke,
    Standard,
    Large,
}

impl From<CorpusPresetArg> for GutenbergPreset {
    fn from(value: CorpusPresetArg) -> Self {
        match value {
            CorpusPresetArg::Smoke => Self::Smoke,
            CorpusPresetArg::Standard => Self::Standard,
            CorpusPresetArg::Large => Self::Large,
        }
    }
}

impl From<CorpusPresetArg> for SecPreset {
    fn from(value: CorpusPresetArg) -> Self {
        match value {
            CorpusPresetArg::Smoke => Self::Smoke,
            CorpusPresetArg::Standard => Self::Standard,
            CorpusPresetArg::Large => Self::Large,
        }
    }
}

#[derive(Debug, Clone)]
struct BenchmarkDocument {
    external_id: String,
    title: String,
    body: String,
    words: Vec<String>,
    tags: Vec<String>,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct GeneratedQuery {
    id: String,
    source_document: usize,
    profile: &'static str,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct ScoredPair {
    query_id: String,
    profile: String,
    source_id: String,
    candidate_id: String,
    label: bool,
    scores: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
struct RealBenchmarkReport {
    schema_version: u32,
    generated_at_unix: u64,
    corpus_id: String,
    corpus_provider: String,
    corpus_manifest_documents: usize,
    indexed_documents: usize,
    source_documents: usize,
    queries: usize,
    pairs: usize,
    seed: u64,
    profiles: Vec<String>,
    build: BuildReport,
    methods: Vec<MethodReport>,
}

#[derive(Debug, Serialize)]
struct BuildReport {
    build_ms: f64,
    serialization_ms: f64,
    index_bytes: u64,
    overlap_fingerprints: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
}

#[derive(Debug, Serialize)]
struct MethodReport {
    name: String,
    elapsed_ms: f64,
    queries_per_second: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    false_positives_per_query_at_best_f1: f64,
    quality: GroupedEvaluationReport,
    profiles: Vec<ProfileQuality>,
}

#[derive(Debug, Serialize)]
struct ProfileQuality {
    profile: String,
    queries: usize,
    micro_auprc: f64,
    macro_auprc: f64,
    recall_at_1: f64,
    mean_reciprocal_rank: f64,
}

#[derive(Default)]
struct MethodAccumulator {
    examples: Vec<GroupedLabeledScore>,
    profile_examples: BTreeMap<String, Vec<GroupedLabeledScore>>,
    latencies_ms: Vec<f64>,
    elapsed: Duration,
}

impl MethodAccumulator {
    fn observe(
        &mut self,
        query: &GeneratedQuery,
        scores: &[f64],
        documents: &[BenchmarkDocument],
        elapsed: Duration,
    ) {
        self.elapsed += elapsed;
        self.latencies_ms.push(elapsed.as_secs_f64() * 1_000.0);
        let profile = self
            .profile_examples
            .entry(query.profile.to_owned())
            .or_default();
        for (document_index, score) in scores.iter().copied().enumerate() {
            let example = GroupedLabeledScore {
                query_id: query.id.clone(),
                score: score.clamp(0.0, 1.0),
                label: document_index == query.source_document,
            };
            self.examples.push(example.clone());
            profile.push(example);
        }
        debug_assert_eq!(scores.len(), documents.len());
    }

    fn report(self, name: &str, query_count: usize) -> BenchResult<MethodReport> {
        let options = evaluation_options();
        let quality = grouped_evaluation_report(&self.examples, options.clone())?;
        let threshold = quality.micro.best_threshold;
        let false_positives = self
            .examples
            .iter()
            .filter(|example| !example.label && example.score >= threshold)
            .count();
        let mut profiles = Vec::new();
        for (profile, examples) in self.profile_examples {
            let queries = examples
                .iter()
                .map(|example| example.query_id.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            let report = grouped_evaluation_report(&examples, options.clone())?;
            profiles.push(ProfileQuality {
                profile,
                queries,
                micro_auprc: report.micro.average_precision,
                macro_auprc: report.macro_average_precision,
                recall_at_1: metric_at(&report, 1),
                mean_reciprocal_rank: report.mean_reciprocal_rank,
            });
        }
        profiles.sort_unstable_by(|left, right| left.profile.cmp(&right.profile));
        let seconds = self.elapsed.as_secs_f64();
        let mut latencies = self.latencies_ms;
        latencies.sort_by(f64::total_cmp);
        Ok(MethodReport {
            name: name.to_owned(),
            elapsed_ms: seconds * 1_000.0,
            queries_per_second: query_count as f64 / seconds.max(1.0e-12),
            p50_ms: percentile(&latencies, 0.50),
            p95_ms: percentile(&latencies, 0.95),
            p99_ms: percentile(&latencies, 0.99),
            false_positives_per_query_at_best_f1: false_positives as f64
                / query_count.max(1) as f64,
            quality,
            profiles,
        })
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-real-bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> BenchResult<()> {
    let command = Cli::parse();
    validate_command(&command)?;
    let manifest = prepare_corpus(&command)?;
    let documents = load_documents(
        &command.corpus_root,
        &manifest,
        command.maximum_documents,
        command.passage_words,
        command.seed,
    )?;
    if documents.len() < 2 {
        return Err(invalid_input(
            "real-corpus benchmark requires at least two sufficiently long documents",
        ));
    }
    let source_count = command.source_documents.min(documents.len());
    let queries = generate_queries(
        &documents,
        source_count,
        command.queries_per_document,
        command.passage_words,
        command.seed,
    );
    if queries.is_empty() {
        return Err(invalid_input("benchmark generated no queries"));
    }

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
    let hybrid_index = builder.build()?;
    let build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;

    let (serialization_ms, index_bytes) =
        persist_and_measure_index(&hybrid_index, command.index_output.as_deref())?;
    let stats = hybrid_index.stats();
    let build = BuildReport {
        build_ms,
        serialization_ms,
        index_bytes,
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
    let document_qgrams = normalized_documents
        .iter()
        .map(|document| fingerprint_set(&document.tokens, 5))
        .collect::<fo_core::Result<Vec<_>>>()?;
    let document_simhashes = document_qgrams.iter().map(simhash).collect::<Vec<_>>();

    let mut exact = MethodAccumulator::default();
    let mut jaccard = MethodAccumulator::default();
    let mut simhash_method = MethodAccumulator::default();
    let mut lexical = MethodAccumulator::default();
    let mut overlap = MethodAccumulator::default();
    let mut hybrid = MethodAccumulator::default();
    let mut scored_pairs = Vec::with_capacity(documents.len().saturating_mul(queries.len()));

    for query in &queries {
        let mut method_scores = BTreeMap::<String, Vec<f64>>::new();
        let normalized_query = normalize(&query.text, &normalization);

        let started = Instant::now();
        let scores = normalized_documents
            .iter()
            .map(|document| {
                if document.text.contains(&normalized_query.text) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        exact.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("normalized_exact_substring".to_owned(), scores);

        let started = Instant::now();
        let query_qgrams = fingerprint_set(&normalized_query.tokens, 5)?;
        let scores = document_qgrams
            .iter()
            .map(|document| jaccard_similarity(&query_qgrams, document))
            .collect::<Vec<_>>();
        jaccard.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("character_qgram_jaccard".to_owned(), scores);

        let started = Instant::now();
        let query_simhash = simhash(&query_qgrams);
        let scores = document_simhashes
            .iter()
            .map(|document| 1.0 - f64::from((query_simhash ^ document).count_ones()) / 64.0)
            .collect::<Vec<_>>();
        simhash_method.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("character_qgram_simhash".to_owned(), scores);

        let started = Instant::now();
        let lexical_results = hybrid_index.lexical_index().search_text(
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
        for result in lexical_results {
            if let Some(index) = document_index(&documents, &result.external_id) {
                scores[index] = 1.0 - (-f64::from(result.score.max(0.0)) / 4.0).exp();
            }
        }
        lexical.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("fielded_bm25_phrase_proximity".to_owned(), scores);

        let search_options = SearchOptions {
            max_results: documents.len(),
            max_candidates: documents.len().saturating_mul(32).max(200),
            max_postings_per_feature: 10_000_000,
            minimum_matched_tokens: 8.min(normalized_query.len().max(1)),
            minimum_query_coverage: 0.0,
            minimum_source_coverage: 0.0,
            direct_fallback_work_limit: 500_000_000,
            short_query_candidates: documents.len().clamp(8, 4_096),
            minimum_similarity: 0.0,
            ..SearchOptions::default()
        };
        let started = Instant::now();
        let overlap_results = hybrid_index
            .overlap_index()
            .search(&query.text, &search_options)?;
        let mut scores = vec![0.0; documents.len()];
        for result in overlap_results {
            if let Some(index) = document_index(&documents, &result.path) {
                scores[index] = f64::from(result.combined_score.clamp(0.0, 1.0));
            }
        }
        overlap.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("franken_overlap".to_owned(), scores);

        let started = Instant::now();
        let hybrid_results = hybrid_index.search(
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
        let mut scores = vec![0.0; documents.len()];
        for result in hybrid_results.results {
            if let Some(index) = document_index(&documents, &result.external_id) {
                scores[index] = f64::from(result.score);
            }
        }
        hybrid.observe(query, &scores, &documents, started.elapsed());
        method_scores.insert("franken_hybrid".to_owned(), scores);

        for document_index in 0..documents.len() {
            let scores = method_scores
                .iter()
                .map(|(name, values)| (name.clone(), values[document_index]))
                .collect::<BTreeMap<_, _>>();
            scored_pairs.push(ScoredPair {
                query_id: query.id.clone(),
                profile: query.profile.to_owned(),
                source_id: documents[query.source_document].external_id.clone(),
                candidate_id: documents[document_index].external_id.clone(),
                label: document_index == query.source_document,
                scores,
            });
        }
    }

    let query_count = queries.len();
    let methods = vec![
        exact.report("normalized_exact_substring", query_count)?,
        jaccard.report("character_qgram_jaccard", query_count)?,
        simhash_method.report("character_qgram_simhash", query_count)?,
        lexical.report("fielded_bm25_phrase_proximity", query_count)?,
        overlap.report("franken_overlap", query_count)?,
        hybrid.report("franken_hybrid", query_count)?,
    ];
    let mut profiles = queries
        .iter()
        .map(|query| query.profile.to_owned())
        .collect::<Vec<_>>();
    profiles.sort_unstable();
    profiles.dedup();
    let report = RealBenchmarkReport {
        schema_version: SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id.clone(),
        corpus_provider: format!("{:?}", manifest.provider),
        corpus_manifest_documents: manifest.documents.len(),
        indexed_documents: documents.len(),
        source_documents: source_count,
        queries: query_count,
        pairs: query_count.saturating_mul(documents.len()),
        seed: command.seed,
        profiles,
        build,
        methods,
    };

    if let Some(path) = &command.output {
        atomic_write(path, &serde_json::to_vec_pretty(&report)?)?;
    }
    if let Some(path) = &command.scores_output {
        write_scores(path, &scored_pairs)?;
    }
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    enforce_gates(&command, &report)?;
    Ok(())
}

fn validate_command(command: &Cli) -> BenchResult<()> {
    if command.maximum_documents < 2
        || command.source_documents == 0
        || command.queries_per_document == 0
        || command.queries_per_document > MAX_QUERY_PROFILES
        || command.passage_words < 24
    {
        return Err(invalid_input(
            "documents, source count, query profiles, or passage length are outside safe bounds",
        ));
    }
    for (name, value) in [
        ("minimum_hybrid_auprc", command.minimum_hybrid_auprc),
        (
            "minimum_hybrid_recall_at_1",
            command.minimum_hybrid_recall_at_1,
        ),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return Err(invalid_input(format!("{name} must lie in [0, 1]")));
        }
    }
    if command
        .require_hybrid_auprc_delta
        .is_some_and(|value| !value.is_finite())
        || command
            .maximum_hybrid_p95_ms
            .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(invalid_input("benchmark gates must be finite"));
    }
    Ok(())
}

fn prepare_corpus(command: &Cli) -> BenchResult<CorpusManifest> {
    match command.provider {
        CorpusSourceArg::Existing => Ok(CorpusManifest::load(&command.corpus_root)?),
        CorpusSourceArg::Gutenberg => {
            let mirror = command
                .gutenberg_mirror
                .clone()
                .or_else(|| std::env::var("GUTENBERG_MIRROR").ok())
                .unwrap_or_else(|| fo_corpus::DEFAULT_GUTENBERG_MIRROR.to_owned());
            let report = fetch_gutenberg(GutenbergOptions {
                output_dir: command.corpus_root.clone(),
                preset: command.preset.into(),
                document_limit: command.provider_items,
                mirror_base: mirror,
                overwrite: command.refresh_downloads,
                refresh_catalog: command.refresh_downloads,
                seed: command.seed,
                ..GutenbergOptions::default()
            })?;
            Ok(report.manifest)
        }
        CorpusSourceArg::Sec10k => {
            let user_agent = command
                .sec_user_agent
                .clone()
                .or_else(|| std::env::var("SEC_USER_AGENT").ok())
                .unwrap_or_default();
            let report = fetch_sec_10k(Sec10KOptions {
                output_dir: command.corpus_root.clone(),
                preset: command.preset.into(),
                sampled_companies: command.provider_items,
                filings_per_company: command.sec_filings_per_company,
                user_agent,
                overwrite: command.refresh_downloads,
                seed: command.seed,
                ..Sec10KOptions::default()
            })?;
            Ok(report.manifest)
        }
    }
}

fn load_documents(
    root: &Path,
    manifest: &CorpusManifest,
    maximum_documents: usize,
    passage_words: usize,
    seed: u64,
) -> BenchResult<Vec<BenchmarkDocument>> {
    let mut candidates = manifest.documents.iter().collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|document| stable_hash(&document.id, seed));
    let mut documents = Vec::new();
    for document in candidates {
        if documents.len() >= maximum_documents {
            break;
        }
        validate_relative_path(&document.relative_path)?;
        let path = root.join(&document.relative_path);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(_) => continue,
        };
        let words = tokenize_words(&body);
        if words.len() < passage_words.saturating_mul(2) {
            continue;
        }
        let mut tags = Vec::new();
        if let Some(language) = &document.language {
            tags.push(language.clone());
        }
        for key in ["form", "tickers", "subjects"] {
            if let Some(value) = document.metadata.get(key) {
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
        documents.push(BenchmarkDocument {
            external_id: document.id.clone(),
            title: document.title.clone(),
            body,
            words,
            tags,
            metadata: document.metadata.clone(),
        });
    }
    documents.sort_unstable_by(|left, right| left.external_id.cmp(&right.external_id));
    Ok(documents)
}

fn generate_queries(
    documents: &[BenchmarkDocument],
    source_count: usize,
    queries_per_document: usize,
    passage_words: usize,
    seed: u64,
) -> Vec<GeneratedQuery> {
    let mut source_indices = (0..documents.len()).collect::<Vec<_>>();
    source_indices.sort_unstable_by_key(|&index| {
        stable_hash(&documents[index].external_id, seed ^ 0xa5a5_a5a5_a5a5_a5a5)
    });
    source_indices.truncate(source_count);
    let mut queries = Vec::with_capacity(source_count.saturating_mul(queries_per_document));
    for source_document in source_indices {
        let document = &documents[source_document];
        for profile_index in 0..queries_per_document {
            let profile = MutationProfile::from_index(profile_index);
            let mut rng = DeterministicRng::new(stable_hash(
                &document.external_id,
                seed ^ profile_index as u64,
            ));
            let maximum_start = document.words.len().saturating_sub(passage_words);
            let start = if maximum_start == 0 {
                0
            } else {
                rng.range(maximum_start + 1)
            };
            let passage = &document.words[start..start + passage_words];
            let text = mutate_passage(passage, profile, &mut rng);
            queries.push(GeneratedQuery {
                id: format!("{}/{}", document.external_id, profile.name()),
                source_document,
                profile: profile.name(),
                text,
            });
        }
    }
    queries
}

#[derive(Debug, Clone, Copy)]
enum MutationProfile {
    Exact,
    Formatting,
    Substitution,
    InsertDelete,
    Ocr,
    Fragmented,
    Reordered,
    Keywords,
}

impl MutationProfile {
    const fn from_index(index: usize) -> Self {
        match index % MAX_QUERY_PROFILES {
            0 => Self::Exact,
            1 => Self::Formatting,
            2 => Self::Substitution,
            3 => Self::InsertDelete,
            4 => Self::Ocr,
            5 => Self::Fragmented,
            6 => Self::Reordered,
            _ => Self::Keywords,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Exact => "exact_passage",
            Self::Formatting => "formatting_case",
            Self::Substitution => "word_substitution",
            Self::InsertDelete => "insertion_deletion",
            Self::Ocr => "ocr_noise",
            Self::Fragmented => "fragmented_context",
            Self::Reordered => "reordered_blocks",
            Self::Keywords => "keyword_query",
        }
    }
}

fn mutate_passage(
    passage: &[String],
    profile: MutationProfile,
    rng: &mut DeterministicRng,
) -> String {
    match profile {
        MutationProfile::Exact => passage.join(" "),
        MutationProfile::Formatting => passage
            .iter()
            .enumerate()
            .map(|(index, word)| {
                let rendered = if index % 9 == 0 {
                    word.to_ascii_uppercase()
                } else {
                    word.clone()
                };
                if index % 13 == 0 {
                    format!("{rendered},\n")
                } else if index % 7 == 0 {
                    format!("{rendered};")
                } else {
                    rendered
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        MutationProfile::Substitution => passage
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
        MutationProfile::InsertDelete => {
            let mut words = Vec::new();
            for (index, word) in passage.iter().enumerate() {
                if index % 11 == 5 {
                    continue;
                }
                words.push(word.clone());
                if index % 13 == 7 {
                    words.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
                    words.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
                }
            }
            words.join(" ")
        }
        MutationProfile::Ocr => passage
            .join(" ")
            .chars()
            .enumerate()
            .map(|(index, character)| {
                if index % 29 != 11 {
                    return character;
                }
                match character {
                    'o' | 'O' => '0',
                    'l' | 'I' => '1',
                    'e' | 'E' => 'c',
                    'm' | 'M' => 'n',
                    _ => character,
                }
            })
            .collect(),
        MutationProfile::Fragmented => {
            let third = passage.len() / 3;
            let mut words = passage[..third].to_vec();
            for _ in 0..24 {
                words.push(NOISE_WORDS[rng.range(NOISE_WORDS.len())].to_owned());
            }
            words.extend_from_slice(&passage[passage.len() - third..]);
            words.join(" ")
        }
        MutationProfile::Reordered => {
            let third = passage.len() / 3;
            let mut words = passage[third..third * 2].to_vec();
            words.extend_from_slice(&passage[..third]);
            words.extend_from_slice(&passage[third * 2..]);
            words.join(" ")
        }
        MutationProfile::Keywords => {
            let mut selected = BTreeSet::new();
            let stride = (passage.len() / 12).max(1);
            for index in (0..passage.len()).step_by(stride) {
                let word =
                    passage[index].trim_matches(|character: char| !character.is_alphanumeric());
                if word.chars().count() >= 4 {
                    selected.insert(word.to_owned());
                }
                if selected.len() >= 10 {
                    break;
                }
            }
            if selected.is_empty() {
                passage
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                selected.into_iter().collect::<Vec<_>>().join(" ")
            }
        }
    }
}

fn persist_and_measure_index(
    index: &HybridIndex,
    requested: Option<&Path>,
) -> BenchResult<(f64, u64)> {
    let destination = requested.map_or_else(
        || {
            std::env::temp_dir().join(format!(
                "franken-overlap-real-bench-{}-{}",
                std::process::id(),
                unix_nanos()
            ))
        },
        Path::to_path_buf,
    );
    let started = Instant::now();
    index.save(&destination)?;
    let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
    let bytes = directory_bytes(&destination)?;
    if requested.is_none() {
        fs::remove_dir_all(&destination).ok();
    }
    Ok((elapsed, bytes))
}

fn directory_bytes(path: &Path) -> io::Result<u64> {
    let metadata = fs::metadata(path)?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        total = total.saturating_add(directory_bytes(&entry?.path())?);
    }
    Ok(total)
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

fn simhash(features: &HashSet<Fingerprint>) -> u64 {
    let mut accumulators = [0i64; 64];
    for feature in features {
        let value = feature.hi ^ feature.lo.rotate_left(17);
        for (bit, accumulator) in accumulators.iter_mut().enumerate() {
            if value & (1u64 << bit) == 0 {
                *accumulator -= 1;
            } else {
                *accumulator += 1;
            }
        }
    }
    accumulators
        .iter()
        .enumerate()
        .fold(0u64, |value, (bit, accumulator)| {
            if *accumulator >= 0 {
                value | (1u64 << bit)
            } else {
                value
            }
        })
}

fn document_index(documents: &[BenchmarkDocument], external_id: &str) -> Option<usize> {
    documents
        .binary_search_by(|document| document.external_id.as_str().cmp(external_id))
        .ok()
}

fn evaluation_options() -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    }
}

fn metric_at(report: &GroupedEvaluationReport, k: usize) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == k)
        .map_or(0.0, |metric| metric.value)
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

fn print_report(report: &RealBenchmarkReport) {
    println!("Corpus:             {}", report.corpus_id);
    println!("Provider:           {}", report.corpus_provider);
    println!("Indexed documents:  {}", report.indexed_documents);
    println!("Source documents:   {}", report.source_documents);
    println!("Queries:             {}", report.queries);
    println!("Pairs:               {}", report.pairs);
    println!("Build ms:            {:.3}", report.build.build_ms);
    println!("Serialization ms:    {:.3}", report.build.serialization_ms);
    println!("Index bytes:         {}", report.build.index_bytes);
    println!();
    for method in &report.methods {
        println!("{}", method.name);
        println!(
            "  AUPRC micro/macro: {:.6} / {:.6}",
            method.quality.micro.average_precision, method.quality.macro_average_precision
        );
        println!(
            "  Recall@1 / MRR:    {:.6} / {:.6}",
            metric_at(&method.quality, 1),
            method.quality.mean_reciprocal_rank
        );
        println!(
            "  latency p50/p95/p99:{:.3} / {:.3} / {:.3} ms",
            method.p50_ms, method.p95_ms, method.p99_ms
        );
        println!("  queries/sec:       {:.3}", method.queries_per_second);
        println!(
            "  false positives/q: {:.6}",
            method.false_positives_per_query_at_best_f1
        );
    }
}

fn enforce_gates(command: &Cli, report: &RealBenchmarkReport) -> BenchResult<()> {
    let hybrid = report
        .methods
        .iter()
        .find(|method| method.name == "franken_hybrid")
        .ok_or_else(|| invalid_input("benchmark produced no hybrid method"))?;
    if command
        .minimum_hybrid_auprc
        .is_some_and(|minimum| hybrid.quality.micro.average_precision < minimum)
    {
        return Err(invalid_input(format!(
            "hybrid AUPRC {:.6} is below required {:.6}",
            hybrid.quality.micro.average_precision,
            command.minimum_hybrid_auprc.unwrap_or_default()
        )));
    }
    if command
        .minimum_hybrid_recall_at_1
        .is_some_and(|minimum| metric_at(&hybrid.quality, 1) < minimum)
    {
        return Err(invalid_input(format!(
            "hybrid Recall@1 {:.6} is below required {:.6}",
            metric_at(&hybrid.quality, 1),
            command.minimum_hybrid_recall_at_1.unwrap_or_default()
        )));
    }
    if command
        .maximum_hybrid_p95_ms
        .is_some_and(|maximum| hybrid.p95_ms > maximum)
    {
        return Err(invalid_input(format!(
            "hybrid p95 {:.3} ms exceeds required {:.3} ms",
            hybrid.p95_ms,
            command.maximum_hybrid_p95_ms.unwrap_or_default()
        )));
    }
    if let Some(required_delta) = command.require_hybrid_auprc_delta {
        let best_baseline = report
            .methods
            .iter()
            .filter(|method| method.name != "franken_hybrid")
            .map(|method| method.quality.micro.average_precision)
            .fold(0.0f64, f64::max);
        let delta = hybrid.quality.micro.average_precision - best_baseline;
        if delta < required_delta {
            return Err(invalid_input(format!(
                "hybrid AUPRC delta {delta:+.6} is below required {required_delta:+.6}"
            )));
        }
    }
    Ok(())
}

fn write_scores(path: &Path, scores: &[ScoredPair]) -> BenchResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("jsonl"),
        std::process::id()
    ));
    let file = File::create(&temporary)?;
    let mut writer = BufWriter::new(file);
    for score in scores {
        serde_json::to_writer(&mut writer, score)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.unicode_words()
        .map(|word| word.to_lowercase())
        .filter(|word| !word.is_empty())
        .collect()
}

fn validate_relative_path(value: &str) -> BenchResult<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid_input(format!(
            "unsafe corpus relative path {value:?}"
        )));
    }
    Ok(())
}

fn stable_hash(value: &str, seed: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    avalanche(hash)
}

fn avalanche(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next() % upper as u64) as usize
        }
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
