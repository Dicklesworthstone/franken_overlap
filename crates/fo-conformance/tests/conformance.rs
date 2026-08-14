#![forbid(unsafe_code)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_core::{
    FoError, Index, IndexBuilder, IndexConfig, NormalizationProfile, SearchOptions,
    SpectralOptions, spectral_scan,
};

const SOURCE: &str = include_str!("../../../fixtures/corpus/source.txt");
const ORIGIN: &str = include_str!("../../../fixtures/corpus/origin.txt");
const PARTIAL: &str = include_str!("../../../fixtures/corpus/partial.txt");
const NOISE: &str = include_str!("../../../fixtures/corpus/noise.txt");
const UNRELATED: &str = include_str!("../../../fixtures/corpus/unrelated.txt");
const EDITED: &str = include_str!("../../../fixtures/specimens/edited.txt");
const EDITED_ORIGIN: &str = include_str!("../../../fixtures/specimens/edited_origin.txt");

fn fixture_index() -> Index {
    let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
    for (path, text) in [
        ("source.txt", SOURCE),
        ("origin.txt", ORIGIN),
        ("partial.txt", PARTIAL),
        ("noise.txt", NOISE),
        ("unrelated.txt", UNRELATED),
    ] {
        builder.add_document(path, text).expect("document");
    }
    builder.build().expect("index")
}

fn permissive_options() -> SearchOptions {
    SearchOptions {
        minimum_similarity: 0.24,
        max_results: 10,
        ..SearchOptions::default()
    }
}

#[test]
fn edited_observatory_passage_ranks_source_first() {
    let hits = fixture_index()
        .search(EDITED, &permissive_options())
        .expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].path, "source.txt", "{hits:#?}");
    assert!(hits[0].combined_score > 0.45, "{:#?}", hits[0]);
}

#[test]
fn edited_methodology_passage_ranks_origin_first() {
    let hits = fixture_index()
        .search(EDITED_ORIGIN, &permissive_options())
        .expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].path, "origin.txt", "{hits:#?}");
}

#[test]
fn partial_reuse_is_recovered_as_a_supported_span() {
    let specimen = "A long preface was added by an editor. Every transformation must be documented and raw measurements should be preserved before rival causal models are compared. This unrelated epilogue discusses typography and printing.";
    let hits = fixture_index()
        .search(specimen, &SearchOptions {
            minimum_similarity: 0.18,
            ..SearchOptions::default()
        })
        .expect("search");
    assert!(hits.iter().any(|hit| hit.path == "partial.txt"), "{hits:#?}");
}

#[test]
fn unicode_width_case_and_punctuation_drift_normalize_away() {
    let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
    builder
        .add_document("unicode.txt", "ＴＨＥ Signal—Was VERIFIED, Twice!")
        .expect("document");
    let hits = builder
        .build()
        .expect("index")
        .search(
            "the signal was verified twice",
            &SearchOptions {
                minimum_similarity: 0.6,
                ..SearchOptions::default()
            },
        )
        .expect("search");
    assert_eq!(hits[0].path, "unicode.txt");
}

#[test]
fn unrelated_text_does_not_pass_a_strict_threshold() {
    let hits = fixture_index()
        .search(
            "Volcanic basalt cools into hexagonal columns while seawater erodes the surrounding cliff.",
            &SearchOptions {
                minimum_similarity: 0.72,
                ..SearchOptions::default()
            },
        )
        .expect("search");
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn persisted_index_produces_identical_query_results() {
    let index = fixture_index();
    let path = temporary_path("roundtrip.foidx");
    index.save(&path).expect("save");
    let loaded = Index::load(&path).expect("load");
    let before = index.search(EDITED, &permissive_options()).expect("before");
    let after = loaded.search(EDITED, &permissive_options()).expect("after");
    fs::remove_file(&path).ok();
    assert_eq!(before.len(), after.len());
    for (left, right) in before.iter().zip(after.iter()) {
        assert_eq!(left.path, right.path);
        assert_eq!(left.corpus_start, right.corpus_start);
        assert_eq!(left.corpus_end, right.corpus_end);
        assert!((left.combined_score - right.combined_score).abs() < 1e-6);
    }
}

#[test]
fn trailing_bytes_are_rejected_fail_closed() {
    let path = temporary_path("corrupt.foidx");
    fixture_index().save(&path).expect("save");
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open")
        .write_all(&[0x42])
        .expect("append");
    let error = Index::load(&path).expect_err("corruption must fail");
    fs::remove_file(&path).ok();
    assert!(matches!(error, FoError::InvalidIndex(_)), "{error:?}");
}

#[test]
fn dense_correlation_places_an_exact_peak() {
    let peaks = spectral_scan(
        "alpha beta gamma delta epsilon zeta",
        "gamma delta epsilon",
        &NormalizationProfile::default(),
        &SpectralOptions {
            minimum_score: 0.8,
            local_maximum_radius: 1,
            ..SpectralOptions::default()
        },
    )
    .expect("spectral scan");
    assert_eq!(peaks[0].matched_text, "gamma delta epsilon");
    assert!((peaks[0].score - 1.0).abs() < 1e-6);
}

fn temporary_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("franken-overlap-{}-{nonce}-{name}", std::process::id()))
}
