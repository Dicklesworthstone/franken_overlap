#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn builds_routes_and_queries_a_hybrid_index() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let index = root.join("corpus.fohybrid");
    fs::create_dir_all(&corpus).expect("create corpus");
    fs::write(
        corpus.join("observatory.txt"),
        "Copper Shutter Observatory\nThe observatory opened the copper shutters before dawn and calibrated every detector before publishing the raw observations.",
    )
    .expect("write observatory");
    fs::write(
        corpus.join("finance.txt"),
        "Liquidity Risk Review\nThe portfolio review measured issuer liquidity risk and covenant exposure.",
    )
    .expect("write finance");

    let build = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("build")
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

    let lexical = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("query")
        .arg(&index)
        .arg("issuer liquidity")
        .arg("--json")
        .output()
        .expect("run lexical query");
    assert!(lexical.status.success());
    let lexical_report: serde_json::Value =
        serde_json::from_slice(&lexical.stdout).expect("parse lexical report");
    assert_eq!(lexical_report["analysis"]["route"], "lexical");
    assert_eq!(lexical_report["results"][0]["external_id"], "finance.txt");

    let hybrid = Command::new(env!("CARGO_BIN_EXE_fo-search"))
        .arg("query")
        .arg(&index)
        .arg("copper shutters calibrated detector observatory")
        .arg("--mode")
        .arg("hybrid")
        .arg("--json")
        .output()
        .expect("run hybrid query");
    assert!(hybrid.status.success());
    let hybrid_report: serde_json::Value =
        serde_json::from_slice(&hybrid.stdout).expect("parse hybrid report");
    assert_eq!(hybrid_report["analysis"]["route"], "hybrid");
    assert_eq!(hybrid_report["results"][0]["external_id"], "observatory.txt");
    assert_eq!(
        hybrid_report["results"][0]["explanation"]["cross_lane_support"],
        true
    );

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-hybrid-cli-{}-{nonce}",
        std::process::id()
    ))
}
