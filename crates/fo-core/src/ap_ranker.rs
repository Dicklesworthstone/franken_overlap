use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    FoError, GroupedEvaluationOptions, GroupedEvaluationReport, GroupedFeedbackExample,
    GroupedLabeledScore, RANKING_FEATURE_COUNT, Result, SearchResult, grouped_evaluation_report,
    ranking_evidence_vector, ranking_feature_names,
};

pub const AP_RANKING_SCHEMA_VERSION: u32 = 1;
pub const AP_RANKING_FEATURE_COUNT: usize = RANKING_FEATURE_COUNT;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ApRankingOptions {
    pub epochs: usize,
    pub learning_rate: f64,
    pub l2: f64,
    pub maximum_negatives_per_query: usize,
    pub minimum_ap_delta: f64,
    pub convergence_tolerance: f64,
}

impl Default for ApRankingOptions {
    fn default() -> Self {
        Self {
            epochs: 500,
            learning_rate: 0.08,
            l2: 0.01,
            maximum_negatives_per_query: 24,
            minimum_ap_delta: 1.0e-8,
            convergence_tolerance: 1.0e-9,
        }
    }
}

impl ApRankingOptions {
    pub fn validate(self) -> Result<Self> {
        if self.epochs == 0 || self.epochs > 1_000_000 {
            return Err(FoError::InvalidConfig(
                "AP-ranking epochs must be between 1 and 1,000,000".to_owned(),
            ));
        }
        if !self.learning_rate.is_finite()
            || self.learning_rate <= 0.0
            || self.learning_rate > 10.0
        {
            return Err(FoError::InvalidConfig(
                "AP-ranking learning_rate must be finite and lie in (0, 10]".to_owned(),
            ));
        }
        if !self.l2.is_finite() || self.l2 < 0.0 {
            return Err(FoError::InvalidConfig(
                "AP-ranking l2 must be finite and non-negative".to_owned(),
            ));
        }
        if self.maximum_negatives_per_query == 0
            || self.maximum_negatives_per_query > 1_000_000
        {
            return Err(FoError::InvalidConfig(
                "maximum_negatives_per_query must be between 1 and 1,000,000".to_owned(),
            ));
        }
        if !self.minimum_ap_delta.is_finite()
            || !(0.0..=1.0).contains(&self.minimum_ap_delta)
        {
            return Err(FoError::InvalidConfig(
                "minimum_ap_delta must be finite and lie in [0, 1]".to_owned(),
            ));
        }
        if !self.convergence_tolerance.is_finite() || self.convergence_tolerance < 0.0 {
            return Err(FoError::InvalidConfig(
                "convergence_tolerance must be finite and non-negative".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApRankingModel {
    pub schema_version: u32,
    pub feature_names: Vec<String>,
    pub means: Vec<f64>,
    pub scales: Vec<f64>,
    pub weights: Vec<f64>,
    pub training_examples: usize,
    pub training_queries: usize,
    pub trainable_queries: usize,
    pub last_epoch_pairs: usize,
    pub completed_epochs: usize,
    pub raw_training_report: GroupedEvaluationReport,
    pub ranked_training_report: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApRankedResult {
    pub rank_score: f64,
    pub result: SearchResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApRankingComparison {
    pub raw: GroupedEvaluationReport,
    pub ranked: GroupedEvaluationReport,
    pub micro_auprc_delta: f64,
    pub macro_auprc_delta: f64,
    pub mean_reciprocal_rank_delta: f64,
    pub recall_at_1_delta: f64,
}

impl ApRankingModel {
    pub fn fit(
        examples: &[GroupedFeedbackExample],
        options: ApRankingOptions,
        evaluation: GroupedEvaluationOptions,
    ) -> Result<Self> {
        let options = options.validate()?;
        let grouped = validate_and_group(examples)?;
        let evaluation = evaluation.validate()?;
        let rows = examples
            .iter()
            .map(|example| ranking_evidence_vector(&example.result))
            .collect::<Vec<_>>();
        let means = feature_means(&rows, examples);
        let scales = feature_scales(&rows, examples, &means);
        let standardized = rows
            .iter()
            .map(|row| standardize(row, &means, &scales))
            .collect::<Vec<_>>();

        let mut weights = [0.0f64; AP_RANKING_FEATURE_COUNT];
        weights[0] = 0.35;
        weights[1] = 0.10;
        weights[2] = 0.10;
        weights[6] = 0.05;
        weights[10] = 0.08;
        let mut prior_loss = f64::INFINITY;
        let mut stable_epochs = 0usize;
        let mut completed_epochs = 0usize;
        let mut last_epoch_pairs = 0usize;
        let mut trainable_queries = 0usize;

        for epoch in 0..options.epochs {
            let scores = standardized
                .iter()
                .map(|row| dot(&weights, row))
                .collect::<Vec<_>>();
            let mut gradient = [0.0f64; AP_RANKING_FEATURE_COUNT];
            let mut epoch_loss = 0.0;
            let mut epoch_queries = 0usize;
            let mut epoch_pairs = 0usize;

            for indices in grouped.values() {
                let pairs = ap_weighted_pairs(
                    indices,
                    examples,
                    &scores,
                    options.maximum_negatives_per_query,
                    options.minimum_ap_delta,
                );
                if pairs.is_empty() {
                    continue;
                }
                let pair_weight_sum = pairs.iter().map(|pair| pair.importance).sum::<f64>();
                if pair_weight_sum <= 0.0 || !pair_weight_sum.is_finite() {
                    continue;
                }
                epoch_queries += 1;
                epoch_pairs = epoch_pairs.saturating_add(pairs.len());
                for pair in pairs {
                    let normalized_importance = pair.importance / pair_weight_sum;
                    let difference = subtract(
                        &standardized[pair.positive],
                        &standardized[pair.negative],
                    );
                    let margin = dot(&weights, &difference);
                    let mistake_probability = sigmoid(-margin);
                    epoch_loss += normalized_importance * softplus(-margin);
                    for (target, value) in gradient.iter_mut().zip(difference) {
                        *target -= normalized_importance * mistake_probability * value;
                    }
                }
            }

            if epoch_queries == 0 {
                return Err(FoError::InvalidConfig(
                    "AP ranking requires at least one query with a positive and a negative"
                        .to_owned(),
                ));
            }
            trainable_queries = epoch_queries;
            last_epoch_pairs = epoch_pairs;
            let query_scale = 1.0 / epoch_queries as f64;
            epoch_loss *= query_scale;
            epoch_loss += 0.5 * options.l2 * weights.iter().map(|weight| weight * weight).sum::<f64>();
            let learning_rate = options.learning_rate / (1.0 + epoch as f64 / 100.0).sqrt();
            for (weight, gradient) in weights.iter_mut().zip(gradient) {
                let regularized = gradient * query_scale + options.l2 * *weight;
                *weight -= learning_rate * regularized;
            }
            completed_epochs = epoch + 1;

            if (prior_loss - epoch_loss).abs() <= options.convergence_tolerance {
                stable_epochs += 1;
                if stable_epochs >= 12 {
                    break;
                }
            } else {
                stable_epochs = 0;
            }
            prior_loss = epoch_loss;
        }

        let raw_scores = grouped_scores(examples, |example| {
            f64::from(example.result.combined_score.clamp(0.0, 1.0))
        });
        let raw_training_report = grouped_evaluation_report(&raw_scores, evaluation.clone())?;
        let ranked_scores = examples
            .iter()
            .zip(&standardized)
            .map(|(example, row)| GroupedLabeledScore {
                query_id: example.query_id.clone(),
                score: sigmoid(dot(&weights, row)),
                label: example.label,
            })
            .collect::<Vec<_>>();
        let ranked_training_report = grouped_evaluation_report(&ranked_scores, evaluation)?;
        let model = Self {
            schema_version: AP_RANKING_SCHEMA_VERSION,
            feature_names: ranking_feature_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            means: means.to_vec(),
            scales: scales.to_vec(),
            weights: weights.to_vec(),
            training_examples: examples.len(),
            training_queries: grouped.len(),
            trainable_queries,
            last_epoch_pairs,
            completed_epochs,
            raw_training_report,
            ranked_training_report,
        };
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != AP_RANKING_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported AP-ranking schema version {}",
                self.schema_version
            )));
        }
        let expected_names = ranking_feature_names();
        if self.feature_names.len() != AP_RANKING_FEATURE_COUNT
            || self
                .feature_names
                .iter()
                .map(String::as_str)
                .ne(expected_names)
        {
            return Err(FoError::InvalidConfig(
                "AP-ranking feature contract does not match this build".to_owned(),
            ));
        }
        for (name, values) in [
            ("means", &self.means),
            ("scales", &self.scales),
            ("weights", &self.weights),
        ] {
            if values.len() != AP_RANKING_FEATURE_COUNT
                || values.iter().any(|value| !value.is_finite())
            {
                return Err(FoError::InvalidConfig(format!(
                    "AP-ranking {name} has invalid dimensions or values"
                )));
            }
        }
        if self.scales.iter().any(|scale| *scale <= 0.0) {
            return Err(FoError::InvalidConfig(
                "AP-ranking feature scales must be positive".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn score(&self, result: &SearchResult) -> f64 {
        let row = ranking_evidence_vector(result);
        let standardized = standardize_slices(&row, &self.means, &self.scales);
        sigmoid(dot_slices(&self.weights, &standardized))
    }

    pub fn rerank(&self, results: &[SearchResult]) -> Result<Vec<ApRankedResult>> {
        self.validate()?;
        let mut ranked = results
            .iter()
            .cloned()
            .map(|result| ApRankedResult {
                rank_score: self.score(&result),
                result,
            })
            .collect::<Vec<_>>();
        ranked.sort_unstable_by(|left, right| {
            right
                .rank_score
                .total_cmp(&left.rank_score)
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
        evaluation: GroupedEvaluationOptions,
    ) -> Result<ApRankingComparison> {
        self.validate()?;
        validate_and_group(examples)?;
        let evaluation = evaluation.validate()?;
        let raw_scores = grouped_scores(examples, |example| {
            f64::from(example.result.combined_score.clamp(0.0, 1.0))
        });
        let ranked_scores = grouped_scores(examples, |example| self.score(&example.result));
        let raw = grouped_evaluation_report(&raw_scores, evaluation.clone())?;
        let ranked = grouped_evaluation_report(&ranked_scores, evaluation)?;
        Ok(ApRankingComparison {
            micro_auprc_delta: ranked.micro.average_precision - raw.micro.average_precision,
            macro_auprc_delta: ranked.macro_average_precision - raw.macro_average_precision,
            mean_reciprocal_rank_delta: ranked.mean_reciprocal_rank
                - raw.mean_reciprocal_rank,
            recall_at_1_delta: metric_at_one(&ranked) - metric_at_one(&raw),
            raw,
            ranked,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct WeightedPair {
    positive: usize,
    negative: usize,
    importance: f64,
}

fn ap_weighted_pairs(
    indices: &[usize],
    examples: &[GroupedFeedbackExample],
    scores: &[f64],
    maximum_negatives: usize,
    minimum_ap_delta: f64,
) -> Vec<WeightedPair> {
    let mut order = indices.to_vec();
    order.sort_unstable_by(|&left, &right| {
        scores[right]
            .total_cmp(&scores[left])
            .then_with(|| {
                examples[right]
                    .result
                    .combined_score
                    .total_cmp(&examples[left].result.combined_score)
            })
            .then_with(|| left.cmp(&right))
    });
    let labels = order
        .iter()
        .map(|&index| examples[index].label)
        .collect::<Vec<_>>();
    let ranks = order
        .iter()
        .enumerate()
        .map(|(rank, &index)| (index, rank))
        .collect::<BTreeMap<_, _>>();
    let positives = order
        .iter()
        .copied()
        .filter(|&index| examples[index].label)
        .collect::<Vec<_>>();
    let negatives = order
        .iter()
        .copied()
        .filter(|&index| !examples[index].label)
        .take(maximum_negatives)
        .collect::<Vec<_>>();
    let mut pairs = Vec::with_capacity(positives.len().saturating_mul(negatives.len()));
    for positive in positives {
        for &negative in &negatives {
            let positive_rank = ranks[&positive];
            let negative_rank = ranks[&negative];
            let delta = average_precision_swap_delta(&labels, positive_rank, negative_rank);
            if delta < minimum_ap_delta {
                continue;
            }
            let feedback_weight =
                (examples[positive].weight * examples[negative].weight).sqrt();
            let importance = delta * feedback_weight;
            if importance.is_finite() && importance > 0.0 {
                pairs.push(WeightedPair {
                    positive,
                    negative,
                    importance,
                });
            }
        }
    }
    pairs
}

fn average_precision_swap_delta(labels: &[bool], positive_rank: usize, negative_rank: usize) -> f64 {
    if positive_rank == negative_rank
        || positive_rank >= labels.len()
        || negative_rank >= labels.len()
        || !labels[positive_rank]
        || labels[negative_rank]
    {
        return 0.0;
    }
    let positives = labels.iter().filter(|&&label| label).count();
    if positives == 0 {
        return 0.0;
    }
    let (low, high, positive_moves_up) = if positive_rank > negative_rank {
        (negative_rank, positive_rank, true)
    } else {
        (positive_rank, negative_rank, false)
    };
    let positives_before_low = labels[..low].iter().filter(|&&label| label).count();
    let mut intermediate_positives = 0usize;
    let mut intermediate_effect = 0.0;
    for (rank, &label) in labels.iter().enumerate().take(high).skip(low + 1) {
        if label {
            intermediate_positives += 1;
            intermediate_effect += 1.0 / (rank + 1) as f64;
        }
    }
    let low_precision = (positives_before_low + 1) as f64 / (low + 1) as f64;
    let high_precision = (positives_before_low + intermediate_positives + 1) as f64
        / (high + 1) as f64;
    let numerator = if positive_moves_up {
        low_precision + intermediate_effect - high_precision
    } else {
        low_precision + intermediate_effect - high_precision
    };
    (numerator / positives as f64).abs()
}

fn grouped_scores(
    examples: &[GroupedFeedbackExample],
    mut score: impl FnMut(&GroupedFeedbackExample) -> f64,
) -> Vec<GroupedLabeledScore> {
    examples
        .iter()
        .map(|example| GroupedLabeledScore {
            query_id: example.query_id.clone(),
            score: score(example).clamp(0.0, 1.0),
            label: example.label,
        })
        .collect()
}

fn validate_and_group(
    examples: &[GroupedFeedbackExample],
) -> Result<BTreeMap<&str, Vec<usize>>> {
    if examples.len() < 2 {
        return Err(FoError::InvalidConfig(
            "AP ranking requires at least two examples".to_owned(),
        ));
    }
    let mut grouped = BTreeMap::<&str, Vec<usize>>::new();
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
        grouped
            .entry(example.query_id.as_str())
            .or_default()
            .push(index);
    }
    if !grouped.values().any(|indices| {
        indices.iter().any(|&index| examples[index].label)
            && indices.iter().any(|&index| !examples[index].label)
    }) {
        return Err(FoError::InvalidConfig(
            "AP ranking requires a query containing both positive and negative examples"
                .to_owned(),
        ));
    }
    Ok(grouped)
}

fn feature_means(
    rows: &[[f64; AP_RANKING_FEATURE_COUNT]],
    examples: &[GroupedFeedbackExample],
) -> [f64; AP_RANKING_FEATURE_COUNT] {
    let total_weight = examples.iter().map(|example| example.weight).sum::<f64>();
    let mut means = [0.0; AP_RANKING_FEATURE_COUNT];
    for (row, example) in rows.iter().zip(examples) {
        for (mean, value) in means.iter_mut().zip(row) {
            *mean += *value * example.weight;
        }
    }
    for mean in &mut means {
        *mean /= total_weight;
    }
    means
}

fn feature_scales(
    rows: &[[f64; AP_RANKING_FEATURE_COUNT]],
    examples: &[GroupedFeedbackExample],
    means: &[f64; AP_RANKING_FEATURE_COUNT],
) -> [f64; AP_RANKING_FEATURE_COUNT] {
    let total_weight = examples.iter().map(|example| example.weight).sum::<f64>();
    let mut scales = [0.0; AP_RANKING_FEATURE_COUNT];
    for ((row, example), means) in rows.iter().zip(examples).zip(std::iter::repeat(means)) {
        for ((scale, value), mean) in scales.iter_mut().zip(row).zip(means) {
            *scale += example.weight * (*value - *mean).powi(2);
        }
    }
    for scale in &mut scales {
        *scale = (*scale / total_weight).sqrt();
        if *scale < 1.0e-9 {
            *scale = 1.0;
        }
    }
    scales
}

fn standardize(
    row: &[f64; AP_RANKING_FEATURE_COUNT],
    means: &[f64; AP_RANKING_FEATURE_COUNT],
    scales: &[f64; AP_RANKING_FEATURE_COUNT],
) -> [f64; AP_RANKING_FEATURE_COUNT] {
    std::array::from_fn(|feature| (row[feature] - means[feature]) / scales[feature])
}

fn standardize_slices(
    row: &[f64; AP_RANKING_FEATURE_COUNT],
    means: &[f64],
    scales: &[f64],
) -> [f64; AP_RANKING_FEATURE_COUNT] {
    std::array::from_fn(|feature| (row[feature] - means[feature]) / scales[feature])
}

fn subtract(
    left: &[f64; AP_RANKING_FEATURE_COUNT],
    right: &[f64; AP_RANKING_FEATURE_COUNT],
) -> [f64; AP_RANKING_FEATURE_COUNT] {
    std::array::from_fn(|feature| left[feature] - right[feature])
}

fn dot(
    left: &[f64; AP_RANKING_FEATURE_COUNT],
    right: &[f64; AP_RANKING_FEATURE_COUNT],
) -> f64 {
    left.iter().zip(right).map(|(left, right)| left * right).sum()
}

fn dot_slices(left: &[f64], right: &[f64; AP_RANKING_FEATURE_COUNT]) -> f64 {
    left.iter().zip(right).map(|(left, right)| left * right).sum()
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
    if value > 40.0 {
        value
    } else if value < -40.0 {
        value.exp()
    } else {
        (1.0 + value.exp()).ln()
    }
}

fn metric_at_one(report: &GroupedEvaluationReport) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == 1)
        .map_or(0.0, |metric| metric.value)
}

#[cfg(test)]
mod tests {
    use super::{
        ApRankingModel, ApRankingOptions, average_precision_swap_delta,
    };
    use crate::{
        GroupedEvaluationOptions, GroupedFeedbackExample, SearchIntent, SearchResult,
    };

    #[test]
    fn swap_delta_matches_brute_force_average_precision() {
        for length in 2..=9 {
            for mask in 1usize..(1usize << length) - 1 {
                let labels = (0..length)
                    .map(|bit| mask & (1usize << bit) != 0)
                    .collect::<Vec<_>>();
                for positive in 0..length {
                    if !labels[positive] {
                        continue;
                    }
                    for negative in 0..length {
                        if labels[negative] {
                            continue;
                        }
                        let expected = brute_swap_delta(&labels, positive, negative);
                        let actual = average_precision_swap_delta(&labels, positive, negative);
                        assert!(
                            (actual - expected).abs() < 1.0e-12,
                            "labels={labels:?} positive={positive} negative={negative} actual={actual} expected={expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ap_weighting_promotes_supported_sources_across_queries() {
        let examples = vec![
            example("q1", true, 0.42, 0.96, 0.90, 0.88),
            example("q1", false, 0.76, 0.45, 0.20, 0.18),
            example("q2", true, 0.38, 0.93, 0.86, 0.91),
            example("q2", false, 0.72, 0.40, 0.18, 0.22),
            example("q3", true, 0.44, 0.95, 0.92, 0.89),
            example("q3", false, 0.79, 0.42, 0.25, 0.17),
        ];
        let model = ApRankingModel::fit(
            &examples,
            ApRankingOptions {
                epochs: 800,
                learning_rate: 0.12,
                ..ApRankingOptions::default()
            },
            GroupedEvaluationOptions {
                bootstrap_samples: 0,
                ..GroupedEvaluationOptions::default()
            },
        )
        .expect("model");
        assert!(
            model.ranked_training_report.macro_average_precision
                > model.raw_training_report.macro_average_precision
        );
        for pair in examples.chunks_exact(2) {
            assert!(model.score(&pair[0].result) > model.score(&pair[1].result));
        }
    }

    fn brute_swap_delta(labels: &[bool], left: usize, right: usize) -> f64 {
        let before = average_precision(labels);
        let mut swapped = labels.to_vec();
        swapped.swap(left, right);
        (average_precision(&swapped) - before).abs()
    }

    fn average_precision(labels: &[bool]) -> f64 {
        let positives = labels.iter().filter(|&&label| label).count();
        if positives == 0 {
            return 0.0;
        }
        let mut seen = 0usize;
        let mut total = 0.0;
        for (rank, &label) in labels.iter().enumerate() {
            if label {
                seen += 1;
                total += seen as f64 / (rank + 1) as f64;
            }
        }
        total / positives as f64
    }

    fn example(
        query_id: &str,
        label: bool,
        raw_score: f32,
        edit_similarity: f32,
        query_coverage: f32,
        chain_consistency: f32,
    ) -> GroupedFeedbackExample {
        GroupedFeedbackExample {
            query_id: query_id.to_owned(),
            label,
            weight: 1.0,
            result: SearchResult {
                document_id: 0,
                path: format!("{query_id}-{label}"),
                intent: SearchIntent::SourceAttribution,
                corpus_start: 0,
                corpus_end: 100,
                query_start: 0,
                query_end: 100,
                edit_distance: 0,
                edit_similarity,
                anchor_coverage: query_coverage,
                query_coverage,
                source_coverage: query_coverage,
                anchor_score: query_coverage,
                vote_support: query_coverage,
                chain_consistency,
                matched_tokens: 100,
                distinct_anchor_count: 8,
                estimated_false_matches: if label { 0.01 } else { 10.0 },
                combined_score: raw_score,
                matched_text: String::new(),
            },
        }
    }
}
