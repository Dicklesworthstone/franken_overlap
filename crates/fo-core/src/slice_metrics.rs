use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    FoError, GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore, Result,
    grouped_evaluation_report,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlicedLabeledScore {
    pub query_id: String,
    pub score: f64,
    pub label: bool,
    #[serde(default)]
    pub facets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SliceKey {
    pub facets: BTreeMap<String, String>,
}

impl SliceKey {
    #[must_use]
    pub fn display_name(&self) -> String {
        self.facets
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" & ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceEvaluationOptions {
    pub grouped: GroupedEvaluationOptions,
    pub minimum_examples: usize,
    pub minimum_queries: usize,
    pub minimum_positives: usize,
    pub minimum_negatives: usize,
    pub maximum_slices: usize,
    pub maximum_intersection_depth: usize,
}

impl Default for SliceEvaluationOptions {
    fn default() -> Self {
        Self {
            grouped: GroupedEvaluationOptions::default(),
            minimum_examples: 20,
            minimum_queries: 2,
            minimum_positives: 2,
            minimum_negatives: 2,
            maximum_slices: 512,
            maximum_intersection_depth: 1,
        }
    }
}

impl SliceEvaluationOptions {
    pub fn validate(&self) -> Result<()> {
        self.grouped.validate()?;
        if self.minimum_examples == 0
            || self.minimum_queries == 0
            || self.minimum_positives == 0
            || self.minimum_negatives == 0
            || self.maximum_slices == 0
        {
            return Err(FoError::InvalidConfig(
                "slice minimum counts and maximum_slices must be positive".to_owned(),
            ));
        }
        if !(1..=3).contains(&self.maximum_intersection_depth) {
            return Err(FoError::InvalidConfig(
                "maximum_intersection_depth must be between 1 and 3".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceMetrics {
    pub key: SliceKey,
    pub examples: usize,
    pub queries: usize,
    pub positives: usize,
    pub negatives: usize,
    pub micro_auprc_delta: f64,
    pub macro_auprc_delta: f64,
    pub mean_reciprocal_rank_delta: f64,
    pub recall_at_1_delta: f64,
    pub report: GroupedEvaluationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceEvaluationReport {
    pub examples: usize,
    pub queries: usize,
    pub candidate_slices: usize,
    pub evaluated_slices: usize,
    pub skipped_slices: usize,
    pub overall: GroupedEvaluationReport,
    pub slices: Vec<SliceMetrics>,
    pub worst_micro_slice: Option<SliceKey>,
    pub worst_macro_slice: Option<SliceKey>,
    pub worst_recall_at_1_slice: Option<SliceKey>,
    pub worst_micro_auprc: Option<f64>,
    pub worst_macro_auprc: Option<f64>,
    pub worst_recall_at_1: Option<f64>,
    pub maximum_micro_auprc_gap: f64,
    pub maximum_macro_auprc_gap: f64,
    pub maximum_recall_at_1_gap: f64,
}

pub fn slice_evaluation_report(
    examples: &[SlicedLabeledScore],
    options: SliceEvaluationOptions,
) -> Result<SliceEvaluationReport> {
    options.validate()?;
    validate_examples(examples)?;
    let overall_records = examples
        .iter()
        .map(grouped_record)
        .collect::<Vec<_>>();
    let overall = grouped_evaluation_report(&overall_records, options.grouped.clone())?;
    let overall_micro = overall.micro.average_precision;
    let overall_macro = overall.macro_average_precision;
    let overall_mrr = overall.mean_reciprocal_rank;
    let overall_recall_at_1 = recall_at_one(&overall);

    let memberships = build_memberships(examples, options.maximum_intersection_depth)?;
    let candidate_slices = memberships.len();
    if candidate_slices > options.maximum_slices {
        return Err(FoError::InvalidConfig(format!(
            "slice expansion produced {candidate_slices} candidates, exceeding maximum_slices {}",
            options.maximum_slices
        )));
    }

    let mut slices = Vec::new();
    for (key, indices) in memberships {
        let positives = indices
            .iter()
            .filter(|&&index| examples[index].label)
            .count();
        let negatives = indices.len().saturating_sub(positives);
        let queries = indices
            .iter()
            .map(|&index| examples[index].query_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        if indices.len() < options.minimum_examples
            || queries < options.minimum_queries
            || positives < options.minimum_positives
            || negatives < options.minimum_negatives
        {
            continue;
        }
        let records = indices
            .iter()
            .map(|&index| grouped_record(&examples[index]))
            .collect::<Vec<_>>();
        let report = grouped_evaluation_report(&records, options.grouped.clone())?;
        slices.push(SliceMetrics {
            key,
            examples: indices.len(),
            queries,
            positives,
            negatives,
            micro_auprc_delta: report.micro.average_precision - overall_micro,
            macro_auprc_delta: report.macro_average_precision - overall_macro,
            mean_reciprocal_rank_delta: report.mean_reciprocal_rank - overall_mrr,
            recall_at_1_delta: recall_at_one(&report) - overall_recall_at_1,
            report,
        });
    }
    slices.sort_unstable_by(|left, right| {
        left.report
            .macro_average_precision
            .total_cmp(&right.report.macro_average_precision)
            .then_with(|| {
                left.report
                    .micro
                    .average_precision
                    .total_cmp(&right.report.micro.average_precision)
            })
            .then_with(|| left.key.cmp(&right.key))
    });

    let worst_micro = slices.iter().min_by(|left, right| {
        left.report
            .micro
            .average_precision
            .total_cmp(&right.report.micro.average_precision)
            .then_with(|| left.key.cmp(&right.key))
    });
    let worst_macro = slices.iter().min_by(|left, right| {
        left.report
            .macro_average_precision
            .total_cmp(&right.report.macro_average_precision)
            .then_with(|| left.key.cmp(&right.key))
    });
    let worst_recall = slices.iter().min_by(|left, right| {
        recall_at_one(&left.report)
            .total_cmp(&recall_at_one(&right.report))
            .then_with(|| left.key.cmp(&right.key))
    });
    let worst_micro_auprc = worst_micro.map(|slice| slice.report.micro.average_precision);
    let worst_macro_auprc = worst_macro.map(|slice| slice.report.macro_average_precision);
    let worst_recall_at_1 = worst_recall.map(|slice| recall_at_one(&slice.report));

    Ok(SliceEvaluationReport {
        examples: examples.len(),
        queries: overall.queries,
        candidate_slices,
        evaluated_slices: slices.len(),
        skipped_slices: candidate_slices.saturating_sub(slices.len()),
        worst_micro_slice: worst_micro.map(|slice| slice.key.clone()),
        worst_macro_slice: worst_macro.map(|slice| slice.key.clone()),
        worst_recall_at_1_slice: worst_recall.map(|slice| slice.key.clone()),
        worst_micro_auprc,
        worst_macro_auprc,
        worst_recall_at_1,
        maximum_micro_auprc_gap: worst_micro_auprc
            .map_or(0.0, |value| (overall_micro - value).max(0.0)),
        maximum_macro_auprc_gap: worst_macro_auprc
            .map_or(0.0, |value| (overall_macro - value).max(0.0)),
        maximum_recall_at_1_gap: worst_recall_at_1
            .map_or(0.0, |value| (overall_recall_at_1 - value).max(0.0)),
        overall,
        slices,
    })
}

fn build_memberships(
    examples: &[SlicedLabeledScore],
    maximum_depth: usize,
) -> Result<BTreeMap<SliceKey, Vec<usize>>> {
    let mut memberships = BTreeMap::<SliceKey, Vec<usize>>::new();
    for (index, example) in examples.iter().enumerate() {
        let facets = example
            .facets
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        for depth in 1..=maximum_depth.min(facets.len()) {
            let mut selected = Vec::with_capacity(depth);
            add_combinations(
                &facets,
                depth,
                0,
                &mut selected,
                index,
                &mut memberships,
            );
        }
    }
    Ok(memberships)
}

fn add_combinations(
    facets: &[(String, String)],
    depth: usize,
    start: usize,
    selected: &mut Vec<(String, String)>,
    example_index: usize,
    memberships: &mut BTreeMap<SliceKey, Vec<usize>>,
) {
    if selected.len() == depth {
        memberships
            .entry(SliceKey {
                facets: selected.iter().cloned().collect(),
            })
            .or_default()
            .push(example_index);
        return;
    }
    let needed = depth - selected.len();
    if facets.len().saturating_sub(start) < needed {
        return;
    }
    for index in start..=facets.len() - needed {
        selected.push(facets[index].clone());
        add_combinations(
            facets,
            depth,
            index + 1,
            selected,
            example_index,
            memberships,
        );
        selected.pop();
    }
}

fn validate_examples(examples: &[SlicedLabeledScore]) -> Result<()> {
    if examples.is_empty() {
        return Err(FoError::InvalidConfig(
            "slice evaluation requires at least one example".to_owned(),
        ));
    }
    let mut positives = 0usize;
    let mut negatives = 0usize;
    for (index, example) in examples.iter().enumerate() {
        if example.query_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(format!(
                "slice example {index} has an empty query_id"
            )));
        }
        if !example.score.is_finite() || !(0.0..=1.0).contains(&example.score) {
            return Err(FoError::InvalidConfig(format!(
                "slice example {index} has score {}; scores must lie in [0, 1]",
                example.score
            )));
        }
        if example.label {
            positives += 1;
        } else {
            negatives += 1;
        }
        for (name, value) in &example.facets {
            if name.trim().is_empty()
                || value.trim().is_empty()
                || name.len() > 256
                || value.len() > 1024
            {
                return Err(FoError::InvalidConfig(format!(
                    "slice example {index} has an invalid facet {name:?}={value:?}"
                )));
            }
        }
    }
    if positives == 0 || negatives == 0 {
        return Err(FoError::InvalidConfig(
            "slice evaluation requires at least one positive and one negative".to_owned(),
        ));
    }
    Ok(())
}

fn grouped_record(example: &SlicedLabeledScore) -> GroupedLabeledScore {
    GroupedLabeledScore {
        query_id: example.query_id.clone(),
        score: example.score,
        label: example.label,
    }
}

fn recall_at_one(report: &GroupedEvaluationReport) -> f64 {
    report
        .recall_at_k
        .iter()
        .find(|metric| metric.k == 1)
        .map_or(0.0, |metric| metric.value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{SliceEvaluationOptions, SlicedLabeledScore, slice_evaluation_report};
    use crate::GroupedEvaluationOptions;

    #[test]
    fn exposes_a_bad_noise_slice_hidden_by_the_overall_stream() {
        let mut examples = Vec::new();
        for query in 0..12 {
            examples.push(example(query, 0.95, true, "clean"));
            examples.push(example(query, 0.10, false, "clean"));
        }
        for query in 12..16 {
            examples.push(example(query, 0.25, true, "ocr"));
            examples.push(example(query, 0.85, false, "ocr"));
        }
        let report = slice_evaluation_report(
            &examples,
            SliceEvaluationOptions {
                grouped: GroupedEvaluationOptions {
                    bootstrap_samples: 0,
                    ..GroupedEvaluationOptions::default()
                },
                minimum_examples: 4,
                minimum_queries: 2,
                minimum_positives: 2,
                minimum_negatives: 2,
                maximum_slices: 32,
                maximum_intersection_depth: 1,
            },
        )
        .expect("report");
        let worst = report.worst_macro_slice.expect("worst");
        assert_eq!(worst.facets.get("noise").map(String::as_str), Some("ocr"));
        assert!(report.maximum_macro_auprc_gap > 0.4);
    }

    #[test]
    fn intersection_slices_are_deterministic() {
        let examples = vec![
            example_with_facets(0, 0.9, true, [("noise", "ocr"), ("length", "short")]),
            example_with_facets(0, 0.2, false, [("noise", "ocr"), ("length", "short")]),
            example_with_facets(1, 0.8, true, [("noise", "ocr"), ("length", "short")]),
            example_with_facets(1, 0.3, false, [("noise", "ocr"), ("length", "short")]),
        ];
        let report = slice_evaluation_report(
            &examples,
            SliceEvaluationOptions {
                grouped: GroupedEvaluationOptions {
                    bootstrap_samples: 0,
                    ..GroupedEvaluationOptions::default()
                },
                minimum_examples: 2,
                minimum_queries: 1,
                minimum_positives: 1,
                minimum_negatives: 1,
                maximum_slices: 32,
                maximum_intersection_depth: 2,
            },
        )
        .expect("report");
        assert!(report.slices.iter().any(|slice| slice.key.facets.len() == 2));
    }

    fn example(query: usize, score: f64, label: bool, noise: &str) -> SlicedLabeledScore {
        example_with_facets(query, score, label, [("noise", noise)])
    }

    fn example_with_facets<const N: usize>(
        query: usize,
        score: f64,
        label: bool,
        facets: [(&str, &str); N],
    ) -> SlicedLabeledScore {
        SlicedLabeledScore {
            query_id: format!("q-{query}"),
            score,
            label,
            facets: facets
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect::<BTreeMap<_, _>>(),
        }
    }
}
