use serde::{Deserialize, Serialize};

use crate::error::{FoError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PunctuationMode {
    Keep,
    ToSpace,
    Drop,
}

impl PunctuationMode {
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Self::Keep => 0,
            Self::ToSpace => 1,
            Self::Drop => 2,
        }
    }

    pub(crate) fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Keep),
            1 => Ok(Self::ToSpace),
            2 => Ok(Self::Drop),
            _ => Err(FoError::InvalidIndex(format!(
                "unknown punctuation mode {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizationProfile {
    pub nfkc: bool,
    pub lowercase: bool,
    pub collapse_whitespace: bool,
    pub punctuation: PunctuationMode,
}

impl Default for NormalizationProfile {
    fn default() -> Self {
        Self {
            nfkc: true,
            lowercase: true,
            collapse_whitespace: true,
            punctuation: PunctuationMode::ToSpace,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexConfig {
    pub normalization: NormalizationProfile,
    pub qgram_size: usize,
    pub winnow_window: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            normalization: NormalizationProfile::default(),
            qgram_size: 7,
            winnow_window: 12,
        }
    }
}

impl IndexConfig {
    pub fn validate(&self) -> Result<()> {
        if !(2..=64).contains(&self.qgram_size) {
            return Err(FoError::InvalidConfig(
                "qgram_size must be between 2 and 64".to_owned(),
            ));
        }
        if !(1..=4096).contains(&self.winnow_window) {
            return Err(FoError::InvalidConfig(
                "winnow_window must be between 1 and 4096".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Posting {
    pub document_id: u32,
    pub position: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchIntent {
    AnyPassage,
    #[default]
    SourceAttribution,
    NearDuplicate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    #[serde(default)]
    pub intent: SearchIntent,
    pub max_results: usize,
    pub max_candidates: usize,
    pub max_postings_per_feature: usize,
    pub maximum_posting_pairs: u64,
    pub minimum_anchor_hits: u32,
    pub diagonal_bin_width: i64,
    pub candidate_suppression_bins: i64,
    pub anchor_diagonal_band: i64,
    pub maximum_anchors_per_candidate: usize,
    pub predecessor_lookback: usize,
    pub maximum_chain_gap: u32,
    pub verification_slack: usize,
    pub verification_band: usize,
    pub minimum_matched_tokens: usize,
    pub minimum_query_coverage: f32,
    pub minimum_source_coverage: f32,
    pub direct_fallback_work_limit: u64,
    pub short_query_candidates: usize,
    pub minimum_similarity: f32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            intent: SearchIntent::SourceAttribution,
            max_results: 20,
            max_candidates: 200,
            max_postings_per_feature: 50_000,
            maximum_posting_pairs: 25_000_000,
            minimum_anchor_hits: 2,
            diagonal_bin_width: 4,
            candidate_suppression_bins: 4,
            anchor_diagonal_band: 64,
            maximum_anchors_per_candidate: 4096,
            predecessor_lookback: 256,
            maximum_chain_gap: 8192,
            verification_slack: 192,
            verification_band: 256,
            minimum_matched_tokens: 24,
            minimum_query_coverage: 0.10,
            minimum_source_coverage: 0.10,
            direct_fallback_work_limit: 50_000_000,
            short_query_candidates: 8,
            minimum_similarity: 0.35,
        }
    }
}

impl SearchOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0 || self.max_candidates == 0 {
            return Err(FoError::InvalidConfig(
                "max_results and max_candidates must be positive".to_owned(),
            ));
        }
        if self.max_postings_per_feature == 0
            || self.maximum_posting_pairs == 0
            || self.minimum_anchor_hits == 0
            || self.maximum_anchors_per_candidate == 0
            || self.predecessor_lookback == 0
            || self.short_query_candidates == 0
            || self.direct_fallback_work_limit == 0
        {
            return Err(FoError::InvalidConfig(
                "posting, posting-pair, anchor, predecessor, fallback-work, and short-query limits must be positive"
                    .to_owned(),
            ));
        }
        if self.diagonal_bin_width <= 0 {
            return Err(FoError::InvalidConfig(
                "diagonal_bin_width must be positive".to_owned(),
            ));
        }
        if self.candidate_suppression_bins < 0 || self.anchor_diagonal_band < 0 {
            return Err(FoError::InvalidConfig(
                "candidate_suppression_bins and anchor_diagonal_band must be non-negative"
                    .to_owned(),
            ));
        }
        for (name, value) in [
            ("minimum_query_coverage", self.minimum_query_coverage),
            ("minimum_source_coverage", self.minimum_source_coverage),
            ("minimum_similarity", self.minimum_similarity),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "{name} must lie in [0, 1]"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub document_id: u32,
    pub path: String,
    #[serde(default)]
    pub intent: SearchIntent,
    pub corpus_start: usize,
    pub corpus_end: usize,
    pub query_start: usize,
    pub query_end: usize,
    pub edit_distance: usize,
    pub edit_similarity: f32,
    pub anchor_coverage: f32,
    #[serde(default)]
    pub query_coverage: f32,
    #[serde(default)]
    pub source_coverage: f32,
    pub anchor_score: f32,
    #[serde(default)]
    pub vote_support: f32,
    #[serde(default)]
    pub chain_consistency: f32,
    #[serde(default)]
    pub matched_tokens: usize,
    #[serde(default)]
    pub distinct_anchor_count: usize,
    #[serde(default)]
    pub estimated_false_matches: f64,
    pub combined_score: f32,
    pub matched_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub documents: usize,
    pub normalized_tokens: usize,
    pub distinct_fingerprints: usize,
    pub postings: usize,
}
