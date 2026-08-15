# PAN Text-Alignment Evaluation

`fo-pan` runs FrankenOverlap against PAN-format text-alignment corpora and evaluates PAN XML detections without a Python or NumPy dependency.

The PAN 2013 task defines one `pairs` file, suspicious/source text directories, and XML annotations containing `this_offset`, `this_length`, `source_offset`, and `source_length`. Offsets and lengths are Unicode character counts in the original documents.

Official task description: <https://pan.webis.de/clef13/pan13-web/text-alignment.html>

PAN13 corpus archive: <https://zenodo.org/records/3715980>

## Evaluate existing detections

```bash
cargo run -p fo-bench --bin fo-pan -- evaluate \
  ./truth \
  ./detections \
  --json
```

The report contains macro and micro recall, precision, F1, granularity, and PlagDet.

## Run FrankenOverlap

```bash
cargo run -p fo-bench --bin fo-pan -- run \
  ./pairs \
  ./src \
  ./susp \
  ./truth \
  --output ./detections \
  --minimum-score 0.12 \
  --minimum-block-tokens 20 \
  --maximum-blocks 24 \
  --json
```

Each pair is evaluated independently with a one-document source index. Composite passage search can emit several separated or reordered detections for the same pair. The output directory contains PAN-compatible `detected-plagiarism` XML files.

## Metric compatibility

The Rust implementation follows PAN's reference evaluator semantics:

- external annotations overlap only when both suspicious and source spans overlap and both references agree,
- macro recall averages paired character coverage per reference case,
- macro precision applies the same calculation with detections and cases reversed,
- micro coverage unions overlapping characters before counting,
- granularity measures the number of detections used for each detected case,
- PlagDet divides harmonic precision/recall by `log2(1 + granularity)`.

Both suspicious and source characters contribute to external-case coverage, matching the reference implementation.

## Original-coordinate correctness

The matcher operates in normalized Unicode-scalar coordinates. `normalize_with_provenance` maps each detected query/source token span back to original UTF-8 bytes; `fo-pan` then converts those byte ranges to PAN character offsets. Compatibility folding, combining marks, punctuation replacement, and collapsed whitespace therefore do not silently corrupt evaluation spans.

## Corpus handling

The corpus is not vendored or redistributed. The runner consumes a local unpacked PAN corpus. Document references must be relative paths without parent traversal, and XML parsing fails closed on malformed or missing required attributes.
