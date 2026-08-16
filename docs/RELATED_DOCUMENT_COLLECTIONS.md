# Related-document collections

`fo-collection` turns an arbitrary directory of related UTF-8 documents into a verified `fo-corpus` plus a richer `collection.json` relationship manifest.

The collection layer is intended for agreements, policies, filings, research documents, source-code snapshots, and any corpus where versions or related families matter.

## Import

```bash
cargo run --release -p fo-corpus --bin fo-collection -- import \
  ./raw-leases \
  --output ./corpora/leases \
  --collection-id retail-leases \
  --profile retail-lease \
  --metadata ./lease-metadata.jsonl
```

The importer copies verified UTF-8 bytes into `documents/`, writes SHA-256 receipts, emits the ordinary `manifest.json` consumed by `fo-search`, and writes `collection.json` with family/version metadata and relations.

## Metadata JSONL

Each row is keyed by `source_path` relative to the import root:

```json
{
  "source_path":"store-017/lease-2025-01-01.txt",
  "id":"store-017-lease-2025",
  "title":"Store 17 retail lease amendment",
  "family_id":"store-017-lease",
  "version_id":"2025-amendment-2",
  "document_type":"retail_lease_amendment",
  "effective_date":"2025-01-01",
  "executed_date":"2024-12-14",
  "parties":["Example Landlord LLC","Example Retailer Inc."],
  "tags":["retail","lease","northeast"],
  "previous_version_id":"store-017-lease-2023",
  "metadata":{"property_id":"store-017","state":"NY"}
}
```

Supported explicit relations include previous version, amendment, supersession, and arbitrary related documents. When enabled, the importer infers `previous_version` edges by sorting documents inside each family by effective date and version ID.

## Profiles

Built-in profiles are:

```text
general
sec-filings
retail-lease
professional-services
nda
contract
policy
source-code
research
```

A profile supplies a default document type and is retained as searchable metadata. Later contract and investor analyzers use it to choose clause taxonomies and alert policies.

## Verification and inspection

```bash
cargo run --release -p fo-corpus --bin fo-collection -- verify ./corpora/leases

cargo run --release -p fo-corpus --bin fo-collection -- inspect \
  ./corpora/leases \
  --family store-017-lease
```

Verification checks both manifests, every relation endpoint, every safe relative path, byte lengths, and SHA-256 digests.

## Search integration

The output is an ordinary corpus input:

```bash
cargo run --release -p fo-cli --bin fo-search -- build \
  ./corpora/leases \
  --input-format corpus \
  --output ./indexes/leases.fohybrid
```

Collection metadata such as `family_id`, `version_id`, `document_type`, dates, parties, and tags flows into the hybrid index for filtering and downstream lineage analysis.
