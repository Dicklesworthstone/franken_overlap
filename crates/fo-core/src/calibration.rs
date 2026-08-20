use serde::{Deserialize, Serialize};

use crate::{
    EvaluationOptions, FoError, LabeledScore, PrecisionRecallReport, Result, SearchResult,
    precision_recall_report,
};

pub const CALIBRATION_FEATURE_COUNT: usize = 10;
pub const CALIBRATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackExample {
    pub result: SearchResult,
    pub label: bool,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibrationOptions {
    pub epochs: usize,
    pub learning_rate: f64,
    pub l2: f64,
    pub convergence_tolerance: f64,
}

impl Default for CalibrationOptions {
    fn default() -> Self {
        Self {
            epochs: 600,
            learning_rate: 0.12,
            l2: 0.01,
            convergence_tolerance: 1.0e-9,
        }
    }
}

impl CalibrationOptions {
    pub fn validate(self) -> Result<Self> {
        if self.epochs == 0 || self.epochs > 1_000_000 {
            return Err(FoError::InvalidConfig(
                "calibration epochs must be between 1 and 1,000,000".to_owned(),
            ));
        }
        if !self.learning_rate.is_finite() || !(0.0..=10.0).contains(&self.learning_rate) {
            return Err(FoError::InvalidConfig(
                "calibration learning_rate must be finite and lie in (0, 10]".to_owned(),
            ));
        }
        if self.learning_rate == 0.0 {
            return Err(FoError::InvalidConfig(
                "calibration learning_rate must be positive".to_owned(),
            ));
        }
        if !self.l2.is_finite() || self.l2 < 0.0 {
            return Err(FoError::InvalidConfig(
                "calibration l2 must be finite and non-negative".to_owned(),
            ));
        }
        if !self.convergence_tolerance.is_finite() || self.convergence_tolerance < 0.0 {
            return Err(FoError::InvalidConfig(
                "calibration convergence_tolerance must be finite and non-negative".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationModel {
    pub schema_version: u32,
    pub feature_names: Vec<String>,
    pub means: Vec<f64>,
    pub scales: Vec<f64>,
    pub weights: Vec<f64>,
    pub bias: f64,
    pub training_examples: usize,
    pub training_positives: usize,
    pub training_negatives: usize,
    pub completed_epochs: usize,
    pub training_report: PrecisionRecallReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibratedResult {
    pub probability: f64,
    pub result: SearchResult,
}

impl CalibrationModel {
    pub fn fit(
        examples: &[FeedbackExample],
        options: CalibrationOptions,
        evaluation: EvaluationOptions,
    ) -> Result<Self> {
        let options = options.validate()?;
        validate_feedback(examples)?;
        let training_positives = examples.iter().filter(|example| example.label).count();
        let training_negatives = examples.len() - training_positives;
        if training_positives == 0 || training_negatives == 0 {
            return Err(FoError::InvalidConfig(
                "calibration requires at least one positive and one negative example".to_owned(),
            ));
        }

        let rows = examples
            .iter()
            .map(|example| evidence_vector(&example.result))
            .collect::<Vec<_>>();
        let means = feature_means(&rows);
        let scales = feature_scales(&rows, &means);
        let standardized = rows
            .iter()
            .map(|row| standardize(row, &means, &scales))
            .collect::<Vec<_>>();

        let prevalence = training_positives as f64 / examples.len() as f64;
        let mut bias = logit(prevalence.clamp(1.0e-6, 1.0 - 1.0e-6));
        let mut weights = [0.0f64; CALIBRATION_FEATURE_COUNT];
        let total_weight = examples.iter().map(|example| example.weight).sum::<f64>();
        if !total_weight.is_finite() || total_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "feedback weights have an invalid total".to_owned(),
            ));
        }
        let mut prior_loss = f64::INFINITY;
        let mut stable_epochs = 0usize;
        let mut completed_epochs = 0usize;

        for epoch in 0..options.epochs {
            let mut bias_gradient = 0.0;
            let mut weight_gradient = [0.0f64; CALIBRATION_FEATURE_COUNT];
            let mut loss = 0.0;
            for (example, row) in examples.iter().zip(&standardized) {
                let probability = sigmoid(bias + dot(&weights, row));
                let target = if example.label { 1.0 } else { 0.0 };
                let error = (probability - target) * example.weight;
                bias_gradient += error;
                for (gradient, value) in weight_gradient.iter_mut().zip(row) {
                    *gradient += error * *value;
                }
                loss += example.weight * binary_log_loss(probability, example.label);
            }
            loss /= total_weight;
            loss += 0.5 * options.l2 * weights.iter().map(|weight| weight * weight).sum::<f64>();

            let learning_rate = options.learning_rate / (1.0 + epoch as f64 / 100.0).sqrt();
            bias -= learning_rate * bias_gradient / total_weight;
            for (weight, gradient) in weights.iter_mut().zip(weight_gradient) {
                let regularized = gradient / total_weight + options.l2 * *weight;
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

        let mut model = Self {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            feature_names: feature_names().into_iter().map(str::to_owned).collect(),
            means: means.to_vec(),
            scales: scales.to_vec(),
            weights: weights.to_vec(),
            bias,
            training_examples: examples.len(),
            training_positives,
            training_negatives,
            completed_epochs,
            training_report: precision_recall_report(
                &[
                    LabeledScore {
                        score: prevalence,
                        label: true,
                    },
                    LabeledScore {
                        score: prevalence,
                        label: false,
                    },
                ],
                evaluation,
            )?,
        };
        let training_scores = examples
            .iter()
            .map(|example| LabeledScore {
                score: model.predict(&example.result),
                label: example.label,
            })
            .collect::<Vec<_>>();
        model.training_report = precision_recall_report(&training_scores, evaluation)?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CALIBRATION_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported calibration schema version {}",
                self.schema_version
            )));
        }
        if self.feature_names.len() != CALIBRATION_FEATURE_COUNT {
            return Err(FoError::InvalidConfig(format!(
                "calibration feature_names has {} values; expected {CALIBRATION_FEATURE_COUNT}",
                self.feature_names.len()
            )));
        }
        for (name, values) in [
            ("means", &self.means),
            ("scales", &self.scales),
            ("weights", &self.weights),
        ] {
            if values.len() != CALIBRATION_FEATURE_COUNT {
                return Err(FoError::InvalidConfig(format!(
                    "calibration {name} has {} values; expected {CALIBRATION_FEATURE_COUNT}",
                    values.len()
                )));
            }
            if values.iter().any(|value| !value.is_finite()) {
                return Err(FoError::InvalidConfig(format!(
                    "calibration {name} contains a non-finite value"
                )));
            }
        }
        if self.scales.iter().any(|scale| *scale <= 0.0) || !self.bias.is_finite() {
            return Err(FoError::InvalidConfig(
                "calibration model contains invalid scale or bias values".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn predict(&self, result: &SearchResult) -> f64 {
        let row = evidence_vector(result);
        let mut linear = self.bias;
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

    pub fn rerank(&self, results: &[SearchResult]) -> Result<Vec<CalibratedResult>> {
        self.validate()?;
        let mut calibrated = results
            .iter()
            .cloned()
            .map(|result| CalibratedResult {
                probability: self.predict(&result),
                result,
            })
            .collect::<Vec<_>>();
        calibrated.sort_unstable_by(|left, right| {
            right
                .probability
                .total_cmp(&left.probability)
                .then_with(|| {
                    right
                        .result
                        .combined_score
                        .total_cmp(&left.result.combined_score)
                })
                .then_with(|| left.result.document_id.cmp(&right.result.document_id))
                .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
        });
        Ok(calibrated)
    }
}

#[must_use]
pub fn evidence_vector(result: &SearchResult) -> [f64; CALIBRATION_FEATURE_COUNT] {
    let length_factor = 1.0 - (-(result.matched_tokens as f64) / 32.0).exp();
    let anchor_count_factor = 1.0 - (-(result.distinct_anchor_count as f64) / 8.0).exp();
    let false_match_confidence = 1.0 / (1.0 + result.estimated_false_matches.max(0.0));
    [
        f64::from(result.combined_score.clamp(0.0, 1.0)),
        f64::from(result.edit_similarity.clamp(0.0, 1.0)),
        f64::from(result.query_coverage.clamp(0.0, 1.0)),
        f64::from(result.source_coverage.clamp(0.0, 1.0)),
        f64::from(result.anchor_coverage.clamp(0.0, 1.0)),
        f64::from(result.vote_support.clamp(0.0, 1.0)),
        f64::from(result.chain_consistency.clamp(0.0, 1.0)),
        length_factor,
        anchor_count_factor,
        false_match_confidence,
    ]
}

#[must_use]
pub fn feature_names() -> [&'static str; CALIBRATION_FEATURE_COUNT] {
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
    ]
}

fn validate_feedback(examples: &[FeedbackExample]) -> Result<()> {
    if examples.len() < 2 {
        return Err(FoError::InvalidConfig(
            "at least two feedback examples are required".to_owned(),
        ));
    }
    for (index, example) in examples.iter().enumerate() {
        if !example.weight.is_finite() || example.weight <= 0.0 {
            return Err(FoError::InvalidConfig(format!(
                "feedback example {index} has invalid weight {}",
                example.weight
            )));
        }
        if evidence_vector(&example.result)
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(FoError::InvalidConfig(format!(
                "feedback example {index} contains non-finite evidence"
            )));
        }
    }
    Ok(())
}

fn feature_means(rows: &[[f64; CALIBRATION_FEATURE_COUNT]]) -> [f64; CALIBRATION_FEATURE_COUNT] {
    let mut means = [0.0; CALIBRATION_FEATURE_COUNT];
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
    rows: &[[f64; CALIBRATION_FEATURE_COUNT]],
    means: &[f64; CALIBRATION_FEATURE_COUNT],
) -> [f64; CALIBRATION_FEATURE_COUNT] {
    let mut scales = [0.0; CALIBRATION_FEATURE_COUNT];
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
    row: &[f64; CALIBRATION_FEATURE_COUNT],
    means: &[f64; CALIBRATION_FEATURE_COUNT],
    scales: &[f64; CALIBRATION_FEATURE_COUNT],
) -> [f64; CALIBRATION_FEATURE_COUNT] {
    std::array::from_fn(|feature| (row[feature] - means[feature]) / scales[feature])
}

fn dot(left: &[f64; CALIBRATION_FEATURE_COUNT], right: &[f64; CALIBRATION_FEATURE_COUNT]) -> f64 {
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

fn logit(probability: f64) -> f64 {
    (probability / (1.0 - probability)).ln()
}

fn binary_log_loss(probability: f64, label: bool) -> f64 {
    let probability = probability.clamp(1.0e-15, 1.0 - 1.0e-15);
    if label {
        -probability.ln()
    } else {
        -(1.0 - probability).ln()
    }
}

fn default_weight() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::{CalibrationModel, CalibrationOptions, FeedbackExample};
    use crate::{EvaluationOptions, SearchIntent, SearchResult};

    fn result(
        raw_score: f32,
        edit_similarity: f32,
        query_coverage: f32,
        anchor_coverage: f32,
    ) -> SearchResult {
        SearchResult {
            document_id: 0,
            path: "fixture".to_owned(),
            intent: SearchIntent::SourceAttribution,
            corpus_start: 0,
            corpus_end: 100,
            query_start: 0,
            query_end: 100,
            edit_distance: 0,
            edit_similarity,
            anchor_coverage,
            query_coverage,
            source_coverage: query_coverage,
            anchor_score: anchor_coverage,
            vote_support: anchor_coverage,
            chain_consistency: anchor_coverage,
            matched_tokens: (query_coverage * 100.0) as usize,
            distinct_anchor_count: (anchor_coverage * 12.0) as usize,
            estimated_false_matches: f64::from(1.0 - anchor_coverage),
            combined_score: raw_score,
            matched_text: "fixture".to_owned(),
        }
    }

    #[test]
    fn calibration_learns_separable_evidence() {
        let examples = vec![
            FeedbackExample {
                result: result(0.55, 0.95, 0.95, 0.90),
                label: true,
                weight: 1.0,
            },
            FeedbackExample {
                result: result(0.52, 0.90, 0.85, 0.88),
                label: true,
                weight: 1.0,
            },
            FeedbackExample {
                result: result(0.58, 0.45, 0.10, 0.05),
                label: false,
                weight: 1.0,
            },
            FeedbackExample {
                result: result(0.50, 0.35, 0.15, 0.10),
                label: false,
                weight: 1.0,
            },
        ];
        let model = CalibrationModel::fit(
            &examples,
            CalibrationOptions::default(),
            EvaluationOptions::default(),
        )
        .expect("fit");
        assert!(model.predict(&examples[0].result) > model.predict(&examples[2].result));
        assert!(model.training_report.average_precision > 0.99);
        assert!(model.completed_epochs > 0);
    }

    #[test]
    fn rerank_orders_by_calibrated_probability() {
        let examples = vec![
            FeedbackExample {
                result: result(0.40, 0.95, 0.95, 0.90),
                label: true,
                weight: 1.0,
            },
            FeedbackExample {
                result: result(0.80, 0.30, 0.10, 0.05),
                label: false,
                weight: 1.0,
            },
        ];
        let model = CalibrationModel::fit(
            &examples,
            CalibrationOptions::default(),
            EvaluationOptions::default(),
        )
        .expect("fit");
        let reranked = model
            .rerank(&[examples[1].result.clone(), examples[0].result.clone()])
            .expect("rerank");
        assert!(reranked[0].probability > reranked[1].probability);
        assert!(reranked[0].result.query_coverage > reranked[1].result.query_coverage);
    }
}
