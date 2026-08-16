# Immutable evidence bundles

`fo-proof-report` converts corpus, query, benchmark, score, claim, and optional gold-validation receipts into a human-readable evidence bundle.

The renderer does not recompute retrieval scores. It verifies that the corpus-manifest and query digests match the proof report, then presents the already measured evidence without silently changing the experiment.

## Generate a bundle

```bash
cargo run --release -p fo-bench --bin fo-proof-report -- \
  showcase/gutenberg/sections \
  benchmark-artifacts/gutenberg-gold.jsonl \
  benchmark-artifacts/gutenberg-proof.json \
  benchmark-artifacts/gutenberg-proof-scores.jsonl \
  --claim-report benchmark-artifacts/gutenberg-claims.json \
  --gold-validation benchmark-artifacts/gutenberg-gold-validation.json \
  --output evidence/gutenberg-2026-08-15 \
  --title 'FrankenOverlap Gutenberg evidence report'
```

The output directory must not already exist. Evidence bundles are immutable by default.

## Generated artifacts

```text
evidence/gutenberg-2026-08-15/
  RESULTS.md
  RESULTS.html
  environment.json
  examples.json
  artifacts.json
```

### `RESULTS.md`

A repository-friendly report containing:

- corpus and query fingerprints;
- corpus sizes and benchmark protocol;
- build, serialization, and cold-load cost;
- AUPRC, Recall@k, MRR, p95, throughput, and span accuracy tables;
- exhaustive-Levenshtein completion and DP-cell accounting;
- measured break-even points;
- preregistered claim verdicts;
- representative real-world cases;
- explicit interpretation boundaries.

### `RESULTS.html`

A standalone, dependency-free HTML report with the same evidence. It can be opened directly or published as a static artifact.

### `environment.json`

Records:

```text
full command line
Git commit and dirty-worktree status when available
rustc -Vv
cargo -V
operating system and architecture
CPU model
logical core count
physical memory
hostname
generation time
```

Missing system commands produce null fields rather than failing the evidence run.

### `examples.json`

Machine-readable representative query cases with:

- query text and profile;
- source and all acceptable positives;
- source passage snippet;
- positive rank interval per method;
- top candidates and scores;
- available aligned snippets;
- hybrid rank advantage over the best conventional baseline;
- deterministic selection reason.

### `artifacts.json`

Contains SHA-256, byte length, and path for every input receipt and generated artifact except itself.

Inputs include:

```text
corpus manifest
query/gold JSONL
proof report
pair-level score JSONL
optional claim report
optional gold-validation report
```

## Representative-case selection

The report does not choose only FrankenOverlap wins.

For each profile it selects up to three distinct queries:

1. lexicographically first query;
2. largest hybrid positive-rank gain over the strongest conventional baseline;
3. largest hybrid positive-rank loss against the strongest conventional baseline.

This deliberately exposes failure cases and non-dominance.

Positive ranks are intervals, not arbitrary tie ordering. If ten candidates share a score and the first acceptable positive is in that tie group, the report shows the entire possible rank interval.

## Source and candidate snippets

Controlled scenarios contain an expected source span. The renderer loads the actual corpus document, applies the same normalization profile, and displays the surrounding passage.

For methods with predicted spans, top-result snippets are drawn around the returned alignment. Methods without span evidence retain titles, scores, and ranks but do not pretend to localize text.

## Claim language

The top-level claim status is:

| Status | Meaning |
|---|---|
| `supported` | every supplied preregistered comparison passed |
| `inconclusive` | no hard failure, but one or more evidence gates were unresolved |
| `unsupported` | at least one hard comparison failed |
| `not_evaluated` | no claim report was supplied |

The generated report states that only comparisons marked `supported` are established. It never converts `inconclusive` into a positive claim.

Exact substring and BM25 remain visible controls. The report is expected to show them winning workloads naturally suited to them.

## Digest checks

Before rendering, the CLI recomputes:

```text
SHA-256(corpus manifest)
SHA-256(query file)
```

and compares both with the proof report. A changed corpus or query set therefore cannot be combined accidentally with stale measurements.

The bundle manifest then hashes the proof report, scores, claims, validation result, environment, examples, Markdown, and HTML.

## Publishing

A public benchmark should commit or release:

```text
claim-gate manifest
claim-gate result
proof report
pair-level scores or an availability receipt
RESULTS.md
RESULTS.html
environment.json
examples.json
artifacts.json
```

Downloaded corpus text need not be redistributed. The corpus manifest preserves source URLs and SHA-256 digests so another user can reconstruct and verify the same corpus subject to provider terms.

## Prohibited interpretations

Do not claim:

- full-corpus exhaustive latency when exhaustive coverage is partial;
- semantic paraphrase retrieval from lexical-overlap evidence;
- unique source attribution when several gold positives are acceptable;
- span accuracy from methods that return only document scores;
- general superiority from one machine, one corpus, or an inconclusive confidence interval.
