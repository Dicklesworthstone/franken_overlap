# Changelog

All notable changes to FrankenOverlap are recorded here.

## Unreleased

### Added

- Tie-correct native AUPRC, precision-recall, best-threshold, Brier, log-loss, ECE, and MCE evaluation.
- `fo-bench` evaluation and deterministic edited-text benchmark crate.
- Conventional exact-substring, character q-gram Jaccard, and SimHash benchmark baselines.
- Explicit `any_passage`, `source_attribution`, and `near_duplicate` search intents.
- Rich search evidence including query/source coverage, matched length, vote support, chain consistency, anchor count, and estimated false matches.
- Exact Myers bit-vector infix matching for short specimens.
- Compact band-local semi-global verification with geometric widening for longer candidates.
- Linear KMP exact fallback and bounded sampled candidate generation.
- Append-only accepted/rejected feedback records.
- Deterministic regularized logistic calibration, held-out AUPRC gates, and calibrated reranking.
- Deterministic end-to-end AUPRC and Recall@1 quality-floor integration test.

### Changed

- Query fingerprints are grouped once and processed rarest-first.
- Candidate voting now uses two shifted diagonal grids to reduce bin-boundary failures.
- Source-attribution scoring now penalizes insufficient specimen coverage and insignificant local fragments.
- The formerly quadratic short-query fallback is explicitly work-bounded.
- The README and quality documentation now describe the implemented system rather than the initial prototype.

## 0.1.0 - 2026-08-14

### Added

- Safe-Rust workspace and public `franken-overlap` facade.
- Unicode normalization with explicit punctuation and whitespace policies.
- 128-bit rolling q-gram fingerprints and rightmost-minimum winnowing.
- Immutable defensive `.foidx` V1 index.
- IDF-weighted sparse diagonal voting.
- Monotone anchor chaining with drift/gap penalties.
- Partial-span-aware semi-global exact verification.
- Short-query direct fallback.
- Dense categorical CountSketch correlation.
- Optional commit-pinned FrankenSciPy FFT backend.
- `fo index`, `query`, `inspect`, and `scan` commands.
- End-to-end, persistence, Unicode, negative, corruption, and dense-correlation tests.
- Architecture, algorithm, format, integration, benchmark, and execution-plan documentation.
