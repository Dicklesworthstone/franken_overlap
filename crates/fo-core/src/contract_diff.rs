use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ClauseKind, ContractAnalysis, ContractClause, ContractObligation, DefinedTerm, EconomicTerm,
    EconomicTermKind, Fingerprint, FoError, NormalizationProfile, ObligationModality, Result,
    global_levenshtein, normalize, qgram_hashes,
};

pub const CONTRACT_COMPARISON_SCHEMA_VERSION: u32 = 1;
pub const CONTRACT_PORTFOLIO_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseChangeKind {
    Unchanged,
    MinorRevision,
    MaterialRevision,
    Moved,
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionChangeKind {
    Unchanged,
    Changed,
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationChangeKind {
    Unchanged,
    Changed,
    Strengthened,
    Weakened,
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicChangeKind {
    Unchanged,
    Increased,
    Decreased,
    Changed,
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestorImpactCategory {
    Economics,
    DurationAndRenewal,
    TerminationAndDefault,
    LiabilityAndIndemnity,
    ExclusivityAndCompetition,
    OperationalFlexibility,
    IntellectualProperty,
    DataAndCybersecurity,
    ComplianceAndAudit,
    AssignmentAndControl,
    RealEstateOperations,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDirection {
    MoreRestrictive,
    LessRestrictive,
    HigherEconomicBurden,
    LowerEconomicBurden,
    AddedProtection,
    RemovedProtection,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClauseChange {
    pub kind: ClauseChangeKind,
    pub clause_kind: ClauseKind,
    pub previous_clause_index: Option<usize>,
    pub current_clause_index: Option<usize>,
    pub previous_heading: Option<String>,
    pub current_heading: Option<String>,
    pub previous_start_byte: Option<usize>,
    pub previous_end_byte: Option<usize>,
    pub current_start_byte: Option<usize>,
    pub current_end_byte: Option<usize>,
    pub text_similarity: f32,
    pub heading_similarity: f32,
    pub alignment_score: f32,
    pub impact: InvestorImpactCategory,
    pub materiality_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionChange {
    pub kind: DefinitionChangeKind,
    pub term: String,
    pub previous: Option<DefinedTerm>,
    pub current: Option<DefinedTerm>,
    pub similarity: f32,
    pub materiality_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObligationChange {
    pub kind: ObligationChangeKind,
    pub previous: Option<ContractObligation>,
    pub current: Option<ContractObligation>,
    pub similarity: f32,
    pub impact: InvestorImpactCategory,
    pub direction: ChangeDirection,
    pub materiality_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicTermChange {
    pub kind: EconomicChangeKind,
    pub term_kind: EconomicTermKind,
    pub previous: Option<EconomicTerm>,
    pub current: Option<EconomicTerm>,
    pub absolute_delta: Option<f64>,
    pub relative_delta: Option<f64>,
    pub direction: ChangeDirection,
    pub materiality_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractChangeAlert {
    pub code: String,
    pub title: String,
    pub rationale: String,
    pub impact: InvestorImpactCategory,
    pub direction: ChangeDirection,
    pub severity: f32,
    pub previous_clause_index: Option<usize>,
    pub current_clause_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContractComparisonOptions {
    pub minimum_clause_alignment_score: f32,
    pub unchanged_similarity: f32,
    pub minor_revision_similarity: f32,
    pub moved_index_distance: usize,
    pub maximum_clause_pair_evaluations: usize,
    pub maximum_exact_tokens: usize,
    pub qgram_size: usize,
    pub minimum_obligation_alignment_score: f32,
    pub economic_equality_tolerance: f64,
    pub economic_materiality_fraction: f64,
    pub maximum_alerts: usize,
}

impl Default for ContractComparisonOptions {
    fn default() -> Self {
        Self {
            minimum_clause_alignment_score: 0.42,
            unchanged_similarity: 0.975,
            minor_revision_similarity: 0.82,
            moved_index_distance: 2,
            maximum_clause_pair_evaluations: 1_000_000,
            maximum_exact_tokens: 4_096,
            qgram_size: 5,
            minimum_obligation_alignment_score: 0.56,
            economic_equality_tolerance: 1.0e-9,
            economic_materiality_fraction: 0.05,
            maximum_alerts: 1_000,
        }
    }
}

impl ContractComparisonOptions {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            (
                "minimum_clause_alignment_score",
                self.minimum_clause_alignment_score,
            ),
            ("unchanged_similarity", self.unchanged_similarity),
            ("minor_revision_similarity", self.minor_revision_similarity),
            (
                "minimum_obligation_alignment_score",
                self.minimum_obligation_alignment_score,
            ),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "contract comparison {name} must lie in [0, 1]"
                )));
            }
        }
        if self.minor_revision_similarity > self.unchanged_similarity
            || self.maximum_clause_pair_evaluations == 0
            || self.maximum_exact_tokens == 0
            || !(2..=32).contains(&self.qgram_size)
            || !self.economic_equality_tolerance.is_finite()
            || self.economic_equality_tolerance < 0.0
            || !self.economic_materiality_fraction.is_finite()
            || self.economic_materiality_fraction < 0.0
            || self.maximum_alerts == 0
        {
            return Err(FoError::InvalidConfig(
                "contract comparison thresholds or limits are invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractComparison {
    pub schema_version: u32,
    pub previous_profile: crate::ContractProfile,
    pub current_profile: crate::ContractProfile,
    pub previous_bytes: usize,
    pub current_bytes: usize,
    pub overall_similarity: f32,
    pub matched_clauses: usize,
    pub added_clauses: usize,
    pub removed_clauses: usize,
    pub materially_revised_clauses: usize,
    pub clause_changes: Vec<ClauseChange>,
    pub definition_changes: Vec<DefinitionChange>,
    pub obligation_changes: Vec<ObligationChange>,
    pub economic_term_changes: Vec<EconomicTermChange>,
    pub alerts: Vec<ContractChangeAlert>,
}

pub fn compare_contracts(
    previous: &ContractAnalysis,
    current: &ContractAnalysis,
    options: &ContractComparisonOptions,
) -> Result<ContractComparison> {
    options.validate()?;
    validate_analysis(previous)?;
    validate_analysis(current)?;
    let clause_changes = compare_clauses(previous, current, options)?;
    let definition_changes =
        compare_definitions(&previous.definitions, &current.definitions, options);
    let obligation_changes = compare_obligations(previous, current, options)?;
    let economic_term_changes = compare_economic_terms(previous, current, options);
    let matched_clauses = clause_changes
        .iter()
        .filter(|change| {
            change.previous_clause_index.is_some() && change.current_clause_index.is_some()
        })
        .count();
    let added_clauses = clause_changes
        .iter()
        .filter(|change| change.kind == ClauseChangeKind::Added)
        .count();
    let removed_clauses = clause_changes
        .iter()
        .filter(|change| change.kind == ClauseChangeKind::Removed)
        .count();
    let materially_revised_clauses = clause_changes
        .iter()
        .filter(|change| change.kind == ClauseChangeKind::MaterialRevision)
        .count();
    let overall_similarity = weighted_clause_similarity(&clause_changes);
    let mut alerts = build_change_alerts(
        &clause_changes,
        &definition_changes,
        &obligation_changes,
        &economic_term_changes,
        options.maximum_alerts,
    );
    alerts.sort_unstable_by(|left, right| {
        right
            .severity
            .total_cmp(&left.severity)
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.title.cmp(&right.title))
    });
    alerts.truncate(options.maximum_alerts);
    Ok(ContractComparison {
        schema_version: CONTRACT_COMPARISON_SCHEMA_VERSION,
        previous_profile: previous.profile,
        current_profile: current.profile,
        previous_bytes: previous.bytes,
        current_bytes: current.bytes,
        overall_similarity,
        matched_clauses,
        added_clauses,
        removed_clauses,
        materially_revised_clauses,
        clause_changes,
        definition_changes,
        obligation_changes,
        economic_term_changes,
        alerts,
    })
}

fn validate_analysis(analysis: &ContractAnalysis) -> Result<()> {
    if analysis.schema_version != crate::CONTRACT_ANALYSIS_SCHEMA_VERSION {
        return Err(FoError::InvalidConfig(format!(
            "unsupported contract analysis schema {}",
            analysis.schema_version
        )));
    }
    for clause in &analysis.clauses {
        if clause.start_byte >= clause.end_byte || clause.end_byte > analysis.bytes {
            return Err(FoError::InvalidConfig(format!(
                "contract clause {} has invalid byte coordinates",
                clause.index
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ClausePair {
    previous: usize,
    current: usize,
    text_similarity: f32,
    heading_similarity: f32,
    score: f32,
}

fn compare_clauses(
    previous: &ContractAnalysis,
    current: &ContractAnalysis,
    options: &ContractComparisonOptions,
) -> Result<Vec<ClauseChange>> {
    let evaluations = previous.clauses.len().saturating_mul(current.clauses.len());
    if evaluations > options.maximum_clause_pair_evaluations {
        return Err(FoError::InvalidConfig(format!(
            "contract comparison requires {evaluations} clause-pair evaluations, exceeding {}",
            options.maximum_clause_pair_evaluations
        )));
    }
    let profile = NormalizationProfile::default();
    let mut pairs = Vec::new();
    for previous_clause in &previous.clauses {
        for current_clause in &current.clauses {
            let previous_kind = primary_kind(previous_clause);
            let current_kind = primary_kind(current_clause);
            let heading_similarity = text_similarity(
                &previous_clause.heading,
                &current_clause.heading,
                options,
                &profile,
            )?;
            if previous_kind != current_kind && heading_similarity < 0.45 {
                continue;
            }
            let text_similarity = text_similarity(
                &previous_clause.text,
                &current_clause.text,
                options,
                &profile,
            )?;
            let kind_bonus = if previous_kind == current_kind {
                0.18
            } else {
                0.0
            };
            let score =
                (0.72 * text_similarity + 0.10 * heading_similarity + kind_bonus).clamp(0.0, 1.0);
            if score >= options.minimum_clause_alignment_score {
                pairs.push(ClausePair {
                    previous: previous_clause.index,
                    current: current_clause.index,
                    text_similarity,
                    heading_similarity,
                    score,
                });
            }
        }
    }
    pairs.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.text_similarity.total_cmp(&left.text_similarity))
            .then_with(|| left.previous.cmp(&right.previous))
            .then_with(|| left.current.cmp(&right.current))
    });
    let mut used_previous = BTreeSet::new();
    let mut used_current = BTreeSet::new();
    let mut changes = Vec::new();
    for pair in pairs {
        if !used_previous.insert(pair.previous) || !used_current.insert(pair.current) {
            continue;
        }
        let old = &previous.clauses[pair.previous];
        let new = &current.clauses[pair.current];
        let moved = old.index.abs_diff(new.index) > options.moved_index_distance;
        let kind = if pair.text_similarity >= options.unchanged_similarity {
            if moved {
                ClauseChangeKind::Moved
            } else {
                ClauseChangeKind::Unchanged
            }
        } else if pair.text_similarity >= options.minor_revision_similarity {
            ClauseChangeKind::MinorRevision
        } else {
            ClauseChangeKind::MaterialRevision
        };
        let clause_kind = primary_kind(new);
        let materiality_score = clause_materiality(clause_kind)
            * (1.0 - pair.text_similarity).max(if moved { 0.08 } else { 0.0 });
        changes.push(ClauseChange {
            kind,
            clause_kind,
            previous_clause_index: Some(old.index),
            current_clause_index: Some(new.index),
            previous_heading: Some(old.heading.clone()),
            current_heading: Some(new.heading.clone()),
            previous_start_byte: Some(old.start_byte),
            previous_end_byte: Some(old.end_byte),
            current_start_byte: Some(new.start_byte),
            current_end_byte: Some(new.end_byte),
            text_similarity: pair.text_similarity,
            heading_similarity: pair.heading_similarity,
            alignment_score: pair.score,
            impact: clause_impact(clause_kind),
            materiality_score: materiality_score.clamp(0.0, 1.0),
        });
    }
    for old in &previous.clauses {
        if used_previous.contains(&old.index) {
            continue;
        }
        let clause_kind = primary_kind(old);
        changes.push(ClauseChange {
            kind: ClauseChangeKind::Removed,
            clause_kind,
            previous_clause_index: Some(old.index),
            current_clause_index: None,
            previous_heading: Some(old.heading.clone()),
            current_heading: None,
            previous_start_byte: Some(old.start_byte),
            previous_end_byte: Some(old.end_byte),
            current_start_byte: None,
            current_end_byte: None,
            text_similarity: 0.0,
            heading_similarity: 0.0,
            alignment_score: 0.0,
            impact: clause_impact(clause_kind),
            materiality_score: clause_materiality(clause_kind),
        });
    }
    for new in &current.clauses {
        if used_current.contains(&new.index) {
            continue;
        }
        let clause_kind = primary_kind(new);
        changes.push(ClauseChange {
            kind: ClauseChangeKind::Added,
            clause_kind,
            previous_clause_index: None,
            current_clause_index: Some(new.index),
            previous_heading: None,
            current_heading: Some(new.heading.clone()),
            previous_start_byte: None,
            previous_end_byte: None,
            current_start_byte: Some(new.start_byte),
            current_end_byte: Some(new.end_byte),
            text_similarity: 0.0,
            heading_similarity: 0.0,
            alignment_score: 0.0,
            impact: clause_impact(clause_kind),
            materiality_score: clause_materiality(clause_kind),
        });
    }
    changes.sort_unstable_by(|left, right| {
        left.current_clause_index
            .unwrap_or(usize::MAX)
            .cmp(&right.current_clause_index.unwrap_or(usize::MAX))
            .then_with(|| {
                left.previous_clause_index
                    .unwrap_or(usize::MAX)
                    .cmp(&right.previous_clause_index.unwrap_or(usize::MAX))
            })
    });
    Ok(changes)
}

fn text_similarity(
    left: &str,
    right: &str,
    options: &ContractComparisonOptions,
    profile: &NormalizationProfile,
) -> Result<f32> {
    let left = normalize(left, profile);
    let right = normalize(right, profile);
    if left.tokens.is_empty() && right.tokens.is_empty() {
        return Ok(1.0);
    }
    if left.tokens.is_empty() || right.tokens.is_empty() {
        return Ok(0.0);
    }
    if left.tokens.len().max(right.tokens.len()) <= options.maximum_exact_tokens {
        let distance = global_levenshtein(&left.tokens, &right.tokens);
        return Ok((1.0
            - distance as f32 / left.tokens.len().max(right.tokens.len()).max(1) as f32)
            .clamp(0.0, 1.0));
    }
    let left_features = qgram_set(&left.tokens, options.qgram_size)?;
    let right_features = qgram_set(&right.tokens, options.qgram_size)?;
    if left_features.is_empty() || right_features.is_empty() {
        let shorter = left.tokens.len().min(right.tokens.len()) as f32;
        let longer = left.tokens.len().max(right.tokens.len()) as f32;
        return Ok((shorter / longer.max(1.0)).clamp(0.0, 1.0));
    }
    let intersection = left_features.intersection(&right_features).count();
    let union = left_features.union(&right_features).count();
    Ok((intersection as f32 / union.max(1) as f32).clamp(0.0, 1.0))
}

fn qgram_set(tokens: &[u32], qgram_size: usize) -> Result<BTreeSet<Fingerprint>> {
    Ok(qgram_hashes(tokens, qgram_size)?
        .into_iter()
        .map(|feature| feature.fingerprint)
        .collect())
}

fn primary_kind(clause: &ContractClause) -> ClauseKind {
    clause
        .classifications
        .first()
        .map_or(ClauseKind::Unclassified, |classification| {
            classification.kind
        })
}

fn compare_definitions(
    previous: &[DefinedTerm],
    current: &[DefinedTerm],
    options: &ContractComparisonOptions,
) -> Vec<DefinitionChange> {
    let previous_map = definition_map(previous);
    let current_map = definition_map(current);
    let terms = previous_map
        .keys()
        .chain(current_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let profile = NormalizationProfile::default();
    let mut output = Vec::new();
    for term in terms {
        match (previous_map.get(&term), current_map.get(&term)) {
            (Some(old), Some(new)) => {
                let similarity =
                    text_similarity(&old.definition, &new.definition, options, &profile)
                        .unwrap_or(0.0);
                let kind = if similarity >= options.unchanged_similarity {
                    DefinitionChangeKind::Unchanged
                } else {
                    DefinitionChangeKind::Changed
                };
                output.push(DefinitionChange {
                    kind,
                    term: new.term.clone(),
                    previous: Some((*old).clone()),
                    current: Some((*new).clone()),
                    similarity,
                    materiality_score: if kind == DefinitionChangeKind::Changed {
                        (0.35 + 0.65 * (1.0 - similarity)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                });
            }
            (Some(old), None) => output.push(DefinitionChange {
                kind: DefinitionChangeKind::Removed,
                term: old.term.clone(),
                previous: Some((*old).clone()),
                current: None,
                similarity: 0.0,
                materiality_score: 0.70,
            }),
            (None, Some(new)) => output.push(DefinitionChange {
                kind: DefinitionChangeKind::Added,
                term: new.term.clone(),
                previous: None,
                current: Some((*new).clone()),
                similarity: 0.0,
                materiality_score: 0.60,
            }),
            (None, None) => {}
        }
    }
    output
}

fn definition_map(definitions: &[DefinedTerm]) -> BTreeMap<String, &DefinedTerm> {
    let mut output = BTreeMap::new();
    for definition in definitions {
        output
            .entry(definition.term.trim().to_ascii_lowercase())
            .or_insert(definition);
    }
    output
}

#[derive(Debug)]
struct ObligationPair {
    previous: usize,
    current: usize,
    similarity: f32,
}

fn compare_obligations(
    previous: &ContractAnalysis,
    current: &ContractAnalysis,
    options: &ContractComparisonOptions,
) -> Result<Vec<ObligationChange>> {
    let profile = NormalizationProfile::default();
    let mut pairs = Vec::new();
    for (old_index, old) in previous.obligations.iter().enumerate() {
        for (new_index, new) in current.obligations.iter().enumerate() {
            let old_kind = previous
                .clauses
                .get(old.clause_index)
                .map_or(ClauseKind::Unclassified, primary_kind);
            let new_kind = current
                .clauses
                .get(new.clause_index)
                .map_or(ClauseKind::Unclassified, primary_kind);
            if old_kind != new_kind {
                continue;
            }
            let sentence = text_similarity(&old.sentence, &new.sentence, options, &profile)?;
            let subject = text_similarity(&old.subject, &new.subject, options, &profile)?;
            let action = text_similarity(&old.action, &new.action, options, &profile)?;
            let similarity = (0.55 * sentence + 0.15 * subject + 0.30 * action).clamp(0.0, 1.0);
            if similarity >= options.minimum_obligation_alignment_score {
                pairs.push(ObligationPair {
                    previous: old_index,
                    current: new_index,
                    similarity,
                });
            }
        }
    }
    pairs.sort_unstable_by(|left, right| {
        right
            .similarity
            .total_cmp(&left.similarity)
            .then_with(|| left.previous.cmp(&right.previous))
            .then_with(|| left.current.cmp(&right.current))
    });
    let mut used_previous = BTreeSet::new();
    let mut used_current = BTreeSet::new();
    let mut output = Vec::new();
    for pair in pairs {
        if !used_previous.insert(pair.previous) || !used_current.insert(pair.current) {
            continue;
        }
        let old = &previous.obligations[pair.previous];
        let new = &current.obligations[pair.current];
        let clause_kind = current
            .clauses
            .get(new.clause_index)
            .map_or(ClauseKind::Unclassified, primary_kind);
        let old_strength = modality_strength(old.modality);
        let new_strength = modality_strength(new.modality);
        let kind =
            if pair.similarity >= options.unchanged_similarity && old.modality == new.modality {
                ObligationChangeKind::Unchanged
            } else if new_strength > old_strength {
                ObligationChangeKind::Strengthened
            } else if new_strength < old_strength {
                ObligationChangeKind::Weakened
            } else {
                ObligationChangeKind::Changed
            };
        let direction = match kind {
            ObligationChangeKind::Strengthened => ChangeDirection::MoreRestrictive,
            ObligationChangeKind::Weakened => ChangeDirection::LessRestrictive,
            _ => ChangeDirection::Ambiguous,
        };
        let materiality = clause_materiality(clause_kind)
            * (1.0 - pair.similarity
                + if old.modality != new.modality {
                    0.35
                } else {
                    0.0
                });
        output.push(ObligationChange {
            kind,
            previous: Some(old.clone()),
            current: Some(new.clone()),
            similarity: pair.similarity,
            impact: clause_impact(clause_kind),
            direction,
            materiality_score: materiality.clamp(0.0, 1.0),
        });
    }
    for (index, old) in previous.obligations.iter().enumerate() {
        if used_previous.contains(&index) {
            continue;
        }
        let clause_kind = previous
            .clauses
            .get(old.clause_index)
            .map_or(ClauseKind::Unclassified, primary_kind);
        output.push(ObligationChange {
            kind: ObligationChangeKind::Removed,
            previous: Some(old.clone()),
            current: None,
            similarity: 0.0,
            impact: clause_impact(clause_kind),
            direction: ChangeDirection::LessRestrictive,
            materiality_score: clause_materiality(clause_kind),
        });
    }
    for (index, new) in current.obligations.iter().enumerate() {
        if used_current.contains(&index) {
            continue;
        }
        let clause_kind = current
            .clauses
            .get(new.clause_index)
            .map_or(ClauseKind::Unclassified, primary_kind);
        output.push(ObligationChange {
            kind: ObligationChangeKind::Added,
            previous: None,
            current: Some(new.clone()),
            similarity: 0.0,
            impact: clause_impact(clause_kind),
            direction: ChangeDirection::MoreRestrictive,
            materiality_score: clause_materiality(clause_kind),
        });
    }
    Ok(output)
}

fn modality_strength(modality: ObligationModality) -> u8 {
    match modality {
        ObligationModality::May => 1,
        ObligationModality::Should => 2,
        ObligationModality::Will => 3,
        ObligationModality::Shall | ObligationModality::Must => 4,
        ObligationModality::MayNot => 5,
        ObligationModality::ShallNot | ObligationModality::MustNot => 6,
    }
}

fn compare_economic_terms(
    previous: &ContractAnalysis,
    current: &ContractAnalysis,
    options: &ContractComparisonOptions,
) -> Vec<EconomicTermChange> {
    let mut candidates = Vec::<(usize, usize, f32)>::new();
    for (old_index, old) in previous.economic_terms.iter().enumerate() {
        for (new_index, new) in current.economic_terms.iter().enumerate() {
            if old.kind != new.kind {
                continue;
            }
            let old_clause_kind = previous
                .clauses
                .get(old.clause_index)
                .map_or(ClauseKind::Unclassified, primary_kind);
            let new_clause_kind = current
                .clauses
                .get(new.clause_index)
                .map_or(ClauseKind::Unclassified, primary_kind);
            if old_clause_kind != new_clause_kind {
                continue;
            }
            let score = economic_pair_score(old, new);
            candidates.push((old_index, new_index, score));
        }
    }
    candidates.sort_unstable_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    let mut used_previous = BTreeSet::new();
    let mut used_current = BTreeSet::new();
    let mut output = Vec::new();
    for (old_index, new_index, _score) in candidates {
        if !used_previous.insert(old_index) || !used_current.insert(new_index) {
            continue;
        }
        let old = &previous.economic_terms[old_index];
        let new = &current.economic_terms[new_index];
        let delta = old
            .normalized_value
            .zip(new.normalized_value)
            .map(|(old, new)| new - old);
        let relative_delta =
            old.normalized_value
                .zip(new.normalized_value)
                .and_then(|(old, new)| {
                    (old.abs() > options.economic_equality_tolerance)
                        .then_some((new - old) / old.abs())
                });
        let equal = delta.is_some_and(|value| value.abs() <= options.economic_equality_tolerance)
            && old.unit == new.unit
            && old.currency == new.currency;
        let kind = if equal {
            EconomicChangeKind::Unchanged
        } else if delta.is_some_and(|value| value > options.economic_equality_tolerance) {
            EconomicChangeKind::Increased
        } else if delta.is_some_and(|value| value < -options.economic_equality_tolerance) {
            EconomicChangeKind::Decreased
        } else {
            EconomicChangeKind::Changed
        };
        let direction = match kind {
            EconomicChangeKind::Increased => ChangeDirection::HigherEconomicBurden,
            EconomicChangeKind::Decreased => ChangeDirection::LowerEconomicBurden,
            _ => ChangeDirection::Ambiguous,
        };
        output.push(EconomicTermChange {
            kind,
            term_kind: new.kind,
            previous: Some(old.clone()),
            current: Some(new.clone()),
            absolute_delta: delta,
            relative_delta,
            direction,
            materiality_score: economic_materiality(new.kind, relative_delta, kind),
        });
    }
    for (index, old) in previous.economic_terms.iter().enumerate() {
        if !used_previous.contains(&index) {
            output.push(EconomicTermChange {
                kind: EconomicChangeKind::Removed,
                term_kind: old.kind,
                previous: Some(old.clone()),
                current: None,
                absolute_delta: None,
                relative_delta: None,
                direction: ChangeDirection::RemovedProtection,
                materiality_score: economic_kind_weight(old.kind),
            });
        }
    }
    for (index, new) in current.economic_terms.iter().enumerate() {
        if !used_current.contains(&index) {
            output.push(EconomicTermChange {
                kind: EconomicChangeKind::Added,
                term_kind: new.kind,
                previous: None,
                current: Some(new.clone()),
                absolute_delta: None,
                relative_delta: None,
                direction: ChangeDirection::HigherEconomicBurden,
                materiality_score: economic_kind_weight(new.kind),
            });
        }
    }
    output
}

fn economic_pair_score(left: &EconomicTerm, right: &EconomicTerm) -> f32 {
    let unit = if left.unit == right.unit { 0.20 } else { 0.0 };
    let currency = if left.currency == right.currency {
        0.10
    } else {
        0.0
    };
    let context = word_jaccard(&left.context, &right.context);
    (0.70 * context + unit + currency).clamp(0.0, 1.0)
}

fn word_jaccard(left: &str, right: &str) -> f32 {
    let words = |value: &str| {
        value
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| word.len() >= 3)
            .map(str::to_ascii_lowercase)
            .collect::<BTreeSet<_>>()
    };
    let left = words(left);
    let right = words(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    left.intersection(&right).count() as f32 / left.union(&right).count().max(1) as f32
}

fn weighted_clause_similarity(changes: &[ClauseChange]) -> f32 {
    let mut weighted = 0.0f64;
    let mut total = 0.0f64;
    for change in changes {
        let weight = f64::from(clause_materiality(change.clause_kind).max(0.10));
        total += weight;
        weighted += weight * f64::from(change.text_similarity);
    }
    if total == 0.0 {
        0.0
    } else {
        (weighted / total).clamp(0.0, 1.0) as f32
    }
}

fn build_change_alerts(
    clauses: &[ClauseChange],
    definitions: &[DefinitionChange],
    obligations: &[ObligationChange],
    economics: &[EconomicTermChange],
    maximum: usize,
) -> Vec<ContractChangeAlert> {
    let mut output = Vec::new();
    for change in clauses {
        if change.materiality_score < 0.35 || change.kind == ClauseChangeKind::Unchanged {
            continue;
        }
        let direction = match change.kind {
            ClauseChangeKind::Added => ChangeDirection::MoreRestrictive,
            ClauseChangeKind::Removed => ChangeDirection::RemovedProtection,
            _ => ChangeDirection::Ambiguous,
        };
        output.push(ContractChangeAlert {
            code: format!("clause_{:?}", change.kind).to_ascii_lowercase(),
            title: format!("{:?} clause {:?}", change.kind, change.clause_kind),
            rationale: format!(
                "clause similarity {:.3}; alignment {:.3}; materiality {:.3}",
                change.text_similarity, change.alignment_score, change.materiality_score
            ),
            impact: change.impact,
            direction,
            severity: change.materiality_score,
            previous_clause_index: change.previous_clause_index,
            current_clause_index: change.current_clause_index,
        });
        if output.len() >= maximum {
            return output;
        }
    }
    for change in definitions {
        if change.kind == DefinitionChangeKind::Unchanged || change.materiality_score < 0.45 {
            continue;
        }
        output.push(ContractChangeAlert {
            code: format!("definition_{:?}", change.kind).to_ascii_lowercase(),
            title: format!("Defined term changed: {}", change.term),
            rationale: format!(
                "definition change {:?}; similarity {:.3}",
                change.kind, change.similarity
            ),
            impact: InvestorImpactCategory::Other,
            direction: ChangeDirection::Ambiguous,
            severity: change.materiality_score,
            previous_clause_index: change.previous.as_ref().map(|value| value.clause_index),
            current_clause_index: change.current.as_ref().map(|value| value.clause_index),
        });
        if output.len() >= maximum {
            return output;
        }
    }
    for change in obligations {
        if change.kind == ObligationChangeKind::Unchanged || change.materiality_score < 0.40 {
            continue;
        }
        output.push(ContractChangeAlert {
            code: format!("obligation_{:?}", change.kind).to_ascii_lowercase(),
            title: format!("Obligation {:?}", change.kind),
            rationale: format!(
                "obligation similarity {:.3}; direction {:?}",
                change.similarity, change.direction
            ),
            impact: change.impact,
            direction: change.direction,
            severity: change.materiality_score,
            previous_clause_index: change.previous.as_ref().map(|value| value.clause_index),
            current_clause_index: change.current.as_ref().map(|value| value.clause_index),
        });
        if output.len() >= maximum {
            return output;
        }
    }
    for change in economics {
        if change.kind == EconomicChangeKind::Unchanged || change.materiality_score < 0.35 {
            continue;
        }
        output.push(ContractChangeAlert {
            code: format!("economic_{:?}", change.kind).to_ascii_lowercase(),
            title: format!("Economic term {:?}: {:?}", change.term_kind, change.kind),
            rationale: match change.relative_delta {
                Some(delta) => format!("relative change {:+.1}%", delta * 100.0),
                None => format!("economic term was {:?}", change.kind),
            },
            impact: InvestorImpactCategory::Economics,
            direction: change.direction,
            severity: change.materiality_score,
            previous_clause_index: change.previous.as_ref().map(|value| value.clause_index),
            current_clause_index: change.current.as_ref().map(|value| value.clause_index),
        });
        if output.len() >= maximum {
            return output;
        }
    }
    output
}

fn clause_impact(kind: ClauseKind) -> InvestorImpactCategory {
    match kind {
        ClauseKind::Fees
        | ClauseKind::Invoicing
        | ClauseKind::Payment
        | ClauseKind::Expenses
        | ClauseKind::Taxes
        | ClauseKind::BaseRent
        | ClauseKind::PercentageRent
        | ClauseKind::CommonAreaMaintenance
        | ClauseKind::SecurityDeposit
        | ClauseKind::TenantImprovement => InvestorImpactCategory::Economics,
        ClauseKind::Term
        | ClauseKind::Renewal
        | ClauseKind::RenewalOption
        | ClauseKind::Holdover => InvestorImpactCategory::DurationAndRenewal,
        ClauseKind::Termination
        | ClauseKind::KickOut
        | ClauseKind::Casualty
        | ClauseKind::Condemnation => InvestorImpactCategory::TerminationAndDefault,
        ClauseKind::Indemnification
        | ClauseKind::LimitationOfLiability
        | ClauseKind::Insurance
        | ClauseKind::Guaranty => InvestorImpactCategory::LiabilityAndIndemnity,
        ClauseKind::Exclusivity
        | ClauseKind::CoTenancy
        | ClauseKind::RadiusRestriction
        | ClauseKind::Standstill
        | ClauseKind::NonSolicit
        | ClauseKind::NoHire
        | ClauseKind::NonCircumvention => InvestorImpactCategory::ExclusivityAndCompetition,
        ClauseKind::Scope
        | ClauseKind::Deliverables
        | ClauseKind::Milestones
        | ClauseKind::Acceptance
        | ClauseKind::ChangeControl
        | ClauseKind::ServiceLevels
        | ClauseKind::Staffing
        | ClauseKind::Subcontracting
        | ClauseKind::TransitionAssistance => InvestorImpactCategory::OperationalFlexibility,
        ClauseKind::IntellectualProperty
        | ClauseKind::WorkProduct
        | ClauseKind::BackgroundIp
        | ClauseKind::OpenSource
        | ClauseKind::NoLicense => InvestorImpactCategory::IntellectualProperty,
        ClauseKind::DataProtection | ClauseKind::DataSecurity => {
            InvestorImpactCategory::DataAndCybersecurity
        }
        ClauseKind::Audit
        | ClauseKind::Compliance
        | ClauseKind::Representations
        | ClauseKind::Warranties => InvestorImpactCategory::ComplianceAndAudit,
        ClauseKind::Assignment
        | ClauseKind::AssignmentAndSubletting
        | ClauseKind::ChangeOfControl => InvestorImpactCategory::AssignmentAndControl,
        ClauseKind::Premises
        | ClauseKind::PermittedUse
        | ClauseKind::Utilities
        | ClauseKind::MaintenanceAndRepair
        | ClauseKind::Alterations
        | ClauseKind::Signage
        | ClauseKind::GoDark
        | ClauseKind::DeliveryCondition
        | ClauseKind::OpeningCovenant
        | ClauseKind::OperatingHours
        | ClauseKind::SubordinationNondisturbance
        | ClauseKind::Estoppel
        | ClauseKind::Surrender => InvestorImpactCategory::RealEstateOperations,
        _ => InvestorImpactCategory::Other,
    }
}

fn clause_materiality(kind: ClauseKind) -> f32 {
    match clause_impact(kind) {
        InvestorImpactCategory::Economics
        | InvestorImpactCategory::TerminationAndDefault
        | InvestorImpactCategory::LiabilityAndIndemnity
        | InvestorImpactCategory::ExclusivityAndCompetition => 1.0,
        InvestorImpactCategory::DurationAndRenewal
        | InvestorImpactCategory::IntellectualProperty
        | InvestorImpactCategory::DataAndCybersecurity
        | InvestorImpactCategory::AssignmentAndControl => 0.88,
        InvestorImpactCategory::OperationalFlexibility
        | InvestorImpactCategory::ComplianceAndAudit
        | InvestorImpactCategory::RealEstateOperations => 0.72,
        InvestorImpactCategory::Other => 0.45,
    }
}

fn economic_kind_weight(kind: EconomicTermKind) -> f32 {
    match kind {
        EconomicTermKind::BaseRent
        | EconomicTermKind::RentEscalation
        | EconomicTermKind::PercentageRent
        | EconomicTermKind::LiabilityCap
        | EconomicTermKind::TenantImprovementAllowance => 1.0,
        EconomicTermKind::Money
        | EconomicTermKind::PaymentTerm
        | EconomicTermKind::SecurityDeposit
        | EconomicTermKind::InsuranceLimit
        | EconomicTermKind::ServiceLevel
        | EconomicTermKind::InterestRate
        | EconomicTermKind::RenewalTerm => 0.82,
        EconomicTermKind::Percentage
        | EconomicTermKind::Duration
        | EconomicTermKind::NoticePeriod => 0.62,
    }
}

fn economic_materiality(
    kind: EconomicTermKind,
    relative_delta: Option<f64>,
    change: EconomicChangeKind,
) -> f32 {
    if change == EconomicChangeKind::Unchanged {
        return 0.0;
    }
    let delta_factor = relative_delta
        .map(|value| (value.abs() as f32 / 0.25).clamp(0.20, 1.0))
        .unwrap_or(0.70);
    (economic_kind_weight(kind) * delta_factor).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClausePrevalence {
    pub kind: ClauseKind,
    pub documents_with_clause: usize,
    pub total_documents: usize,
    pub prevalence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicDistribution {
    pub kind: EconomicTermKind,
    pub observations: usize,
    pub minimum: f64,
    pub median: f64,
    pub maximum: f64,
    pub median_absolute_deviation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOutlier {
    pub document_id: String,
    pub code: String,
    pub description: String,
    pub severity: f32,
    pub clause_kind: Option<ClauseKind>,
    pub economic_term_kind: Option<EconomicTermKind>,
    pub observed_value: Option<f64>,
    pub portfolio_median: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioDocumentAnalysis {
    pub document_id: String,
    pub analysis: ContractAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContractPortfolioOptions {
    pub rare_clause_prevalence: f32,
    pub common_clause_prevalence: f32,
    pub outlier_mad_multiplier: f64,
    pub minimum_distribution_observations: usize,
    pub maximum_outliers: usize,
}

impl Default for ContractPortfolioOptions {
    fn default() -> Self {
        Self {
            rare_clause_prevalence: 0.10,
            common_clause_prevalence: 0.80,
            outlier_mad_multiplier: 3.5,
            minimum_distribution_observations: 5,
            maximum_outliers: 10_000,
        }
    }
}

impl ContractPortfolioOptions {
    pub fn validate(&self) -> Result<()> {
        if !self.rare_clause_prevalence.is_finite()
            || !self.common_clause_prevalence.is_finite()
            || !(0.0..=1.0).contains(&self.rare_clause_prevalence)
            || !(0.0..=1.0).contains(&self.common_clause_prevalence)
            || self.rare_clause_prevalence > self.common_clause_prevalence
            || !self.outlier_mad_multiplier.is_finite()
            || self.outlier_mad_multiplier <= 0.0
            || self.minimum_distribution_observations == 0
            || self.maximum_outliers == 0
        {
            return Err(FoError::InvalidConfig(
                "contract portfolio thresholds or limits are invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractPortfolioBenchmark {
    pub schema_version: u32,
    pub documents: usize,
    pub profiles: BTreeMap<String, usize>,
    pub clause_prevalence: Vec<ClausePrevalence>,
    pub economic_distributions: Vec<EconomicDistribution>,
    pub obligation_modalities: BTreeMap<String, usize>,
    pub outliers: Vec<PortfolioOutlier>,
}

pub fn benchmark_contract_portfolio(
    documents: &[PortfolioDocumentAnalysis],
    options: &ContractPortfolioOptions,
) -> Result<ContractPortfolioBenchmark> {
    options.validate()?;
    if documents.is_empty() {
        return Err(FoError::InvalidConfig(
            "contract portfolio must contain at least one document".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut profile_counts = BTreeMap::new();
    let mut clause_documents = BTreeMap::<ClauseKind, BTreeSet<String>>::new();
    let mut economic_values = BTreeMap::<EconomicTermKind, Vec<(String, f64)>>::new();
    let mut obligation_modalities = BTreeMap::new();
    for document in documents {
        if document.document_id.trim().is_empty() || !ids.insert(document.document_id.clone()) {
            return Err(FoError::InvalidConfig(
                "contract portfolio document IDs must be nonempty and unique".to_owned(),
            ));
        }
        validate_analysis(&document.analysis)?;
        *profile_counts
            .entry(format!("{:?}", document.analysis.profile).to_ascii_lowercase())
            .or_insert(0usize) += 1;
        let kinds = document
            .analysis
            .clauses
            .iter()
            .map(primary_kind)
            .collect::<BTreeSet<_>>();
        for kind in kinds {
            clause_documents
                .entry(kind)
                .or_default()
                .insert(document.document_id.clone());
        }
        for term in &document.analysis.economic_terms {
            if let Some(value) = term.normalized_value
                && value.is_finite()
            {
                economic_values
                    .entry(term.kind)
                    .or_default()
                    .push((document.document_id.clone(), value));
            }
        }
        for obligation in &document.analysis.obligations {
            *obligation_modalities
                .entry(format!("{:?}", obligation.modality).to_ascii_lowercase())
                .or_insert(0usize) += 1;
        }
    }
    let mut clause_prevalence = clause_documents
        .into_iter()
        .map(|(kind, ids)| ClausePrevalence {
            kind,
            documents_with_clause: ids.len(),
            total_documents: documents.len(),
            prevalence: ids.len() as f32 / documents.len() as f32,
        })
        .collect::<Vec<_>>();
    clause_prevalence.sort_unstable_by_key(|entry| entry.kind);

    let mut economic_distributions = Vec::new();
    let mut outliers = Vec::new();
    for (kind, observations) in economic_values {
        let mut values = observations
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        values.sort_unstable_by(f64::total_cmp);
        let median_value = median(&values);
        let deviations = values
            .iter()
            .map(|value| (value - median_value).abs())
            .collect::<Vec<_>>();
        let mut deviations_sorted = deviations;
        deviations_sorted.sort_unstable_by(f64::total_cmp);
        let mad = median(&deviations_sorted);
        economic_distributions.push(EconomicDistribution {
            kind,
            observations: values.len(),
            minimum: *values.first().unwrap_or(&median_value),
            median: median_value,
            maximum: *values.last().unwrap_or(&median_value),
            median_absolute_deviation: mad,
        });
        if values.len() >= options.minimum_distribution_observations {
            for (document_id, value) in observations {
                let scale = if mad > 1.0e-12 {
                    mad
                } else {
                    median_value.abs().max(1.0) * 0.05
                };
                let robust_z = (value - median_value).abs() / scale;
                if robust_z >= options.outlier_mad_multiplier {
                    outliers.push(PortfolioOutlier {
                        document_id,
                        code: "economic_term_outlier".to_owned(),
                        description: format!(
                            "{:?} value {value} differs from portfolio median {median_value}",
                            kind
                        ),
                        severity: (robust_z as f32 / 8.0).clamp(0.0, 1.0),
                        clause_kind: None,
                        economic_term_kind: Some(kind),
                        observed_value: Some(value),
                        portfolio_median: Some(median_value),
                    });
                }
            }
        }
    }
    economic_distributions.sort_unstable_by_key(|entry| entry.kind);

    for document in documents {
        let kinds = document
            .analysis
            .clauses
            .iter()
            .map(primary_kind)
            .collect::<BTreeSet<_>>();
        for prevalence in &clause_prevalence {
            if prevalence.prevalence >= options.common_clause_prevalence
                && !kinds.contains(&prevalence.kind)
            {
                outliers.push(PortfolioOutlier {
                    document_id: document.document_id.clone(),
                    code: "missing_common_clause".to_owned(),
                    description: format!(
                        "missing {:?}, present in {:.1}% of portfolio documents",
                        prevalence.kind,
                        prevalence.prevalence * 100.0
                    ),
                    severity: (0.45 + 0.55 * prevalence.prevalence).clamp(0.0, 1.0),
                    clause_kind: Some(prevalence.kind),
                    economic_term_kind: None,
                    observed_value: None,
                    portfolio_median: None,
                });
            }
            if prevalence.prevalence <= options.rare_clause_prevalence
                && kinds.contains(&prevalence.kind)
            {
                outliers.push(PortfolioOutlier {
                    document_id: document.document_id.clone(),
                    code: "rare_clause".to_owned(),
                    description: format!(
                        "contains rare {:?}, present in {:.1}% of portfolio documents",
                        prevalence.kind,
                        prevalence.prevalence * 100.0
                    ),
                    severity: (0.75 - 0.50 * prevalence.prevalence).clamp(0.0, 1.0),
                    clause_kind: Some(prevalence.kind),
                    economic_term_kind: None,
                    observed_value: None,
                    portfolio_median: None,
                });
            }
        }
    }
    outliers.sort_unstable_by(|left, right| {
        right
            .severity
            .total_cmp(&left.severity)
            .then_with(|| left.document_id.cmp(&right.document_id))
            .then_with(|| left.code.cmp(&right.code))
    });
    outliers.truncate(options.maximum_outliers);
    Ok(ContractPortfolioBenchmark {
        schema_version: CONTRACT_PORTFOLIO_SCHEMA_VERSION,
        documents: documents.len(),
        profiles: profile_counts,
        clause_prevalence,
        economic_distributions,
        obligation_modalities,
        outliers,
    })
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClauseChangeKind, ContractComparisonOptions, ContractPortfolioOptions, EconomicChangeKind,
        PortfolioDocumentAnalysis, benchmark_contract_portfolio, compare_contracts,
    };
    use crate::{ContractAnalysisOptions, ContractProfile, analyze_contract};

    #[test]
    fn detects_material_lease_economic_and_clause_changes() {
        let old = analyze_contract(
            "ARTICLE 1. BASE RENT\nTenant shall pay Base Rent of $100,000 per year.\n\nARTICLE 2. TERM\nThe Term is 5 years.",
            ContractProfile::RetailLease,
            &ContractAnalysisOptions::default(),
        )
        .expect("old");
        let new = analyze_contract(
            "ARTICLE 1. BASE RENT\nTenant shall pay Base Rent of $125,000 per year, increasing by 4% annually.\n\nARTICLE 2. TERM\nThe Term is 10 years.\n\nARTICLE 3. EXCLUSIVITY\nLandlord shall not lease space to a competing tenant.",
            ContractProfile::RetailLease,
            &ContractAnalysisOptions::default(),
        )
        .expect("new");
        let comparison = compare_contracts(&old, &new, &ContractComparisonOptions::default())
            .expect("comparison");
        assert!(
            comparison
                .clause_changes
                .iter()
                .any(|change| change.kind == ClauseChangeKind::Added)
        );
        assert!(
            comparison
                .economic_term_changes
                .iter()
                .any(|change| matches!(
                    change.kind,
                    EconomicChangeKind::Increased | EconomicChangeKind::Added
                ))
        );
        assert!(!comparison.alerts.is_empty());
    }

    #[test]
    fn benchmarks_clause_prevalence_and_economic_outliers() {
        let analyses = [
            ("a", "$100,000"),
            ("b", "$102,000"),
            ("c", "$101,000"),
            ("d", "$99,000"),
            ("e", "$500,000"),
        ]
        .into_iter()
        .map(|(id, rent)| PortfolioDocumentAnalysis {
            document_id: id.to_owned(),
            analysis: analyze_contract(
                &format!("ARTICLE 1. BASE RENT\nTenant shall pay Base Rent of {rent} per year.\n\nARTICLE 2. TERM\nThe Term is 5 years."),
                ContractProfile::RetailLease,
                &ContractAnalysisOptions::default(),
            )
            .expect("analysis"),
        })
        .collect::<Vec<_>>();
        let benchmark =
            benchmark_contract_portfolio(&analyses, &ContractPortfolioOptions::default())
                .expect("benchmark");
        assert_eq!(benchmark.documents, 5);
        assert!(
            benchmark
                .outliers
                .iter()
                .any(|outlier| outlier.document_id == "e")
        );
    }
}
