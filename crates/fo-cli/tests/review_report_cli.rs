#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_core::{NormalizationProfile, SearchIntent, SearchResult, normalize};
use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn renders_original_source_and_specimen_highlights() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("create corpus");
    let source = "alpha beta gamma delta epsilon";
    let specimen = "beta gamma";
    fs::write(documents.join("source.txt"), source).expect("source");
    let mut manifest = CorpusManifest::new("review-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(CorpusDocument {
        id: "source".to_owned(),
        relative_path: "documents/source.txt".to_owned(),
        source_url: "https://example.invalid/source".to_owned(),
        title: "Source document".to_owned(),
        author_or_issuer: "Fixture Author".to_owned(),
        language: Some("en".to_owned()),
        published_or_filed: Some("2026-08-15".to_owned()),
        sha256: sha256_hex(source.as_bytes()),
        bytes: source.len() as u64,
        characters: source.chars().count(),
        downloaded_at_unix: 0,
        metadata: BTreeMap::from([("kind".to_owned(), "fixture".to_owned())]),
    });
    manifest.save(&corpus).expect("save manifest");

    let specimen_path = root.join("specimen.txt");
    fs::write(&specimen_path, specimen).expect("specimen");
    let profile = NormalizationProfile::default();
    let normalized_source = normalize(source, &profile);
    let normalized_specimen = normalize(specimen, &profile);
    let corpus_start = normalized_source
        .text
        .find(&normalized_specimen.text)
        .expect("source offset");
    let result = SearchResult {
        document_id: 0,
        path: "source".to_owned(),
        intent: SearchIntent::SourceAttribution,
        corpus_start,
        corpus_end: corpus_start + normalized_specimen.len(),
        query_start: 0,
        query_end: normalized_specimen.len(),
        edit_distance: 0,
        edit_similarity: 1.0,
        anchor_coverage: 1.0,
        query_coverage: 1.0,
        source_coverage: normalized_specimen.len() as f32 / normalized_source.len() as f32,
        anchor_score: 1.0,
        vote_support: 1.0,
        chain_consistency: 1.0,
        matched_tokens: normalized_specimen.len(),
        distinct_anchor_count: 2,
        estimated_false_matches: 0.0,
        combined_score: 0.99,
        matched_text: specimen.to_owned(),
    };
    let result_path = root.join("results.json");
    fs::write(
        &result_path,
        serde_json::to_vec_pretty(&vec![result]).expect("serialize results"),
    )
    .expect("results");
    let output = root.join("review");

    let command = Command::new(env!("CARGO_BIN_EXE_fo-review-report"))
        .arg(&corpus)
        .arg(&specimen_path)
        .arg(&result_path)
        .arg("--output")
        .arg(&output)
        .arg("--target-id")
        .arg("specimen-fixture")
        .output()
        .expect("run review report");
    assert!(
        command.status.success(),
        "review report failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&command.stdout),
        String::from_utf8_lossy(&command.stderr)
    );

    let html = fs::read_to_string(output.join("index.html")).expect("html");
    assert!(html.contains("Source document"));
    assert!(html.contains("<mark>beta gamma</mark>"));
    assert!(html.contains("Download decisions.jsonl"));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("review.json")).expect("review JSON"))
            .expect("parse review");
    assert_eq!(report["target_id"], "specimen-fixture");
    assert_eq!(report["candidates"][0]["external_id"], "source");
    assert_eq!(report["candidates"][0]["blocks"][0]["source_byte_start"], 6);
    let decisions = fs::read_to_string(output.join("decisions.jsonl")).expect("decisions");
    assert!(decisions.contains("\"decision\":\"unreviewed\""));
    let artifacts: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("artifacts.json")).expect("artifacts"))
            .expect("parse artifacts");
    assert_eq!(artifacts["files"].as_array().map(Vec::len), Some(3));

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-review-report-{}-{nonce}",
        std::process::id()
    ))
}
