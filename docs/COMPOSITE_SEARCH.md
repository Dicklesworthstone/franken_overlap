# Composite Fragmented-Source Search

Ordinary overlap search returns one locally coherent alignment at a time. That is the right primitive for passage discovery, but source attribution often needs to combine several passages from the same document when material has been inserted, deleted, or reordered.

`Index::search_composite` runs the ordinary high-recall passage engine, groups hits by source document, and selects blocks that add new specimen coverage. It rejects near-duplicate blocks in either coordinate system and reports whether the selected corpus blocks occur in a different order from the specimen.

```rust
use franken_overlap::{
    CompositeSearchOptions, Index, SearchIntent, SearchOptions,
};

let index = Index::load("corpus.foidx")?;
let results = index.search_composite(
    &specimen,
    &SearchOptions {
        intent: SearchIntent::SourceAttribution,
        minimum_query_coverage: 0.30,
        ..SearchOptions::default()
    },
    CompositeSearchOptions::default(),
)?;
```

The standalone CLI is discovered automatically by Cargo:

```bash
cargo run -p fo-cli --bin fo-composite -- \
  corpus.foidx specimen.txt \
  --maximum-blocks 8 \
  --minimum-block-tokens 24 \
  --minimum-query-coverage 0.30
```

Each result contains the selected blocks, union query/source coverage, weighted edit similarity, total matched tokens, reordered-block status, aggregate expected false matches, and an intent-aware aggregate score.

The aggregation layer deliberately preserves the ordinary matcher as its candidate generator. Improvements to indexing, passage retrieval, exact verification, and calibrated scoring therefore accrue automatically to composite search without maintaining a second alignment implementation.
