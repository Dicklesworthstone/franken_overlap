use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::{
    Document, Feature, Fingerprint, FoError, Index, IndexConfig, IndexEntry, Posting, Result,
    SearchOptions, SearchResult, normalize, qgram_hashes, winnow,
};

pub const PREPARED_QUERY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedQueryFeature {
    pub fingerprint: Fingerprint,
    pub positions: Vec<u32>,
    pub posting_count: usize,
    pub document_frequency: usize,
    pub idf: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedOverlapQuery {
    pub schema_version: u32,
    pub index_config: IndexConfig,
    pub specimen: String,
    pub normalized_tokens: usize,
    pub selected_feature_occurrences: usize,
    pub matching_feature_occurrences: usize,
    pub missing_feature_occurrences: usize,
    pub features: Vec<PreparedQueryFeature>,
}

impl PreparedOverlapQuery {
    pub fn validate_for(&self, index: &Index) -> Result<()> {
        if self.schema_version != PREPARED_QUERY_SCHEMA_VERSION {
            return Err(FoError::InvalidConfig(format!(
                "unsupported prepared query schema {}",
                self.schema_version
            )));
        }
        if self.index_config != index.config {
            return Err(FoError::InvalidConfig(
                "prepared query index configuration differs from the loaded index".to_owned(),
            ));
        }
        if self.specimen.trim().is_empty() || self.normalized_tokens == 0 {
            return Err(FoError::EmptySpecimen);
        }
        if self.selected_feature_occurrences
            != self
                .matching_feature_occurrences
                .saturating_add(self.missing_feature_occurrences)
        {
            return Err(FoError::InvalidConfig(
                "prepared query feature occurrence accounting is inconsistent".to_owned(),
            ));
        }
        let mut previous = None;
        let mut observed_occurrences = 0usize;
        for feature in &self.features {
            if feature.positions.is_empty()
                || feature.positions.windows(2).any(|window| window[0] >= window[1])
                || previous.is_some_and(|value| value >= feature.fingerprint)
                || !feature.idf.is_finite()
                || feature.idf < 0.0
                || feature.document_frequency > index.documents.len()
            {
                return Err(FoError::InvalidConfig(
                    "prepared query contains invalid, duplicate, or unsorted feature evidence"
                        .to_owned(),
                ));
            }
            if feature.posting_count == 0 || feature.document_frequency == 0 {
                return Err(FoError::InvalidConfig(
                    "prepared matching features must have postings and document frequency"
                        .to_owned(),
                ));
            }
            observed_occurrences = observed_occurrences.saturating_add(feature.positions.len());
            previous = Some(feature.fingerprint);
        }
        if observed_occurrences != self.matching_feature_occurrences {
            return Err(FoError::InvalidConfig(
                "prepared query matching occurrence count disagrees with feature positions"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentFirstOptions {
    pub maximum_documents: usize,
    pub minimum_document_score_fraction: f32,
    pub minimum_distinct_features: usize,
    pub maximum_postings_per_feature: usize,
    pub maximum_posting_pairs: u64,
    pub maximum_selected_document_fraction: f32,
    pub fallback_to_full_index: bool,
}

impl Default for DocumentFirstOptions {
    fn default() -> Self {
        Self {
            maximum_documents: 128,
            minimum_document_score_fraction: 0.08,
            minimum_distinct_features: 2,
            maximum_postings_per_feature: 50_000,
            maximum_posting_pairs: 10_000_000,
            maximum_selected_document_fraction: 0.50,
            fallback_to_full_index: true,
        }
    }
}

impl DocumentFirstOptions {
    pub fn validate(&self) -> Result<()> {
        if self.maximum_documents == 0
            || self.minimum_distinct_features == 0
            || self.maximum_postings_per_feature == 0
            || self.maximum_posting_pairs == 0
        {
            return Err(FoError::InvalidConfig(
                "document-first count and work limits must be positive".to_owned(),
            ));
        }
        for (name, value) in [
            (
                "minimum_document_score_fraction",
                self.minimum_document_score_fraction,
            ),
            (
                "maximum_selected_document_fraction",
                self.maximum_selected_document_fraction,
            ),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "document-first {name} must lie in [0, 1]"
                )));
            }
        }
        if self.maximum_selected_document_fraction <= 0.0 {
            return Err(FoError::InvalidConfig(
                "maximum_selected_document_fraction must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFirstStatus {
    Filtered,
    FullIndexFallbackThinEvidence,
    FullIndexFallbackBroadCandidateSet,
    NoCandidates,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentCandidate {
    pub document_id: u32,
    pub path: String,
    pub score: f32,
    pub distinct_features: usize,
    pub matched_query_feature_occurrences: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentFirstSearchReport {
    pub status: DocumentFirstStatus,
    pub corpus_documents: usize,
    pub selected_documents: usize,
    pub selected_fraction: f32,
    pub prepared_feature_occurrences: usize,
    pub retained_distinct_features: usize,
    pub suppressed_features_by_posting_cap: usize,
    pub suppressed_features_by_work_budget: usize,
    pub postings_scanned: usize,
    pub posting_pairs: u64,
    pub document_candidates: Vec<DocumentCandidate>,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Default)]
struct CandidateAccumulator {
    score: f64,
    distinct_features: usize,
    matched_occurrences: usize,
}

impl Index {
    pub fn prepare_overlap_query(&self, specimen: &str) -> Result<PreparedOverlapQuery> {
        let normalized = normalize(specimen, &self.config.normalization);
        if normalized.is_empty() {
            return Err(FoError::EmptySpecimen);
        }
        let selected = if normalized.len() < self.config.qgram_size {
            Vec::new()
        } else {
            winnow(
                &qgram_hashes(&normalized.tokens, self.config.qgram_size)?,
                self.config.winnow_window,
            )
        };
        let selected_feature_occurrences = selected.len();
        let grouped = group_positions(&selected);
        let document_count = self.documents.len().max(1);
        let mut matching_feature_occurrences = 0usize;
        let mut features = Vec::new();
        for (fingerprint, positions) in grouped {
            let Some(entry) = self.lookup(fingerprint) else {
                continue;
            };
            matching_feature_occurrences =
                matching_feature_occurrences.saturating_add(positions.len());
            let idf = ((document_count as f32 + 1.0)
                / (entry.document_frequency as f32 + 1.0))
                .ln()
                + 1.0;
            features.push(PreparedQueryFeature {
                fingerprint,
                positions,
                posting_count: entry.postings.len(),
                document_frequency: entry.document_frequency as usize,
                idf,
            });
        }
        let prepared = PreparedOverlapQuery {
            schema_version: PREPARED_QUERY_SCHEMA_VERSION,
            index_config: self.config.clone(),
            specimen: specimen.to_owned(),
            normalized_tokens: normalized.len(),
            selected_feature_occurrences,
            matching_feature_occurrences,
            missing_feature_occurrences: selected_feature_occurrences
                .saturating_sub(matching_feature_occurrences),
            features,
        };
        prepared.validate_for(self)?;
        Ok(prepared)
    }

    pub fn search_document_first(
        &self,
        prepared: &PreparedOverlapQuery,
        document_options: &DocumentFirstOptions,
        search_options: &SearchOptions,
    ) -> Result<DocumentFirstSearchReport> {
        prepared.validate_for(self)?;
        document_options.validate()?;
        search_options.validate()?;
        if self.documents.is_empty() {
            return Ok(empty_report(DocumentFirstStatus::NoCandidates, prepared));
        }

        let mut ordered_features = prepared.features.iter().collect::<Vec<_>>();
        ordered_features.sort_unstable_by(|left, right| {
            left
                .posting_count
                .saturating_mul(left.positions.len())
                .cmp(&right.posting_count.saturating_mul(right.positions.len()))
                .then_with(|| left.posting_count.cmp(&right.posting_count))
                .then_with(|| right.idf.total_cmp(&left.idf))
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
        });

        let mut accumulators = HashMap::<u32, CandidateAccumulator>::new();
        let mut retained_fingerprints = BTreeSet::new();
        let mut retained_distinct_features = 0usize;
        let mut suppressed_features_by_posting_cap = 0usize;
        let mut suppressed_features_by_work_budget = 0usize;
        let mut postings_scanned = 0usize;
        let mut posting_pairs = 0u64;

        for feature in ordered_features {
            if feature.posting_count > document_options.maximum_postings_per_feature {
                suppressed_features_by_posting_cap += 1;
                continue;
            }
            let feature_pairs = saturating_pairs(feature.posting_count, feature.positions.len());
            if posting_pairs.saturating_add(feature_pairs) > document_options.maximum_posting_pairs {
                suppressed_features_by_work_budget += 1;
                continue;
            }
            let Some(entry) = self.lookup(feature.fingerprint) else {
                continue;
            };
            posting_pairs = posting_pairs.saturating_add(feature_pairs);
            postings_scanned = postings_scanned.saturating_add(entry.postings.len());
            retained_distinct_features += 1;
            retained_fingerprints.insert(feature.fingerprint);
            let multiplicity = (feature.positions.len() as f64).ln_1p() + 1.0;
            let contribution = f64::from(feature.idf) * multiplicity;
            let mut previous_document = None;
            for posting in &entry.postings {
                if previous_document == Some(posting.document_id) {
                    continue;
                }
                previous_document = Some(posting.document_id);
                let accumulator = accumulators.entry(posting.document_id).or_default();
                accumulator.score += contribution;
                accumulator.distinct_features += 1;
                accumulator.matched_occurrences = accumulator
                    .matched_occurrences
                    .saturating_add(feature.positions.len());
            }
        }

        let mut candidates = accumulators
            .into_iter()
            .filter(|(_, value)| {
                value.distinct_features >= document_options.minimum_distinct_features
            })
            .filter_map(|(document_id, value)| {
                self.document(document_id).map(|document| DocumentCandidate {
                    document_id,
                    path: document.path.clone(),
                    score: value.score.min(f64::from(f32::MAX)) as f32,
                    distinct_features: value.distinct_features,
                    matched_query_feature_occurrences: value.matched_occurrences,
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.distinct_features.cmp(&left.distinct_features))
                .then_with(|| right.matched_query_feature_occurrences.cmp(&left.matched_query_feature_occurrences))
                .then_with(|| left.document_id.cmp(&right.document_id))
        });
        let best_score = candidates.first().map_or(0.0, |candidate| candidate.score);
        let score_floor = best_score * document_options.minimum_document_score_fraction;
        candidates.retain(|candidate| candidate.score >= score_floor);
        candidates.truncate(document_options.maximum_documents);

        if candidates.is_empty() {
            return if document_options.fallback_to_full_index {
                full_fallback(
                    self,
                    prepared,
                    search_options,
                    DocumentFirstStatus::FullIndexFallbackThinEvidence,
                    retained_distinct_features,
                    suppressed_features_by_posting_cap,
                    suppressed_features_by_work_budget,
                    postings_scanned,
                    posting_pairs,
                    candidates,
                )
            } else {
                Ok(DocumentFirstSearchReport {
                    status: DocumentFirstStatus::NoCandidates,
                    corpus_documents: self.documents.len(),
                    selected_documents: 0,
                    selected_fraction: 0.0,
                    prepared_feature_occurrences: prepared.selected_feature_occurrences,
                    retained_distinct_features,
                    suppressed_features_by_posting_cap,
                    suppressed_features_by_work_budget,
                    postings_scanned,
                    posting_pairs,
                    document_candidates: candidates,
                    results: Vec::new(),
                })
            };
        }

        let selected_fraction = candidates.len() as f32 / self.documents.len() as f32;
        if selected_fraction > document_options.maximum_selected_document_fraction
            && document_options.fallback_to_full_index
        {
            return full_fallback(
                self,
                prepared,
                search_options,
                DocumentFirstStatus::FullIndexFallbackBroadCandidateSet,
                retained_distinct_features,
                suppressed_features_by_posting_cap,
                suppressed_features_by_work_budget,
                postings_scanned,
                posting_pairs,
                candidates,
            );
        }

        let (projected, local_to_original) =
            projected_index(self, &candidates, &retained_fingerprints)?;
        let mut results = projected.search(&prepared.specimen, search_options)?;
        for result in &mut results {
            let local = usize::try_from(result.document_id).map_err(|_| {
                FoError::InvalidIndex("projected result document id exceeds usize".to_owned())
            })?;
            result.document_id = *local_to_original.get(local).ok_or_else(|| {
                FoError::InvalidIndex(
                    "projected result references a missing original document".to_owned(),
                )
            })?;
        }
        Ok(DocumentFirstSearchReport {
            status: DocumentFirstStatus::Filtered,
            corpus_documents: self.documents.len(),
            selected_documents: candidates.len(),
            selected_fraction,
            prepared_feature_occurrences: prepared.selected_feature_occurrences,
            retained_distinct_features,
            suppressed_features_by_posting_cap,
            suppressed_features_by_work_budget,
            postings_scanned,
            posting_pairs,
            document_candidates: candidates,
            results,
        })
    }
}

fn projected_index(
    index: &Index,
    candidates: &[DocumentCandidate],
    retained_fingerprints: &BTreeSet<Fingerprint>,
) -> Result<(Index, Vec<u32>)> {
    let mut original_ids = candidates
        .iter()
        .map(|candidate| candidate.document_id)
        .collect::<Vec<_>>();
    original_ids.sort_unstable();
    original_ids.dedup();
    let mut old_to_local = vec![None; index.documents.len()];
    let mut documents = Vec::with_capacity(original_ids.len());
    for &original_id in &original_ids {
        let original = index.document(original_id).ok_or_else(|| {
            FoError::InvalidIndex(format!(
                "document-first candidate references missing document {original_id}"
            ))
        })?;
        let local_id = u32::try_from(documents.len()).map_err(|_| FoError::TooManyDocuments)?;
        old_to_local[original_id as usize] = Some(local_id);
        documents.push(Document {
            id: local_id,
            path: original.path.clone(),
            normalized: original.normalized.clone(),
        });
    }

    let mut entries = Vec::new();
    for fingerprint in retained_fingerprints {
        let Some(entry) = index.lookup(*fingerprint) else {
            continue;
        };
        let mut postings = Vec::new();
        let mut document_frequency = 0u32;
        let mut previous_document = None;
        for posting in &entry.postings {
            let Some(local_id) = old_to_local
                .get(posting.document_id as usize)
                .and_then(|value| *value)
            else {
                continue;
            };
            if previous_document != Some(local_id) {
                document_frequency = document_frequency.checked_add(1).ok_or_else(|| {
                    FoError::InvalidIndex(
                        "projected document frequency exceeds u32".to_owned(),
                    )
                })?;
                previous_document = Some(local_id);
            }
            postings.push(Posting {
                document_id: local_id,
                position: posting.position,
            });
        }
        if !postings.is_empty() {
            entries.push(IndexEntry {
                fingerprint: *fingerprint,
                document_frequency,
                postings,
            });
        }
    }
    Ok((
        Index {
            config: index.config.clone(),
            documents,
            entries,
        },
        original_ids,
    ))
}

#[allow(clippy::too_many_arguments)]
fn full_fallback(
    index: &Index,
    prepared: &PreparedOverlapQuery,
    search_options: &SearchOptions,
    status: DocumentFirstStatus,
    retained_distinct_features: usize,
    suppressed_features_by_posting_cap: usize,
    suppressed_features_by_work_budget: usize,
    postings_scanned: usize,
    posting_pairs: u64,
    candidates: Vec<DocumentCandidate>,
) -> Result<DocumentFirstSearchReport> {
    Ok(DocumentFirstSearchReport {
        status,
        corpus_documents: index.documents.len(),
        selected_documents: index.documents.len(),
        selected_fraction: 1.0,
        prepared_feature_occurrences: prepared.selected_feature_occurrences,
        retained_distinct_features,
        suppressed_features_by_posting_cap,
        suppressed_features_by_work_budget,
        postings_scanned,
        posting_pairs,
        document_candidates: candidates,
        results: index.search(&prepared.specimen, search_options)?,
    })
}

fn empty_report(
    status: DocumentFirstStatus,
    prepared: &PreparedOverlapQuery,
) -> DocumentFirstSearchReport {
    DocumentFirstSearchReport {
        status,
        corpus_documents: 0,
        selected_documents: 0,
        selected_fraction: 0.0,
        prepared_feature_occurrences: prepared.selected_feature_occurrences,
        retained_distinct_features: 0,
        suppressed_features_by_posting_cap: 0,
        suppressed_features_by_work_budget: 0,
        postings_scanned: 0,
        posting_pairs: 0,
        document_candidates: Vec::new(),
        results: Vec::new(),
    }
}

fn group_positions(features: &[Feature]) -> BTreeMap<Fingerprint, Vec<u32>> {
    let mut grouped = BTreeMap::new();
    for feature in features {
        grouped
            .entry(feature.fingerprint)
            .or_insert_with(Vec::new)
            .push(feature.position);
    }
    grouped
}

fn saturating_pairs(left: usize, right: usize) -> u64 {
    let value = (left as u128).saturating_mul(right as u128);
    value.min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        DocumentFirstOptions, DocumentFirstStatus, PreparedOverlapQuery,
        PREPARED_QUERY_SCHEMA_VERSION,
    };
    use crate::{IndexBuilder, IndexConfig, SearchOptions};

    #[test]
    fn document_first_search_prunes_unrelated_documents() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..64 {
            let body = if index == 37 {
                "copper shutters opened before dawn while the observatory calibrated every detector and preserved the raw measurements"
                    .to_owned()
            } else {
                format!(
                    "unrelated archive document {index} about gardens railways kitchens and weather patterns"
                )
            };
            builder
                .add_document(format!("document-{index}"), body)
                .expect("document");
        }
        let index = builder.build().expect("index");
        let prepared = index
            .prepare_overlap_query(
                "the copper shutters opened before dawn and the observatory calibrated every detector while preserving raw measurements",
            )
            .expect("prepare");
        let report = index
            .search_document_first(
                &prepared,
                &DocumentFirstOptions {
                    maximum_documents: 8,
                    minimum_document_score_fraction: 0.05,
                    maximum_selected_document_fraction: 0.25,
                    ..DocumentFirstOptions::default()
                },
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_query_coverage: 0.0,
                    minimum_source_coverage: 0.0,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.status, DocumentFirstStatus::Filtered);
        assert!(report.selected_documents <= 8);
        assert_eq!(report.results.first().expect("result").path, "document-37");
        assert_eq!(report.results.first().expect("result").document_id, 37);
    }

    #[test]
    fn prepared_query_round_trip_preserves_contract() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source",
                "alpha beta gamma delta epsilon zeta eta theta iota kappa",
            )
            .expect("source");
        let index = builder.build().expect("index");
        let prepared = index
            .prepare_overlap_query("beta gamma delta epsilon zeta eta")
            .expect("prepare");
        let bytes = serde_json::to_vec(&prepared).expect("serialize");
        let decoded = serde_json::from_slice::<PreparedOverlapQuery>(&bytes).expect("decode");
        assert_eq!(decoded.schema_version, PREPARED_QUERY_SCHEMA_VERSION);
        decoded.validate_for(&index).expect("valid");
    }

    #[test]
    fn broad_candidate_set_can_fall_back_to_full_index() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..4 {
            builder
                .add_document(
                    format!("document-{index}"),
                    "the same repeated passage appears across every document in this small corpus",
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let prepared = index
            .prepare_overlap_query("the same repeated passage appears across every document")
            .expect("prepare");
        let report = index
            .search_document_first(
                &prepared,
                &DocumentFirstOptions {
                    maximum_selected_document_fraction: 0.25,
                    ..DocumentFirstOptions::default()
                },
                &SearchOptions {
                    minimum_similarity: 0.0,
                    minimum_query_coverage: 0.0,
                    minimum_source_coverage: 0.0,
                    minimum_matched_tokens: 1,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert!(matches!(
            report.status,
            DocumentFirstStatus::FullIndexFallbackBroadCandidateSet
                | DocumentFirstStatus::FullIndexFallbackThinEvidence
        ));
        assert_eq!(report.selected_documents, 4);
    }
}
