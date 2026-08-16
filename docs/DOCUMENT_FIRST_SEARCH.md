# Prepared document-first retrieval

The ordinary sparse matcher performs positional voting as soon as a matching fingerprint posting is read. That is ideal when a specimen touches a small number of highly selective postings. It is less attractive when many documents contain at least some matching language but only a small subset deserves positional alignment and exact verification.

`fo-document-first` adds a bounded first stage:

```text
prepare specimen once
  → group repeated fingerprints
  → process rare posting lists first
  → score each document at most once per distinct feature
  → retain a bounded document set
  → project only the required documents and query fingerprints
  → run the ordinary diagonal/chaining/verifier pipeline unchanged
```

The first stage is a retrieval filter, not a final scorer. Every returned match still passes FrankenOverlap's ordinary positional and exact verification logic.

## Run it

```bash
cargo run --release -p fo-cli --bin fo-document-first -- \
  corpus.foidx specimen.txt \
  --maximum-documents 128 \
  --minimum-document-score-fraction 0.08 \
  --maximum-posting-pairs 10000000
```

Inspect only the document stage:

```bash
fo-document-first corpus.foidx specimen.txt \
  --plan-only \
  --json
```

## Prepared queries

The specimen's normalization, q-gram hashing, winnowing, grouping, posting counts, document frequencies, and IDF values can be serialized once:

```bash
fo-document-first corpus.foidx specimen.txt \
  --prepared-output specimen.foprepared \
  --plan-only
```

Reuse it later:

```bash
fo-document-first corpus.foidx \
  --prepared-query specimen.foprepared \
  --maximum-documents 256
```

A prepared query is tied to the complete `IndexConfig`. Loading it against an index with different normalization, q-gram size, or winnowing settings fails closed.

This is useful when:

- the same specimen is searched under several thresholds;
- a review process reruns one specimen repeatedly;
- several ranking or verification configurations share one candidate-generation stage;
- a resident service caches hot queries.

## Document scoring

For one distinct query fingerprint, each document receives at most one contribution regardless of how many times the feature appears in that document.

The contribution is based on:

```text
document-level IDF × bounded query multiplicity
```

Query multiplicity uses `1 + ln(occurrences)`, so repeated specimen features matter without allowing one repeated phrase to dominate linearly.

Candidates are ordered by:

1. document evidence score;
2. number of distinct matching query features;
3. matching query-feature occurrences;
4. stable original document ID.

## Work limits

```text
--maximum-documents 128
--minimum-document-score-fraction 0.08
--minimum-distinct-features 2
--maximum-postings-per-feature 50000
--maximum-posting-pairs 10000000
--maximum-selected-document-fraction 0.50
```

Features are processed rarest-first. A feature that would exceed the total posting-pair budget is skipped and reported.

The report includes:

```text
corpus and selected document counts
selected fraction
prepared feature occurrences
retained distinct features
features suppressed by posting cap
features suppressed by total work budget
postings scanned
posting/query-position pairs
ranked document candidates
verified final results
```

## Projected index behavior

The retained search view does not re-normalize or re-fingerprint candidate documents.

It creates an in-memory projection containing:

- cloned normalized documents for selected IDs;
- only query fingerprints retained by the first stage;
- only postings belonging to selected documents;
- local document IDs mapped back to original stable IDs after search.

Source positions are unchanged because the projected documents retain the exact normalized token streams from the original index.

## Full-index fallback

The document stage can be counterproductive when:

- too little indexed evidence survives;
- nearly every document receives a plausible score;
- the selected set exceeds the configured fraction of the corpus.

By default, these cases fall back to the full index and are labeled:

```text
full_index_fallback_thin_evidence
full_index_fallback_broad_candidate_set
```

Disable fallback when a hard work boundary matters more than recall:

```text
--no-full-index-fallback
```

A no-fallback query with no adequate document candidate returns `no_candidates` rather than silently scanning the complete corpus.

## Expected value

Document-first retrieval should improve wall time when:

```text
many documents share some query language
but only a small fraction share several distinctive features
```

Examples include:

- SEC and contract corpora after common boilerplate is suppressed;
- large books or archives with recurring phrases;
- code corpora with common framework tokens;
- repeated provenance queries over one static corpus.

It may add overhead for already-selective queries. Benchmark it against ordinary sparse search using the same queries, candidate universe, and final verification thresholds.

Report at minimum:

```text
candidate recall before verification
macro and micro AUPRC
Recall@1 and Recall@10
p50/p95/p99
postings and posting pairs touched
selected document fraction
projection construction time
verification time
full-index fallback rate
```

The stage should be adopted only when candidate recall remains acceptable and end-to-end latency improves on held-out workloads.
