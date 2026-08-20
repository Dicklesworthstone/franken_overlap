use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    EvaluationOptions, FoError, LabeledScore, PrecisionRecallPoint, PrecisionRecallReport, Result,
    precision_recall_report,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupedLabeledScore {
    pub query_id: String,
    pub score: f64,
    pub label: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupedEvaluationOptions {
    pub evaluation: EvaluationOptions,
    pub recall_ks: Vec<usize>,
    pub bootstrap_samples: usize,
    pub confidence_level: f64,
    pub seed: u64,
}

impl Default for GroupedEvaluationOptions {
    fn default() -> Self {
        Self {
            evaluation: EvaluationOptions::default(),
            recall_ks: vec![1, 5, 10],
            bootstrap_samples: 500,
            confidence_level: 0.95,
            seed: 0x8f3c_21d7_4a9b_65e1,
        }
    }
}

impl GroupedEvaluationOptions {
    pub fn validate(mut self) -> Result<Self> {
        self.evaluation = self.evaluation.validate()?;
        if self.recall_ks.is_empty() || self.recall_ks.len() > 64 {
            return Err(FoError::InvalidConfig(
                "recall_ks must contain between 1 and 64 values".to_owned(),
            ));
        }
        if self.recall_ks.contains(&0) {
            return Err(FoError::InvalidConfig(
                "recall_ks values must be positive".to_owned(),
            ));
        }
        self.recall_ks.sort_unstable();
        self.recall_ks.dedup();
        if self.bootstrap_samples > 100_000 {
            return Err(FoError::InvalidConfig(
                "bootstrap_samples must not exceed 100,000".to_owned(),
            ));
        }
        if !self.confidence_level.is_finite()
            || self.confidence_level <= 0.0
            || self.confidence_level >= 1.0
        {
            return Err(FoError::InvalidConfig(
                "confidence_level must be finite and lie in (0, 1)".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceInterval {
    pub lower: f64,
    pub upper: f64,
    pub confidence_level: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AtKMetric {
    pub k: usize,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupedEvaluationReport {
    pub queries: usize,
    pub queries_with_positives: usize,
    pub queries_without_positives: usize,
    pub examples: usize,
    pub positives: usize,
    pub negatives: usize,
    pub mean_candidates_per_query: f64,
    pub micro: PrecisionRecallReport,
    pub macro_average_precision: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_k: Vec<AtKMetric>,
    pub ndcg_at_k: Vec<AtKMetric>,
    pub micro_average_precision_interval: Option<ConfidenceInterval>,
    pub macro_average_precision_interval: Option<ConfidenceInterval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThresholdConstraints {
    pub minimum_precision: f64,
    pub minimum_recall: f64,
    pub maximum_false_positives_per_query: Option<f64>,
}

impl Default for ThresholdConstraints {
    fn default() -> Self {
        Self {
            minimum_precision: 0.0,
            minimum_recall: 0.0,
            maximum_false_positives_per_query: None,
        }
    }
}

impl ThresholdConstraints {
    pub fn validate(self) -> Result<Self> {
        for (name, value) in [
            ("minimum_precision", self.minimum_precision),
            ("minimum_recall", self.minimum_recall),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "{name} must be finite and lie in [0, 1]"
                )));
            }
        }
        if self
            .maximum_false_positives_per_query
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err(FoError::InvalidConfig(
                "maximum_false_positives_per_query must be finite and non-negative".to_owned(),
            ));
        }
        Ok(self)
    }
}

pub fn grouped_evaluation_report(
    examples: &[GroupedLabeledScore],
    options: GroupedEvaluationOptions,
) -> Result<GroupedEvaluationReport> {
    let options = options.validate()?;
    let grouped = validate_and_group(examples)?;
    let flat = examples
        .iter()
        .map(|example| LabeledScore {
            score: example.score,
            label: example.label,
        })
        .collect::<Vec<_>>();
    let micro = precision_recall_report(&flat, options.evaluation)?;

    let mut macro_sum = 0.0;
    let mut mean_reciprocal_rank = 0.0;
    let mut recall_sums = vec![0.0f64; options.recall_ks.len()];
    let mut ndcg_sums = vec![0.0f64; options.recall_ks.len()];
    let mut queries_with_positives = 0usize;
    for group in grouped.values() {
        let positives = group.iter().filter(|example| example.label).count();
        if positives == 0 {
            continue;
        }
        queries_with_positives += 1;
        let group_scores = group
            .iter()
            .map(|example| LabeledScore {
                score: example.score,
                label: example.label,
            })
            .collect::<Vec<_>>();
        macro_sum += precision_recall_report(&group_scores, options.evaluation)?.average_precision;
        let ranking = ranking_metrics(group, &options.recall_ks);
        mean_reciprocal_rank += ranking.reciprocal_rank;
        for (target, value) in recall_sums.iter_mut().zip(ranking.recall_at_k) {
            *target += value;
        }
        for (target, value) in ndcg_sums.iter_mut().zip(ranking.ndcg_at_k) {
            *target += value;
        }
    }
    if queries_with_positives == 0 {
        return Err(FoError::InvalidConfig(
            "grouped evaluation requires at least one query with a positive result".to_owned(),
        ));
    }
    let denominator = queries_with_positives as f64;
    let macro_average_precision = macro_sum / denominator;
    mean_reciprocal_rank /= denominator;
    let recall_at_k = options
        .recall_ks
        .iter()
        .copied()
        .zip(recall_sums.into_iter().map(|value| value / denominator))
        .map(|(k, value)| AtKMetric { k, value })
        .collect::<Vec<_>>();
    let ndcg_at_k = options
        .recall_ks
        .iter()
        .copied()
        .zip(ndcg_sums.into_iter().map(|value| value / denominator))
        .map(|(k, value)| AtKMetric { k, value })
        .collect::<Vec<_>>();

    let (micro_interval, macro_interval) = bootstrap_intervals(&grouped, &options)?;
    Ok(GroupedEvaluationReport {
        queries: grouped.len(),
        queries_with_positives,
        queries_without_positives: grouped.len() - queries_with_positives,
        examples: examples.len(),
        positives: micro.positives,
        negatives: micro.negatives,
        mean_candidates_per_query: examples.len() as f64 / grouped.len() as f64,
        micro,
        macro_average_precision,
        mean_reciprocal_rank,
        recall_at_k,
        ndcg_at_k,
        micro_average_precision_interval: micro_interval,
        macro_average_precision_interval: macro_interval,
    })
}

pub fn select_operating_point(
    examples: &[GroupedLabeledScore],
    evaluation: EvaluationOptions,
    constraints: ThresholdConstraints,
) -> Result<PrecisionRecallPoint> {
    let constraints = constraints.validate()?;
    let grouped = validate_and_group(examples)?;
    let flat = examples
        .iter()
        .map(|example| LabeledScore {
            score: example.score,
            label: example.label,
        })
        .collect::<Vec<_>>();
    let complete_evaluation = EvaluationOptions {
        max_curve_points: flat.len().max(2),
        calibration_bins: evaluation.calibration_bins,
    };
    let report = precision_recall_report(&flat, complete_evaluation)?;
    report
        .curve
        .into_iter()
        .filter(|point| {
            point.precision >= constraints.minimum_precision
                && point.recall >= constraints.minimum_recall
                && constraints
                    .maximum_false_positives_per_query
                    .is_none_or(|maximum| {
                        point.false_positives as f64 / grouped.len() as f64 <= maximum
                    })
        })
        .max_by(|left, right| {
            left.recall
                .total_cmp(&right.recall)
                .then_with(|| left.precision.total_cmp(&right.precision))
                .then_with(|| left.f1.total_cmp(&right.f1))
                .then_with(|| left.threshold.total_cmp(&right.threshold))
        })
        .ok_or_else(|| {
            FoError::InvalidConfig(
                "no score threshold satisfies the requested operating constraints".to_owned(),
            )
        })
}

fn validate_and_group(
    examples: &[GroupedLabeledScore],
) -> Result<BTreeMap<&str, Vec<&GroupedLabeledScore>>> {
    if examples.is_empty() {
        return Err(FoError::InvalidConfig(
            "at least one grouped labeled score is required".to_owned(),
        ));
    }
    let mut grouped = BTreeMap::<&str, Vec<&GroupedLabeledScore>>::new();
    for (index, example) in examples.iter().enumerate() {
        if example.query_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(format!(
                "example {index} has an empty query_id"
            )));
        }
        if !example.score.is_finite() || !(0.0..=1.0).contains(&example.score) {
            return Err(FoError::InvalidConfig(format!(
                "example {index} has score {}; scores must lie in [0, 1]",
                example.score
            )));
        }
        grouped
            .entry(example.query_id.as_str())
            .or_default()
            .push(example);
    }
    Ok(grouped)
}

struct QueryRankingMetrics {
    reciprocal_rank: f64,
    recall_at_k: Vec<f64>,
    ndcg_at_k: Vec<f64>,
}

fn ranking_metrics(examples: &[&GroupedLabeledScore], ks: &[usize]) -> QueryRankingMetrics {
    let mut ranked = examples.iter().copied().enumerate().collect::<Vec<_>>();
    ranked.sort_unstable_by(|(left_index, left), (right_index, right)| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left_index.cmp(right_index))
    });
    let positives = ranked.iter().filter(|(_, example)| example.label).count();
    let mut recall_at_k = vec![0.0; ks.len()];
    let mut ndcg_at_k = vec![0.0; ks.len()];
    let mut reciprocal_rank = 0.0;
    let mut cursor = 0usize;
    let mut prior_positive = false;
    while cursor < ranked.len() {
        let score = ranked[cursor].1.score;
        let mut end = cursor + 1;
        while end < ranked.len() && ranked[end].1.score.total_cmp(&score).is_eq() {
            end += 1;
        }
        let group_size = end - cursor;
        let group_positives = ranked[cursor..end]
            .iter()
            .filter(|(_, example)| example.label)
            .count();
        if !prior_positive && group_positives > 0 {
            reciprocal_rank = expected_reciprocal_rank(cursor, group_size, group_positives);
            prior_positive = true;
        }
        let positive_fraction = group_positives as f64 / group_size as f64;
        for (metric_index, &k) in ks.iter().enumerate() {
            let slots = k.min(end).saturating_sub(cursor.min(k));
            if slots > 0 {
                recall_at_k[metric_index] += slots as f64 * positive_fraction;
                for position in cursor..cursor + slots {
                    ndcg_at_k[metric_index] += positive_fraction * discount(position);
                }
            }
        }
        cursor = end;
    }
    for (metric_index, &k) in ks.iter().enumerate() {
        recall_at_k[metric_index] /= positives.max(1) as f64;
        let ideal = (0..positives.min(k)).map(discount).sum::<f64>();
        ndcg_at_k[metric_index] = if ideal > 0.0 {
            ndcg_at_k[metric_index] / ideal
        } else {
            0.0
        };
    }
    QueryRankingMetrics {
        reciprocal_rank,
        recall_at_k,
        ndcg_at_k,
    }
}

