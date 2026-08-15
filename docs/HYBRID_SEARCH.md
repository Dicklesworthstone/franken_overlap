# Unified hybrid search

`fo-search` combines FrankenOverlap's edited-passage alignment with ordinary fielded lexical retrieval under one persisted index and one explainable ranking contract.

## Build

A hybrid index is a directory containing:

```text
manifest.json
overlap.foidx
lexical.folex
```

Both components share the same document IDs and external identifiers. Loading fails if the manifest, configurations, or document identity spaces disagree.

Build from an ordinary directory tree:

```bash
cargo run -p fo-cli --bin fo-search -- build \
  ./documents \
  --output ./documents.fohybrid
```

Build directly from a corpus downloaded by `fo-corpus`:

```bash
cargo run -p fo-cli --bin fo-search -- build \
  ./corpora/sec-standard \
  --input-format corpus \
  --output ./sec.fohybrid
```

A JSONL input can provide explicit title, body, tags, and metadata through `HybridDocumentInput`.

## Query modes

### Automatic

```bash
fo-search query documents.fohybrid 'material weakness liquidity covenant'
```

The automatic router selects:

- lexical search for short keyword, fielded, Boolean, and quoted-phrase queries;
- overlap alignment for very long specimen passages;
- hybrid fusion for the middle regime.

The selected route is returned in both human and JSON output.

### Lexical

```bash
fo-search query documents.fohybrid \
  '+title:observatory "copper shutters" detector -tag:cooking' \
  --mode lexical
```

This uses rare-term-first candidate generation, fielded BM25, exact phrase occurrences, and positional proximity.

### Overlap

```bash
fo-search query documents.fohybrid \
  --query-file specimen.txt \
  --mode overlap
```

This uses the adaptive overlap planner, diagonal voting, anchor chaining, exact verification, and composite aggregation when the specimen appears fragmented or reordered.

### Hybrid

```bash
fo-search query documents.fohybrid \
  'the observatory opened copper shutters before sunrise and the team checked every detector twice' \
  --mode hybrid
```

Hybrid ranking combines:

1. a saturating lexical score;
2. the verified overlap or composite score;
3. reciprocal-rank fusion;
4. a bounded bonus when independent lexical and overlap lanes agree;
5. a bounded exact-phrase bonus.

No lexical score is treated as edit-distance evidence, and no approximate overlap candidate is accepted without the existing verifier.

## Filters

The hybrid index retains corpus metadata and tags:

```bash
fo-search query sec.fohybrid 'material weakness liquidity' \
  --require-tag 10-k \
  --metadata form=10-K \
  --external-id-prefix CIK0000320193
```

Filters are applied against the canonical lexical document record after candidate fusion.

## Explainability

Each result includes:

- lexical and overlap ranks;
- raw and saturated lexical score;
- overlap score;
- reciprocal-rank contribution;
- phrase signal;
- agreement and phrase bonuses;
- final score;
- complete lexical explanation;
- complete passage or composite overlap evidence.

This is intentionally suitable for learned reranking: future models can consume the component evidence without losing the deterministic baseline.

## Complexity

Lexical cost is driven by retained term posting lists and the bounded document candidate set. Overlap cost is governed by the existing query planner and sparse posting-pair budget. Hybrid mode runs both candidate generators and fuses only their union, so memory is linear in the retained candidate count rather than corpus size.

The persisted components remain independently inspectable and can be benchmarked separately against the fused result.
