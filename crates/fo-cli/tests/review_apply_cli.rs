#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_core::{ReviewDecisionKind, ReviewDecisionRecord, SearchIntent, SearchResult};

#[test]
fn accepted_and_rejected_decisions_update_feedback_and_lineage_idempotently() {
    let root = temporary_root();
    fs::create_dir_all(&root).expect("root");
    let results_path = root.join("results.json");
    let decisions_path = root.join("decisions.jsonl");
    let feedback_path = root.join("ranking-feedback.jsonl");
    let calibration_path = root.join("calibration-feedback.jsonl");
    let lineage_path = root.join("lineage.json");
    let ledger_path = root.join("decision-ledger.jsonl");

    let results = vec![
        result("accepted-source", 0.91, 10),
        result("rejected-source", 0.82, 30),
    ];
    fs::write(
        &results_path,
        serde_json::to_vec_pretty(&results).expect("serialize results"),
    )
    .expect("results");
    let decisions = vec![
        decision("accepted-source", ReviewDecisionKind::Accept),
        decision("rejected-source", ReviewDecisionKind::Reject),
    ];
    let mut decision_bytes = Vec::new();
    for decision in &decisions {
        serde_json::to_writer(&mut decision_bytes, decision).expect("decision");
        decision_bytes.push(b'\n');
    }
    fs::write(&decisions_path, decision_bytes).expect("decisions");

    let first = run_apply(
        &results_path,
        &decisions_path,
        &feedback_path,
        &calibration_path,
        &lineage_path,
        &ledger_path,
    );
    assert!(
        first.status.success(),
        "first application failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_report: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("first report");
    assert_eq!(first_report["ranking_feedback_records_added"], 2);
    assert_eq!(first_report["calibration_feedback_records_added"], 2);
    assert_eq!(first_report["lineage_edges_changed"], 1);

    let feedback = fs::read_to_string(&feedback_path).expect("feedback");
    assert_eq!(feedback.lines().count(), 2);
    assert!(feedback.contains("\"label\":true"));
    assert!(feedback.contains("\"label\":false"));
    let lineage: serde_json::Value =
        serde_json::from_slice(&fs::read(&lineage_path).expect("lineage")).expect("parse lineage");
    assert_eq!(
        lineage["nodes"].as_object().map(|nodes| nodes.len()),
        Some(2)
    );
    assert_eq!(
        lineage["edges"].as_object().map(|edges| edges.len()),
        Some(1)
    );

    let second = run_apply(
        &results_path,
        &decisions_path,
        &feedback_path,
        &calibration_path,
        &lineage_path,
        &ledger_path,
    );
    assert!(second.status.success(), "second application failed");
    let second_report: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second report");
    assert_eq!(second_report["ranking_feedback_records_added"], 0);
    assert_eq!(second_report["calibration_feedback_records_added"], 0);
    assert_eq!(second_report["decision_ledger_records_added"], 0);
    assert_eq!(second_report["lineage_edges_changed"], 0);

    fs::remove_dir_all(root).ok();
}

fn run_apply(
    results: &std::path::Path,
    decisions: &std::path::Path,
    feedback: &std::path::Path,
    calibration: &std::path::Path,
    lineage: &std::path::Path,
    ledger: &std::path::Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fo-review-apply"))
        .arg(results)
        .arg(decisions)
        .arg("--feedback-output")
        .arg(feedback)
        .arg("--calibration-output")
        .arg(calibration)
        .arg("--lineage")
        .arg(lineage)
        .arg("--decision-ledger")
        .arg(ledger)
        .arg("--target-title")
        .arg("Reviewed target")
        .arg("--minimum-lineage-score")
        .arg("0.1")
        .arg("--minimum-lineage-query-coverage")
        .arg("0.1")
        .arg("--minimum-lineage-matched-tokens")
        .arg("4")
        .arg("--json")
        .output()
        .expect("run review apply")
}

fn result(path: &str, score: f32, start: usize) -> SearchResult {
    SearchResult {
        document_id: 0,
        path: path.to_owned(),
        intent: SearchIntent::SourceAttribution,
        corpus_start: start,
        corpus_end: start + 12,
        query_start: 0,
        query_end: 12,
        edit_distance: 1,
        edit_similarity: 0.92,
        anchor_coverage: 0.85,
        query_coverage: 0.80,
        source_coverage: 0.40,
        anchor_score: score,
        vote_support: 0.75,
        chain_consistency: 0.90,
        matched_tokens: 12,
        distinct_anchor_count: 3,
        estimated_false_matches: 0.001,
        combined_score: score,
        matched_text: "localized source text".to_owned(),
    }
}

fn decision(candidate_id: &str, decision: ReviewDecisionKind) -> ReviewDecisionRecord {
    ReviewDecisionRecord {
        schema_version: 1,
        target_id: "target-document".to_owned(),
        candidate_id: candidate_id.to_owned(),
        decision,
        reviewer: "fixture-reviewer".to_owned(),
        notes: "fixture decision".to_owned(),
        corrected_source_id: None,
        accepted_block_indexes: Vec::new(),
        reviewed_at_unix: 1_786_854_000,
    }
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-review-apply-{}-{nonce}",
        std::process::id()
    ))
}
