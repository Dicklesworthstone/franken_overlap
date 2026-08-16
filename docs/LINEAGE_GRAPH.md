# Durable textual lineage graphs

FrankenOverlap's differentiated product is not merely ranking one source for one specimen. Repeated overlap decisions should accumulate into a durable graph that answers:

- where a passage or document came from;
- which later documents reused it;
- which evidence supports every edge;
- which version is the likely canonical origin;
- which documents form one reuse family;
- whether paragraphs were fragmented or reordered.

`fo-lineage` stores that graph as versioned, validated JSON.

## Data model

A node represents a document, filing item, contract clause, policy version, book section, code file, or other stable unit.

An edge is directed from the proposed source to the proposed descendant:

```text
source ──derived_from/reuses/near_duplicate──▶ target
```

Each edge retains one or more evidence records containing:

```text
method
score
edit similarity
query and source coverage
matched-token count
estimated false matches
source and target spans
reordered-block flag
detection time
metadata
```

Re-ingesting the same result is idempotent. Independent evidence records are retained and combined into an aggregate confidence rather than overwriting history.

## Create and enrich a graph

```bash
cargo run --release -p fo-cli --bin fo-lineage -- \
  init lineage.json
```

```bash
cargo run --release -p fo-cli --bin fo-lineage -- \
  node lineage.json \
  --id AAPL-2025-item-1a \
  --title 'Apple 2025 Item 1A' \
  --observed-at-unix 1756425600 \
  --metadata issuer=AAPL \
  --metadata form=10-K \
  --metadata item='1A'
```

## Ingest search evidence

Run ordinary, composite, or hybrid search with JSON output, then ingest it against a target document:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  sec-items.fohybrid \
  --query-file current-item-1a.txt \
  --mode hybrid \
  --json > current-item-1a-results.json
```

```bash
cargo run --release -p fo-cli --bin fo-lineage -- \
  ingest lineage.json current-item-1a-results.json \
  --target-id AAPL-2025-item-1a \
  --target-title 'Apple 2025 Item 1A' \
  --target-observed-at-unix 1756425600 \
  --relation derived-from \
  --minimum-score 0.45 \
  --minimum-query-coverage 0.25 \
  --minimum-matched-tokens 48
```

Hybrid results that contain only lexical evidence are deliberately skipped. A lineage edge requires localized overlap evidence, not merely topical relevance.

The same command accepts:

- `Vec<SearchResult>` from `fo query --json`;
- `Vec<CompositeSearchResult>` from `fo-composite --json`;
- `HybridSearchReport` from `fo-search query --json`;
- newline-delimited raw `SearchResult` records.

## Query the graph

```bash
fo-lineage ancestors lineage.json AAPL-2025-item-1a --maximum-depth 12
fo-lineage descendants lineage.json AAPL-2021-item-1a --maximum-depth 12
fo-lineage origin lineage.json AAPL-2025-item-1a
fo-lineage families lineage.json --minimum-confidence 0.65
fo-lineage summary lineage.json
fo-lineage verify lineage.json
```

Traversal confidence is the weakest edge on the retained path. Canonical-origin selection prefers:

1. nodes with no incoming edge inside the component;
2. earlier observed timestamps;
3. more direct descendants;
4. deterministic node ID order.

This is an evidence-based heuristic, not a claim of historical causality. Conflicting timestamps and ambiguous origins remain visible in the graph.

## Merge independent runs

Graphs produced for separate corpora, dates, hosts, or analyst review sessions can be combined:

```bash
fo-lineage merge lineage.json analyst-two.json
```

Nodes are upserted by stable ID. Edges are upserted by deterministic `(source, target, relation)` identity. Distinct evidence is retained; exact duplicates are ignored.

## Export for review

```bash
fo-lineage dot lineage.json \
  --minimum-confidence 0.60 \
  --output lineage.dot
```

The DOT output can be rendered by Graphviz or imported into a graph-analysis workflow.

## SEC product workflow

A pragmatic SEC lineage product can use the graph as its durable core:

```text
new 10-K arrives
  → derive Item 1A / Item 7 / other sections
  → search prior filings and peer sections
  → retain localized high-confidence overlap
  → ingest edges into lineage graph
  → identify new roots, changed descendants, and peer-language migration
  → alert an analyst with side-by-side spans
  → preserve analyst acceptance/rejection as additional evidence
```

The graph is intentionally domain-neutral. Contracts, policy versions, book editions, OCR variants, source files, and dataset documents use the same representation.

## Trust boundaries

- Edge identifiers are deterministic and validated on load.
- Every edge must reference two existing nodes.
- Scores and confidence values must be finite and lie in `[0, 1]`.
- Spans must be nonempty and ordered.
- Evidence records are append-only within an edge.
- Pure semantic or lexical similarity should not be mislabeled as textual descent.
- Canonical origin is a graph heuristic and should be reviewed when chronology is incomplete.
