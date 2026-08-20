#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn benchmarks_a_small_manifest_end_to_end() {
    let root = temporary_root();
    let documents = root.join("documents");
    fs::create_dir_all(&documents).expect("create documents");

    let astronomy = repeated_document(
        "observatory detector telescope photon spectrum calibration measurement causal model",
        80,
    );
    let finance = repeated_document(
        "issuer liquidity covenant maturity filing capital market portfolio valuation",
        80,
    );
    fs::write(documents.join("astronomy.txt"), &astronomy).expect("astronomy");
    fs::write(documents.join("finance.txt"), &finance).expect("finance");

    let mut manifest = CorpusManifest::new("fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document_record(
        "astronomy",
        "documents/astronomy.txt",
        "Astronomy",
        &astronomy,
    ));
    manifest.upsert_document(document_record(
        "finance",
        "documents/finance.txt",
        "Finance",
        &finance,
    ));
    manifest.save(&root).expect("manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_fo-real-bench"))
        .arg("--corpus-root")
        .arg(&root)
        .arg("--provider")
        .arg("existing")
        .arg("--maximum-documents")
        .arg("2")
        .arg("--source-documents")
        .arg("2")
        .arg("--queries-per-document")
        .arg("2")
        .arg("--passage-words")
        .arg("24")
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
    assert_eq!(report["queries"], 4);
    assert!(report["methods"].as_array().is_some_and(|methods| {
        methods
            .iter()
            .any(|method| method["name"] == "franken_hybrid")
    }));

    fs::remove_dir_all(root).ok();
}

fn document_record(id: &str, relative_path: &str, title: &str, body: &str) -> CorpusDocument {
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

fn repeated_document(vocabulary: &str, repetitions: usize) -> String {
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
        "franken-overlap-real-bench-test-{}-{nonce}",
        std::process::id()
    ))
}
