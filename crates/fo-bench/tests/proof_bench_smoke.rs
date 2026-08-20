#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn benchmarks_identical_realistic_scenarios_across_all_methods() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("create documents");

    let edition_a = repeated(
        "victor creature laboratory lightning mountain village letter science responsibility",
        48,
    );
    let edition_b = repeated(
        "victor creature laboratory electricity mountain village letter science responsibility",
        48,
    );
    let detective = repeated(
        "detective railway london violin client mystery evidence footprint telegram",
        48,
    );
    fs::write(documents.join("edition-a.txt"), &edition_a).expect("edition a");
    fs::write(documents.join("edition-b.txt"), &edition_b).expect("edition b");
    fs::write(documents.join("detective.txt"), &detective).expect("detective");

    let mut manifest = CorpusManifest::new("proof-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "edition-a",
        "documents/edition-a.txt",
        "Frankenstein A",
        &edition_a,
    ));
    manifest.upsert_document(document(
        "edition-b",
        "documents/edition-b.txt",
        "Frankenstein B",
        &edition_b,
    ));
    manifest.upsert_document(document(
        "detective",
        "documents/detective.txt",
        "Detective",
        &detective,
    ));
    manifest.save(&corpus).expect("manifest");

    let query_path = root.join("queries.jsonl");
    let exact = serde_json::json!({
        "id": "edition-a:exact",
        "profile": "exact",
        "text": "victor creature laboratory lightning mountain village letter science responsibility victor creature laboratory lightning mountain village letter science responsibility victor creature laboratory lightning mountain village letter",
        "positive_ids": ["edition-a"],
        "source_id": "edition-a",
        "source_title": "Frankenstein A",
        "relation_key": "frankenstein",
        "metadata": {"passage_start_word": "0", "passage_words": "24"}
    });
    let natural = serde_json::json!({
        "id": "edition-a:natural_relation",
        "profile": "natural_relation",
        "text": "victor creature laboratory lightning mountain village letter science responsibility victor creature laboratory lightning mountain village letter science responsibility victor creature laboratory lightning mountain village letter",
        "positive_ids": ["edition-a", "edition-b"],
        "source_id": "edition-a",
        "source_title": "Frankenstein A",
        "relation_key": "frankenstein",
        "metadata": {"passage_start_word": "0", "passage_words": "24"}
    });
    fs::write(
        &query_path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&exact).expect("exact json"),
            serde_json::to_string(&natural).expect("natural json")
        ),
    )
    .expect("queries");

    let report_path = root.join("report.json");
    let scores_path = root.join("scores.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_fo-proof-bench"))
        .arg(&corpus)
        .arg(&query_path)
        .arg("--output")
        .arg(&report_path)
        .arg("--scores-output")
        .arg(&scores_path)
        .arg("--corpus-size")
        .arg("3")
        .arg("--maximum-documents")
        .arg("3")
        .arg("--warmup-runs")
        .arg("0")
        .arg("--measurement-repetitions")
        .arg("1")
        .arg("--maximum-exhaustive-cells-per-query")
        .arg("100000000")
        .arg("--maximum-exhaustive-cells-per-scale")
        .arg("200000000")
        .arg("--json")
        .output()
        .expect("run proof benchmark");

    assert!(
        output.status.success(),
        "proof benchmark failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse report");
    assert_eq!(report["queries"], 2);
    assert_eq!(report["evaluated_corpus_sizes"][0], 3);
    assert_eq!(report["scales"][0]["exhaustive"]["complete"], true);
    let methods = report["scales"][0]["methods"].as_array().expect("methods");
    for expected in [
        "normalized_exact_substring",
        "character_qgram_jaccard",
        "character_qgram_simhash",
        "fielded_bm25_phrase_proximity",
        "exhaustive_levenshtein",
        "franken_overlap",
        "franken_hybrid",
    ] {
        assert!(methods.iter().any(|method| method["name"] == expected));
    }
    assert!(report_path.is_file());
    assert_eq!(
        fs::read_to_string(scores_path)
            .expect("scores")
            .lines()
            .count(),
        6
    );

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
        "franken-overlap-proof-bench-test-{}-{nonce}",
        std::process::id()
    ))
}
