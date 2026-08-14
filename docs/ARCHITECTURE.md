# Architecture

## Dependency direction

```text
franken-overlap (public facade)
              │
              ▼
           fo-core
      ┌───────┼────────┐
      │       │        │
    fo-cli  tests  downstream users
              ▲
              │
       fo-conformance
```

`fo-core` owns all matching semantics. The facade re-exports its public API under the user-facing crate name. The CLI remains a thin orchestration layer. Conformance tests depend on public behavior rather than private implementation details.

## `fo-core` modules

- `normalize`: deterministic Unicode normalization and token/byte-offset representation
- `fingerprint`: 128-bit rolling q-grams and deterministic categorical sketch hashes
- `winnow`: robust sparse fingerprint selection
- `index`: builder, immutable in-memory representation, and fail-closed V1 persistence
- `search`: sparse candidate voting, anchor expansion, exact verification, ranking
- `chain`: monotone positional chaining
- `verify`: semi-global and global edit distance
- `spectral`: CountSketch dense correlation and optional FrankenSciPy execution
- `model`: stable public configuration/result types
- `error`: typed failures with path-aware I/O context

## Hot-path constraints

- No DataFrame, JSON, regex, or dynamic object representation inside matching loops.
- Corpus and specimen tokens are contiguous `u32` arrays.
- Fingerprint dictionaries and posting lists are sorted.
- Candidate state is private to a query.
- Exact DP is restricted to candidate windows.
- Dense mode reads back or returns compact peaks when possible.

## Threading

The initial CLI parallelizes corpus file loading and normalization through Rayon. Index construction and query execution are deterministic single-process reference paths. Planned parallel query execution partitions by immutable segment or corpus block and merges private candidate tables.

Nested parallelism is prohibited. A future runtime policy will decide whether cores are assigned across blocks or within a large transform.

## Persistence

V1 is a single immutable `.foidx` file. Save writes to a sibling temporary file, flushes and synchronizes it, and renames it into place. Load validates every structural invariant before returning an index.

The production format will move to immutable segments and an atomic manifest, preserving the same fail-closed semantics.

## Optional numerical backend

The default workspace does not depend on FrankenSciPy at build time. Feature `frankenscipy` adds a commit-pinned `fsci-fft` dependency used by dense correlation. This isolates upstream network/build cost from the sparse engine and keeps the default library portable.

## GPU boundary

GPU work will enter only through FrankenTorch’s sanctioned Metal boundary. Text-specific code must not import `metal-rs` directly. The planned API keeps channel buffers resident, encodes a whole spectral batch in one command buffer, and returns compact candidate records.

## Public stability policy

The V1 public API is intentionally small:

- normalization and configuration types
- index builder/index reader
- search options/results
- spectral options/peaks
- low-level anchor/fingerprint/verifier primitives useful for experimentation

Internal format and ranking policy may evolve before 1.0. Persistent format changes require a new magic/version and explicit migration tooling; silent reinterpretation is forbidden.
