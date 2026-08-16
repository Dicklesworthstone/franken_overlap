use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::{
    Feature, Fingerprint, FoError, Index, Result, SearchOptions, SearchResult, normalize,
    qgram_hashes, winnow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextDomain {
    General,
    SecFiling,
    Contract,
    Ocr,
    SourceCode,
}

impl Default for TextDomain {
    fn default() -> Self {
        Self::General
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DomainFeaturePolicy {
    pub maximum_document_frequency_fraction: f32,
    pub minimum_feature_idf: f32,
    pub maximum_query_posting_pairs: u64,
    pub minimum_informative_feature_fraction: f32,
    pub minimum_informative_occurrences: usize,
    pub allow_direct_fallback_on_thin_evidence: bool,
}

impl DomainFeaturePolicy {
    #[must_use]
    pub const fn for_domain(domain: TextDomain) -> Self {
        match domain {
            TextDomain::General => Self {
                maximum_document_frequency_fraction: 1.0,
                minimum_feature_idf: 0.0,
                maximum_query_posting_pairs: u64::MAX,
                minimum_informative_feature_fraction: 0.0,
                minimum_informative_occurrences: 2,
                allow_direct_fallback_on_thin_evidence: true,
            },
            TextDomain::SecFiling => Self {
                maximum_document_frequency_fraction: 0.30,
                minimum_feature_idf: 1.20,
                maximum_query_posting_pairs: 5_000_000,
                minimum_informative_feature_fraction: 0.12,
                minimum_informative_occurrences: 3,
                allow_direct_fallback_on_thin_evidence: false,
            },
            TextDomain::Contract => Self {
                maximum_document_frequency_fraction: 0.40,
                minimum_feature_idf: 1.10,
                maximum_query_posting_pairs: 8_000_000,
                minimum_informative_feature_fraction: 0.10,
                minimum_informative_occurrences: 3,
                allow_direct_fallback_on_thin_evidence: false,
            },
            TextDomain::Ocr => Self {
                maximum_document_frequency_fraction: 0.80,
                minimum_feature_idf: 1.0,
                maximum_query_posting_pairs: 12_000_000,
                minimum_informative_feature_fraction: 0.05,
                minimum_informative_occurrences: 2,
                allow_direct_fallback_on_thin_evidence: true,
            },
            TextDomain::SourceCode => Self {
                maximum_document_frequency_fraction: 0.55,
                minimum_feature_idf: 1.05,
                maximum_query_posting_pairs: 10_000_000,
                minimum_informative_feature_fraction: 0.08,
                minimum_informative_occurrences: 3,
                allow_direct_fallback_on_thin_evidence: false,
            },
        }
    }

    pub fn validate(self) -> Result<Self> {
        if !self.maximum_document_frequency_fraction.is_finite()
            || self.maximum_document_frequency_fraction <= 0.0
            || self.maximum_document_frequency_fraction > 1.0
            || !self.minimum_feature_idf.is_finite()
            || self.minimum_feature_idf < 0.0
            || !self.minimum_informative_feature_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.minimum_informative_feature_fraction)
            || self.maximum_query_posting_pairs == 0
            || self.minimum_informative_occurrences == 0
        {
            return Err(FoError::InvalidConfig(
                "domain feature policy contains an invalid threshold or work limit".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DomainSearchOptions {
    pub domain: TextDomain,
    pub policy: DomainFeaturePolicy,
    pub search: SearchOptions,
}

impl Default for DomainSearchOptions {
    fn default() -> Self {
        Self::for_domain(TextDomain::General)
    }
}

impl DomainSearchOptions {
    #[must_use]
    pub fn for_domain(domain: TextDomain) -> Self {
        Self {
            domain,
            policy: DomainFeaturePolicy::for_domain(domain),
            search: SearchOptions::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.policy.validate()?;
        self.search.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainSearchStatus {
    Executed,
    ShortQueryDirectFallback,
    ThinEvidenceDirectFallback,
    InsufficientInformativeEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainQueryAnalysis {
    pub normalized_query_tokens: usize,
    pub selected_feature_occurrences: usize,
    pub selected_distinct_features: usize,
    pub missing_feature_occurrences: usize,
    pub retained_feature_occurrences: usize,
    pub retained_distinct_features: usize,
    pub suppressed_by_posting_cap_occurrences: usize,
    pub suppressed_by_document_frequency_occurrences: usize,
    pub suppressed_by_idf_occurrences: usize,
    pub suppressed_by_work_budget_occurrences: usize,
    pub predicted_posting_pairs_before_policy: u64,
    pub predicted_posting_pairs_after_policy: u64,
    pub informative_feature_fraction: f32,
    pub mean_retained_idf: f32,
    pub maximum_retained_document_frequency_fraction: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainSearchReport {
    pub domain: TextDomain,
    pub policy: DomainFeaturePolicy,
    pub status: DomainSearchStatus,
    pub analysis: DomainQueryAnalysis,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone)]
struct FeatureCandidate {
    fingerprint: Fingerprint,
    occurrences: usize,
    posting_count: usize,
    posting_pairs: u64,
    idf: f32,
    document_frequency_fraction: f32,
}

impl Index {
    /// Search after suppressing corpus-wide boilerplate and enforcing one explicit
    /// query-level posting-pair budget.
    ///
    /// The underlying scorer and exact verifier are unchanged. This method builds
    /// a query-specific view of the immutable index containing only informative
    /// fingerprints, then invokes the ordinary search engine on that view.
    pub fn search_domain(
        &self,
        specimen: &str,
        options: &DomainSearchOptions,
    ) -> Result<DomainSearchReport> {
        options.validate()?;
        let query = normalize(specimen, &self.config.normalization);
        if query.is_empty() {
            return Err(FoError::EmptySpecimen);
        }
        if query.len() < self.config.qgram_size {
            let results = if options.policy.allow_direct_fallback_on_thin_evidence {
                self.search(specimen, &options.search)?
            } else {
                Vec::new()
            };
            return Ok(DomainSearchReport {
                domain: options.domain,
                policy: options.policy,
                status: if options.policy.allow_direct_fallback_on_thin_evidence {
                    DomainSearchStatus::ShortQueryDirectFallback
                } else {
                    DomainSearchStatus::InsufficientInformativeEvidence
                },
                analysis: empty_analysis(query.len()),
                results,
            });
        }

        let selected = winnow(
            &qgram_hashes(&query.tokens, self.config.qgram_size)?,
            self.config.winnow_window,
        );
        let selected_occurrences = selected.len();
        let grouped = group_feature_occurrences(&selected);
        let selected_distinct_features = grouped.len();
        let document_count = self.documents.len().max(1);
        let effective_max_df = options
            .policy
            .maximum_document_frequency_fraction
            .min(options.search.maximum_document_frequency_fraction);
        let effective_min_idf = options
            .policy
            .minimum_feature_idf
            .max(options.search.minimum_feature_idf);
        let effective_pair_budget = options
            .policy
            .maximum_query_posting_pairs
            .min(options.search.maximum_query_posting_pairs);
        let required_fraction = options
            .policy
            .minimum_informative_feature_fraction
            .max(options.search.minimum_informative_feature_fraction);

        let mut missing_feature_occurrences = 0usize;
        let mut suppressed_by_posting_cap_occurrences = 0usize;
        let mut suppressed_by_document_frequency_occurrences = 0usize;
        let mut suppressed_by_idf_occurrences = 0usize;
        let mut predicted_before = 0u64;
        let mut candidates = Vec::new();

        for (fingerprint, occurrences) in grouped {
            let Some(entry) = self.lookup(fingerprint) else {
                missing_feature_occurrences =
                    missing_feature_occurrences.saturating_add(occurrences);
                continue;
            };
            let posting_pairs = saturating_pairs(entry.postings.len(), occurrences);
            predicted_before = predicted_before.saturating_add(posting_pairs);
            if entry.postings.len() > options.search.max_postings_per_feature {
                suppressed_by_posting_cap_occurrences =
                    suppressed_by_posting_cap_occurrences.saturating_add(occurrences);
                continue;
            }
            let df_fraction = entry.document_frequency as f32 / document_count as f32;
            if df_fraction > effective_max_df {
                suppressed_by_document_frequency_occurrences =
                    suppressed_by_document_frequency_occurrences.saturating_add(occurrences);
                continue;
            }
            let idf = ((document_count as f32 + 1.0)
                / (entry.document_frequency as f32 + 1.0))
                .ln()
                + 1.0;
            if idf < effective_min_idf {
                suppressed_by_idf_occurrences =
                    suppressed_by_idf_occurrences.saturating_add(occurrences);
                continue;
            }
            candidates.push(FeatureCandidate {
                fingerprint,
                occurrences,
                posting_count: entry.postings.len(),
                posting_pairs,
                idf,
                document_frequency_fraction: df_fraction,
            });
        }

        candidates.sort_unstable_by(|left, right| {
            left
                .posting_pairs
                .cmp(&right.posting_pairs)
                .then_with(|| left.posting_count.cmp(&right.posting_count))
                .then_with(|| right.idf.total_cmp(&left.idf))
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
        });

        let mut retained = BTreeSet::new();
        let mut retained_occurrences = 0usize;
        let mut retained_pairs = 0u64;
        let mut retained_idf_weight = 0.0f64;
        let mut maximum_retained_df = 0.0f32;
        let mut suppressed_by_work_budget_occurrences = 0usize;
        for candidate in candidates {
            if retained_pairs.saturating_add(candidate.posting_pairs) > effective_pair_budget {
                suppressed_by_work_budget_occurrences = suppressed_by_work_budget_occurrences
                    .saturating_add(candidate.occurrences);
                continue;
            }
            retained_pairs = retained_pairs.saturating_add(candidate.posting_pairs);
            retained_occurrences = retained_occurrences.saturating_add(candidate.occurrences);
            retained_idf_weight +=
                f64::from(candidate.idf) * candidate.occurrences as f64;
            maximum_retained_df =
                maximum_retained_df.max(candidate.document_frequency_fraction);
            retained.insert(candidate.fingerprint);
        }

        let informative_fraction = retained_occurrences as f32
            / selected_occurrences.max(1) as f32;
        let analysis = DomainQueryAnalysis {
            normalized_query_tokens: query.len(),
            selected_feature_occurrences: selected_occurrences,
            selected_distinct_features,
            missing_feature_occurrences,
            retained_feature_occurrences: retained_occurrences,
            retained_distinct_features: retained.len(),
            suppressed_by_posting_cap_occurrences,
            suppressed_by_document_frequency_occurrences,
            suppressed_by_idf_occurrences,
            suppressed_by_work_budget_occurrences,
            predicted_posting_pairs_before_policy: predicted_before,
            predicted_posting_pairs_after_policy: retained_pairs,
            informative_feature_fraction: informative_fraction,
            mean_retained_idf: if retained_occurrences == 0 {
                0.0
            } else {
                (retained_idf_weight / retained_occurrences as f64) as f32
            },
            maximum_retained_document_frequency_fraction: maximum_retained_df,
        };

        let minimum_occurrences = options
            .policy
            .minimum_informative_occurrences
            .max(options.search.minimum_anchor_hits as usize);
        let thin = retained_occurrences < minimum_occurrences
            || informative_fraction < required_fraction;
        if thin {
            let results = if options.policy.allow_direct_fallback_on_thin_evidence {
                self.search(specimen, &options.search)?
            } else {
                Vec::new()
            };
            return Ok(DomainSearchReport {
                domain: options.domain,
                policy: options.policy,
                status: if options.policy.allow_direct_fallback_on_thin_evidence {
                    DomainSearchStatus::ThinEvidenceDirectFallback
                } else {
                    DomainSearchStatus::InsufficientInformativeEvidence
                },
                analysis,
                results,
            });
        }

        let filtered = Index {
            config: self.config.clone(),
            documents: self.documents.clone(),
            entries: self
                .entries
                .iter()
                .filter(|entry| retained.contains(&entry.fingerprint))
                .cloned()
                .collect(),
        };
        let results = filtered.search(specimen, &options.search)?;
        Ok(DomainSearchReport {
            domain: options.domain,
            policy: options.policy,
            status: DomainSearchStatus::Executed,
            analysis,
            results,
        })
    }
}

fn group_feature_occurrences(features: &[Feature]) -> HashMap<Fingerprint, usize> {
    let mut grouped = HashMap::new();
    for feature in features {
        *grouped.entry(feature.fingerprint).or_insert(0usize) += 1;
    }
    grouped
}

fn saturating_pairs(postings: usize, occurrences: usize) -> u64 {
    let value = (postings as u128).saturating_mul(occurrences as u128);
    value.min(u128::from(u64::MAX)) as u64
}

fn empty_analysis(query_tokens: usize) -> DomainQueryAnalysis {
    DomainQueryAnalysis {
        normalized_query_tokens: query_tokens,
        selected_feature_occurrences: 0,
        selected_distinct_features: 0,
        missing_feature_occurrences: 0,
        retained_feature_occurrences: 0,
        retained_distinct_features: 0,
        suppressed_by_posting_cap_occurrences: 0,
        suppressed_by_document_frequency_occurrences: 0,
        suppressed_by_idf_occurrences: 0,
        suppressed_by_work_budget_occurrences: 0,
        predicted_posting_pairs_before_policy: 0,
        predicted_posting_pairs_after_policy: 0,
        informative_feature_fraction: 0.0,
        mean_retained_idf: 0.0,
        maximum_retained_document_frequency_fraction: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{DomainSearchOptions, DomainSearchStatus, TextDomain};
    use crate::{IndexBuilder, IndexConfig, SearchOptions};

    #[test]
    fn sec_policy_suppresses_corpus_wide_boilerplate() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..12 {
            builder
                .add_document(
                    format!("filing-{index}"),
                    format!(
                        "forward looking statements are subject to risks and uncertainties \
                         common boilerplate repeated in every filing unique-marker-{index} \
                         liquidity covenant maturity disclosure for issuer {index}"
                    ),
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let report = index
            .search_domain(
                "forward looking statements are subject to risks and uncertainties \
                 common boilerplate repeated in every filing unique marker 7 liquidity \
                 covenant maturity disclosure for issuer 7",
                &DomainSearchOptions {
                    search: SearchOptions {
                        minimum_similarity: 0.10,
                        ..SearchOptions::default()
                    },
                    ..DomainSearchOptions::for_domain(TextDomain::SecFiling)
                },
            )
            .expect("search");
        assert_eq!(report.status, DomainSearchStatus::Executed);
        assert!(report.analysis.suppressed_by_document_frequency_occurrences > 0);
        assert!(
            report.analysis.predicted_posting_pairs_after_policy
                < report.analysis.predicted_posting_pairs_before_policy
        );
        assert_eq!(report.results.first().expect("hit").path, "filing-7");
    }

    #[test]
    fn thin_sec_evidence_fails_closed_instead_of_scanning_every_document() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..8 {
            builder
                .add_document(
                    format!("filing-{index}"),
                    "forward looking statements are subject to risks and uncertainties common boilerplate",
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let report = index
            .search_domain(
                "forward looking statements are subject to risks and uncertainties",
                &DomainSearchOptions::for_domain(TextDomain::SecFiling),
            )
            .expect("search");
        assert_eq!(
            report.status,
            DomainSearchStatus::InsufficientInformativeEvidence
        );
        assert!(report.results.is_empty());
    }

    #[test]
    fn general_policy_preserves_direct_fallback() {
        let config = IndexConfig {
            qgram_size: 8,
            ..IndexConfig::default()
        };
        let mut builder = IndexBuilder::new(config).expect("builder");
        builder.add_document("source", "alpha beta gamma").expect("add");
        let report = builder
            .build()
            .expect("index")
            .search_domain("beta", &DomainSearchOptions::for_domain(TextDomain::General))
            .expect("search");
        assert_eq!(report.status, DomainSearchStatus::ShortQueryDirectFallback);
        assert_eq!(report.results.first().expect("hit").path, "source");
    }
}
