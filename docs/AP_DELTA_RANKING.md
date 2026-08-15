# Query-Balanced AP-Delta Ranking

The original `RankingModel` learns a general positive-over-negative pairwise preference. `ApRankingModel` targets average precision more directly.

## Objective

For each training epoch and query group:

1. score every candidate with the current linear model,
2. rank candidates by that score,
3. retain the currently strongest configurable number of negatives,
4. pair every positive with those hard negatives,
5. compute the exact absolute change in query average precision if the positive and negative ranks were swapped,
6. weight the logistic pair loss by that AP delta and the geometric mean of feedback weights,
7. normalize pair importance within the query,
8. average gradients across trainable queries.

This has two important effects:

- a swap near the top of the ranking receives much more weight than a harmless swap at the bottom,
- a query with thousands of candidates does not automatically dominate a query with twenty candidates.

The AP-swap implementation is differentially tested against brute-force recomputation for every binary relevance arrangement through length nine.

## Features

The model uses the same persisted ranking evidence contract as `RankingModel`:

- raw score,
- edit similarity,
- query/source/anchor coverage,
- vote support,
- chain consistency,
- matched-length and anchor-count saturation,
- false-match confidence,
- edit × query coverage,
- query coverage × chain consistency,
- anchor coverage × vote support,
- bidirectional coverage harmonic mean.

Feature names, standardization, weights, schema version, query count, pair count, completed epochs, and raw/ranked grouped reports are stored in the model JSON.

## Fit

Input is `GroupedFeedbackExample` JSONL, with all candidates for a specimen sharing one `query_id`.

```bash
cargo run -p fo-bench --bin fo-ap-rank -- fit \
  train.jsonl \
  --output ap-ranker.json \
  --maximum-negatives-per-query 24
```

## Held-out adoption gate

```bash
cargo run -p fo-bench --bin fo-ap-rank -- compare \
  heldout.jsonl ap-ranker.json \
  --require-micro-auprc-delta 0.005 \
  --require-macro-auprc-delta 0.005 \
  --require-mrr-delta 0.0 \
  --require-recall-at-1-delta 0.0 \
  --bootstrap-samples 1000 \
  --json
```

The command exits unsuccessfully unless every requested held-out gate passes. Micro AUPRC prevents total candidate-stream regressions; macro AUPRC prevents improvements concentrated in only a few large query groups.

## Rerank

```bash
cargo run -p fo-bench --bin fo-ap-rank -- rerank \
  raw-results.json ap-ranker.json \
  --output ranked-results.json
```

The output preserves the complete original `SearchResult` and adds `rank_score`. Pairwise scores are ranking signals, not calibrated probabilities; probability thresholds should continue to use `CalibrationModel`.

## Relationship to active learning

The active-learning queue should include candidates where the raw score, calibrated probability, generic ranker, and AP-delta ranker disagree. Those examples are disproportionately useful because labeling them can change both ordering and the operating threshold.
