#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

#[test]
fn adjudicates_ambiguous_natural_relation_into_valid_gold() {
    let root = temporary_root();
    let corpus = root.join("corpus");
    let documents = corpus.join("documents");
    fs::create_dir_all(&documents).expect("create documents");

    let edition_a = repeated(
        "victor creature laboratory lightning mountain village letter science responsibility",
        48,
    );
    let edition_b = repeated(
        "victor creature laboratory electricity mountain village letter science responsibility",
        48,
    );
    let distractor = repeated(
        "detective railway london violin client mystery evidence footprint telegram",
        48,
    );
    fs::write(documents.join("edition-a.txt"), &edition_a).expect("edition a");
    fs::write(documents.join("edition-b.txt"), &edition_b).expect("edition b");
    fs::write(documents.join("distractor.txt"), &distractor).expect("distractor");

    let mut manifest =
        CorpusManifest::new("adjudication-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(document(
        "edition-a",
        "documents/edition-a.txt",
        "Frankenstein A",
        &edition_a,
    ));
    manifest.upsert_document(document(
        "edition-b",
        "documents/edition-b.txt",
        "Frankenstein B",
        &edition_b,
    ));
    manifest.upsert_document(document(
        "distractor",
        "documents/distractor.txt",
        "Detective",
        &distractor,
    ));
    manifest.save(&corpus).expect("manifest");

    let queries = root.join("queries.jsonl");
    let query = serde_json::json!({
        "id":"frankenstein:natural",
        "profile":"natural_relation",
        "text":"victor creature laboratory lightning mountain village letter science responsibility victor creature laboratory lightning mountain village letter science responsibility",
        "positive_ids":["edition-a"],
        "source_id":"edition-a",
        "source_title":"Frankenstein A",
        "relation_key":"frankenstein",
        "metadata":{"passage_start_word":"0","passage_words":"18"}
    });
    fs::write(
        &queries,
        format!("{}\n", serde_json::to_string(&query).expect("query json")),
    )
    .expect("queries");

    let scores = root.join("scores.jsonl");
    let rows = [
        score_row(
            "edition-a",
            "Frankenstein A",
            true,
            0.71,
            0.83,
            0.81,
            Some((0, 180)),
        ),
        score_row(
            "edition-b",
            "Frankenstein B",
            false,
            0.78,
            0.76,
            0.84,
            Some((0, 180)),
        ),
        score_row("distractor", "Detective", false, 0.12, 0.05, 0.08, None),
    ];
    fs::write(
        &scores,
        rows.iter()
            .map(|row| serde_json::to_string(row).expect("score json"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .expect("scores");

    let queue = root.join("review.jsonl");
    let queue_output = Command::new(env!("CARGO_BIN_EXE_fo-adjudicate"))
        .arg("queue")
        .arg(&corpus)
        .arg(&queries)
        .arg(&scores)
        .arg("--output")
        .arg(&queue)
        .arg("--corpus-size")
        .arg("3")
        .arg("--json")
        .output()
        .expect("run queue");
    assert!(
        queue_output.status.success(),
        "queue failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&queue_output.stdout),
        String::from_utf8_lossy(&queue_output.stderr)
    );
    let queue_report: serde_json::Value =
        serde_json::from_slice(&queue_output.stdout).expect("queue report");
    assert_eq!(queue_report["queued_queries"], 1);
    let task: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&queue)
            .expect("queue file")
            .lines()
            .next()
            .expect("task"),
    )
    .expect("task json");
    assert!(
        task["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == "natural_relation"))
    );
    assert!(
        task["candidates"]
            .as_array()
            .is_some_and(|candidates| candidates.len() >= 2)
    );

    let decisions = root.join("decisions.jsonl");
    let decision = serde_json::json!({
        "query_id":"frankenstein:natural",
        "status":"replace",
        "positive_ids":["edition-a","edition-b"],
        "graded_relevance":{"edition-a":3,"edition-b":2},
        "acceptable_spans":{
            "edition-a":[{"start":0,"end":180}],
            "edition-b":[{"start":0,"end":180}]
        },
        "reviewer":"fixture-reviewer",
        "notes":"both editions contain the same adjudicated passage family",
        "reviewed_at_unix":1
    });
    fs::write(
        &decisions,
        format!(
            "{}\n",
            serde_json::to_string(&decision).expect("decision json")
        ),
    )
    .expect("decisions");

    let gold = root.join("gold.jsonl");
    let apply_output = Command::new(env!("CARGO_BIN_EXE_fo-adjudicate"))
        .arg("apply")
        .arg(&queries)
        .arg(&decisions)
        .arg("--output")
        .arg(&gold)
        .arg("--json")
        .output()
        .expect("run apply");
    assert!(
        apply_output.status.success(),
        "apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply_output.stdout),
        String::from_utf8_lossy(&apply_output.stderr)
    );
    let apply_report: serde_json::Value =
        serde_json::from_slice(&apply_output.stdout).expect("apply report");
    assert_eq!(apply_report["replaced"], 1);
    let gold_query: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&gold)
            .expect("gold")
            .lines()
            .next()
            .expect("gold row"),
    )
    .expect("gold json");
    assert_eq!(gold_query["positive_ids"].as_array().map(Vec::len), Some(2));
    assert_eq!(gold_query["gold"]["graded_relevance"]["edition-b"], 2);

    let validate_output = Command::new(env!("CARGO_BIN_EXE_fo-adjudicate"))
        .arg("validate")
        .arg(&corpus)
        .arg(&gold)
        .arg("--json")
        .output()
        .expect("run validate");
    assert!(
        validate_output.status.success(),
        "validation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&validate_output.stdout),
        String::from_utf8_lossy(&validate_output.stderr)
    );
    let validation: serde_json::Value =
        serde_json::from_slice(&validate_output.stdout).expect("validation report");
    assert_eq!(validation["queries"], 1);
    assert_eq!(validation["multi_positive_queries"], 1);
    assert_eq!(validation["acceptable_spans"], 2);

    fs::remove_dir_all(root).ok();
}

fn score_row(
    candidate_id: &str,
    candidate_title: &str,
    label: bool,
    lexical: f64,
    overlap: f64,
    hybrid: f64,
    span: Option<(usize, usize)>,
) -> serde_json::Value {
    let mut predicted = serde_json::Map::new();
    if let Some((start, end)) = span {
        predicted.insert(
            "franken_hybrid".to_owned(),
            serde_json::json!([{"start":start,"end":end}]),
        );
    }
    serde_json::json!({
        "corpus_size":3,
        "query_id":"frankenstein:natural",
        "profile":"natural_relation",
        "query_text":"fixture",
        "source_id":"edition-a",
        "source_title":"Frankenstein A",
        "positive_ids":["edition-a"],
        "candidate_id":candidate_id,
        "candidate_title":candidate_title,
        "label":label,
        "scores":{
            "fielded_bm25_phrase_proximity":lexical,
            "franken_overlap":overlap,
            "franken_hybrid":hybrid
        },
        "expected_source_span":null,
        "predicted_spans":predicted,
        "exhaustive_alignment":null
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
        "franken-overlap-adjudication-test-{}-{nonce}",
        std::process::id()
    ))
}
