# Comprehensive Plan for FrankenOverlap

## 1. Mission

FrankenOverlap will become a hardware-adaptive engine for locating and aligning reused text across corpora ranging from one large document to multi-terabyte archives. Its defining abstraction is **categorical positional correlation**: sparse inverted-index voting and dense FFT correlation are not unrelated tricks, but alternative evaluation strategies for the same equality kernel.

The system must find:

- exact copied passages
- passages with substitutions, spelling changes, punctuation/case/width drift, and OCR noise
- passages disrupted by insertions and deletions
- partial reuse where only part of the specimen occurs
- multiple moved or reordered blocks
- optional semantic reuse when lexical evidence is weak

It must do so with predictable resource bounds, deterministic index artifacts, machine-readable evidence, exact final verification, and profile-proven execution on high-core-count CPUs and Apple-Silicon GPUs.

## 2. Non-negotiable principles

1. **Categorical correctness.** Raw token or Unicode IDs are labels, never scalar coordinates.
2. **Sparse before dense.** For an indexed corpus, avoid reading text that cannot match.
3. **Candidate generation is not truth.** Hashes and sketches may nominate spans; exact normalized-token verification accepts them.
4. **No quadratic corpus scan.** Quadratic or dynamic-programming work is restricted to bounded candidate windows.
5. **No single-algorithm dogma.** Runtime policy chooses direct, bit-parallel, sparse, spectral, or wavefront paths from measured costs.
6. **Evidence is explicit.** Return anchor coverage, chain score, vote support, sketch score, edit cost, and provenance separately.
7. **Safe Rust core.** Unsafe code may exist only behind already-sanctioned GPU or mmap boundaries in upstream libraries.
8. **Immutable reproducibility.** Index manifests pin normalization, tokenizer identity, q-gram parameters, hash seeds, format version, and source fingerprints.
9. **Performance claims require recall.** A faster candidate generator that misses edited passages is a regression.
10. **Partial delivery is sequencing, not permanent scope reduction.** Every temporary gap maps to a documented closure item.

## 3. Mathematical model

For specimen tokens `P[0..m)` and corpus tokens `T[0..n)`, the exact weighted positional overlap at offset `s` is

```text
M(s) = Σ_j w(P[j]) · 1[T[s+j] = P[j]].
```

The one-hot expansion is

```text
M(s) = Σ_a corr(1[T=a], w(a)·1[P=a])(s).
```

This formulation is invariant under arbitrary renumbering of the token vocabulary.

### 3.1 Sparse realization

For feature `a`, let `Q_a` and `C_a` be its specimen and corpus positions. Every pair contributes to diagonal `s = c - q`:

```text
M(s) = Σ_a w(a) Σ_{q∈Q_a} Σ_{c∈C_a} 1[s=c-q].
```

An inverted index evaluates only nonzero products. Winnowing and frequency-aware selection reduce the number of positions while retaining local exact evidence.

### 3.2 Dense realization

CountSketch maps each token to a bucket and random sign for each repetition. Summed channel correlation is unbiased for categorical equality. More buckets reduce collision probability; more repetitions reduce variance.

### 3.3 Edit tolerance

A single offset is insufficient after an insertion or deletion. Local matches become anchors `(query_position, corpus_position, span, weight)`. A monotone chain allows the diagonal `corpus_position - query_position` to change gradually. A concave gap penalty makes one long insertion cheaper than many unrelated discontinuities.

### 3.4 Exact verification

Candidate chains define bounded query and corpus intervals. The verifier portfolio will contain:

- direct SIMD equality/Hamming kernels
- Myers bit-vector edit distance
- banded semi-global Levenshtein
- safe-Rust Wavefront Alignment with linear-memory modes
- optional traceback only for returned hits

The initial implementation includes the banded semi-global path and global fallback.

## 4. Data model

### 4.1 Normalization profile

A profile records:

- Unicode normalization form
- case policy
- whitespace policy
- punctuation policy
- optional accent/diacritic policy
- optional number, URL, email, and identifier placeholders
- tokenizer or segmentation revision

Normalization must produce both tokens and an offset map back to the source bytes. V1 stores normalized-token offsets; source-offset persistence is a near-term format extension.

### 4.2 Feature families

The terminal engine combines independent views:

- Unicode-scalar q-grams
- normalized UTF-8 byte q-grams
- lexical word q-grams
- spaced q-grams for local corruption tolerance
- optional pinned BPE q-grams
- sentence and paragraph boundaries
- structured classes such as number/entity/identifier placeholders
- optional semantic phrase embeddings

Feature families receive separate frequency statistics and calibration weights.

### 4.3 Corpus segments

The scalable format is an immutable segment family:

```text
segment/
  manifest
  documents
  normalized_text_blocks
  source_offset_maps
  fingerprint_dictionary
  postings
  frequency_statistics
  optional_prepared_spectra
  integrity_ledger
```

