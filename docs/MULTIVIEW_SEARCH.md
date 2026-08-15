# Multi-View Consensus Retrieval

A single q-gram scale has an unavoidable tradeoff:

- short q-grams preserve recall under substitutions and OCR-like corruption but produce more common-feature collisions;
- long q-grams are highly selective but disappear more quickly under edits;
- intermediate q-grams provide a useful compromise but cannot dominate both extremes.

`MultiViewIndex` persists several ordinary FrankenOverlap indexes over the same ordered document set and fuses only span-compatible results. The default balanced preset uses q-gram sizes 5, 7, and 11.

## Presets

| Preset | Scales | Minimum support | Purpose |
|---|---:|---:|---|
| `balanced` | 5, 7, 11 | 2 | general use |
| `high-recall` | 4, 6, 8 | 1 | noisy or aggressively edited text |
| `high-precision` | 7, 11, 15 | 2 | boilerplate-heavy corpora |

All views share one normalization profile so query and corpus coordinates remain directly comparable.

## CLI

```bash
cargo run -p fo-cli --bin fo-multiview -- build \
  ./corpus \
  --output ./corpus.fomv \
  --preset balanced

cargo run -p fo-cli --bin fo-multiview -- query \
  ./corpus.fomv specimen.txt \
  --intent source-attribution \
  --minimum-score 0.30

cargo run -p fo-cli --bin fo-multiview -- inspect ./corpus.fomv
```

The output reports how many views supported the span, score disagreement, weighted edit similarity, weighted query/source coverage, and every view’s raw evidence.

## Fusion

Hits are grouped only when they share a document and substantially overlap in both specimen and corpus coordinates. One view can contribute at most one evidence record to a cluster. Fusion combines:

- weighted raw score;
- weighted edit similarity;
- weighted query coverage;
- weighted chain consistency;
- estimated-false-match confidence;
- fraction of configured views supporting the span;
- cross-view score agreement.

Single-view accidents are therefore penalized in balanced and high-precision modes, while stable evidence surviving several q-scales receives stronger support. The original representative `SearchResult` is preserved rather than overwritten, so downstream calibration and audit tooling retain the raw engine evidence.

## Persistence

A multi-view index is a directory containing an atomic JSON manifest and one ordinary `.foidx` file per view. Loading validates:

- manifest version and filenames;
- every view’s q-gram, winnowing, and normalization configuration;
- identical document IDs, paths, counts, and normalized coordinate lengths across views.

The architecture deliberately reuses the ordinary index format. Improvements to postings, verification, and sparse retrieval automatically benefit every view.
