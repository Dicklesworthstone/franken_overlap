# FrankenOverlap

**Explainable textual provenance, edited-passage retrieval, approximate alignment, and hybrid lexical search in safe Rust.**

FrankenOverlap finds where a specimen passage came from even after formatting changes, substitutions, insertions, deletions, OCR corruption, fragmentation, or reordering. It combines rare-feature positional retrieval, anchor chaining, and exact verification rather than scanning every corpus window with edit distance.

The same infrastructure also supports fielded BM25, phrase and proximity search, explainable lexical/overlap fusion, incremental indexes, real-corpus benchmarking, learned reranking, and evidence-gated deployment.

The default core is safe Rust and has no C, C++, Python, BLAS, or GPU runtime dependency.

## The upshot

FrankenOverlap is most valuable when the question is not merely:

> Which documents discuss this topic?

but:

> Where did this passage come from, how much survived, what changed, was it fragmented or reordered, and what exact source evidence supports the answer?

Its strongest potential applications are:

- SEC filing, contract, and policy-language lineage;
- plagiarism and unattributed-reuse analysis;
- edition and version comparison;
- OCR recovery and historical-text alignment;
- code and license provenance after token-aware adaptation;
- dataset deduplication and training-data provenance;
- internal document-reuse and policy-lineage tracking.

The general lexical layer is useful, but the differentiated product is **localized, edit-tolerant, auditable textual provenance**. FrankenOverlap should not pretend to replace Lucene, Elasticsearch, or a vector database for every search workload.

## Empirical status: promising and testable, not yet benchmark-proven

As of August 15, 2026, the repository contains a substantial implementation and the full machinery needed to evaluate it fairly:

- Project Gutenberg and SEC Form 10-K acquisition and verification;
- chapter and filing-item sectioning;
- controlled and natural real-corpus scenarios;
- exact substring, q-gram Jaccard, SimHash, BM25, and exhaustive Levenshtein controls;
- nested corpus-size quality and latency benchmarks;
- natural-label adjudication;
- paired bootstrap claim gates;
- immutable Markdown and HTML evidence bundles;
- a one-command evidence-suite orchestrator.

What `main` does **not** yet contain is a completed real-corpus evidence run with pinned corpus receipts and numerical results. Therefore this README does not claim that FrankenOverlap has already beaten BM25, exact search, Jaccard, SimHash, or exhaustive edit-distance retrieval by a particular AUPRC or wall-time margin.

The correct conclusion today is:

> FrankenOverlap is a serious specialized search system with a plausible advantage on long edited, fragmented, and reordered passage retrieval. That comparative advantage remains a hypothesis until a checked-in evidence suite demonstrates it.

See [`docs/EMPIRICAL_STATUS.md`](docs/EMPIRICAL_STATUS.md).

## Where each method should win

| Workload | Natural first choice | FrankenOverlap's role |
|---|---|---|
| Exact unchanged quotation | exact substring search | normalization and edited fallback |
| Short keyword or fielded query | BM25 / positional lexical search | optional hybrid evidence |
| Pure semantic paraphrase | embeddings or another semantic retriever | textual verification after semantic candidates |
| One short pair of known texts | Myers or direct edit distance | indexing is unnecessary |
| Long edited specimen against a static corpus | sparse overlap index | primary differentiated use case |
| Fragmented or reordered reuse | composite overlap search | primary differentiated use case |
| Several edit/noise regimes | multi-view q-gram consensus | high-recall and agreement evidence |
| Repeated queries over a growing corpus | indexed and segmented retrieval | strong intended use case |
| One-off unindexed scan | direct equality or dense correlation | optional dense route |

A trustworthy benchmark should allow exact search to win exact-query latency and BM25 to win short keyword queries. FrankenOverlap earns its complexity only if it improves edited-passage retrieval, source localization, or provenance at acceptable cost.

## How it works

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
  → shifted diagonal votes
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

Unicode code points, BPE IDs, and vocabulary IDs are categorical labels. Their numeric magnitudes have no similarity meaning, so FrankenOverlap never correlates raw token IDs as coordinates.

## Quick start

```bash
git clone https://github.com/Dicklesworthstone/franken_overlap
cd franken_overlap
cargo build --release --workspace
```

Build a hybrid index:

```bash
cargo run --release -p fo-cli --bin fo-search -- build \
  ./documents \
  --output ./documents.fohybrid
```

Short lexical query:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  'material weakness liquidity covenant'
```

Fielded phrase query:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  '+title:observatory "copper shutters" detector -tag:cooking'
```

Edited specimen:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  --query-file specimen.txt \
  --mode overlap
```

Machine-readable output retains lexical and overlap explanations:

```bash
cargo run --release -p fo-cli --bin fo-search -- query \
  ./documents.fohybrid \
  'risk factors semiconductor demand' \
  --json
```

See [`docs/HYBRID_SEARCH.md`](docs/HYBRID_SEARCH.md).

## Search capabilities

### Edited, fragmented, and reordered passage search

```bash
cargo run --release -p fo-cli -- \
  index ./documents --output corpus.foidx

cargo run --release -p fo-cli -- \
  query corpus.foidx specimen.txt --intent source-attribution

cargo run --release -p fo-cli --bin fo-composite -- \
  corpus.foidx specimen.txt --maximum-blocks 8
```

The sparse path uses 128-bit rolling q-gram fingerprints, rightmost-minimum winnowing, rare-first positional postings, shifted diagonal voting, monotone chains, and exact verification.

### Multi-view consensus

```bash
cargo run --release -p fo-cli --bin fo-multiview -- build \
  ./documents --output corpus.fomv --preset balanced
```

Short q-grams recover heavily edited text; long q-grams suppress common-feature collisions. Cross-view agreement is an explicit precision signal.

### Adaptive planning

```bash
cargo run --release -p fo-cli --bin fo-plan -- \
  corpus.foidx specimen.txt --execute --json
```

The planner reports entropy, repetition, feature retention, suppressed features, predicted posting work, selected route, and effective work limits.

### Parallel and incremental operation

```bash
cargo run --release -p fo-cli --bin fo-batch -- \
  corpus.foidx queries.jsonl --output results.jsonl --threads 32

cargo run --release -p fo-cli --bin fo-segment -- create corpus.fosegments
cargo run --release -p fo-cli --bin fo-segment -- append corpus.fosegments ./new-documents
cargo run --release -p fo-cli --bin fo-segment -- compact corpus.fosegments
```

One resident immutable index can serve parallel specimens. Growing corpora can append immutable segments, apply tombstones, and compact later.

### Dense categorical correlation

Below the configured crossover, dense scan computes exact positional equality. Above it, the optional FrankenSciPy path uses unit-circle phase embeddings and FFT correlation.

```bash
cargo build --release -p fo-cli --features frankenscipy
cargo run --release -p fo-cli --features frankenscipy -- \
  scan large-document.txt specimen.txt
```

Dense mode is for unindexed text or all-offset scoring. Sparse search remains the default for repeated corpus queries because it avoids reading unrelated text.

## Real corpora

Download and verify Project Gutenberg books:

```bash
cargo run --release -p fo-corpus -- gutenberg \
  --preset smoke --output corpora/gutenberg-smoke
```

Download SEC 10-K filings with a declared identity:

```bash
export SEC_USER_AGENT='Example Research research@example.com'

cargo run --release -p fo-corpus -- sec10k \
  --preset standard --output corpora/sec-standard
```

Turn whole books and filings into meaningful retrieval units:

```bash
cargo run --release -p fo-corpus --bin fo-section -- \
  corpora/gutenberg-standard \
  --output corpora/gutenberg-chapters \
  --strategy gutenberg

cargo run --release -p fo-corpus --bin fo-section -- \
  corpora/sec-standard \
  --output corpora/sec-items \
  --strategy sec10k
```

Manifests retain source URLs, provider snapshots, dates, identifiers, metadata, SHA-256 digests, and exact parent byte ranges.

See [`docs/CORPUS_ACQUISITION.md`](docs/CORPUS_ACQUISITION.md) and [`docs/CORPUS_SECTIONING.md`](docs/CORPUS_SECTIONING.md).

## Prove it instead of asserting it

Create curated Gutenberg scenarios:

```bash
cargo run --release -p fo-bench --bin fo-showcase -- \
  gutenberg --output showcase/gutenberg
```

Create SEC filing-history scenarios:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
cargo run --release -p fo-bench --bin fo-showcase -- \
  sec10k --output showcase/sec-10k
```

