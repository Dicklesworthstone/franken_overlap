#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_core::HybridFusionProfile;
use serde::Serialize;

#[derive(Serialize)]
struct ScoreRow {
    query_id: String,
    candidate_id: String,
    label: bool,
    scores: BTreeMap<String, f64>,
}

#[test]
fn fits_and_persists_a_valid_profile() {
    let root = temporary_root();
    fs::create_dir_all(&root).expect("root");
    let input = root.join("scores.jsonl");
    let profile_path = root.join("profile.json");
    let file = File::create(&input).expect("scores");
    let mut writer = BufWriter::new(file);
    for query in 0..12 {
        for candidate in 0..4 {
            let positive = candidate == 0;
            let lexical = if positive {
                0.72
            } else {
                0.55 - candidate as f64 * 0.08
            };
            let overlap = if positive {
                0.94
            } else {
                0.22 - candidate as f64 * 0.03
            };
            let baseline = if positive {
                0.70
            } else {
                0.45 - candidate as f64 * 0.05
            };
            serde_json::to_writer(
                &mut writer,
                &ScoreRow {
                    query_id: format!("q-{query:03}"),
                    candidate_id: format!("d-{candidate:03}"),
                    label: positive,
                    scores: BTreeMap::from([
                        ("fielded_bm25_phrase_proximity".to_owned(), lexical),
                        ("franken_overlap".to_owned(), overlap),
                        ("franken_hybrid".to_owned(), baseline),
                    ]),
                },
            )
            .expect("row");
            writer.write_all(b"\n").expect("newline");
        }
    }
    writer.flush().expect("flush");

    let output = Command::new(env!("CARGO_BIN_EXE_fo-hybrid-tune"))
        .arg("fit")
        .arg(&input)
        .arg("--output")
        .arg(&profile_path)
        .arg("--json")
        .output()
        .expect("run tuner");
    assert!(
        output.status.success(),
        "tuner failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let profile = serde_json::from_slice::<HybridFusionProfile>(
        &fs::read(&profile_path).expect("profile bytes"),
    )
    .expect("profile");
    profile.validate().expect("valid profile");
    assert!(profile.overlap_weight > profile.lexical_weight);
    assert_eq!(
        profile.train_queries + profile.validation_queries + profile.test_queries,
        12
    );

    fs::remove_dir_all(root).ok();
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-hybrid-tune-{}-{nonce}",
        std::process::id()
    ))
}
