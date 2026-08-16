# FrankenOverlap

**Sparse-spectral textual overlap, approximate alignment, and hybrid lexical search in safe Rust.**

FrankenOverlap started from one observation: approximate passage search does not have to begin with a corpus-wide edit-distance scan. Equality-preserving textual features can vote for likely alignments through sparse positional postings, while dense categorical cross-correlation remains available when the corpus is unindexed or every offset matters.

The project has since grown into a broader search workbench that uses the same principles throughout:

- categorical identifiers are never treated as numeric coordinates;
- rare evidence is processed before common evidence;
- approximate stages generate candidates rather than final truth;
- positions, phrases, diagonals, and chains preserve structure;
- expensive exact alignment is spent only on surviving spans;
- ranking changes are measured on query groups and real corpora;
- accepted improvements become durable profiles and experiment records.

FrankenOverlap now supports:

- edited and partially copied passage discovery;
- fragmented and reordered source attribution;
- near-duplicate detection;
- fielded BM25 keyword search;
- exact phrase and positional proximity search;
- lexical/overlap hybrid retrieval;
- multi-q-gram consensus;
- batch and segmented-corpus operation;
- calibrated and pairwise/listwise learned ranking;
- active learning and hard-negative mining;
- native Project Gutenberg and SEC Form 10-K acquisition;
- chapter and 10-K-item section derivation;
- real-corpus AUPRC and latency benchmarking;
- held-out fusion tuning;
- append-only experiment history and evidence-gated profile promotion.

The default core is safe Rust and has no C, C++, Python, BLAS, or GPU runtime dependency.

## The central model

For specimen tokens `P[0..m]` and corpus tokens `T[0..n]`, exact positional overlap at offset `s` is:

```text
M(s) = Σ_j weight(P[j]) · 1[T[s+j] = P[j]]
```

FrankenOverlap evaluates this categorical correlation through a portfolio:

```text
static indexed corpus
  → normalized q-grams
  → winnowed rare fingerprints
  → positional postings
  → diagonal votes
  → anchor chains
  → bounded exact verification

unindexed corpus
  → exact direct equality below crossover
  → unit-circle phase-sketch FFT above crossover
  → local maxima
  → candidate verification

ordinary search query
  → fielded terms / phrases / positions
  → BM25 + proximity evidence
  ↘
    explainable hybrid fusion
  ↗
  → edited-passage overlap evidence
```

Sparse postings and dense FFTs are two execution strategies for the same equality statistic. The runtime chooses the route based on corpus state, query length, entropy, feature retention, and predicted posting work.

## Why not correlate token IDs directly?

Unicode code points, BPE IDs, and vocabulary IDs are categorical labels. Renumbering a vocabulary must not change textual similarity. Token `50,001` is not intrinsically closer to `50,002` than to `7`.

FrankenOverlap therefore uses:

- equality-preserving rolling fingerprints for sparse retrieval;
- one-hot-equivalent positional evidence;
- signed or phase-hashed categorical embeddings for approximate dense correlation;
- exact normalized-token verification before a lexical overlap is accepted.

## Quick start

Build the workspace with the moving nightly selected by `rust-toolchain.toml`:

```bash
git clone https://github.com/Dicklesworthstone/franken_overlap
cd franken_overlap
cargo build --release --workspace
```

### Build a general hybrid search index

```bash
cargo run --release -p fo-cli --bin fo-search -- build \
  ./documents \
  --output ./documents.fohybrid
```

Search it:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  'material weakness liquidity covenant'
```

Fielded and phrase query:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  '+title:observatory "copper shutters" detector -tag:cooking'
```

Edited passage query:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  --query-file specimen.txt \
  --mode overlap
```

Machine-readable output preserves the complete lexical and overlap explanations:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  'risk factors semiconductor demand' \
  --json
```

See [`docs/HYBRID_SEARCH.md`](docs/HYBRID_SEARCH.md).

## Download real corpora natively

The `fo-corpus` crate downloads and verifies text without shell scripts or Python glue.

Available providers:

```text
Project Gutenberg
SEC EDGAR Form 10-K
```

