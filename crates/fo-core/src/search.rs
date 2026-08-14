use std::collections::HashMap;

use crate::{
    Anchor, ChainOptions, FoError, Index, Result, SearchOptions, SearchResult, chain_anchors,
    global_levenshtein, normalize, qgram_hashes, semi_global_banded, winnow,
};

#[derive(Debug, Clone, Copy)]
struct Vote {
    weight: f32,
    hits: u32,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    document_id: u32,
    diagonal_bin: i64,
    weight: f32,
    hits: u32,
}

impl Index {
    /// Search this index for spans lexically similar to `specimen`.
    ///
    /// The implementation is deliberately two-stage: rare winnowed q-grams vote
    /// for document/diagonal candidates, then collinear anchors are chained and
    /// only the surviving spans receive an exact semi-global edit-distance pass.
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

        let mut votes = HashMap::<(u32, i64), Vote>::new();
        let document_count = self.documents.len() as f32;
        for feature in &query_features {
            let Some(entry) = self.lookup(feature.fingerprint) else {
                continue;
            };
            if entry.postings.len() > options.max_postings_per_feature {
                continue;
            }
            let idf = ((document_count + 1.0) / (entry.document_frequency as f32 + 1.0)).ln() + 1.0;
            for posting in &entry.postings {
                let diagonal = i64::from(posting.position) - i64::from(feature.position);
                let bin = diagonal.div_euclid(options.diagonal_bin_width);
                let vote = votes.entry((posting.document_id, bin)).or_insert(Vote {
                    weight: 0.0,
                    hits: 0,
                });
                vote.weight += idf;
                vote.hits = vote.hits.saturating_add(1);
            }
        }

