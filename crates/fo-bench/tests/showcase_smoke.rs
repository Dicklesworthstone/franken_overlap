#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn existing_showcase_emits_controlled_and_multi_positive_queries() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("create documents");

    let edition_a = repeated(
        "victor creature laboratory lightning mountain village letter science responsibility",
        64,
    );
    let edition_b = repeated(
        "victor creature laboratory electricity mountain village letter science responsibility",
        64,
    );
    let distractor = repeated(
        "detective railway london violin client mystery evidence footprint telegram",
        64,
    );
    fs::write(documents.join("edition-a.txt"), &edition_a).expect("edition a");
    fs::write(documents.join("edition-b.txt"), &edition_b).expect("edition b");
    fs::write(documents.join("distractor.txt"), &distractor).expect("distractor");

    let mut manifest = CorpusManifest::new("showcase-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "edition-a",
        "documents/edition-a.txt",
        "Frankenstein; Or, The Modern Prometheus",
        "Mary Wollstonecraft Shelley",
        &edition_a,
    ));
    manifest.upsert_document(document(
        "edition-b",
        "documents/edition-b.txt",
        "Frankenstein; or, the Modern Prometheus",
        "Mary Wollstonecraft Shelley",
        &edition_b,
    ));
    manifest.upsert_document(document(
        "distractor",
        "documents/distractor.txt",
        "The Adventures of a Detective",
        "Arthur Conan Doyle",
        &distractor,
    ));
    manifest.save(&corpus).expect("manifest");

    let output_root = root.join("prepared");
    let output = Command::new(env!("CARGO_BIN_EXE_fo-showcase"))
        .arg("existing")
        .arg(&corpus)
        .arg("--output")
        .arg(&output_root)
        .arg("--no-sectioning")
        .arg("--source-documents")
        .arg("3")
        .arg("--queries-per-source")
        .arg("8")
        .arg("--passage-words")
        .arg("24")
        .arg("--json")
        .output()
        .expect("run showcase");

    assert!(
        output.status.success(),
        "showcase failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse report");
    assert_eq!(report["raw"]["documents"], 3);
    assert_eq!(report["searchable"]["documents"], 3);
    assert_eq!(report["scenarios"]["queries"], 24);
    assert!(report["scenarios"]["multi_positive_queries"].as_u64() >= Some(2));

    let query_text = fs::read_to_string(output_root.join("queries.jsonl")).expect("queries");
    let queries = query_text
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("query row"))
        .collect::<Vec<_>>();
    assert!(queries.iter().any(|query| {
        query["profile"] == "natural_relation"
            && query["positive_ids"]
                .as_array()
                .is_some_and(|positives| positives.len() == 2)
    }));
    for profile in [
        "exact",
        "format_drift",
        "substitution_10pct",
        "insertion_deletion",
        "ocr_noise",
        "fragmented",
        "reordered",
        "natural_relation",
    ] {
        assert!(queries.iter().any(|query| query["profile"] == profile));
    }

    fs::remove_dir_all(root).ok();
}

fn document(
    id: &str,
    relative_path: &str,
    title: &str,
    author: &str,
    body: &str,
) -> CorpusDocument {
    CorpusDocument {
        id: id.to_owned(),
        relative_path: relative_path.to_owned(),
        source_url: format!("https://example.invalid/{id}"),
        title: title.to_owned(),
        author_or_issuer: author.to_owned(),
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
        "franken-overlap-showcase-test-{}-{nonce}",
        std::process::id()
    ))
}
