# Validation Record

This repository was assembled in an environment without a Rust toolchain and without outbound package-network access. The source tree therefore records separately what was verified locally and what the included GitHub Actions workflow must verify after publication.

## Completed locally

- Parsed every `Cargo.toml` with Python's TOML parser.
- Confirmed that every declared workspace member exists and every Rust `mod` declaration resolves to a source file.
- Ran a delimiter-aware Rust source scan across the complete workspace.
- Ran `git diff --check` and Bash syntax validation for `scripts/publish_to_github.sh`.
- Parsed the GitHub Actions workflow as YAML.
- Checked the source tree for placeholder implementations such as `todo!()` and `unimplemented!()`.
- Reimplemented the normalization, 128-bit rolling q-gram hash, rightmost-minimum winnowing, sparse diagonal voting, anchor chaining, and semi-global verification policy in an independent Python smoke harness. The edited observatory fixture ranked `source.txt` first, the edited methodology fixture ranked `origin.txt` first, partial reuse recovered `partial.txt`, and the strict unrelated-text query returned no hit.
- Confirmed that the Git working tree can be exported as a complete Git bundle and source archive.

## Required after publication

The CI workflow runs the authoritative Rust checks:

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets
cargo test -p fo-core --features frankenscipy
```

Formatting is intentionally not used as the first CI gate: compilation and conformance results should remain visible even if a fresh `rustfmt` version proposes cosmetic changes. Run `cargo fmt --all` before the first tagged release and then promote `cargo fmt --all -- --check` to a required gate.

## Reproducibility note

No local claim in this record substitutes for a successful Rust build. The independent behavioral smoke harness validates the fixture design and algorithm translation, while GitHub Actions validates Rust syntax, types, dependency resolution, tests, Clippy diagnostics, and the optional FrankenSciPy integration.