New documents create a new segment. An atomic manifest publishes the segment set. Queries fan out across segments; background compaction merges them. Readers retain old manifests until quiescence.

## 5. Runtime portfolio

The policy controller estimates costs before dispatch:

```text
C_direct ≈ α · N · ceil(m / SIMD_width)
C_bit    ≈ β · N · ceil(m / machine_word_bits)
C_sparse ≈ γ · Σ_a |Q_a||C_a|
C_fft    ≈ δ · repetitions · buckets · L log L
C_verify ≈ ε · candidate_count · expected_band_work
```

The constants are calibrated per host and stored with CPU model, core count, cache topology, memory bandwidth, operating system, compiler revision, and optional GPU identity.

### 5.1 Dispatch inputs

- specimen length
- corpus indexed/unindexed status
- selected feature count
- posting-list lengths and entropy
- expected edit rate
- requested dense heat map versus top-k hits
- batch size and query repetition
- CPU/NUMA/GPU availability
- latency versus throughput objective
- configured memory cap

### 5.2 Parallelism policy

For many corpus blocks or segments, parallelize across independent blocks. Parallelize inside one FFT only when block-level work cannot occupy the machine. Never allow nested pools to oversubscribe a 64–128-core host.

NUMA execution pins index partitions and worker scratch to local domains. Candidate accumulators are worker-private and merged after voting; no globally contended hash table sits in the hot loop.

## 6. Sparse engine roadmap

### Phase S1: current vertical slice

- 128-bit rolling q-grams
- winnowing
- fixed-width postings
- IDF voting
- diagonal bins
- anchor chaining
- exact verification

### Phase S2: postings performance

- sorted fingerprint dictionary with succinct offset directory
- inline singleton/doubleton postings
- delta-coded document IDs and positions
- block compression with skip pointers
- mmap-backed read-only access
- posting-list caps and adaptive rare-feature ordering
- per-query arena allocation

### Phase S3: heavy-light exact correlation

For each feature estimate `|Q_a||C_a|`:

- rare: enumerate pairs
- medium: block/bucket correlation
- heavy: dense bitset or FFT channel
- uninformative: suppress or cap

This feature-by-feature dispatch is the deepest form of the sparse/dense unification.

### Phase S4: robust seeds

- multiple independent winnowing streams
- syncmers/minimizers comparison
- spaced q-grams
- low-complexity masking
- OCR-oriented character confusion classes
- language-specific lexical views

## 7. Spectral engine roadmap

### Phase F1: current reference path

- signed categorical channels
- exact direct evaluation under a work cap
- optional FrankenSciPy `fftcorrelate`
- local maxima and top-k extraction

### Phase F2: prepared CPU plans in FrankenSciPy

Add reusable primitives upstream:

- `PreparedCorrelationPlanF32`
- `PreparedKernelSpectrumF32`
- `CorrelationWorkspaceF32`
- batched `rfft_into` / `irfft_into`
- overlap-save plans
- two-real-channel complex packing
- fused conjugate multiply/accumulate
- fused threshold/non-maximum suppression/top-k
- thread-local scratch keyed by `(length, batch, dtype)`

The specimen spectrum and all scratch must be reusable. The API must permit score extraction without materializing a full output vector.

### Phase F3: Metal through FrankenTorch

Create a spectral batch analogous to FrankenTorch’s resident fused execution:

1. encode q-grams/tokens to CountSketch channels
2. perform batched Stockham FFT stages
3. multiply prepared spectra
4. inverse transform
5. reduce repetitions/channels
6. normalize scores
7. perform non-maximum suppression
8. compact top-k peaks
9. read back only candidate records

Persistent shared buffers and one command buffer per batch are mandatory. The synchronous generic gateway is an integration proof, not the terminal throughput path.

### Phase F4: multi-query amortization

- cache corpus block spectra for stable corpora
- batch many specimen spectra
- choose query-major or corpus-major scheduling from cache residency
- spill prepared spectra under an explicit admission policy

## 8. Chaining and rearrangement

The chain state will evolve from the current one-best dynamic program to:

- multiple chains per document
- concave affine-like gap models
- endpoint bonuses and overlap handling
- sparse Fenwick/segment-tree acceleration
- beam preservation of alternate diagonals
- paragraph transposition detection as multiple ordered chains
- optional block-reordering graph

A returned hit should explain which specimen intervals were supported, which intervals were inserted/deleted, and whether evidence forms one contiguous chain or several moved blocks.

## 9. Exact verifier roadmap

### V1

Current semi-global banded Levenshtein with full fallback.

### V2

Myers bit-vector kernels specialized by pattern length and machine word count. AVX2, AVX-512, NEON, and portable scalar paths must be selected behind one safe interface.

