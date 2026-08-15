# Active-Learning Feedback Queue

The feedback ledger becomes more valuable when labeling effort is spent on examples that can change the model. Randomly reviewing easy exact matches or obvious negatives adds volume without much information.

`select_active_learning_queue` prioritizes four signals:

1. **uncertainty** — calibrated probability near 0.5;
2. **model disagreement** — disagreement among raw score, calibrated probability, and pairwise ranking score;
3. **hard-negative risk** — a high raw retrieval score supported by weak coverage, anchors, chain consistency, or false-match evidence;
4. **evidence novelty** — distance from examples already selected in the fourteen-dimensional ranking evidence space.

Selection is greedy and deterministic. Per-query and per-document caps prevent one difficult specimen or boilerplate-heavy source from consuming the entire review queue. Exact duplicate query/document/span candidates are removed before selection, and previously labeled examples are skipped by default.

## Input

Each JSONL row contains a stable query ID, the full raw result, optional model outputs, and an optional existing label:

```json
{
  "query_id":"specimen-0042",
  "result":{...},
  "calibrated_probability":0.47,
  "ranking_score":0.83,
  "label":null
}
```

## CLI

```bash
cargo run -p fo-bench --bin fo-active -- \
  candidates.jsonl \
  --output review-queue.jsonl \
  --maximum-examples 200 \
  --maximum-per-query 3 \
  --maximum-per-document 12
```

Each selected record includes the overall priority, component scores, evidence novelty, and a recommended feedback weight. The complete original candidate is retained so a reviewer can assign a label and append it directly to the durable feedback dataset.

## Accretive loop

1. Generate broad candidates with ordinary, composite, or multi-view retrieval.
2. Attach calibrated and pairwise-ranking scores when models are available.
3. Select a diverse active-learning queue.
4. Review and label the selected records.
5. Append judgments to grouped feedback.
6. Mine hard negatives, refit the pairwise ranker, then refit calibration.
7. Adopt only models that pass held-out micro/macro AUPRC and calibration gates.

This loop makes every review cycle concentrate on the current decision boundary and newly discovered failure modes rather than repeatedly sampling what the system already understands.
