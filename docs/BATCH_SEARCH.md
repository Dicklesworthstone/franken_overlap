# Parallel Batch Search

FrankenOverlap’s common production workload is many specimens against one resident immutable index. `Index::search_batch` parallelizes at the query level, where work units are independent and large enough to occupy high-core-count CPUs without introducing synchronization inside the sparse matcher.

## Guarantees

- output order always matches input order;
- query IDs must be nonempty and unique;
- identical specimen strings share one search by default;
- duplicate queries retain their own IDs and record the canonical query index;
- total query count and specimen bytes are bounded before work starts;
- individual errors are isolated unless fail-fast mode is requested;
- an optional private Rayon pool sets an exact worker count without changing the global pool.

## NDJSON interface

Input:

```json
{"id":"query-0001","specimen":"preserve the raw measurements"}
{"id":"query-0002","specimen":"the shutters opened before dawn"}
```

```bash
cargo run -p fo-cli --bin fo-batch -- \
  corpus.foidx queries.jsonl \
  --output results.jsonl \
  --threads 32 \
  --intent source-attribution
```

By default, each output line is a `BatchSearchResult`. `--json` emits one complete `BatchSearchReport` containing totals and all results.

## Throughput behavior

The index is loaded once and shared immutably across workers. Every unique specimen runs through the ordinary search implementation, so intent-aware scoring, bounded verification, and future sparse-index improvements accrue automatically. Identical specimens are deduplicated before scheduling; their result vectors are cloned only after the canonical search finishes.

Batch-level limits prevent accidentally queuing unbounded data. Per-query failures, including specimens empty after normalization, are represented as error strings while unrelated queries continue. The CLI returns a nonzero status when any query failed, making partial failure visible to orchestration systems without discarding successful outputs.

For a single very large specimen, use ordinary or dense search. Batch parallelism is intended for many independent specimens and avoids nested parallelism inside each query.