        let mut candidates = votes
            .into_iter()
            .filter_map(|((document_id, diagonal_bin), vote)| {
                (vote.hits >= options.minimum_anchor_hits).then_some(Candidate {
                    document_id,
                    diagonal_bin,
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
                .then_with(|| left.diagonal_bin.cmp(&right.diagonal_bin))
        });
        suppress_nearby_candidates(&mut candidates, options);
        candidates.truncate(options.max_candidates);

        let mut results = Vec::with_capacity(
            candidates
                .len()
                .min(options.max_results.saturating_mul(4)),
        );
        for candidate in candidates {
            let Some(result) = self.score_candidate(
                &query.tokens,
                &query_features,
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

    fn score_candidate(
        &self,
        query_tokens: &[u32],
        query_features: &[crate::Feature],
        candidate: Candidate,
        options: &SearchOptions,
    ) -> Option<SearchResult> {
        let document = self.document(candidate.document_id)?;
        let expected_diagonal = candidate
            .diagonal_bin
            .saturating_mul(options.diagonal_bin_width)
            .saturating_add(options.diagonal_bin_width / 2);
        let span = u16::try_from(self.config.qgram_size).ok()?;
        let mut anchors = Vec::new();

        for feature in query_features {
            let Some(entry) = self.lookup(feature.fingerprint) else {
                continue;
            };
            if entry.postings.len() > options.max_postings_per_feature {
                continue;
            }
            let idf = (((self.documents.len() + 1) as f32)
                / (entry.document_frequency as f32 + 1.0))
                .ln()
                + 1.0;
            for posting in entry.postings_for_document(candidate.document_id) {
                let diagonal = i64::from(posting.position) - i64::from(feature.position);
                if diagonal.abs_diff(expected_diagonal) > options.anchor_diagonal_band as u64 {
                    continue;
                }
                anchors.push(Anchor {
                    query_position: feature.position,
                    corpus_position: posting.position,
                    span,
                    weight: idf,
                });
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

        // Verify the query interval actually supported by the chain, with one
        // q-gram of context on either side. This preserves sensitivity to partial
        // reuse without allowing a tiny isolated phrase to masquerade as a full
        // match: anchor_coverage remains part of the final score.
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

        let anchor_coverage = (chain.covered_query_tokens as f32 / query_tokens.len() as f32)
            .clamp(0.0, 1.0);
        let anchor_score = (chain.score / query_features.len().max(1) as f32).max(0.0);
        let vote_support = (candidate.hits as f32 / query_features.len().max(1) as f32)
            .clamp(0.0, 1.0);
        let combined_score = (0.62 * alignment.similarity
            + 0.25 * anchor_coverage
            + 0.08 * anchor_score.tanh()
            + 0.05 * vote_support)
            .clamp(0.0, 1.0);

        Some(SearchResult {
            document_id: candidate.document_id,
            path: document.path.clone(),
            corpus_start,
            corpus_end,
            query_start,
            query_end,
            edit_distance: alignment.distance,
            edit_similarity: alignment.similarity,
            anchor_coverage,
            anchor_score,
            combined_score,
            matched_text: document
                .normalized
                .slice_tokens(corpus_start, corpus_end)
                .to_owned(),
        })
    }

    fn search_short_query(&self, query: &[u32], options: &SearchOptions) -> Vec<SearchResult> {
        let mut results = Vec::new();
        for document in &self.documents {
            if document.normalized.tokens.is_empty() {
                continue;
            }
            if document.normalized.tokens.len() < query.len() {
                let distance = global_levenshtein(query, &document.normalized.tokens);
                let denominator = query.len().max(document.normalized.tokens.len()).max(1);
                let similarity = (1.0 - distance as f32 / denominator as f32).clamp(0.0, 1.0);
                if similarity >= options.minimum_similarity {
                    results.push(SearchResult {
                        document_id: document.id,
                        path: document.path.clone(),
                        corpus_start: 0,
                        corpus_end: document.normalized.tokens.len(),
                        query_start: 0,
                        query_end: query.len(),
                        edit_distance: distance,
                        edit_similarity: similarity,
                        anchor_coverage: 0.0,
                        anchor_score: 0.0,
                        combined_score: similarity,
                        matched_text: document.normalized.text.clone(),
                    });
                }
                continue;
            }

            let mut local = Vec::<(usize, usize)>::new();
            for start in 0..=document.normalized.tokens.len() - query.len() {
                let mismatches = query
                    .iter()
                    .zip(&document.normalized.tokens[start..start + query.len()])
                    .filter(|(left, right)| left != right)
                    .count();
                if local.len() < 4 {
                    local.push((mismatches, start));
                    local.sort_unstable();
                } else if (mismatches, start) < local[3] {
                    local[3] = (mismatches, start);
                    local.sort_unstable();
                }
            }
            for (mismatches, predicted_start) in local {
                let window_start = predicted_start.saturating_sub(options.verification_slack);
                let window_end = predicted_start
                    .saturating_add(query.len())
                    .saturating_add(
                        options
                            .verification_slack
                            .min(query.len().saturating_mul(2).saturating_add(8)),
                    )
                    .min(document.normalized.tokens.len());
                let alignment = semi_global_banded(
                    query,
                    &document.normalized.tokens[window_start..window_end],
                    predicted_start.saturating_sub(window_start),
                    options.verification_band.max(query.len().saturating_add(8)),
                );
                let corpus_start = window_start + alignment.text_start;
                let corpus_end = window_start + alignment.text_end;
                let hamming_similarity =
                    (1.0 - mismatches as f32 / query.len().max(1) as f32).clamp(0.0, 1.0);
                let combined_score = alignment.similarity.max(hamming_similarity * 0.95);
                if combined_score < options.minimum_similarity || corpus_start >= corpus_end {
                    continue;
                }
                results.push(SearchResult {
                    document_id: document.id,
                    path: document.path.clone(),
                    corpus_start,
                    corpus_end,
                    query_start: 0,
                    query_end: query.len(),
                    edit_distance: alignment.distance,
                    edit_similarity: alignment.similarity,
                    anchor_coverage: 0.0,
                    anchor_score: 0.0,
                    combined_score,
                    matched_text: document
                        .normalized
                        .slice_tokens(corpus_start, corpus_end)
                        .to_owned(),
                });
            }
        }
        rank_and_deduplicate(&mut results, options.max_results);
        results
    }
}

fn suppress_nearby_candidates(candidates: &mut Vec<Candidate>, options: &SearchOptions) {
    let mut selected = Vec::<Candidate>::with_capacity(candidates.len());
    for candidate in candidates.drain(..) {
        if selected.iter().any(|prior| {
            prior.document_id == candidate.document_id
                && prior.diagonal_bin.abs_diff(candidate.diagonal_bin)
                    <= options.candidate_suppression_bins as u64
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
            shorter > 0
                && intersection.saturating_mul(5) >= shorter.saturating_mul(4)
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
    use crate::{IndexBuilder, IndexConfig, SearchOptions};

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
                    minimum_similarity: 0.25,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "source.txt");
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
}
