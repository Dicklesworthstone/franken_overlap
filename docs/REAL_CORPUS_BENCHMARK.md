# Real-corpus retrieval benchmark

`fo-real-bench` evaluates FrankenOverlap on actual books or SEC filings rather than only synthetic documents. It can either consume an existing `fo-corpus` manifest or invoke the native downloader before benchmarking.

## Project Gutenberg

Small demonstration using the official catalog and conservative main-site rate:

```bash
cargo run -p fo-bench --bin fo-real-bench -- \
  --provider gutenberg \
  --preset smoke \
  --corpus-root corpora/gutenberg-smoke \
  --maximum-documents 25 \
  --source-documents 16 \
  --output benchmark-artifacts/gutenberg-smoke.json \
  --scores-output benchmark-artifacts/gutenberg-smoke-scores.jsonl
```

Larger runs require an explicit Project Gutenberg mirror:

```bash
cargo run -p fo-bench --bin fo-real-bench -- \
  --provider gutenberg \
  --preset standard \
  --gutenberg-mirror "$GUTENBERG_MIRROR" \
  --corpus-root corpora/gutenberg-standard \
  --maximum-documents 250 \
  --source-documents 32
```

## SEC Form 10-K

SEC acquisition requires a declared identity containing a contact email and remains at or below the configured fair-access request rate:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
cargo run -p fo-bench --bin fo-real-bench -- \
  --provider sec10k \
  --preset standard \
  --corpus-root corpora/sec-standard \
  --maximum-documents 75 \
  --source-documents 24 \
  --output benchmark-artifacts/sec-standard.json
```

Downloaded corpora are resumable and SHA-256 verified through the `fo-corpus` manifest.

## Query profiles

For each deterministic source document the benchmark extracts a real passage and creates up to eight workloads:

1. exact passage;
2. case, punctuation, and line-wrap drift;
3. regular word substitutions;
4. burst insertions and deletions;
5. OCR-like character corruption;
6. separated source fragments surrounded by unrelated text;
7. reordered passage thirds;
8. short keyword retrieval.

The source document is the positive label; every other indexed document is a negative. All methods receive exactly the same query-document candidate universe.

## Methods

The benchmark compares:

- normalized exact substring search;
- character q-gram Jaccard;
- character q-gram SimHash;
- fielded BM25 plus exact phrase and proximity evidence;
- FrankenOverlap sparse alignment;
- unified hybrid retrieval.

The global edit-distance matrix is deliberately not used as a corpus retrieval baseline: it is a verifier, not a scalable retrieval index. Existing exact and banded verifiers remain available after candidate generation.

## Metrics

Each method reports:

- micro candidate-stream AUPRC;
- macro per-query AUPRC;
- tie-aware Recall@1, Recall@5, Recall@10, and MRR;
- false positives per query at the best-F1 threshold;
- p50, p95, and p99 query latency;
- queries per second.

The build report adds index construction time, serialization time, total persisted bytes, overlap posting counts, and lexical posting counts. Per-profile quality exposes failures hidden by aggregate AUPRC.

## Machine-readable score stream

`--scores-output` writes one JSONL row per query-document pair:

```json
{
  "query_id": "book-84/ocr_noise",
  "profile": "ocr_noise",
  "source_id": "84",
  "candidate_id": "1342",
  "label": false,
  "scores": {
    "normalized_exact_substring": 0.0,
    "character_qgram_jaccard": 0.17,
    "character_qgram_simhash": 0.55,
    "fielded_bm25_phrase_proximity": 0.41,
    "franken_overlap": 0.26,
    "franken_hybrid": 0.48
  }
}
```

This is the input surface for model tournaments, hard-negative mining, active learning, slice evaluation, and future automatic tuning.

## Regression gates

A run can fail closed when quality or latency regresses:

```bash
fo-real-bench \
  --corpus-root corpora/gutenberg-standard \
  --minimum-hybrid-auprc 0.80 \
  --minimum-hybrid-recall-at-1 0.75 \
  --require-hybrid-auprc-delta 0.01 \
  --maximum-hybrid-p95-ms 250
```

The benchmark does not claim an improvement until the chosen corpus, seed, compiler, machine, and score report demonstrate it.
