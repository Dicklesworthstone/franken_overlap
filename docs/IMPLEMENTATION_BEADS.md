# Implementation Beads

This file is a ready-to-import work breakdown. IDs are stable labels, not claims that a specific issue tracker has already been populated.

## Foundation

- **FO-001** Add source-byte offset maps to normalization and result records.
- **FO-002** Add deterministic corpus/normalization manifest digest.
- **FO-003** Add fuzz targets for `.foidx`, normalization, rolling q-grams, chaining, and verifier.
- **FO-004** Add differential Python oracle for normalization and edit distance.
- **FO-005** Add criterion and end-to-end benchmark binaries with machine fingerprinting.

## Sparse index

- **FO-101** Replace `HashMap` build aggregation with sorted run generation for bounded-memory indexing.
- **FO-102** Design checksummed section-directory V2 format.
- **FO-103** Add inline singleton/doubleton postings.
- **FO-104** Add delta-coded block postings with skip pointers.
- **FO-105** Add mmap reader behind a sanctioned reviewed boundary.
- **FO-106** Add low-complexity masks and posting entropy statistics.
- **FO-107** Implement heavy-light feature dispatch.
- **FO-108** Add immutable multi-segment manifest query.
- **FO-109** Add segment compaction and crash-safe manifest publication.

## Query and chaining

- **FO-201** Add multiple chains per document.
- **FO-202** Add concave-gap accelerated chaining data structure.
- **FO-203** Add paragraph transposition/multi-block evidence graph.
- **FO-204** Calibrate composite ranking on controlled edits.
- **FO-205** Add explanation/CIGAR-like result provenance.
- **FO-206** Add language-aware lexical token feature family.
- **FO-207** Add spaced q-gram and multiple-winnow feature families.

## Verification

- **FO-301** Implement short-pattern Shift-Or exact search.
- **FO-302** Implement safe-Rust Myers bit-vector edit distance.
- **FO-303** Implement safe-Rust Wavefront Alignment score-only mode.
- **FO-304** Add affine-gap traceback and linear-memory mode.
- **FO-305** Benchmark verifier policy crossover and persist calibration.

## FrankenSciPy

- **FO-401** Specify prepared f32 correlation API upstream.
- **FO-402** Add batched `rfft_into`/`irfft_into` and caller scratch.
- **FO-403** Add prepared specimen spectrum reuse.
- **FO-404** Add overlap-save streaming plan.
- **FO-405** Add channel packing and fused multiply-accumulate.
- **FO-406** Add fused local maxima/top-k output.

## FrankenTorch / Metal

- **FO-501** Add resident spectral context and buffers.
- **FO-502** Add CountSketch encode MSL kernel.
- **FO-503** Add batched Stockham FFT kernels.
- **FO-504** Add fused spectral reduction/NMS/top-k kernel.
- **FO-505** Add one-command-buffer `SpectralBatch`.
- **FO-506** Add async double buffering and CPU/GPU policy calibration.

## Interoperability

- **FO-601** Add PyO3 bindings and typed result objects.
- **FO-602** Add FrankenNumPy zero-copy token/score buffers.
- **FO-603** Add FrankenPandas result DataFrame conversion/export.
- **FO-604** Add Arrow/Parquet corpus manifest ingestion.
- **FO-605** Add optional semantic phrase-anchor plugin with explicit provenance.
