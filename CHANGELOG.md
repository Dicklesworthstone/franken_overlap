# Changelog

All notable changes to FrankenOverlap are recorded here.

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
