# Integration with FrankenNumPy, FrankenTorch, FrankenSciPy, and FrankenPandas

## Ownership rule

FrankenOverlap owns textual semantics. General numerical or device primitives should be implemented in the most reusable upstream library and consumed through narrow interfaces.

## FrankenSciPy

### Existing seam

Feature `frankenscipy` routes dense CountSketch channels through `fsci_fft::fftcorrelate` at a pinned commit.

### Required upstream work

A production scanner should not repeatedly allocate both padded operands and transform the specimen for every corpus block. Proposed reusable API:

```rust
PreparedCorrelationPlanF32
PreparedKernelSpectrumF32
CorrelationWorkspaceF32
OverlapSavePlanF32
```

Required operations:

- plan creation for a transform length and batch
- prepare/reuse specimen spectrum
- caller-provided input/output/scratch buffers
- batched real forward/inverse transforms
- fused conjugate multiply and channel accumulation
- two-real-channel complex packing
- overlap-save streaming
- threshold/non-maximum suppression/top-k without a full result allocation

Plan caches must include dtype, direction, length, batch, and normalization. Thread-local scratch should avoid a shared lock on every block.

## FrankenTorch

### Existing seam

FrankenTorch already exposes a generic Metal gateway and a resident fused `Batch` pattern. The latter is the correct throughput model.

### Proposed `ft-kernel-metal::spectral`

- `SpectralContext`: device, pipeline and persistent plan cache
- `GpuSpectralBuffer`: resident f32/complex channels
- `SpectralBatch`: one command buffer containing all stages
- CountSketch encoding kernel
- batched Stockham FFT kernels
- conjugate multiply/reduction kernel
- inverse FFT kernels
- local normalization/NMS/top-k compaction kernel

Only compact peak records return to the CPU. Shared-memory input buffers should support safe caller writes without an extra host-to-host copy. Multiple corpus blocks should be double or triple buffered with completion callbacks.

## FrankenNumPy

FrankenNumPy should provide interoperability, not matching policy:

- contiguous typed arrays for tokens, fingerprints, scores, and offsets
- safe strided/read-only candidate-span views
- prefix sums for local normalization
- Python buffer exposure
- `.npy`/`.npz` artifacts for benchmark or oracle exchange

A sliding-window view must not be materialized into an `N × m` array; that recreates the original quadratic problem.

## FrankenPandas

FrankenPandas should sit above the hot loop:

- corpus manifest ingestion
- document metadata joins
- source/language/date partitioning
- query result tables
- benchmark result analysis
- Arrow/Parquet/CSV/JSON export

Candidate voting, FFT, chaining, and edit distance remain flat-buffer Rust code. Results are converted to a DataFrame only after ranking.

Suggested result columns:

```text
doc_id
path
corpus_token_start
corpus_token_end
source_byte_start
source_byte_end
query_token_start
query_token_end
vote_support
anchor_score
anchor_coverage
spectral_score
edit_distance
edit_similarity
combined_score
normalization_profile_id
engine_route
```

## Dependency policy

- Default FrankenOverlap remains lightweight and CPU-only.
- Upstream integrations are feature-gated.
- Git dependencies are commit-pinned until compatible crates are released.
- No duplicate C/C++ FFT or GPU stack is introduced.
- Device-specific unsafe code stays inside the sanctioned upstream boundary.
