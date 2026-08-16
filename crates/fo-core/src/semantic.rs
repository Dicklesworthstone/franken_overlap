use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FoError, HybridSearchReport, HybridSearchResult, Result};

pub const SEMANTIC_FUSION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticEvidence {
    pub external_id: String,
    #[serde(default)]
    pub title: String,
    pub score: f32,
    pub model: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub passage_start: Option<usize>,
    #[serde(default)]
    pub passage_end: Option<usize>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl SemanticEvidence {
    pub fn validate(&self) -> Result<()> {
        if self.external_id.trim().is_empty() || self.model.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "semantic external_id and model must not be empty".to_owned(),
            ));
        }
        if !self.score.is_finite() || !(0.0..=1.0).contains(&self.score) {
            return Err(FoError::InvalidConfig(
                "semantic score must be finite and lie in [0, 1]".to_owned(),
            ));
        }
        match (self.passage_start, self.passage_end) {
            (None, None) => {}
            (Some(start), Some(end)) if start < end => {}
            _ => {
                return Err(FoError::InvalidConfig(
                    "semantic passage coordinates must be absent together or form a nonempty span"
                        .to_owned(),
                ));
            }
        }
        if self.metadata.keys().any(|key| key.trim().is_empty()) {
            return Err(FoError::InvalidConfig(
                "semantic metadata keys must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCandidateSet {
    #[serde(default = "semantic_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub query_id: Option<String>,
    pub candidates: Vec<SemanticEvidence>,
}

impl SemanticCandidateSet {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SEMANTIC_FUSION_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported semantic candidate schema {}",
                self.schema_version
            )));
        }
        if self
            .query_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(FoError::InvalidConfig(
                "semantic query_id must not be empty when present".to_owned(),
            ));
        }
        for candidate in &self.candidates {
            candidate.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SemanticFusionOptions {
    pub max_results: usize,
    pub minimum_score: f32,
    pub minimum_semantic_score: f32,
    pub hybrid_weight: f32,
    pub semantic_weight: f32,
    pub reciprocal_rank_weight: f32,
    pub reciprocal_rank_constant: f32,
    pub agreement_bonus: f32,
    pub allow_semantic_only: bool,
    pub maximum_semantic_only: usize,
}

impl Default for SemanticFusionOptions {
    fn default() -> Self {
        Self {
            max_results: 20,
            minimum_score: 0.0,
            minimum_semantic_score: 0.0,
            hybrid_weight: 0.70,
            semantic_weight: 0.20,
            reciprocal_rank_weight: 0.10,
            reciprocal_rank_constant: 60.0,
            agreement_bonus: 0.05,
            allow_semantic_only: false,
            maximum_semantic_only: 5,
        }
    }
}

impl SemanticFusionOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0 || self.maximum_semantic_only == 0 {
            return Err(FoError::InvalidConfig(
                "semantic fusion result limits must be positive".to_owned(),
            ));
        }
        for (name, value) in [
            ("minimum_score", self.minimum_score),
            ("minimum_semantic_score", self.minimum_semantic_score),
            ("hybrid_weight", self.hybrid_weight),
            ("semantic_weight", self.semantic_weight),
            ("reciprocal_rank_weight", self.reciprocal_rank_weight),
            ("agreement_bonus", self.agreement_bonus),
        ] {
            if !value.is_finite() || value < 0.0 || value > 10.0 {
                return Err(FoError::InvalidConfig(format!(
                    "semantic fusion {name} must be finite and lie in [0, 10]"
                )));
            }
        }
        if self.minimum_score > 1.0 || self.minimum_semantic_score > 1.0 {
            return Err(FoError::InvalidConfig(
                "semantic fusion score floors must lie in [0, 1]".to_owned(),
            ));
        }
        if self.hybrid_weight + self.semantic_weight + self.reciprocal_rank_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "at least one semantic fusion weight must be positive".to_owned(),
            ));
        }
        if !self.reciprocal_rank_constant.is_finite()
            || self.reciprocal_rank_constant <= 0.0
        {
            return Err(FoError::InvalidConfig(
                "semantic reciprocal_rank_constant must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRelationshipClass {
    TextualProvenance,
    TextualAndSemantic,
    LexicalOnly,
    LexicalAndSemantic,
    SemanticOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticScoreExplanation {
    pub hybrid_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
    pub hybrid_score: f32,
    pub semantic_score: f32,
    pub reciprocal_rank_score: f32,
    pub agreement: bool,
    pub agreement_bonus: f32,
    pub active_weight: f32,
    pub base_score: f32,
    pub final_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticFusionResult {
    pub external_id: String,
    pub title: String,
    pub score: f32,
    pub relationship: SemanticRelationshipClass,
    pub textual_provenance_supported: bool,
    pub lexical_supported: bool,
    pub semantic_only: bool,
    #[serde(default)]
    pub hybrid: Option<HybridSearchResult>,
    pub semantic: Vec<SemanticEvidence>,
    pub explanation: SemanticScoreExplanation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticFusionReport {
    pub schema_version: u32,
    pub query_id: Option<String>,
    pub hybrid_candidates: usize,
    pub semantic_candidates: usize,
    pub overlapping_candidates: usize,
    pub semantic_only_candidates_considered: usize,
    pub semantic_only_candidates_retained: usize,
    pub results: Vec<SemanticFusionResult>,
}

#[derive(Debug, Default)]
struct Candidate {
    hybrid: Option<(usize, HybridSearchResult)>,
    semantic: Vec<(usize, SemanticEvidence)>,
}

pub fn fuse_semantic_candidates(
    hybrid: &HybridSearchReport,
    semantic: &SemanticCandidateSet,
    options: &SemanticFusionOptions,
) -> Result<SemanticFusionReport> {
    semantic.validate()?;
    options.validate()?;

    let mut candidates = BTreeMap::<String, Candidate>::new();
    for (rank, result) in hybrid.results.iter().cloned().enumerate() {
        candidates
            .entry(result.external_id.clone())
            .or_default()
            .hybrid = Some((rank + 1, result));
    }

    let mut semantic_ranked = semantic
        .candidates
        .iter()
        .filter(|candidate| candidate.score >= options.minimum_semantic_score)
        .cloned()
        .collect::<Vec<_>>();
    semantic_ranked.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.external_id.cmp(&right.external_id))
            .then_with(|| left.model.cmp(&right.model))
    });
    for (rank, evidence) in semantic_ranked.iter().cloned().enumerate() {
        candidates
            .entry(evidence.external_id.clone())
            .or_default()
            .semantic
            .push((rank + 1, evidence));
    }

    let overlapping_candidates = candidates
        .values()
        .filter(|candidate| candidate.hybrid.is_some() && !candidate.semantic.is_empty())
        .count();
    let semantic_only_candidates_considered = candidates
        .values()
        .filter(|candidate| candidate.hybrid.is_none() && !candidate.semantic.is_empty())
        .count();

    let mut output = Vec::new();
    let mut semantic_only_seen = 0usize;
    for (external_id, mut candidate) in candidates {
        if candidate.hybrid.is_none() && !options.allow_semantic_only {
            continue;
        }
        if candidate.hybrid.is_none() {
            if semantic_only_seen >= options.maximum_semantic_only {
                continue;
            }
            semantic_only_seen += 1;
        }
        candidate.semantic.sort_unstable_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.score.total_cmp(&left.1.score))
                .then_with(|| left.1.model.cmp(&right.1.model))
        });
        let hybrid_rank = candidate.hybrid.as_ref().map(|(rank, _)| *rank);
        let semantic_rank = candidate.semantic.first().map(|(rank, _)| *rank);
        let hybrid_score = candidate
            .hybrid
            .as_ref()
            .map_or(0.0, |(_, result)| result.score.clamp(0.0, 1.0));
        let semantic_score = aggregate_semantic_score(&candidate.semantic);
        let reciprocal_rank_score = reciprocal_rank_fusion(
            hybrid_rank,
            semantic_rank,
            options.reciprocal_rank_constant,
        );
        let agreement = candidate.hybrid.is_some() && !candidate.semantic.is_empty();
        let hybrid_component = if candidate.hybrid.is_some() {
            options.hybrid_weight
        } else {
            0.0
        };
        let semantic_component = if candidate.semantic.is_empty() {
            0.0
        } else {
            options.semantic_weight
        };
        let active_weight =
            hybrid_component + semantic_component + options.reciprocal_rank_weight;
        let weighted = options.hybrid_weight * hybrid_score
            + options.semantic_weight * semantic_score
            + options.reciprocal_rank_weight * reciprocal_rank_score;
        let base_score = if active_weight > 0.0 {
            weighted / active_weight
        } else {
            0.0
        };
        let agreement_bonus = if agreement {
            options.agreement_bonus
        } else {
            0.0
        };
        let final_score = (1.0
            - (1.0 - base_score.clamp(0.0, 1.0))
                * (1.0 - agreement_bonus.clamp(0.0, 0.75)))
            .clamp(0.0, 1.0);
        if final_score < options.minimum_score {
            continue;
        }

        let textual_provenance_supported = candidate
            .hybrid
            .as_ref()
            .is_some_and(|(_, result)| result.overlap.is_some());
        let lexical_supported = candidate
            .hybrid
            .as_ref()
            .is_some_and(|(_, result)| result.lexical.is_some());
        let semantic_only = candidate.hybrid.is_none();
        let has_semantic = !candidate.semantic.is_empty();
        let relationship = if semantic_only {
            SemanticRelationshipClass::SemanticOnly
        } else if textual_provenance_supported && has_semantic {
            SemanticRelationshipClass::TextualAndSemantic
        } else if textual_provenance_supported {
            SemanticRelationshipClass::TextualProvenance
        } else if has_semantic {
            SemanticRelationshipClass::LexicalAndSemantic
        } else {
            SemanticRelationshipClass::LexicalOnly
        };
        let title = candidate
            .hybrid
            .as_ref()
            .map(|(_, result)| result.title.clone())
            .or_else(|| {
                candidate
                    .semantic
                    .iter()
                    .map(|(_, evidence)| evidence.title.as_str())
                    .find(|title| !title.trim().is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| external_id.clone());
        output.push(SemanticFusionResult {
            external_id,
            title,
            score: final_score,
            relationship,
            textual_provenance_supported,
            lexical_supported,
            semantic_only,
            hybrid: candidate.hybrid.map(|(_, result)| result),
            semantic: candidate
                .semantic
                .into_iter()
                .map(|(_, evidence)| evidence)
                .collect(),
            explanation: SemanticScoreExplanation {
                hybrid_rank,
                semantic_rank,
                hybrid_score,
                semantic_score,
                reciprocal_rank_score,
                agreement,
                agreement_bonus,
                active_weight,
                base_score,
                final_score,
            },
        });
    }

    output.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                right
                    .textual_provenance_supported
                    .cmp(&left.textual_provenance_supported)
            })
            .then_with(|| left.external_id.cmp(&right.external_id))
    });
    output.truncate(options.max_results);

    Ok(SemanticFusionReport {
        schema_version: SEMANTIC_FUSION_SCHEMA_VERSION,
        query_id: semantic.query_id.clone(),
        hybrid_candidates: hybrid.results.len(),
        semantic_candidates: semantic_ranked.len(),
        overlapping_candidates,
        semantic_only_candidates_considered,
        semantic_only_candidates_retained: output
            .iter()
            .filter(|result| result.semantic_only)
            .count(),
        results: output,
    })
}

