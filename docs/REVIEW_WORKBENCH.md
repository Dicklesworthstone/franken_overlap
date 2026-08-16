# Human evidence review workbench

Search quality is not useful when a reviewer cannot see exactly what the system is claiming.

`fo-review-report` converts raw, composite, hybrid, or semantic-fusion results into one immutable, standalone review directory containing:

```text
index.html          dependency-free evidence page
review.json         machine-readable candidate and span evidence
decisions.jsonl     editable review-decision templates
artifacts.json      SHA-256 receipts for every generated artifact
```

No server is required. Open `index.html` directly in a browser.

## Generate a review page

```bash
fo-search query sec-items.fohybrid \
  --query-file current-item-1a.txt \
  --mode hybrid \
  --json > current-item-1a-results.json
```

```bash
fo-review-report \
  corpora/sec-items \
  current-item-1a.txt \
  current-item-1a-results.json \
  --target-id AAPL-2025-item-1a \
  --output reviews/AAPL-2025-item-1a
```

The output directory must not already exist. Review artifacts are intended to be immutable receipts.

## Supported inputs

The result file can contain:

- `Vec<SearchResult>` from the ordinary overlap CLI;
- `Vec<CompositeSearchResult>` from `fo-composite`;
- `HybridSearchReport` from `fo-search query --json`;
- `SemanticFusionReport` from `fo-semantic-fuse`.

Every candidate ID must exist in the supplied `fo-corpus` manifest. The source file is read through the manifest's safe relative path and rechecked against its recorded byte length and SHA-256 digest before rendering.

## Normalization and original coordinates

Search coordinates are normalized Unicode-scalar positions. The workbench uses `normalize_with_provenance` to map them back to exact original UTF-8 byte ranges for both:

- the specimen;
- the proposed source document.

The default normalization profile matches the ordinary FrankenOverlap default. When the source index used another profile, supply it explicitly:

```bash
fo-review-report corpus specimen.txt results.json \
  --normalization-profile normalization.json \
  --output review
```

Out-of-range or incompatible spans fail closed rather than producing misleading highlighting.

## What the page shows

For every candidate:

- rank and score;
- relationship class;
- textual, lexical, and semantic evidence badges;
- source identity, issuer/author, filing/publication date, and URL;
- each proposed source/specimen block side by side;
- original highlighted text with surrounding context;
- normalized token and original byte coordinates;
- edit similarity and distance;
- matched-token count;
- raw block score and estimated false matches;
- explicit notice when no localized textual evidence exists.

The page treats this as a hard boundary:

> Lexical or semantic relevance is not evidence that one text descended from another.

Only candidates containing localized overlap blocks are labeled as textual-provenance evidence.

## Review decisions

Each candidate exposes:

```text
accept source
reject source
uncertain
correct source ID
accepted block indexes
free-form notes
reviewer identity
```

The page can download the current state as `decisions.jsonl`. The generated `decisions.jsonl` file is an initial unreviewed template suitable for version control or assignment to another reviewer.

A decision record has this shape:

```json
{
  "schema_version": 1,
  "target_id": "AAPL-2025-item-1a",
  "candidate_id": "AAPL-2024-item-1a",
  "decision": "accept",
  "reviewer": "analyst@example.com",
  "notes": "Item 1A language is clearly inherited with one new paragraph.",
  "corrected_source_id": null,
  "accepted_block_indexes": [0, 1],
  "reviewed_at_unix": 1786854000
}
```

These records can feed future calibration, ranking, lineage, and active-learning workflows without scraping the HTML presentation.

## Semantic-fusion reviews

A semantic-only candidate can be displayed when it appears in a `SemanticFusionReport`, but it has:

```text
textual_provenance_supported = false
blocks = []
```

The workbench displays a warning rather than inventing an alignment. A reviewer can still mark it relevant or correct the proposed source, but it should not be ingested as a lineage edge without separate localized evidence.

## SEC workflow

A practical analyst loop is:

```text
new filing item
  → domain-aware overlap/hybrid search
  → optional semantic candidate fusion
  → standalone review page
  → analyst decisions
  → accepted localized results enter fo-lineage
  → rejected/uncertain results enter feedback and active learning
```

This turns the matcher into a reviewable provenance product rather than an opaque ranking API.

## Safety and integrity limits

- Output directories are immutable.
- Corpus paths cannot be absolute or contain parent traversal.
- Source bytes must match the corpus manifest receipt.
- Candidate and source byte limits are explicit.
- HTML text and attributes are escaped.
- Embedded JSON escapes script-significant characters.
- Original-coordinate mapping must succeed for every rendered block.
- `artifacts.json` records byte lengths and SHA-256 digests for the generated HTML, review JSON, and decision template.
