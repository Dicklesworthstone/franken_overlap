#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn query_intent_and_probability_contract_are_exposed() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let index = root.join("corpus.foidx");
    let specimen = root.join("specimen.txt");
    fs::create_dir_all(&corpus).expect("create corpus");
    fs::write(
        corpus.join("source.txt"),
        concat!(
            "Before dawn the observatory opened its copper shutters. ",
            "The team checked every instrument twice and released the raw observations."
        ),
    )
    .expect("write source");
    fs::write(
        &specimen,
        concat!(
            "the observatory opened its copper shutters the team checked every ",
            "instrument twice and released the raw observations"
        ),
    )
    .expect("write specimen");

    let indexed = Command::new(env!("CARGO_BIN_EXE_fo"))
        .arg("index")
        .arg(&corpus)
        .arg("--output")
        .arg(&index)
        .arg("--json")
        .output()
        .expect("run index");
    assert!(
        indexed.status.success(),
        "index failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&indexed.stdout),
        String::from_utf8_lossy(&indexed.stderr)
    );

    let queried = Command::new(env!("CARGO_BIN_EXE_fo"))
        .arg("query")
        .arg(&index)
        .arg(&specimen)
        .arg("--intent")
        .arg("any-passage")
        .arg("--minimum-similarity")
        .arg("0.10")
        .arg("--minimum-matched-tokens")
        .arg("8")
        .arg("--json")
        .output()
        .expect("run query");
    assert!(
        queried.status.success(),
        "query failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&queried.stdout),
        String::from_utf8_lossy(&queried.stderr)
    );
    let results: serde_json::Value =
        serde_json::from_slice(&queried.stdout).expect("parse query JSON");
    assert!(results.as_array().is_some_and(|items| !items.is_empty()));
    assert_eq!(results[0]["intent"].as_str(), Some("any_passage"));

    let invalid = Command::new(env!("CARGO_BIN_EXE_fo"))
        .arg("query")
        .arg(&index)
        .arg(&specimen)
        .arg("--minimum-probability")
        .arg("0.5")
        .output()
        .expect("run invalid query");
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr)
            .contains("--minimum-probability requires --calibration-model")
    );

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-query-cli-{}-{nonce}",
        std::process::id()
    ))
}
