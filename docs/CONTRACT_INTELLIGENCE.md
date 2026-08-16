# Contract intelligence

`fo-contract-analyze` converts agreements into reviewable clause, definition, obligation, and economic-term evidence. Every extracted record retains its original UTF-8 byte coordinates and source sentence or context.

## Profiles

```text
general
retail-lease
professional-services
nda
```

The common taxonomy covers term, renewal, fees, invoicing, payment, confidentiality, IP, privacy, security, audit, compliance, warranties, indemnification, liability, insurance, assignment, change of control, termination, force majeure, governing law, disputes, notices, amendment, waiver, severability, and survival.

### Retail leases

The profile recognizes premises and use, base and percentage rent, CAM, utilities, taxes, maintenance, alterations, signage, assignment, subletting, co-tenancy, exclusivity, go-dark, kick-out, radius restrictions, renewal options, security deposits, tenant-improvement allowances, delivery conditions, opening covenants, operating hours, casualty, condemnation, SNDA, estoppel, holdover, surrender, and guaranties.

### Professional services

The profile recognizes scope, statements of work, deliverables, milestones, acceptance, change orders, fees, expenses, invoicing, payment, service levels, staffing, subcontracting, work product, background IP, open-source restrictions, data protection, security, and transition assistance.

### NDAs

The profile recognizes confidential-information definitions, exclusions, use restrictions, permitted recipients, compelled disclosure, return or destruction, residuals, no-license language, standstill, non-solicitation, no-hire, non-circumvention, and injunctive relief.

## Analyze one agreement

```bash
cargo run --release -p fo-cli --bin fo-contract-analyze -- document \
  agreement.txt \
  --profile professional-services \
  --output agreement.analysis.json
```

The JSON includes:

- heading-aware clause spans and classifications;
- definitions using `means`, `shall mean`, and related forms;
- modal obligations with subject, modality, action, trigger, deadline, and remedy evidence;
- money, percentages, durations, payment terms, notice periods, rent escalations, liability caps, insurance limits, service levels, and renewal terms;
- missing-expected-clause and duplicate-definition warnings.

## Analyze a collection

```bash
cargo run --release -p fo-cli --bin fo-contract-analyze -- collection \
  corpora/retail-leases \
  --output analysis/retail-leases \
  --threads 16
```

The collection command verifies both manifests, analyzes documents in parallel, and writes:

```text
summary.json
SUMMARY.md
documents/<stable-document-id>.json
```

The summary aggregates clause coverage, warnings, obligations, and economic terms across the portfolio. Individual analyses feed version comparison, portfolio benchmarking, lineage, and analyst review.

## Review boundary

The extractor produces candidates and source evidence. Important terms can be expressed in unusual language, tables, exhibits, amendments, or scanned images. Missing extraction does not prove that a clause is absent. Domain review and held-out evaluation remain part of the workflow.
