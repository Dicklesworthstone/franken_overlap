use crate::{DomainSearchOptions, DomainSearchReport, Index, Result};

impl Index {
    /// Apply a domain policy without allowing its corpus-frequency thresholds to
    /// suppress every nonempty posting list in a very small candidate corpus.
    ///
    /// For `N` indexed documents, at least a fingerprint occurring in exactly one
    /// document remains eligible. The minimum IDF is also capped at the maximum
    /// IDF attainable by a one-document feature in that corpus. Larger corpora
    /// retain the configured policy unchanged whenever its thresholds are
    /// attainable. Posting-count, work-budget, and thin-evidence rules remain
    /// enforced by `search_domain`.
    pub fn search_domain_adaptive(
        &self,
        specimen: &str,
        options: &DomainSearchOptions,
    ) -> Result<DomainSearchReport> {
        let mut effective = options.clone();
        let document_count = self.documents().len().max(1);
        let minimum_nonzero_fraction = 1.0 / document_count as f32;
        effective.policy.maximum_document_frequency_fraction = effective
            .policy
            .maximum_document_frequency_fraction
            .max(minimum_nonzero_fraction)
            .min(1.0);
        effective.search.maximum_document_frequency_fraction = effective
            .search
            .maximum_document_frequency_fraction
            .max(minimum_nonzero_fraction)
            .min(1.0);

        let maximum_unique_idf = ((document_count as f32 + 1.0) / 2.0).ln() + 1.0;
        effective.policy.minimum_feature_idf = effective
            .policy
            .minimum_feature_idf
            .min(maximum_unique_idf);
        effective.search.minimum_feature_idf = effective
            .search
            .minimum_feature_idf
            .min(maximum_unique_idf);
        self.search_domain(specimen, &effective)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        DomainSearchOptions, DomainSearchStatus, IndexBuilder, IndexConfig, SearchOptions,
        TextDomain,
    };

    #[test]
    fn unique_features_survive_a_two_document_sec_history() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "older",
                "shared filing boilerplate alpha legacy language unrelated details",
            )
            .expect("older");
        builder
            .add_document(
                "recent",
                "shared filing boilerplate copper liquidity covenant maturity disclosure",
            )
            .expect("recent");
        let index = builder.build().expect("index");
        let report = index
            .search_domain_adaptive(
                "shared filing boilerplate copper liquidity covenant maturity disclosure",
                &DomainSearchOptions {
                    search: SearchOptions {
                        minimum_similarity: 0.10,
                        minimum_query_coverage: 0.0,
                        minimum_source_coverage: 0.0,
                        ..SearchOptions::default()
                    },
                    ..DomainSearchOptions::for_domain(TextDomain::SecFiling)
                },
            )
            .expect("search");
        assert_eq!(report.status, DomainSearchStatus::Executed);
        assert_eq!(report.results.first().expect("hit").path, "recent");
        assert!(report.analysis.retained_feature_occurrences > 0);
    }

    #[test]
    fn one_document_history_caps_idf_at_the_attainable_value() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "prior",
                "issuer specific liquidity covenant maturity disclosure and refinancing plan",
            )
            .expect("prior");
        let index = builder.build().expect("index");
        let report = index
            .search_domain_adaptive(
                "issuer specific liquidity covenant maturity disclosure and refinancing plan",
                &DomainSearchOptions {
                    search: SearchOptions {
                        minimum_similarity: 0.10,
                        minimum_query_coverage: 0.0,
                        minimum_source_coverage: 0.0,
                        ..SearchOptions::default()
                    },
                    ..DomainSearchOptions::for_domain(TextDomain::SecFiling)
                },
            )
            .expect("search");
        assert_eq!(report.status, DomainSearchStatus::Executed);
        assert_eq!(report.results.first().expect("hit").path, "prior");
        assert_eq!(report.policy.minimum_feature_idf, 1.0);
    }

    #[test]
    fn large_corpus_policy_fraction_is_not_relaxed() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for index in 0..10 {
            builder
                .add_document(
                    format!("document-{index}"),
                    format!("common language unique marker {index} additional evidence"),
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let options = DomainSearchOptions::for_domain(TextDomain::SecFiling);
        let report = index
            .search_domain_adaptive(
                "common language unique marker 4 additional evidence",
                &options,
            )
            .expect("search");
        assert_eq!(
            report.policy.maximum_document_frequency_fraction,
            options.policy.maximum_document_frequency_fraction
        );
        assert_eq!(
            report.policy.minimum_feature_idf,
            options.policy.minimum_feature_idf
        );
    }
}
