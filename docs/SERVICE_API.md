# Resident query service

`fo-serve` keeps one persisted hybrid index resident in memory and exposes the existing explainable search APIs over a small dependency-free HTTP/1.1 service.

The service is intended for:

- analyst workstations;
- trusted internal services;
- benchmark and integration environments;
- repeated high-throughput queries where loading an index for every command would dominate wall time.

It is not a distributed search cluster or a general-purpose web framework.

## Start locally

```bash
cargo run --release -p fo-cli --bin fo-serve -- \
  indexes/sec-items.fohybrid \
  --bind 127.0.0.1:7070 \
  --threads 16 \
  --queue-capacity 256
```

The index is loaded and validated before the listening socket opens.

## Search

```bash
curl -sS http://127.0.0.1:7070/v1/search \
  -H 'content-type: application/json' \
  --data '{
    "query":"material weakness liquidity covenant",
    "mode":"auto",
    "limit":20
  }'
```

Response variants are explicitly tagged:

```json
{"kind":"hybrid","report":{...}}
{"kind":"domain","report":{...}}
{"kind":"semantic","report":{...}}
```

## Domain-aware overlap

```bash
curl -sS http://127.0.0.1:7070/v1/search \
  -H 'content-type: application/json' \
  --data '{
    "query":"<long Item 1A specimen>",
    "domain":"sec_filing",
    "minimum_similarity":0.2,
    "minimum_matched_tokens":48,
    "minimum_query_coverage":0.2
  }'
```

A request can supply an explicit `domain_policy`; otherwise the built-in general, SEC, contract, OCR, or source-code policy is used.

The service uses `search_domain_adaptive`, so a one- or two-document history does not lose every unique feature to an unattainable document-frequency or IDF threshold.

## Semantic candidate fusion

External semantic candidates can be supplied with an ordinary hybrid request:

```json
{
  "query":"a heavily paraphrased risk disclosure",
  "mode":"hybrid",
  "semantic_candidates":{
    "schema_version":1,
    "query_id":"risk-17",
    "candidates":[{
      "external_id":"CIK0000320193#section-0004",
      "title":"Apple Item 1A",
      "score":0.91,
      "model":"example-embedding-model"
    }]
  }
}
```

The response retains textual, lexical, and semantic evidence separately. Semantic similarity never becomes an implicit textual-provenance claim.

## Index and document metadata

```bash
curl -sS http://127.0.0.1:7070/v1/index
```

```bash
curl -sS \
  'http://127.0.0.1:7070/v1/document/CIK0000320193%23section-0004'
```

The document endpoint returns title, tags, metadata, and body byte count. It deliberately does not expose the complete indexed body.

## Health and metrics

`GET /health` is always available:

```json
{
  "status":"ok",
  "uptime_seconds":42,
  "documents":12000,
  "in_flight":3,
  "queue_capacity":256,
  "worker_threads":16
}
```

`GET /metrics` returns Prometheus text including:

```text
accepted connections
queue rejections
requests and completions
failures and unauthorized requests
oversized requests
in-flight work
search requests
bytes in and out
cumulative latency histogram, sum, and count
```

## Bounded concurrency

The service uses:

- a fixed worker count;
- a bounded synchronous request queue;
- one immutable shared `HybridIndex`;
- one request per connection;
- explicit request and query byte limits;
- read and write timeouts;
- no nested per-query worker pool.

When the queue is full, a new connection receives HTTP 503 immediately instead of creating unbounded tasks or memory pressure.

Important controls:

```text
--threads 8
--queue-capacity 128
--maximum-request-bytes 1048576
--maximum-query-bytes 262144
--read-timeout-ms 15000
--write-timeout-ms 30000
--default-limit 20
--candidate-multiplier 8
```

## Authentication

Loopback binds may run without a token. A non-loopback bind fails unless either:

```text
--api-key-env VARIABLE
```

or the explicit escape hatch is supplied:

```text
--allow-unauthenticated-public
```

Example:

```bash
export FRANKEN_OVERLAP_API_KEY='replace-with-a-long-random-token'

fo-serve indexes/sec.fohybrid \
  --bind 0.0.0.0:7070 \
  --api-key-env FRANKEN_OVERLAP_API_KEY
```

```bash
curl -sS http://host:7070/v1/search \
  -H "authorization: Bearer $FRANKEN_OVERLAP_API_KEY" \
  -H 'content-type: application/json' \
  --data '{"query":"liquidity covenant maturity"}'
```

The token is read from the environment and is not accepted on the command line.

## HTTP scope

The implementation intentionally supports a narrow protocol surface:

- HTTP/1.x request lines;
- `GET` and `POST`;
- required `Content-Length` for POST;
- no chunked transfer encoding;
- one response followed by connection close;
- strict header, body, and timeout limits;
- JSON error objects;
- optional fixed `Access-Control-Allow-Origin`.

For internet-facing deployment, place it behind a mature reverse proxy that provides TLS, request logging, rate limiting, network policy, and authentication appropriate to the environment.

## Search request fields

A request can control:

```text
query
mode                       auto | lexical | overlap | hybrid
domain                     optional domain-aware overlap route
limit
candidate_multiplier
minimum_score
minimum_similarity
minimum_matched_tokens
minimum_query_coverage
minimum_source_coverage
maximum_postings_per_feature
maximum_postings_per_term
lexical_candidates
filter                      external-ID/tag/metadata filters
domain_policy               optional explicit policy
semantic_candidates
semantic_options
```

All embedded options are validated by the same core types used by the CLIs.

## Operational interpretation

The service removes repeated index-load overhead and supplies the bounded-concurrency and observability foundation required for a practical provenance product. It does not itself decide which score thresholds are correct. Production defaults should come from held-out evidence and the experiment/promotion registry.
