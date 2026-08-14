# Contributing

Contributions are welcome when they preserve the project’s categorical-correctness and evidence-first design.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The optional dense FFT integration is tested separately:

```bash
cargo test -p fo-core --features frankenscipy
```

## Pull requests

A pull request should include:

- the problem and affected invariant
- the algorithmic or systems change
- tests, including a regression fixture for fixes
- retrieval-quality impact
- performance evidence for performance claims
- persistent-format compatibility impact

Do not combine unrelated refactors and algorithm changes. Do not add a dependency merely to avoid a small, auditable implementation; explain why the dependency is worth its compile time, attack surface, and semantic ownership.

## Persistent format

Changes to `.foidx` require a versioned format update, parser tests, corruption tests, and migration/compatibility notes. Never reinterpret existing bytes under changed semantics.
