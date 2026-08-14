#![forbid(unsafe_code)]

use franken_overlap::{IndexBuilder, IndexConfig, SearchOptions};

#[test]
fn facade_supports_an_end_to_end_query() {
    let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
    builder
        .add_document(
            "paper.txt",
            "Preserve the raw measurements and document every transformation before comparing causal models.",
        )
        .expect("document");
    let results = builder
        .build()
        .expect("index")
        .search(
            "Document each transformation and preserve the raw measurements before comparing causal models.",
            &SearchOptions {
                minimum_similarity: 0.2,
                ..SearchOptions::default()
            },
        )
        .expect("search");
    assert_eq!(results[0].path, "paper.txt");
}
