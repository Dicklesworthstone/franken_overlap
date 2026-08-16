#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn builds_a_filing_history_alert_and_lineage_edge() {
    let root = temporary_root();
    let corpus = root.join("sec-items");
    fs::create_dir_all(corpus.join("documents")).expect("documents");

    let common = "forward looking statements are subject to risks and uncertainties ";
    let old = format!(
        "{common}{}",
        "legacy overseas distribution exposure and currency volatility remained the principal risk ".repeat(12)
    );
    let prior = format!(
        "{common}{}",
        "copper liquidity covenant maturity refinancing concentration created a distinctive issuer risk disclosure ".repeat(12)
    );
    let current = format!(
        "{common}{}{}",
        "copper liquidity covenant maturity refinancing concentration created a distinctive issuer risk disclosure ".repeat(10),
        "a new supplier interruption paragraph was added for the current reporting period ".repeat(3)
    );

    let mut manifest = CorpusManifest::new("sec-lineage-fixture", CorpusProvider::SecEdgar10K);
    add_document(
        &corpus,
        &mut manifest,
        "CIK0000000001-2023-item1a",
        "2023.txt",
        "2023-02-01",
        &old,
    );
    add_document(
        &corpus,
        &mut manifest,
        "CIK0000000001-2024-item1a",
        "2024.txt",
        "2024-02-01",
        &prior,
    );
    add_document(
        &corpus,
        &mut manifest,
        "CIK0000000001-2025-item1a",
        "2025.txt",
        "2025-02-01",
        &current,
    );
    manifest.save(&corpus).expect("manifest");

    let output = root.join("analysis");
    let command = Command::new(env!("CARGO_BIN_EXE_fo-sec-lineage"))
        .arg(&corpus)
        .arg("--output")
        .arg(&output)
        .arg("--threads")
        .arg("1")
        .arg("--minimum-section-characters")
        .arg("100")
        .arg("--minimum-edge-score")
        .arg("0.10")
        .arg("--minimum-edge-query-coverage")
        .arg("0.05")
        .arg("--minimum-edge-matched-tokens")
        .arg("8")
        .output()
        .expect("run SEC lineage");
    assert!(
        command.status.success(),
        "SEC lineage failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&command.stdout),
        String::from_utf8_lossy(&command.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("report.json")).expect("report"),
    )
    .expect("parse report");
    assert_eq!(report["eligible_sections"], 3);
    assert_eq!(report["analyzed_targets"], 1);
    assert_eq!(
        report["targets"][0]["target_id"],
        "CIK0000000001-2025-item1a"
    );
    assert_eq!(
        report["targets"][0]["best_previous"]["source_id"],
        "CIK0000000001-2024-item1a"
    );
    assert!(report["targets"][0]["alerts"].as_array().is_some_and(|alerts| !alerts.is_empty()));
    assert!(report["lineage"]["edges"].as_u64().is_some_and(|edges| edges >= 1));

    let lineage: serde_json::Value = serde_json::from_slice(
        &fs::read(output.join("lineage.json")).expect("lineage"),
    )
    .expect("parse lineage");
    assert_eq!(lineage["nodes"].as_object().map(BTreeMap::len), None);
    assert_eq!(lineage["nodes"].as_object().map(|nodes| nodes.len()), Some(3));
    assert!(lineage["edges"].as_object().is_some_and(|edges| !edges.is_empty()));
    assert!(output.join("SUMMARY.md").is_file());
    assert!(output.join("artifacts.json").is_file());
    let result_file = report["targets"][0]["results_file"]
        .as_str()
        .expect("result file");
    assert!(output.join(result_file).is_file());

    fs::remove_dir_all(root).ok();
}

fn add_document(
    corpus: &std::path::Path,
    manifest: &mut CorpusManifest,
    id: &str,
    filename: &str,
    filing_date: &str,
    body: &str,
) {
    let relative_path = format!("documents/{filename}");
    fs::write(corpus.join(&relative_path), body).expect("write document");
    manifest.upsert_document(CorpusDocument {
        id: id.to_owned(),
        relative_path,
        source_url: format!("https://example.invalid/{id}"),
        title: format!("Fixture issuer Item 1A filed {filing_date}"),
        author_or_issuer: "Fixture Issuer".to_owned(),
        language: Some("en".to_owned()),
        published_or_filed: Some(filing_date.to_owned()),
        sha256: sha256_hex(body.as_bytes()),
        bytes: body.len() as u64,
        characters: body.chars().count(),
        downloaded_at_unix: 0,
        metadata: BTreeMap::from([
            ("cik".to_owned(), "1".to_owned()),
            ("form".to_owned(), "10-K".to_owned()),
            ("section_title".to_owned(), "Item 1A Risk Factors".to_owned()),
            ("parent_id".to_owned(), id.replace("-item1a", "")),
        ]),
    });
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-sec-lineage-{}-{nonce}",
        std::process::id()
    ))
}
