# Unified hybrid search

`fo-search` exposes one persisted index and one result schema for four retrieval strategies:

- fielded lexical search for short information-seeking queries
- overlap alignment for long specimen passages
- composite overlap for fragmented or reordered passages
- hybrid evidence fusion when both lexical relevance and textual reuse matter

## Build once

```bash
cargo run -p fo-cli --bin fo-search -- build \
  corpora/sec-standard \
  --input-format corpus \
  --output sec.fohybrid
```

A hybrid directory contains a checksummed compressed overlap index, a fielded lexical index, and a manifest proving that both lanes share the same document identity space.

## Automatic routing

```bash
cargo run -p fo-cli --bin fo-search -- query \
  sec.fohybrid \
  'material weakness liquidity controls'
```

The default route is selected from query structure:

| Query shape | Route |
|---|---|
| up to 12 words | lexical |
| 13–47 words | hybrid |
| at least 48 words | overlap |
| at least 180 words and multiple paragraphs | composite |
| explicit quotes, +/- clauses, or field scopes | lexical unless extremely long |

All thresholds are configurable and the selected route is returned in `HybridQueryAnalysis`.

Force a route when the application already knows the intent:

```bash
fo-search query sec.fohybrid '"material weakness" controls' --mode lexical
fo-search query books.fohybrid --query-file specimen.txt --mode overlap
fo-search query books.fohybrid --query-file long-specimen.txt --mode composite
fo-search query sec.fohybrid 'risk factor language' --mode hybrid
```

## Hybrid fusion

The hybrid route runs both lanes over expanded candidate sets and joins results by stable document ID. Its final score combines:

1. weighted reciprocal-rank fusion
2. normalized lexical evidence
3. verified overlap evidence
4. a small cross-lane consensus bonus

A document supported by only one lane remains eligible, but agreement is visible and rewarded. Every result preserves the complete lexical and overlap evidence rather than reducing them to one opaque number.

```bash
fo-search query corpus.fohybrid 'query text' \
  --lexical-weight 1.0 \
  --overlap-weight 1.5 \
  --rrf-k 60 \
  --json
```

## Filters

Metadata and tags apply identically to every route:

```bash
fo-search query sec.fohybrid 'liquidity covenant' \
  --metadata form=10-K \
  --require-tag finance \
  --exclude-tag amendment
```

The filter source is the lexical document record embedded in the shared hybrid identity space.

## Result contract

Each `HybridSearchResult` contains:

- stable document ID and external ID
- title, tags, metadata, and snippet
- selected route and final score
- optional complete lexical result
- optional complete overlap result
- optional composite result
- lane ranks and normalized scores
- reciprocal-rank and evidence components
- cross-lane support indicator

This turns FrankenOverlap from a specialized passage detector into a general search system while retaining exact alignment as a first-class evidence lane rather than replacing it with ordinary bag-of-words retrieval.
