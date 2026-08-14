# FrankenOverlap

**Ultra-fast sparse-spectral textual overlap detection and approximate alignment in safe Rust.**

FrankenOverlap finds where a specimen passage reappears inside a large corpus even when the text has been edited, reformatted, partially copied, or shifted by insertions and deletions. It treats inverted-index voting and FFT cross-correlation as two execution strategies for the same categorical correlation problem, then chains local anchors and spends exact edit-distance work only on the surviving spans.

```text
specimen ── normalize ── q-grams ── winnow ── rare postings ── diagonal votes
                                                                  │
                                                                  ▼
corpus ───── immutable .foidx index ─────────────────────── anchor chaining
                                                                  │
                                                                  ▼
                                             bounded exact verification ── hits
```

## Why this is not “FFT over token IDs”

Unicode code points, BPE IDs, and vocabulary IDs are categorical labels. Their numeric values have no metric meaning: token 50,001 is not inherently closer to token 50,002 than to token 7. Raw numerical correlation over those IDs is therefore not invariant to a harmless vocabulary renumbering.

FrankenOverlap preserves categorical equality instead:

- The indexed engine hashes normalized q-grams into 128-bit fingerprints, retrieves positional postings, and accumulates sparse cross-correlation votes by `corpus_position - query_position`.
- The dense engine uses independently signed CountSketch channels. Equal categories always contribute positively; unequal categories collide only probabilistically and cancel in expectation.
- Anchor chaining allows the best alignment diagonal to change when text is inserted or deleted.
- A semi-global edit-distance verifier confirms candidate spans and prevents hash or sketch collisions from becoming accepted matches.

## Current state

The initial repository contains a coherent, working vertical slice:

- Unicode NFKC normalization, lowercasing, punctuation policy, and whitespace canonicalization
- 128-bit rolling q-gram fingerprints
- rightmost-minimum winnowing
- immutable defensive `.foidx` index serialization
- IDF-weighted sparse diagonal voting
- monotone anchor chaining with drift and concave gap penalties
- partial-span-aware semi-global Levenshtein verification
- short-query direct fallback
- categorical CountSketch dense cross-correlation
- optional FrankenSciPy FFT execution through the `frankenscipy` feature
- `fo index`, `fo query`, `fo inspect`, and `fo scan`
- end-to-end fixtures covering edited passages, partial reuse, Unicode drift, unrelated negatives, persistence equivalence, corruption rejection, and dense peak placement

The codebase is CPU-first and entirely safe Rust. The default build has no C, C++, BLAS, Python, or GPU runtime dependency.

## Quick start

```bash
git clone https://github.com/Dicklesworthstone/franken_overlap
cd franken_overlap
cargo build --release

./target/release/fo index ./my-corpus --output ./my-corpus.foidx
./target/release/fo query ./my-corpus.foidx ./specimen.txt
```

Example output:

```text
1. papers/observatory.txt [1432..1769] score=0.8421 edit=0.8114 coverage=0.9032 distance=31
   the observatory opened its copper shutters under a clear winter sky ...
```

Use JSON for machine pipelines:

```bash
fo query corpus.foidx specimen.txt --json
fo inspect corpus.foidx --json
```

Supply a specimen directly:

```bash
fo query corpus.foidx --text "preserve the raw measurements before comparing causal models"
```

Run dense correlation over one unindexed text:

```bash
fo scan large_document.txt specimen.txt --minimum-score 0.35
```

The default dense path is a bounded direct reference implementation. Enable FrankenSciPy for FFT-backed correlation:

```bash
cargo build --release --features frankenscipy
```

## Rust API

```rust
use franken_overlap::{IndexBuilder, IndexConfig, SearchOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    builder.add_document(
        "paper.txt",
        "Preserve the raw measurements and document every transformation before comparing causal models.",
    )?;
    let index = builder.build()?;
    let hits = index.search(
        "Document each transformation and preserve the raw measurements before comparing causal models.",
        &SearchOptions::default(),
    )?;
    for hit in hits {
        println!("{} {:.3}: {}", hit.path, hit.combined_score, hit.matched_text);
    }
    Ok(())
}
```

For a persistent index:

```rust
index.save("corpus.foidx")?;
let index = franken_overlap::Index::load("corpus.foidx")?;
```

## Search architecture

### 1. Normalize once

Each corpus document and specimen is transformed under an explicit profile:

- optional NFKC normalization
- optional lowercase expansion
- punctuation retained, removed, or mapped to spaces
- optional whitespace collapse

The normalized string and Unicode-scalar token stream are stored together so token ranges can be returned as readable text without rebuilding offsets at query time.

### 2. Select robust fingerprints

For q-gram length `q`, every normalized token window receives two independent rolling 64-bit polynomial hashes. The pair forms a 128-bit fingerprint. Winnowing selects the rightmost minimum fingerprint from each window, sharply reducing index density while preserving local evidence.

### 3. Perform sparse cross-correlation

A specimen fingerprint at position `q` and corpus occurrence at position `c` vote for diagonal `d = c - q`. Votes are grouped by document and quantized diagonal. Rare fingerprints receive greater inverse-document-frequency weight; excessive posting lists are suppressed.