fn expected_reciprocal_rank(prior_positions: usize, group_size: usize, positives: usize) -> f64 {
    let mut probability_no_positive = 1.0;
    let mut expectation = 0.0;
    let latest_first = group_size.saturating_sub(positives);
    for offset in 0..=latest_first {
        let remaining = group_size - offset;
        let probability_first = probability_no_positive * positives as f64 / remaining as f64;
        expectation += probability_first / (prior_positions + offset + 1) as f64;
        if offset < latest_first {
            probability_no_positive *= (group_size - positives - offset) as f64 / remaining as f64;
        }
    }
    expectation
}

fn discount(zero_based_position: usize) -> f64 {
    1.0 / ((zero_based_position + 2) as f64).log2()
}

fn bootstrap_intervals(
    grouped: &BTreeMap<&str, Vec<&GroupedLabeledScore>>,
    options: &GroupedEvaluationOptions,
) -> Result<(Option<ConfidenceInterval>, Option<ConfidenceInterval>)> {
    if options.bootstrap_samples == 0 {
        return Ok((None, None));
    }
    let groups = grouped.values().collect::<Vec<_>>();
    let mut rng = DeterministicRng::new(options.seed);
    let mut micro_values = Vec::with_capacity(options.bootstrap_samples);
    let mut macro_values = Vec::with_capacity(options.bootstrap_samples);
    for _ in 0..options.bootstrap_samples {
        let mut flat = Vec::new();
        let mut macro_sum = 0.0;
        let mut positive_groups = 0usize;
        for _ in 0..groups.len() {
            let group = groups[rng.range(groups.len())];
            let scores = group
                .iter()
                .map(|example| LabeledScore {
                    score: example.score,
                    label: example.label,
                })
                .collect::<Vec<_>>();
            if scores.iter().any(|example| example.label) {
                macro_sum +=
                    precision_recall_report(&scores, options.evaluation)?.average_precision;
                positive_groups += 1;
            }
            flat.extend(scores);
        }
        if flat.iter().any(|example| example.label) && positive_groups > 0 {
            micro_values
                .push(precision_recall_report(&flat, options.evaluation)?.average_precision);
            macro_values.push(macro_sum / positive_groups as f64);
        }
    }
    Ok((
        confidence_interval(micro_values, options.confidence_level),
        confidence_interval(macro_values, options.confidence_level),
    ))
}

