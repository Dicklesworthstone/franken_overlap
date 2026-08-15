# FrankenOverlap

**Ultra-fast textual-overlap retrieval, approximate alignment, and source attribution in safe Rust.**

FrankenOverlap finds where a specimen passage reappears inside a corpus even when the text has been reformatted, partially copied, substituted, or shifted by insertions and deletions. It treats inverted-index voting and dense cross-correlation as two execution strategies for categorical overlap, chains local anchors, and spends exact edit-distance work only on the strongest candidate spans.

```text
specimen ─ normalize ─ q-grams ─ winnow ─ rare postings ─ shifted diagonal votes
                                                                       │
                                                                       ▼
corpus ───── defensive immutable .foidx index ──────────────── monotone chaining
                                                                       │
                                                                       ▼
                                                exact bounded verification ─ hits
```

## Why this is not numerical correlation over token IDs

Unicode code points, BPE IDs, and vocabulary IDs are categorical labels. Their numeric magnitudes have no similarity meaning. FrankenOverlap therefore preserves equality rather than correlating arbitrary identifier values:

- the indexed engine hashes normalized q-grams into 128-bit fingerprints and accumulates positional votes for `corpus_position - query_position`;
- duplicate query features are grouped once and processed rarest-first;
- two shifted diagonal grids reduce quantization-boundary failures;
- anchor chaining tolerates insertions and deletions as controlled diagonal drift;
- exact Myers or band-local dynamic programming verifies every accepted span;
- the dense engine uses independently signed CountSketch channels so unequal categories cancel in expectation.

## Current capabilities

- Unicode NFKC normalization, lowercase expansion, punctuation policy, and whitespace canonicalization
- 128-bit rolling q-gram fingerprints and rightmost-minimum winnowing
- defensive immutable `.foidx` serialization with fail-closed corruption checks
- IDF-weighted sparse positional correlation
- grouped, rare-first query preparation
- dual shifted diagonal voting
- monotone anchor chaining with drift and concave-gap penalties
- exact Myers infix alignment for specimens up to 64 symbols
- exact compact band-local semi-global verification with geometric widening for longer candidates
- linear KMP exact fallback and bounded short-query candidate generation
- explicit `any_passage`, `source_attribution`, and `near_duplicate` search intents
- rich result evidence: query/source coverage, matched length, vote support, chain consistency, anchor count, and estimated false matches
- categorical CountSketch dense correlation, with optional FrankenSciPy FFT execution
- native AUPRC, precision-recall, threshold, Brier, log-loss, ECE, and MCE evaluation
- append-only accepted/rejected feedback records
- deterministic regularized logistic calibration and calibrated reranking
- deterministic edited-text benchmark against exact substring, q-gram Jaccard, and SimHash baselines
- local quality floors that fail on AUPRC or Recall@1 regression

The default implementation is CPU-first, safe Rust, and has no C, C++, BLAS, Python, or GPU runtime dependency.

## Quick start

```bash
git clone https://github.com/Dicklesworthstone/franken_overlap
cd franken_overlap
cargo build --release --workspace

./target/release/fo index ./my-corpus --output ./my-corpus.foidx
./target/release/fo query ./my-corpus.foidx ./specimen.txt
```

Machine-readable output:

```bash
./target/release/fo query corpus.foidx specimen.txt --json > results.json
./target/release/fo inspect corpus.foidx --json
```

Inline specimen:

```bash
./target/release/fo query corpus.foidx \
  --text "preserve the raw measurements before comparing causal models"
```

Dense scan over one unindexed document:

```bash
./target/release/fo scan large-document.txt specimen.txt --minimum-score 0.35
```

Enable the commit-pinned FrankenSciPy FFT backend:

```bash
cargo build --release -p fo-cli --features frankenscipy
```

## Search intents

Different tasks should not share one accidental scoring rule. `SearchOptions::intent` selects the retrieval objective:

| Intent | Purpose | Primary ranking pressure |
|---|---|---|
| `AnyPassage` | find any meaningful reused span | strong local alignment plus significant length |
| `SourceAttribution` | identify the source of a specimen | local quality multiplied by specimen coverage |
| `NearDuplicate` | compare substantially similar documents | symmetric query/source coverage |

Every result retains raw evidence, so downstream applications can replace or calibrate the ranking policy without rerunning retrieval.

## Rust API

```rust
use franken_overlap::{
    IndexBuilder, IndexConfig, SearchIntent, SearchOptions,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    builder.add_document(
        "paper.txt",
        "Preserve the raw measurements and document every transformation before comparing causal models.",
    )?;
    let index = builder.build()?;
    let hits = index.search(
        "Document each transformation and preserve the raw measurements before comparing causal models.",
        &SearchOptions {
            intent: SearchIntent::SourceAttribution,
            ..SearchOptions::default()
        },
    )?;
    for hit in hits {
        println!(
            "{} score={:.3} edit={:.3} query_coverage={:.3}",
            hit.path,
            hit.combined_score,
            hit.edit_similarity,
            hit.query_coverage,
        );
    }
    Ok(())
}
```

Persistent indexes:

