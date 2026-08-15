use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    EvaluationOptions, FoError, LabeledScore, PrecisionRecallReport, Result, SearchIntent,
    SearchResult, precision_recall_report,
};

pub const RANKING_FEATURE_COUNT: usize = 14;
pub const RANKING_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupedFeedbackExample {
    pub query_id: String,
    pub result: SearchResult,
    pub label: bool,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PairwiseRankingOptions {
    pub epochs: usize,
    pub learning_rate: f64,
    pub l2: f64,
    pub convergence_tolerance: f64,
    pub max_negatives_per_positive: usize,
}

impl Default for PairwiseRankingOptions {
    fn default() -> Self {
        Self {
            epochs: 600,
            learning_rate: 0.08,
            l2: 0.01,
            convergence_tolerance: 1.0e-9,
            max_negatives_per_positive: 16,
        }
    }
}

impl PairwiseRankingOptions {
    pub fn validate(self) -> Result<Self> {
        if self.epochs == 0 || self.epochs > 1_000_000 {
            return Err(FoError::InvalidConfig(
                "ranking epochs must be between 1 and 1,000,000".to_owned(),
            ));
        }
        if !self.learning_rate.is_finite()
            || self.learning_rate <= 0.0
            || self.learning_rate > 10.0
        {
            return Err(FoError::InvalidConfig(
                "ranking learning_rate must be finite and lie in (0, 10]".to_owned(),
            ));
        }
        if !self.l2.is_finite() || self.l2 < 0.0 {
            return Err(FoError::InvalidConfig(
                "ranking l2 must be finite and non-negative".to_owned(),
            ));
        }
        if !self.convergence_tolerance.is_finite() || self.convergence_tolerance < 0.0 {
            return Err(FoError::InvalidConfig(
                "ranking convergence_tolerance must be finite and non-negative".to_owned(),
            ));
        }
        if self.max_negatives_per_positive == 0
            || self.max_negatives_per_positive > 1_000_000
        {
            return Err(FoError::InvalidConfig(
                "max_negatives_per_positive must be between 1 and 1,000,000".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingModel {
    pub schema_version: u32,
    pub feature_names: Vec<String>,
    pub means: Vec<f64>,
    pub scales: Vec<f64>,
    pub weights: Vec<f64>,
    pub training_examples: usize,
    pub training_queries: usize,
    pub training_pairs: usize,
    pub completed_epochs: usize,
    pub raw_training_report: PrecisionRecallReport,
    pub ranked_training_report: PrecisionRecallReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedResult {
    pub ranking_score: f64,
    pub result: SearchResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingComparison {
    pub raw: PrecisionRecallReport,
    pub ranked: PrecisionRecallReport,
    pub auprc_delta: f64,
    pub brier_delta: f64,
}

#[derive(Debug, Clone, Copy)]
struct TrainingPair {
    positive: usize,
    negative: usize,
    weight: f64,
}

impl RankingModel {
    pub fn fit(
        examples: &[GroupedFeedbackExample],
        options: PairwiseRankingOptions,
        evaluation: EvaluationOptions,
    ) -> Result<Self> {
        let options = options.validate()?;
        validate_grouped_feedback(examples)?;
        let rows = examples
            .iter()
            .map(|example| ranking_evidence_vector(&example.result))
            .collect::<Vec<_>>();
        let means = feature_means(&rows);
        let scales = feature_scales(&rows, &means);
        let standardized = rows
            .iter()
            .map(|row| standardize(row, &means, &scales))
            .collect::<Vec<_>>();
        let grouped = group_indices(examples);
        let pairs = build_training_pairs(examples, &grouped, options.max_negatives_per_positive);
        if pairs.is_empty() {
            return Err(FoError::InvalidConfig(
                "pairwise ranking requires at least one query with both a positive and a negative"
                    .to_owned(),
            ));
        }
        let total_pair_weight = pairs.iter().map(|pair| pair.weight).sum::<f64>();
        if !total_pair_weight.is_finite() || total_pair_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "pairwise training weights have an invalid total".to_owned(),
            ));
        }

        let mut weights = [0.0f64; RANKING_FEATURE_COUNT];
        let mut prior_loss = f64::INFINITY;
        let mut stable_epochs = 0usize;
        let mut completed_epochs = 0usize;
        for epoch in 0..options.epochs {
            let mut gradient = [0.0f64; RANKING_FEATURE_COUNT];
            let mut loss = 0.0;
            for pair in &pairs {
                let difference = subtract(
                    &standardized[pair.positive],
                    &standardized[pair.negative],
                );
                let margin = dot(&weights, &difference);
                let derivative = -sigmoid(-margin) * pair.weight;
                for (gradient_value, difference_value) in
                    gradient.iter_mut().zip(difference)
                {
                    *gradient_value += derivative * difference_value;
                }
                loss += pair.weight * softplus(-margin);
            }
            loss /= total_pair_weight;
            loss += 0.5 * options.l2 * weights.iter().map(|weight| weight * weight).sum::<f64>();

            let learning_rate =
                options.learning_rate / (1.0 + epoch as f64 / 100.0).sqrt();
            for (weight, gradient_value) in weights.iter_mut().zip(gradient) {
                let regularized = gradient_value / total_pair_weight + options.l2 * *weight;
                *weight -= learning_rate * regularized;
            }
            completed_epochs = epoch + 1;

            if (prior_loss - loss).abs() <= options.convergence_tolerance {
                stable_epochs += 1;
                if stable_epochs >= 12 {
                    break;
                }
            } else {
                stable_epochs = 0;
            }
            prior_loss = loss;
        }

        let raw_scores = examples
            .iter()
            .map(|example| LabeledScore {
                score: f64::from(example.result.combined_score.clamp(0.0, 1.0)),
                label: example.label,
            })
            .collect::<Vec<_>>();
        let mut model = Self {
            schema_version: RANKING_SCHEMA_VERSION,
            feature_names: ranking_feature_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            means: means.to_vec(),
            scales: scales.to_vec(),
            weights: weights.to_vec(),
            training_examples: examples.len(),
            training_queries: grouped.len(),
            training_pairs: pairs.len(),
            completed_epochs,
            raw_training_report: precision_recall_report(&raw_scores, evaluation)?,
            ranked_training_report: precision_recall_report(&raw_scores, evaluation)?,
        };
        let ranked_scores = examples
            .iter()
            .map(|example| LabeledScore {
                score: model.predict(&example.result),
                label: example.label,
            })
            .collect::<Vec<_>>();
        model.ranked_training_report = precision_recall_report(&ranked_scores, evaluation)?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != RANKING_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported ranking schema version {}",
                self.schema_version
            )));
        }
        for (name, values) in [
            ("feature_names", self.feature_names.len()),
            ("means", self.means.len()),
            ("scales", self.scales.len()),
            ("weights", self.weights.len()),
        ] {
            if values != RANKING_FEATURE_COUNT {
                return Err(FoError::InvalidConfig(format!(
                    "ranking {name} has {values} values; expected {RANKING_FEATURE_COUNT}"
                )));
            }
        }
        if self
            .means
            .iter()
            .chain(&self.scales)
            .chain(&self.weights)
            .any(|value| !value.is_finite())
            || self.scales.iter().any(|scale| *scale <= 0.0)
        {
            return Err(FoError::InvalidConfig(
                "ranking model contains non-finite values or non-positive scales".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn predict(&self, result: &SearchResult) -> f64 {
        let row = ranking_evidence_vector(result);
        let mut linear = 0.0;
        for (((value, mean), scale), weight) in row
            .iter()
            .zip(&self.means)
            .zip(&self.scales)
            .zip(&self.weights)
        {
            linear += *weight * ((*value - *mean) / *scale);
        }
        sigmoid(linear)
    }

    pub fn rerank(&self, results: &[SearchResult]) -> Result<Vec<RankedResult>> {
        self.validate()?;
        let mut ranked = results
            .iter()
            .cloned()
            .map(|result| RankedResult {
                ranking_score: self.predict(&result),
                result,
            })
            .collect::<Vec<_>>();
        ranked.sort_unstable_by(|left, right| {
            right
                .ranking_score
                .total_cmp(&left.ranking_score)
                .then_with(|| {
                    right
                        .result
                        .combined_score
                        .total_cmp(&left.result.combined_score)
                })
                .then_with(|| left.result.document_id.cmp(&right.result.document_id))
                .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
        });
        Ok(ranked)
    }

    pub fn compare(
        &self,
        examples: &[GroupedFeedbackExample],
        evaluation: EvaluationOptions,
    ) -> Result<RankingComparison> {
        self.validate()?;
        validate_grouped_feedback(examples)?;
        let raw = examples
            .iter()
            .map(|example| LabeledScore {
                score: f64::from(example.result.combined_score.clamp(0.0, 1.0)),
                label: example.label,
            })
            .collect::<Vec<_>>();
        let ranked = examples
            .iter()
            .map(|example| LabeledScore {
                score: self.predict(&example.result),
                label: example.label,
            })
            .collect::<Vec<_>>();
        let raw = precision_recall_report(&raw, evaluation)?;
        let ranked = precision_recall_report(&ranked, evaluation)?;
        Ok(RankingComparison {
            auprc_delta: ranked.average_precision - raw.average_precision,
            brier_delta: ranked.brier_score - raw.brier_score,
            raw,
            ranked,
        })
    }
}

pub fn mine_hard_negatives(
    examples: &[GroupedFeedbackExample],
    maximum_negatives_per_query: usize,
) -> Result<Vec<GroupedFeedbackExample>> {
    validate_grouped_feedback(examples)?;
    if maximum_negatives_per_query == 0 {
        return Err(FoError::InvalidConfig(
            "maximum_negatives_per_query must be positive".to_owned(),
        ));
    }
    let grouped = group_indices(examples);
    let mut selected = Vec::new();
    for indices in grouped.values() {
        let mut positives = indices
            .iter()
            .copied()
            .filter(|&index| examples[index].label)
            .collect::<Vec<_>>();
        let mut negatives = indices
            .iter()
            .copied()
            .filter(|&index| !examples[index].label)
            .collect::<Vec<_>>();
        positives.sort_unstable();
        negatives.sort_unstable_by(|&left, &right| {
            hardness(&examples[right].result)
                .total_cmp(&hardness(&examples[left].result))
                .then_with(|| left.cmp(&right))
        });
        selected.extend(positives.into_iter().map(|index| examples[index].clone()));
        selected.extend(
            negatives
                .into_iter()
                .take(maximum_negatives_per_query)
                .map(|index| examples[index].clone()),
        );
    }
    selected.sort_unstable_by(|left, right| {
        left.query_id
            .cmp(&right.query_id)
            .then_with(|| right.label.cmp(&left.label))
            .then_with(|| {
                right
                    .result
                    .combined_score
                    .total_cmp(&left.result.combined_score)
            })
            .then_with(|| left.result.document_id.cmp(&right.result.document_id))
    });
    Ok(selected)
}

#[must_use]
pub fn ranking_evidence_vector(result: &SearchResult) -> [f64; RANKING_FEATURE_COUNT] {
    let raw = f64::from(result.combined_score.clamp(0.0, 1.0));
    let edit = f64::from(result.edit_similarity.clamp(0.0, 1.0));
    let query = f64::from(result.query_coverage.clamp(0.0, 1.0));
    let source = f64::from(result.source_coverage.clamp(0.0, 1.0));
    let anchor = f64::from(result.anchor_coverage.clamp(0.0, 1.0));
    let vote = f64::from(result.vote_support.clamp(0.0, 1.0));
    let chain = f64::from(result.chain_consistency.clamp(0.0, 1.0));
    let length_factor = 1.0 - (-(result.matched_tokens as f64) / 32.0).exp();
    let anchor_count_factor =
        1.0 - (-(result.distinct_anchor_count as f64) / 8.0).exp();
    let false_match_confidence = 1.0 / (1.0 + result.estimated_false_matches.max(0.0));
    [
        raw,
        edit,
        query,
        source,
        anchor,
        vote,
        chain,
        length_factor,
        anchor_count_factor,
        false_match_confidence,
        edit * query,
        query * chain,
        anchor * vote,
        harmonic_mean(query, source),
    ]
}

#[must_use]
pub fn ranking_feature_names() -> [&'static str; RANKING_FEATURE_COUNT] {
    [
        "raw_combined_score",
        "edit_similarity",
        "query_coverage",
        "source_coverage",
        "anchor_coverage",
        "vote_support",
        "chain_consistency",
        "matched_length_saturation",
        "anchor_count_saturation",
        "false_match_confidence",
        "edit_x_query_coverage",
        "query_coverage_x_chain_consistency",
        "anchor_coverage_x_vote_support",
        "bidirectional_coverage_harmonic_mean",
    ]
}

fn validate_grouped_feedback(examples: &[GroupedFeedbackExample]) -> Result<()> {
    if examples.len() < 2 {
        return Err(FoError::InvalidConfig(
            "at least two grouped feedback examples are required".to_owned(),
        ));
    }
    let mut has_positive = false;
    let mut has_negative = false;
    for (index, example) in examples.iter().enumerate() {
        if example.query_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(format!(
                "grouped feedback example {index} has an empty query_id"
            )));
        }
        if !example.weight.is_finite() || example.weight <= 0.0 {
            return Err(FoError::InvalidConfig(format!(
                "grouped feedback example {index} has invalid weight {}",
                example.weight
            )));
        }
        if ranking_evidence_vector(&example.result)
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(FoError::InvalidConfig(format!(
                "grouped feedback example {index} contains non-finite evidence"
            )));
        }
        has_positive |= example.label;
        has_negative |= !example.label;
    }
    if !has_positive || !has_negative {
        return Err(FoError::InvalidConfig(
            "grouped feedback requires at least one positive and one negative".to_owned(),
        ));
    }
    Ok(())
}

fn group_indices(examples: &[GroupedFeedbackExample]) -> BTreeMap<&str, Vec<usize>> {
    let mut grouped = BTreeMap::<&str, Vec<usize>>::new();
    for (index, example) in examples.iter().enumerate() {
        grouped.entry(example.query_id.as_str()).or_default().push(index);
    }
    grouped
}

fn build_training_pairs(
    examples: &[GroupedFeedbackExample],
    grouped: &BTreeMap<&str, Vec<usize>>,
    maximum_negatives: usize,
) -> Vec<TrainingPair> {
    let mut pairs = Vec::new();
    for indices in grouped.values() {
        let positives = indices
            .iter()
            .copied()
            .filter(|&index| examples[index].label)
            .collect::<Vec<_>>();
        let mut negatives = indices
            .iter()
            .copied()
            .filter(|&index| !examples[index].label)
            .collect::<Vec<_>>();
        negatives.sort_unstable_by(|&left, &right| {
            hardness(&examples[right].result)
                .total_cmp(&hardness(&examples[left].result))
                .then_with(|| left.cmp(&right))
        });
        for positive in positives {
            for &negative in negatives.iter().take(maximum_negatives) {
                pairs.push(TrainingPair {
                    positive,
                    negative,
                    weight: (examples[positive].weight * examples[negative].weight).sqrt(),
                });
            }
        }
    }
    pairs
}

fn hardness(result: &SearchResult) -> f64 {
    f64::from(result.combined_score.clamp(0.0, 1.0))
        + 0.20 * f64::from(result.edit_similarity.clamp(0.0, 1.0))
        + 0.15 * f64::from(result.query_coverage.clamp(0.0, 1.0))
        + 0.10 * f64::from(result.chain_consistency.clamp(0.0, 1.0))
}

fn feature_means(
    rows: &[[f64; RANKING_FEATURE_COUNT]],
) -> [f64; RANKING_FEATURE_COUNT] {
    let mut means = [0.0; RANKING_FEATURE_COUNT];
    for row in rows {
        for (mean, value) in means.iter_mut().zip(row) {
            *mean += *value;
        }
    }
    for mean in &mut means {
        *mean /= rows.len() as f64;
    }
    means
}

fn feature_scales(
    rows: &[[f64; RANKING_FEATURE_COUNT]],
    means: &[f64; RANKING_FEATURE_COUNT],
) -> [f64; RANKING_FEATURE_COUNT] {
    let mut scales = [0.0; RANKING_FEATURE_COUNT];
    for row in rows {
        for ((scale, value), mean) in scales.iter_mut().zip(row).zip(means) {
            *scale += (*value - *mean).powi(2);
        }
    }
    for scale in &mut scales {
        *scale = (*scale / rows.len() as f64).sqrt();
        if *scale < 1.0e-9 {
            *scale = 1.0;
        }
    }
    scales
}

fn standardize(
    row: &[f64; RANKING_FEATURE_COUNT],
    means: &[f64; RANKING_FEATURE_COUNT],
    scales: &[f64; RANKING_FEATURE_COUNT],
) -> [f64; RANKING_FEATURE_COUNT] {
    std::array::from_fn(|feature| (row[feature] - means[feature]) / scales[feature])
}

fn subtract(
    left: &[f64; RANKING_FEATURE_COUNT],
    right: &[f64; RANKING_FEATURE_COUNT],
) -> [f64; RANKING_FEATURE_COUNT] {
    std::array::from_fn(|feature| left[feature] - right[feature])
}

fn dot(
    left: &[f64; RANKING_FEATURE_COUNT],
    right: &[f64; RANKING_FEATURE_COUNT],
) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| *left * *right)
        .sum()
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponential = value.exp();
        exponential / (1.0 + exponential)
    }
}

