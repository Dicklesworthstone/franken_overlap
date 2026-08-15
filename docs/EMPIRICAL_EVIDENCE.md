# Empirical evidence bundles

Architecture, tests, and benchmark code do not establish that FrankenOverlap is better than another method. A performance claim requires a concrete corpus snapshot, a fixed query and relevance set, complete candidate scores, a recorded machine/compiler environment, and uncertainty around the observed quality difference.

`fo-evidence` converts the two primary outputs of `fo-real-bench` into such a bundle:

```bash
cargo run --release -p fo-bench --bin fo-real-bench -- \
  --corpus-root corpora/gutenberg-chapters \
  --provider existing \
  --maximum-documents 250 \
  --source-documents 48 \
  --queries-per-document 8 \
  --output benchmark-artifacts/gutenberg/report.json \
  --scores-output benchmark-artifacts/gutenberg/scores.jsonl

cargo run --release -p fo-bench --bin fo-evidence -- \
  benchmark-artifacts/gutenberg/report.json \
  benchmark-artifacts/gutenberg/scores.jsonl \
  --output benchmark-artifacts/gutenberg/evidence \
  --method franken_hybrid \
  --baseline normalized_exact_substring \
  --baseline character_qgram_jaccard \
  --baseline character_qgram_simhash \
  --baseline fielded_bm25_phrase_proximity \
  --baseline franken_overlap \
  --bootstrap-samples 5000 \
  --minimum-macro-delta-lower-bound 0.0
```

The evidence directory contains:

```text
evidence.json
EXAMPLES.md
environment.json
manifest.json
SHA256SUMS
```

## Validation performed

The generator refuses to proceed when:

- the report and score stream disagree about pair or query counts;
- a query/candidate pair appears twice;
- a query group has no relevant candidate;
- query-level source or profile metadata is inconsistent;
- a score is non-finite or outside `[0, 1]`;
- a score row has a different method set from the benchmark report;
- AUPRC or MRR recomputed from the score stream differs from the report.

The report is therefore not trusted merely because it is valid JSON.

## Paired query bootstrap

Every bootstrap draw samples complete query groups with replacement. Candidate rows belonging to a query are never split across samples. For every selected-method/baseline pair, the bundle reports point estimates and percentile intervals for:

- micro candidate-stream AUPRC;
- macro query AUPRC;
- expected tie-aware MRR;
- expected tie-aware Recall@1.

Macro query AUPRC is the primary default gate because it prevents a few large candidate lists from dominating the conclusion.

## Quality and wall time are separate claims

A more accurate method can be slower than exact substring search or SimHash. Conversely, a very fast method can have unusable recall. `fo-evidence` therefore records separately:

- quality superiority against each baseline;
- p50, p95, and p99 latency;
- throughput;
- whether the selected method is faster at p95;
- whether it strictly dominates a baseline on both the declared quality gate and p95.

The possible bundle verdicts are:

```text
quality_and_wall_time_superiority_supported
quality_superiority_supported_wall_time_not_dominant
superiority_not_established
```

A p95 speed ratio above one means the selected method was faster than that baseline.

## Illustrative examples

For every mutation profile, the Markdown report includes:

- the query with the largest selected-method rank improvement;
- the hardest selected-method query when it is a distinct case;
- the true source;
- the tie-aware source-rank interval;
- top candidates and scores for the selected method and every baseline.

Score streams may optionally include `query_text`, `source_title`, and `candidate_title`. The next-stage real-world showcase emits those fields so examples display actual edited Gutenberg and SEC passages rather than opaque query IDs.

## What this does not prove

One bundle establishes results only for its recorded:

- corpus and sectioning snapshot;
- source-document sample;
- mutation profiles;
- candidate universe;
- compiler and hardware;
- search configuration;
- thresholds.

It does not establish performance on arbitrary corpora or semantic paraphrases. Re-run the benchmark and regenerate evidence after any material change to normalization, indexing, candidate generation, verification, ranking, corpus sectioning, or fusion weights.
