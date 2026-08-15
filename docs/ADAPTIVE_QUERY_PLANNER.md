# Adaptive Query Planning

One matcher is not optimal for every specimen. `Index::plan_query` inspects the normalized query against the loaded index before spending candidate-generation work.

## Measured signals

The plan reports:

- normalized and distinct token counts,
- Shannon token entropy and entropy ratio,
- repetition fraction,
- raw and winnowed q-gram counts,
- distinct selected fingerprints,
- retained, missing, and heavy-suppressed feature counts,
- exact estimated posting-pair products,
- two-grid diagonal-vote work,
- maximum and mean retained posting-list lengths,
- a suggested heavy-feature cap when the configured sparse work budget is exceeded.

Posting work is computed after grouping repeated query fingerprints, so repeated phrases are represented honestly rather than counted as unrelated features.

## Routes

`search_adaptive` selects one executable route:

- `short_direct`: ordinary exact/Myers short-query portfolio,
- `sparse`: ordinary rare-feature diagonal voting,
- `composite`: fragmented and reordered passage aggregation,
- `bounded_sparse`: ordinary search with a dynamically lowered heavy-feature cap.

Composite routing is limited to source-attribution and near-duplicate intents. It is selected for sufficiently long specimens when feature retention is weak, repetition is high, or the specimen is large enough that independent reused sections are plausible.

## Advisories

The planner also emits structured advisories that can be consumed by a higher-level service:

- low entropy,
- high repetition,
- many absent query features,
- heavy-feature suppression,
- sparse-work budget exceeded,
- multiview recommended,
- dense scan recommended,
- composite recommended.

A plain `.foidx` can execute ordinary and composite routes. Multiview and dense advisories are explicit because those routes require a multiview index or an unindexed corpus buffer respectively; the planner never pretends those resources are present.

## CLI

Inspect a plan:

```bash
cargo run -p fo-cli --bin fo-plan -- \
  corpus.foidx specimen.txt \
  --intent source-attribution \
  --json
```

Execute the selected route:

```bash
cargo run -p fo-cli --bin fo-plan -- \
  corpus.foidx specimen.txt \
  --execute \
  --sparse-posting-pair-budget 25000000 \
  --composite-minimum-tokens 256 \
  --json
```

## Determinism and safety

Planning is deterministic for a fixed index, specimen, and option set. Every length and work estimate uses saturating arithmetic. The planner does not mutate the index or hide execution changes: the report includes both the route and the effective posting-list cap used by adaptive search.
