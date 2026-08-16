# Contract version and portfolio intelligence

`fo-contract-compare` converts deterministic contract analyses into two higher-value outputs:

1. **Version intelligence:** what changed between two agreements or adjacent versions in one family.
2. **Portfolio intelligence:** what is common, missing, rare, or economically unusual across a lease or contract portfolio.

## Pair comparison

```bash
cargo run --release -p fo-cli --bin fo-contract-compare -- pair \
  analysis/store-17-2023.json \
  analysis/store-17-2025.json \
  --output comparisons/store-17.json
```

The comparison aligns clauses using clause type, heading similarity, and normalized text similarity. It reports:

- unchanged, moved, minor-revision, material-revision, added, and removed clauses;
- added, removed, and changed definitions;
- added, removed, changed, strengthened, and weakened obligations;
- added, removed, increased, decreased, and otherwise changed economic terms;
- source coordinates in each version;
- investor-impact category, direction, and bounded materiality score;
- deterministic alerts sorted by severity.

Important impact categories include economics, duration and renewal, termination and default, liability and indemnity, exclusivity and competition, operational flexibility, IP, data and cybersecurity, compliance, assignment/change of control, and real-estate operations.

## Portfolio analysis

First extract every document:

```bash
cargo run --release -p fo-cli --bin fo-contract-analyze -- collection \
  corpora/retail-leases \
  --output analysis/retail-leases
```

Then compare adjacent versions and benchmark the portfolio:

```bash
cargo run --release -p fo-cli --bin fo-contract-compare -- portfolio \
  corpora/retail-leases \
  analysis/retail-leases \
  --output intelligence/retail-leases
```

The output contains:

```text
summary.json
SUMMARY.md
benchmark.json
alerts.jsonl
comparisons/<family>--<previous>--<current>.json
```

Portfolio benchmarking calculates:

- clause prevalence by document;
- missing clauses that are common elsewhere in the portfolio;
- clauses that are rare across the portfolio;
- distributions of extracted economic terms;
- robust median/MAD outliers;
- obligation-modality counts.

For retail leases, this can expose unusual rent, escalation, deposits, tenant-improvement allowances, co-tenancy rights, exclusivity, kick-out rights, assignment restrictions, and renewal terms. For services agreements it can surface unusual liability caps, payment terms, acceptance procedures, service levels, IP ownership, and termination rights. For NDAs it can expose residuals, standstill, no-hire, non-solicitation, unusual term length, or missing exclusions.

## Interpretation boundary

A change direction is intentionally described as more restrictive, less restrictive, higher or lower economic burden, added or removed protection, or ambiguous. It is not labeled universally favorable or unfavorable because contractual incidence depends on the party, negotiating context, and surrounding provisions.

Every alert is traceable to structured extraction and original source coordinates. Portfolio rarity is not itself materiality; it is a triage signal for expert review.