### V3

Safe-Rust Wavefront Alignment supporting edit, gap-linear, and gap-affine costs, score-only and traceback modes, and low-memory bidirectional reconstruction.

### V4

Batch verification policy for GPU only when enough independent candidates exist to occupy the device. Irregular small candidates remain CPU work.

## 10. Semantic extension

Semantic retrieval is a separate evidence lane, not a relaxation of lexical correctness.

- Precompute sentence/phrase embeddings.
- Use ANN search to propose semantic anchors.
- Retain positions and chain anchors just like lexical evidence.
- Calibrate semantic and lexical scores separately.
- Require stronger verification or human-visible provenance for semantic-only matches.

The system must never label a semantic paraphrase as a textual overlap without saying which evidence lane produced it.

## 11. Interfaces

### CLI

- `fo index`
- `fo query`
- `fo inspect`
- `fo scan`
- future `fo append`, `fo compact`, `fo benchmark`, `fo calibrate`, `fo explain`

### Rust

- immutable index builder/reader
- query plan and evidence-rich result structs
- streaming corpus scanner
- pluggable normalizer and tokenizer traits
- verifier trait
- runtime policy trait

### Python

A PyO3 layer will expose:

- zero-copy or bounded-copy NumPy buffers
- pandas/FrankenPandas result tables
- iterator/stream APIs for large result sets
- deterministic configuration serialization

## 12. Reliability and security

- bounded file sizes, document counts, token counts, entries, and postings
- checked arithmetic for all format lengths and offsets
- strict dictionary and posting ordering validation
- source document boundary enforcement
- no candidate windows crossing documents
- hash collisions always subject to exact verification
- randomized sketches use recorded deterministic seeds
- malformed artifacts fail closed
- fuzz targets for normalization, index parser, q-gram rolling hash, chain logic, and verifier
- differential tests against reference Python/third-party algorithms
- RaptorQ sidecars can be added for durable large index manifests after the core format stabilizes

## 13. Benchmark and quality gates

Every release candidate runs:

- exact duplicate retrieval
- controlled substitutions/insertions/deletions
- OCR confusion corpus
- punctuation/case/Unicode normalization corpus
- partial overlap at varying coverage
- repeated/low-entropy adversarial corpus
- cross-document boundary probes
- random unrelated negatives
- hash-collision and sketch-collision stress
- persistence corruption corpus

Primary gates:

- candidate recall at fixed top-k
- final span precision/recall
- offset error
- throughput in normalized GiB/s and million tokens/s
- query p50/p95/p99
- index bytes per normalized token
- peak RSS and scratch bytes per worker
- build throughput
- deterministic result equivalence across thread counts

No speed result is publishable without the corresponding recall/precision result.

## 14. Upstream ownership

- Text normalization, feature policy, postings, chaining, and result semantics remain in FrankenOverlap.
- General-purpose prepared correlation primitives belong in FrankenSciPy.
- General-purpose Metal resident compute/batching belongs in FrankenTorch.
- Array interoperability belongs in FrankenNumPy.
- tabular ingestion/export belongs in FrankenPandas.

This prevents the project from duplicating numerical infrastructure while keeping text-specific policy out of compatibility libraries.

## 15. Execution waves

### Wave 0: repository foundation

Complete in the initial scaffold: coherent crates, CLI, tests, binary format, docs, CI, publish tooling.

### Wave 1: performance baseline

- Criterion and end-to-end benchmark harness
- synthetic corpus generator
- CPU hardware fingerprint
- allocation and flamegraph capture
- baseline against ripgrep exact search, SimHash/MinHash candidates, edit-distance scan, and a conventional fuzzy matcher

### Wave 2: sparse productionization

- compressed mmap postings
- source offset maps
- parallel segment query
- short-query Myers
- low-entropy policy

### Wave 3: prepared spectral CPU

- upstream FrankenSciPy f32 batched API
- overlap-save streaming
- channel packing
- fused top-k

### Wave 4: Metal

- resident spectral batch
- async double buffering
- CPU/GPU policy calibration

### Wave 5: exact alignment portfolio

- Myers and WFA
- traceback/explanation
- multiple-chain rearrangement support

### Wave 6: incremental corpus and Python

- immutable segments, manifest publication, compaction
- PyO3 and FrankenPandas integration

### Wave 7: semantic anchors

- optional embedding index
- calibrated hybrid ranking
- explicit semantic provenance

## 16. Definition of success

FrankenOverlap succeeds when a user can point it at a large heterogeneous corpus, submit an edited passage, and receive the correct source spans with explainable evidence at interactive latency; when corpus-scale throughput is limited primarily by storage/memory bandwidth rather than object allocation; when high-core CPUs and Apple GPUs are both used only where they win; and when exact verification makes every accepted lexical match reproducible and defensible.