fn aggregate_semantic_score(evidence: &[(usize, SemanticEvidence)]) -> f32 {
    let miss_probability = evidence.iter().fold(1.0f64, |probability, (_, item)| {
        probability * (1.0 - f64::from(item.score).clamp(0.0, 1.0))
    });
    (1.0 - miss_probability).clamp(0.0, 1.0) as f32
}

fn reciprocal_rank_fusion(left: Option<usize>, right: Option<usize>, constant: f32) -> f32 {
    let contribution = |rank: Option<usize>| {
        rank.map_or(0.0, |rank| 1.0 / (constant + rank as f32))
    };
    let maximum = 2.0 / (constant + 1.0);
    if maximum <= 0.0 {
        0.0
    } else {
        ((contribution(left) + contribution(right)) / maximum).clamp(0.0, 1.0)
    }
}

const fn semantic_schema_version() -> u32 {
    SEMANTIC_FUSION_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        SemanticCandidateSet, SemanticEvidence, SemanticFusionOptions,
        SemanticRelationshipClass, fuse_semantic_candidates,
    };
    use crate::{
        HybridQueryMode, HybridScoreExplanation, HybridSearchReport, HybridSearchResult,
    };

    fn hybrid_result(id: &str, score: f32, overlap: bool) -> HybridSearchResult {
        HybridSearchResult {
            external_id: id.to_owned(),
            title: id.to_owned(),
            score,
            snippet: String::new(),
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            lexical: None,
            overlap: overlap.then(|| {
                crate::HybridOverlapEvidence::Passage(crate::SearchResult {
                    document_id: 0,
                    path: id.to_owned(),
                    intent: crate::SearchIntent::SourceAttribution,
                    corpus_start: 0,
                    corpus_end: 20,
                    query_start: 0,
                    query_end: 20,
                    edit_distance: 0,
                    edit_similarity: 1.0,
                    anchor_coverage: 1.0,
                    query_coverage: 1.0,
                    source_coverage: 0.5,
                    anchor_score: 1.0,
                    vote_support: 1.0,
                    chain_consistency: 1.0,
                    matched_tokens: 20,
                    distinct_anchor_count: 3,
                    estimated_false_matches: 0.0,
                    combined_score: score,
                    matched_text: "matched text".to_owned(),
                })
            }),
            explanation: HybridScoreExplanation {
                selected_mode: HybridQueryMode::Hybrid,
                lexical_rank: None,
                overlap_rank: Some(1),
                lexical_raw_score: 0.0,
                lexical_score: 0.0,
                overlap_score: score,
                reciprocal_rank_score: 1.0,
                agreement: false,
                agreement_bonus: 0.0,
                phrase_signal: 0.0,
                phrase_bonus: 0.0,
                base_score: score,
                final_score: score,
            },
        }
    }

    fn report(results: Vec<HybridSearchResult>) -> HybridSearchReport {
        HybridSearchReport {
            requested_mode: HybridQueryMode::Hybrid,
            selected_mode: HybridQueryMode::Hybrid,
            positive_terms: 4,
            positive_term_occurrences: 4,
            overlap_plan: None,
            lexical_candidates: 0,
            overlap_candidates: results.len(),
            results,
        }
    }

    fn semantic(id: &str, score: f32) -> SemanticEvidence {
        SemanticEvidence {
            external_id: id.to_owned(),
            title: id.to_owned(),
            score,
            model: "fixture-embedding".to_owned(),
            revision: Some("1".to_owned()),
            passage_start: None,
            passage_end: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn semantic_evidence_reranks_but_does_not_invent_provenance() {
        let hybrid = report(vec![
            hybrid_result("textual", 0.65, true),
            hybrid_result("lexical", 0.70, false),
        ]);
        let semantic = SemanticCandidateSet {
            schema_version: 1,
            query_id: Some("q".to_owned()),
            candidates: vec![semantic("textual", 0.90), semantic("semantic-only", 0.99)],
        };
        let fused = fuse_semantic_candidates(
            &hybrid,
            &semantic,
            &SemanticFusionOptions::default(),
        )
        .expect("fuse");
        assert_eq!(fused.results.len(), 2);
        assert_eq!(fused.results[0].external_id, "textual");
        assert!(fused.results[0].textual_provenance_supported);
        assert_eq!(
            fused.results[0].relationship,
            SemanticRelationshipClass::TextualAndSemantic
        );
        assert_eq!(
            fused.results[1].relationship,
            SemanticRelationshipClass::LexicalOnly
        );
        assert!(
            fused
                .results
                .iter()
                .all(|result| result.external_id != "semantic-only")
        );
    }

    #[test]
    fn semantic_only_candidates_remain_explicit_when_enabled() {
        let hybrid = report(Vec::new());
        let semantic = SemanticCandidateSet {
            schema_version: 1,
            query_id: None,
            candidates: vec![semantic("candidate", 0.9)],
        };
        let fused = fuse_semantic_candidates(
            &hybrid,
            &semantic,
            &SemanticFusionOptions {
                allow_semantic_only: true,
                ..SemanticFusionOptions::default()
            },
        )
        .expect("fuse");
        assert_eq!(fused.results.len(), 1);
        assert!(fused.results[0].semantic_only);
        assert!(!fused.results[0].textual_provenance_supported);
        assert_eq!(
            fused.results[0].relationship,
            SemanticRelationshipClass::SemanticOnly
        );
    }
}
