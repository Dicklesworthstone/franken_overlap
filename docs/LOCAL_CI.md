# Local CI on Rust Nightly

Development validation runs on owner-controlled machines with the pinned Rust
nightly toolchain selected by `rust-toolchain.toml`. GitHub Actions additionally
builds, tests, packages, and publishes Linux x86_64 CLI releases through
`.github/workflows/release.yml`.

## Binary releases

Push a `vMAJOR.MINOR.PATCH` tag, manually dispatch the release workflow with a
version tag, or update `RELEASE_VERSION` on main. A release workflow change on
main also retries the requested release. Use a new version for new source after
a release has been published; never move a published tag or replace its binaries.

The workflow uses the pinned nightly and `Cargo.lock`, builds all executables
from `fo-cli` and `fo-corpus` on Ubuntu 22.04, and normalizes archive order,
ownership, permissions, and timestamps. It tests the default and optional
spectral backends, executes an edited-passage search from the extracted archive,
and verifies checksums and executable behavior after downloading the official
release asset. The GNU/Linux binary requires glibc 2.35 or newer.

Archive metadata is reproducible for a fixed input build. This is not a claim
that independent Rust/compiler builds are byte-identical.

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
