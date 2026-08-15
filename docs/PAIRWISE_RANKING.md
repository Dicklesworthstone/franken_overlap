# Query-Grouped Pairwise Ranking

Probability calibration answers “how likely is this hit to be correct?” Pairwise ranking answers a different question that is often closer to AUPRC and source-attribution quality:

> For this specimen, should the known positive source rank above this difficult negative source?

`RankingModel` trains a deterministic RankNet-style linear scorer over within-query positive/negative pairs. Negatives are ordered by their current apparent strength and the hardest examples are retained first, so training effort is concentrated on false positives that can actually damage precision.

## Feedback schema

Each JSONL row contains a stable `query_id`, the full `SearchResult`, a label, and an optional weight:

```json
{"query_id":"specimen-0042","result":{...},"label":true,"weight":1.0}
```

All positives for a query are retained. `fo-rank mine-hard-negatives` keeps the highest-scoring negatives under a declared per-query cap:

```bash
cargo run -p fo-bench --bin fo-rank -- mine-hard-negatives \
  feedback-all.jsonl \
  --maximum-negatives-per-query 16 \
  --output feedback-mined.jsonl
```

## Fit and held-out acceptance

```bash
cargo run -p fo-bench --bin fo-rank -- fit \
  feedback-train.jsonl \
  --output ranker.json

cargo run -p fo-bench --bin fo-rank -- compare \
  feedback-test.jsonl ranker.json \
  --require-auprc-delta 0.01 \
  --json
```

The comparison reports raw and ranked AUPRC plus the delta. The command exits unsuccessfully when the held-out improvement is below the requested floor.

## Evidence vector

The ranker preserves and combines the engine’s auditable evidence:

- raw combined score
- edit similarity
- query and source coverage
- anchor coverage and vote support
- chain consistency
- matched-length and anchor-count saturation
- estimated-false-match confidence
- edit × query-coverage interaction
- query-coverage × chain-consistency interaction
- anchor-coverage × vote-support interaction
- bidirectional coverage harmonic mean

The persisted model records schema version, exact feature order, means, scales, learned weights, training query/pair counts, completed epochs, and raw/ranked training reports.

The ranker is intentionally separate from the calibrated probability model. Ranking can be adopted for retrieval ordering based on held-out AUPRC while probability calibration can still be fitted afterward on the ranker’s candidate distribution.
