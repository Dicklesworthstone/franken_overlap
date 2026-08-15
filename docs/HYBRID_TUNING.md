# Held-out hybrid fusion tuning

The default hybrid weights are intentionally conservative. `fo-hybrid-tune` replaces hand tuning with a deterministic train/validation/test procedure over the query-document score stream emitted by `fo-real-bench`.

## Produce a score stream

```bash
cargo run -p fo-bench --bin fo-real-bench -- \
  --corpus-root corpora/gutenberg-chapters \
  --provider existing \
  --scores-output benchmark-artifacts/gutenberg-scores.jsonl
```

Each row contains a query ID, candidate ID, relevance label, and named method scores.

## Fit

```bash
cargo run -p fo-bench --bin fo-hybrid-tune -- fit \
  benchmark-artifacts/gutenberg-scores.jsonl \
  --output profiles/gutenberg-hybrid.json \
  --report benchmark-artifacts/gutenberg-tuning.json \
  --require-test-micro-delta 0.005 \
  --require-test-macro-delta 0.005
```

Complete query groups are assigned deterministically to:

- 60% training;
- 20% validation;
- 20% untouched test.

Candidates from one specimen never cross split boundaries.

## Search space

The tuner combines:

- `fielded_bm25_phrase_proximity`;
- `franken_overlap`;
- reciprocal-rank evidence derived independently from both lists.

The deterministic grid evaluates:

- lexical share from 0.00 through 1.00 in 0.05 steps;
- RRF share from 0.00 through 0.40 in 0.10 steps;
- RRF constants 10, 30, 60, and 100.

Training macro query AUPRC produces a bounded shortlist. Validation macro query AUPRC selects one configuration, with micro AUPRC, MRR, Recall@1, and deterministic parameter ordering as tie-breakers. The untouched test split is used only for the final report and adoption gates.

The output profile records:

```text
schema version
lexical / overlap / RRF weights
RRF constant
lexical saturation
agreement and phrase bonuses
candidate multiplier
input score-stream fingerprint
train / validation / test query counts
validation metrics
test metrics
baseline test metrics
```

Agreement and phrase bonuses are set to zero by this tuner because the current score stream contains document-level method scores, not all runtime explanation fields. A later explanation-aware tuner can add those dimensions without changing the profile schema.

## Apply and compare

Emit tuned grouped scores:

```bash
fo-hybrid-tune apply \
  benchmark-artifacts/sec-scores.jsonl \
  profiles/sec-hybrid.json \
  --output benchmark-artifacts/sec-tuned.jsonl
```

Evaluate a fitted profile against the stored `franken_hybrid` baseline:

```bash
fo-hybrid-tune compare \
  benchmark-artifacts/sec-held-out.jsonl \
  profiles/sec-hybrid.json \
  --json
```

## Runtime use

```bash
cargo run -p fo-cli --bin fo-search-profiled -- \
  indexes/sec-items.fohybrid \
  profiles/sec-hybrid.json \
  'material weakness liquidity covenant' \
  --json
```

The profile is validated before use. Invalid schemas, non-finite values, zero total weight, non-positive RRF constants, invalid thresholds, and malformed metric snapshots fail closed.

## Why macro AUPRC is primary

A global candidate stream can be dominated by a handful of large query groups. Macro query AUPRC gives every specimen equal influence during model selection, while micro AUPRC remains a secondary objective and an explicit adoption gate. This prevents an apparent gain caused by one easy document family from hiding broad per-query degradation.

## Reproducibility

The fit report records the input fingerprint, method names, split counts, number of evaluated configurations, shortlist size, selected parameters, and all train/validation/test metrics. Re-running with the same score stream and seed produces the same profile.
