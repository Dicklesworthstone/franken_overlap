#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn runs_benchmark_scores_and_bundle_as_one_immutable_suite() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("documents");
    let source = repeated(
        "observatory telescope detector photon spectrum calibration measurement orbit",
        48,
    );
    let distractor = repeated(
        "issuer liquidity covenant maturity filing capital market portfolio valuation",
        48,
    );
    fs::write(documents.join("source.txt"), &source).expect("source");
    fs::write(documents.join("distractor.txt"), &distractor).expect("distractor");
    let mut manifest = CorpusManifest::new("suite-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "source",
        "documents/source.txt",
        "Astronomy",
        &source,
    ));
    manifest.upsert_document(document(
        "distractor",
        "documents/distractor.txt",
        "Finance",
        &distractor,
    ));
    manifest.save(&corpus).expect("manifest");

    let queries = root.join("queries.jsonl");
    let query = serde_json::json!({
        "id":"source:exact",
        "profile":"exact",
        "text":"observatory telescope detector photon spectrum calibration measurement orbit observatory telescope detector photon spectrum calibration measurement orbit observatory telescope detector photon spectrum calibration measurement orbit",
        "positive_ids":["source"],
        "source_id":"source",
        "source_title":"Astronomy",
        "relation_key":"astronomy",
        "metadata":{"passage_start_word":"0","passage_words":"24"}
    });
    fs::write(
        &queries,
        format!("{}\n", serde_json::to_string(&query).expect("query")),
    )
    .expect("queries");

    let suite = root.join("suite");
    let output = Command::new(env!("CARGO_BIN_EXE_fo-evidence-suite"))
        .arg(&corpus)
        .arg(&queries)
        .arg("--output")
        .arg(&suite)
        .arg("--corpus-size")
        .arg("2")
        .arg("--maximum-documents")
        .arg("2")
        .arg("--warmup-runs")
        .arg("0")
        .arg("--measurement-repetitions")
        .arg("1")
        .arg("--maximum-exhaustive-cells-per-query")
        .arg("100000000")
        .arg("--maximum-exhaustive-cells-per-scale")
        .arg("100000000")
        .arg("--json")
        .output()
        .expect("run suite");
    assert!(
        output.status.success(),
        "suite failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("suite report");
    assert_eq!(report["status"], "complete");
    assert_eq!(report["claim_status"], "not_evaluated");
    assert_eq!(report["evaluated_corpus_sizes"][0], 2);
    for path in [
        "proof.json",
        "scores.jsonl",
        "suite.json",
        "suite-status.json",
        "bundle/RESULTS.md",
        "bundle/RESULTS.html",
        "bundle/environment.json",
        "bundle/examples.json",
        "bundle/artifacts.json",
    ] {
        assert!(suite.join(path).is_file(), "missing {path}");
    }
    let status: serde_json::Value =
        serde_json::from_slice(&fs::read(suite.join("suite-status.json")).expect("status"))
            .expect("status json");
    assert_eq!(status["status"], "complete");
    let markdown = fs::read_to_string(suite.join("bundle/RESULTS.md")).expect("markdown");
    assert!(markdown.contains("No claim report was supplied"));

    let second = Command::new(env!("CARGO_BIN_EXE_fo-evidence-suite"))
        .arg(&corpus)
        .arg(&queries)
        .arg("--output")
        .arg(&suite)
        .output()
        .expect("rerun suite");
    assert!(!second.status.success(), "suite overwrote immutable output");

    fs::remove_dir_all(root).ok();
}

fn document(id: &str, relative_path: &str, title: &str, body: &str) -> CorpusDocument {
    CorpusDocument {
        id: id.to_owned(),
        relative_path: relative_path.to_owned(),
        source_url: format!("https://example.invalid/{id}"),
        title: title.to_owned(),
        author_or_issuer: "fixture".to_owned(),
        language: Some("en".to_owned()),
        published_or_filed: None,
        sha256: sha256_hex(body.as_bytes()),
        bytes: body.len() as u64,
        characters: body.chars().count(),
        downloaded_at_unix: 0,
        metadata: BTreeMap::new(),
    }
}

fn repeated(vocabulary: &str, repetitions: usize) -> String {
    std::iter::repeat_n(vocabulary, repetitions)
        .collect::<Vec<_>>()
        .join(" ")
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-evidence-suite-test-{}-{nonce}",
        std::process::id()
    ))
}
