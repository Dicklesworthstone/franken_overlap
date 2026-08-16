# SEC filing lineage analyst

`fo-sec-lineage` turns sectioned 10-K histories into a practical analyst artifact rather than a pile of pairwise search results.

For every selected filing item, it can:

- compare the current section with earlier same-issuer versions;
- suppress corpus-wide filing boilerplate before positional voting;
- identify the strongest previous source and additional plausible ancestors;
- optionally search older same-item sections from other issuers;
- distinguish high reuse, moderate revision, material revision, and largely new language;
- identify likely reintroduced legacy language;
- flag possible peer-language migration;
- write review-ready result JSON for each target;
- build a cumulative `LineageGraph` with exact localized evidence;
- emit machine-readable alerts and an analyst-facing Markdown summary.

## Input

The input must be an SEC `fo-corpus`, normally derived from raw filings:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
  fo-corpus sec10k \
  --preset standard \
  --output corpora/sec-raw
```

```bash
fo-section corpora/sec-raw \
  --output corpora/sec-items \
  --strategy sec10k
```

Each eligible section must retain:

```text
cik
section_title
published_or_filed
relative_path
SHA-256 and byte-length receipt
```

Source bytes are reverified against the corpus manifest before analysis.

## Analyze the latest filing item in each history

```bash
cargo run --release -p fo-cli --bin fo-sec-lineage -- \
  corpora/sec-items \
  --output analysis/sec-lineage \
  --section 'Item 1A' \
  --section 'Item 7' \
  --threads 8
```

By default, the newest section in every `(CIK, canonical section title)` history is compared with up to eight earlier versions.

Analyze every year after the first:

```bash
fo-sec-lineage corpora/sec-items \
  --output analysis/sec-history \
  --all-targets
```

## Cross-issuer language migration

```bash
fo-sec-lineage corpora/sec-items \
  --output analysis/sec-peer-lineage \
  --include-peer-search \
  --maximum-peer-candidates 500
```

Peer search considers only earlier sections with the same canonical item title and a different CIK. It is optional because it expands work substantially and because widespread boilerplate must not be mistaken for migration.

The SEC domain policy is applied before search. A peer alert additionally requires independent score and target-coverage floors.

## Output

```text
analysis/sec-lineage/
  report.json
  alerts.jsonl
  lineage.json
  SUMMARY.md
  artifacts.json
  results/
    <target-id>.json
```

`report.json` contains:

- corpus and manifest identity;
- all thresholds and work limits;
- eligible section and history counts;
- every target filing item;
- same-issuer and peer query-policy diagnostics;
- strongest prior and peer sources;
- accepted lineage sources;
- alerts;
- a ready-to-run `fo-review-report` command for each target.

`lineage.json` can be inspected or merged with `fo-lineage`.

`artifacts.json` records byte lengths and SHA-256 digests for all report, alert, lineage, summary, and target-result files.

## Alert taxonomy

| Alert | Meaning |
|---|---|
| `no_history` | no earlier same-issuer item is loaded |
| `insufficient_distinctive_evidence` | common or novel language left too little informative overlap |
| `new_language` | the strongest prior source explains little or none of the target |
| `material_revision` | meaningful lineage exists, but coverage or local edit similarity is low |
| `moderate_revision` | related prior language survives with substantial changes |
| `high_reuse` | most target wording survives with high local similarity |
| `legacy_language_reintroduced` | an older filing materially outranks the immediately prior filing |
| `peer_language_migration` | an older filing from another issuer explains a meaningful target span |

These are deterministic triage labels, not legal or causal conclusions.

## Important thresholds

```text
--minimum-edge-score 0.30
--minimum-edge-query-coverage 0.15
--minimum-edge-matched-tokens 32
--new-language-coverage 0.20
--material-change-coverage 0.55
--material-change-similarity 0.72
--high-reuse-coverage 0.80
--high-reuse-similarity 0.88
--legacy-language-margin 0.08
--peer-migration-score 0.55
--peer-migration-coverage 0.35
```

These are starting values. They must be calibrated and evaluated on adjudicated held-out SEC histories before they are treated as production policies.

## Human review

Every target in `report.json` includes a command resembling:

```bash
fo-review-report \
  corpora/sec-items \
  corpora/sec-items/documents/<target>.txt \
  analysis/sec-lineage/results/<target>.json \
  --target-id <target> \
  --output reviews/<target>
```

The resulting page shows original filing text, localized source spans, edit evidence, and structured accept/reject decisions.

Recommended decision flow:

```text
high and review-severity alerts
  → fo-review-report
  → analyst accepts/rejects/corrects source
  → accepted localized edges remain in lineage
  → rejections become hard negatives
  → uncertain cases enter active learning
```

## Performance model

Issuer histories are intentionally indexed as small candidate sets. This gives three benefits:

1. the search cannot be distracted by unrelated filing items;
2. the full SEC boilerplate policy remains inspectable;
3. each target can be analyzed independently across a bounded Rayon pool.

Controls:

```text
--threads 4
--maximum-targets 500
--maximum-prior-filings 8
--maximum-peer-candidates 250
--maximum-sources-per-target 3
--review-candidates 20
```

Parallelism is across independent filing targets, not nested inside a single sparse query.

## Product interpretation

The highest-value use is not merely “find similar filings.” It is a durable chronology of language:

```text
which risk appeared this year?
which paragraph disappeared?
which older version was restored?
which issuer first used this distinctive wording?
which peer language migrated into the current filing?
what exact source spans support the answer?
```

FrankenOverlap supplies candidate lineage and exact evidence. Analyst review, filing chronology, and external context remain necessary before drawing causal or legal conclusions.
