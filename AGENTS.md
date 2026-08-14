# AGENTS.md

## Mission

Build FrankenOverlap into the fastest trustworthy sparse-spectral textual overlap and approximate-alignment engine available in safe Rust.

## Core invariants

1. Token/code-point IDs are categorical labels. Never use raw numeric proximity as textual similarity.
2. Candidate generators may be approximate; accepted lexical matches must pass normalized-token verification.
3. No corpus-wide edit-distance scan.
4. Never materialize an `N × pattern_length` sliding-window matrix.
5. No match may cross a document boundary.
6. Unknown binary-format semantics fail closed.
7. Performance work requires a baseline, profile, correctness evidence, and after-measurement.
8. Default core remains safe Rust and free of C/C++ runtime dependencies.
9. GPU unsafe/FFI remains inside FrankenTorch’s sanctioned boundary.
10. General FFT primitives belong upstream in FrankenSciPy; text policy remains here.

## Repository map

- `crates/fo-core`: implementation and public evidence types
- `crates/fo-cli`: orchestration only; no duplicate matching logic
- `crates/fo-conformance`: public end-to-end and corruption contracts
- `crates/franken-overlap`: stable facade
- `fixtures`: deterministic behavioral corpus
- `docs`: design and execution contracts

## Required checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p fo-core --features frankenscipy
```

When a check cannot run, report the exact missing tool or dependency. Never state that validation passed based only on inspection.

## Change discipline

- Keep commits focused and explain the algorithmic invariant affected.
- Add a regression fixture before or with every bug fix.
- Preserve deterministic ordering across thread counts.
- Use checked arithmetic for lengths, byte counts, offsets, and format fields.
- Avoid hidden global mutable state.
- Avoid per-token heap allocation and hash lookup in hot loops when a sorted or flat representation works.
- Do not weaken parser bounds to accept malformed artifacts.
- Do not silently change normalization or persistent-format semantics.

## Performance workflow

1. Record corpus, query distribution, hardware, compiler, commit, and feature set.
2. Capture end-to-end and kernel profiles.
3. State the proposed lever and correctness isomorphism.
4. Implement one lever.
5. Run conformance and adversarial fixtures.
6. Measure p50/p95/p99, throughput, allocations, RSS, and retrieval quality.
7. Keep or revert based on the complete result.

## Documentation expectations

Public algorithms require:

- complexity and memory behavior
- assumptions and failure modes
- deterministic tie rules
- exact versus approximate status
- collision/verification treatment
- benchmark evidence before performance claims
