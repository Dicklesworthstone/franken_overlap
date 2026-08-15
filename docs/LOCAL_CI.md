# Local CI on Rust Nightly

FrankenOverlap does not rely on GitHub Actions. Validation runs on owner-controlled machines with the moving latest Rust nightly toolchain selected by `rust-toolchain.toml`.

## Development validation

```bash
./scripts/ci-local.sh quick
```

This runs formatting, workspace checks, and workspace tests.

## Pre-merge and release validation

```bash
FO_UPDATE_NIGHTLY=1 ./scripts/ci-local.sh full
```

`FO_UPDATE_NIGHTLY=1` updates only the selected nightly toolchain, not stable or any unrelated toolchain. Full mode adds Clippy with warnings denied and the optional FrankenSciPy backend's checks and tests.

## Remote compilation workers

To send Cargo compilation and test commands through `rch` while keeping formatting and repository checks local:

```bash
FO_USE_RCH=1 ./scripts/ci-local.sh full
```

The remote worker must have the selected nightly toolchain and repository dependencies available.

## Toolchain prerequisites

The routine validator never installs or updates toolchains unless `FO_UPDATE_NIGHTLY=1` is explicitly set. If nightly or its components are missing:

```bash
rustup toolchain install nightly --profile minimal --component rustfmt,clippy
```

## Dependency locking

If `Cargo.lock` exists, all Cargo commands use `--locked`. If it is absent, Cargo may create it during validation; commit it for reproducible application and CLI builds.
