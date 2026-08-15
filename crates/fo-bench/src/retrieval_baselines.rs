use std::collections::BTreeSet;
use std::error::Error;

use fo_core::{
    Fingerprint, NormalizationProfile, NormalizedText, normalize, qgram_hashes,
};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

pub type BaselineResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct PreparedBaselineDocument {
    pub normalized: NormalizedText,
    qgrams: BTreeSet<Fingerprint>,
    simhash: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedBaselines {
    profile: NormalizationProfile,
    qgram_size: usize,
    documents: Vec<PreparedBaselineDocument>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
}

impl TokenSpan {
    #[must_use]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExhaustiveAlignment {
    pub distance: usize,
    pub similarity: f64,
    pub text_start: usize,
    pub text_end: usize,
    pub cells: u64,
}

impl ExhaustiveAlignment {
    #[must_use]
    pub fn span(self) -> TokenSpan {
        TokenSpan {
            start: self.text_start,
            end: self.text_end,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustiveQueryOutput {
    pub scores: Vec<Option<f64>>,
    pub alignments: Vec<Option<ExhaustiveAlignment>>,
    pub cells: u64,
    pub evaluated_documents: usize,
    pub skipped_documents: usize,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpanAccuracy {
    pub intersection_tokens: usize,
    pub union_tokens: usize,
    pub intersection_over_union: f64,
    pub expected_coverage: f64,
    pub predicted_coverage: f64,
    pub start_absolute_error: usize,
    pub end_absolute_error: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    cost: u32,
    start: u32,
}

impl PreparedBaselines {
    pub fn new(
        bodies: &[String],
        profile: NormalizationProfile,
        qgram_size: usize,
    ) -> BaselineResult<Self> {
        if qgram_size == 0 {
            return Err(invalid("baseline q-gram size must be positive"));
        }
        let mut documents = Vec::with_capacity(bodies.len());
        for body in bodies {
            let normalized = normalize(body, &profile);
            let qgrams = fingerprint_set(&normalized.tokens, qgram_size)?;
            let simhash = simhash(&qgrams);
            documents.push(PreparedBaselineDocument {
                normalized,
                qgrams,
                simhash,
            });
        }
        Ok(Self {
            profile,
            qgram_size,
            documents,
        })
    }

    #[must_use]
    pub fn documents(&self) -> &[PreparedBaselineDocument] {
        &self.documents
    }

    #[must_use]
    pub fn normalize_query(&self, query: &str) -> NormalizedText {
        normalize(query, &self.profile)
    }

    pub fn exact_scores(&self, normalized_query: &NormalizedText) -> Vec<f64> {
        self.documents
            .iter()
            .map(|document| {
                if !normalized_query.text.is_empty()
                    && document.normalized.text.contains(&normalized_query.text)
                {
                    1.0
                } else {
                    0.0
                }
            })
            .collect()
    }

    pub fn jaccard_scores(
        &self,
        normalized_query: &NormalizedText,
    ) -> BaselineResult<Vec<f64>> {
        let query = fingerprint_set(&normalized_query.tokens, self.qgram_size)?;
        Ok(self
            .documents
            .iter()
            .map(|document| jaccard(&query, &document.qgrams))
            .collect())
    }

    pub fn simhash_scores(
        &self,
        normalized_query: &NormalizedText,
    ) -> BaselineResult<Vec<f64>> {
        let query = fingerprint_set(&normalized_query.tokens, self.qgram_size)?;
        let query_hash = simhash(&query);
        Ok(self
            .documents
            .iter()
            .map(|document| {
                1.0 - f64::from((query_hash ^ document.simhash).count_ones()) / 64.0
            })
            .collect())
    }

    pub fn exhaustive_scores(
        &self,
        normalized_query: &NormalizedText,
        maximum_cells: u64,
    ) -> BaselineResult<ExhaustiveQueryOutput> {
        let mut scores = vec![None; self.documents.len()];
        let mut alignments = vec![None; self.documents.len()];
        let mut cells = 0u64;
        let mut evaluated_documents = 0usize;
        let mut skipped_documents = 0usize;
        for (index, document) in self.documents.iter().enumerate() {
            let required = checked_cells(normalized_query.len(), document.normalized.len())?;
            if cells.saturating_add(required) > maximum_cells {
                skipped_documents += 1;
                continue;
            }
            let alignment = exhaustive_semi_global(
                &normalized_query.tokens,
                &document.normalized.tokens,
            )?;
            debug_assert_eq!(alignment.cells, required);
            cells = cells.saturating_add(required);
            evaluated_documents += 1;
            scores[index] = Some(alignment.similarity);
            alignments[index] = Some(alignment);
        }
        Ok(ExhaustiveQueryOutput {
            scores,
            alignments,
            cells,
            evaluated_documents,
            skipped_documents,
            complete: skipped_documents == 0,
        })
    }
}

pub fn expected_token_span(
    body: &str,
    start_word: usize,
    word_count: usize,
    profile: &NormalizationProfile,
) -> Option<TokenSpan> {
    if word_count == 0 {
        return None;
    }
    let words = body.unicode_word_indices().collect::<Vec<_>>();
    if start_word >= words.len() {
        return None;
    }
    let end_word = start_word.saturating_add(word_count).min(words.len());
    let start_byte = words[start_word].0;
    let end_byte = if end_word < words.len() {
        words[end_word].0
    } else {
        body.len()
    };
    let start = normalize(body.get(..start_byte)?, profile).len();
    let end = normalize(body.get(..end_byte)?, profile).len();
    (start < end).then_some(TokenSpan { start, end })
}

#[must_use]
pub fn span_accuracy(expected: TokenSpan, predicted: &[TokenSpan]) -> Option<SpanAccuracy> {
    if expected.is_empty() || predicted.is_empty() {
        return None;
    }
    let mut intervals = predicted
        .iter()
        .copied()
        .filter(|span| !span.is_empty())
        .collect::<Vec<_>>();
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_unstable_by_key(|span| (span.start, span.end));
    let intervals = merge_spans(&intervals);
    let predicted_tokens = intervals.iter().map(|span| span.len()).sum::<usize>();
    let intersection_tokens = intervals
        .iter()
        .map(|span| overlap_length(expected, *span))
        .sum::<usize>();
    let union_tokens = expected
        .len()
        .saturating_add(predicted_tokens)
        .saturating_sub(intersection_tokens);
    let predicted_start = intervals.first()?.start;
    let predicted_end = intervals.last()?.end;
    Some(SpanAccuracy {
        intersection_tokens,
        union_tokens,
        intersection_over_union: ratio(intersection_tokens, union_tokens),
        expected_coverage: ratio(intersection_tokens, expected.len()),
        predicted_coverage: ratio(intersection_tokens, predicted_tokens),
        start_absolute_error: predicted_start.abs_diff(expected.start),
        end_absolute_error: predicted_end.abs_diff(expected.end),
    })
}

pub fn exhaustive_semi_global(
    pattern: &[u32],
    text: &[u32],
) -> BaselineResult<ExhaustiveAlignment> {
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
            let substitution = if pattern_token == text_token { 0 } else { 1 };
            let diagonal = Cell {
                cost: previous[column - 1].cost.saturating_add(substitution),
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
    let denominator = pattern
        .len()
        .max(text_end.saturating_sub(text_start))
        .max(1);
    let similarity = (1.0 - f64::from(best.cost) / denominator as f64).clamp(0.0, 1.0);
    Ok(ExhaustiveAlignment {
        distance: best.cost as usize,
        similarity,
        text_start,
        text_end,
        cells,
    })
}

pub fn checked_cells(pattern: usize, text: usize) -> BaselineResult<u64> {
    let cells = (pattern as u128).saturating_mul(text as u128);
    u64::try_from(cells).map_err(|_| invalid("dynamic-programming cell count exceeds u64"))
}

fn fingerprint_set(tokens: &[u32], qgram_size: usize) -> BaselineResult<BTreeSet<Fingerprint>> {
    Ok(qgram_hashes(tokens, qgram_size)?
        .into_iter()
        .map(|feature| feature.fingerprint)
        .collect())
}

fn jaccard(left: &BTreeSet<Fingerprint>, right: &BTreeSet<Fingerprint>) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(right).count();
    let union = left
        .len()
        .saturating_add(right.len())
        .saturating_sub(intersection);
    ratio(intersection, union)
}

fn simhash(fingerprints: &BTreeSet<Fingerprint>) -> u64 {
    if fingerprints.is_empty() {
        return 0;
    }
    let mut accumulators = [0i64; 64];
    for fingerprint in fingerprints {
        let value = mix(fingerprint.hi ^ fingerprint.lo.rotate_left(17));
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
        .fold(0u64, |output, (bit, accumulator)| {
            if *accumulator >= 0 {
                output | (1u64 << bit)
            } else {
                output
            }
        })
}

fn merge_spans(spans: &[TokenSpan]) -> Vec<TokenSpan> {
    let mut merged = Vec::<TokenSpan>::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(*span),
        }
    }
    merged
}

fn overlap_length(left: TokenSpan, right: TokenSpan) -> usize {
    left.end
        .min(right.end)
        .saturating_sub(left.start.max(right.start))
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

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{TokenSpan, exhaustive_semi_global, expected_token_span, span_accuracy};
    use fo_core::NormalizationProfile;

    #[test]
    fn exhaustive_alignment_finds_an_exact_infix() {
        let alignment = exhaustive_semi_global(&[2, 3, 4], &[0, 1, 2, 3, 4, 5])
            .expect("alignment");
        assert_eq!(alignment.distance, 0);
        assert_eq!((alignment.text_start, alignment.text_end), (2, 5));
    }

    #[test]
    fn expected_word_span_maps_into_normalized_coordinates() {
        let text = "zero one two three four";
        let span = expected_token_span(text, 1, 3, &NormalizationProfile::default())
            .expect("span");
        assert!(span.start > 0);
        assert!(span.end > span.start);
    }

    #[test]
    fn span_accuracy_unions_fragmented_predictions() {
        let accuracy = span_accuracy(
            TokenSpan { start: 10, end: 30 },
            &[
                TokenSpan { start: 10, end: 18 },
                TokenSpan { start: 22, end: 30 },
            ],
        )
        .expect("accuracy");
        assert_eq!(accuracy.intersection_tokens, 16);
        assert_eq!(accuracy.union_tokens, 20);
        assert!((accuracy.intersection_over_union - 0.8).abs() < 1.0e-12);
    }
}
