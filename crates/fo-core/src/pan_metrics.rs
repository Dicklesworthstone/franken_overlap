use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FoError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PanAnnotation {
    pub this_reference: String,
    pub this_offset: usize,
    pub this_length: usize,
    pub source_reference: String,
    pub source_offset: usize,
    pub source_length: usize,
    pub is_external: bool,
}

impl PanAnnotation {
    pub fn validate(&self) -> Result<()> {
        if self.this_reference.trim().is_empty() || self.this_length == 0 {
            return Err(FoError::InvalidConfig(
                "PAN annotations require a this_reference and positive this_length".to_owned(),
            ));
        }
        self.this_offset
            .checked_add(self.this_length)
            .ok_or_else(|| FoError::InvalidConfig("PAN this span overflows usize".to_owned()))?;
        if self.is_external {
            if self.source_reference.trim().is_empty() || self.source_length == 0 {
                return Err(FoError::InvalidConfig(
                    "external PAN annotations require a source_reference and positive source_length"
                        .to_owned(),
                ));
            }
            self.source_offset
                .checked_add(self.source_length)
                .ok_or_else(|| {
                    FoError::InvalidConfig("PAN source span overflows usize".to_owned())
                })?;
        }
        Ok(())
    }

    #[must_use]
    pub fn this_end(&self) -> usize {
        self.this_offset.saturating_add(self.this_length)
    }