fn confidence_interval(mut values: Vec<f64>, confidence_level: f64) -> Option<ConfidenceInterval> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let tail = (1.0 - confidence_level) / 2.0;
    let lower_index = ((values.len() - 1) as f64 * tail).floor() as usize;
    let upper_index = ((values.len() - 1) as f64 * (1.0 - tail)).ceil() as usize;
    Some(ConfidenceInterval {
        lower: values[lower_index.min(values.len() - 1)],
        upper: values[upper_index.min(values.len() - 1)],
        confidence_level,
        samples: values.len(),
    })
}

struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next_u64() % upper as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GroupedEvaluationOptions, GroupedLabeledScore, ThresholdConstraints,
        grouped_evaluation_report, select_operating_point,
    };
    use crate::EvaluationOptions;

    fn example(query: &str, score: f64, label: bool) -> GroupedLabeledScore {
        GroupedLabeledScore {
            query_id: query.to_owned(),
            score,
            label,
        }
    }

    #[test]
    fn reports_query_ranking_and_macro_quality() {
        let examples = vec![
            example("q1", 0.9, true),
            example("q1", 0.4, false),
            example("q2", 0.8, true),
            example("q2", 0.7, false),
            example("q2", 0.1, false),
        ];
        let report = grouped_evaluation_report(
            &examples,
            GroupedEvaluationOptions {
                bootstrap_samples: 32,
                seed: 7,
                ..GroupedEvaluationOptions::default()
            },
        )
        .expect("report");
        assert!((report.micro.average_precision - 1.0).abs() < 1e-12);
        assert!((report.macro_average_precision - 1.0).abs() < 1e-12);
        assert!((report.mean_reciprocal_rank - 1.0).abs() < 1e-12);
        assert_eq!(report.recall_at_k[0].value, 1.0);
        assert!(report.micro_average_precision_interval.is_some());
    }

    #[test]
    fn tie_aware_top_one_uses_expected_recall() {
        let examples = vec![example("q", 0.8, true), example("q", 0.8, false)];
        let report = grouped_evaluation_report(
            &examples,
            GroupedEvaluationOptions {
                bootstrap_samples: 0,
                ..GroupedEvaluationOptions::default()
            },
        )
        .expect("report");
        assert!((report.recall_at_k[0].value - 0.5).abs() < 1e-12);
        assert!((report.mean_reciprocal_rank - 0.75).abs() < 1e-12);
    }

    #[test]
    fn operating_point_enforces_false_positive_budget() {
        let examples = vec![
            example("q1", 0.9, true),
            example("q1", 0.8, false),
            example("q2", 0.7, true),
            example("q2", 0.2, false),
        ];
        let point = select_operating_point(
            &examples,
            EvaluationOptions::default(),
            ThresholdConstraints {
                minimum_precision: 0.5,
                minimum_recall: 0.5,
                maximum_false_positives_per_query: Some(0.0),
            },
        )
        .expect("point");
        assert_eq!(point.false_positives, 0);
        assert_eq!(point.threshold, 0.9);
    }
}