Run the full immutable proof transaction:

```bash
cargo run --release -p fo-bench --bin fo-evidence-suite -- \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  --claim-manifest evidence/gutenberg-claims.json \
  --output evidence-runs/gutenberg-final
```

The suite compares identical candidate universes across:

```text
normalized exact substring
character q-gram Jaccard
character q-gram SimHash
fielded BM25 + phrase + proximity
bounded exhaustive semi-global Levenshtein
FrankenOverlap sparse alignment
unified hybrid retrieval
```

It measures micro and macro query AUPRC, Recall@1/5/10, MRR, nDCG, false positives per query, span accuracy, p50/p95/p99, throughput, build time, index size, and break-even query count. Exhaustive work that exceeds a declared dynamic-programming budget is marked incomplete rather than extrapolated.

Evidence output includes:

```text
suite-status.json
proof.json
scores.jsonl
claims.json
bundle/RESULTS.md
bundle/RESULTS.html
bundle/environment.json
bundle/examples.json
bundle/artifacts.json
suite.json
```

Ambiguous natural positives can be reviewed with `fo-adjudicate`; statistical comparisons can be preregistered with `fo-claim-gate`; standalone reports can be rendered with `fo-proof-report`.

See:

- [`docs/REAL_SHOWCASE_SCENARIOS.md`](docs/REAL_SHOWCASE_SCENARIOS.md)
- [`docs/SCENARIO_PROOF_BENCHMARK.md`](docs/SCENARIO_PROOF_BENCHMARK.md)
- [`docs/GOLD_ADJUDICATION.md`](docs/GOLD_ADJUDICATION.md)
- [`docs/PAIRED_CLAIM_GATES.md`](docs/PAIRED_CLAIM_GATES.md)
- [`docs/EVIDENCE_BUNDLES.md`](docs/EVIDENCE_BUNDLES.md)
- [`docs/EVIDENCE_SUITE.md`](docs/EVIDENCE_SUITE.md)

## Accretive learning and deployment

The repository includes:

- held-out hybrid fusion tuning;
- logistic probability calibration;
- query-grouped hard-negative ranking;
- AP-delta-weighted listwise ranking;
- active-learning review queues;
- false-discovery and conformal acceptance policies;
- slice-aware worst-cohort evaluation;
- append-only experiment history;
- atomic corpus-specific profile promotion.

Training, evaluation, claim support, and deployment promotion are separate operations. A new profile can be retained as evidence without being promoted.

See [`docs/HYBRID_TUNING.md`](docs/HYBRID_TUNING.md), [`docs/EXPERIMENT_LEDGER.md`](docs/EXPERIMENT_LEDGER.md), and [`docs/ACTIVE_LEARNING.md`](docs/ACTIVE_LEARNING.md).

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

## Workspace

```text
crates/
  fo-core/          indexing, retrieval, alignment, storage, ranking evidence, metrics
  fo-cli/           overlap, lexical, hybrid, batch, composite, multiview, planner, segment CLIs
  fo-corpus/        Gutenberg/SEC acquisition, manifests, verification, section derivation
  fo-bench/         benchmarks, baselines, adjudication, claims, evidence, tuning, experiments
  fo-conformance/   behavioral, persistence, and corruption contracts
  franken-overlap/  public facade crate

docs/               algorithm, format, benchmark, corpus, evidence, and deployment contracts
fixtures/           deterministic conformance corpus
```

## Correctness and performance doctrine

- Token and vocabulary IDs are categorical labels.
- No corpus-wide edit-distance matrix is used as the normal retrieval path.
- Matches cannot cross document boundaries.
- Approximate candidates do not bypass exact textual verification.
- Persistent index and manifest semantics fail closed.
- Query groups remain intact during tuning, resampling, and evaluation.
- Incomplete exhaustive runs remain explicitly incomplete.
- Public speed or quality claims require a checked-in evidence bundle with corpus, query, commit, compiler, hardware, baseline, quality, span, latency, and uncertainty receipts.

Until those conditions are met, superiority claims are hypotheses rather than project facts.

## Validation

The repository uses the moving latest Rust nightly selected by `rust-toolchain.toml` and does not depend on active GitHub Actions workflows.

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
