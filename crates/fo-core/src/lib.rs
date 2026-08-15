#![forbid(unsafe_code)]

mod calibration;
mod chain;
mod composite;
mod error;
mod fingerprint;
mod index;
pub mod metrics;
mod model;
mod multiview;
mod normalize;
mod ranking;
mod search;
pub mod spectral;
mod verify;
mod winnow;

pub use calibration::{
    CALIBRATION_FEATURE_COUNT, CALIBRATION_SCHEMA_VERSION, CalibratedResult, CalibrationModel,
    CalibrationOptions, FeedbackExample, evidence_vector, feature_names,
};
pub use chain::{Anchor, AnchorChain, ChainOptions, chain_anchors};
pub use composite::{
    CompositeMatchBlock, CompositeSearchOptions, CompositeSearchResult,
};
pub use error::{FoError, Result};
pub use fingerprint::{Feature, Fingerprint, qgram_hashes};
pub use index::{Document, Index, IndexBuilder, IndexEntry};
pub use metrics::{
    EvaluationOptions, LabeledScore, PrecisionRecallPoint, PrecisionRecallReport,
    precision_recall_report,
};
pub use model::{
    IndexConfig, IndexStats, NormalizationProfile, Posting, PunctuationMode, SearchIntent,
    SearchOptions, SearchResult,
};
pub use multiview::{
    FeatureViewConfig, MultiViewConfig, MultiViewIndex, MultiViewIndexBuilder,
    MultiViewSearchResult, MultiViewStats, MultiViewViewStats, ViewEvidence,
};
pub use normalize::{NormalizedText, normalize};
pub use ranking::{
    GroupedFeedbackExample, PairwiseRankingOptions, RankedResult, RankingComparison, RankingModel,
    RANKING_FEATURE_COUNT, RANKING_SCHEMA_VERSION, mine_hard_negatives,
    ranking_evidence_vector, ranking_feature_names,
};
pub use spectral::{SpectralOptions, SpectralPeak, spectral_scan};
pub use verify::{
    Alignment, InfixCandidate, global_levenshtein, myers_infix_candidates, semi_global_banded,
};
pub use winnow::winnow;
