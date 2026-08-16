# Separate semantic candidate lane

Textual descent and semantic relatedness are different claims.

A passage can be:

- textually reused but discuss a different context;
- semantically related but share almost no wording;
- both semantically and textually related;
- retrieved lexically without enough localized evidence to establish provenance.

FrankenOverlap therefore does not convert an embedding score into overlap evidence. `fo-semantic-fuse` combines externally generated semantic candidates with a `HybridSearchReport` while preserving each evidence lane separately.

## Default behavior

By default, semantic candidates may rerank documents already found by the lexical/overlap system, but cannot create new results on their own:

```text
allow_semantic_only = false
```

This gives a useful high-recall architecture:

```text
embedding or semantic retriever
        ↓ candidate IDs and scores
FrankenOverlap lexical / positional overlap search
        ↓
explicit fusion with separate evidence fields
```

A result reports:

```text
hybrid score and rank
semantic score and rank
reciprocal-rank contribution
agreement bonus
whether lexical evidence exists
whether localized textual-overlap evidence exists
whether the result is semantic-only
relationship class
complete original hybrid result
complete semantic evidence records
```

## Input format

Semantic candidates can be a versioned object:

```json
{
  "schema_version": 1,
  "query_id": "risk-factor-query-17",
  "candidates": [
    {
      "external_id": "AAPL-2024-item-1a",
      "title": "Apple 2024 Item 1A",
      "score": 0.91,
      "model": "example-embedding-model",
      "revision": "2026-08-01",
      "passage_start": 120,
      "passage_end": 340,
      "metadata": {"index": "sec-embeddings-v3"}
    }
  ]
}
```

The CLI also accepts `Vec<SemanticEvidence>` JSON or newline-delimited `SemanticEvidence` records.

## Fuse with hybrid results

```bash
fo-search query sec.fohybrid \
  --query-file specimen.txt \
  --mode hybrid \
  --json > hybrid.json
```

```bash
fo-semantic-fuse hybrid.json semantic-candidates.json \
  --output fused.json
```

Important defaults:

```text
hybrid weight            0.70
semantic weight          0.20
reciprocal-rank weight   0.10
agreement bonus          0.05
semantic-only            disabled
```

These defaults are starting values, not validated universal weights. Fit and evaluate them on complete held-out query groups before deployment.

## Semantic-only discovery

Semantic-only results can be included explicitly:

```bash
fo-semantic-fuse hybrid.json semantic-candidates.json \
  --allow-semantic-only \
  --maximum-semantic-only 5 \
  --output fused.json
```

Such a result is labeled:

```text
relationship = semantic_only
textual_provenance_supported = false
semantic_only = true
```

It must not be ingested into a textual lineage graph merely because its final fused score is high.

## Relationship classes

| Class | Meaning |
|---|---|
| `textual_provenance` | localized overlap exists; no semantic evidence supplied |
| `textual_and_semantic` | localized overlap and semantic evidence agree |
| `lexical_only` | hybrid retrieval supplied lexical evidence, but no localized overlap or semantic evidence exists |
| `lexical_and_semantic` | lexical retrieval and semantic evidence agree, but no localized provenance evidence exists |
| `semantic_only` | external semantic retrieval only |

## Multiple semantic models

Several semantic evidence records can refer to one document. Their scores are combined with a bounded noisy-OR calculation and every original record remains available for audit.

Because embedding models are often correlated, adding more models is not automatically independent evidence. The combined score is a deterministic fusion feature, not a calibrated probability.

## Evaluation

Evaluate the semantic lane separately on at least:

```text
heavy paraphrase
light paraphrase
edited textual reuse
same-topic hard negatives
boilerplate hard negatives
unrelated documents
```

Report:

```text
semantic candidate Recall@k
textual-verification recall among semantic candidates
macro and micro AUPRC
semantic-only false positives
fraction of fused results with localized provenance
latency added by semantic generation and fusion separately
```

A semantic lane is valuable when it recovers wording-divergent relationships without causing `semantic_only` results to be presented as copied-text provenance.
