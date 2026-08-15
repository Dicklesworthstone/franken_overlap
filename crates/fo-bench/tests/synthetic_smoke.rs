#![forbid(unsafe_code)]

use std::process::Command;

#[test]
fn deterministic_quality_floor_holds() {
    let output = Command::new(env!("CARGO_BIN_EXE_fo-bench"))
        .args([
            "synthetic",
            "--documents",
            "4",
            "--queries-per-document",
            "4",
            "--seed",
            "7",
            "--minimum-auprc",
            "0.50",
            "--minimum-recall-at-1",
            "0.50",
            "--json",
        ])
        .output()
        .expect("run fo-bench synthetic");

    assert!(
        output.status.success(),
        "synthetic quality gate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse synthetic report");
    assert_eq!(report["documents"], 4);
    assert_eq!(report["queries"], 16);
    assert_eq!(report["methods"][0]["name"], "franken_overlap");
}
