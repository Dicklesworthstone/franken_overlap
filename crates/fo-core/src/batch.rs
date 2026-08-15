use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};

use crate::{FoError, Index, Result, SearchOptions, SearchResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchQuery {
    pub id: String,
    pub specimen: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchSearchOptions {
    /// Zero uses Rayon's configured global worker count.
    pub threads: usize,
    pub maximum_queries: usize,
    pub maximum_total_specimen_bytes: usize,
    pub deduplicate_identical_specimens: bool,
    pub fail_fast: bool,
}

impl Default for BatchSearchOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            maximum_queries: 1_000_000,
            maximum_total_specimen_bytes: 1024 * 1024 * 1024,
            deduplicate_identical_specimens: true,
            fail_fast: false,
        }
    }
}

impl BatchSearchOptions {
    pub fn validate(self) -> Result<Self> {
        if self.threads > 4096 {
            return Err(FoError::InvalidConfig(
                "batch threads must not exceed 4096".to_owned(),
            ));
        }
        if self.maximum_queries == 0 || self.maximum_total_specimen_bytes == 0 {
            return Err(FoError::InvalidConfig(
                "batch query and byte limits must be positive".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchResult {
    pub query_index: usize,
    pub query_id: String,
    pub deduplicated_from: Option<usize>,
    pub results: Vec<SearchResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchReport {
    pub queries: usize,
    pub unique_specimens: usize,
    pub deduplicated_queries: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub total_hits: usize,
    pub results: Vec<BatchSearchResult>,
}

#[derive(Debug, Clone)]
struct QueryOutcome {
    results: Vec<SearchResult>,
    error: Option<String>,
}

impl Index {
    /// Search many specimens in parallel while preserving input order.
    ///
    /// Identical specimen strings share one search by default. Individual query
    /// failures are recorded without aborting unrelated work unless `fail_fast`
    /// is enabled.
    pub fn search_batch(
        &self,
        queries: &[BatchQuery],
        search_options: &SearchOptions,
        batch_options: BatchSearchOptions,
    ) -> Result<BatchSearchReport> {
        search_options.validate()?;
        let batch_options = batch_options.validate()?;
        validate_queries(queries, batch_options)?;

        let (canonical_for, unique_indices) =
            canonical_queries(queries, batch_options.deduplicate_identical_specimens);
        let execute = || {
            unique_indices
                .par_iter()
                .map(|&query_index| {
                    let outcome = match self.search(&queries[query_index].specimen, search_options) {
                        Ok(results) => QueryOutcome {
                            results,
                            error: None,
                        },
                        Err(error) => QueryOutcome {
                            results: Vec::new(),
                            error: Some(error.to_string()),
                        },
                    };
                    (query_index, outcome)
                })
                .collect::<Vec<_>>()
        };
        let unique_outcomes = if batch_options.threads == 0 {
            execute()
        } else {
            ThreadPoolBuilder::new()
                .num_threads(batch_options.threads)
                .thread_name(|index| format!("fo-batch-{index}"))
                .build()
                .map_err(|error| {
                    FoError::InvalidConfig(format!("could not build batch thread pool: {error}"))
                })?
                .install(execute)
        };

        let mut outcomes = vec![None::<QueryOutcome>; queries.len()];
        for (query_index, outcome) in unique_outcomes {
            if batch_options.fail_fast {
                if let Some(error) = &outcome.error {
                    return Err(FoError::InvalidConfig(format!(
                        "batch query {} ({}) failed: {error}",
                        query_index, queries[query_index].id
                    )));
                }
            }
            outcomes[query_index] = Some(outcome);
        }
        for query_index in 0..queries.len() {
            let canonical = canonical_for[query_index];
            if query_index != canonical {
                outcomes[query_index] = outcomes[canonical].clone();
            }
        }

        let mut results = Vec::with_capacity(queries.len());
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut total_hits = 0usize;
        for (query_index, query) in queries.iter().enumerate() {
            let outcome = outcomes[query_index].take().unwrap_or_else(|| QueryOutcome {
                results: Vec::new(),
                error: Some("batch scheduler produced no outcome".to_owned()),
            });
            if outcome.error.is_some() {
                failed += 1;
            } else {
                succeeded += 1;
                total_hits = total_hits.saturating_add(outcome.results.len());
            }
            results.push(BatchSearchResult {
                query_index,
                query_id: query.id.clone(),
                deduplicated_from: (canonical_for[query_index] != query_index)
                    .then_some(canonical_for[query_index]),
                results: outcome.results,
                error: outcome.error,
            });
        }
        Ok(BatchSearchReport {
            queries: queries.len(),
            unique_specimens: unique_indices.len(),
            deduplicated_queries: queries.len().saturating_sub(unique_indices.len()),
            succeeded,
            failed,
            total_hits,
            results,
        })
    }
}

fn validate_queries(queries: &[BatchQuery], options: BatchSearchOptions) -> Result<()> {
    if queries.len() > options.maximum_queries {
        return Err(FoError::InvalidConfig(format!(
            "batch contains {} queries; limit is {}",
            queries.len(), options.maximum_queries
        )));
    }
    let mut ids = HashSet::with_capacity(queries.len());
    let mut total_bytes = 0usize;
    for (index, query) in queries.iter().enumerate() {
        if query.id.trim().is_empty() {
            return Err(FoError::InvalidConfig(format!(
                "batch query {index} has an empty id"
            )));
        }
        if !ids.insert(query.id.as_str()) {
            return Err(FoError::InvalidConfig(format!(
                "duplicate batch query id {}",
                query.id
            )));
        }
        total_bytes = total_bytes.checked_add(query.specimen.len()).ok_or_else(|| {
            FoError::InvalidConfig("batch specimen byte total overflowed usize".to_owned())
        })?;
        if total_bytes > options.maximum_total_specimen_bytes {
            return Err(FoError::InvalidConfig(format!(
                "batch specimens contain {total_bytes} bytes; limit is {}",
                options.maximum_total_specimen_bytes
            )));
        }
    }
    Ok(())
}

fn canonical_queries(queries: &[BatchQuery], deduplicate: bool) -> (Vec<usize>, Vec<usize>) {
    if !deduplicate {
        let indices = (0..queries.len()).collect::<Vec<_>>();
        return (indices.clone(), indices);
    }
    let mut first_seen = HashMap::<&str, usize>::with_capacity(queries.len());
    let mut canonical_for = Vec::with_capacity(queries.len());
    let mut unique_indices = Vec::new();
    for (query_index, query) in queries.iter().enumerate() {
        let canonical = *first_seen.entry(query.specimen.as_str()).or_insert_with(|| {
            unique_indices.push(query_index);
            query_index
        });
        canonical_for.push(canonical);
    }
    (canonical_for, unique_indices)
}

#[cfg(test)]
mod tests {
    use super::{BatchQuery, BatchSearchOptions};
    use crate::{IndexBuilder, IndexConfig, SearchOptions};

    fn index() -> crate::Index {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document(
                "source.txt",
                "preserve every raw observation before comparing causal models",
            )
            .expect("source");
        builder
            .add_document(
                "noise.txt",
                "winter vegetables and railway timetables fill the old cabinet",
            )
            .expect("noise");
        builder.build().expect("index")
    }

    #[test]
    fn preserves_order_and_reuses_identical_specimens() {
        let queries = vec![
            BatchQuery {
                id: "first".to_owned(),
                specimen: "preserve every raw observation".to_owned(),
            },
            BatchQuery {
                id: "second".to_owned(),
                specimen: "winter vegetables".to_owned(),
            },
            BatchQuery {
                id: "duplicate".to_owned(),
                specimen: "preserve every raw observation".to_owned(),
            },
        ];
        let report = index()
            .search_batch(
                &queries,
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_matched_tokens: 4,
                    ..SearchOptions::default()
                },
                BatchSearchOptions {
                    threads: 2,
                    ..BatchSearchOptions::default()
                },
            )
            .expect("batch");
        assert_eq!(report.results[0].query_id, "first");
        assert_eq!(report.results[1].query_id, "second");
        assert_eq!(report.results[2].query_id, "duplicate");
        assert_eq!(report.unique_specimens, 2);
        assert_eq!(report.results[2].deduplicated_from, Some(0));
        assert_eq!(report.results[0].results.len(), report.results[2].results.len());
        for (left, right) in report.results[0]
            .results
            .iter()
            .zip(&report.results[2].results)
        {
            assert_eq!(left.path, right.path);
            assert_eq!(left.corpus_start, right.corpus_start);
            assert_eq!(left.corpus_end, right.corpus_end);
            assert!((left.combined_score - right.combined_score).abs() < 1e-6);
        }
    }

    #[test]
    fn isolates_query_failures_unless_fail_fast_is_enabled() {
        let queries = vec![
            BatchQuery {
                id: "bad".to_owned(),
                specimen: "   ".to_owned(),
            },
            BatchQuery {
                id: "good".to_owned(),
                specimen: "winter vegetables".to_owned(),
            },
        ];
        let report = index()
            .search_batch(
                &queries,
                &SearchOptions::default(),
                BatchSearchOptions::default(),
            )
            .expect("isolated batch");
        assert!(report.results[0].error.is_some());
        assert!(report.results[1].error.is_none());

        let error = index()
            .search_batch(
                &queries,
                &SearchOptions::default(),
                BatchSearchOptions {
                    fail_fast: true,
                    ..BatchSearchOptions::default()
                },
            )
            .expect_err("fail fast");
        assert!(error.to_string().contains("bad"));
    }
}
