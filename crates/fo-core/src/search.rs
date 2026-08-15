use std::collections::HashMap;

use crate::{
    Anchor, ChainOptions, Feature, Fingerprint, FoError, Index, Result, SearchIntent, SearchOptions,
    SearchResult, chain_anchors, global_levenshtein, myers_infix_candidates, normalize,
    qgram_hashes, semi_global_banded, winnow,
};

#[derive(Debug, Clone, Copy)]
struct Vote {
    weight: f32,
    hits: u32,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    document_id: u32,
    expected_diagonal: i64,
    weight: f32,
    hits: u32,
}

#[derive(Debug, Clone)]
struct PreparedFeature {
    fingerprint: Fingerprint,
    query_positions: Vec<u32>,
    idf: f32,
    posting_count: usize,
}

impl Index {
    /// Search this index for spans lexically similar to `specimen`.
    ///
    /// Query fingerprints are grouped once, ordered rarest-first, and voted into
    /// two offset diagonal grids. Candidate spans are chained and verified before
    /// intent-aware scoring penalizes unsupported or insignificantly short reuse.
    pub fn search(&self, specimen: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        options.validate()?;
        let query = normalize(specimen, &self.config.normalization);
        if query.is_empty() {
            return Err(FoError::EmptySpecimen);
        }
        if query.len() < self.config.qgram_size {
            return Ok(self.search_short_query(&query.tokens, options));
        }

        let query_features = winnow(
            &qgram_hashes(&query.tokens, self.config.qgram_size)?,
            self.config.winnow_window,
        );
        if query_features.len() < options.minimum_anchor_hits as usize {
            return Ok(self.search_short_query(&query.tokens, options));
        }
        let prepared = self.prepare_query_features(&query_features, options);
        let retained_feature_count = prepared
            .iter()
            .map(|feature| feature.query_positions.len())
            .sum::<usize>();
        if retained_feature_count < options.minimum_anchor_hits as usize {
            return Ok(self.search_short_query(&query.tokens, options));
        }
        let query_feature_count = query_features.len();

        let mut votes = HashMap::<(u32, i64), Vote>::new();
        for feature in &prepared {
            let Some(entry) = self.lookup(feature.fingerprint) else {
                continue;
            };
            for posting in &entry.postings {
                for &query_position in &feature.query_positions {
                    let diagonal = i64::from(posting.position) - i64::from(query_position);
                    for shifted in [false, true] {
                        let encoded_bin =
                            encode_diagonal_bin(diagonal, options.diagonal_bin_width, shifted);
                        let vote = votes
                            .entry((posting.document_id, encoded_bin))
                            .or_insert(Vote {
                                weight: 0.0,
                                hits: 0,
                            });
                        vote.weight += feature.idf;
                        vote.hits = vote.hits.saturating_add(1);
                    }
                }
            }
        }

        let mut candidates = votes
            .into_iter()
            .filter_map(|((document_id, encoded_bin), vote)| {
                (vote.hits >= options.minimum_anchor_hits).then_some(Candidate {
                    document_id,
                    expected_diagonal: diagonal_bin_center(
                        encoded_bin,
                        options.diagonal_bin_width,
                    ),
                    weight: vote.weight,
                    hits: vote.hits,
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by(|left, right| {
            right
                .weight
                .total_cmp(&left.weight)
                .then_with(|| right.hits.cmp(&left.hits))
                .then_with(|| left.document_id.cmp(&right.document_id))
                .then_with(|| left.expected_diagonal.cmp(&right.expected_diagonal))
        });
        suppress_nearby_candidates(&mut candidates, options);
        candidates.truncate(options.max_candidates);

        let corpus_trials = self.stats().normalized_tokens.max(1) as f64;
        let mut results = Vec::with_capacity(
            candidates
                .len()
                .min(options.max_results.saturating_mul(4)),
        );
        for candidate in candidates {
            let Some(result) = self.score_candidate(
                &query.tokens,
                &prepared,
                query_feature_count,
                corpus_trials,
                candidate,
                options,
            ) else {
                continue;
            };
            if result.combined_score >= options.minimum_similarity {
                results.push(result);
            }
        }
        rank_and_deduplicate(&mut results, options.max_results);
        Ok(results)
    }

    fn prepare_query_features(
        &self,
        query_features: &[Feature],
        options: &SearchOptions,
    ) -> Vec<PreparedFeature> {
        let mut positions = HashMap::<Fingerprint, Vec<u32>>::new();
        for feature in query_features {
            positions
                .entry(feature.fingerprint)
                .or_default()
                .push(feature.position);
        }

        let document_count = self.documents.len() as f32;
        let mut prepared = positions
            .into_iter()
            .filter_map(|(fingerprint, mut query_positions)| {
                let entry = self.lookup(fingerprint)?;
                if entry.postings.len() > options.max_postings_per_feature {
                    return None;
                }
                query_positions.sort_unstable();
                query_positions.dedup();
                let idf =
                    ((document_count + 1.0) / (entry.document_frequency as f32 + 1.0)).ln() + 1.0;
                Some(PreparedFeature {
                    fingerprint,
                    query_positions,
                    idf,
                    posting_count: entry.postings.len(),
                })
            })
            .collect::<Vec<_>>();
        prepared.sort_unstable_by(|left, right| {
            left.posting_count
                .cmp(&right.posting_count)
                .then_with(|| right.idf.total_cmp(&left.idf))
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
        });
        prepared
    }

    fn score_candidate(
        &self,
        query_tokens: &[u32],
        query_features: &[PreparedFeature],
        query_feature_count: usize,
        corpus_trials: f64,
        candidate: Candidate,
        options: &SearchOptions,
    ) -> Option<SearchResult> {
        let document = self.document(candidate.document_id)?;
        let span = u16::try_from(self.config.qgram_size).ok()?;
        let mut anchors = Vec::new();

        for feature in query_features {
            let entry = self.lookup(feature.fingerprint)?;
            for posting in entry.postings_for_document(candidate.document_id) {
                for &query_position in &feature.query_positions {
                    let diagonal = i64::from(posting.position) - i64::from(query_position);
                    if diagonal.abs_diff(candidate.expected_diagonal)
                        > options.anchor_diagonal_band as u64
                    {
                        continue;
                    }
                    anchors.push(Anchor {
                        query_position,
                        corpus_position: posting.position,
                        span,
                        weight: feature.idf,
                    });
                    if anchors.len() >= options.maximum_anchors_per_candidate {
                        break;
                    }
                }
                if anchors.len() >= options.maximum_anchors_per_candidate {
                    break;
                }
            }
            if anchors.len() >= options.maximum_anchors_per_candidate {
                break;
            }
        }

        let chain_options = ChainOptions {
            maximum_anchors: options.maximum_anchors_per_candidate,
            predecessor_lookback: options.predecessor_lookback,
            maximum_gap: options.maximum_chain_gap,
            ..ChainOptions::default()
        };
        let chain = chain_anchors(anchors, &chain_options)?;
        if chain.anchors.len() < options.minimum_anchor_hits as usize {
            return None;
        }

        let context = self.config.qgram_size;
        let query_start = (chain.query_start as usize).saturating_sub(context);
        let query_end = (chain.query_end as usize)
            .saturating_add(context)
            .min(query_tokens.len());
        if query_start >= query_end {
            return None;
        }
        let verified_query = &query_tokens[query_start..query_end];

        let predicted_start = chain
            .median_diagonal
            .saturating_add(query_start as i64)
            .max(0) as usize;
        let window_start = predicted_start.saturating_sub(options.verification_slack);
        let window_end = predicted_start
            .saturating_add(verified_query.len())
            .saturating_add(options.verification_slack)
            .min(document.normalized.tokens.len());
        if window_start >= window_end {
            return None;
        }
        let expected_start = predicted_start.saturating_sub(window_start);
        let alignment = semi_global_banded(
            verified_query,
            &document.normalized.tokens[window_start..window_end],
            expected_start,
            options.verification_band,
        );
        let corpus_start = window_start.saturating_add(alignment.text_start);
        let corpus_end = window_start.saturating_add(alignment.text_end);
        if corpus_start >= corpus_end || corpus_end > document.normalized.tokens.len() {
            return None;
        }

        let corpus_span = corpus_end.saturating_sub(corpus_start);
        let matched_tokens = verified_query.len().min(corpus_span.max(1));
        let required_tokens = options.minimum_matched_tokens.min(query_tokens.len());
        if matched_tokens < required_tokens {
            return None;
        }
        let anchor_coverage = (chain.covered_query_tokens as f32 / query_tokens.len() as f32)
            .clamp(0.0, 1.0);
        let query_coverage =
            (verified_query.len() as f32 / query_tokens.len() as f32).clamp(0.0, 1.0);
        let source_coverage =
            (corpus_span as f32 / document.normalized.tokens.len().max(1) as f32).clamp(0.0, 1.0);
        if !passes_intent_coverage(options, query_coverage, source_coverage) {
            return None;
        }
        let anchor_score = (chain.score / query_feature_count.max(1) as f32).max(0.0);
        let vote_support =
            (candidate.hits as f32 / query_feature_count.max(1) as f32).clamp(0.0, 1.0);
        let chain_consistency = diagonal_consistency(
            &chain.anchors,
            chain.median_diagonal,
            options.diagonal_bin_width,
        );
        let estimated_false_matches =
            corpus_trials * (-f64::from(candidate.weight.max(0.0))).exp();
        let combined_score = score_evidence(
            options.intent,
            alignment.similarity,
            anchor_coverage,
            query_coverage,
            source_coverage,
            vote_support,
            chain_consistency,
            matched_tokens,
            estimated_false_matches,
        );

        Some(SearchResult {
            document_id: candidate.document_id,
            path: document.path.clone(),
            intent: options.intent,
            corpus_start,
            corpus_end,
            query_start,
            query_end,
            edit_distance: alignment.distance,
            edit_similarity: alignment.similarity,
            anchor_coverage,
            query_coverage,
            source_coverage,
            anchor_score,
            vote_support,
            chain_consistency,
            matched_tokens,
            distinct_anchor_count: chain.anchors.len(),
            estimated_false_matches,
            combined_score,
            matched_text: document
                .normalized
                .slice_tokens(corpus_start, corpus_end)
                .to_owned(),
        })
    }

    fn search_short_query(&self, query: &[u32], options: &SearchOptions) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let mut remaining_work = u128::from(options.direct_fallback_work_limit);
        for document in &self.documents {
            let text = &document.normalized.tokens;
            if text.is_empty() {
                continue;
            }
            if text.len() < query.len() {
                let work = (text.len() as u128).saturating_mul(query.len() as u128);
                if work > remaining_work {
                    continue;
                }
                remaining_work -= work;
                let distance = global_levenshtein(query, text);
                let denominator = query.len().max(text.len()).max(1);
                let similarity = (1.0 - distance as f32 / denominator as f32).clamp(0.0, 1.0);
                let query_coverage = text.len() as f32 / query.len().max(1) as f32;
                let result = direct_result(
                    document,
                    query,
                    0,
                    text.len(),
                    distance,
                    similarity,
                    query_coverage,
                    options,
                );
                if direct_result_passes_floors(&result, query.len(), options) {
                    results.push(result);
                }
                continue;
            }

            let exact = exact_occurrences(query, text, options.short_query_candidates);
            if !exact.is_empty() {
                for start in exact {
                    let result = direct_result(
                        document,
                        query,
                        start,
                        start + query.len(),
                        0,
                        1.0,
                        1.0,
                        options,
                    );
                    if direct_result_passes_floors(&result, query.len(), options) {
                        results.push(result);
                    }
                }
                continue;
            }

            let local = if query.len() <= 64 {
                let work = text.len() as u128;
                if work > remaining_work {
                    continue;
                }
                remaining_work -= work;
                myers_infix_candidates(query, text, options.short_query_candidates)
                    .into_iter()
                    .map(|candidate| {
                        (
                            candidate.distance,
                            candidate.end.saturating_sub(query.len()),
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                let samples = query.len().min(16);
                let windows = text.len() - query.len() + 1;
                let work = (windows as u128).saturating_mul(samples as u128);
                if work > remaining_work {
                    continue;
                }
                remaining_work -= work;
                sampled_hamming_candidates(
                    query,
                    text,
                    options.short_query_candidates,
                    samples,
                )
            };

            for (seed_distance, predicted_start) in local {
                let result = verify_short_candidate(
                    document,
                    query,
                    predicted_start,
                    seed_distance,
                    options,
                );
                if direct_result_passes_floors(&result, query.len(), options) {
                    results.push(result);
                }
            }
        }
        rank_and_deduplicate(&mut results, options.max_results);
        results
    }
}

fn exact_occurrences(pattern: &[u32], text: &[u32], maximum: usize) -> Vec<usize> {
    if pattern.is_empty() || text.len() < pattern.len() || maximum == 0 {
        return Vec::new();
    }
    let mut prefix = vec![0usize; pattern.len()];
    for position in 1..pattern.len() {
        let mut length = prefix[position - 1];
        while length > 0 && pattern[position] != pattern[length] {
            length = prefix[length - 1];
        }
        if pattern[position] == pattern[length] {
            length += 1;
        }
        prefix[position] = length;
    }

    let mut matched = 0usize;
    let mut occurrences = Vec::with_capacity(maximum);
    for (position, &token) in text.iter().enumerate() {
        while matched > 0 && token != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if token == pattern[matched] {
            matched += 1;
        }
        if matched == pattern.len() {
            occurrences.push(position + 1 - pattern.len());
            if occurrences.len() >= maximum {
                break;
            }
            matched = prefix[matched - 1];
        }
    }
    occurrences
}

fn sampled_hamming_candidates(
    pattern: &[u32],
    text: &[u32],
    maximum: usize,
    sample_count: usize,
) -> Vec<(usize, usize)> {
    if maximum == 0 || sample_count == 0 || text.len() < pattern.len() {
        return Vec::new();
    }
    let samples = sample_positions(pattern.len(), sample_count);
    let windows = text.len() - pattern.len() + 1;
    let mut candidates = Vec::<(usize, usize)>::with_capacity(maximum);
    for start in 0..windows {
        let mismatches = samples
            .iter()
            .filter(|&&position| pattern[position] != text[start + position])
            .count();
        retain_seed(&mut candidates, (mismatches, start), maximum);
    }
    candidates
}

fn sample_positions(length: usize, count: usize) -> Vec<usize> {
    let count = count.min(length).max(1);
    if count == 1 {
        return vec![length / 2];
    }
    let last = length - 1;
    let mut positions = (0..count)
        .map(|index| index.saturating_mul(last) / (count - 1))
        .collect::<Vec<_>>();
    positions.dedup();
    positions
}

fn retain_seed(candidates: &mut Vec<(usize, usize)>, candidate: (usize, usize), maximum: usize) {
    if candidates.len() < maximum {
        candidates.push(candidate);
        candidates.sort_unstable();
    } else if candidate < candidates[maximum - 1] {
        candidates[maximum - 1] = candidate;
        candidates.sort_unstable();
    }
}

fn verify_short_candidate(
    document: &crate::Document,
    query: &[u32],
    predicted_start: usize,
    seed_distance: usize,
    options: &SearchOptions,
) -> SearchResult {
    let text = &document.normalized.tokens;
    let adaptive_slack = options
        .verification_slack
        .max(seed_distance.saturating_mul(2).saturating_add(4));
    let window_start = predicted_start.saturating_sub(adaptive_slack);
    let window_end = predicted_start
        .saturating_add(query.len())
        .saturating_add(adaptive_slack)
        .min(text.len());
    let alignment = semi_global_banded(
        query,
        &text[window_start..window_end],
        predicted_start.saturating_sub(window_start),
        options.verification_band.max(seed_distance.saturating_add(8)),
    );
    let corpus_start = window_start + alignment.text_start;
    let corpus_end = window_start + alignment.text_end;
    direct_result(
        document,
        query,
        corpus_start,
        corpus_end,
        alignment.distance,
        alignment.similarity,
        1.0,
        options,
    )
}

#[allow(clippy::too_many_arguments)]
fn direct_result(
    document: &crate::Document,
    query: &[u32],
    corpus_start: usize,
    corpus_end: usize,
    edit_distance: usize,
    edit_similarity: f32,
    query_coverage: f32,
    options: &SearchOptions,
) -> SearchResult {
    let corpus_span = corpus_end.saturating_sub(corpus_start);
    let source_coverage =
        (corpus_span as f32 / document.normalized.tokens.len().max(1) as f32).clamp(0.0, 1.0);
    let matched_tokens = query.len().min(corpus_span.max(1));
    let trials = document
        .normalized
        .tokens
        .len()
        .saturating_sub(matched_tokens)
        .saturating_add(1) as f64;
    let estimated_false_matches =
        trials * (-(matched_tokens as f64) * f64::from(edit_similarity)).exp();
    let combined_score = score_evidence(
        options.intent,
        edit_similarity,
        0.0,
        query_coverage,
        source_coverage,
        0.0,
        1.0,
        matched_tokens,
        estimated_false_matches,
    );
    SearchResult {
        document_id: document.id,
        path: document.path.clone(),
        intent: options.intent,
        corpus_start,
        corpus_end,
        query_start: 0,
        query_end: query.len(),
        edit_distance,
        edit_similarity,
        anchor_coverage: 0.0,
        query_coverage,
        source_coverage,
        anchor_score: 0.0,
        vote_support: 0.0,
        chain_consistency: 1.0,
        matched_tokens,
        distinct_anchor_count: 0,
        estimated_false_matches,
        combined_score,
        matched_text: document
            .normalized
            .slice_tokens(corpus_start, corpus_end)
            .to_owned(),
    }
}

fn direct_result_passes_floors(
    result: &SearchResult,
    query_length: usize,
    options: &SearchOptions,
) -> bool {
    result.corpus_start < result.corpus_end
        && result.matched_tokens >= options.minimum_matched_tokens.min(query_length)
        && passes_intent_coverage(options, result.query_coverage, result.source_coverage)
        && result.combined_score >= options.minimum_similarity
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

#[allow(clippy::too_many_arguments)]
fn score_evidence(
    intent: SearchIntent,
    edit_similarity: f32,
    anchor_coverage: f32,
    query_coverage: f32,
    source_coverage: f32,
    vote_support: f32,
    chain_consistency: f32,
    matched_tokens: usize,
    estimated_false_matches: f64,
) -> f32 {
    let edit_similarity = edit_similarity.clamp(0.0, 1.0);
    let length_factor = (1.0 - (-(matched_tokens as f32) / 32.0).exp()).clamp(0.0, 1.0);
    let evidence_confidence = (1.0 / (1.0 + estimated_false_matches.max(0.0))) as f32;
    match intent {
        SearchIntent::AnyPassage => {
            let base = 0.58 * edit_similarity
                + 0.12 * anchor_coverage
                + 0.10 * vote_support
                + 0.10 * chain_consistency
                + 0.10 * length_factor;
            (base * (0.75 + 0.25 * length_factor)).clamp(0.0, 1.0)
        }
        SearchIntent::SourceAttribution => {
            let base = 0.50 * edit_similarity
                + 0.18 * anchor_coverage
                + 0.10 * chain_consistency
                + 0.07 * vote_support
                + 0.08 * length_factor
                + 0.07 * evidence_confidence;
            (base * query_coverage.sqrt()).clamp(0.0, 1.0)
        }
        SearchIntent::NearDuplicate => {
            let coverage = if query_coverage + source_coverage > 0.0 {
                2.0 * query_coverage * source_coverage / (query_coverage + source_coverage)
            } else {
                0.0
            };
            let base = 0.67 * edit_similarity
                + 0.18 * chain_consistency
                + 0.15 * evidence_confidence;
            (base * coverage.sqrt()).clamp(0.0, 1.0)
        }
    }
}

fn diagonal_consistency(anchors: &[Anchor], median_diagonal: i64, bin_width: i64) -> f32 {
    if anchors.is_empty() {
        return 0.0;
    }
    let mut deviations = anchors
        .iter()
        .map(|anchor| {
            (i64::from(anchor.corpus_position) - i64::from(anchor.query_position))
                .abs_diff(median_diagonal)
        })
        .collect::<Vec<_>>();
    deviations.sort_unstable();
    let median_deviation = deviations[deviations.len() / 2] as f32;
    let scale = bin_width.max(1) as f32;
    (1.0 / (1.0 + median_deviation / scale)).clamp(0.0, 1.0)
}

fn encode_diagonal_bin(diagonal: i64, width: i64, shifted: bool) -> i64 {
    let half = width / 2;
    let bin = if shifted {
        diagonal.saturating_add(half).div_euclid(width)
    } else {
        diagonal.div_euclid(width)
    };
    bin.saturating_mul(2)
        .saturating_add(if shifted { 1 } else { 0 })
}

fn diagonal_bin_center(encoded: i64, width: i64) -> i64 {
    let shifted = encoded.rem_euclid(2) == 1;
    let bin = encoded.div_euclid(2);
    if shifted {
        bin.saturating_mul(width)
    } else {
        bin.saturating_mul(width).saturating_add(width / 2)
    }
}

fn suppress_nearby_candidates(candidates: &mut Vec<Candidate>, options: &SearchOptions) {
    let radius = options
        .candidate_suppression_bins
        .saturating_mul(options.diagonal_bin_width);
    let mut selected = Vec::<Candidate>::with_capacity(candidates.len());
    for candidate in candidates.drain(..) {
        if selected.iter().any(|prior| {
            prior.document_id == candidate.document_id
                && prior
                    .expected_diagonal
                    .abs_diff(candidate.expected_diagonal)
                    <= radius as u64
        }) {
            continue;
        }
        selected.push(candidate);
        if selected.len() >= options.max_candidates {
            break;
        }
    }
    *candidates = selected;
}

fn rank_and_deduplicate(results: &mut Vec<SearchResult>, max_results: usize) {
    results.sort_unstable_by(|left, right| {
        right
            .combined_score
            .total_cmp(&left.combined_score)
            .then_with(|| right.query_coverage.total_cmp(&left.query_coverage))
            .then_with(|| right.anchor_coverage.total_cmp(&left.anchor_coverage))
            .then_with(|| left.edit_distance.cmp(&right.edit_distance))
            .then_with(|| left.document_id.cmp(&right.document_id))
            .then_with(|| left.corpus_start.cmp(&right.corpus_start))
    });
    let mut selected = Vec::<SearchResult>::with_capacity(max_results.min(results.len()));
    for result in results.drain(..) {
        let duplicate = selected.iter().any(|prior| {
            if prior.document_id != result.document_id {
                return false;
            }
            let intersection = prior
                .corpus_end
                .min(result.corpus_end)
                .saturating_sub(prior.corpus_start.max(result.corpus_start));
            let shorter = prior
                .corpus_end
                .saturating_sub(prior.corpus_start)
                .min(result.corpus_end.saturating_sub(result.corpus_start));
            shorter > 0 && intersection.saturating_mul(5) >= shorter.saturating_mul(4)
        });
        if !duplicate {
            selected.push(result);
            if selected.len() >= max_results {
                break;
            }
        }
    }
    *results = selected;
}

#[cfg(test)]
mod tests {
    use super::{exact_occurrences, sampled_hamming_candidates};
    use crate::{IndexBuilder, IndexConfig, SearchIntent, SearchOptions};

    #[test]
    fn edited_passage_ranks_the_source_document() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source.txt",
                concat!(
                    "Before dawn the observatory opened its copper shutters. The team measured ",
                    "a faint repeating signal, checked every instrument, and published the raw ",
                    "observations before proposing an explanation."
                ),
            )
            .expect("source");
        builder
            .add_document(
                "noise.txt",
                "A cookbook describing winter vegetables, cast iron pans, and sourdough starters.",
            )
            .expect("noise");
        let index = builder.build().expect("index");
        let hits = index
            .search(
                concat!(
                    "The researchers detected a faint, repeating signal before sunrise, verified ",
                    "all their instruments, and released the raw observations before suggesting ",
                    "an explanation."
                ),
                &SearchOptions {
                    minimum_similarity: 0.20,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "source.txt");
        assert!(hits[0].query_coverage > 0.25);
    }

    #[test]
    fn short_query_uses_direct_fallback() {
        let config = IndexConfig {
            qgram_size: 8,
            ..IndexConfig::default()
        };
        let mut builder = IndexBuilder::new(config).expect("builder");
        builder.add_document("a", "xyz abc xyz").expect("add");
        let hits = builder
            .build()
            .expect("index")
            .search("abc", &SearchOptions::default())
            .expect("search");
        assert_eq!(hits[0].matched_text, "abc");
        assert_eq!(hits[0].matched_tokens, 3);
    }

    #[test]
    fn one_fingerprint_query_uses_direct_fallback() {
        let config = IndexConfig {
            qgram_size: 7,
            winnow_window: 12,
            ..IndexConfig::default()
        };
        let mut builder = IndexBuilder::new(config).expect("builder");
        builder
            .add_document("a", "prefix abcdefg suffix")
            .expect("add");
        let hits = builder
            .build()
            .expect("index")
            .search("abcdefg", &SearchOptions::default())
            .expect("search");
        assert_eq!(hits[0].matched_text, "abcdefg");
    }

    #[test]
    fn source_attribution_rejects_tiny_fragment_but_passage_mode_keeps_it() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source",
                "the copper shutters opened before dawn and the instruments were checked twice",
            )
            .expect("source");
        let index = builder.build().expect("index");
        let specimen = concat!(
            "This very long unrelated preface discusses typography, printing, and paper. ",
            "The copper shutters opened before dawn. ",
            "A long unrelated epilogue discusses cooking, railroads, and municipal finance."
        );
        let attributed = index
            .search(
                specimen,
                &SearchOptions {
                    minimum_query_coverage: 0.50,
                    minimum_similarity: 0.10,
                    ..SearchOptions::default()
                },
            )
            .expect("source attribution");
        assert!(attributed.is_empty());

        let passage = index
            .search(
                specimen,
                &SearchOptions {
                    intent: SearchIntent::AnyPassage,
                    minimum_similarity: 0.10,
                    ..SearchOptions::default()
                },
            )
            .expect("passage search");
        assert!(!passage.is_empty());
    }

    #[test]
    fn kmp_finds_overlapping_exact_occurrences() {
        let pattern = vec![1, 1];
        let text = vec![1, 1, 1, 1];
        assert_eq!(exact_occurrences(&pattern, &text, 10), vec![0, 1, 2]);
    }

    #[test]
    fn sampled_candidates_preserve_exact_window() {
        let pattern = (0..80).collect::<Vec<_>>();
        let mut text = vec![999; 100];
        text.extend_from_slice(&pattern);
        text.extend_from_slice(&[999; 100]);
        let candidates = sampled_hamming_candidates(&pattern, &text, 4, 16);
        assert!(candidates.iter().any(|&(distance, start)| distance == 0 && start == 100));
    }

    #[test]
    fn myers_short_fallback_handles_insertion() {
        let config = IndexConfig {
            qgram_size: 32,
            ..IndexConfig::default()
        };
        let mut builder = IndexBuilder::new(config).expect("builder");
        builder
            .add_document("a", "prefix abcXdef suffix")
            .expect("add");
        let hits = builder
            .build()
            .expect("index")
            .search(
                "abcdef",
                &SearchOptions {
                    minimum_similarity: 0.30,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].edit_distance, 1);
    }
}
