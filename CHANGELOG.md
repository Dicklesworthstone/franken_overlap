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
- `fo query` flags for intent, coverage and matched-length floors, fallback work budgets, fitted calibration models, and calibrated-probability thresholds.
- End-to-end CLI tests for query intents and fail-closed calibration requirements.
- Composite fragmented-source aggregation that combines non-overlapping passages, measures union coverage, and detects reordered blocks.
- `fo-composite` CLI for source attribution and near-duplicate search across moved or interrupted passages.
- Query-grouped pairwise ranking over hard positive/negative comparisons, with persisted feature standardization and held-out AUPRC reports.
- Deterministic hard-negative mining and the `fo-rank` fit, compare, mine, and rerank CLI.
- Exact positional-equality dense scoring below the configured direct-work crossover.
- Unit-circle phase-sketch FFT correlation requiring two channels per repetition instead of one channel per bucket.
- Persisted multi-view indexes spanning short, balanced, and selective q-gram scales.
- Cross-view span consensus with support ratios, disagreement penalties, weighted evidence, and balanced/high-recall/high-precision presets.
- `fo-multiview` build, query, and inspect CLI.
- Query-grouped macro AUPRC, expected tie-aware Recall@k/MRR/nDCG, and deterministic query-bootstrap confidence intervals.
- Constraint-based operating-threshold selection over every distinct score.
- `fo-group-eval` CLI for grouped quality reports and production operating points.
- Evidence-diverse active-learning selection combining uncertainty, model disagreement, hard-negative risk, and feature-space novelty.
- Query/document diversity caps, duplicate suppression, and recommended feedback weights.
- `fo-active` CLI for generating compact high-value review queues from unlabeled candidate streams.

### Changed

- Query fingerprints are grouped once and processed rarest-first.
- Candidate voting now uses two shifted diagonal grids to reduce bin-boundary failures.
- Source-attribution scoring now penalizes insufficient specimen coverage and insignificant local fragments.
- The formerly quadratic short-query fallback is explicitly work-bounded.
- Human-readable query output now exposes the evidence needed to audit each ranking decision.
- The README and quality documentation now describe the implemented system rather than the initial prototype.
- Removed the obsolete stable/MSRV recovery wrapper; repository validation remains nightly-first.
- Dense direct scans are now exact and independent of sketch parameters.
- Default FFT channel correlations fall from repetitions × buckets to 2 × repetitions.
- Multi-view consensus penalizes single-scale accidents while retaining a dedicated high-recall preset for noisy text.
- Quality gates can now distinguish global candidate-stream gains from broadly distributed per-query gains.
- Feedback acquisition now targets the current decision boundary and newly discovered failure modes instead of repeatedly sampling easy examples.

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