    #[must_use]
    pub fn source_end(&self) -> usize {
        self.source_offset.saturating_add(self.source_length)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanEvaluationReport {
    pub cases: usize,
    pub detections: usize,
    pub macro_recall: f64,
    pub macro_precision: f64,
    pub macro_f1: f64,
    pub micro_recall: f64,
    pub micro_precision: f64,
    pub micro_f1: f64,
    pub granularity: f64,
    pub macro_plagdet: f64,
    pub micro_plagdet: f64,
}

pub fn pan_evaluate(
    cases: &[PanAnnotation],
    detections: &[PanAnnotation],
) -> Result<PanEvaluationReport> {
    for annotation in cases.iter().chain(detections) {
        annotation.validate()?;
    }

    let macro_recall_score = macro_recall(cases, detections);
    let macro_precision = macro_recall(detections, cases);
    let (micro_recall, micro_precision) = micro_recall_and_precision(cases, detections);
    let granularity = granularity(cases, detections);
    let macro_f1 = harmonic_mean(macro_recall_score, macro_precision);
    let micro_f1 = harmonic_mean(micro_recall, micro_precision);

    Ok(PanEvaluationReport {
        cases: cases.len(),
        detections: detections.len(),
        macro_recall: macro_recall_score,
        macro_precision,
        macro_f1,
        micro_recall,
        micro_precision,
        micro_f1,
        granularity,
        macro_plagdet: plagdet_score(macro_recall_score, macro_precision, granularity),
        micro_plagdet: plagdet_score(micro_recall, micro_precision, granularity),
    })
}

#[must_use]
pub fn plagdet_score(recall: f64, precision: f64, granularity: f64) -> f64 {
    if (recall == 0.0 && precision == 0.0) || recall < 0.0 || precision < 0.0 || granularity < 1.0 {
        return 0.0;
    }
    harmonic_mean(recall, precision) / (1.0 + granularity).log2()
}

fn macro_recall(cases: &[PanAnnotation], detections: &[PanAnnotation]) -> f64 {
    if cases.is_empty() && detections.is_empty() {
        return 1.0;
    }
    if cases.is_empty() || detections.is_empty() {
        return 0.0;
    }

    let detections_by_document = index_by_this_reference(detections);
    let total = cases
        .iter()
        .map(|case| {
            detections_by_document
                .get(case.this_reference.as_str())
                .map_or(0.0, |candidates| case_recall(case, candidates))
        })
        .sum::<f64>();
    total / cases.len() as f64
}

fn case_recall(case: &PanAnnotation, detections: &[&PanAnnotation]) -> f64 {
    let overlapping = detections
        .iter()
        .copied()
        .filter(|detection| is_overlapping(case, detection))
        .collect::<Vec<_>>();
    if overlapping.is_empty() {
        return 0.0;
    }

    let this_intervals = overlapping
        .iter()
        .filter_map(|detection| {
            clipped_overlap(
                case.this_offset,
                case.this_end(),
                detection.this_offset,
                detection.this_end(),
            )
        })
        .collect::<Vec<_>>();
    let mut detected = interval_union_length(&this_intervals);
    let mut denominator = case.this_length;

    if case.is_external {
        let source_intervals = overlapping
            .iter()
            .filter_map(|detection| {
                clipped_overlap(
                    case.source_offset,
                    case.source_end(),
                    detection.source_offset,
                    detection.source_end(),
                )
            })
            .collect::<Vec<_>>();
        detected = detected.saturating_add(interval_union_length(&source_intervals));
        denominator = denominator.saturating_add(case.source_length);
    }

    detected as f64 / denominator.max(1) as f64
}

fn micro_recall_and_precision(cases: &[PanAnnotation], detections: &[PanAnnotation]) -> (f64, f64) {
    if cases.is_empty() && detections.is_empty() {
        return (1.0, 1.0);
    }
    if cases.is_empty() || detections.is_empty() {
        return (0.0, 0.0);
    }

    let plagiarized = count_chars(cases);
    let detected = count_chars(detections);
    let true_detections = true_detection_overlaps(cases, detections);
    let detected_plagiarized = count_chars(&true_detections);
    let recall = if plagiarized == 0 {
        0.0
    } else {
        detected_plagiarized as f64 / plagiarized as f64
    };
    let precision = if detected == 0 {
        0.0
    } else {
        detected_plagiarized as f64 / detected as f64
    };
    (recall, precision)
}

fn granularity(cases: &[PanAnnotation], detections: &[PanAnnotation]) -> f64 {
    if detections.is_empty() {
        return 1.0;
    }
    let detections_by_document = index_by_this_reference(detections);
    let counts = cases
        .iter()
        .filter_map(|case| {
            let count = detections_by_document
                .get(case.this_reference.as_str())?
                .iter()
                .filter(|detection| is_overlapping(case, detection))
                .count();
            (count > 0).then_some(count)
        })
        .collect::<Vec<_>>();
    if counts.is_empty() {
        1.0
    } else {
        counts.iter().sum::<usize>() as f64 / counts.len() as f64
    }
}

fn true_detection_overlaps(
    cases: &[PanAnnotation],
    detections: &[PanAnnotation],
) -> Vec<PanAnnotation> {
    let mut overlaps = Vec::new();
    for case in cases {
        for detection in detections {
            if !is_overlapping(case, detection) {
                continue;
            }
            let Some((this_start, this_end)) = clipped_overlap(
                case.this_offset,
                case.this_end(),
                detection.this_offset,
                detection.this_end(),
            ) else {
                continue;
            };
            let external = case.is_external && detection.is_external;
            let (source_start, source_end) = if external {
                let Some(overlap) = clipped_overlap(
                    case.source_offset,
                    case.source_end(),
                    detection.source_offset,
                    detection.source_end(),
                ) else {
                    continue;
                };
                overlap
            } else {
                (0, 0)
            };
            overlaps.push(PanAnnotation {
                this_reference: case.this_reference.clone(),
                this_offset: this_start,
                this_length: this_end - this_start,
                source_reference: if external {
                    case.source_reference.clone()
                } else {
                    String::new()
                },
                source_offset: source_start,
                source_length: source_end - source_start,
                is_external: external,
            });
        }
    }
    overlaps
}

fn count_chars(annotations: &[PanAnnotation]) -> usize {
    count_axis(annotations, false).saturating_add(count_axis(annotations, true))
}

fn count_axis(annotations: &[PanAnnotation], source: bool) -> usize {
    let mut by_reference = BTreeMap::<&str, Vec<(usize, usize)>>::new();
    for annotation in annotations {
        if source && !annotation.is_external {
            continue;
        }
        let (reference, start, end) = if source {
            (
                annotation.source_reference.as_str(),
                annotation.source_offset,
                annotation.source_end(),
            )
        } else {
            (
                annotation.this_reference.as_str(),
                annotation.this_offset,
                annotation.this_end(),
            )
        };
        by_reference
            .entry(reference)
            .or_default()
            .push((start, end));
    }
    by_reference
        .into_values()
        .map(|intervals| interval_union_length(&intervals))
        .sum()
}

fn index_by_this_reference(annotations: &[PanAnnotation]) -> BTreeMap<&str, Vec<&PanAnnotation>> {
    let mut index = BTreeMap::<&str, Vec<&PanAnnotation>>::new();
    for annotation in annotations {
        index
            .entry(annotation.this_reference.as_str())
            .or_default()
            .push(annotation);
    }
    index
}

fn is_overlapping(left: &PanAnnotation, right: &PanAnnotation) -> bool {
    if left.this_reference != right.this_reference
        || !intervals_overlap(
            left.this_offset,
            left.this_end(),
            right.this_offset,
            right.this_end(),
        )
    {
        return false;
    }
    if left.is_external && right.is_external {
        left.source_reference == right.source_reference
            && intervals_overlap(
                left.source_offset,
                left.source_end(),
                right.source_offset,
                right.source_end(),
            )
    } else {
        true
    }
}

fn intervals_overlap(
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) -> bool {
    right_end > left_start && right_start < left_end
}

fn clipped_overlap(
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) -> Option<(usize, usize)> {
    let start = left_start.max(right_start);
    let end = left_end.min(right_end);
    (start < end).then_some((start, end))
}

fn interval_union_length(intervals: &[(usize, usize)]) -> usize {
    let mut intervals = intervals
        .iter()
        .copied()
        .filter(|(start, end)| start < end)
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let Some((mut start, mut end)) = intervals.first().copied() else {
        return 0;
    };
    let mut total = 0usize;
    for (next_start, next_end) in intervals.into_iter().skip(1) {
        if next_start <= end {
            end = end.max(next_end);
        } else {
            total = total.saturating_add(end - start);
            start = next_start;
            end = next_end;
        }
    }
    total.saturating_add(end - start)
}

fn harmonic_mean(left: f64, right: f64) -> f64 {
    if left + right == 0.0 {
        0.0
    } else {
        2.0 * left * right / (left + right)
    }
}

#[cfg(test)]
mod tests {
    use super::{PanAnnotation, pan_evaluate};

    fn annotation(
        this_offset: usize,
        this_length: usize,
        source_offset: usize,
        source_length: usize,
    ) -> PanAnnotation {
        PanAnnotation {
            this_reference: "suspicious.txt".to_owned(),
            this_offset,
            this_length,
            source_reference: "source.txt".to_owned(),
            source_offset,
            source_length,
            is_external: true,
        }
    }

    #[test]
    fn exact_detection_is_perfect() {
        let case = annotation(10, 100, 20, 120);
        let detection = case.clone();
        let report = pan_evaluate(&[case], &[detection]).expect("report");
        assert_eq!(report.macro_recall, 1.0);
        assert_eq!(report.macro_precision, 1.0);
        assert_eq!(report.micro_recall, 1.0);
        assert_eq!(report.micro_precision, 1.0);
        assert_eq!(report.granularity, 1.0);
        assert_eq!(report.macro_plagdet, 1.0);
    }

    #[test]
    fn paired_half_detection_has_half_recall_and_full_precision() {
        let case = annotation(0, 100, 0, 100);
        let detection = annotation(0, 50, 0, 50);
        let report = pan_evaluate(&[case], &[detection]).expect("report");
        assert!((report.macro_recall - 0.5).abs() < 1.0e-12);
        assert!((report.macro_precision - 1.0).abs() < 1.0e-12);
        assert!((report.micro_recall - 0.5).abs() < 1.0e-12);
        assert!((report.micro_precision - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn split_detection_is_penalized_by_granularity() {
        let case = annotation(0, 100, 0, 100);
        let detections = [annotation(0, 50, 0, 50), annotation(50, 50, 50, 50)];
        let report = pan_evaluate(&[case], &detections).expect("report");
        assert_eq!(report.macro_recall, 1.0);
        assert_eq!(report.macro_precision, 1.0);
        assert_eq!(report.granularity, 2.0);
        assert!((report.macro_plagdet - 1.0 / 3.0f64.log2()).abs() < 1.0e-12);
    }

    #[test]
    fn wrong_source_reference_is_not_a_true_detection() {
        let case = annotation(0, 100, 0, 100);
        let mut detection = annotation(0, 100, 0, 100);
        detection.source_reference = "wrong.txt".to_owned();
        let report = pan_evaluate(&[case], &[detection]).expect("report");
        assert_eq!(report.macro_recall, 0.0);
        assert_eq!(report.macro_precision, 0.0);
    }

    #[test]
    fn overlapping_detections_do_not_double_count_micro_coverage() {
        let case = annotation(0, 100, 0, 100);
        let detections = [annotation(0, 75, 0, 75), annotation(25, 75, 25, 75)];
        let report = pan_evaluate(&[case], &detections).expect("report");
        assert_eq!(report.micro_recall, 1.0);
        assert_eq!(report.micro_precision, 1.0);
        assert_eq!(report.granularity, 2.0);
    }
}
