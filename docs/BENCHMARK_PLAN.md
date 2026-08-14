# Benchmark and Evaluation Plan

## Rule zero

Latency without retrieval quality is not a useful result. Every benchmark report pairs speed and memory measurements with candidate recall and final span accuracy.

## Corpora

### Synthetic controlled-edit corpus

Generate passages with seeded edits:

- substitutions at 0–40%
- insertion/deletion bursts
- affine long gaps
- punctuation/case/Unicode width changes
- whitespace and line-wrap changes
- OCR confusion maps
- random prefix/suffix additions
- partial coverage from 10–100%
- moved paragraph blocks

Ground truth records source span and edit operations.

### Natural corpora

- source-code repository history
- scientific papers and preprints
- legal/regulatory filings
- news/web crawl samples
- OCR books/newspapers
- mixed-language Unicode text

### Adversarial corpora

- repeated single characters
- boilerplate templates
- generated tables
- highly repetitive source code
- cross-document boundary traps
- deliberately colliding reduced hashes in test-only configurations

## Baselines

- exact substring search/ripgrep
- naïve per-window Levenshtein
- Myers/bit-parallel implementation
- conventional fuzzy-match library
- MinHash/SimHash candidate retrieval
- suffix-array/FM-index exact seeds where applicable
- dense FFT one-hot or sketch implementation
- WFA verifier on the same candidates

## Metrics

### Retrieval

- top-1/top-10/top-100 candidate recall
- final document precision/recall
- span intersection-over-union
- normalized endpoint error
- partial-overlap coverage calibration
- false positives per GiB

### Performance

- indexing MiB/s and million normalized tokens/s
- query p50/p95/p99
- dense scan GiB/s
- exact verification candidates/s
- index bytes per normalized token
- peak RSS
- scratch bytes per worker
- allocations/query
- CPU cycles, instructions, branch misses, cache misses
- CPU package and GPU energy where available

## Hardware matrix

- Apple M4/M5 family: efficiency/performance cores and Metal GPU
- high-core-count AMD Zen 4/5/6
- mainstream x86 AVX2
- AVX-512 capable server
- ARM NEON server/desktop

Every artifact records CPU/GPU model, core count, memory, OS, compiler, commit, features, normalization profile, index parameters, and corpus digest.

## Experiments

1. q-gram length versus recall/index density
2. winnow window versus postings/query latency
3. posting cap versus boilerplate false positives
4. diagonal bin width versus edit tolerance
5. chain lookback/gap policy versus moved/inserted text
6. verification band versus exactness/work
7. CountSketch buckets/repetitions versus estimator error
8. direct versus FFT crossover by `N`, `m`, and batch size
9. block-level versus intra-FFT parallelism
10. CPU versus Metal crossover including transfer/readback
11. compressed versus fixed-width postings
12. mmap cold/warm-cache behavior

## Acceptance gates

A candidate optimization lands only if:

- deterministic conformance passes
- exact-match recall remains 100% for the supported normalization profile
- edited-passage top-k recall does not regress beyond a declared tolerance
- corruption tests remain fail-closed
- p95 and peak RSS stay within budget
- benchmark methodology and raw output are committed

Microbenchmark wins that worsen end-to-end query latency are rejected.
