# Experiment ledger and profile promotion

A search system becomes accretive only when benchmark evidence survives the individual run. `fo-experiment` stores every real-corpus result in an append-only JSONL ledger and promotes a fusion profile only when explicit quality and latency gates are satisfied.

## Record a benchmark

```bash
cargo run -p fo-bench --bin fo-experiment -- record \
  benchmark-artifacts/experiments.jsonl \
  benchmark-artifacts/gutenberg-run.json \
  --method franken_hybrid \
  --profile profiles/gutenberg-hybrid.json \
  --commit "$(git rev-parse HEAD)" \
  --compiler "$(rustc -Vv | tr '\n' ' ')" \
  --host "m4-max"
```

The recorder extracts the chosen method from a `fo-real-bench` report and persists:

- corpus ID and provider;
- document, source-query, query, and pair counts;
- deterministic seed;
- method and commit;
- optional compiler, host, and notes;
- report path and content fingerprint;
- optional fusion-profile fingerprint and complete profile;
- micro and macro AUPRC;
- MRR and Recall@1/5/10;
- false positives per query;
- p50/p95/p99 and throughput;
- build, serialization, and index-size measurements.

Run IDs are unique. Duplicate IDs and malformed existing ledger rows fail closed. A single-writer lock prevents concurrent append corruption.

## Inspect history

```bash
fo-experiment list benchmark-artifacts/experiments.jsonl

fo-experiment list benchmark-artifacts/experiments.jsonl \
  --corpus-id gutenberg-standard-sections \
  --method franken_hybrid \
  --json
```

The ledger itself is append-only. Historical evidence is never silently rewritten when a new profile is promoted.

## Select the best eligible run

```bash
fo-experiment best benchmark-artifacts/experiments.jsonl \
  --corpus-id sec-standard-sections \
  --minimum-micro-auprc 0.80 \
  --minimum-macro-auprc 0.75 \
  --minimum-recall-at-1 0.70 \
  --maximum-p95-ms 250 \
  --require-profile
```

Eligible runs are ordered by:

1. macro query AUPRC;
2. micro AUPRC;
3. Recall@1;
4. lower p95 latency;
5. recency and stable run ID.

Macro AUPRC leads because it prevents very large candidate groups from dominating promotion.

## Promote

```bash
fo-experiment promote \
  benchmark-artifacts/experiments.jsonl \
  profiles/registry.json \
  --corpus-id sec-standard-sections \
  --minimum-macro-delta 0.005 \
  --maximum-micro-regression 0.002 \
  --maximum-recall-at-1-regression 0.0 \
  --maximum-p95-regression-fraction 0.10
```

The registry is keyed by corpus ID and stores the complete promoted profile plus the exact evidence record. When a current promotion exists, the candidate must satisfy every delta gate:

```text
macro AUPRC improvement >= minimum
micro AUPRC regression <= maximum
Recall@1 regression <= maximum
p95 latency regression fraction <= maximum
```

The registry is replaced atomically only after validation.

## Use the promoted profile

Export or inspect the corpus entry:

```bash
fo-experiment registry profiles/registry.json \
  --corpus-id sec-standard-sections \
  --json
```

The embedded `HybridFusionProfile` can be written to its own file or loaded directly by an application. `fo-search-profiled` accepts the same profile schema.

## Recommended loop

```text
fo-corpus / fo-section
        ↓
fo-real-bench --scores-output
        ↓
fo-hybrid-tune fit
        ↓
fo-real-bench with candidate profile
        ↓
fo-experiment record
        ↓
fo-experiment promote
        ↓
fo-search-profiled
```

This loop separates evidence generation, model selection, final benchmark measurement, historical recording, and deployment promotion. A new experiment can add information indefinitely without mutating previous results.