This is cross-correlation evaluated only at nonzero equality products. For a static corpus it usually beats an FFT because the query does not need to read unrelated corpus text at all.

### 4. Chain anchors

Candidate diagonals are expanded back into positional anchors. A dynamic program finds a monotone chain in query/corpus coordinates while penalizing:

- disagreement between query and corpus gap lengths
- large discontinuities
- implausibly long jumps

An insertion or deletion appears as a controlled diagonal change rather than destroying all evidence after the edit.

### 5. Verify exactly

Only chain-supported spans are compared with a bounded semi-global Levenshtein pass. If the band cannot safely contain the optimum, the verifier falls back to the full dynamic program. Hash collisions can create candidates but cannot create accepted exact evidence.

### 6. Rank composite evidence

The current ranking combines:

- verified edit similarity
- fraction of the specimen covered by chained anchors
- normalized chain score
- diagonal-vote support

All components are returned separately so downstream applications can replace the policy without losing evidence.

## Dense categorical correlation

For repetition `r`, every token `x` receives a bucket `b_r(x)` and sign `σ_r(x)`. Channel correlation estimates positional equality:

```text
X[r,b,i] = σ_r(T[i]) when b_r(T[i]) = b, otherwise 0
Y[r,b,j] = σ_r(P[j]) when b_r(P[j]) = b, otherwise 0
```

Summing channel cross-correlations gives an unbiased estimator of categorical overlap. Equal tokens contribute `+1` in every repetition. Unequal tokens contribute only on bucket collision, with expected signed contribution zero.

Dense mode is useful when:

- the corpus is not indexed
- every offset needs a heat-map score
- many query spectra can be amortized
- a GPU can keep channels and FFT scratch resident

It is not the default for repeated queries over a static corpus because sparse postings avoid scanning the corpus.

## Workspace

```text
crates/
  fo-core/          normalization, fingerprints, index, search, chaining, verification, spectral scan
  fo-cli/           `fo` command-line interface
  fo-conformance/   cross-module and corruption tests
  franken-overlap/  public facade crate
fixtures/           deterministic end-to-end corpus/specimens
docs/               algorithms, architecture, format, benchmark and integration contracts
```

## Integration with the Franken numerical stack

FrankenOverlap is a text-specific project rather than an awkward extension of a compatibility library, but it deliberately creates reusable integration seams:

- **FrankenSciPy**: optional FFT correlation backend today; prepared f32 batched overlap-save plans are the next upstream target.
- **FrankenTorch**: planned Apple-Silicon Metal path for resident CountSketch encoding, FFT stages, score reduction, non-maximum suppression, and compact top-k readback.
- **FrankenNumPy**: planned typed zero-copy array interchange, prefix sums, and safe candidate-span views.
- **FrankenPandas**: planned corpus-manifest ingestion and result DataFrame/export layer, outside the hot loop.

See [`docs/UPSTREAM_INTEGRATION.md`](docs/UPSTREAM_INTEGRATION.md).

## Performance doctrine

No algorithm is universally optimal. FrankenOverlap will maintain a measured portfolio:

| Workload | Intended route |
|---|---|
| Very short specimen | direct SIMD / bit-parallel scan |
| Static corpus, repeated queries | sparse winnowed index |
| Very frequent features | suppression or dense heavy-feature path |
| Unindexed corpus | dense CountSketch correlation |
| Long edited passage | sparse anchors + chaining + exact verifier |
| Final candidate | banded/Wavefront-style exact alignment |

Every performance change must preserve a correctness oracle and report throughput, p50/p95/p99 latency, peak RSS, index density, candidate recall, and final precision. See [`docs/BENCHMARK_PLAN.md`](docs/BENCHMARK_PLAN.md).

## Known limitations of the initial version

- Similarity is lexical, not a claim of semantic equivalence. A paraphrase with entirely different vocabulary requires a separate semantic-anchor layer.
- The exact verifier is a safe-Rust semi-global Levenshtein implementation, not yet Myers/WFA portfolio dispatch.
- The on-disk format currently stores normalized document text directly; block compression and mmap-backed postings are planned.
- Dense FFT mode currently constructs channels independently. Prepared spectra, f32 batching, channel packing, overlap-save, and GPU-resident top-k are planned.
- Original-source byte offsets are not yet persisted; returned offsets index normalized Unicode scalars.
- Index updates currently rebuild an immutable file; LSM-style segments and compaction are planned.

These are sequencing limits, not scope cuts. The intended terminal system is described in [`COMPREHENSIVE_PLAN_FOR_FRANKEN_OVERLAP.md`](COMPREHENSIVE_PLAN_FOR_FRANKEN_OVERLAP.md).

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p fo-core --features frankenscipy
```

The binary format is fail-closed: bad magic, unknown versions or flags, unsorted dictionaries, invalid postings, inconsistent document frequencies, impossible sizes, and trailing bytes are rejected.

## License

MIT License with the repository’s OpenAI/Anthropic rider. See [`LICENSE`](LICENSE).
