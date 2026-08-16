# Domain-aware informative-feature search

Large legal, financial, policy, and code corpora contain phrases that appear everywhere. Those phrases can be lexically real while still being weak source-attribution evidence.

Examples include:

```text
forward-looking statements are subject to risks and uncertainties
except as otherwise provided herein
all rights reserved
use strict
```

`fo-domain-search` applies a query-specific policy before diagonal voting:

1. normalize and winnow the specimen exactly as the index does;
2. group repeated specimen fingerprints;
3. inspect each matching posting list and document frequency;
4. suppress fingerprints that are too common or too weak by IDF;
5. retain rare evidence first under one total posting-pair budget;
6. fail closed when a domain requires distinctive evidence but too little survives;
7. invoke the ordinary FrankenOverlap chain and exact verifier on the filtered index view.

The underlying source spans, edit distance, and verification semantics are unchanged.

## Presets

```text
general
sec-filing
contract
ocr
source-code
```

The presets are conservative starting policies, not universal optimal parameters.

| Domain | Intent |
|---|---|
| `general` | preserve ordinary behavior, including direct fallback |
| `sec-filing` | suppress corpus-wide filing boilerplate and bound posting work |
| `contract` | suppress repeated clause templates while retaining local wording |
| `ocr` | tolerate more common evidence because character corruption removes rare q-grams |
| `source-code` | suppress language/framework boilerplate before code-specific adaptation |

## SEC example

```bash
cargo run --release -p fo-cli --bin fo-domain-search -- \
  sec-items.foidx current-item-1a.txt \
  --domain sec-filing \
  --intent source-attribution \
  --minimum-matched-tokens 48 \
  --minimum-query-coverage 0.25 \
  --json
```

The JSON report contains:

```text
selected feature occurrences
retained feature occurrences
missing feature occurrences
suppressed occurrences by posting cap
suppressed occurrences by document frequency
suppressed occurrences by IDF
suppressed occurrences by total work budget
posting pairs before and after policy
informative-feature fraction
mean retained IDF
maximum retained document-frequency fraction
execution status
verified results
```

## Inspect a query without returning hits

```bash
fo-domain-search sec-items.foidx current-item-1a.txt \
  --domain sec-filing \
  --plan-only \
  --json
```

This is useful for calibrating a policy on a real corpus before changing retrieval thresholds.

## Explicit overrides

```text
--maximum-document-frequency-fraction 0.25
--minimum-feature-idf 1.4
--maximum-query-posting-pairs 3000000
--minimum-informative-feature-fraction 0.15
--maximum-postings-per-feature 50000
```

A feature must pass every active restriction. The effective document-frequency and pair-budget limits are the stricter of the domain policy and the embedded `SearchOptions` limits.

## Thin-evidence behavior

For SEC, contracts, and source code, a specimen composed almost entirely of corpus-wide boilerplate returns:

```text
status = insufficient_informative_evidence
results = []
```

It does not silently switch to an expensive corpus-wide direct scan and then assign a unique source to non-unique language.

For general and OCR profiles, direct fallback remains enabled by default. It can be enabled explicitly for any domain with:

```text
--allow-direct-fallback-on-thin-evidence
```

## Quality and performance expectations

This policy is designed to improve two properties simultaneously:

- **precision/AUPRC:** common boilerplate contributes less source-attribution evidence;
- **wall time:** posting products that cannot distinguish documents are never voted.

Those improvements must still be measured on held-out corpus queries. Use `fo-proof-bench`, grouped evaluation, and paired claim gates rather than treating the policy design as a result.

Recommended SEC ablation:

```text
ordinary FrankenOverlap
versus
SEC-domain FrankenOverlap
```

Measure at minimum:

```text
macro query AUPRC
micro AUPRC
Recall@1 and Recall@10
false positives per query
worst Item/profile slice
p50/p95/p99
posting pairs before/after
fraction of queries failing closed
```

A policy should not be promoted merely because it is faster. It must retain acceptable source recall and improve or preserve held-out ranking quality.
