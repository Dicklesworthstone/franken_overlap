# Empirical status and value proposition

This document separates three different questions that are easy to blur together:

1. **Does FrankenOverlap contain a real implementation?** Yes.
2. **Does it contain the machinery needed to compare that implementation fairly with alternatives?** Yes.
3. **Has the repository already established, with a checked-in real-corpus evidence bundle, that FrankenOverlap is better than those alternatives?** Not yet.

## Current status

As of August 15, 2026, `main` contains:

- sparse positional overlap retrieval;
- exact candidate verification;
- fragmented and reordered passage aggregation;
- fielded BM25, phrase, and proximity search;
- explainable lexical/overlap hybrid ranking;
- Project Gutenberg and SEC Form 10-K acquisition and sectioning;
- deterministic real-corpus scenario generation;
- conventional retrieval baselines;
- a bounded exhaustive semi-global Levenshtein control;
- nested corpus-size benchmarks;
- query-group AUPRC, Recall@k, MRR, nDCG, span, latency, and throughput measurement;
- natural-label adjudication;
- paired bootstrap claim gates;
- immutable Markdown/HTML evidence bundles;
- a one-command evidence-suite orchestrator.

What `main` does **not** yet contain is a completed, reviewable real-corpus evidence run with pinned corpus receipts and numerical results. Therefore the repository should not currently claim that FrankenOverlap has already beaten BM25, exact substring search, q-gram Jaccard, SimHash, or exhaustive edit-distance retrieval by a particular AUPRC or wall-time margin.

The defensible summary is:

> FrankenOverlap is a substantial and testable specialized search system whose comparative advantage is plausible, but not yet empirically established in the repository.

## Where it is already useful

FrankenOverlap's distinctive value is **not** that it provides yet another keyword search box. Its strongest capability is explainable, localized textual reuse detection:

- finding the source of a long passage after substitutions, insertions, deletions, OCR errors, or formatting changes;
- recovering several copied fragments after they were separated or reordered;
- aligning versions and showing which source spans survived;
- building document-lineage and provenance graphs;
- distinguishing a strong local textual match from broad topical similarity;
- returning exact evidence that can be reviewed, highlighted, and audited.

That makes the project potentially valuable for:

- SEC filing and contract-language lineage;
- plagiarism and unattributed reuse analysis;
- publisher, archive, and library edition comparison;
- OCR recovery and historical-text alignment;
- source-code and license-provenance analysis after token-aware adaptation;
- dataset deduplication and training-data provenance;
- internal knowledge-base version and policy reuse tracking.

The project is also valuable today as an evaluation workbench. It can create real corpora, generate controlled and natural scenarios, compare several retrieval families, adjudicate ambiguous positives, and prevent weak evidence from becoming a public performance claim.

## Where it should not pretend to win

No one method should be expected to dominate every workload.

| Workload | Natural first choice | FrankenOverlap's role |
|---|---|---|
| Exact unchanged quotation | exact substring search | fallback for normalization or edits |
| Short keyword query | BM25 / positional lexical search | hybrid evidence only when useful |
| Pure semantic paraphrase | embeddings or another semantic retriever | lexical/provenance verification after semantic candidate generation |
| One short pair of known texts | direct Myers or edit-distance alignment | unnecessary indexing overhead |
| Long edited specimen against a static corpus | sparse overlap index | primary differentiated use case |
| Fragmented or reordered reuse | composite overlap search | primary differentiated use case |
| Many repeated queries over a growing corpus | indexed and segmented retrieval | strong intended use case |
| One-off scan over unindexed resident text | direct equality or dense correlation | optional dense route |

FrankenOverlap is not currently a replacement for Elasticsearch, Lucene, or a vector database as a general distributed search service. It has useful lexical and hybrid layers, but the defensible differentiation is source attribution, alignment, provenance, and edited-overlap retrieval.

## What would establish comparative value

A serious claim requires a completed `fo-evidence-suite` run with:

- a pinned corpus manifest and SHA-256 receipts;
- a frozen query set or adjudicated gold labels;
- the exact repository commit and Rust compiler;
- hardware and thread-count metadata;
- identical candidate universes for all compared methods;
- exact, BM25, q-gram Jaccard, SimHash, FrankenOverlap, and hybrid results;
- exhaustive Levenshtein where the declared work budget permits it;
- micro and macro query AUPRC;
- Recall@1/5/10, MRR, and nDCG;
- false positives per query;
- span precision, recall, F1, IoU, and endpoint error;
- p50, p95, p99, throughput, build time, index size, and break-even query count;
- per-profile and worst-slice results;
- paired query-bootstrap confidence intervals;
- explicit `supported`, `inconclusive`, or `unsupported` claim verdicts.

Until such a bundle is checked in, the benchmark commands demonstrate reproducibility—not superiority.

## Highest-value next product direction

The strongest near-term product is an **explainable textual provenance engine**, not a generic search replacement.

A pragmatic first product should focus on one corpus with valuable natural lineage, such as SEC filings:

1. Continuously acquire new filings.
2. Derive stable filing-item and paragraph units.
3. Detect reuse and changes across years and issuers.
4. Build a durable source/descendant graph.
5. Show aligned changed and unchanged spans.
6. Rank unusual copied language above ubiquitous boilerplate.
7. Expose alerts, APIs, and an analyst-facing review interface.

That use case exploits the parts of FrankenOverlap that conventional keyword and semantic retrieval do not naturally provide: fixed source coordinates, edit-tolerant passage identity, fragmented reuse, and auditable lineage.

## Engineering priorities after empirical proof

Once real evidence identifies the actual bottlenecks, the highest-leverage engineering work is likely to include:

- mapped and block-compressed immutable indexes;
- document-first WAND/MaxScore-style pruning before positional work;
- prepared-query and prepared-spectrum reuse;
- more aggressive worker-local allocation reuse;
- NUMA-aware partitioning for large AMD systems;
- a persistent query service with bounded concurrency and observability;
- stable public APIs and language bindings;
- domain-specific tokenization for code, contracts, OCR, and filings;
- a semantic candidate lane whose outputs remain separate from textual-overlap evidence;
- a review UI for alignments, provenance edges, and adjudication.

The benchmark should determine which of these matters. The project should not optimize a secondary kernel merely because it is technically interesting.

## Evidence commands

The preferred end-to-end path is:

```bash
fo-showcase gutenberg --output showcase/gutenberg

fo-evidence-suite \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  --claim-manifest evidence/gutenberg-claims.json \
  --output evidence-runs/gutenberg-final
```

For ambiguous natural relationships, adjudicate first:

```bash
fo-adjudicate queue \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  preliminary-scores.jsonl \
  --output review/gutenberg.jsonl

fo-adjudicate apply \
  showcase/gutenberg/queries.jsonl \
  review/gutenberg-decisions.jsonl \
  --output gold/gutenberg.jsonl
```

See also:

- [`EVIDENCE_SUITE.md`](EVIDENCE_SUITE.md)
- [`SCENARIO_PROOF_BENCHMARK.md`](SCENARIO_PROOF_BENCHMARK.md)
- [`PAIRED_CLAIM_GATES.md`](PAIRED_CLAIM_GATES.md)
- [`EVIDENCE_BUNDLES.md`](EVIDENCE_BUNDLES.md)
- [`GOLD_ADJUDICATION.md`](GOLD_ADJUDICATION.md)
