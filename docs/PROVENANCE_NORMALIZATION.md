# Provenance-Preserving Normalization

FrankenOverlap's matching coordinates are normalized Unicode-scalar positions. External corpora and review tools usually describe spans in the original UTF-8 document. `normalize_with_provenance` preserves a deterministic bridge between those coordinate systems.

## Contract

The function returns:

- the original UTF-8 text,
- the exact `NormalizedText` produced by the ordinary normalization profile,
- one original byte range for every normalized token.

Compatibility expansion, lowercase expansion, combining-mark composition, punctuation replacement, and collapsed whitespace may map several normalized tokens to one original range or one normalized separator to a larger original range.

```rust
use franken_overlap::{NormalizationProfile, normalize_with_provenance};

let text = normalize_with_provenance(
    "Ａ---  Cafe\u{301}",
    &NormalizationProfile::default(),
);
assert_eq!(text.normalized.text, "a café");
assert_eq!(text.original_slice_for_tokens(1, 2), Some("---  "));
assert_eq!(text.original_slice_for_tokens(2, 6), Some("Cafe\u{301}"));
```

## Normalization stability

Extended grapheme clusters are normalized independently so every emitted scalar can normally retain a precise source range. The implementation also computes the canonical ordinary normalization result. If an unusual Unicode sequence would differ across a grapheme boundary, it preserves the canonical normalized text and conservatively maps the affected output to the complete original input rather than returning incorrect offsets.

The ordinary index format and existing token-coordinate API are unchanged. Provenance is an opt-in layer for evaluators, corpus adapters, highlighting, and future index formats.

## Complexity

Time is linear in the input plus the ordinary normalization pass. Memory is linear in the number of normalized Unicode scalars. No per-token heap allocation occurs beyond the contiguous range vector.
