# `.foidx` Version 2: Delta-Varint Postings

Version 1 stores every posting as two fixed-width little-endian `u32` values. That is simple and fast to decode, but it spends eight bytes per `(document_id, position)` even though both values are sorted and usually change by small amounts.

Version 2 keeps the same logical index while encoding posting deltas as unsigned varints and appending a whole-file checksum.

## Compatibility

- Version 1 magic: `FROV0001`
- Version 2 magic: `FROV0002`
- `Index::load_auto` reads both versions.
- `Index::load` remains the strict legacy-v1 API.
- `Index::save_compressed` writes v2.
- `Index::save_with_options` explicitly selects either format.
- The primary `fo index` CLI writes v2 by default; `--storage legacy-fixed` is available for old consumers.

## Posting encoding

Posting lists are strictly sorted by document ID and then position. Each posting writes two unsigned LEB128-style varints:

1. document ID delta from the previous posting, or the absolute document ID for the first posting,
2. absolute position for the first posting in a document, otherwise the position delta within that document.

Each dictionary entry stores the exact encoded payload length. The loader must decode exactly the declared posting count while consuming exactly that byte length. It rejects:

- truncated or overlong varints,
- `u64`, `u32`, document-ID, or position overflow,
- unsorted or duplicate postings,
- missing documents,
- positions outside the document's valid q-gram range,
- impossible document-frequency counts,
- unconsumed payload bytes.

## File checksum

Version 2 appends a 64-bit FNV-1a checksum over every preceding byte, including the header, document strings, dictionary, and encoded postings. This is intended to detect accidental corruption and incomplete transfers; it is not a cryptographic authentication primitive.

The loader verifies the checksum and rejects any trailing bytes.

## Storage statistics

`IndexFileStats` reports:

- physical file bytes,
- posting payload bytes,
- equivalent v1 fixed posting bytes,
- posting payload compression ratio,
- stored checksum.

```bash
fo inspect corpus.foidx --json
```

The posting compression ratio isolates the posting payload rather than claiming that normalized source text or dictionary metadata is compressed.

## CLI

```bash
fo index ./corpus --output corpus.foidx
```

writes checksummed delta-varint v2.

```bash
fo index ./corpus \
  --output legacy.foidx \
  --storage legacy-fixed
```

writes version 1.

## Memory and decoding

Serialization computes encoded lengths without allocating a temporary posting byte vector. Loading decodes directly from a bounded byte count into the final posting vector. The implementation therefore adds no second full copy of large posting lists.

The current in-memory representation remains decoded `Vec<Posting>` for fast search. A future mapped/block-decoding backend can reuse the v2 entry boundaries without changing the logical API.