fn softplus(value: f64) -> f64 {
    if value > 0.0 {
        value + (-value).exp().ln_1p()
    } else {
        value.exp().ln_1p()
    }
}

fn harmonic_mean(left: f64, right: f64) -> f64 {
    if left + right <= 0.0 {
        0.0
    } else {
        2.0 * left * right / (left + right)
    }
}

fn default_weight() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::{
        GroupedFeedbackExample, PairwiseRankingOptions, RankingModel, mine_hard_negatives,
    };
    use crate::{EvaluationOptions, SearchIntent, SearchResult};

    fn result(raw: f32, edit: f32, query: f32, chain: f32, document_id: u32) -> SearchResult {
        SearchResult {
            document_id,
            path: format!("document-{document_id}"),
            intent: SearchIntent::SourceAttribution,
            corpus_start: 0,
            corpus_end: 96,
            query_start: 0,
            query_end: 96,
            edit_distance: ((1.0 - edit) * 96.0) as usize,
            edit_similarity: edit,
            anchor_coverage: query,
            query_coverage: query,
            source_coverage: 0.40,
            anchor_score: 1.0,
            vote_support: query,
            chain_consistency: chain,
            matched_tokens: (query * 96.0) as usize,
            distinct_anchor_count: 8,
            estimated_false_matches: f64::from(1.0 - chain),
            combined_score: raw,
            matched_text: "fixture".to_owned(),
        }
    }

    fn example(
        query_id: &str,
        label: bool,
        raw: f32,
        edit: f32,
        query: f32,
        chain: f32,
        document_id: u32,
    ) -> GroupedFeedbackExample {
        GroupedFeedbackExample {
            query_id: query_id.to_owned(),
            result: result(raw, edit, query, chain, document_id),
            label,
            weight: 1.0,
        }
    }

    #[test]
    fn pairwise_model_learns_to_promote_supported_sources() {
        let examples = vec![
            example("q1", true, 0.55, 0.91, 0.88, 0.94, 1),
            example("q1", false, 0.78, 0.72, 0.24, 0.38, 2),
            example("q2", true, 0.52, 0.89, 0.84, 0.91, 3),
            example("q2", false, 0.80, 0.75, 0.20, 0.41, 4),
            example("q3", true, 0.58, 0.93, 0.90, 0.96, 5),
            example("q3", false, 0.76, 0.70, 0.28, 0.35, 6),
        ];
        let model = RankingModel::fit(
            &examples,
            PairwiseRankingOptions::default(),
            EvaluationOptions::default(),
        )
        .expect("fit");
        assert!(model.predict(&examples[0].result) > model.predict(&examples[1].result));
        let comparison = model
            .compare(&examples, EvaluationOptions::default())
            .expect("compare");
        assert!(comparison.auprc_delta > 0.0, "{comparison:#?}");
    }

    #[test]
    fn hard_negative_mining_keeps_all_positives_and_top_negative() {
        let examples = vec![
            example("q", true, 0.60, 0.90, 0.90, 0.90, 1),
            example("q", false, 0.90, 0.70, 0.20, 0.30, 2),
            example("q", false, 0.20, 0.20, 0.10, 0.10, 3),
        ];
        let mined = mine_hard_negatives(&examples, 1).expect("mine");
        assert_eq!(mined.len(), 2);
        assert!(mined.iter().any(|example| example.label));
        assert!(mined.iter().any(|example| example.result.document_id == 2));
    }
}
