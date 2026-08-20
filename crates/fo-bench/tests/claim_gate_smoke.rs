#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn supports_a_uniformly_better_challenger_with_familywise_bootstrap() {
    let root = temporary_root();
    fs::create_dir_all(&root).expect("root");
    let proof = root.join("proof.json");
    let scores = root.join("scores.jsonl");
    let manifest = root.join("claims.json");

    fs::write(
        &proof,
        serde_json::to_vec_pretty(&serde_json::json!({
            "corpus_id":"claim-fixture",
            "scales":[{
                "corpus_size":3,
                "exhaustive":{"complete":true,"complete_queries":6,"partial_queries":0},
                "methods":[
                    {"name":"fielded_bm25_phrase_proximity","timing":{"repeat_p95_ms":2.0}},
                    {"name":"franken_hybrid","timing":{"repeat_p95_ms":3.0}}
                ]
            }]
        }))
        .expect("proof json"),
    )
    .expect("proof");

    let mut rows = Vec::new();
    for query in 0..6 {
        let profile = if query < 3 { "edited" } else { "ocr_noise" };
        for candidate in 0..3 {
            let positive = candidate == 0;
            let baseline = match candidate {
                0 => 0.40,
                1 => 0.80,
                _ => 0.10,
            };
            let challenger = match candidate {
                0 => 0.90,
                1 => 0.20,
                _ => 0.10,
            };
            rows.push(serde_json::json!({
                "corpus_size":3,
                "query_id":format!("q{query}"),
                "profile":profile,
                "candidate_id":format!("d{candidate}"),
                "label":positive,
                "scores":{
                    "fielded_bm25_phrase_proximity":baseline,
                    "franken_hybrid":challenger
                }
            }));
        }
    }
    fs::write(
        &scores,
        rows.iter()
            .map(|row| serde_json::to_string(row).expect("score json"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .expect("scores");

    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version":1,
            "corpus_size":3,
            "bootstrap_samples":200,
            "confidence_level":0.95,
            "seed":7,
            "minimum_queries":6,
            "minimum_profile_queries":3,
            "comparisons":[{
                "id":"hybrid-vs-bm25",
                "baseline_method":"fielded_bm25_phrase_proximity",
                "challenger_method":"franken_hybrid",
                "minimum_challenger_micro_auprc":0.95,
                "minimum_challenger_macro_auprc":0.95,
                "minimum_challenger_recall_at_1":0.95,
                "minimum_micro_auprc_delta":0.40,
                "minimum_macro_auprc_delta":0.40,
                "minimum_recall_at_1_delta":0.90,
                "minimum_mrr_delta":0.40,
                "minimum_micro_delta_lower_bound":0.40,
                "minimum_macro_delta_lower_bound":0.40,
                "minimum_recall_at_1_delta_lower_bound":0.90,
                "maximum_worst_profile_macro_regression":0.0,
                "maximum_challenger_p95_ms":4.0,
                "maximum_p95_ratio":2.0,
                "require_complete_baseline":true
            }]
        }))
        .expect("manifest json"),
    )
    .expect("manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_fo-claim-gate"))
        .arg("evaluate")
        .arg(&proof)
        .arg(&scores)
        .arg(&manifest)
        .arg("--require-supported")
        .arg("--json")
        .output()
        .expect("run claim gate");
    assert!(
        output.status.success(),
        "claim gate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report");
    assert_eq!(report["all_supported"], true);
    assert_eq!(report["comparisons"][0]["verdict"], "supported");
    assert_eq!(report["comparisons"][0]["eligible_queries"], 6);
    assert!(
        report["comparisons"][0]["delta"]["macro_auprc"]
            .as_f64()
            .is_some_and(|delta| delta >= 0.49)
    );
    assert!(
        report["comparisons"][0]["bootstrap"]["macro_auprc"]["lower"]
            .as_f64()
            .is_some_and(|lower| lower >= 0.49)
    );

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-claim-gate-test-{}-{nonce}",
        std::process::id()
    ))
}
