# Fielded positional lexical search

FrankenOverlap now includes a conventional search mode built from the same systems principles as overlap retrieval: normalize categorical text, process the rarest evidence first, retain positions, bound candidate work, and expose every ranking component.

## Build

Build directly from a directory tree:

```bash
cargo run -p fo-cli --bin fo-lexical -- build \
  ./documents \
  --output corpus.folex
```

Build from a corpus acquired by `fo-corpus`:

```bash
cargo run -p fo-cli --bin fo-lexical -- build \
  corpora/sec-standard \
  --input-format corpus \
  --output sec.folex
```

A JSONL input can provide explicit fields:

```json
{"external_id":"doc-1","title":"Copper shutters","body":"...","tags":["astronomy"],"metadata":{"year":"2026"}}
```

## Query syntax

```bash
cargo run -p fo-cli --bin fo-lexical -- query corpus.folex \
  '+title:observatory "copper shutters" detector -tag:cooking'
```

Supported clauses:

- `term`: optional term
- `+term`: required term
- `-term`: prohibited term
- `"exact phrase"`: phrase boost
- `+"exact phrase"`: required phrase
- `title:`, `body:`, `tag:` or `tags:` field scopes

## Retrieval architecture

1. Unicode word segmentation, NFKC normalization, and Unicode lowercase produce categorical terms.
2. Every field retains sorted term positions.
3. Query terms are grouped once and looked up in the sorted dictionary.
4. The rarest bounded terms generate a document candidate set.
5. All query terms are then scored only inside that set.
6. Required and prohibited clauses filter candidates before ranking.
7. Fielded BM25, exact phrase occurrences, minimum covering spans, and term coverage contribute separately.
8. Snippets are cut from original body byte spans around the strongest rare-term evidence.

The score explanation exposes title/body/tag BM25, phrase boost, proximity boost, coverage boost, matched terms, clause counts, and the tightest observed positional span.

## Fielded BM25

Each field uses its own document length and corpus average. The default weights are:

```text
title = 2.5
body  = 1.0
tags  = 3.0
```

These can be changed at index-build time. Query-time boosts are independent, so ranking experiments do not require rebuilding the term dictionary.

## Candidate bounds

```bash
fo-lexical query corpus.folex 'risk liquidity covenant' \
  --candidate-terms 8 \
  --candidates 50000 \
  --maximum-postings-per-term 1000000
```

The engine does not score every document. It uses the rarest eligible query terms to construct a preliminary candidate set, truncates it deterministically, and then applies complete fielded scoring and phrase/proximity verification.

## Persistence

`.folex` files preserve documents, field lengths, original body byte spans, metadata, the sorted term dictionary, and positional postings. Loading validates document IDs, dictionary ordering, posting order, field bounds, byte spans, and schema version before exposing the index.

The current `.folex` representation is compact JSON for auditability and easy fixture construction. Its public logical model deliberately separates storage from ranking so a later compressed/mapped physical format can replace it without changing search semantics.
