#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn renders_checksums_claims_metrics_and_examples() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("create documents");
    let source = repeated(
        "observatory telescope detector photon spectrum calibration measurement orbit",
        48,
    );
    let distractor = repeated(
        "issuer liquidity covenant maturity filing capital market portfolio valuation",
        48,
    );
    fs::write(documents.join("source.txt"), &source).expect("source");
    fs::write(documents.join("distractor.txt"), &distractor).expect("distractor");
    let mut manifest = CorpusManifest::new("report-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "source",
        "documents/source.txt",
        "Astronomy Source",
        &source,
    ));
    manifest.upsert_document(document(
        "distractor",
        "documents/distractor.txt",
        "Finance Distractor",
        &distractor,
    ));
    manifest.save(&corpus).expect("manifest");
    let manifest_bytes = fs::read(corpus.join("manifest.json")).expect("manifest bytes");

    let queries = root.join("queries.jsonl");
    let query = serde_json::json!({
        "id":"source:edited",
        "profile":"insertion_deletion",
        "text":"observatory telescope detector lantern photon spectrum calibration measurement orbit",
        "positive_ids":["source"],
        "source_id":"source",
        "source_title":"Astronomy Source",
        "relation_key":"astronomy",
        "metadata":{"passage_start_word":"0","passage_words":"8"}
    });
    fs::write(
        &queries,
        format!("{}\n", serde_json::to_string(&query).expect("query")),
    )
    .expect("queries");
    let query_bytes = fs::read(&queries).expect("query bytes");

    let proof = root.join("proof.json");
    fs::write(
        &proof,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version":1,
            "corpus_id":"report-fixture",
            "corpus_provider":"ProjectGutenberg",
            "corpus_manifest_sha256":sha256_hex(&manifest_bytes),
            "query_file_sha256":sha256_hex(&query_bytes),
            "available_documents":2,
            "required_positive_documents":1,
            "queries":1,
            "profiles":["insertion_deletion"],
            "requested_corpus_sizes":[2],
            "skipped_corpus_sizes":[],
            "evaluated_corpus_sizes":[2],
            "warmup_runs":1,
            "measurement_repetitions":3,
            "seed":7,
            "scales":[{
                "corpus_size":2,
                "required_positive_documents":1,
                "candidate_ids_sha256":"abc",
                "build":{
                    "build_ms":10.0,
                    "serialization_ms":2.0,
                    "cold_load_ms":1.0,
                    "index_bytes":4096,
                    "source_bytes":8192,
                    "overlap_fingerprints":100,
                    "overlap_postings":300,
                    "lexical_terms":40,
                    "lexical_postings":80,
                    "peak_rss_kib":1024
                },
                "methods":[
                    method("normalized_exact_substring",0.5,0.5,0.0,0.5,0.01,None),
                    method("fielded_bm25_phrase_proximity",0.5,0.5,0.0,0.5,0.2,None),
                    method("franken_overlap",1.0,1.0,1.0,1.0,0.5,Some(0.8)),
                    method("franken_hybrid",1.0,1.0,1.0,1.0,0.7,Some(0.8))
                ],
                "exhaustive":{
                    "complete":true,
                    "complete_queries":1,
                    "partial_queries":0,
                    "evaluated_pairs":2,
                    "skipped_pairs":0,
                    "cells":10000,
                    "maximum_cells_per_query":100000,
                    "maximum_cells_per_scale":100000
                },
                "break_even":[{
                    "baseline_method":"normalized_exact_substring",
                    "indexed_method":"franken_hybrid",
                    "baseline_p95_ms":0.01,
                    "indexed_p95_ms":0.7,
                    "index_build_serialization_load_ms":13.0,
                    "saved_ms_per_query_at_p95":null,
                    "break_even_queries":null
                }]
            }]
        }))
        .expect("proof"),
    )
    .expect("proof");

    let scores = root.join("scores.jsonl");
    let source_row = serde_json::json!({
        "corpus_size":2,
        "query_id":"source:edited",
        "profile":"insertion_deletion",
        "query_text":"observatory telescope detector lantern photon spectrum calibration measurement orbit",
        "source_id":"source",
        "source_title":"Astronomy Source",
        "positive_ids":["source"],
        "candidate_id":"source",
        "candidate_title":"Astronomy Source",
        "label":true,
        "scores":{
            "normalized_exact_substring":0.0,
            "fielded_bm25_phrase_proximity":0.4,
            "franken_overlap":0.9,
            "franken_hybrid":0.95
        },
        "expected_source_span":{"start":0,"end":80},
        "predicted_spans":{
            "franken_overlap":[{"start":0,"end":80}],
            "franken_hybrid":[{"start":0,"end":80}]
        },
        "exhaustive_alignment":null
    });
    let distractor_row = serde_json::json!({
        "corpus_size":2,
        "query_id":"source:edited",
        "profile":"insertion_deletion",
        "query_text":"fixture",
        "source_id":"source",
        "source_title":"Astronomy Source",
        "positive_ids":["source"],
        "candidate_id":"distractor",
        "candidate_title":"Finance Distractor",
        "label":false,
        "scores":{
            "normalized_exact_substring":0.0,
            "fielded_bm25_phrase_proximity":0.6,
            "franken_overlap":0.1,
            "franken_hybrid":0.2
        },
        "expected_source_span":null,
        "predicted_spans":{},
        "exhaustive_alignment":null
    });
    fs::write(
        &scores,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&source_row).expect("source row"),
            serde_json::to_string(&distractor_row).expect("distractor row")
        ),
    )
    .expect("scores");

    let claims = root.join("claims.json");
    fs::write(
        &claims,
        serde_json::to_vec_pretty(&serde_json::json!({
            "all_supported":true,
            "nominal_confidence_level":0.95,
            "familywise_confidence_level":0.95,
            "comparisons":[{
                "id":"hybrid-vs-bm25",
                "verdict":"supported",
                "baseline_method":"fielded_bm25_phrase_proximity",
                "challenger_method":"franken_hybrid",
                "eligible_queries":1,
                "excluded_incomplete_queries":0,
                "baseline":{"micro_auprc":0.5,"macro_auprc":0.5,"mean_reciprocal_rank":0.5,"recall_at_1":0.0},
                "challenger":{"micro_auprc":1.0,"macro_auprc":1.0,"mean_reciprocal_rank":1.0,"recall_at_1":1.0},
                "delta":{"micro_auprc":0.5,"macro_auprc":0.5,"mean_reciprocal_rank":0.5,"recall_at_1":1.0},
                "bootstrap":{
                    "micro_auprc":{"lower":0.5,"median":0.5,"upper":0.5},
                    "macro_auprc":{"lower":0.5,"median":0.5,"upper":0.5},
                    "recall_at_1":{"lower":1.0,"median":1.0,"upper":1.0}
                },
                "worst_profile":"insertion_deletion",
                "worst_profile_macro_delta":0.5,
                "baseline_p95_ms":0.2,
                "challenger_p95_ms":0.7,
                "p95_ratio":3.5,
                "failures":[],
                "uncertainties":[]
            }]
        }))
        .expect("claims"),
    )
    .expect("claims");

    let bundle = root.join("bundle");
    let output = Command::new(env!("CARGO_BIN_EXE_fo-proof-report"))
        .arg(&corpus)
        .arg(&queries)
        .arg(&proof)
        .arg(&scores)
        .arg("--claim-report")
        .arg(&claims)
        .arg("--output")
        .arg(&bundle)
        .arg("--json")
        .output()
        .expect("run renderer");
    assert!(
        output.status.success(),
        "renderer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("renderer report");
    assert_eq!(report["claim_status"], "supported");
    for file in [
        "RESULTS.md",
        "RESULTS.html",
        "environment.json",
        "examples.json",
        "artifacts.json",
    ] {
        assert!(bundle.join(file).is_file(), "missing {file}");
    }
    let markdown = fs::read_to_string(bundle.join("RESULTS.md")).expect("markdown");
    assert!(markdown.contains("Predeclared claim verdicts"));
    assert!(markdown.contains("supported"));
    assert!(
        markdown.contains("largest_hybrid_rank_gain") || markdown.contains("deterministic_first")
    );
    let html = fs::read_to_string(bundle.join("RESULTS.html")).expect("html");
    assert!(html.contains("badge-supported"));
    let artifacts: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("artifacts.json")).expect("artifacts"))
            .expect("artifact json");
    assert_eq!(artifacts["claim_status"], "supported");
    assert_eq!(artifacts["artifacts"].as_array().map(Vec::len), Some(4));

    let second = Command::new(env!("CARGO_BIN_EXE_fo-proof-report"))
        .arg(&corpus)
        .arg(&queries)
        .arg(&proof)
        .arg(&scores)
        .arg("--output")
        .arg(&bundle)
        .output()
        .expect("rerun renderer");
    assert!(
        !second.status.success(),
        "renderer overwrote immutable bundle"
    );

    fs::remove_dir_all(root).ok();
}