```rust
index.save("corpus.foidx")?;
let index = franken_overlap::Index::load("corpus.foidx")?;
```

## Quality evaluation

### Deterministic end-to-end benchmark

```bash
cargo run -p fo-bench -- synthetic
```

This generates known-source specimens under formatting drift, substitutions, insertions/deletions, and partial reuse with unrelated surrounding text. Every query-document pair is scored by FrankenOverlap and three conventional baselines.

Persist the report and score stream:

```bash
cargo run -p fo-bench -- synthetic \
  --documents 64 \
  --queries-per-document 8 \
  --output artifacts/quality/synthetic.json \
  --labeled-scores artifacts/quality/synthetic-scores.jsonl \
  --json
```

Enforce quality floors:

```bash
cargo run -p fo-bench -- synthetic \
  --minimum-auprc 0.80 \
  --minimum-recall-at-1 0.80
```

See [`docs/SYNTHETIC_BENCHMARK.md`](docs/SYNTHETIC_BENCHMARK.md).

### Evaluate arbitrary labeled scores

```bash
cargo run -p fo-bench -- evaluate judgments.jsonl --json
```

Input records are one JSON object per line:

```json
{"score":0.91,"label":true}
{"score":0.37,"label":false}
```

See [`docs/AUPRC.md`](docs/AUPRC.md).

## Accretive feedback and calibration

Save search results, label them, fit on training feedback, and require held-out improvement:

```bash
./target/release/fo query corpus.foidx specimen.txt --json > results.json

cargo run -p fo-bench -- record-feedback results.json \
  --rank 1 --label positive --output feedback-train.jsonl

cargo run -p fo-bench -- fit-calibration feedback-train.jsonl \
  --output calibration.json

cargo run -p fo-bench -- compare-calibration feedback-test.jsonl \
  calibration.json \
  --require-auprc-delta 0.01 \
  --maximum-brier-regression 0.00

cargo run -p fo-bench -- rerank results.json calibration.json \
  --output reranked.json
```

The calibration model operates on ten bounded evidence features and preserves every original result and score. See [`docs/FEEDBACK_CALIBRATION.md`](docs/FEEDBACK_CALIBRATION.md).

## Execution portfolio

| Workload | Route |
|---|---|
| exact short specimen | linear KMP |
| edited specimen up to 64 symbols | Myers bit-vector infix candidates plus exact localized verification |
| static corpus with repeated queries | sparse winnowed postings and diagonal voting |
| long edited candidate | compact band-local semi-global DP with geometric widening |
| unindexed corpus or dense heat map | CountSketch correlation, optionally FFT-backed |
| source attribution | coverage-aware evidence score or learned calibration |
| near-duplicate detection | symmetric coverage-aware ranking |

No algorithm is assumed universally optimal. Performance work is accepted only when quality metrics and end-to-end latency are reported together.

## Workspace

```text
crates/
  fo-core/          normalization, index, retrieval, chaining, verification, metrics, calibration
  fo-cli/           `fo` command-line interface
  fo-bench/         AUPRC, feedback, calibration, deterministic benchmarks, baselines
  fo-conformance/   cross-module, persistence, corruption, and behavioral tests
  franken-overlap/  public facade crate
fixtures/           deterministic end-to-end corpus and specimens
docs/               algorithms, format, quality, benchmark, and integration contracts
```

## Validation

The repository follows the moving latest Rust nightly selected by `rust-toolchain.toml`. GitHub Actions are not used.

```bash
./scripts/ci-local.sh quick
FO_UPDATE_NIGHTLY=1 ./scripts/ci-local.sh full
```

Set `FO_USE_RCH=1` to route Cargo work through an owner-controlled `rch` worker. The normal workspace tests include a deterministic end-to-end AUPRC and Recall@1 floor.

## Integration direction

- **FrankenSciPy** supplies the optional FFT backend today; prepared f32 batched overlap-save plans remain an upstream optimization target.
- **FrankenTorch** is the intended Apple-Silicon Metal gateway for future resident dense-correlation batches.
- **FrankenNumPy** can provide typed array interchange and safe candidate-span views.
- **FrankenPandas** can provide corpus manifests and result-table export outside the hot loop.

See [`docs/UPSTREAM_INTEGRATION.md`](docs/UPSTREAM_INTEGRATION.md) and [`COMPREHENSIVE_PLAN_FOR_FRANKEN_OVERLAP.md`](COMPREHENSIVE_PLAN_FOR_FRANKEN_OVERLAP.md).

## Current limitations

- lexical overlap is distinct from semantic equivalence; there is not yet an embedding-based semantic-anchor lane;
- returned offsets currently refer to normalized Unicode-scalar positions rather than original source-byte positions;
- index updates still rebuild one immutable file; segmented append/delete/compaction is not yet implemented;
- postings are not yet compressed or mmap-backed;
- the FFT lane still lacks prepared f32 batching, channel packing, and overlap-save reuse;
- there is no production Metal backend yet;
- the deterministic synthetic benchmark complements, but does not replace, PAN, OCR, code-clone, SEC, and web-reuse evaluation corpora.

## License

MIT License with the repository’s OpenAI/Anthropic rider. See [`LICENSE`](LICENSE).
