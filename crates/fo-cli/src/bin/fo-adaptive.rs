#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use fo_core::{
    Fingerprint, Index, SearchIntent, SearchOptions, SearchResult, normalize, qgram_hashes, winnow,
};
use serde::Serialize;

const SHIFTED_DIAGONAL_GRIDS: u128 = 2;
type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "fo-adaptive",
    version,
    about = "Analyze query cost and execute an adaptive FrankenOverlap search"
)]
struct Cli {
    index: PathBuf,
    specimen: Option<PathBuf>,
    #[arg(long, conflicts_with = "specimen")]
    text: Option<String>,
    #[arg(long, value_enum, default_value = "source-attribution")]
    intent: SearchIntentArg,
    #[arg(short, long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = 200)]
    candidates: usize,
    #[arg(long, default_value_t = 50_000)]
    max_postings: usize,
    /// Maximum estimated posting/query-position products before feature suppression.
    #[arg(long, default_value_t = 5_000_000)]
    vote_pair_budget: u64,
    #[arg(long, default_value_t = 0.35)]
    minimum_similarity: f32,
    #[arg(long, default_value_t = 24)]
    minimum_matched_tokens: usize,
    #[arg(long, default_value_t = 0.10)]
    minimum_query_coverage: f32,
    #[arg(long, default_value_t = 0.10)]
    minimum_source_coverage: f32,
    /// Analyze and print the plan without executing retrieval.
    #[arg(long)]
    plan_only: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchIntentArg {
    AnyPassage,
    SourceAttribution,
    NearDuplicate,
}

impl From<SearchIntentArg> for SearchIntent {
    fn from(value: SearchIntentArg) -> Self {
        match value {
            SearchIntentArg::AnyPassage => Self::AnyPassage,
            SearchIntentArg::SourceAttribution => Self::SourceAttribution,
            SearchIntentArg::NearDuplicate => Self::NearDuplicate,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlannedRoute {
    ShortBitParallel,
    SparseBalanced,
    SparseSelective,
    SparseLowEntropy,
    CompositeRecommended,
    MultiViewRecommended,
    DirectFallbackLikely,
}

#[derive(Debug, Clone, Serialize)]
struct PostingDistribution {
    minimum: usize,
    median: usize,
    percentile_95: usize,
    maximum: usize,
}

#[derive(Debug, Clone, Serialize)]
struct EffectiveSearchOptions {
    intent: SearchIntent,
    max_results: usize,
    max_candidates: usize,
    max_postings_per_feature: usize,
    minimum_anchor_hits: u32,
    minimum_matched_tokens: usize,
    minimum_query_coverage: f32,
    minimum_source_coverage: f32,
    minimum_similarity: f32,
}

impl EffectiveSearchOptions {
    fn into_search_options(self) -> SearchOptions {
        SearchOptions {
            intent: self.intent,
            max_results: self.max_results,
            max_candidates: self.max_candidates,
            max_postings_per_feature: self.max_postings_per_feature,
            minimum_anchor_hits: self.minimum_anchor_hits,
            minimum_matched_tokens: self.minimum_matched_tokens,
            minimum_query_coverage: self.minimum_query_coverage,
            minimum_source_coverage: self.minimum_source_coverage,
            minimum_similarity: self.minimum_similarity,
            ..SearchOptions::default()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct QueryPlan {
    route: PlannedRoute,
    normalized_tokens: usize,
    unique_tokens: usize,
    token_entropy_bits: f64,
    repeated_token_fraction: f64,
    qgram_count: usize,
    selected_feature_occurrences: usize,
    unique_selected_features: usize,
    matched_feature_occurrences: usize,
    unmatched_feature_occurrences: usize,
    suppressed_feature_occurrences: usize,
    posting_distribution: PostingDistribution,
    estimated_vote_pairs_before_suppression: u64,
    estimated_vote_pairs_after_suppression: u64,
    estimated_posting_bytes_after_suppression: u64,
    effective: EffectiveSearchOptions,
    rationale: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AdaptiveSearchReport {
    plan: QueryPlan,
    results: Vec<SearchResult>,
}

#[derive(Debug)]
struct FeatureCost {
    posting_count: usize,
    multiplicity: usize,
    vote_pairs: u128,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-adaptive: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    validate_cli(&command)?;
    let specimen = specimen_text(command.specimen.as_deref(), command.text.clone())?;
    let index = Index::load(&command.index)?;
    let plan = build_plan(&index, &specimen, &command)?;
    if command.plan_only {
        print_plan(&plan, command.json)?;
        return Ok(());
    }
    let results = index.search(&specimen, &plan.effective.clone().into_search_options())?;
    if command.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&AdaptiveSearchReport { plan, results })?
        );
    } else {
        print_plan(&plan, false)?;
        print_results(&results);
    }
    Ok(())
}

fn validate_cli(command: &Cli) -> CliResult<()> {
    if command.max_postings == 0 || command.vote_pair_budget == 0 {
        return Err(invalid_input(
            "--max-postings and --vote-pair-budget must be positive",
        ));
    }
    if command.limit == 0 || command.candidates == 0 {
        return Err(invalid_input("--limit and --candidates must be positive"));
    }
    for (name, value) in [
        ("--minimum-similarity", command.minimum_similarity),
        ("--minimum-query-coverage", command.minimum_query_coverage),
        ("--minimum-source-coverage", command.minimum_source_coverage),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid_input(format!("{name} must lie in [0, 1]")));
        }
    }
    Ok(())
}

fn build_plan(index: &Index, specimen: &str, command: &Cli) -> CliResult<QueryPlan> {
    let normalized = normalize(specimen, &index.config.normalization);
    if normalized.is_empty() {
        return Err(invalid_input("specimen is empty after normalization"));
    }
    let (unique_tokens, token_entropy_bits) = token_statistics(&normalized.tokens);
    let repeated_token_fraction = if normalized.tokens.is_empty() {
        0.0
    } else {
        1.0 - unique_tokens as f64 / normalized.tokens.len() as f64
    };

    let hashes = if normalized.tokens.len() >= index.config.qgram_size {
        qgram_hashes(&normalized.tokens, index.config.qgram_size)?
    } else {
        Vec::new()
    };
    let selected = winnow(&hashes, index.config.winnow_window);
    let mut multiplicities = BTreeMap::<Fingerprint, usize>::new();
    for feature in &selected {
        *multiplicities.entry(feature.fingerprint).or_default() += 1;
    }

    let mut costs = Vec::new();
    let mut matched_feature_occurrences = 0usize;
    for (fingerprint, multiplicity) in multiplicities {
        let Some(entry) = index.lookup(fingerprint) else {
            continue;
        };
        matched_feature_occurrences = matched_feature_occurrences.saturating_add(multiplicity);
        let vote_pairs = (entry.postings.len() as u128)
            .saturating_mul(multiplicity as u128)
            .saturating_mul(SHIFTED_DIAGONAL_GRIDS);
        costs.push(FeatureCost {
            posting_count: entry.postings.len(),
            multiplicity,
            vote_pairs,
        });
    }
    costs.sort_unstable_by_key(|cost| cost.posting_count);
    let posting_distribution = posting_distribution(&costs);
    let estimated_before = costs
        .iter()
        .filter(|cost| cost.posting_count <= command.max_postings)
        .map(|cost| cost.vote_pairs)
        .sum::<u128>();
    let adaptive_cap = adaptive_posting_cap(
        &costs,
        command.max_postings,
        u128::from(command.vote_pair_budget),
        SearchOptions::default().minimum_anchor_hits as usize,
    );
    let estimated_after = costs
        .iter()
        .filter(|cost| cost.posting_count <= adaptive_cap)
        .map(|cost| cost.vote_pairs)
        .sum::<u128>();
    let retained_occurrences = costs
        .iter()
        .filter(|cost| cost.posting_count <= adaptive_cap)
        .map(|cost| cost.multiplicity)
        .sum::<usize>();
    let suppressed_feature_occurrences = matched_feature_occurrences
        .saturating_sub(retained_occurrences);
    let unmatched_feature_occurrences = selected
        .len()
        .saturating_sub(matched_feature_occurrences);

    let low_entropy = token_entropy_bits < 2.25 || repeated_token_fraction > 0.72;
    let over_budget = estimated_before > u128::from(command.vote_pair_budget);
    let query_is_short = normalized.tokens.len() <= 64;
    let sparse_evidence_is_thin = retained_occurrences < 2;
    let intent: SearchIntent = command.intent.into();

    let route = if query_is_short {
        PlannedRoute::ShortBitParallel
    } else if sparse_evidence_is_thin {
        PlannedRoute::DirectFallbackLikely
    } else if low_entropy {
        PlannedRoute::SparseLowEntropy
    } else if over_budget {
        PlannedRoute::SparseSelective
    } else if normalized.tokens.len() >= 1_024 && intent == SearchIntent::SourceAttribution {
        PlannedRoute::CompositeRecommended
    } else if unmatched_feature_occurrences > matched_feature_occurrences
        && selected.len() >= 8
    {
        PlannedRoute::MultiViewRecommended
    } else {
        PlannedRoute::SparseBalanced
    };

    let mut minimum_anchor_hits = SearchOptions::default().minimum_anchor_hits;
    if low_entropy && retained_occurrences >= 4 {
        minimum_anchor_hits = minimum_anchor_hits.max(4);
    } else if retained_occurrences >= 3 {
        minimum_anchor_hits = minimum_anchor_hits.max(3);
    }
    minimum_anchor_hits = minimum_anchor_hits.min(retained_occurrences.max(1) as u32);

    let mut minimum_matched_tokens = command
        .minimum_matched_tokens
        .min(normalized.tokens.len())
        .max(index.config.qgram_size.min(normalized.tokens.len()));
    if low_entropy {
        minimum_matched_tokens = minimum_matched_tokens
            .max((normalized.tokens.len() / 8).clamp(24, 96))
            .min(normalized.tokens.len());
    }
    let mut minimum_query_coverage = command.minimum_query_coverage;
    if intent == SearchIntent::SourceAttribution && normalized.tokens.len() >= 512 {
        minimum_query_coverage = minimum_query_coverage.max(0.15);
    }
    if intent == SearchIntent::NearDuplicate {
        minimum_query_coverage = minimum_query_coverage.max(0.50);
    }
    let minimum_source_coverage = if intent == SearchIntent::NearDuplicate {
        command.minimum_source_coverage.max(0.50)
    } else {
        command.minimum_source_coverage
    };
    let max_candidates = if over_budget || low_entropy {
        command.candidates.min(128).max(command.limit)
    } else {
        command.candidates
    };

    let effective = EffectiveSearchOptions {
        intent,
        max_results: command.limit,
        max_candidates,
        max_postings_per_feature: adaptive_cap.max(1),
        minimum_anchor_hits,
        minimum_matched_tokens,
        minimum_query_coverage,
        minimum_source_coverage,
        minimum_similarity: command.minimum_similarity,
    };

    let mut rationale = Vec::new();
    if query_is_short {
        rationale.push(
            "the normalized specimen fits the exact Myers bit-vector path".to_owned(),
        );
    }
    if over_budget {
        rationale.push(format!(
            "estimated sparse voting work exceeded the {}-pair budget; the posting cap was reduced from {} to {}",
            command.vote_pair_budget, command.max_postings, adaptive_cap
        ));
    }
    if low_entropy {
        rationale.push(format!(
            "token entropy is {:.3} bits and repetition is {:.1}%; stronger anchor and matched-length evidence is required",
            token_entropy_bits,
            repeated_token_fraction * 100.0
        ));
    }
    if sparse_evidence_is_thin {
        rationale.push(
            "fewer than two retained indexed feature occurrences remain, so direct fallback is likely"
                .to_owned(),
        );
    }
    if matches!(route, PlannedRoute::CompositeRecommended) {
        rationale.push(
            "the long source-attribution specimen may benefit from fragmented/reordered block aggregation"
                .to_owned(),
        );
    }
    if matches!(route, PlannedRoute::MultiViewRecommended) {
        rationale.push(
            "most selected features are absent from this scale; multi-view short/long q-gram consensus is recommended"
                .to_owned(),
        );
    }
    if rationale.is_empty() {
        rationale.push("the ordinary sparse indexed route fits the declared work budget".to_owned());
    }

    Ok(QueryPlan {
        route,
        normalized_tokens: normalized.tokens.len(),
        unique_tokens,
        token_entropy_bits,
        repeated_token_fraction,
        qgram_count: hashes.len(),
        selected_feature_occurrences: selected.len(),
        unique_selected_features: costs.len(),
        matched_feature_occurrences,
        unmatched_feature_occurrences,
        suppressed_feature_occurrences,
        posting_distribution,
        estimated_vote_pairs_before_suppression: saturating_u64(estimated_before),
        estimated_vote_pairs_after_suppression: saturating_u64(estimated_after),
        estimated_posting_bytes_after_suppression: saturating_u64(
            estimated_after.saturating_mul(8) / SHIFTED_DIAGONAL_GRIDS,
        ),
        effective,
        rationale,
    })
}

fn adaptive_posting_cap(
    costs: &[FeatureCost],
    maximum_cap: usize,
    budget: u128,
    minimum_occurrences: usize,
) -> usize {
    if costs.is_empty() {
        return maximum_cap.max(1);
    }
    let mut accumulated = 0u128;
    let mut retained = 0usize;
    let mut cap = 0usize;
    for cost in costs
        .iter()
        .filter(|cost| cost.posting_count <= maximum_cap)
    {
        let next = accumulated.saturating_add(cost.vote_pairs);
        if next > budget && retained >= minimum_occurrences {
            break;
        }
        accumulated = next;
        retained = retained.saturating_add(cost.multiplicity);
        cap = cap.max(cost.posting_count);
    }
    if retained < minimum_occurrences {
        for cost in costs.iter().filter(|cost| cost.posting_count <= maximum_cap) {
            retained = retained.saturating_add(cost.multiplicity);
            cap = cap.max(cost.posting_count);
            if retained >= minimum_occurrences {
                break;
            }
        }
    }
    cap.max(1).min(maximum_cap.max(1))
}

fn posting_distribution(costs: &[FeatureCost]) -> PostingDistribution {
    if costs.is_empty() {
        return PostingDistribution {
            minimum: 0,
            median: 0,
            percentile_95: 0,
            maximum: 0,
        };
    }
    PostingDistribution {
        minimum: costs[0].posting_count,
        median: costs[costs.len() / 2].posting_count,
        percentile_95: costs[(costs.len() - 1) * 95 / 100].posting_count,
        maximum: costs[costs.len() - 1].posting_count,
    }
}

fn token_statistics(tokens: &[u32]) -> (usize, f64) {
    let mut counts = HashMap::<u32, usize>::new();
    for &token in tokens {
        *counts.entry(token).or_default() += 1;
    }
    let length = tokens.len().max(1) as f64;
    let entropy = counts
        .values()
        .map(|&count| {
            let probability = count as f64 / length;
            -probability * probability.log2()
        })
        .sum();
    (counts.len(), entropy)
}

fn saturating_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn print_plan(plan: &QueryPlan, json: bool) -> CliResult<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
        return Ok(());
    }
    println!("Route:                         {:?}", plan.route);
    println!("Normalized tokens:             {}", plan.normalized_tokens);
    println!("Token entropy:                 {:.4} bits", plan.token_entropy_bits);
    println!(
        "Repeated-token fraction:       {:.2}%",
        plan.repeated_token_fraction * 100.0
    );
    println!(
        "Selected/matched/suppressed:   {}/{}/{}",
        plan.selected_feature_occurrences,
        plan.matched_feature_occurrences,
        plan.suppressed_feature_occurrences
    );
    println!(
        "Posting min/median/p95/max:    {}/{}/{}/{}",
        plan.posting_distribution.minimum,
        plan.posting_distribution.median,
        plan.posting_distribution.percentile_95,
        plan.posting_distribution.maximum
    );
    println!(
        "Estimated vote pairs:          {} -> {}",
        plan.estimated_vote_pairs_before_suppression,
        plan.estimated_vote_pairs_after_suppression
    );
    println!(
        "Effective posting cap:         {}",
        plan.effective.max_postings_per_feature
    );
    println!(
        "Effective anchor/match floors: {}/{}",
        plan.effective.minimum_anchor_hits, plan.effective.minimum_matched_tokens
    );
    for reason in &plan.rationale {
        println!("  - {reason}");
    }
    Ok(())
}

fn print_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("No match met the adaptive search thresholds.");
        return;
    }
    for (rank, result) in results.iter().enumerate() {
        println!(
            "{}. {} [{}..{}] score={:.4} edit={:.4} query={:.4} tokens={}",
            rank + 1,
            result.path,
            result.corpus_start,
            result.corpus_end,
            result.combined_score,
            result.edit_similarity,
            result.query_coverage,
            result.matched_tokens,
        );
        println!("   {}", one_line(&result.matched_text, 240));
    }
}

