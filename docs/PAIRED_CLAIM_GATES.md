# Paired statistical claim gates

`fo-claim-gate` decides whether a proposed quality or performance statement is supported, inconclusive, or unsupported by a `fo-proof-bench` report and its pair-level score receipt.

The gate manifest should be committed or otherwise frozen **before** examining the final held-out result.

## Create a starter manifest

```bash
cargo run --release -p fo-bench --bin fo-claim-gate -- init \
  evidence/gutenberg-claims.json
```

Starter comparison:

```json
{
  "schema_version":1,
  "corpus_size":null,
  "bootstrap_samples":2000,
  "confidence_level":0.95,
  "seed":7165066974406149985,
  "minimum_queries":20,
  "minimum_profile_queries":3,
  "comparisons":[{
    "id":"hybrid-vs-bm25",
    "baseline_method":"fielded_bm25_phrase_proximity",
    "challenger_method":"franken_hybrid",
    "minimum_challenger_micro_auprc":0.0,
    "minimum_challenger_macro_auprc":0.0,
    "minimum_challenger_recall_at_1":0.0,
    "minimum_micro_auprc_delta":0.0,
    "minimum_macro_auprc_delta":0.0,
    "minimum_recall_at_1_delta":-0.01,
    "minimum_mrr_delta":-0.01,
    "minimum_micro_delta_lower_bound":0.0,
    "minimum_macro_delta_lower_bound":0.0,
    "minimum_recall_at_1_delta_lower_bound":-0.01,
    "maximum_worst_profile_macro_regression":0.02,
    "maximum_challenger_p95_ms":null,
    "maximum_p95_ratio":2.0,
    "require_complete_baseline":true
  }]
}
```

`corpus_size: null` selects the largest scale in the proof report. An explicit size is recommended for a preregistered public comparison.

## Evaluate

```bash
cargo run --release -p fo-bench --bin fo-claim-gate -- evaluate \
  benchmark-artifacts/gutenberg-proof.json \
  benchmark-artifacts/gutenberg-proof-scores.jsonl \
  evidence/gutenberg-claims.json \
  --output evidence/gutenberg-claim-results.json \
  --require-supported
```

`--require-supported` returns a failing status when any comparison is inconclusive or unsupported.

## Complete paired-query rule

For one comparison, a query is eligible only when **every candidate row** has both the baseline and challenger score.

This is essential for exhaustive Levenshtein. A partially completed exhaustive query is not converted into zero-scored negatives and cannot contribute to paired AUPRC.

The report records:

```text
eligible_queries
excluded_incomplete_queries
candidate_pairs
baseline_complete
```

When `require_complete_baseline` is true, an incomplete exhaustive run fails the claim even when a small complete subset looks favorable.

## Metrics

Every comparison reports baseline, challenger, and delta for:

- micro candidate-stream AUPRC;
- macro per-query AUPRC;
- tie-aware expected Recall@1;
- tie-aware expected MRR;
- nDCG@10.

Absolute challenger floors and delta floors are separate. A challenger can therefore be rejected for being absolutely poor even if an even poorer baseline makes its delta positive.

## Paired query bootstrap

The bootstrap samples complete query groups with replacement. All candidates belonging to one query remain together.

For each bootstrap sample:

1. draw the same query groups for baseline and challenger;
2. assign a unique synthetic ID to each sampled query copy;
3. recompute micro and macro AUPRC, Recall@1, MRR, and nDCG;
4. store challenger minus baseline.

The report includes lower, median, and upper delta quantiles.

## Family-wise confidence adjustment

When a manifest declares several comparisons, the nominal error budget is divided by the number of comparisons:

```text
familywise confidence = 1 - (1 - nominal confidence) / comparison count
```

At nominal 95% confidence with five comparisons, each lower bound uses 99% confidence. This conservative Bonferroni-style adjustment reduces the chance that one of many attempted claims appears significant by luck.

The exact comparison set must be declared in the manifest before final evaluation.

## Worst-profile protection

Profiles with at least `minimum_profile_queries` complete paired queries receive independent quality reports.

The gate identifies the lowest challenger-minus-baseline macro-AUPRC delta and rejects the claim when the regression exceeds:

```text
maximum_worst_profile_macro_regression
```

This prevents aggregate gains on exact or easy queries from hiding OCR, insertion/deletion, fragmented, reordered, or natural-relation failures.

A comparison is inconclusive when no profile has enough queries to evaluate this protection.

## Latency gates

Two optional constraints are supported:

```text
maximum_challenger_p95_ms
maximum_p95_ratio = challenger p95 / baseline p95
```

Timing comes from the same corpus scale in the proof report. It uses measured repeat p95, not build time or an extrapolated timeout.

Index build, serialization, cold load, and break-even remain available in the proof report and generated evidence bundle. The p95 gate answers a different question: steady-state query latency.

## Verdicts

### `supported`

All point-estimate, absolute-quality, profile, completeness, latency, sample-size, and lower-bound gates pass.

### `inconclusive`

No hard gate fails, but evidence is insufficient. Typical reasons:

- too few paired queries;
- bootstrap lower bound below the preregistered threshold;
- no profile has enough examples;
- baseline latency is zero, preventing a ratio.

An inconclusive result must not be presented as evidence of superiority.

### `unsupported`

At least one hard gate fails, including:

- absolute quality below its floor;
- point delta below its floor;
- worst-profile regression too large;
- p95 latency or ratio too high;
- required baseline completeness absent.

## Recommended claim set

A realistic public evidence manifest can include distinct comparisons:

```text
hybrid vs BM25 on all mixed queries
overlap vs Jaccard on edited-passage profiles
composite/hybrid vs ordinary overlap on fragmented/reordered profiles
hybrid vs exhaustive Levenshtein for quality on complete small scales
hybrid vs exhaustive Levenshtein for p95 speed at the largest complete scale
exact substring vs hybrid on exact-query latency as a non-dominance control
```

The final item is deliberately expected to favor exact substring. Including natural controls makes the evaluation more trustworthy than a benchmark designed so one system wins every row.

## Receipts

The output hashes:

- proof report;
- pair-level score file;
- gate manifest.

A generated results page can therefore cite the exact inputs behind every verdict.
