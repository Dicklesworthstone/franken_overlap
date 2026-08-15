# Dense Spectral Backends

Dense scanning now uses two execution lanes selected by the declared comparison budget.

## Exact direct lane

When

```text
(corpus_tokens - specimen_tokens + 1) × specimen_tokens
```

is within `direct_work_limit`, FrankenOverlap computes the exact fraction of equal categorical tokens at every offset. The result is independent of sketch repetitions and phase buckets, has no collision noise, and performs one equality comparison per token pair rather than repeating CountSketch work.

## Unit-circle phase FFT lane

Larger workloads use the optional FrankenSciPy backend. Every token receives a deterministic unit vector on a quantized circle for each repetition:

```text
v_r(token) = (cos θ, sin θ)
```

Equal tokens contribute `cos² θ + sin² θ = 1`. Independently hashed unequal tokens have zero expected dot product. Each repetition requires two real cross-correlations, one for each component.

The previous bucketed CountSketch implementation required one correlation for every `(repetition, bucket)` pair. With the default four repetitions and eight buckets, that meant 32 real correlations. The phase representation requires eight, a 4× reduction in channel correlations before future prepared-spectrum and batching work.

The serialized `buckets` option is retained for compatibility and now controls the number of quantized phases. More phases reduce exact phase collisions; more repetitions reduce estimator variance.

Dense mode remains a candidate-generation and heat-map tool. Indexed sparse retrieval is normally preferable for repeated queries because it avoids scanning unrelated corpus text.
