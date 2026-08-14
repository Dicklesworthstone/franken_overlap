# Algorithmic Specification

## Categorical invariance

Any legal text representation must be invariant under a permutation of symbol IDs. Unicode scalar values and BPE token IDs can be stored as integers, but the integers are labels. Direct products such as `token_a * token_b` do not measure textual similarity.

FrankenOverlap uses equality-preserving features throughout.

## Sparse q-gram correlation

Let the normalized specimen be `P[0..m)` and a corpus document be `T[0..n)`. A q-gram fingerprint at specimen position `q` and an equal fingerprint at corpus position `c` support alignment diagonal

```text
d = c - q.
```

Accumulating all such differences is equivalent to sparse cross-correlation of one-hot feature channels. The initial engine quantizes diagonals into bins to tolerate small local shifts during candidate generation.

### Fingerprints

The implementation computes two rolling polynomial hashes under wrapping 64-bit arithmetic and then avalanches each value. The pair is the persistent 128-bit fingerprint. Equal q-grams are guaranteed to produce equal fingerprints. Unequal q-grams can collide, but collisions only create candidates; verification compares normalized tokens.

### Winnowing

For every window of consecutive q-gram hashes, select the rightmost minimum. The rightmost tie rule makes selection deterministic and prevents repeated emission while a minimum remains in adjacent windows.

### Frequency weighting

For a fingerprint with document frequency `df` among `D` documents, the initial weight is

```text
idf = ln((D + 1) / (df + 1)) + 1.
```

Posting lists above a configurable cap are skipped. This suppresses boilerplate and low-information features while retaining rare evidence.

## Candidate generation

Votes are keyed by `(document_id, diagonal_bin)`. Candidates are sorted by weighted vote mass, raw anchor hits, document ID, and diagonal. Nearby bins in the same document are suppressed before anchor expansion.

This stage is intentionally high recall. Its score is not reported as an exact similarity probability.

## Anchor chaining

An anchor is

```text
(query_position, corpus_position, span, weight).
```

Anchors are sorted monotonically in both coordinate systems. A dynamic program chooses a predecessor among a bounded lookback window when both query and corpus positions increase and gaps remain below a configured maximum.

For query gap `g_q` and corpus gap `g_c`, the transition penalty is

```text
|g_q - g_c| · drift_penalty
+ ln(1 + max(g_q, g_c) / 32) · gap_penalty.
```

The first term penalizes insertion/deletion drift. The logarithmic term is concave, so one coherent long gap costs less than many unrelated jumps.

The chain records:

- total score
- supported query and corpus endpoints
- union coverage of query intervals
- median diagonal
- complete anchor sequence

## Verification

The chain determines a supported specimen interval. One q-gram of context is added on each side. The corpus verification window is centered on the median diagonal and enlarged by configurable slack.

The semi-global dynamic program gives free corpus prefix/suffix choice while charging edits inside the selected match. Each DP cell stores both cost and alignment start. Ties prefer:

1. lower edit cost
2. start closer to the predicted diagonal
3. matched length closer to the specimen segment length

The verifier first restricts computation to a band. If the band cannot produce a finite path, it reruns without the band. This fallback favors correctness over latency.

## Composite ranking

The initial score is

```text
0.62 · edit_similarity
+ 0.25 · anchor_coverage
+ 0.08 · tanh(normalized_anchor_score)
+ 0.05 · vote_support.
```

The coefficients are an explicit initial policy, not a permanent statistical claim. A benchmark-calibrated ranker may replace them, but each evidence component remains separately available.

## Short specimens

When the normalized specimen is shorter than the configured q-gram size, the index cannot produce q-gram seeds. The reference fallback directly evaluates fixed-length Hamming candidates and verifies the best local windows semi-globally. Because this path only applies to short patterns, its `O(nm)` reference cost remains bounded.

The production portfolio will replace much of this path with Shift-Or/Myers and SIMD kernels.

## Dense CountSketch correlation

For repetition `r`, token `x` maps to bucket `b_r(x)` and sign `σ_r(x) ∈ {-1,+1}`. Define

```text
X[r,b,i] = σ_r(T[i]) if b_r(T[i]) = b else 0
Y[r,b,j] = σ_r(P[j]) if b_r(P[j]) = b else 0.
```

The score at offset `s` is

```text
S(s) = 1/(Rm) · Σ_r Σ_b corr(X[r,b], Y[r,b])(s).
```

For equal tokens, bucket and sign agree, contributing `+1`. For unequal tokens, contribution occurs only on bucket collision and has expectation zero under independent signs.

The default implementation evaluates this expression directly under a work cap. With the `frankenscipy` feature, each channel uses FFT correlation. Local non-maximum suppression returns compact peaks rather than every score.

## Future heavy-light exactness

A feature-specific dispatcher will estimate `|Q_a||C_a|` and choose:

- posting-pair enumeration for rare features
- blocked/bucketed correlation for medium features
- bitset/FFT channels for heavy features
- suppression for uninformative features

This produces an exact or controlled-estimator hybrid without forcing every feature through the same representation.
