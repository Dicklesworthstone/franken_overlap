use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    CompositeSearchOptions, CompositeSearchResult, Fingerprint, FoError, Index, Result,
    SearchIntent, SearchOptions, SearchResult, normalize, qgram_hashes, winnow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptiveRoute {
    ShortDirect,
    Sparse,
    Composite,
    BoundedSparse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryAdvisory {
    LowEntropy,
    HighRepetition,
    ManyMissingFeatures,
    HeavyFeaturesSuppressed,
    SparseBudgetExceeded,
    MultiViewRecommended,
    DenseScanRecommended,
    CompositeRecommended,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueryPlannerOptions {
    pub maximum_sparse_posting_pairs: u64,
    pub composite_minimum_tokens: usize,
    pub composite_retained_fraction: f32,
    pub composite_repetition_fraction: f32,
    pub low_entropy_ratio: f32,
    pub missing_feature_fraction: f32,
    pub heavy_feature_fraction: f32,
}

impl Default for QueryPlannerOptions {
    fn default() -> Self {
        Self {
            maximum_sparse_posting_pairs: 25_000_000,
            composite_minimum_tokens: 256,
            composite_retained_fraction: 0.55,
            composite_repetition_fraction: 0.25,
            low_entropy_ratio: 0.38,
            missing_feature_fraction: 0.35,
            heavy_feature_fraction: 0.20,
        }
    }
}

impl QueryPlannerOptions {
    pub fn validate(self) -> Result<Self> {
        if self.maximum_sparse_posting_pairs == 0 || self.composite_minimum_tokens == 0 {
            return Err(FoError::InvalidConfig(
                "planner work and composite-length limits must be positive".to_owned(),
            ));
        }
        for (name, value) in [
            (
                "composite_retained_fraction",
                self.composite_retained_fraction,
            ),
            (
                "composite_repetition_fraction",
                self.composite_repetition_fraction,
            ),
            ("low_entropy_ratio", self.low_entropy_ratio),
            ("missing_feature_fraction", self.missing_feature_fraction),
            ("heavy_feature_fraction", self.heavy_feature_fraction),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "planner {name} must be finite and lie in [0, 1]"
                )));
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    pub route: AdaptiveRoute,
    pub advisories: Vec<QueryAdvisory>,
    pub normalized_tokens: usize,
    pub distinct_tokens: usize,
    pub token_entropy_bits: f64,
    pub token_entropy_ratio: f64,
    pub repetition_fraction: f64,
    pub qgrams: usize,
    pub selected_features: usize,
    pub distinct_selected_features: usize,
    pub retained_features: usize,
    pub missing_features: usize,
    pub suppressed_features: usize,
    pub retained_fraction: f64,
    pub missing_fraction: f64,
    pub suppressed_fraction: f64,
    pub estimated_posting_pairs: u64,
    pub estimated_diagonal_votes: u64,
    pub sparse_budget_exceeded: bool,
    pub maximum_posting_list: usize,
    pub mean_retained_posting_list: f64,
    pub suggested_max_postings_per_feature: usize,
    pub corpus_documents: usize,
    pub corpus_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "match", rename_all = "snake_case")]
