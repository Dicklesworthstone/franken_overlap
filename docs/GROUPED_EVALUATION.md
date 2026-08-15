# Query-Grouped Evaluation

A single global AUPRC is necessary but insufficient for source retrieval. It can be dominated by queries with many easy negatives and does not reveal whether the correct source actually appears near the top for each specimen.

`fo-group-eval` consumes JSONL records with a stable query ID:

```json
{"query_id":"specimen-0042","score":0.91,"label":true}
{"query_id":"specimen-0042","score":0.54,"label":false}
```

```bash
cargo run -p fo-bench --bin fo-group-eval -- \
  grouped-scores.jsonl \
  --recall-k 1,5,10 \
  --bootstrap-samples 1000 \
  --confidence-level 0.95 \
  --json
```

## Reported metrics

- micro/global average precision and complete calibration report;
- macro average precision, giving each positive-bearing query equal weight;
- mean reciprocal rank;
- Recall@k;
- nDCG@k;
- candidate counts and positive/negative totals;
- deterministic query-bootstrap confidence intervals for micro and macro AUPRC.

Score ties are treated as groups. Recall@k, nDCG, and reciprocal rank report the expected metric under a random ordering inside a tie rather than receiving an optimistic boost from label-dependent sorting.

## Operating thresholds

A production threshold can be selected under explicit constraints:

```bash
cargo run -p fo-bench --bin fo-group-eval -- \
  grouped-scores.jsonl \
  --minimum-precision 0.95 \
  --minimum-recall 0.70 \
  --maximum-false-positives-per-query 0.10
```

Every distinct score threshold is evaluated, regardless of the curve-downsampling setting used for display. The selected point maximizes recall, then precision, F1, and threshold, subject to all constraints. The command fails closed when no threshold qualifies.

## Why both micro and macro AUPRC matter

Micro AUPRC answers how well the entire candidate stream is ordered. Macro query AUPRC answers whether quality is broadly distributed across specimens. A change that improves micro AUPRC while damaging macro AUPRC may simply be getting better on high-candidate or easy queries. FrankenOverlap quality gates should normally track both, plus Recall@1 and MRR.

Query-level bootstrap sampling preserves the correlation among candidates generated for the same specimen and produces more honest uncertainty intervals than resampling individual candidate rows independently.