fn method(
    name: &str,
    micro: f64,
    macro_auprc: f64,
    recall_at_1: f64,
    mrr: f64,
    p95: f64,
    span_iou: Option<f64>,
) -> serde_json::Value {
    serde_json::json!({
        "name":name,
        "complete_quality_queries":1,
        "evaluated_pairs":2,
        "nonzero_scores":2,
        "quality":{
            "micro":{
                "average_precision":micro,
                "best_f1":micro,
                "best_threshold":0.5,
                "brier_score":0.1,
                "expected_calibration_error":0.1
            },
            "macro_average_precision":macro_auprc,
            "mean_reciprocal_rank":mrr,
            "recall_at_k":[{"k":1,"value":recall_at_1},{"k":5,"value":1.0},{"k":10,"value":1.0}],
            "ndcg_at_k":[{"k":1,"value":recall_at_1},{"k":5,"value":1.0},{"k":10,"value":1.0}]
        },
        "profiles":[],
        "timing":{
            "first_execution_samples":1,
            "repeat_samples":3,
            "first_p50_ms":p95,
            "first_p95_ms":p95,
            "repeat_p50_ms":p95,
            "repeat_p95_ms":p95,
            "repeat_p99_ms":p95,
            "measured_operations":4,
            "measured_elapsed_ms":p95*4.0,
            "operations_per_second":1000.0/p95,
            "one_shot_total_ms":p95
        },
        "span":span_iou.map(|iou| serde_json::json!({
            "eligible_queries":1,
            "predicted_queries":1,
            "mean_iou":iou,
            "median_iou":iou,
            "mean_expected_coverage":1.0,
            "mean_predicted_coverage":1.0,
            "mean_start_absolute_error":0.0,
            "mean_end_absolute_error":0.0
        }))
    })
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
        "franken-overlap-proof-report-test-{}-{nonce}",
        std::process::id()
    ))
}
