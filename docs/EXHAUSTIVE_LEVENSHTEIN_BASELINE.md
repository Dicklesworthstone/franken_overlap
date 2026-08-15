# Bounded exhaustive Levenshtein baseline

`fo-exhaustive-bench` measures the corpus-wide algorithm FrankenOverlap is intended to replace: exact semi-global Levenshtein dynamic programming against every candidate document.

The baseline is deliberately real rather than extrapolated. Every completed query/document pair evaluates the full dynamic-programming matrix and reports its measured wall time. Work that does not fit the declared budgets is reported as incomplete instead of being assigned a fictional latency or score.

## Query format

Queries are JSONL records with one or more acceptable positive documents:

```json
{"id":"frankenstein-1818-01","profile":"natural-edition","text":"...","positive_ids":["41445","84"]}
{"id":"aapl-risk-2024","profile":"year-over-year-risk","text":"...","positive_ids":["CIK0000320193-2024-item-1a"]}
```

Multiple positives are important for naturally related editions, repeated filing language, and other cases where exactly one source label would be false precision.

## Run

```bash
cargo run --release -p fo-bench --bin fo-exhaustive-bench -- \
  corpora/gutenberg-chapters \
  examples/gutenberg/queries.jsonl \
  --maximum-documents 250 \
  --maximum-cells-per-query 2000000000 \
  --maximum-total-cells 20000000000 \
  --output benchmark-artifacts/gutenberg-exhaustive.json \
  --scores-output benchmark-artifacts/gutenberg-exhaustive-scores.jsonl
```

Use `--require-complete-exhaustive` for corpus sizes where every pair must finish. On larger corpora, omit it and inspect:

```text
exhaustive_coverage.complete
exhaustive_coverage.complete_queries
exhaustive_coverage.partial_queries
exhaustive_coverage.evaluated_pairs
exhaustive_coverage.skipped_pairs
exhaustive_coverage.cells
```

## Compared methods

The same query set is scored by:

- normalized exact substring search;
- fielded BM25, phrase, and proximity retrieval;
- exhaustive semi-global Levenshtein;
- FrankenOverlap sparse alignment;
- unified hybrid search.

AUPRC and ranking metrics for exhaustive Levenshtein are calculated only over queries for which every candidate document completed. Partial rows retain an optional alignment receipt but are never silently treated as zero-score negatives.

## Exact DP semantics

The exhaustive baseline uses ordinary unit-cost Levenshtein operations with free corpus prefix and suffix. It therefore finds the minimum edit distance between the complete query and any contiguous corpus substring.

For each completed pair it records:

```text
distance
similarity
text_start
text_end
cells
```

Memory is linear in candidate-document length. Runtime work is exactly:

```text
normalized_query_tokens × normalized_document_tokens
```

## Break-even accounting

The report compares the hybrid index build cost with measured exhaustive and indexed p95 query latency:

```text
saved_ms_per_query_at_p95 = exhaustive_p95_ms - indexed_p95_ms
break_even_queries = build_ms / saved_ms_per_query_at_p95
```

No break-even value is emitted when exhaustive retrieval is not faster than the indexed route or no exhaustive query completed.

This separates two legitimate operating regimes:

- one exact query over a tiny corpus, where a direct method can win;
- repeated or large-corpus retrieval, where index construction can amortize quickly.

## Interpretation

A partial exhaustive run is useful evidence about a work limit, but it is not a quality comparison over the full corpus. Public claims should distinguish:

- completed measured latency;
- explicitly skipped work;
- quality on complete queries;
- indexed quality on the full query set.

Do not extrapolate a partial run into an invented speedup.