pub enum AdaptiveMatch {
    Passage(SearchResult),
    Composite(CompositeSearchResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveSearchReport {
    pub plan: QueryPlan,
    pub effective_max_postings_per_feature: usize,
    pub matches: Vec<AdaptiveMatch>,
}

impl Index {
    pub fn plan_query(
        &self,
        specimen: &str,
        search: &SearchOptions,
        planner: QueryPlannerOptions,
    ) -> Result<QueryPlan> {
        search.validate()?;
        let planner = planner.validate()?;
        let query = normalize(specimen, &self.config.normalization);
        if query.is_empty() {
            return Err(FoError::EmptySpecimen);
        }

        // BTreeMap: entropy is a float sum, so iteration order must be
        // deterministic or identical queries produce ULP-different plans.
        let mut token_counts = BTreeMap::<u32, usize>::new();
        for &token in &query.tokens {
            *token_counts.entry(token).or_default() += 1;
        }
        let distinct_tokens = token_counts.len();
        let token_entropy_bits = shannon_entropy(&token_counts, query.len());
        let token_entropy_ratio = sequence_entropy_ratio(token_entropy_bits, query.len());
        let repetition_fraction =
            (1.0 - distinct_tokens as f64 / query.len().max(1) as f64).clamp(0.0, 1.0);

        let hashes = qgram_hashes(&query.tokens, self.config.qgram_size)?;
        let selected = winnow(&hashes, self.config.winnow_window);
        let mut occurrences = BTreeMap::<Fingerprint, usize>::new();
        for feature in &selected {
            *occurrences.entry(feature.fingerprint).or_default() += 1;
        }

        let mut retained_features = 0usize;
        let mut missing_features = 0usize;
        let mut suppressed_features = 0usize;
        let mut estimated_posting_pairs = 0u64;
        let mut maximum_posting_list = 0usize;
        let mut retained_posting_sum = 0u128;
        let mut retained_distinct = 0usize;

        for (fingerprint, query_occurrences) in &occurrences {
            let Some(entry) = self.lookup(*fingerprint) else {
                missing_features = missing_features.saturating_add(*query_occurrences);
                continue;
            };
            maximum_posting_list = maximum_posting_list.max(entry.postings.len());
            if entry.postings.len() > search.max_postings_per_feature {
                suppressed_features = suppressed_features.saturating_add(*query_occurrences);
                continue;
            }
            retained_features = retained_features.saturating_add(*query_occurrences);
            retained_distinct = retained_distinct.saturating_add(1);
            retained_posting_sum =
                retained_posting_sum.saturating_add(entry.postings.len() as u128);
            let pairs = (entry.postings.len() as u128)
                .saturating_mul(*query_occurrences as u128)
                .min(u64::MAX as u128) as u64;
            estimated_posting_pairs = estimated_posting_pairs.saturating_add(pairs);
        }

        let selected_count = selected.len();
        let retained_fraction = fraction(retained_features, selected_count);
        let missing_fraction = fraction(missing_features, selected_count);
        let suppressed_fraction = fraction(suppressed_features, selected_count);
        let estimated_diagonal_votes = estimated_posting_pairs.saturating_mul(2);
        let mean_retained_posting_list = if retained_distinct == 0 {
            0.0
        } else {
            retained_posting_sum as f64 / retained_distinct as f64
        };
        let suggested_max_postings_per_feature = if retained_features == 0 {
            search.max_postings_per_feature
        } else {
            usize::try_from(
                planner
                    .maximum_sparse_posting_pairs
                    .saturating_div(retained_features as u64),
            )
            .unwrap_or(usize::MAX)
            .clamp(1, search.max_postings_per_feature)
        };

        let short = query.len() < self.config.qgram_size
            || selected_count < search.minimum_anchor_hits as usize;
        let insufficient_sparse_evidence = retained_features < search.minimum_anchor_hits as usize;
        let sparse_budget_exceeded = estimated_posting_pairs > planner.maximum_sparse_posting_pairs;
        let composite_signal = search.intent != SearchIntent::AnyPassage
            && query.len() >= planner.composite_minimum_tokens
            && (retained_fraction <= f64::from(planner.composite_retained_fraction)
                || repetition_fraction >= f64::from(planner.composite_repetition_fraction)
                || query.len() >= planner.composite_minimum_tokens.saturating_mul(4));

        let route = if short {
            AdaptiveRoute::ShortDirect
        } else if composite_signal {
            AdaptiveRoute::Composite
        } else if insufficient_sparse_evidence || sparse_budget_exceeded {
            AdaptiveRoute::BoundedSparse
        } else {
            AdaptiveRoute::Sparse
        };

        let mut advisories = Vec::new();
        if token_entropy_ratio <= f64::from(planner.low_entropy_ratio) {
            push_advisory(&mut advisories, QueryAdvisory::LowEntropy);
            push_advisory(&mut advisories, QueryAdvisory::DenseScanRecommended);
        }
        if repetition_fraction >= f64::from(planner.composite_repetition_fraction) {
            push_advisory(&mut advisories, QueryAdvisory::HighRepetition);
        }
        if missing_fraction >= f64::from(planner.missing_feature_fraction) {
            push_advisory(&mut advisories, QueryAdvisory::ManyMissingFeatures);
            push_advisory(&mut advisories, QueryAdvisory::MultiViewRecommended);
        }
        if suppressed_fraction >= f64::from(planner.heavy_feature_fraction) {
            push_advisory(&mut advisories, QueryAdvisory::HeavyFeaturesSuppressed);
            push_advisory(&mut advisories, QueryAdvisory::MultiViewRecommended);
        }
        if sparse_budget_exceeded {
            push_advisory(&mut advisories, QueryAdvisory::SparseBudgetExceeded);
            push_advisory(&mut advisories, QueryAdvisory::DenseScanRecommended);
        }
        if composite_signal {
            push_advisory(&mut advisories, QueryAdvisory::CompositeRecommended);
        }

        let stats = self.stats();
        Ok(QueryPlan {
            route,
            advisories,
            normalized_tokens: query.len(),
            distinct_tokens,
            token_entropy_bits,
            token_entropy_ratio,
            repetition_fraction,
            qgrams: hashes.len(),
            selected_features: selected_count,
            distinct_selected_features: occurrences.len(),
            retained_features,
            missing_features,
            suppressed_features,
            retained_fraction,
            missing_fraction,
            suppressed_fraction,
            estimated_posting_pairs,
            estimated_diagonal_votes,
            sparse_budget_exceeded,
            maximum_posting_list,
            mean_retained_posting_list,
            suggested_max_postings_per_feature,
            corpus_documents: stats.documents,
            corpus_tokens: stats.normalized_tokens,
        })
    }

    pub fn search_adaptive(
        &self,
        specimen: &str,
        search: &SearchOptions,
        planner: QueryPlannerOptions,
        composite: CompositeSearchOptions,
    ) -> Result<AdaptiveSearchReport> {
        let plan = self.plan_query(specimen, search, planner)?;
        let mut effective = search.clone();
        if plan.sparse_budget_exceeded || plan.route == AdaptiveRoute::BoundedSparse {
            effective.max_postings_per_feature = effective
                .max_postings_per_feature
                .min(plan.suggested_max_postings_per_feature);
        }
        let effective_max_postings_per_feature = effective.max_postings_per_feature;
        let matches = if plan.route == AdaptiveRoute::Composite {
            self.search_composite(specimen, &effective, composite)?
                .into_iter()
                .map(AdaptiveMatch::Composite)
                .collect()
        } else {
            self.search(specimen, &effective)?
                .into_iter()
                .map(AdaptiveMatch::Passage)
                .collect()
        };
        Ok(AdaptiveSearchReport {
            plan,
            effective_max_postings_per_feature,
            matches,
        })
    }
}

fn shannon_entropy(counts: &BTreeMap<u32, usize>, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|&count| {
            let probability = count as f64 / total as f64;
            -probability * probability.log2()
        })
        .sum()
}

fn sequence_entropy_ratio(entropy: f64, total: usize) -> f64 {
    if total <= 1 {
        0.0
    } else {
        (entropy / (total as f64).log2()).clamp(0.0, 1.0)
    }
}

fn fraction(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn push_advisory(advisories: &mut Vec<QueryAdvisory>, advisory: QueryAdvisory) {
    if !advisories.contains(&advisory) {
        advisories.push(advisory);
        advisories.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use super::{AdaptiveMatch, AdaptiveRoute, QueryAdvisory, QueryPlannerOptions};
    use crate::{CompositeSearchOptions, IndexBuilder, IndexConfig, SearchOptions};

    fn index() -> crate::Index {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source",
                concat!(
                    "the observatory opened its copper shutters before dawn and recorded a signal ",
                    "the team checked every instrument and published all raw observations ",
                    "an unrelated middle passage discusses architecture and winter vegetables ",
                    "the final section compares rival causal explanations with the measurements"
                ),
            )
            .expect("source");
        builder
            .add_document(
                "noise",
                "railway lantern railway lantern railway lantern railway lantern",
            )
            .expect("noise");
        builder.build().expect("index")
    }

    #[test]
    fn short_queries_route_to_direct_search() {
        let plan = index()
            .plan_query(
                "signal",
                &SearchOptions::default(),
                QueryPlannerOptions::default(),
            )
            .expect("plan");
        assert_eq!(plan.route, AdaptiveRoute::ShortDirect);
    }

    #[test]
    fn long_fragmented_queries_route_to_composite() {
        let specimen = concat!(
            "the observatory opened its copper shutters before dawn and recorded a signal ",
            "a long inserted passage discusses typography finance railroads and cooking ",
            "the final section compares rival causal explanations with the measurements ",
            "another long unrelated tail extends the specimen beyond one local passage"
        );
        let plan = index()
            .plan_query(
                specimen,
                &SearchOptions {
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
                QueryPlannerOptions {
                    composite_minimum_tokens: 64,
                    ..QueryPlannerOptions::default()
                },
            )
            .expect("plan");
        assert_eq!(plan.route, AdaptiveRoute::Composite);
        assert!(
            plan.advisories
                .contains(&QueryAdvisory::CompositeRecommended)
        );
    }

    #[test]
    fn repeated_queries_are_marked_low_entropy_and_repetitive() {
        let specimen = "signal ".repeat(80);
        let plan = index()
            .plan_query(
                &specimen,
                &SearchOptions::default(),
                QueryPlannerOptions::default(),
            )
            .expect("plan");
        assert!(plan.advisories.contains(&QueryAdvisory::LowEntropy));
        assert!(plan.advisories.contains(&QueryAdvisory::HighRepetition));
        assert!(plan.repetition_fraction > 0.90);
    }

    #[test]
    fn adaptive_search_executes_the_planned_composite_route() {
        let specimen = concat!(
            "the observatory opened its copper shutters before dawn and recorded a signal ",
            "inserted unrelated words inserted unrelated words inserted unrelated words ",
            "the final section compares rival causal explanations with the measurements"
        );
        let report = index()
            .search_adaptive(
                specimen,
                &SearchOptions {
                    minimum_similarity: 0.05,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
                QueryPlannerOptions {
                    composite_minimum_tokens: 48,
                    ..QueryPlannerOptions::default()
                },
                CompositeSearchOptions {
                    minimum_block_tokens: 8,
                    minimum_incremental_query_tokens: 4,
                    minimum_aggregate_score: 0.05,
                    ..CompositeSearchOptions::default()
                },
            )
            .expect("adaptive search");
        assert_eq!(report.plan.route, AdaptiveRoute::Composite);
        assert!(
            report
                .matches
                .iter()
                .all(|item| matches!(item, AdaptiveMatch::Composite(_)))
        );
    }
}
