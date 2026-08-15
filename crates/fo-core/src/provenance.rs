use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::model::{NormalizationProfile, PunctuationMode};
use crate::normalize::{NormalizedText, normalize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalByteRange {
    pub start: usize,
    pub end: usize,
}

impl OriginalByteRange {
    #[must_use]
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone)]
pub struct ProvenanceNormalizedText {
    pub original: String,
    pub normalized: NormalizedText,
    token_original_ranges: Vec<OriginalByteRange>,
}

impl ProvenanceNormalizedText {
    #[must_use]
    pub fn len(&self) -> usize {
        self.normalized.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }

    #[must_use]
    pub fn token_original_ranges(&self) -> &[OriginalByteRange] {
        &self.token_original_ranges
    }

    #[must_use]
    pub fn token_original_range(&self, token: usize) -> Option<OriginalByteRange> {
        self.token_original_ranges.get(token).copied()
    }

    #[must_use]
    pub fn original_range_for_tokens(
        &self,
        start: usize,
        end: usize,
    ) -> Option<OriginalByteRange> {
        let start = start.min(self.len());
        let end = end.min(self.len()).max(start);
        if start == end {
            return None;
        }
        let first = self.token_original_ranges[start];
        let last = self.token_original_ranges[end - 1];
        Some(OriginalByteRange {
            start: first.start.min(last.start),
            end: first.end.max(last.end),
        })
    }

    #[must_use]
    pub fn original_slice_for_tokens(&self, start: usize, end: usize) -> Option<&str> {
        let range = self.original_range_for_tokens(start, end)?;
        self.original.get(range.start..range.end)
    }

    #[must_use]
    pub fn normalized_slice_tokens(&self, start: usize, end: usize) -> &str {
        self.normalized.slice_tokens(start, end)
    }

    #[must_use]
    pub fn into_normalized(self) -> NormalizedText {
        self.normalized
    }
}

#[must_use]
pub fn normalize_with_provenance(
    input: &str,
    profile: &NormalizationProfile,
) -> ProvenanceNormalizedText {
    let mut output = String::with_capacity(input.len());
    let mut ranges = Vec::<OriginalByteRange>::with_capacity(input.chars().count());
    let mut last_was_space = true;

    for (original_start, grapheme) in input.grapheme_indices(true) {
        let original_range = OriginalByteRange {
            start: original_start,
            end: original_start.saturating_add(grapheme.len()),
        };
        let mut stage = if profile.nfkc {
            grapheme.nfkc().collect::<String>()
        } else {
            grapheme.to_owned()
        };
        if profile.lowercase {
            stage = stage.chars().flat_map(char::to_lowercase).collect();
        }

        for ch in stage.chars() {
            if ch.is_whitespace() {
                emit_mapped_space(
                    &mut output,
                    &mut ranges,
                    &mut last_was_space,
                    profile.collapse_whitespace,
                    ch,
                    original_range,
                );
                continue;
            }
            let wordish = ch.is_alphanumeric() || ch == '_';
            if wordish || matches!(profile.punctuation, PunctuationMode::Keep) {
                output.push(ch);
                ranges.push(original_range);
                last_was_space = false;
                continue;
            }
            match profile.punctuation {
                PunctuationMode::Keep => {}
                PunctuationMode::ToSpace => emit_mapped_space(
                    &mut output,
                    &mut ranges,
                    &mut last_was_space,
                    true,
                    ' ',
                    original_range,
                ),
                PunctuationMode::Drop => {}
            }
        }
    }

    while output.ends_with(' ') {
        output.pop();
        ranges.pop();
    }

    let candidate = NormalizedText::from_stored(output);
    let canonical = normalize(input, profile);
    let (normalized, token_original_ranges) = if candidate.text == canonical.text {
        (candidate, ranges)
    } else {
        let complete_input = OriginalByteRange {
            start: 0,
            end: input.len(),
        };
        let coarse_ranges = vec![complete_input; canonical.len()];
        (canonical, coarse_ranges)
    };
    debug_assert_eq!(normalized.len(), token_original_ranges.len());

    ProvenanceNormalizedText {
        original: input.to_owned(),
        normalized,
        token_original_ranges,
    }
}

fn emit_mapped_space(
    output: &mut String,
    ranges: &mut Vec<OriginalByteRange>,
    last_was_space: &mut bool,
    collapse: bool,
    original: char,
    original_range: OriginalByteRange,
) {
    if collapse {
        if *last_was_space || output.is_empty() {
            if let Some(range) = ranges.last_mut()
                && output.ends_with(' ')
            {
                range.start = range.start.min(original_range.start);
                range.end = range.end.max(original_range.end);
            }
            return;
        }
        output.push(' ');
        ranges.push(original_range);
        *last_was_space = true;
    } else {
        output.push(original);
        ranges.push(original_range);
        *last_was_space = original.is_whitespace();
    }
}

#[cfg(test)]
mod tests {
    use super::{OriginalByteRange, normalize_with_provenance};
    use crate::{NormalizationProfile, PunctuationMode, normalize};

    #[test]
    fn output_matches_the_existing_normalizer() {
        let profile = NormalizationProfile::default();
        for input in [
            "  Ｈe\u{301}llo,\tWORLD!  ",
            "가 각 ＡＢＣ",
            "A---  B",
            "Straße and İSTANBUL",
        ] {
            let mapped = normalize_with_provenance(input, &profile);
            assert_eq!(mapped.normalized.text, normalize(input, &profile).text);
            assert_eq!(mapped.normalized.len(), mapped.token_original_ranges().len());
        }
    }

    #[test]
    fn compatibility_expansion_points_back_to_the_original_bytes() {
        let mapped = normalize_with_provenance("Ａ", &NormalizationProfile::default());
        assert_eq!(mapped.normalized.text, "a");
        assert_eq!(
            mapped.token_original_range(0),
            Some(OriginalByteRange { start: 0, end: 3 })
        );
        assert_eq!(mapped.original_slice_for_tokens(0, 1), Some("Ａ"));
    }

    #[test]
    fn composed_grapheme_uses_the_complete_original_cluster() {
        let mapped = normalize_with_provenance("Cafe\u{301}", &NormalizationProfile::default());
        assert_eq!(mapped.normalized.text, "café");
        assert_eq!(
            mapped.token_original_range(3),
            Some(OriginalByteRange { start: 3, end: 6 })
        );
        assert_eq!(mapped.original_slice_for_tokens(3, 4), Some("e\u{301}"));
    }

    #[test]
    fn collapsed_separator_range_includes_every_consumed_byte() {
        let mapped = normalize_with_provenance("A---  B", &NormalizationProfile::default());
        assert_eq!(mapped.normalized.text, "a b");
        assert_eq!(mapped.original_slice_for_tokens(1, 2), Some("---  "));
    }

    #[test]
    fn kept_punctuation_retains_precise_provenance() {
        let profile = NormalizationProfile {
            punctuation: PunctuationMode::Keep,
            ..NormalizationProfile::default()
        };
        let mapped = normalize_with_provenance("A+B", &profile);
        assert_eq!(mapped.normalized.text, "a+b");
        assert_eq!(mapped.original_slice_for_tokens(1, 2), Some("+"));
    }
}
