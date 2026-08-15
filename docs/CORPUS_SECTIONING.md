# Corpus-aware sectioning

Long books and annual filings are poor retrieval units for several reasons:

- a correct passage may cover only a tiny fraction of the full document;
- snippets and highlights should point to a meaningful chapter or filing item;
- generic lexical signals become diluted by hundreds of unrelated pages;
- AUPRC suffers when every answer is scored at whole-book or whole-filing granularity.

`fo-section` derives a new, checksummed `fo-corpus` whose documents are searchable sections while retaining exact parent and source-coordinate metadata.

## Project Gutenberg chapters

```bash
cargo run -p fo-corpus --bin fo-section -- \
  corpora/gutenberg-standard \
  --output corpora/gutenberg-chapters \
  --strategy gutenberg
```

The heading recognizer accepts chapter, book, part, volume, preface, introduction, prologue, epilogue, and conclusion lines. Project Gutenberg front matter that is large enough becomes its own section. Duplicate short table-of-contents headings are suppressed by retaining the largest span for each canonical heading.

## SEC Form 10-K items

```bash
cargo run -p fo-corpus --bin fo-section -- \
  corpora/sec-standard \
  --output corpora/sec-items \
  --strategy sec10k
```

The SEC recognizer extracts bounded `ITEM` headings such as:

- Item 1: Business;
- Item 1A: Risk Factors;
- Item 7: Management's Discussion and Analysis;
- Item 7A: Quantitative and Qualitative Disclosures;
- Item 8: Financial Statements.

Short table-of-contents runs are discarded by the minimum-section threshold. If a real item exceeds the configured maximum, it is subdivided at paragraph boundaries with deterministic overlap.

## Automatic and generic strategies

```bash
fo-section input-corpus \
  --output derived-corpus \
  --strategy auto
```

`auto` uses the parent manifest provider: Gutenberg chapter rules for books and SEC item rules for 10-K filings. If no usable headings are present, or when `paragraph-windows` is selected explicitly, the document is divided into overlapping paragraph-aligned windows.

Key controls:

```text
--minimum-characters 2000
--target-characters 18000
--maximum-characters 36000
--overlap-characters 1000
--maximum-sections-per-document 512
```

The constraints require:

```text
minimum <= target <= maximum
0 <= overlap < target
```

## Derived manifest contract

Each section retains:

```text
parent_id
parent_title
section_index
section_title
section_origin        heading | window
source_start_byte
source_end_byte
```

The section's source URL, issuer/author, language, filing/publication date, and parent metadata are preserved. The derived manifest records the parent corpus ID, parent-manifest SHA-256, strategy, and all section-size parameters.

Section IDs are deterministic:

```text
<parent-id>#section-0001
<parent-id>#section-0002
```

Files are written atomically below a sanitized parent directory. Unsafe parent paths are rejected. Existing output is never destroyed unless `--replace-output` is supplied explicitly.

## Search and benchmark use

A derived corpus is an ordinary `fo-corpus` input:

```bash
fo-search build corpora/sec-items \
  --input-format corpus \
  --output indexes/sec-items.fohybrid

fo-real-bench \
  --corpus-root corpora/gutenberg-chapters \
  --provider existing
```

This improves granularity without changing the matching algorithms. Whole-document provenance remains available through `parent_id`, while ranking and coverage operate on a much more meaningful retrieval unit.