fn specimen_text(path: Option<&Path>, inline: Option<String>) -> CliResult<String> {
    match (path, inline) {
        (Some(path), None) => Ok(fs::read_to_string(path)?),
        (None, Some(text)) => Ok(text),
        (None, None) => Err(invalid_input("provide a specimen file or --text")),
        (Some(_), Some(_)) => Err(invalid_input(
            "specimen file and --text are mutually exclusive",
        )),
    }
}

fn one_line(value: &str, maximum_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_chars {
        return compact;
    }
    let mut output = compact.chars().take(maximum_chars).collect::<String>();
    output.push('…');
    output
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use fo_core::{IndexBuilder, IndexConfig};

    use super::{Cli, PlannedRoute, build_plan};

    fn command() -> Cli {
        Cli {
            index: "unused".into(),
            specimen: None,
            text: None,
            intent: super::SearchIntentArg::SourceAttribution,
            limit: 20,
            candidates: 200,
            max_postings: 50_000,
            vote_pair_budget: 10_000,
            minimum_similarity: 0.10,
            minimum_matched_tokens: 8,
            minimum_query_coverage: 0.10,
            minimum_source_coverage: 0.10,
            plan_only: false,
            json: false,
        }
    }

    #[test]
    fn ordinary_rare_query_stays_on_sparse_route() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source",
                "before dawn the observatory opened copper shutters and calibrated every detector",
            )
            .expect("source");
        builder
            .add_document(
                "noise",
                "winter vegetables railway timetables ceramics orchards municipal budgets",
            )
            .expect("noise");
        let index = builder.build().expect("index");
        let plan = build_plan(
            &index,
            "the observatory opened copper shutters and calibrated every detector",
            &command(),
        )
        .expect("plan");
        assert!(matches!(
            plan.route,
            PlannedRoute::SparseBalanced | PlannedRoute::ShortBitParallel
        ));
        assert!(plan.estimated_vote_pairs_after_suppression <= 10_000);
    }

    #[test]
    fn repetitive_query_is_classified_as_low_entropy() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..32 {
            builder
                .add_document(
                    format!("doc-{index}"),
                    "alpha alpha alpha alpha beta alpha alpha alpha alpha beta",
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let plan = build_plan(
            &index,
            "alpha alpha alpha alpha alpha alpha alpha alpha alpha alpha alpha alpha",
            &command(),
        )
        .expect("plan");
        assert!(matches!(
            plan.route,
            PlannedRoute::SparseLowEntropy | PlannedRoute::DirectFallbackLikely
        ));
        assert!(plan.repeated_token_fraction > 0.70);
    }
}
