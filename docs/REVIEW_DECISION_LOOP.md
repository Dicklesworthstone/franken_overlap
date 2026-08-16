# Apply review decisions to feedback and lineage

`fo-review-report` makes evidence inspectable. `fo-review-apply` makes the resulting decisions accretive.

One completed review can update four durable stores:

```text
query-grouped ranking feedback
probability-calibration feedback
accepted textual-lineage edges
auditable decision-application ledger
```

The source results and decisions are content-addressed by SHA-256 in every ledger record.

## Complete a review

Generate the standalone page:

```bash
fo-review-report \
  corpora/sec-items \
  current-item-1a.txt \
  results/current-item-1a.json \
  --target-id AAPL-2025-item-1a \
  --output reviews/AAPL-2025-item-1a
```

Open `reviews/AAPL-2025-item-1a/index.html`, make decisions, and download `decisions.jsonl`.

Completed decisions require:

```text
reviewer
reviewed_at_unix
candidate ID
decision
optional notes
optional accepted block indexes
```

## Apply the decisions

```bash
fo-review-apply \
  results/current-item-1a.json \
  reviews/AAPL-2025-item-1a/decisions.jsonl \
  --feedback-output evidence/ranking-feedback.jsonl \
  --calibration-output evidence/calibration-feedback.jsonl \
  --lineage evidence/lineage.json \
  --decision-ledger evidence/review-applications.jsonl \
  --target-title 'Apple 2025 Item 1A'
```

Inspect without writing:

```bash
fo-review-apply results.json decisions.jsonl \
  --feedback-output feedback.jsonl \
  --lineage lineage.json \
  --dry-run \
  --json
```

## Decision meanings

| Decision | Ranking label | Calibration label | Lineage effect |
|---|---:|---:|---|
| `accept` | positive | positive | accepted localized blocks may add a source→target edge |
| `reject` | negative | negative | none |
| `uncertain` | none | none | none |
| `unreviewed` | none | none | none |
| `correct_source` | reviewed candidate becomes negative | reviewed candidate becomes negative | corrected ID is recorded, but no edge is fabricated without localized evidence for that source |

The tool never turns semantic-only or lexical-only relevance into a textual lineage edge.

## Block-level feedback

For composite or fragmented results, a reviewer can retain only selected block indexes:

```json
{
  "accepted_block_indexes":[0,2]
}
```

Each selected localized block becomes an individual feedback record. The candidate's total review weight is divided across selected blocks so a ten-block result does not count ten times more than a one-block result.

When `accepted_block_indexes` is empty, every localized block for that candidate is used.

## Ranking feedback

`--feedback-output` writes `GroupedFeedbackExample` JSONL:

```text
query_id = target_id
result = localized SearchResult
label = reviewer decision
weight = candidate review weight divided across selected blocks
```

This file can feed:

- grouped hard-negative mining;
- pairwise ranking;
- AP-delta ranking;
- active-learning exclusion of already reviewed examples.

Existing records are loaded, merged, sorted, and deduplicated by query, candidate span, and label. Reapplying the same result and decision files adds zero new records.

## Calibration feedback

`--calibration-output` writes ordinary `FeedbackExample` JSONL for the logistic calibration layer.

It uses the same localized evidence and labels but omits `query_id`, matching the existing calibration contract.

## Lineage thresholds

An accepted candidate adds lineage evidence only when each selected block satisfies:

```text
--minimum-lineage-score 0.30
--minimum-lineage-query-coverage 0.15
--minimum-lineage-matched-tokens 24
```

The edge evidence records:

```text
reviewer
review notes
accept decision
source and target spans
edit and coverage evidence
estimated false matches
review time
```

Accepting a candidate with no localized block is counted and reported as `accepted_without_localized_evidence`; no edge is added.

## Decision ledger

`--decision-ledger` stores one `AppliedDecision` per target/candidate review containing:

```text
application ID
application time
result and decision file digests
complete original decision
localized result count
ranking/calibration records emitted
lineage edge changes
corrected-source indicator
```

The application ID is deterministic for the exact result file, decision file, target, candidate, and decision. Exact reruns are idempotent.

## Validation

The decision contract fails closed on:

- unknown schema versions;
- empty target or candidate IDs;
- target equal to candidate;
- duplicate target/candidate pairs;
- completed decisions without reviewer and timestamp;
- unreviewed records that falsely claim completion;
- invalid or redundant corrected-source IDs;
- unsorted or duplicate block indexes;
- block indexes outside the localized result set;
- decisions for candidates absent from the reviewed result file.

## Accretive operating loop

```text
search
  → review page
  → human decision
  → fo-review-apply
       ├─ positive/negative ranking evidence
       ├─ calibration evidence
       ├─ accepted lineage edges
       └─ immutable application receipt
  → retrain on accumulated feedback
  → held-out AUPRC and slice gates
  → promote only supported improvements
```

This keeps source attribution, ranking, calibration, and deployment evidence connected without collapsing them into one opaque model score.
