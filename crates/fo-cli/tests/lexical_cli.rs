#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn builds_and_queries_a_fielded_index() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let index = root.join("corpus.folex");
    fs::create_dir_all(&corpus).expect("create corpus");
    fs::write(
        corpus.join("observatory.txt"),
        "Copper Shutter Observatory\nThe observatory opened the copper shutters before dawn and calibrated every detector.",
    )
    .expect("write observatory");
    fs::write(
        corpus.join("kitchen.txt"),
        "Copper Cookware\nThe kitchen displayed copper pans before the winter festival.",
    )
    .expect("write kitchen");

    let build = Command::new(env!("CARGO_BIN_EXE_fo-lexical"))
        .args(["build"])
        .arg(&corpus)
        .arg("--output")
        .arg(&index)
        .arg("--json")
        .output()
        .expect("run build");
    assert!(
        build.status.success(),
        "build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let query = Command::new(env!("CARGO_BIN_EXE_fo-lexical"))
        .arg("query")
        .arg(&index)
        .arg("\"copper shutters\" detector")
        .arg("--json")
        .output()
        .expect("run query");
    assert!(
        query.status.success(),
        "query failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );
    let results: serde_json::Value =
        serde_json::from_slice(&query.stdout).expect("parse query JSON");
    assert_eq!(results[0]["external_id"], "observatory.txt");
    assert!(results[0]["explanation"]["exact_phrase_matches"]
        .as_u64()
        .is_some_and(|matches| matches > 0));

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-lexical-cli-{}-{nonce}",
        std::process::id()
    ))
}
