#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_core::{GroupedEvaluationOptions, GroupedLabeledScore, grouped_evaluation_report};
use serde_json::json;

#[test]
fn creates_a_query_paired_evidence_bundle() {
    let root = temporary_directory();
    fs::create_dir_all(&root).expect("create root");
    let report_path = root.join("report.json");
    let scores_path = root.join("scores.jsonl");
    let output = root.join("evidence");

    let selected_rows = vec![
        grouped("q1", 0.90, true),
        grouped("q1", 0.10, false),
        grouped("q2", 0.85, true),
        grouped("q2", 0.15, false),
    ];
    let baseline_rows = vec![
        grouped("q1", 0.40, true),
        grouped("q1", 0.60, false),
        grouped("q2", 0.45, true),
        grouped("q2", 0.55, false),
    ];
    let evaluation = GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    };
    let selected_report =
        grouped_evaluation_report(&selected_rows, evaluation.clone()).expect("selected report");
    let baseline_report =
        grouped_evaluation_report(&baseline_rows, evaluation).expect("baseline report");
    let report = json!({
        "schema_version": 1,
        "generated_at_unix": 1,
        "corpus_id": "fixture",
        "corpus_provider": "fixture",
        "corpus_manifest_documents": 2,
        "indexed_documents": 2,
        "source_documents": 2,
        "queries": 2,
        "pairs": 4,
        "seed": 7,
        "profiles": ["edited"],
        "build": {
            "build_ms": 1.0,
            "serialization_ms": 1.0,
            "index_bytes": 128,
            "overlap_fingerprints": 8,
            "overlap_postings": 16,
            "lexical_terms": 8,
            "lexical_postings": 16
        },
        "methods": [
            {
                "name": "franken_hybrid",
                "elapsed_ms": 2.0,
                "queries_per_second": 1000.0,
                "p50_ms": 1.0,
                "p95_ms": 1.2,
                "p99_ms": 1.3,
                "false_positives_per_query_at_best_f1": 0.0,
                "quality": selected_report,
                "profiles": []
            },
            {
                "name": "baseline",
                "elapsed_ms": 4.0,
                "queries_per_second": 500.0,
                "p50_ms": 2.0,
                "p95_ms": 2.2,
                "p99_ms": 2.3,
                "false_positives_per_query_at_best_f1": 1.0,
                "quality": baseline_report,
                "profiles": []
            }
        ]
    });
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("write report");

    let rows = [
        score_row("q1", "a", "a", true, 0.90, 0.40),
        score_row("q1", "a", "b", false, 0.10, 0.60),
        score_row("q2", "b", "a", false, 0.15, 0.55),
        score_row("q2", "b", "b", true, 0.85, 0.45),
    ];
    let score_text = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("serialize row"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&scores_path, score_text).expect("write scores");

    let status = Command::new(env!("CARGO_BIN_EXE_fo-evidence"))
        .arg(&report_path)
        .arg(&scores_path)
        .arg("--output")
        .arg(&output)
        .arg("--baseline")
        .arg("baseline")
        .arg("--bootstrap-samples")
        .arg("200")
        .status()
        .expect("run evidence binary");
    assert!(status.success());
    for filename in [
        "evidence.json",
        "environment.json",
        "EXAMPLES.md",
        "manifest.json",
        "SHA256SUMS",
    ] {
        assert!(output.join(filename).is_file(), "missing {filename}");
    }
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("evidence.json")).expect("read evidence"))
            .expect("parse evidence");
    assert_eq!(
        evidence["verdict"]["claim"],
        "quality_and_wall_time_superiority_supported"
    );
    fs::remove_dir_all(root).ok();
}

fn grouped(query_id: &str, score: f64, label: bool) -> GroupedLabeledScore {
    GroupedLabeledScore {
        query_id: query_id.to_owned(),
        score,
        label,
    }
}

fn score_row(
    query_id: &str,
    source_id: &str,
    candidate_id: &str,
    label: bool,
    selected: f64,
    baseline: f64,
) -> serde_json::Value {
    json!({
        "query_id": query_id,
        "profile": "edited",
        "source_id": source_id,
        "candidate_id": candidate_id,
        "label": label,
        "scores": {
            "franken_hybrid": selected,
            "baseline": baseline
        },
        "query_text": "edited fixture query",
        "source_title": source_id,
        "candidate_title": candidate_id
    })
}

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-evidence-test-{}-{nonce}",
        std::process::id()
    ))
}