The corpus manifest records every source URL, SHA-256 digest, byte and character count, publication/filing metadata, provider snapshot, and acquisition failure. Downloads are resumable and fail closed on integrity mismatches.

List the exact acquisition commands and presets:

```bash
cargo run --release -p fo-corpus -- --help
cargo run --release -p fo-corpus -- gutenberg --help
cargo run --release -p fo-corpus -- sec10k --help
```

Project Gutenberg automation follows the official machine-readable catalog and mirror model. Larger runs require an explicit mirror rather than scraping the human-facing site.

SEC acquisition requires a declared identity containing a contact email and enforces a configurable rate below the SEC fair-access ceiling:

```bash
export SEC_USER_AGENT='Example Research research@example.com'
```

See [`docs/CORPUS_ACQUISITION.md`](docs/CORPUS_ACQUISITION.md).

## Turn books and 10-Ks into meaningful retrieval units

Whole books and annual filings dilute lexical evidence and make correct passages appear insignificant relative to the complete source document. Derive chapter- or item-level corpora:

```bash
cargo run --release -p fo-corpus --bin fo-section -- \
  corpora/gutenberg-standard \
  --output corpora/gutenberg-chapters \
  --strategy gutenberg
```

```bash
cargo run --release -p fo-corpus --bin fo-section -- \
  corpora/sec-standard \
  --output corpora/sec-items \
  --strategy sec10k
```

The derived manifest retains parent identity, section title/index, extraction strategy, and exact parent byte range. Oversized sections are subdivided at paragraph boundaries with deterministic overlap.

See [`docs/CORPUS_SECTIONING.md`](docs/CORPUS_SECTIONING.md).

## Benchmark on actual books and filings

`fo-real-bench` can consume an existing corpus or invoke the native downloader itself. It extracts real passages and generates deterministic workloads covering:

- exact reuse;
- case, punctuation, and formatting drift;
- word substitutions;
- burst insertions and deletions;
- OCR-like corruption;
- fragmented reuse surrounded by unrelated context;
- reordered blocks;
- short keyword queries.

Example:

```bash
cargo run --release -p fo-bench --bin fo-real-bench -- \
  --provider existing \
  --corpus-root corpora/gutenberg-chapters \
  --maximum-documents 250 \
  --source-documents 32 \
  --output benchmark-artifacts/gutenberg.json \
  --scores-output benchmark-artifacts/gutenberg-scores.jsonl
```

Compared methods:

```text
normalized exact substring
character q-gram Jaccard
character q-gram SimHash
fielded BM25 + phrase + proximity
FrankenOverlap sparse alignment
unified hybrid retrieval
```

Reported measurements include micro and macro query AUPRC, tie-aware Recall@1/5/10 and MRR, false positives per query, p50/p95/p99 latency, throughput, index-build time, serialization time, and persisted bytes.

See [`docs/REAL_CORPUS_BENCHMARK.md`](docs/REAL_CORPUS_BENCHMARK.md).

## Tune hybrid fusion on held-out queries

The benchmark score stream can determine lexical, overlap, and reciprocal-rank weights instead of relying on fixed guesses:

```bash
cargo run --release -p fo-bench --bin fo-hybrid-tune -- fit \
  benchmark-artifacts/gutenberg-scores.jsonl \
  --output profiles/gutenberg-hybrid.json \
  --report benchmark-artifacts/gutenberg-tuning.json \
  --require-test-micro-delta 0.005 \
  --require-test-macro-delta 0.005
```

Complete query groups remain intact across deterministic 60/20/20 train, validation, and untouched test splits. Training produces a bounded shortlist; validation selects one configuration; adoption gates are evaluated on the untouched test split.

Use a promoted profile:

```bash
cargo run --release -p fo-cli --bin fo-search-profiled -- \
  indexes/gutenberg.fohybrid \
  profiles/gutenberg-hybrid.json \
  'the shutters opened before dawn'
```

See [`docs/HYBRID_TUNING.md`](docs/HYBRID_TUNING.md).

## Keep benchmark evidence forever

