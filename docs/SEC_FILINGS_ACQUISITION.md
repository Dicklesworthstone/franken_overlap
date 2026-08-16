# Multi-form SEC filing acquisition

`fo-sec-fetch` acquires a verified, resumable EDGAR corpus across the filings that matter to professional investors, rather than limiting the corpus to Form 10-K.

## Why broaden beyond 10-K

Important information often appears first, or appears only, in another filing family:

- `10-Q`: intra-year changes in risk factors, liquidity, controls, segments, KPIs, and accounting judgments;
- `8-K`: acquisitions, dispositions, financings, impairments, cyber incidents, restructurings, auditor changes, executive turnover, and material agreements;
- `DEF 14A`: compensation, ownership, governance, related-party transactions, and shareholder proposals;
- `S-1`, `F-1`, `S-3`, `F-3`, and `424B*`: offering economics, dilution, capitalization, use of proceeds, selling holders, and changes during the registration process;
- `20-F`, `40-F`, and `6-K`: foreign-private-issuer annual and current reporting;
- `UPLOAD` and `CORRESP`: SEC staff comments and issuer responses;
- amendments: restatements, corrected disclosure, and changed transaction terms.

## Acquire investor-core filings

```bash
export SEC_USER_AGENT='Example Research research@example.com'

cargo run --release -p fo-corpus --bin fo-sec-fetch -- fetch \
  --output corpora/sec-investor \
  --ticker AAPL \
  --ticker MSFT \
  --investor-core \
  --filings-per-company 60 \
  --from-date 2018-01-01
```

The investor-core preset includes annual, quarterly, current, proxy, and principal foreign-issuer reports.

## Add offerings and comment letters

```bash
cargo run --release -p fo-corpus --bin fo-sec-fetch -- fetch \
  --output corpora/sec-full \
  --ticker NVDA \
  --investor-core \
  --registration \
  --comment-letters \
  --filings-per-company 150 \
  --from-date 2015-01-01
```

Exact form names may also be supplied repeatedly:

```bash
--form 10-K --form 10-Q --form 8-K --form 'DEF 14A' --form UPLOAD --form CORRESP
```

## Historical submissions coverage

The SEC company-submissions endpoint exposes recent filing arrays and references to older submission JSON files. By default, `fo-sec-fetch` reads those historical files when their date coverage overlaps the requested range. This matters for issuers with long filing histories and for older comment-letter exchanges.

```bash
--include-historical-submission-files true
--maximum-historical-files-per-company 32
```

Each historical JSON object is parsed using the SEC's camel-case wire names such as `accessionNumber`, `filingDate`, `primaryDocument`, `isXBRL`, and `isInlineXBRL`.

## Manifest evidence

Every retained filing records:

```text
CIK
accession number
filing date
report date
acceptance timestamp
form and filing category
primary document
filing items, when supplied by EDGAR
issuer tickers
XBRL / inline-XBRL flags
declared and downloaded sizes
source URL
ETag and Last-Modified, when supplied
SHA-256
```

The corpus manifest also records the selected forms, date range, company-ticker snapshot digest, and whether historical submission files were read.

## Rate and safety policy

A contact-bearing SEC user agent is mandatory. The acquisition client enforces one global request interval and rejects configured rates above ten requests per second.

The downloader also enforces:

- bounded retry count and request timeout;
- bounded JSON and filing-document byte sizes;
- safe primary-document filenames;
- binary/PDF rejection rather than pretending binary bytes are text;
- minimum extracted-character count;
- resumable reuse of already verified local files;
- periodic manifest checkpoints;
- explicit failure receipts instead of silent omissions.

## Sectioning and search

The output is an ordinary `fo-corpus`:

```bash
cargo run --release -p fo-corpus --bin fo-section -- \
  corpora/sec-investor \
  --output corpora/sec-investor-sections \
  --strategy sec10k
```

Despite the historical name of the strategy, it recognizes general EDGAR `ITEM` headings, including 8-K item numbers such as `ITEM 5.02`, and falls back to bounded paragraph windows when no item headings exist.

The resulting section corpus can feed:

```text
fo-search
fo-domain-search
fo-document-first
fo-sec-lineage
fo-review-report
fo-evidence-suite
```

## Verification

```bash
cargo run --release -p fo-corpus --bin fo-sec-fetch -- verify \
  corpora/sec-investor
```

Verification checks every retained filing's path, byte length, and SHA-256 digest against the manifest.
