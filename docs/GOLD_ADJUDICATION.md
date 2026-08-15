# Gold-query adjudication

Controlled corruptions have exact generated provenance. Natural editions, annual filing histories, and boilerplate do not. `fo-adjudicate` converts ambiguous real-corpus scenarios into reviewable, validated gold queries without changing the benchmark format.

## Why adjudication is required

A single-positive label can be wrong when:

- several Gutenberg editions contain the same passage;
- several years of one issuer's Item 1A remain substantially equivalent;
- several issuers reproduce standard contractual or regulatory language;
- the generated source is not the only acceptable retrieval result;
- different retrieval lanes disagree about the most likely source.

Treating every alternative as a negative artificially lowers precision for a correct system and can train a ranker toward arbitrary source IDs rather than meaningful relevance.

## Create a review queue

Run the scenario benchmark with pair-level receipts:

```bash
fo-proof-bench \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  --output benchmark-artifacts/gutenberg-proof.json \
  --scores-output benchmark-artifacts/gutenberg-proof-scores.jsonl
```

Then create review tasks from the largest measured corpus size:

```bash
cargo run --release -p fo-bench --bin fo-adjudicate -- queue \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  benchmark-artifacts/gutenberg-proof-scores.jsonl \
  --output benchmark-artifacts/gutenberg-review.jsonl \
  --report benchmark-artifacts/gutenberg-review-report.json
```

An explicit size can be selected with `--corpus-size`.

## Queue selection

A query is queued when one or more conditions hold:

- it is a `natural_relation` query;
- retrieval methods disagree at rank one;
- no generated positive appears in hybrid top-k;
- the hybrid top-one margin is below the configured threshold;
- hybrid returns no positive score.

`--include-all` produces tasks for every query.

Each task includes:

```text
original query and generated positives
review reasons
method leaders
hybrid margin
candidate titles and IDs
per-method scores and ranks
generated-positive flag
predicted spans
normalized source snippets
an editable decision template
```

Candidates include every generated positive, every method leader, and the top hybrid results. A positive can therefore never disappear merely because the current system ranks it poorly.

## Decision format

Copy completed decision templates into a decision JSONL file:

```json
{
  "query_id":"84#section-0012:natural_relation",
  "status":"replace",
  "positive_ids":[
    "84#section-0012",
    "41445#section-0011",
    "42324#section-0013"
  ],
  "graded_relevance":{
    "84#section-0012":3,
    "41445#section-0011":3,
    "42324#section-0013":2
  },
  "acceptable_spans":{
    "84#section-0012":[{"start":420,"end":1012}],
    "41445#section-0011":[{"start":397,"end":987}]
  },
  "reviewer":"reviewer-name",
  "notes":"same passage family; 1831 edition is substantially rewritten",
  "reviewed_at_unix":1786820000
}
```

Statuses:

| Status | Meaning |
|---|---|
| `accept_generated` | generated positive set is correct |
| `replace` | use the supplied positive set instead |
| `exclude` | query is intrinsically ambiguous or unsuitable |

Relevance grades are integers from 1 through 3. Every positive receives grade 3 by default when omitted.

Acceptable spans use normalized Unicode-token coordinates, matching the overlap index and proof benchmark. Several spans per positive document are allowed.

## Apply decisions

```bash
cargo run --release -p fo-bench --bin fo-adjudicate -- apply \
  showcase/gutenberg/queries.jsonl \
  benchmark-artifacts/gutenberg-decisions.jsonl \
  --output benchmark-artifacts/gutenberg-gold.jsonl \
  --report benchmark-artifacts/gutenberg-gold-report.json
```

Controlled mutation queries without a decision retain deterministic source provenance. Natural-relation queries fail closed when unreviewed unless `--allow-unreviewed-natural` is supplied explicitly.

The resulting records remain compatible with `fo-proof-bench`. They add a `gold` object containing reviewer, status, notes, relevance grades, acceptable spans, and review time.

## Validate gold data

```bash
cargo run --release -p fo-bench --bin fo-adjudicate -- validate \
  showcase/gutenberg/sections \
  benchmark-artifacts/gutenberg-gold.jsonl
```

Validation checks:

- unique, nonempty query IDs;
- at least one positive per query;
- every positive exists in the corpus manifest;
- relevance grades lie in 1..=3;
- no non-positive receives a grade or span;
- every span is nonempty;
- span endpoints lie within the document's normalized token length;
- complete SHA-256 receipt of the gold query file.

## SEC example

A year-over-year Item 1A query might retain several annual sections with graded relevance:

```json
{
  "query_id":"CIK0000320193-2024-item-1a:natural_relation",
  "status":"replace",
  "positive_ids":[
    "CIK0000320193-2024-item-1a",
    "CIK0000320193-2023-item-1a",
    "CIK0000320193-2022-item-1a"
  ],
  "graded_relevance":{
    "CIK0000320193-2024-item-1a":3,
    "CIK0000320193-2023-item-1a":3,
    "CIK0000320193-2022-item-1a":1
  },
  "acceptable_spans":{},
  "reviewer":"reviewer-name",
  "notes":"2022 contains the same risk family but material wording differences",
  "reviewed_at_unix":1786820000
}
```

This supports binary AUPRC through `positive_ids` and future graded nDCG through `graded_relevance`.

## Review discipline

Gold labels should be created without looking at aggregate method winners. Reviewers see candidate evidence for one query, not which system is favored across the benchmark.

For high-stakes public claims, use at least two independent reviewers and adjudicate disagreements. Store reviewer decisions separately from generated score rows so changes remain auditable.
