# `.foidx` V1 Binary Format

All integers are little-endian. The current format is intentionally simple and fixed-width so correctness can stabilize before compression and mmap specialization.

## Header

| Field | Type | Meaning |
|---|---:|---|
| magic | `[u8; 8]` | ASCII `FROV0001` |
| version | `u32` | format version, currently `1` |
| qgram_size | `u32` | normalized Unicode-scalar q-gram length |
| winnow_window | `u32` | fingerprint selection window |
| normalization_flags | `u32` | bit 0 NFKC, bit 1 lowercase, bit 2 collapse whitespace |
| punctuation | `u8` | 0 keep, 1 map to space, 2 drop |
| reserved | `[u8; 3]` | must be zero |
| document_count | `u32` | number of document records |
| entry_count | `u64` | number of fingerprint dictionary records |

Unknown flag bits, punctuation values, reserved bytes, and versions are rejected.

## Document records

Repeated `document_count` times:

| Field | Type |
|---|---:|
| document_id | `u32` |
| path_length | `u64` |
| path_bytes | UTF-8 bytes |
| normalized_text_length | `u64` |
| normalized_text_bytes | UTF-8 bytes |

Document IDs must appear exactly in ascending order from zero. Normalized text is decoded into Unicode-scalar tokens and checked against the `u32` positional limit.

## Fingerprint records

Repeated `entry_count` times:

| Field | Type |
|---|---:|
| fingerprint_hi | `u64` |
| fingerprint_lo | `u64` |
| document_frequency | `u32` |
| posting_count | `u32` |
| postings | repeated `(document_id: u32, position: u32)` |

Fingerprints must be strictly increasing. Postings must be strictly increasing lexicographically. Every document ID and position must be valid. Observed distinct document count must equal the stored document frequency.

## Defensive bounds

The reader applies bounds before allocation:

- maximum document count
- maximum dictionary entries
- maximum string byte length
- maximum postings per entry
- counts constrained by total file length
- platform `usize` conversion checks

After the final posting, EOF is required. Trailing bytes are treated as corruption or an unsupported extension and fail closed.

## Atomic save behavior

The writer creates a sibling temporary file, writes all records, flushes the buffered writer, calls `sync_all`, removes an existing destination if necessary, and renames the temporary file into place.

## Planned V2 directions

- checksummed section directory
- source byte-offset maps
- compressed normalized text blocks
- delta-coded postings and inline short lists
- mmap-safe alignment
- optional prepared spectral artifacts
- segment UUID and corpus-manifest binding
- end-to-end content digest

V2 will use a distinct version/magic contract. V1 files will never be silently parsed under V2 assumptions.
