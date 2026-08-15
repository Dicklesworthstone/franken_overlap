use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{FoError, RANKING_FEATURE_COUNT, Result, SearchResult, ranking_evidence_vector};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveLearningCandidate {
    pub query_id: String,
    pub result: SearchResult,
    #[serde(default)]
    pub calibrated_probability: Option<f64>,
    #[serde(default)]
    pub ranking_score: Option<f64>,
    #[serde(default)]
    pub label: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActiveLearningOptions {
    pub maximum_examples: usize,
    pub maximum_input_candidates: usize,
    pub maximum_per_query: usize,
    pub maximum_per_document: usize,
    pub minimum_priority: f64,
    pub uncertainty_weight: f64,
    pub disagreement_weight: f64,
    pub hard_negative_weight: f64,
    pub novelty_weight: f64,
    pub include_labeled: bool,
}

impl Default for ActiveLearningOptions {
    fn default() -> Self {
        Self {
            maximum_examples: 200,
            maximum_input_candidates: 100_000,
            maximum_per_query: 3,
            maximum_per_document: 12,
            minimum_priority: 0.10,
            uncertainty_weight: 0.35,
            disagreement_weight: 0.25,
            hard_negative_weight: 0.25,
            novelty_weight: 0.15,
            include_labeled: false,
        }
    }
}

impl ActiveLearningOptions {
    pub fn validate(self) -> Result<Self> {
        if self.maximum_examples == 0
            || self.maximum_input_candidates == 0
            || self.maximum_per_query == 0
            || self.maximum_per_document == 0
        {
            return Err(FoError::InvalidConfig(
                "active-learning size and diversity limits must be positive".to_owned(),
            ));
        }
        if self.maximum_examples > self.maximum_input_candidates {
            return Err(FoError::InvalidConfig(
                "maximum_examples cannot exceed maximum_input_candidates".to_owned(),
            ));
        }
        if !self.minimum_priority.is_finite() || !(0.0..=1.0).contains(&self.minimum_priority) {
            return Err(FoError::InvalidConfig(
                "minimum_priority must be finite and lie in [0, 1]".to_owned(),
            ));
        }
        let weights = [
            self.uncertainty_weight,
            self.disagreement_weight,
            self.hard_negative_weight,
            self.novelty_weight,
        ];
        if weights.iter().any(|weight| !weight.is_finite() || *weight < 0.0)
            || weights.iter().sum::<f64>() <= 0.0
        {
            return Err(FoError::InvalidConfig(
                "active-learning weights must be finite, non-negative, and have a positive sum"
                    .to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveLearningSelection {
    pub priority: f64,
    pub uncertainty: f64,
    pub model_disagreement: f64,
    pub hard_negative_risk: f64,
    pub evidence_novelty: f64,
    pub recommended_feedback_weight: f64,
    pub candidate: ActiveLearningCandidate,
}

#[derive(Debug, Clone)]
struct PreparedCandidate {
    original: ActiveLearningCandidate,
    evidence: [f64; RANKING_FEATURE_COUNT],
    uncertainty: f64,
    disagreement: f64,
    hard_negative_risk: f64,
    base_priority: f64,
}

pub fn select_active_learning_queue(
    candidates: &[ActiveLearningCandidate],
    options: ActiveLearningOptions,
) -> Result<Vec<ActiveLearningSelection>> {
    let options = options.validate()?;
    if candidates.len() > options.maximum_input_candidates {
        return Err(FoError::InvalidConfig(format!(
            "active-learning input has {} candidates; limit is {}",
            candidates.len(), options.maximum_input_candidates
        )));
    }
    let mut prepared = Vec::with_capacity(candidates.len());
    let mut seen = HashSet::<(String, u32, usize, usize)>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        validate_candidate(candidate, index)?;
        if candidate.label.is_some() && !options.include_labeled {
            continue;
        }
        let key = (
            candidate.query_id.clone(),
            candidate.result.document_id,
            candidate.result.corpus_start,
            candidate.result.corpus_end,
        );
        if !seen.insert(key) {
            continue;
        }
        prepared.push(prepare_candidate(candidate.clone(), options));
    }

    let mut selected = Vec::<ActiveLearningSelection>::new();
    let mut selected_vectors = Vec::<[f64; RANKING_FEATURE_COUNT]>::new();
    let mut query_counts = HashMap::<String, usize>::new();
    let mut document_counts = HashMap::<u32, usize>::new();
    let mut consumed = vec![false; prepared.len()];

    while selected.len() < options.maximum_examples {
        let mut best: Option<(usize, f64, f64)> = None;
        for (index, candidate) in prepared.iter().enumerate() {
            if consumed[index]
                || query_counts
                    .get(&candidate.original.query_id)
                    .copied()
                    .unwrap_or(0)
                    >= options.maximum_per_query
                || document_counts
                    .get(&candidate.original.result.document_id)
                    .copied()
                    .unwrap_or(0)
                    >= options.maximum_per_document
            {
                continue;
            }
            let novelty = evidence_novelty(&candidate.evidence, &selected_vectors);
            let priority = combine_priority(candidate, novelty, options);
            if priority < options.minimum_priority {
                continue;
            }
            let replace = best.as_ref().is_none_or(|(best_index, best_priority, best_novelty)| {
                priority.total_cmp(best_priority).is_gt()
                    || (priority.total_cmp(best_priority).is_eq()
                        && (novelty.total_cmp(best_novelty).is_gt()
                            || (novelty.total_cmp(best_novelty).is_eq()
                                && deterministic_order(
                                    &candidate.original,
                                    &prepared[*best_index].original,
                                )
                                .is_lt())))
            });
            if replace {
                best = Some((index, priority, novelty));
            }
        }
        let Some((index, priority, novelty)) = best else {
            break;
        };
        consumed[index] = true;
        let candidate = &prepared[index];
        *query_counts
            .entry(candidate.original.query_id.clone())
            .or_default() += 1;
        *document_counts
            .entry(candidate.original.result.document_id)
            .or_default() += 1;
        selected_vectors.push(candidate.evidence);
        selected.push(ActiveLearningSelection {
            priority,
            uncertainty: candidate.uncertainty,
            model_disagreement: candidate.disagreement,
            hard_negative_risk: candidate.hard_negative_risk,
            evidence_novelty: novelty,
            recommended_feedback_weight: 0.5 + 1.5 * priority,
            candidate: candidate.original.clone(),
        });
    }
    Ok(selected)
}

fn prepare_candidate(
    candidate: ActiveLearningCandidate,
    options: ActiveLearningOptions,
) -> PreparedCandidate {
    let raw = f64::from(candidate.result.combined_score.clamp(0.0, 1.0));
    let probability = candidate
        .calibrated_probability
        .or(candidate.ranking_score)
        .unwrap_or(raw);
    let ranking = candidate.ranking_score.unwrap_or(probability);
    let uncertainty = (1.0 - 2.0 * (probability - 0.5).abs()).clamp(0.0, 1.0);
    let disagreement = (probability - raw)
        .abs()
        .max((ranking - raw).abs())
        .max((probability - ranking).abs())
        .clamp(0.0, 1.0);
    let false_match_confidence =
        1.0 / (1.0 + candidate.result.estimated_false_matches.max(0.0));
    let support = (0.28 * f64::from(candidate.result.query_coverage.clamp(0.0, 1.0))
        + 0.22 * f64::from(candidate.result.anchor_coverage.clamp(0.0, 1.0))
        + 0.18 * f64::from(candidate.result.vote_support.clamp(0.0, 1.0))
        + 0.22 * f64::from(candidate.result.chain_consistency.clamp(0.0, 1.0))
        + 0.10 * false_match_confidence)
        .clamp(0.0, 1.0);
    let model_rejection = (1.0 - probability).clamp(0.0, 1.0);
    let hard_negative_risk =
        (raw * (0.60 * (1.0 - support) + 0.40 * model_rejection)).clamp(0.0, 1.0);
    let base_weight = options.uncertainty_weight
        + options.disagreement_weight
        + options.hard_negative_weight;
    let base_priority = if base_weight > 0.0 {
        (options.uncertainty_weight * uncertainty
            + options.disagreement_weight * disagreement
            + options.hard_negative_weight * hard_negative_risk)
            / base_weight
    } else {
        0.0
    };
    PreparedCandidate {
        evidence: ranking_evidence_vector(&candidate.result),
        original: candidate,
        uncertainty,
        disagreement,
        hard_negative_risk,
        base_priority,
    }
}

fn combine_priority(
    candidate: &PreparedCandidate,
    novelty: f64,
    options: ActiveLearningOptions,
) -> f64 {
    let base_weight = options.uncertainty_weight
        + options.disagreement_weight
        + options.hard_negative_weight;
    let total_weight = base_weight + options.novelty_weight;
    if total_weight <= 0.0 {
        return 0.0;
    }
    ((base_weight * candidate.base_priority + options.novelty_weight * novelty) / total_weight)
        .clamp(0.0, 1.0)
}

fn evidence_novelty(
    evidence: &[f64; RANKING_FEATURE_COUNT],
    selected: &[[f64; RANKING_FEATURE_COUNT]],
) -> f64 {
    if selected.is_empty() {
        return 1.0;
    }
    selected
        .iter()
        .map(|other| normalized_distance(evidence, other))
        .fold(1.0, f64::min)
        .clamp(0.0, 1.0)
}

fn normalized_distance(
    left: &[f64; RANKING_FEATURE_COUNT],
    right: &[f64; RANKING_FEATURE_COUNT],
) -> f64 {
    let squared = left
        .iter()
        .zip(right)
        .map(|(left, right)| (*left - *right).powi(2))
        .sum::<f64>();
    (squared / RANKING_FEATURE_COUNT as f64).sqrt().clamp(0.0, 1.0)
}

fn validate_candidate(candidate: &ActiveLearningCandidate, index: usize) -> Result<()> {
    if candidate.query_id.trim().is_empty() {
        return Err(FoError::InvalidConfig(format!(
            "active-learning candidate {index} has an empty query_id"
        )));
    }
    for (name, value) in [
        ("calibrated_probability", candidate.calibrated_probability),
        ("ranking_score", candidate.ranking_score),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return Err(FoError::InvalidConfig(format!(
                "active-learning candidate {index} has invalid {name}"
            )));
        }
    }
    if ranking_evidence_vector(&candidate.result)
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err(FoError::InvalidConfig(format!(
            "active-learning candidate {index} contains non-finite search evidence"
        )));
    }
    Ok(())
}

fn deterministic_order(
    left: &ActiveLearningCandidate,
    right: &ActiveLearningCandidate,
) -> std::cmp::Ordering {
    left.query_id
        .cmp(&right.query_id)
        .then_with(|| left.result.document_id.cmp(&right.result.document_id))
        .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
        .then_with(|| left.result.corpus_end.cmp(&right.result.corpus_end))
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveLearningCandidate, ActiveLearningOptions, select_active_learning_queue,
    };
    use crate::{SearchIntent, SearchResult};

    fn result(document_id: u32, raw: f32, coverage: f32, chain: f32) -> SearchResult {
        SearchResult {
            document_id,
            path: format!("document-{document_id}"),
            intent: SearchIntent::SourceAttribution,
            corpus_start: 0,
            corpus_end: 80,
            query_start: 0,
            query_end: 80,
            edit_distance: 8,
            edit_similarity: 0.90,
            anchor_coverage: coverage,
            query_coverage: coverage,
            source_coverage: 0.40,
            anchor_score: 1.0,
            vote_support: coverage,
            chain_consistency: chain,
            matched_tokens: 72,
            distinct_anchor_count: 8,
            estimated_false_matches: f64::from(1.0 - chain),
            combined_score: raw,
            matched_text: "fixture".to_owned(),
        }
    }

    fn candidate(
        query: &str,
        document_id: u32,
        raw: f32,
        probability: f64,
        coverage: f32,
        chain: f32,
    ) -> ActiveLearningCandidate {
        ActiveLearningCandidate {
            query_id: query.to_owned(),
            result: result(document_id, raw, coverage, chain),
            calibrated_probability: Some(probability),
            ranking_score: None,
            label: None,
        }
    }

    #[test]
    fn prioritizes_uncertain_and_disputed_examples() {
        let candidates = vec![
            candidate("q1", 1, 0.90, 0.95, 0.90, 0.95),
            candidate("q2", 2, 0.88, 0.48, 0.25, 0.35),
            candidate("q3", 3, 0.15, 0.05, 0.10, 0.10),
        ];
        let selected = select_active_learning_queue(
            &candidates,
            ActiveLearningOptions {
                maximum_examples: 1,
                ..ActiveLearningOptions::default()
            },
        )
        .expect("queue");
        assert_eq!(selected[0].candidate.query_id, "q2");
        assert!(selected[0].model_disagreement > 0.3);
    }

    #[test]
    fn enforces_query_and_document_caps() {
        let candidates = vec![
            candidate("q1", 1, 0.8, 0.5, 0.4, 0.4),
            candidate("q1", 2, 0.8, 0.5, 0.5, 0.5),
            candidate("q2", 1, 0.8, 0.5, 0.6, 0.6),
            candidate("q3", 3, 0.8, 0.5, 0.7, 0.7),
        ];
        let selected = select_active_learning_queue(
            &candidates,
            ActiveLearningOptions {
                maximum_examples: 4,
                maximum_per_query: 1,
                maximum_per_document: 1,
                minimum_priority: 0.0,
                ..ActiveLearningOptions::default()
            },
        )
        .expect("queue");
        let q1_count = selected
            .iter()
            .filter(|selection| selection.candidate.query_id == "q1")
            .count();
        let document_one_count = selected
            .iter()
            .filter(|selection| selection.candidate.result.document_id == 1)
            .count();
        assert_eq!(q1_count, 1);
        assert_eq!(document_one_count, 1);
    }

    #[test]
    fn skips_already_labeled_examples_by_default() {
        let mut labeled = candidate("q1", 1, 0.8, 0.5, 0.4, 0.4);
        labeled.label = Some(false);
        let selected = select_active_learning_queue(
            &[labeled],
            ActiveLearningOptions::default(),
        )
        .expect("queue");
        assert!(selected.is_empty());
    }
}
