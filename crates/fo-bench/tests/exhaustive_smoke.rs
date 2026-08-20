#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn compares_exhaustive_and_indexed_retrieval_end_to_end() {
    let root = temporary_root();
    let documents = root.join("documents");
    fs::create_dir_all(&documents).expect("create documents");

    let astronomy = repeated(
        "observatory telescope detector photon spectrum calibration measurement orbit",
        48,
    );
    let finance = repeated(
        "issuer liquidity covenant maturity filing capital market portfolio valuation",
        48,
    );
    fs::write(documents.join("astronomy.txt"), &astronomy).expect("astronomy");
    fs::write(documents.join("finance.txt"), &finance).expect("finance");

    let mut manifest = CorpusManifest::new("exhaustive-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "astronomy",
        "documents/astronomy.txt",
        "Astronomy",
        &astronomy,
    ));
    manifest.upsert_document(document(
        "finance",
        "documents/finance.txt",
        "Finance",
        &finance,
    ));
    manifest.save(&root).expect("manifest");

    let queries = root.join("queries.jsonl");
    fs::write(
        &queries,
        concat!(
            "{\"id\":\"q1\",\"profile\":\"edited\",",
            "\"text\":\"observatory telescope detector spectrum calibration measurement\",",
            "\"positive_ids\":[\"astronomy\"]}\n"
        ),
    )
    .expect("queries");

    let output = Command::new(env!("CARGO_BIN_EXE_fo-exhaustive-bench"))
        .arg(&root)
        .arg(&queries)
        .arg("--maximum-documents")
        .arg("2")
        .arg("--maximum-cells-per-query")
        .arg("100000000")
        .arg("--maximum-total-cells")
        .arg("100000000")
        .arg("--require-complete-exhaustive")
        .arg("--json")
        .output()
        .expect("run benchmark");

    assert!(
        output.status.success(),
        "benchmark failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse report");
    assert_eq!(report["indexed_documents"], 2);
    assert_eq!(report["queries"], 1);
    assert_eq!(report["exhaustive_coverage"]["complete"], true);
    let exhaustive = report["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .find(|method| method["name"] == "exhaustive_levenshtein")
        .expect("exhaustive method");
    assert_eq!(exhaustive["complete_queries"], 1);
    assert_eq!(exhaustive["quality"]["micro"]["average_precision"], 1.0);

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
        "franken-overlap-exhaustive-test-{}-{nonce}",
        std::process::id()
    ))
}
