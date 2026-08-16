# Complete evidence suite

`fo-evidence-suite` runs the full measurement and publication path inside one immutable output directory:

```text
scenario benchmark
  → pair-level score receipt
  → optional preregistered claim gate
  → immutable Markdown/HTML evidence bundle
  → final suite manifest
```

It is the preferred interface for a real Gutenberg or SEC proof run after corpus acquisition, sectioning, and optional gold-label adjudication.

## Basic run

```bash
cargo run --release -p fo-bench --bin fo-evidence-suite -- \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  --output evidence-runs/gutenberg-2026-08-15 \
  --corpus-size 25 \
  --corpus-size 100 \
  --corpus-size 250 \
  --title 'FrankenOverlap Gutenberg evidence report'
```

With a preregistered claim manifest:

```bash
cargo run --release -p fo-bench --bin fo-evidence-suite -- \
  showcase/gutenberg/sections \
  benchmark-artifacts/gutenberg-gold.jsonl \
  --claim-manifest evidence/gutenberg-claims.json \
  --gold-validation benchmark-artifacts/gutenberg-gold-validation.json \
  --output evidence-runs/gutenberg-held-out \
  --require-supported
```

`--require-supported` is evaluated only after all artifacts have been written. An unsupported or inconclusive run therefore remains inspectable, while the process exits unsuccessfully for automation or release gating.

## Output contract

```text
evidence-runs/gutenberg-held-out/
  suite-status.json
  proof.json
  scores.jsonl
  claims.json                 # when a claim manifest is supplied
  indexes/                    # only with --retain-indexes
  bundle/
    RESULTS.md
    RESULTS.html
    environment.json
    examples.json
    artifacts.json
  suite.json
```

The output directory must not already exist.

## Status lifecycle

The first published file is `suite-status.json`:

```json
{
  "schema_version":1,
  "status":"running",
  "started_at_unix":1786820000,
  "completed_at_unix":null,
  "message":"benchmark and evidence generation in progress"
}
```

If any stage fails, it becomes:

```json
{
  "status":"failed",
  "completed_at_unix":1786820420,
  "message":"exact error text"
}
```

Only after benchmark, scores, claims, and bundle generation all succeed does it become `complete` and `suite.json` appear.

This prevents a partially populated directory from being mistaken for finished evidence.

## Benchmark controls

The suite exposes the same controls as `fo-proof-bench`:

```text
--corpus-size N                     repeat for nested sizes
--maximum-documents N
--maximum-queries N
--profile NAME                      repeat for selected profiles
--warmup-runs N
--measurement-repetitions N
--qgram-size N
--maximum-document-bytes N
--maximum-exhaustive-cells-per-query N
--maximum-exhaustive-cells-per-scale N
--seed N
--retain-indexes
```

Every evaluated corpus size retains all positive documents before adding deterministic distractors.

## Claim controls

```text
--claim-manifest claims.json
--require-supported
```

Without a claim manifest, the bundle status is `not_evaluated`. No superiority statement is established merely because the benchmark completed.

With a manifest, `claims.json` records supported, inconclusive, or unsupported verdicts using paired query bootstrap evidence and the predeclared quality/latency gates.

## Gold-label controls

```text
--gold-validation gold-validation.json
```

The suite does not itself alter labels. Use `fo-adjudicate` first to produce and validate gold query JSONL. The validation report becomes an input receipt in the immutable bundle.

## Evidence presentation controls

```text
--examples-per-profile 3
--top-candidates-per-method 5
--snippet-tokens 180
--title 'FrankenOverlap evidence report'
```

Representative examples are selected by fixed rules and include hybrid losses when present.

## Final `suite.json`

The suite manifest records:

```text
schema and generation time
complete status
claim status
corpus/query paths
proof and score SHA-256 receipts
optional claim and gold-validation receipts
bundle output paths
profiles
evaluated corpus sizes
```

The bundle's own artifact manifest contains hashes for its rendered inputs and outputs.

## Recommended public workflow

### Gutenberg

```bash
fo-showcase gutenberg --output showcase/gutenberg

fo-adjudicate queue \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  preliminary-scores.jsonl \
  --output review/gutenberg.jsonl

fo-adjudicate apply \
  showcase/gutenberg/queries.jsonl \
  review/gutenberg-decisions.jsonl \
  --output gold/gutenberg.jsonl

fo-adjudicate validate \
  showcase/gutenberg/sections \
  gold/gutenberg.jsonl \
  --json > gold/gutenberg-validation.json

fo-evidence-suite \
  showcase/gutenberg/sections \
  gold/gutenberg.jsonl \
  --claim-manifest evidence/gutenberg-claims.json \
  --gold-validation gold/gutenberg-validation.json \
  --output evidence-runs/gutenberg-final \
  --require-supported
```

### SEC

```bash
SEC_USER_AGENT='Example Research research@example.com' \
  fo-showcase sec10k --output showcase/sec-10k

fo-evidence-suite \
  showcase/sec-10k/sections \
  gold/sec-10k.jsonl \
  --claim-manifest evidence/sec-claims.json \
  --gold-validation gold/sec-validation.json \
  --output evidence-runs/sec-final
```

## What the suite does not do

It does not:

- download or refresh a corpus during timing;
- silently change the query set;
- adjudicate ambiguous natural labels;
- promote an inconclusive result;
- extrapolate incomplete exhaustive timings;
- overwrite an existing evidence run;
- hide a failed intermediate stage.

Corpus preparation is deliberately separated from benchmark timing so network and extraction latency cannot contaminate search measurements.
