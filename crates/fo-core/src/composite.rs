use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{FoError, Index, Result, SearchIntent, SearchOptions, SearchResult, normalize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CompositeSearchOptions {
    pub maximum_blocks_per_document: usize,
    pub minimum_block_tokens: usize,
    pub minimum_incremental_query_tokens: usize,
    pub maximum_overlap_fraction: f32,
    pub minimum_aggregate_score: f32,
}

impl Default for CompositeSearchOptions {
    fn default() -> Self {
        Self {
            maximum_blocks_per_document: 8,
            minimum_block_tokens: 20,
            minimum_incremental_query_tokens: 12,
            maximum_overlap_fraction: 0.70,
            minimum_aggregate_score: 0.30,
        }
    }
}

impl CompositeSearchOptions {
    pub fn validate(self) -> Result<Self> {
        if self.maximum_blocks_per_document == 0 || self.maximum_blocks_per_document > 1_024 {
            return Err(FoError::InvalidConfig(
                "maximum_blocks_per_document must be between 1 and 1024".to_owned(),
            ));
        }
        if self.minimum_block_tokens == 0 || self.minimum_incremental_query_tokens == 0 {
            return Err(FoError::InvalidConfig(
                "composite minimum token counts must be positive".to_owned(),
            ));
        }
        for (name, value) in [
            ("maximum_overlap_fraction", self.maximum_overlap_fraction),
            ("minimum_aggregate_score", self.minimum_aggregate_score),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "{name} must be finite and lie in [0, 1]"
                )));
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeMatchBlock {
    pub query_start: usize,
    pub query_end: usize,
    pub corpus_start: usize,
    pub corpus_end: usize,
    pub edit_distance: usize,
    pub edit_similarity: f32,
    pub raw_score: f32,
    pub matched_tokens: usize,
    pub expected_false_matches: f64,
    pub matched_text: String,
}

impl From<SearchResult> for CompositeMatchBlock {
    fn from(result: SearchResult) -> Self {
        Self {
            query_start: result.query_start,
            query_end: result.query_end,
            corpus_start: result.corpus_start,
            corpus_end: result.corpus_end,
            edit_distance: result.edit_distance,
            edit_similarity: result.edit_similarity,
            raw_score: result.combined_score,
            matched_tokens: result.matched_tokens,
            expected_false_matches: result.estimated_false_matches,
            matched_text: result.matched_text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeSearchResult {
    pub document_id: u32,
    pub path: String,
    pub intent: SearchIntent,
    pub blocks: Vec<CompositeMatchBlock>,
    pub query_coverage: f32,
    pub source_coverage: f32,
    pub weighted_edit_similarity: f32,
    pub matched_tokens: usize,
    pub reordered_blocks: bool,
    pub expected_false_matches: f64,
    pub aggregate_score: f32,
}

impl Index {
    /// Search for a source assembled from several independently aligned passages.
    ///
    /// The ordinary passage engine is deliberately used as the high-recall first
    /// stage. Non-overlapping hits from the same document are then selected by
    /// incremental query coverage, allowing moved paragraphs and fragmented reuse
    /// to contribute to one source-attribution decision.
    pub fn search_composite(
        &self,
        specimen: &str,
        search_options: &SearchOptions,
        composite_options: CompositeSearchOptions,
    ) -> Result<Vec<CompositeSearchResult>> {
        search_options.validate()?;
        let composite_options = composite_options.validate()?;
        let query = normalize(specimen, &self.config.normalization);
        if query.is_empty() {
            return Err(FoError::EmptySpecimen);
        }

        let mut passage_options = search_options.clone();
        passage_options.intent = SearchIntent::AnyPassage;
        passage_options.max_results = search_options
            .max_candidates
            .max(
                search_options
                    .max_results
                    .saturating_mul(composite_options.maximum_blocks_per_document)
                    .saturating_mul(4),
            );
        passage_options.minimum_similarity = passage_options
            .minimum_similarity
            .min(composite_options.minimum_aggregate_score * 0.40)
            .min(0.20);
        passage_options.minimum_matched_tokens = passage_options
            .minimum_matched_tokens
            .min(composite_options.minimum_block_tokens);
        passage_options.minimum_query_coverage = 0.0;
        passage_options.minimum_source_coverage = 0.0;

        let passage_hits = self.search(specimen, &passage_options)?;
        let mut by_document = HashMap::<u32, Vec<SearchResult>>::new();
        for hit in passage_hits {
            by_document.entry(hit.document_id).or_default().push(hit);
        }

        let mut results = Vec::with_capacity(by_document.len());
        for (document_id, hits) in by_document {
            let Some(document) = self.document(document_id) else {
                continue;
            };
            let blocks = select_blocks(hits, &composite_options);
            if blocks.is_empty() {
                continue;
            }
            let query_intervals = blocks
                .iter()
                .map(|block| (block.query_start, block.query_end))
                .collect::<Vec<_>>();
            let corpus_intervals = blocks
                .iter()
                .map(|block| (block.corpus_start, block.corpus_end))
                .collect::<Vec<_>>();
            let query_union = interval_union_length(&query_intervals);
            let corpus_union = interval_union_length(&corpus_intervals);
            let query_coverage =
                (query_union as f32 / query.len().max(1) as f32).clamp(0.0, 1.0);
            let source_coverage = (corpus_union as f32
                / document.normalized.tokens.len().max(1) as f32)
                .clamp(0.0, 1.0);
            if !passes_intent_coverage(
                search_options,
                query_coverage,
                source_coverage,
            ) {
                continue;
            }
            let matched_tokens = query_union.min(corpus_union);
            if matched_tokens
                < search_options
                    .minimum_matched_tokens
                    .min(query.len())
            {
                continue;
            }
            let weight_total = blocks
                .iter()
                .map(|block| block.matched_tokens.max(1))
                .sum::<usize>();
            let weighted_edit_similarity = if weight_total == 0 {
                0.0
            } else {
                blocks
                    .iter()
                    .map(|block| {
                        block.edit_similarity * block.matched_tokens.max(1) as f32
                    })
                    .sum::<f32>()
                    / weight_total as f32
            };
            let expected_false_matches = blocks
                .iter()
                .map(|block| block.expected_false_matches.max(0.0))
                .sum::<f64>();
            let reordered_blocks = blocks
                .windows(2)
                .any(|pair| pair[1].corpus_start < pair[0].corpus_start);
            let aggregate_score = composite_score(
                search_options.intent,
                weighted_edit_similarity,
                query_coverage,
                source_coverage,
                blocks.len(),
                reordered_blocks,
                expected_false_matches,
            );
            if aggregate_score < composite_options.minimum_aggregate_score
                || aggregate_score < search_options.minimum_similarity
            {
                continue;
            }
            results.push(CompositeSearchResult {
                document_id,
                path: document.path.clone(),
                intent: search_options.intent,
                blocks,
                query_coverage,
                source_coverage,
                weighted_edit_similarity,
                matched_tokens,
                reordered_blocks,
                expected_false_matches,
                aggregate_score,
            });
        }

        results.sort_unstable_by(|left, right| {
            right
                .aggregate_score
                .total_cmp(&left.aggregate_score)
                .then_with(|| right.query_coverage.total_cmp(&left.query_coverage))
                .then_with(|| {
                    right
                        .weighted_edit_similarity
                        .total_cmp(&left.weighted_edit_similarity)
                })
                .then_with(|| left.document_id.cmp(&right.document_id))
        });
        results.truncate(search_options.max_results);
        Ok(results)
    }
}

fn select_blocks(
    mut hits: Vec<SearchResult>,
    options: &CompositeSearchOptions,
) -> Vec<CompositeMatchBlock> {
    hits.retain(|hit| hit.matched_tokens >= options.minimum_block_tokens);
    hits.sort_unstable_by(|left, right| {
        block_utility(right)
            .total_cmp(&block_utility(left))
            .then_with(|| right.query_coverage.total_cmp(&left.query_coverage))
            .then_with(|| left.query_start.cmp(&right.query_start))
            .then_with(|| left.corpus_start.cmp(&right.corpus_start))
    });

    let mut selected = Vec::<CompositeMatchBlock>::new();
    let mut query_intervals = Vec::<(usize, usize)>::new();
    for hit in hits {
        if selected.len() >= options.maximum_blocks_per_document {
            break;
        }
        let candidate = CompositeMatchBlock::from(hit);
        if selected.iter().any(|prior| {
            overlap_fraction(
                (candidate.query_start, candidate.query_end),
                (prior.query_start, prior.query_end),
            ) > options.maximum_overlap_fraction
                || overlap_fraction(
                    (candidate.corpus_start, candidate.corpus_end),
                    (prior.corpus_start, prior.corpus_end),
                ) > options.maximum_overlap_fraction
        }) {
            continue;
        }
        let before = interval_union_length(&query_intervals);
        query_intervals.push((candidate.query_start, candidate.query_end));
        let after = interval_union_length(&query_intervals);
        if !selected.is_empty()
            && after.saturating_sub(before) < options.minimum_incremental_query_tokens
        {
            query_intervals.pop();
            continue;
        }
        selected.push(candidate);
    }
    selected.sort_unstable_by(|left, right| {
        left.query_start
            .cmp(&right.query_start)
            .then_with(|| left.query_end.cmp(&right.query_end))
            .then_with(|| left.corpus_start.cmp(&right.corpus_start))
    });
    selected
}

fn block_utility(result: &SearchResult) -> f32 {
    result.combined_score
        * (result.matched_tokens.max(1) as f32).sqrt()
        * (0.75 + 0.25 * result.chain_consistency.clamp(0.0, 1.0))
}

fn passes_intent_coverage(
    options: &SearchOptions,
    query_coverage: f32,
    source_coverage: f32,
) -> bool {
    match options.intent {
        SearchIntent::AnyPassage => true,
        SearchIntent::SourceAttribution => query_coverage >= options.minimum_query_coverage,
        SearchIntent::NearDuplicate => {
            query_coverage >= options.minimum_query_coverage
                && source_coverage >= options.minimum_source_coverage
        }
    }
}

fn composite_score(
    intent: SearchIntent,
    edit_similarity: f32,
    query_coverage: f32,
    source_coverage: f32,
    block_count: usize,
    reordered: bool,
    expected_false_matches: f64,
) -> f32 {
    let evidence_confidence = (1.0 / (1.0 + expected_false_matches.max(0.0))) as f32;
    let block_support = (1.0 - (-(block_count as f32) / 2.0).exp()).clamp(0.0, 1.0);
    let reorder_factor = if reordered { 0.97 } else { 1.0 };
    match intent {
        SearchIntent::AnyPassage => {
            (0.72 * edit_similarity
                + 0.13 * query_coverage
                + 0.08 * evidence_confidence
                + 0.07 * block_support)
                .clamp(0.0, 1.0)
        }
        SearchIntent::SourceAttribution => {
            let base = 0.54 * edit_similarity
                + 0.25 * query_coverage
                + 0.08 * source_coverage.sqrt()
                + 0.08 * evidence_confidence
                + 0.05 * block_support;
            (base * query_coverage.sqrt() * reorder_factor).clamp(0.0, 1.0)
        }
        SearchIntent::NearDuplicate => {
            let coverage = harmonic_mean(query_coverage, source_coverage);
            let base = 0.65 * edit_similarity
                + 0.20 * coverage
                + 0.10 * evidence_confidence
                + 0.05 * block_support;
            (base * coverage.sqrt() * reorder_factor).clamp(0.0, 1.0)
        }
    }
}

fn harmonic_mean(left: f32, right: f32) -> f32 {
    if left + right <= 0.0 {
        0.0
    } else {
        2.0 * left * right / (left + right)
    }
}

fn interval_union_length(intervals: &[(usize, usize)]) -> usize {
    let mut intervals = intervals
        .iter()
        .copied()
        .filter(|(start, end)| start < end)
        .collect::<Vec<_>>();
    if intervals.is_empty() {
        return 0;
    }
    intervals.sort_unstable();
    let (mut start, mut end) = intervals[0];
    let mut total = 0usize;
    for (next_start, next_end) in intervals.into_iter().skip(1) {
        if next_start <= end {
            end = end.max(next_end);
        } else {
            total = total.saturating_add(end.saturating_sub(start));
            start = next_start;
            end = next_end;
        }
    }
    total.saturating_add(end.saturating_sub(start))
}

fn overlap_fraction(left: (usize, usize), right: (usize, usize)) -> f32 {
    let intersection = left.1.min(right.1).saturating_sub(left.0.max(right.0));
    let shorter = left
        .1
        .saturating_sub(left.0)
        .min(right.1.saturating_sub(right.0));
    if shorter == 0 {
        0.0
    } else {
        intersection as f32 / shorter as f32
    }
}

#[cfg(test)]
mod tests {
    use super::CompositeSearchOptions;
    use crate::{IndexBuilder, IndexConfig, SearchIntent, SearchOptions};

    #[test]
    fn combines_reordered_passages_from_one_source() {
        let block_a = "the observatory opened the copper shutters before dawn and checked every instrument twice";
        let block_b = "the raw measurements were published before the team proposed a causal explanation";
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source.txt",
                &format!("{block_a}. unrelated bridge material about weather and cooking. {block_b}."),
            )
            .expect("source");
        builder
            .add_document(
                "noise.txt",
                "a railway timetable describes stations, tickets, platforms, and winter maintenance",
            )
            .expect("noise");
        let index = builder.build().expect("index");
        let specimen = format!(
            "preface about typography. {block_b}. unrelated inserted paragraph. {block_a}. epilogue about finance"
        );
        let results = index
            .search_composite(
                &specimen,
                &SearchOptions {
                    intent: SearchIntent::SourceAttribution,
                    minimum_similarity: 0.10,
                    minimum_query_coverage: 0.20,
                    minimum_matched_tokens: 12,
                    max_results: 10,
                    ..SearchOptions::default()
                },
                CompositeSearchOptions {
                    minimum_aggregate_score: 0.10,
                    minimum_block_tokens: 12,
                    minimum_incremental_query_tokens: 8,
                    ..CompositeSearchOptions::default()
                },
            )
            .expect("composite search");
        assert!(!results.is_empty(), "{results:#?}");
        assert_eq!(results[0].path, "source.txt");
        assert!(results[0].blocks.len() >= 2, "{:#?}", results[0]);
        assert!(results[0].reordered_blocks);
        assert!(results[0].query_coverage > 0.25);
    }

    #[test]
    fn interval_selection_does_not_double_count_duplicate_hits() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source.txt",
                "a long distinctive passage about calibrated instruments and preserved raw observations",
            )
            .expect("source");
        let index = builder.build().expect("index");
        let results = index
            .search_composite(
                "a long distinctive passage about calibrated instruments and preserved raw observations",
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
                CompositeSearchOptions {
                    minimum_aggregate_score: 0.10,
                    minimum_block_tokens: 8,
                    ..CompositeSearchOptions::default()
                },
            )
            .expect("composite search");
        assert_eq!(results[0].blocks.len(), 1);
        assert!(results[0].query_coverage <= 1.0);
    }
}
