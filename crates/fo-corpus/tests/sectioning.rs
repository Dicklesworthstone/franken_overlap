#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fo_corpus::{
    section_corpus, sha256_hex, CorpusDocument, CorpusManifest, CorpusProvider,
    SectionCorpusOptions, SectionStrategy,
};

#[test]
fn derives_searchable_gutenberg_chapters_with_parent_metadata() {
    let root = temporary_root();
    let input = root.join("input");
    let output = root.join("output");
    fs::create_dir_all(input.join("documents")).expect("documents");
    let chapter_one = repeated("The observatory opened copper shutters before dawn.", 120);
    let chapter_two = repeated("The kitchen prepared winter vegetables beside the railway.", 120);
    let body = format!(
        "PREFACE\n{}\n\nCHAPTER I. THE OBSERVATORY\n{}\n\nCHAPTER II. THE KITCHEN\n{}",
        repeated("This front matter introduces the book.", 80),
        chapter_one,
        chapter_two,
    );
    fs::write(input.join("documents/book.txt"), &body).expect("book");
    let mut manifest = CorpusManifest::new("book-fixture", CorpusProvider::ProjectGutenberg);
    manifest.upsert_document(CorpusDocument {
        id: "book-1".to_owned(),
        relative_path: "documents/book.txt".to_owned(),
        source_url: "https://example.invalid/book".to_owned(),
        title: "Fixture Book".to_owned(),
        author_or_issuer: "Fixture Author".to_owned(),
        language: Some("en".to_owned()),
        published_or_filed: None,
        sha256: sha256_hex(body.as_bytes()),
        bytes: body.len() as u64,
        characters: body.chars().count(),
        downloaded_at_unix: 0,
        metadata: BTreeMap::from([("subjects".to_owned(), "science;food".to_owned())]),
    });
    manifest.save(&input).expect("manifest");

    let report = section_corpus(
        &input,
        SectionCorpusOptions {
            output_dir: output.clone(),
            strategy: SectionStrategy::Gutenberg,
            minimum_characters: 500,
            target_characters: 4_000,
            maximum_characters: 8_000,
            overlap_characters: 200,
            maximum_sections_per_document: 32,
            replace_output: false,
        },
    )
    .expect("section corpus");

    assert!(report.section_documents >= 3, "{report:#?}");
    assert!(report.heading_sections >= 3);
    let derived = CorpusManifest::load(&output).expect("derived manifest");
    assert!(derived.documents.iter().all(|document| {
        document.metadata.get("parent_id").map(String::as_str) == Some("book-1")
    }));
    assert!(derived.documents.iter().any(|document| {
        document
            .metadata
            .get("section_title")
            .is_some_and(|title| title.contains("OBSERVATORY"))
    }));

    fs::remove_dir_all(root).ok();
}

fn repeated(sentence: &str, count: usize) -> String {
    std::iter::repeat_n(sentence, count)
        .collect::<Vec<_>>()
        .join(" ")
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "franken-overlap-section-test-{}-{nonce}",
        std::process::id()
    ))
}
