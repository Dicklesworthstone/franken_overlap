use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    atomic_write, sha256_hex, unix_timestamp, CorpusDocument, CorpusError, CorpusManifest,
    Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionStrategy {
    Auto,
    Gutenberg,
    Sec10K,
    ParagraphWindows,
}

#[derive(Debug, Clone)]
pub struct SectionCorpusOptions {
    pub output_dir: PathBuf,
    pub strategy: SectionStrategy,
    pub minimum_characters: usize,
    pub target_characters: usize,
    pub maximum_characters: usize,
    pub overlap_characters: usize,
    pub maximum_sections_per_document: usize,
    pub replace_output: bool,
}

impl Default for SectionCorpusOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("corpora/sections"),
            strategy: SectionStrategy::Auto,
            minimum_characters: 2_000,
            target_characters: 18_000,
            maximum_characters: 36_000,
            overlap_characters: 1_000,
            maximum_sections_per_document: 512,
            replace_output: false,
        }
    }
}

impl SectionCorpusOptions {
    pub fn validate(&self) -> Result<()> {
        if self.minimum_characters == 0
            || self.target_characters == 0
            || self.maximum_characters == 0
            || self.maximum_sections_per_document == 0
        {
            return Err(CorpusError::Invalid(
                "section character and count limits must be positive".to_owned(),
            ));
        }
        if self.minimum_characters > self.target_characters
            || self.target_characters > self.maximum_characters
            || self.overlap_characters >= self.target_characters
        {
            return Err(CorpusError::Invalid(
                "section limits must satisfy minimum <= target <= maximum and overlap < target"
                    .to_owned(),
            ));
        }
        if self.maximum_sections_per_document > 100_000 {
            return Err(CorpusError::Invalid(
                "maximum_sections_per_document must not exceed 100,000".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionCorpusReport {
    pub manifest: CorpusManifest,
    pub parent_documents: usize,
    pub section_documents: usize,
    pub heading_sections: usize,
    pub window_sections: usize,
    pub skipped_parent_documents: usize,
    pub total_source_bytes: u64,
    pub total_section_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionSpan {
    start: usize,
    end: usize,
    title: String,
    origin: &'static str,
}

pub fn section_corpus(
    input_root: impl AsRef<Path>,
    options: SectionCorpusOptions,
) -> Result<SectionCorpusReport> {
    options.validate()?;
    let input_root = input_root.as_ref();
    let parent = CorpusManifest::load(input_root)?;
    if options.output_dir == input_root {
        return Err(CorpusError::Invalid(
            "section output directory must differ from the input corpus".to_owned(),
        ));
    }
    if options.output_dir.exists() {
        if !options.replace_output {
            return Err(CorpusError::Invalid(format!(
                "section output {} already exists; pass --replace-output to rebuild it",
                options.output_dir.display()
            )));
        }
        fs::remove_dir_all(&options.output_dir)
            .map_err(|error| CorpusError::io(&options.output_dir, error))?;
    }
    fs::create_dir_all(options.output_dir.join("documents"))
        .map_err(|error| CorpusError::io(options.output_dir.join("documents"), error))?;

    let effective_strategy = resolve_strategy(options.strategy, parent.provider);
    let mut manifest = CorpusManifest::new(
        format!(
            "{}-sections-{}-{}",
            parent.corpus_id,
            strategy_name(effective_strategy),
            options.target_characters
        ),
        parent.provider,
    );
    manifest.source_snapshot.insert(
        "parent_corpus_id".to_owned(),
        parent.corpus_id.clone(),
    );
    manifest.source_snapshot.insert(
        "parent_manifest_sha256".to_owned(),
        sha256_hex(
            &serde_json::to_vec(&parent).map_err(CorpusError::Json)?,
        ),
    );
    manifest.source_snapshot.insert(
        "section_strategy".to_owned(),
        strategy_name(effective_strategy).to_owned(),
    );
    manifest.source_snapshot.insert(
        "minimum_characters".to_owned(),
        options.minimum_characters.to_string(),
    );
    manifest.source_snapshot.insert(
        "target_characters".to_owned(),
        options.target_characters.to_string(),
    );
    manifest.source_snapshot.insert(
        "maximum_characters".to_owned(),
        options.maximum_characters.to_string(),
    );
    manifest.source_snapshot.insert(
        "overlap_characters".to_owned(),
        options.overlap_characters.to_string(),
    );

    let mut heading_sections = 0usize;
    let mut window_sections = 0usize;
    let mut skipped_parent_documents = 0usize;
    let mut total_source_bytes = 0u64;
    let mut total_section_bytes = 0u64;

    for parent_document in &parent.documents {
        validate_relative_path(&parent_document.relative_path)?;
        let source_path = input_root.join(&parent_document.relative_path);
        let source = match fs::read_to_string(&source_path) {
            Ok(source) => source,
            Err(error) => {
                skipped_parent_documents += 1;
                manifest.record_failure(crate::CorpusFailure {
                    id: parent_document.id.clone(),
                    source_url: Some(parent_document.source_url.clone()),
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        total_source_bytes = total_source_bytes.saturating_add(source.len() as u64);
        let mut spans = split_document(&source, effective_strategy, &options);
        spans.retain(|span| {
            source
                .get(span.start..span.end)
                .is_some_and(|value| value.chars().count() >= options.minimum_characters)
        });
        spans.truncate(options.maximum_sections_per_document);
        if spans.is_empty() {
            skipped_parent_documents += 1;
            continue;
        }

        let parent_component = sanitize_component(&parent_document.id, 96);
        for (ordinal, span) in spans.into_iter().enumerate() {
            let Some(section_text) = source.get(span.start..span.end) else {
                continue;
            };
            let section_text = section_text.trim();
            if section_text.is_empty() {
                continue;
            }
            let section_id = format!("{}#section-{:04}", parent_document.id, ordinal + 1);
            let title_component = sanitize_component(&span.title, 72);
            let relative_path = format!(
                "documents/{parent_component}/{:04}_{title_component}.txt",
                ordinal + 1
            );
            let destination = options.output_dir.join(&relative_path);
            atomic_write(&destination, section_text.as_bytes())?;
            total_section_bytes =
                total_section_bytes.saturating_add(section_text.len() as u64);
            if span.origin == "heading" {
                heading_sections += 1;
            } else {
                window_sections += 1;
            }
            let mut metadata = parent_document.metadata.clone();
            metadata.insert("parent_id".to_owned(), parent_document.id.clone());
            metadata.insert(
                "parent_title".to_owned(),
                parent_document.title.clone(),
            );
            metadata.insert("section_index".to_owned(), ordinal.to_string());
            metadata.insert("section_title".to_owned(), span.title.clone());
            metadata.insert("section_origin".to_owned(), span.origin.to_owned());
            metadata.insert("source_start_byte".to_owned(), span.start.to_string());
            metadata.insert("source_end_byte".to_owned(), span.end.to_string());
            manifest.upsert_document(CorpusDocument {
                id: section_id,
                relative_path,
                source_url: parent_document.source_url.clone(),
                title: format!("{} — {}", parent_document.title, span.title),
                author_or_issuer: parent_document.author_or_issuer.clone(),
                language: parent_document.language.clone(),
                published_or_filed: parent_document.published_or_filed.clone(),
                sha256: sha256_hex(section_text.as_bytes()),
                bytes: section_text.len() as u64,
                characters: section_text.chars().count(),
                downloaded_at_unix: unix_timestamp(),
                metadata,
            });
        }
    }

    manifest.save(&options.output_dir)?;
    Ok(SectionCorpusReport {
        parent_documents: parent.documents.len(),
        section_documents: manifest.documents.len(),
        heading_sections,
        window_sections,
        skipped_parent_documents,
        total_source_bytes,
        total_section_bytes,
        manifest,
    })
}

fn split_document(
    source: &str,
    strategy: SectionStrategy,
    options: &SectionCorpusOptions,
) -> Vec<SectionSpan> {
    let headings = match strategy {
        SectionStrategy::Gutenberg => heading_spans(source, gutenberg_heading, options),
        SectionStrategy::Sec10K => heading_spans(source, sec_heading, options),
        SectionStrategy::ParagraphWindows | SectionStrategy::Auto => Vec::new(),
    };
    if headings.is_empty() {
        return paragraph_windows(source, 0, "document", options);
    }
    let mut output = Vec::new();
    for heading in headings {
        if source
            .get(heading.start..heading.end)
            .is_some_and(|text| text.chars().count() <= options.maximum_characters)
        {
            output.push(heading);
        } else if let Some(text) = source.get(heading.start..heading.end) {
            let mut windows = paragraph_windows(text, heading.start, &heading.title, options);
            output.append(&mut windows);
        }
    }
    output
}

fn heading_spans(
    source: &str,
    detector: fn(&str) -> Option<String>,
    options: &SectionCorpusOptions,
) -> Vec<SectionSpan> {
    let mut headings = Vec::<(usize, String)>::new();
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        let content = line.trim_matches(['\r', '\n']).trim();
        if let Some(title) = detector(content) {
            headings.push((offset, title));
        }
        offset = offset.saturating_add(line.len());
    }
    if offset < source.len() {
        let tail = &source[offset..];
        if let Some(title) = detector(tail.trim()) {
            headings.push((offset, title));
        }
    }
    headings.sort_unstable_by_key(|(offset, _)| *offset);
    headings.dedup_by(|left, right| left.0 == right.0);
    if headings.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    if headings[0].0 >= options.minimum_characters {
        candidates.push(SectionSpan {
            start: 0,
            end: headings[0].0,
            title: "front matter".to_owned(),
            origin: "heading",
        });
    }
    for (index, (start, title)) in headings.iter().enumerate() {
        let end = headings
            .get(index + 1)
            .map_or(source.len(), |(offset, _)| *offset);
        if end <= *start {
            continue;
        }
        candidates.push(SectionSpan {
            start: *start,
            end,
            title: title.clone(),
            origin: "heading",
        });
    }

    let mut best_by_title = BTreeMap::<String, SectionSpan>::new();
    for span in candidates {
        let length = source
            .get(span.start..span.end)
            .map_or(0, |text| text.chars().count());
        if length < options.minimum_characters {
            continue;
        }
        let key = canonical_title(&span.title);
        match best_by_title.get(&key) {
            Some(existing)
                if source
                    .get(existing.start..existing.end)
                    .map_or(0, |text| text.chars().count())
                    >= length => {}
            _ => {
                best_by_title.insert(key, span);
            }
        }
    }
    let mut spans = best_by_title.into_values().collect::<Vec<_>>();
    spans.sort_unstable_by_key(|span| span.start);
    spans
}

fn paragraph_windows(
    source: &str,
    base_offset: usize,
    title: &str,
    options: &SectionCorpusOptions,
) -> Vec<SectionSpan> {
    if source.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut ordinal = 1usize;
    while start < source.len() && spans.len() < options.maximum_sections_per_document {
        let remaining = &source[start..];
        if remaining.chars().count() <= options.maximum_characters {
            if remaining.chars().count() >= options.minimum_characters {
                spans.push(SectionSpan {
                    start: base_offset + start,
                    end: base_offset + source.len(),
                    title: format!("{title} — segment {ordinal}"),
                    origin: "window",
                });
            }
            break;
        }
        let target_end = byte_offset_for_chars(source, start, options.target_characters);
        let maximum_end = byte_offset_for_chars(source, start, options.maximum_characters);
        let end = choose_paragraph_boundary(source, target_end, maximum_end)
            .max(target_end)
            .min(source.len());
        if end <= start {
            break;
        }
        spans.push(SectionSpan {
            start: base_offset + start,
            end: base_offset + end,
            title: format!("{title} — segment {ordinal}"),
            origin: "window",
        });
        ordinal += 1;
        if end >= source.len() {
            break;
        }
        let next = retreat_chars(source, end, options.overlap_characters);
        start = next.max(start.saturating_add(1));
    }
    spans
}

fn gutenberg_heading(line: &str) -> Option<String> {
    if line.is_empty() || line.chars().count() > 140 {
        return None;
    }
    let normalized = collapse_spaces(line);
    let uppercase = normalized.to_ascii_uppercase();
    let recognized = ["CHAPTER ", "BOOK ", "PART ", "VOLUME "]
        .iter()
        .any(|prefix| uppercase.starts_with(prefix))
        || matches!(
            uppercase.as_str(),
            "PREFACE" | "INTRODUCTION" | "PROLOGUE" | "EPILOGUE" | "CONCLUSION"
        );
    recognized.then_some(normalized)
}

fn sec_heading(line: &str) -> Option<String> {
    if line.is_empty() || line.chars().count() > 180 {
        return None;
    }
    let normalized = collapse_spaces(line);
    let uppercase = normalized.to_ascii_uppercase();
    let remainder = uppercase.strip_prefix("ITEM ")?;
    let token = remainder
        .split(|character: char| character.is_whitespace() || matches!(character, '.' | ':' | '-'))
        .next()
        .unwrap_or_default();
    if token.is_empty() || token.len() > 3 {
        return None;
    }
    let mut digits = 0usize;
    let mut letters = 0usize;
    for character in token.chars() {
        if character.is_ascii_digit() && letters == 0 {
            digits += 1;
        } else if character.is_ascii_alphabetic() && digits > 0 {
            letters += 1;
        } else {
            return None;
        }
    }
    if digits == 0 || digits > 2 || letters > 1 {
        return None;
    }
    Some(normalized)
}

fn choose_paragraph_boundary(source: &str, target: usize, maximum: usize) -> usize {
    let target = target.min(source.len());
    let maximum = maximum.min(source.len());
    if target >= maximum {
        return maximum;
    }
    let tail = &source[target..maximum];
    if let Some(relative) = tail.find("\n\n") {
        return target + relative + 2;
    }
    let prefix = &source[..target];
    prefix.rfind("\n\n").map_or(target, |offset| offset + 2)
}

fn byte_offset_for_chars(source: &str, start: usize, characters: usize) -> usize {
    source[start..]
        .char_indices()
        .nth(characters)
        .map_or(source.len(), |(relative, _)| start + relative)
}

fn retreat_chars(source: &str, end: usize, characters: usize) -> usize {
    if characters == 0 {
        return end;
    }
    source[..end]
        .char_indices()
        .rev()
        .nth(characters.saturating_sub(1))
        .map_or(0, |(offset, _)| offset)
}

fn resolve_strategy(strategy: SectionStrategy, provider: crate::CorpusProvider) -> SectionStrategy {
    if strategy != SectionStrategy::Auto {
        return strategy;
    }
    match provider {
        crate::CorpusProvider::ProjectGutenberg => SectionStrategy::Gutenberg,
        crate::CorpusProvider::SecEdgar10K => SectionStrategy::Sec10K,
    }
}

const fn strategy_name(strategy: SectionStrategy) -> &'static str {
    match strategy {
        SectionStrategy::Auto => "auto",
        SectionStrategy::Gutenberg => "gutenberg",
        SectionStrategy::Sec10K => "sec_10k",
        SectionStrategy::ParagraphWindows => "paragraph_windows",
    }
}

fn collapse_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn canonical_title(value: &str) -> String {
    collapse_spaces(value).to_ascii_lowercase()
}

fn sanitize_component(value: &str, maximum: usize) -> String {
    let mut output = String::new();
    let mut prior_separator = false;
    for character in value.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '_'
        };
        if mapped == '_' {
            if prior_separator || output.is_empty() {
                continue;
            }
            prior_separator = true;
        } else {
            prior_separator = false;
        }
        output.push(mapped);
        if output.len() >= maximum {
            break;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "section".to_owned()
    } else {
        output
    }
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CorpusError::Invalid(format!(
            "unsafe corpus relative path {value:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        gutenberg_heading, paragraph_windows, sec_heading, SectionCorpusOptions,
    };

    #[test]
    fn recognizes_book_and_filing_headings() {
        assert_eq!(
            gutenberg_heading("CHAPTER VII. THE COPPER SHUTTERS"),
            Some("CHAPTER VII. THE COPPER SHUTTERS".to_owned())
        );
        assert_eq!(
            sec_heading("ITEM 1A. RISK FACTORS"),
            Some("ITEM 1A. RISK FACTORS".to_owned())
        );
        assert!(sec_heading("itemized expense table").is_none());
    }

    #[test]
    fn paragraph_windows_cover_long_text_with_overlap() {
        let source = (0..2_000)
            .map(|index| format!("paragraph {index} with enough repeated text\n\n"))
            .collect::<String>();
        let options = SectionCorpusOptions {
            minimum_characters: 200,
            target_characters: 1_000,
            maximum_characters: 1_500,
            overlap_characters: 100,
            ..SectionCorpusOptions::default()
        };
        let spans = paragraph_windows(&source, 0, "document", &options);
        assert!(spans.len() > 2);
        assert_eq!(spans[0].start, 0);
        assert!(spans.windows(2).all(|pair| pair[1].start < pair[0].end));
        assert_eq!(spans.last().map(|span| span.end), Some(source.len()));
    }
}
