use serde::{Deserialize, Serialize};

use crate::{FoError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LabeledScore {
    pub score: f64,
    pub label: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationOptions {
    pub max_curve_points: usize,
    pub calibration_bins: usize,
}

impl Default for EvaluationOptions {
    fn default() -> Self {
        Self {
            max_curve_points: 256,
            calibration_bins: 15,
        }
    }
}

impl EvaluationOptions {
    pub fn validate(self) -> Result<Self> {
        if self.max_curve_points < 2 {
            return Err(FoError::InvalidConfig(
                "max_curve_points must be at least 2".to_owned(),
            ));
        }
        if !(2..=1024).contains(&self.calibration_bins) {
            return Err(FoError::InvalidConfig(
                "calibration_bins must be between 2 and 1024".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecisionRecallPoint {
    pub threshold: f64,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecisionRecallReport {
    pub examples: usize,
    pub positives: usize,
    pub negatives: usize,
    pub prevalence: f64,
    pub average_precision: f64,
    pub best_f1: f64,
    pub best_threshold: f64,
    pub brier_score: f64,
    pub log_loss: f64,
    pub expected_calibration_error: f64,
    pub maximum_calibration_error: f64,
    pub curve: Vec<PrecisionRecallPoint>,
}

pub fn precision_recall_report(
    examples: &[LabeledScore],
    options: EvaluationOptions,
) -> Result<PrecisionRecallReport> {
    let options = options.validate()?;
    if examples.is_empty() {
        return Err(FoError::InvalidConfig(
            "at least one labeled score is required".to_owned(),
        ));
    }
    for (index, example) in examples.iter().enumerate() {
        if !example.score.is_finite() || !(0.0..=1.0).contains(&example.score) {
            return Err(FoError::InvalidConfig(format!(
                "example {index} has score {}; scores must be finite probabilities in [0, 1]",
                example.score
            )));
        }
    }

    let positives = examples.iter().filter(|example| example.label).count();
    if positives == 0 {
        return Err(FoError::InvalidConfig(
            "AUPRC is undefined without at least one positive example".to_owned(),
        ));
    }
    let negatives = examples.len() - positives;
    let mut ranked = examples.to_vec();
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.label.cmp(&left.label))
    });

    let mut true_positives = 0usize;
    let mut false_positives = 0usize;
    let mut prior_recall = 0.0;
    let mut average_precision = 0.0;
    let mut best_f1 = -1.0;
    let mut best_threshold = 1.0;
    let mut curve = Vec::new();
    let mut cursor = 0usize;

    while cursor < ranked.len() {
        let threshold = ranked[cursor].score;
        let mut end = cursor + 1;
        while end < ranked.len() && ranked[end].score.total_cmp(&threshold).is_eq() {
            end += 1;
        }
        for example in &ranked[cursor..end] {
            if example.label {
                true_positives += 1;
            } else {
                false_positives += 1;
            }
        }
        let false_negatives = positives - true_positives;
        let precision = true_positives as f64 / (true_positives + false_positives).max(1) as f64;
        let recall = true_positives as f64 / positives as f64;
        let f1 = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };
        average_precision += (recall - prior_recall) * precision;
        prior_recall = recall;
        let f1_order = f1.total_cmp(&best_f1);
        if f1_order.is_gt() || (f1_order.is_eq() && threshold > best_threshold) {
            best_f1 = f1;
            best_threshold = threshold;
        }
        curve.push(PrecisionRecallPoint {
            threshold,
            precision,
            recall,
            f1,
            true_positives,
            false_positives,
            false_negatives,
        });
        cursor = end;
    }

    let brier_score = examples
        .iter()
        .map(|example| {
            let target = if example.label { 1.0 } else { 0.0 };
            (example.score - target).powi(2)
        })
        .sum::<f64>()
        / examples.len() as f64;
    let epsilon = 1.0e-15;
    let log_loss = -examples
        .iter()
        .map(|example| {
            let probability = example.score.clamp(epsilon, 1.0 - epsilon);
            if example.label {
                probability.ln()
            } else {
                (1.0 - probability).ln()
            }
        })
        .sum::<f64>()
        / examples.len() as f64;
    let (expected_calibration_error, maximum_calibration_error) =
        calibration_error(examples, options.calibration_bins);

    Ok(PrecisionRecallReport {
        examples: examples.len(),
        positives,
        negatives,
        prevalence: positives as f64 / examples.len() as f64,
        average_precision,
        best_f1: best_f1.max(0.0),
        best_threshold,
        brier_score,
        log_loss,
        expected_calibration_error,
        maximum_calibration_error,
        curve: downsample_curve(curve, options.max_curve_points),
    })
}

fn calibration_error(examples: &[LabeledScore], bins: usize) -> (f64, f64) {
    let mut counts = vec![0usize; bins];
    let mut probability_sums = vec![0.0f64; bins];
    let mut positive_sums = vec![0usize; bins];
    for example in examples {
        let bin = ((example.score * bins as f64).floor() as usize).min(bins - 1);
        counts[bin] += 1;
        probability_sums[bin] += example.score;
        positive_sums[bin] += if example.label { 1 } else { 0 };
    }

    let mut expected = 0.0;
    let mut maximum: f64 = 0.0;
    for bin in 0..bins {
        let count = counts[bin];
        if count == 0 {
            continue;
        }
        let mean_probability = probability_sums[bin] / count as f64;
        let observed_rate = positive_sums[bin] as f64 / count as f64;
        let gap = (mean_probability - observed_rate).abs();
        expected += count as f64 / examples.len() as f64 * gap;
        maximum = maximum.max(gap);
    }
    (expected, maximum)
}

fn downsample_curve(curve: Vec<PrecisionRecallPoint>, maximum: usize) -> Vec<PrecisionRecallPoint> {
    if curve.len() <= maximum {
        return curve;
    }
    let last = curve.len() - 1;
    let mut selected = Vec::with_capacity(maximum);
    let mut prior = None;
    for index in 0..maximum {
        let source = index.saturating_mul(last) / (maximum - 1);
        if prior == Some(source) {
            continue;
        }
        selected.push(curve[source].clone());
        prior = Some(source);
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::{EvaluationOptions, LabeledScore, precision_recall_report};

    fn example(score: f64, label: bool) -> LabeledScore {
        LabeledScore { score, label }
    }

    #[test]
    fn perfect_ranking_has_unit_average_precision() {
        let report = precision_recall_report(
            &[
                example(0.95, true),
                example(0.80, true),
                example(0.30, false),
                example(0.10, false),
            ],
            EvaluationOptions::default(),
        )
        .expect("report");
        assert!((report.average_precision - 1.0).abs() < 1.0e-12);
        assert!((report.best_f1 - 1.0).abs() < 1.0e-12);
        assert!((report.best_threshold - 0.80).abs() < 1.0e-12);
    }

    #[test]
    fn tie_groups_are_evaluated_atomically() {
        let report = precision_recall_report(
            &[
                example(0.8, true),
                example(0.8, false),
                example(0.2, true),
                example(0.1, false),
            ],
            EvaluationOptions::default(),
        )
        .expect("report");
        assert_eq!(report.curve[0].true_positives, 1);
        assert_eq!(report.curve[0].false_positives, 1);
        assert!((report.curve[0].threshold - 0.8).abs() < 1.0e-12);
    }

    #[test]
    fn curve_is_bounded_without_losing_endpoints() {
        let examples = (0..100)
            .map(|index| example(1.0 - index as f64 / 100.0, index % 7 == 0))
            .collect::<Vec<_>>();
        let report = precision_recall_report(
            &examples,
            EvaluationOptions {
                max_curve_points: 8,
                calibration_bins: 10,
            },
        )
        .expect("report");
        assert!(report.curve.len() <= 8);
        assert!((report.curve.first().expect("first").threshold - 1.0).abs() < 1.0e-12);
        assert!((report.curve.last().expect("last").threshold - 0.01).abs() < 1.0e-12);
    }

    #[test]
    fn rejects_non_probability_scores() {
        let error = precision_recall_report(&[example(1.2, true)], EvaluationOptions::default())
            .expect_err("must reject");
        assert!(error.to_string().contains("[0, 1]"));
    }
}
