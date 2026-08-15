# Validation Contract

FrankenOverlap uses the moving latest Rust nightly toolchain declared in `rust-toolchain.toml`. The repository does not depend on GitHub Actions; validation is performed on owner-controlled local or `rch` workers.

## Required development gate

```bash
./scripts/ci-local.sh quick
```

This runs:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo test --workspace`

## Required pre-merge and release gate

```bash
FO_UPDATE_NIGHTLY=1 ./scripts/ci-local.sh full
```

Full mode updates only nightly, then runs the development gate plus:

- workspace Clippy with `-D warnings`
- FrankenSciPy-enabled check and tests
- FrankenSciPy-enabled Clippy with `-D warnings`

Set `FO_USE_RCH=1` to execute Cargo compilation and tests through `rch`.

## Evidence

A change is mergeable only when the exact commit under review passes the required local gate. Benchmark-sensitive changes must additionally retain their raw benchmark output and corpus/configuration digest as described in `docs/BENCHMARK_PLAN.md`.

The index format remains fail-closed: malformed magic, unknown versions or flags, unsorted dictionaries, invalid postings, inconsistent document frequencies, impossible sizes, and trailing bytes are rejected.
