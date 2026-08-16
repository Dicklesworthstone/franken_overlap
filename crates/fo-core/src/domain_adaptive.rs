use crate::{DomainSearchOptions, DomainSearchReport, Index, Result};

impl Index {
    /// Apply a domain policy without allowing its document-frequency fraction to
    /// suppress every nonempty posting list in a very small candidate corpus.
    ///
    /// For `N` indexed documents, at least a fingerprint occurring in exactly one
    /// document remains eligible. Larger corpora retain the configured fraction
    /// unchanged. All IDF, posting-count, work-budget, and thin-evidence rules are
    /// still enforced by `search_domain`.
    pub fn search_domain_adaptive(
        &self,
        specimen: &str,
        options: &DomainSearchOptions,
    ) -> Result<DomainSearchReport> {
        let mut effective = options.clone();
        let minimum_nonzero_fraction = 1.0 / self.documents().len().max(1) as f32;
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
    }
}