Record a benchmark result and its candidate profile:

```bash
cargo run --release -p fo-bench --bin fo-experiment -- record \
  benchmark-artifacts/experiments.jsonl \
  benchmark-artifacts/gutenberg.json \
  --method franken_hybrid \
  --profile profiles/gutenberg-hybrid.json \
  --commit "$(git rev-parse HEAD)"
```

Promote only when explicit constraints are met:

```bash
cargo run --release -p fo-bench --bin fo-experiment -- promote \
  benchmark-artifacts/experiments.jsonl \
  profiles/registry.json \
  --corpus-id gutenberg-standard-sections \
  --minimum-macro-delta 0.005 \
  --maximum-micro-regression 0.002 \
  --maximum-recall-at-1-regression 0.0 \
  --maximum-p95-regression-fraction 0.10
```

The experiment ledger is append-only. The deployment registry is corpus-specific, atomic, and contains the complete promoted profile plus the exact evidence record that justified it.

See [`docs/EXPERIMENT_LEDGER.md`](docs/EXPERIMENT_LEDGER.md).

## Core search capabilities

### Edited passage search

```bash
cargo run --release -p fo-cli -- \
  index ./documents --output corpus.foidx

cargo run --release -p fo-cli -- \
  query corpus.foidx specimen.txt \
  --intent source-attribution
```

The index uses 128-bit rolling q-gram fingerprints, rightmost-minimum winnowing, IDF-weighted diagonal voting, monotone anchor chaining, and exact verification.

### Fragmented and reordered reuse

```bash
cargo run --release -p fo-cli --bin fo-composite -- \
  corpus.foidx specimen.txt \
  --maximum-blocks 8
```

Several non-overlapping passages from one source can contribute to one source-level result even when the passages were moved or separated by unrelated material.

### Multi-view consensus

```bash
cargo run --release -p fo-cli --bin fo-multiview -- build \
  ./documents \
  --output corpus.fomv \
  --preset balanced
```

Short q-grams recover heavily edited text; long q-grams suppress common-feature collisions. Cross-view agreement becomes an explicit precision signal.

### Adaptive planning

```bash
cargo run --release -p fo-cli --bin fo-plan -- \
  corpus.foidx specimen.txt \
  --execute \
  --json
```

The planner reports entropy, repetition, feature retention, missing and suppressed features, estimated posting-pair work, route, effective cap, and route advisories.

### Parallel batches

```bash
cargo run --release -p fo-cli --bin fo-batch -- \
  corpus.foidx queries.jsonl \
  --output results.jsonl \
  --threads 32
```

One immutable index remains resident while independent specimens occupy separate cores. Identical specimens are deduplicated before execution.

### Incremental segmented corpora

```bash
cargo run --release -p fo-cli --bin fo-segment -- create corpus.fosegments
cargo run --release -p fo-cli --bin fo-segment -- append corpus.fosegments ./new-documents
cargo run --release -p fo-cli --bin fo-segment -- compact corpus.fosegments
```

Immutable segments, stable global document IDs, tombstones, generation checks, verification, and compaction allow the corpus to grow without rebuilding every prior segment.

## Ranking, feedback, and evaluation

The repository includes native tooling for:

- tie-correct AUPRC and PR curves;
- Brier score, log loss, ECE, and MCE;
- query-group macro AUPRC;
- tie-aware expected MRR, Recall@k, and nDCG@k;
- query-bootstrap confidence intervals;
- slice-aware worst-cohort evaluation;
- PAN precision, recall, granularity, and PlagDet;
- logistic probability calibration;
- query-grouped hard-negative pairwise ranking;
- AP-delta-weighted listwise ranking;
- active-learning queue construction;
- false-discovery and conformal acceptance policies;
- statistically gated model tournaments.

These tools keep candidate generation, exact verification, ranking, calibration, acceptance policy, and deployment promotion separate and auditable.

## Dense categorical correlation

For direct workloads below the configured crossover, dense scan computes exact positional equality.

