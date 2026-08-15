# AUPRC and Calibration Evaluation

FrankenOverlap treats retrieval quality as a first-class artifact. The `fo-bench`
binary evaluates labeled probability scores without requiring Python or a
notebook:

```bash
cargo run -p fo-bench -- evaluate judgments.jsonl
cargo run -p fo-bench -- evaluate judgments.jsonl --json > report.json
```

Each nonblank input line is JSON:

```json
{"score":0.91,"label":true}
{"score":0.37,"label":false}
```

The report includes average precision (the standard stepwise AUPRC), the
best-F1 operating threshold, prevalence, Brier score, log loss, expected
calibration error, maximum calibration error, and a bounded precision-recall
curve. Equal scores are processed as one threshold group, so results do not
depend on arbitrary tie ordering.

A score change should not be accepted on latency alone. Record at least:

- AUPRC / average precision
- Recall at the chosen precision floor
- False positives per GiB
- p50/p95/p99 query latency
- index bytes per normalized token
- peak RSS

The next benchmark layer will generate these labeled score rows directly from
deterministic mutation corpora and real span annotations.
