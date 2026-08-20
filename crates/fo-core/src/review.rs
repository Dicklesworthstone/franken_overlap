use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{FoError, Result};

pub const REVIEW_DECISION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecisionKind {
    Unreviewed,
    Accept,
    Reject,
    Uncertain,
    CorrectSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDecisionRecord {
    #[serde(default = "review_decision_schema_version")]
    pub schema_version: u32,
    pub target_id: String,
    pub candidate_id: String,
    pub decision: ReviewDecisionKind,
    #[serde(default)]
    pub reviewer: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub corrected_source_id: Option<String>,
    #[serde(default)]
    pub accepted_block_indexes: Vec<usize>,
    #[serde(default)]
    pub reviewed_at_unix: u64,
}

impl ReviewDecisionRecord {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REVIEW_DECISION_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported review decision schema {}",
                self.schema_version
            )));
        }
        if self.target_id.trim().is_empty() || self.candidate_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "review target_id and candidate_id must not be empty".to_owned(),
            ));
        }
        if self.target_id == self.candidate_id {
            return Err(FoError::InvalidConfig(
                "review target and candidate IDs must be distinct".to_owned(),
            ));
        }
        match self.decision {
            ReviewDecisionKind::Unreviewed => {
                if self.reviewed_at_unix != 0 || !self.reviewer.trim().is_empty() {
                    return Err(FoError::InvalidConfig(
                        "unreviewed decisions must not claim a reviewer or review time".to_owned(),
                    ));
                }
            }
            ReviewDecisionKind::Accept
            | ReviewDecisionKind::Reject
            | ReviewDecisionKind::Uncertain => {
                if self.reviewer.trim().is_empty() || self.reviewed_at_unix == 0 {
                    return Err(FoError::InvalidConfig(
                        "completed review decisions require reviewer and reviewed_at_unix"
                            .to_owned(),
                    ));
                }
                if self.corrected_source_id.is_some() {
                    return Err(FoError::InvalidConfig(
                        "corrected_source_id is valid only for correct_source decisions".to_owned(),
                    ));
                }
            }
            ReviewDecisionKind::CorrectSource => {
                if self.reviewer.trim().is_empty() || self.reviewed_at_unix == 0 {
                    return Err(FoError::InvalidConfig(
                        "correct_source decisions require reviewer and reviewed_at_unix".to_owned(),
                    ));
                }
                let corrected = self.corrected_source_id.as_deref().ok_or_else(|| {
                    FoError::InvalidConfig(
                        "correct_source decisions require corrected_source_id".to_owned(),
                    )
                })?;
                if corrected.trim().is_empty()
                    || corrected == self.target_id
                    || corrected == self.candidate_id
                {
                    return Err(FoError::InvalidConfig(
                        "corrected source must be nonempty and differ from target and candidate"
                            .to_owned(),
                    ));
                }
            }
        }
        if self
            .accepted_block_indexes
            .windows(2)
            .any(|window| window[0] >= window[1])
        {
            return Err(FoError::InvalidConfig(
                "accepted block indexes must be strictly increasing".to_owned(),
            ));
        }
        Ok(())
    }
}

pub fn validate_review_decisions(decisions: &[ReviewDecisionRecord]) -> Result<()> {
    if decisions.is_empty() {
        return Err(FoError::InvalidConfig(
            "review decision set must not be empty".to_owned(),
        ));
    }
    let mut pairs = BTreeSet::new();
    for decision in decisions {
        decision.validate()?;
        if !pairs.insert((decision.target_id.as_str(), decision.candidate_id.as_str())) {
            return Err(FoError::InvalidConfig(format!(
                "duplicate review decision for target {} and candidate {}",
                decision.target_id, decision.candidate_id
            )));
        }
    }
    Ok(())
}

const fn review_decision_schema_version() -> u32 {
    REVIEW_DECISION_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::{ReviewDecisionKind, ReviewDecisionRecord, validate_review_decisions};

    fn decision(kind: ReviewDecisionKind) -> ReviewDecisionRecord {
        ReviewDecisionRecord {
            schema_version: 1,
            target_id: "target".to_owned(),
            candidate_id: "source".to_owned(),
            decision: kind,
            reviewer: if kind == ReviewDecisionKind::Unreviewed {
                String::new()
            } else {
                "reviewer".to_owned()
            },
            notes: String::new(),
            corrected_source_id: (kind == ReviewDecisionKind::CorrectSource)
                .then(|| "corrected".to_owned()),
            accepted_block_indexes: vec![0, 2],
            reviewed_at_unix: if kind == ReviewDecisionKind::Unreviewed {
                0
            } else {
                1
            },
        }
    }

    #[test]
    fn validates_completed_and_unreviewed_decisions() {
        for kind in [
            ReviewDecisionKind::Unreviewed,
            ReviewDecisionKind::Accept,
            ReviewDecisionKind::Reject,
            ReviewDecisionKind::Uncertain,
            ReviewDecisionKind::CorrectSource,
        ] {
            decision(kind).validate().expect("valid decision");
        }
    }

    #[test]
    fn rejects_duplicate_target_candidate_pairs() {
        let value = decision(ReviewDecisionKind::Accept);
        assert!(validate_review_decisions(&[value.clone(), value]).is_err());
    }

    #[test]
    fn corrected_source_requires_a_distinct_id() {
        let mut value = decision(ReviewDecisionKind::CorrectSource);
        value.corrected_source_id = Some("source".to_owned());
        assert!(value.validate().is_err());
    }
}