Above the crossover, the FrankenSciPy feature uses deterministic unit-circle phase embeddings. Equal categories contribute one; independently hashed unequal categories cancel in expectation. Each repetition needs two real correlations instead of one correlation per hash bucket.

```bash
cargo build --release -p fo-cli --features frankenscipy
cargo run --release -p fo-cli --features frankenscipy -- \
  scan large-document.txt specimen.txt
```

Dense mode is useful when:

- the corpus is not indexed;
- a score is needed at every offset;
- many prepared specimens can amortize spectra;
- a future device backend can retain buffers and scratch.

Sparse indexed search remains the default for repeated queries because it avoids reading unrelated corpus text.

## Workspace

```text
crates/
  fo-core/          indexing, retrieval, alignment, ranking evidence, storage, metrics
  fo-cli/           search, batch, composite, multiview, planner, segment, profile CLIs
  fo-corpus/        Gutenberg/SEC acquisition, manifests, verification, section derivation
  fo-bench/         synthetic/real benchmarks, tuning, ranking, active learning, experiments
  fo-conformance/   cross-module behavioral and corruption contracts
  franken-overlap/  public facade crate

docs/               algorithm, format, benchmark, corpus, and deployment contracts
fixtures/           deterministic conformance corpus
```

## Rust API

```rust
use franken_overlap::{
    HybridDocumentInput, HybridIndexBuilder, HybridIndexConfig, HybridSearchOptions,
};
use std::collections::BTreeMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = HybridIndexBuilder::new(HybridIndexConfig::default())?;
    builder.add_document(HybridDocumentInput {
        external_id: "paper.txt".to_owned(),
        title: "Causal Measurements".to_owned(),
        body: "Preserve the raw measurements before comparing causal models.".to_owned(),
        tags: vec!["science".to_owned()],
        metadata: BTreeMap::new(),
    })?;
    let index = builder.build()?;
    let report = index.search(
        "raw measurements causal models",
        &HybridSearchOptions::default(),
    )?;
    for hit in report.results {
        println!("{} {:.3}: {}", hit.external_id, hit.score, hit.snippet);
    }
    Ok(())
}
```

## Correctness and storage invariants

- Token IDs are categorical labels.
- No corpus-wide edit-distance scan is used as retrieval.
- No `N × pattern_length` window matrix is materialized.
- Matches cannot cross document boundaries.
- Approximate generators do not bypass exact lexical verification.
- Unknown binary and manifest semantics fail closed.
- Persistent postings are sorted and validated.
- Checksummed delta-varint `.foidx` v2 remains backward-compatible with strict v1 loading.
- Unsafe paths, impossible counts, malformed varints, overflow, mismatched identities, checksum failures, and trailing bytes are rejected.
- Query-group splits remain intact during tuning and evaluation.
- Profile promotion is separate from profile training.

## Performance doctrine

No algorithm is universally optimal. FrankenOverlap maintains a measured portfolio:

| Workload | Preferred route |
|---|---|
| Very short specimen | exact search / Myers bit-vector infix |
| Static corpus, repeated specimen queries | sparse winnowed index |
| Short keyword or fielded query | positional lexical index |
| Medium natural-language query | lexical/overlap hybrid |
| Long fragmented specimen | composite passage aggregation |
| Several edit regimes | multi-view q-gram consensus |
| Unindexed resident text | exact direct or phase-sketch FFT |
| Many independent specimens | stable-order parallel batch |
| Continuously growing corpus | immutable segmented index |

Performance claims require the corpus, query distribution, compiler, hardware, commit, feature set, quality metrics, latency distribution, allocations/RSS, and before/after result.

## Validation

The repository uses the moving latest Rust nightly selected by `rust-toolchain.toml` and does not depend on active GitHub Actions workflows.

Required checks:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p fo-cli --features frankenscipy --all-targets
cargo test -p fo-core --features frankenscipy
```

See [`AGENTS.md`](AGENTS.md), [`VALIDATION.md`](VALIDATION.md), and the focused documents under [`docs/`](docs/).

## License

MIT License with the repository's OpenAI/Anthropic rider. See [`LICENSE`](LICENSE).
