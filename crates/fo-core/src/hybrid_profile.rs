use serde::{Deserialize, Serialize};

use crate::{FoError, HybridSearchOptions, Result};

pub const HYBRID_FUSION_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridMetricSnapshot {
    pub micro_auprc: f64,
    pub macro_auprc: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_1: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridFusionProfile {
    pub schema_version: u32,
    pub name: String,
    pub lexical_weight: f32,
    pub overlap_weight: f32,
    pub rrf_weight: f32,
    pub rrf_constant: f32,
    pub lexical_saturation: f32,
    pub agreement_bonus: f32,
    pub phrase_bonus: f32,
    pub candidate_multiplier: usize,
    pub minimum_score: f32,
    pub trained_from: Option<String>,
    pub train_queries: usize,
    pub validation_queries: usize,
    pub test_queries: usize,
    pub validation_metrics: Option<HybridMetricSnapshot>,
    pub test_metrics: Option<HybridMetricSnapshot>,
    pub baseline_test_metrics: Option<HybridMetricSnapshot>,
}

impl Default for HybridFusionProfile {
    fn default() -> Self {
        let options = HybridSearchOptions::default();
        Self {
            schema_version: HYBRID_FUSION_PROFILE_SCHEMA_VERSION,
            name: "default".to_owned(),
            lexical_weight: options.lexical_weight,
            overlap_weight: options.overlap_weight,
            rrf_weight: options.rrf_weight,
            rrf_constant: options.rrf_constant,
            lexical_saturation: options.lexical_saturation,
            agreement_bonus: options.agreement_bonus,
            phrase_bonus: options.phrase_bonus,
            candidate_multiplier: options.candidate_multiplier,
            minimum_score: options.minimum_score,
            trained_from: None,
            train_queries: 0,
            validation_queries: 0,
            test_queries: 0,
            validation_metrics: None,
            test_metrics: None,
            baseline_test_metrics: None,
        }
    }
}

impl HybridFusionProfile {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != HYBRID_FUSION_PROFILE_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported hybrid fusion profile schema {}",
                self.schema_version
            )));
        }
        if self.name.trim().is_empty() || self.name.len() > 256 {
            return Err(FoError::InvalidConfig(
                "hybrid fusion profile name must contain between 1 and 256 bytes".to_owned(),
            ));
        }
        for (name, value) in [
            ("lexical_weight", self.lexical_weight),
            ("overlap_weight", self.overlap_weight),
            ("rrf_weight", self.rrf_weight),
            ("agreement_bonus", self.agreement_bonus),
            ("phrase_bonus", self.phrase_bonus),
        ] {
            if !value.is_finite() || value < 0.0 || value > 10.0 {
                return Err(FoError::InvalidConfig(format!(
                    "profile {name} must lie in [0, 10]"
                )));
            }
        }
        if self.lexical_weight + self.overlap_weight + self.rrf_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "profile must assign positive weight to at least one retrieval lane".to_owned(),
            ));
        }
        if !self.rrf_constant.is_finite() || self.rrf_constant <= 0.0 {
            return Err(FoError::InvalidConfig(
                "profile rrf_constant must be positive".to_owned(),
            ));
        }
        if !self.lexical_saturation.is_finite() || self.lexical_saturation <= 0.0 {
            return Err(FoError::InvalidConfig(
                "profile lexical_saturation must be positive".to_owned(),
            ));
        }
        if self.candidate_multiplier == 0 || self.candidate_multiplier > 1_000_000 {
            return Err(FoError::InvalidConfig(
                "profile candidate_multiplier must lie in 1..=1,000,000".to_owned(),
            ));
        }
        if !self.minimum_score.is_finite() || !(0.0..=1.0).contains(&self.minimum_score) {
            return Err(FoError::InvalidConfig(
                "profile minimum_score must lie in [0, 1]".to_owned(),
            ));
        }
        for metrics in [
            self.validation_metrics,
            self.test_metrics,
            self.baseline_test_metrics,
        ]
        .into_iter()
        .flatten()
        {
            validate_metrics(metrics)?;
        }
        Ok(())
    }

    pub fn apply(&self, options: &mut HybridSearchOptions) -> Result<()> {
        self.validate()?;
        options.lexical_weight = self.lexical_weight;
        options.overlap_weight = self.overlap_weight;
        options.rrf_weight = self.rrf_weight;
        options.rrf_constant = self.rrf_constant;
        options.lexical_saturation = self.lexical_saturation;
        options.agreement_bonus = self.agreement_bonus;
        options.phrase_bonus = self.phrase_bonus;
        options.candidate_multiplier = self.candidate_multiplier;
        options.minimum_score = self.minimum_score;
        options.validate()
    }

    pub fn configured_options(&self) -> Result<HybridSearchOptions> {
        let mut options = HybridSearchOptions::default();
        self.apply(&mut options)?;
        Ok(options)
    }
}

fn validate_metrics(metrics: HybridMetricSnapshot) -> Result<()> {
    for (name, value) in [
        ("micro_auprc", metrics.micro_auprc),
        ("macro_auprc", metrics.macro_auprc),
        ("mean_reciprocal_rank", metrics.mean_reciprocal_rank),
        ("recall_at_1", metrics.recall_at_1),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(FoError::InvalidConfig(format!(
                "profile metric {name} must lie in [0, 1]"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::HybridFusionProfile;

    #[test]
    fn default_profile_reconstructs_default_options() {
        let profile = HybridFusionProfile::default();
        let options = profile.configured_options().expect("options");
        assert_eq!(options.lexical_weight, profile.lexical_weight);
        assert_eq!(options.overlap_weight, profile.overlap_weight);
        assert_eq!(options.rrf_weight, profile.rrf_weight);
    }

    #[test]
    fn invalid_zero_weight_profile_fails_closed() {
        let profile = HybridFusionProfile {
            lexical_weight: 0.0,
            overlap_weight: 0.0,
            rrf_weight: 0.0,
            ..HybridFusionProfile::default()
        };
        assert!(profile.validate().is_err());
    }
}
