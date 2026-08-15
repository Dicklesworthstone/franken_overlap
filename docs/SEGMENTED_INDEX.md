# Immutable Segmented Indexes

A monolithic `.foidx` is optimal for a static corpus, but rebuilding it for every new document prevents an index from becoming continuously more valuable. `SegmentedIndex` adds an append-only manifest over ordinary immutable `.foidx` files.

## Properties

- New batches create new immutable segment files.
- Every document receives a stable 64-bit global ID.
- Active document paths are unique.
- Deletes are logical tombstones and become visible atomically through the manifest.
- Search loads one segment at a time and never retains every decoded segment simultaneously.
- Compaction rewrites active normalized documents into one segment while preserving global IDs.
- Segment files retain the existing fail-closed `.foidx` parser.
- The manifest records byte lengths, FNV-1a content hashes, physical statistics, generation, and exact local/global document mappings.

## Create and append

```bash
cargo run -p fo-cli --bin fo-segment -- create ./corpus.foindex
```

Append JSONL:

```json
{"path":"papers/a.txt","contents":"The full UTF-8 document..."}
{"path":"papers/b.txt","contents":"Another document..."}
```

```bash
cargo run -p fo-cli --bin fo-segment -- append \
  ./corpus.foindex documents.jsonl
```

A deleted path may later be appended again; it receives a new global ID while the old record remains as history.

## Search

```bash
cargo run -p fo-cli --bin fo-segment -- search \
  ./corpus.foindex specimen.txt \
  --intent source-attribution \
  --json
```

Each result includes:

- stable global document ID,
- physical segment ID,
- segment-local raw `SearchResult`,
- a cross-segment fused score.

The fused score emphasizes verification, coverage, chain consistency, matched length, and false-match evidence so a tiny newly appended segment does not win solely because its local document-frequency estimates are smaller.

## Delete and compact

```bash
cargo run -p fo-cli --bin fo-segment -- delete \
  ./corpus.foindex papers/b.txt

cargo run -p fo-cli --bin fo-segment -- compact \
  ./corpus.foindex
```

Compaction first writes and fsyncs the new segment, then atomically replaces the manifest, and only afterward removes old files. Failure before the manifest swap leaves the old generation authoritative. Cleanup failures after the swap are reported as harmless orphan files.

## Integrity verification

```bash
cargo run -p fo-cli --bin fo-segment -- verify ./corpus.foindex
```

Verification checks:

- manifest schema and generation invariants,
- safe segment filenames,
- segment byte lengths and content hashes,
- `.foidx` structural validation,
- configuration and physical-statistic agreement,
- active local/global document mappings and paths.

## Concurrency

Readers operate on the generation they opened. Writers acquire an exclusive `create_new` lock file and verify that the on-disk generation still equals their loaded generation before mutation. A stale writer therefore fails rather than overwriting a newer manifest. If a process is forcibly terminated while holding the lock, the operator must inspect and remove `.writer.lock` before another write.

## When to compact

Compaction is useful when:

- segment count increases query-open overhead,
- many physical documents are tombstoned,
- local segment document frequencies create excessive score variation,
- old immutable files should be reclaimed.

The manifest exposes active, deleted, and physical document counts so a service can trigger compaction using explicit policy rather than hidden background behavior.
