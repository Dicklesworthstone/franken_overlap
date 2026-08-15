# Native corpus acquisition

`fo-corpus` downloads, cleans, resumes, and verifies real corpora without shell wrappers.

## Project Gutenberg

The downloader obtains the official machine-readable CSV catalog, deterministically samples text records, downloads generated UTF-8 text files, removes standard Gutenberg headers and footers, and records a SHA-256 manifest.

```bash
cargo run -p fo-corpus -- gutenberg \
  --preset smoke \
  --output corpora/gutenberg-smoke
```

Bulk acquisition must use an explicit Project Gutenberg mirror. Runs above 100 books fail closed when pointed at the main-site default:

```bash
GUTENBERG_MIRROR=https://your-nearby-mirror.example/cache/epub \
  cargo run -p fo-corpus -- gutenberg \
  --preset large \
  --output corpora/gutenberg-large
```

The presets are deterministic under `--seed`:

- `smoke`: 25 English texts
- `standard`: 250 English texts
- `large`: 2,500 English texts

Explicit IDs are also supported with repeated `--id` arguments.

## SEC Form 10-K filings

SEC acquisition resolves tickers through the SEC ticker/CIK association file, reads company submission histories from `data.sec.gov`, selects recent 10-K primary documents, converts filing HTML to text, and records filing metadata and hashes.

The SEC requires a declared bot identity. Supply it explicitly or through `SEC_USER_AGENT`:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
  cargo run -p fo-corpus -- sec10k \
  --ticker AAPL \
  --ticker MSFT \
  --filings-per-company 5 \
  --output corpora/sec-10k
```

A deterministic cross-company sample requires no hard-coded company list:

```bash
SEC_USER_AGENT='Example Research research@example.com' \
  cargo run -p fo-corpus -- sec10k \
  --preset standard \
  --from-date 2018-01-01 \
  --output corpora/sec-standard
```

The default rate is five requests per second. Values above the SEC's ten-request-per-second ceiling are rejected.

## Reproducibility and resumption

Every corpus root contains:

```text
manifest.json
documents/
metadata/
```

The manifest records source URLs, snapshot metadata, original identifiers, titles, issuer/author information, dates, byte counts, character counts, SHA-256 digests, and failures. Existing files whose digest matches the manifest are reused, so interrupted downloads resume without repeating completed work.

Verify a corpus at any time:

```bash
cargo run -p fo-corpus -- verify corpora/gutenberg-smoke
cargo run -p fo-corpus -- show corpora/gutenberg-smoke --json
```

Acquisition is bounded by per-object byte limits, redirect limits, request timeouts, retry limits, rate limits, safe relative paths, and atomic file publication.
