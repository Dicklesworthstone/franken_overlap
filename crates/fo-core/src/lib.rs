#![forbid(unsafe_code)]

mod active_learning;
mod batch;
mod calibration;
mod chain;
mod composite;
mod error;
mod fingerprint;
mod grouped_metrics;
mod index;
pub mod metrics;
mod model;
mod multiview;
mod normalize;
mod pan_metrics;
mod planner;
mod provenance;
mod ranking;
mod search;
mod segmented;
pub mod spectral;
mod verify;
mod winnow;

pub use active_learning::{
    ActiveLearningCandidate, ActiveLearningOptions, ActiveLearningSelection,
    select_active_learning_queue,
};
pub use batch::{BatchQuery, BatchSearchOptions, BatchSearchReport, BatchSearchResult};
pub use calibration::{
    CALIBRATION_FEATURE_COUNT, CALIBRATION_SCHEMA_VERSION, CalibratedResult, CalibrationModel,
    CalibrationOptions, FeedbackExample, evidence_vector, feature_names,
};
pub use chain::{Anchor, AnchorChain, ChainOptions, chain_anchors};
pub use composite::{CompositeMatchBlock, CompositeSearchOptions, CompositeSearchResult};
pub use error::{FoError, Result};
pub use fingerprint::{Feature, Fingerprint, qgram_hashes};
pub use grouped_metrics::{
    AtKMetric, ConfidenceInterval, GroupedEvaluationOptions, GroupedEvaluationReport,
    GroupedLabeledScore, ThresholdConstraints, grouped_evaluation_report, select_operating_point,
};
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
pub use pan_metrics::{PanAnnotation, PanEvaluationReport, pan_evaluate, plagdet_score};
pub use planner::{
    AdaptiveMatch, AdaptiveRoute, AdaptiveSearchReport, QueryAdvisory, QueryPlan,
    QueryPlannerOptions,
};
pub use provenance::{
    OriginalByteRange, ProvenanceNormalizedText, normalize_with_provenance,
};
pub use ranking::{
    GroupedFeedbackExample, PairwiseRankingOptions, RANKING_FEATURE_COUNT,
    RANKING_SCHEMA_VERSION, RankedResult, RankingComparison, RankingModel, mine_hard_negatives,
    ranking_evidence_vector, ranking_feature_names,
};
pub use segmented::{
    SegmentAppendReport, SegmentCompactionReport, SegmentDeleteReport, SegmentDescriptor,
    SegmentDocumentInput, SegmentDocumentRecord, SegmentVerificationReport, SegmentedIndex,
    SegmentedIndexStats, SegmentedManifest, SegmentedSearchResult,
};
pub use spectral::{SpectralOptions, SpectralPeak, spectral_scan};
pub use verify::{
    Alignment, InfixCandidate, global_levenshtein, myers_infix_candidates, semi_global_banded,
};
pub use winnow::winnow;
