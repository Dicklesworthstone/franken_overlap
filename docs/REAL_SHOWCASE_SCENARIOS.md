# Curated real-corpus showcase scenarios

`fo-showcase` prepares reproducible, labeled demonstrations from real Project Gutenberg books and SEC Form 10-K filings. It performs acquisition, manifest verification, section derivation, and query generation in native Rust.

The generated queries are suitable for:

- `fo-exhaustive-bench`;
- future proof-suite orchestration;
- manual adjudication;
- AUPRC and ranking evaluation;
- active-learning and hard-negative analysis.

No corpus payload is committed to the repository.

## Project Gutenberg showcase

The default curated set contains:

```text
84      Frankenstein
41445   Frankenstein, 1818 edition
42324   Frankenstein, 1831 edition
1661    The Adventures of Sherlock Holmes
2701    Moby-Dick
11      Alice's Adventures in Wonderland
1342    Pride and Prejudice
98      A Tale of Two Cities
2600    War and Peace
345     Dracula
```

Prepare it with conservative main-site request spacing:

```bash
cargo run --release -p fo-bench --bin fo-showcase -- gutenberg \
  --output showcase/gutenberg
```

For larger custom ID sets, provide a Project Gutenberg mirror:

```bash
GUTENBERG_MIRROR='https://example-mirror.invalid/cache/epub' \
  cargo run --release -p fo-bench --bin fo-showcase -- gutenberg \
  --output showcase/gutenberg-large \
  --id 84 --id 41445 --id 42324
```

Output:

```text
showcase/gutenberg/
  raw/
    manifest.json
    documents/
    metadata/
  sections/
    manifest.json
    documents/
  queries.jsonl
  showcase.json
```

The chapter corpus retains parent eBook identity, source URL, title, author, section title, and original parent byte range.

## SEC 10-K showcase

The default issuer set is:

```text
AAPL
MSFT
NVDA
JPM
WMT
```

Prepare recent filing histories:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
  cargo run --release -p fo-bench --bin fo-showcase -- sec10k \
  --output showcase/sec-10k \
  --filings-per-company 5 \
  --from-date 2018-01-01
```

The raw corpus contains downloaded primary 10-K documents. The searchable corpus contains recognized filing items and bounded paragraph windows, retaining CIK, accession number, form, filing date, parent filing, section title, and original byte range.

The downloader enforces the existing SEC user-agent and rate-limit safeguards.

## Existing corpus mode

Generate the same scenarios without network activity:

```bash
cargo run --release -p fo-bench --bin fo-showcase -- existing \
  corpora/my-corpus \
  --output showcase/my-corpus \
  --strategy auto
```

Use an already sectioned corpus directly:

```bash
fo-showcase existing corpora/sec-items \
  --output showcase/sec-items \
  --no-sectioning
```

## Scenario profiles

Each selected real source contributes up to eight deterministic scenarios:

| Profile | Purpose |
|---|---|
| `exact` | establishes the literal-search floor |
| `format_drift` | case, punctuation, and line-wrap invariance |
| `substitution_10pct` | lexical replacement robustness |
| `insertion_deletion` | edit-distance and chaining robustness |
| `ocr_noise` | character-confusion robustness |
| `fragmented` | separated copied blocks with unrelated insertion |
| `reordered` | moved passage thirds |
| `natural_relation` | naturally related editions or filing-year sections |

Every scenario records the exact source document. `natural_relation` can carry several acceptable positives:

```json
{
  "id":"84#section-0012:natural_relation",
  "profile":"natural_relation",
  "text":"...",
  "positive_ids":[
    "84#section-0012",
    "41445#section-0011",
    "42324#section-0013"
  ],
  "source_id":"84#section-0012",
  "relation_key":"gutenberg:mary wollstonecraft shelley:frankenstein or the modern prometheus:chapter 5"
}
```

For SEC sections, the natural relation key is the issuer CIK plus canonical item title. This labels the same issuer's Item 1A or Item 7 across filing years as related instead of incorrectly treating every prior-year section as a negative.

## Reproducibility contract

`showcase.json` records:

- raw and searchable corpus IDs;
- manifest SHA-256 digests;
- document and byte counts;
- scenario seed and passage length;
- query JSONL SHA-256;
- counts by profile;
- multi-positive relation counts.

Interrupted acquisition is resumable. Existing valid documents are reused. Existing section corpora are reused unless `--rebuild-sections` is supplied. `--replace-output` is required to delete the entire showcase root.

## Running the deliberately naive control

```bash
cargo run --release -p fo-bench --bin fo-exhaustive-bench -- \
  showcase/gutenberg/sections \
  showcase/gutenberg/queries.jsonl \
  --maximum-documents 250 \
  --maximum-cells-per-query 2000000000 \
  --maximum-total-cells 20000000000 \
  --output benchmark-artifacts/gutenberg-exhaustive.json \
  --scores-output benchmark-artifacts/gutenberg-exhaustive-scores.jsonl
```

The exhaustive baseline accepts the extra showcase fields and consumes `id`, `profile`, `text`, and `positive_ids`.

## Label interpretation

Controlled mutation profiles have exact generated provenance. Natural relation labels are silver labels based on stable edition or filing relationships; they are not a claim that every related document contains the exact query wording.

Public gold claims should therefore distinguish:

- controlled-source retrieval;
- natural related-document retrieval;
- manually adjudicated exact spans.
