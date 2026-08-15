# Query Intents and Calibrated Ranking

`fo query` exposes the retrieval objective and all important evidence floors directly.

## Passage search

Find any meaningful reused span, even when it explains only a small portion of the specimen:

```bash
fo query corpus.foidx specimen.txt \
  --intent any-passage \
  --minimum-matched-tokens 32 \
  --minimum-similarity 0.30
```

## Source attribution

Prefer documents that explain a substantial fraction of the specimen:

```bash
fo query corpus.foidx specimen.txt \
  --intent source-attribution \
  --minimum-query-coverage 0.35 \
  --minimum-matched-tokens 48
```

## Near-duplicate search

Require meaningful coverage in both directions:

```bash
fo query corpus.foidx specimen.txt \
  --intent near-duplicate \
  --minimum-query-coverage 0.70 \
  --minimum-source-coverage 0.70
```

## Work budgets

Short-query fallbacks are explicitly bounded:

```bash
fo query corpus.foidx specimen.txt \
  --direct-fallback-work-limit 50000000 \
  --short-query-candidates 8
```

The work limit caps candidate generation; surviving spans are still verified exactly.

## Calibrated reranking

Apply a fitted model produced by `fo-bench fit-calibration`:

```bash
fo query corpus.foidx specimen.txt \
  --minimum-similarity 0.10 \
  --calibration-model calibration.json \
  --minimum-probability 0.75 \
  --json
```

The lower raw-score floor deliberately gives the calibrator a wider candidate set. The calibrated JSON output preserves each original `SearchResult` and adds the learned probability. Invalid model schemas, non-finite parameters, and invalid probability thresholds fail closed.
