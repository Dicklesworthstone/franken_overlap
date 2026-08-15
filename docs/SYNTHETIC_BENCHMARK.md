# Deterministic Retrieval Benchmark

`fo-bench synthetic` creates a reproducible corpus, generates edited specimens with known source documents, scores every query-document pair, and reports both retrieval quality and throughput.

```bash
cargo run -p fo-bench -- synthetic
```

The default suite creates 32 documents and four queries per document. Each source passage is exercised under four mutation profiles:

1. formatting, case, punctuation, and line-wrap drift
2. deterministic word substitutions
3. word insertions and deletions
4. partial reuse surrounded by unrelated text

## Compared methods

The same corpus/query pairs are evaluated with:

- FrankenOverlap sparse retrieval, chaining, intent-aware scoring, and exact verification
- normalized exact substring search
- character q-gram Jaccard similarity
- character q-gram SimHash similarity

The report contains, for every method:

- average precision / AUPRC
- Recall@1
- mean reciprocal rank
- false positives per query at the best-F1 threshold
- elapsed time and pair throughput
- full probability-calibration metrics and a bounded precision-recall curve

## Persist reproducible artifacts

```bash
cargo run -p fo-bench -- synthetic \
  --documents 64 \
  --queries-per-document 8 \
  --seed 6840399175121550014 \
  --output artifacts/quality/synthetic.json \
  --labeled-scores artifacts/quality/synthetic-scores.jsonl \
  --json
```

The labeled score stream can be inspected independently:

```bash
cargo run -p fo-bench -- evaluate \
  artifacts/quality/synthetic-scores.jsonl --json
```

## Enforce local quality floors

The command exits unsuccessfully when a declared quality floor is missed:

```bash
cargo run -p fo-bench -- synthetic \
  --minimum-auprc 0.80 \
  --minimum-recall-at-1 0.80
```

A small integration test runs this mechanism during `cargo test`; larger suites should be run before performance-sensitive merges and releases.

## Interpretation

The generated corpus is deliberately controlled. It catches regressions in candidate recall, score separation, edit tolerance, normalization, and ranking, but it is not a substitute for PAN, OCR, source-code clone, SEC filing, or web-reuse corpora. Its purpose is to provide a fast, deterministic quality gate while those external adapters are developed and while production feedback accumulates.
