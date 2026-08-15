# Accretive Feedback and Calibration

FrankenOverlap can turn accepted and rejected search results into an append-only
evidence corpus, then fit a deterministic logistic calibration model over the
full result evidence vector.

## Record judgments

Save query results as JSON and label the relevant rank:

```bash
fo query corpus.foidx specimen.txt --json > results.json

cargo run -p fo-bench -- record-feedback results.json \
  --rank 1 --label positive --output feedback.jsonl

cargo run -p fo-bench -- record-feedback results.json \
  --rank 2 --label negative --output feedback.jsonl
```

Each JSONL record retains the complete `SearchResult`, the binary judgment, and
an optional positive training weight. No derived evidence is discarded, so a
future model can be refit with a different feature policy.

## Fit a model

```bash
cargo run -p fo-bench -- fit-calibration feedback-train.jsonl \
  --output calibration.json
```

The model standardizes ten bounded evidence features and fits regularized
logistic loss with deterministic batch gradient descent. The persisted JSON
records feature order, means, scales, weights, bias, training counts, completed
epochs, and the training precision-recall report.

## Require out-of-sample improvement

Always evaluate on judgments not used for fitting:

```bash
cargo run -p fo-bench -- compare-calibration feedback-test.jsonl \
  calibration.json --json > calibration-report.json
```

The comparison reports raw and calibrated AUPRC, Brier score, log loss, ECE,
MCE, and explicit deltas. Adopt a model only when the held-out AUPRC and the
operating-point metrics improve without unacceptable calibration regression.

## Rerank existing results

```bash
cargo run -p fo-bench -- rerank results.json calibration.json \
  --output reranked.json
```

The output preserves each original result and adds its calibrated probability.
Raw evidence and the hand-designed score remain available for audit and future
model revisions.
