#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn build_query_and_filter_hybrid_index() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let index = root.join("search.fohybrid");
    fs::create_dir_all(&corpus).expect("create corpus");
    fs::write(
        corpus.join("observatory.txt"),
        "Copper Shutter Observatory\nBefore dawn the observatory opened its copper shutters. The team checked every detector twice and published the raw measurements.",
    )
    .expect("source");
    fs::write(
        corpus.join("kitchen.txt"),
        "Winter Kitchen\nThe cooks prepared winter vegetables beside a railway timetable and a brass lantern.",
    )
    .expect("noise");

    let built = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("build")
        .arg(&corpus)
        .arg("--output")
        .arg(&index)
        .arg("--json")
        .output()
        .expect("build");
    assert!(
        built.status.success(),
        "build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );

    let lexical = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("query")
        .arg(&index)
        .arg("title:observatory detector")
        .arg("--json")
        .output()
        .expect("lexical query");
    assert!(
        lexical.status.success(),
        "query failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&lexical.stdout),
        String::from_utf8_lossy(&lexical.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&lexical.stdout).expect("parse lexical report");
    assert_eq!(report["selected_mode"], "lexical");
    assert_eq!(report["results"][0]["external_id"], "observatory.txt");

    let hybrid = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("query")
        .arg(&index)
        .arg("the observatory opened copper shutters and the team checked every detector twice before publishing raw measurements")
        .arg("--mode")
        .arg("hybrid")
        .arg("--minimum-matched-tokens")
        .arg("8")
        .arg("--overlap-candidate-floor")
        .arg("0.05")
        .arg("--json")
        .output()
        .expect("hybrid query");
    assert!(
        hybrid.status.success(),
        "hybrid failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&hybrid.stdout),
        String::from_utf8_lossy(&hybrid.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&hybrid.stdout).expect("parse hybrid report");
    assert_eq!(report["results"][0]["external_id"], "observatory.txt");
    assert_eq!(report["results"][0]["explanation"]["agreement"], true);

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-search-cli-{}-{nonce}",
        std::process::id()
    ))
}
