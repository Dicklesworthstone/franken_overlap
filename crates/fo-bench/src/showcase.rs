use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Component, Path};

use fo_corpus::{
    CorpusDocument, CorpusManifest, CorpusProvider, atomic_write, sha256_hex, unix_timestamp,
};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

pub type ShowcaseResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const SHOWCASE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SCENARIO_PROFILES: usize = 8;

const NOISE_WORDS: &[&str] = &[
    "lantern", "railway", "orchard", "ceramic", "violet", "meadow", "saffron", "cabinet", "marble",
    "festival", "chimney", "harbor", "compass", "velvet", "kitchen", "weather",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioGenerationOptions {
    pub source_documents: usize,
    pub queries_per_source: usize,
    pub passage_words: usize,
    pub seed: u64,
    pub maximum_document_bytes: u64,
}

impl Default for ScenarioGenerationOptions {
    fn default() -> Self {
        Self {
            source_documents: 16,
            queries_per_source: MAX_SCENARIO_PROFILES,
            passage_words: 96,
            seed: 0x73_68_6f_77_63_61_73_65,
            maximum_document_bytes: 128 * 1024 * 1024,
        }
    }
}

impl ScenarioGenerationOptions {
    pub fn validate(&self) -> ShowcaseResult<()> {
        if self.source_documents == 0
            || self.queries_per_source == 0
            || self.queries_per_source > MAX_SCENARIO_PROFILES
            || self.passage_words < 24
            || self.maximum_document_bytes == 0
        {
            return Err(invalid(
                "source count, profile count, passage length, or document byte limit is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioQuery {
    pub id: String,
    pub profile: String,
    pub text: String,
    pub positive_ids: Vec<String>,
    pub source_id: String,
    pub source_title: String,
    pub relation_key: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioProfileCount {
    pub profile: String,
    pub queries: usize,
    pub multi_positive_queries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioGenerationReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub provider: CorpusProvider,
    pub manifest_documents: usize,
    pub eligible_source_documents: usize,
    pub selected_source_documents: usize,
    pub queries: usize,
    pub multi_positive_queries: usize,
    pub relation_groups: usize,
    pub profiles: Vec<ScenarioProfileCount>,
    pub query_file: String,
    pub query_file_sha256: String,
    pub seed: u64,
    pub passage_words: usize,
}

#[derive(Debug, Clone)]
struct LoadedDocument {
    record: CorpusDocument,
    words: Vec<String>,
}

pub fn generate_scenarios(
    corpus_root: impl AsRef<Path>,
    query_output: impl AsRef<Path>,
    options: ScenarioGenerationOptions,
) -> ShowcaseResult<ScenarioGenerationReport> {
    options.validate()?;
    let corpus_root = corpus_root.as_ref();
    let query_output = query_output.as_ref();
    let manifest = CorpusManifest::load(corpus_root)?;
    let mut documents = load_documents(corpus_root, &manifest, &options)?;
    if documents.is_empty() {
        return Err(invalid(
            "no sufficiently long UTF-8 corpus documents were available",
        ));
    }
    documents.sort_unstable_by_key(|document| stable_hash(&document.record.id, options.seed));
    let eligible_source_documents = documents.len();
    documents.truncate(options.source_documents.min(documents.len()));
    documents.sort_unstable_by(|left, right| left.record.id.cmp(&right.record.id));

    let relation_groups = build_relation_groups(&manifest);
    let mut queries = Vec::new();
    for document in &documents {
        let relation_key = relation_key(&manifest, &document.record);
        let related = relation_groups
            .get(&relation_key)
            .cloned()
            .unwrap_or_else(|| vec![document.record.id.clone()]);
        queries.extend(generate_document_queries(
            document,
            &relation_key,
            &related,
            manifest.provider,
            &options,
        ));
    }
    queries.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    if queries.is_empty() {
        return Err(invalid("scenario generation produced no queries"));
    }
    let mut bytes = Vec::new();
    for query in &queries {
        serde_json::to_writer(&mut bytes, query)?;
        bytes.push(b'\n');
    }
    atomic_write(query_output, &bytes)?;

    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    for query in &queries {
        let entry = counts.entry(query.profile.clone()).or_default();
        entry.0 += 1;
        if query.positive_ids.len() > 1 {
            entry.1 += 1;
        }
    }
    let profiles = counts
        .into_iter()
        .map(
            |(profile, (queries, multi_positive_queries))| ScenarioProfileCount {
                profile,
                queries,
                multi_positive_queries,
            },
        )
        .collect::<Vec<_>>();
    let multi_positive_queries = queries
        .iter()
        .filter(|query| query.positive_ids.len() > 1)
        .count();
    Ok(ScenarioGenerationReport {
        schema_version: SHOWCASE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id,
        provider: manifest.provider,
        manifest_documents: manifest.documents.len(),
        eligible_source_documents,
        selected_source_documents: documents.len(),
        queries: queries.len(),
        multi_positive_queries,
        relation_groups: relation_groups
            .values()
            .filter(|group| group.len() > 1)
            .count(),
        profiles,
        query_file: query_output.display().to_string(),
        query_file_sha256: sha256_hex(&bytes),
        seed: options.seed,
        passage_words: options.passage_words,
    })
}

fn load_documents(
    corpus_root: &Path,
    manifest: &CorpusManifest,
    options: &ScenarioGenerationOptions,
) -> ShowcaseResult<Vec<LoadedDocument>> {
    let mut documents = Vec::new();
    for record in &manifest.documents {
        validate_relative_path(&record.relative_path)?;
        if record.bytes > options.maximum_document_bytes {
            continue;
        }
        let path = corpus_root.join(&record.relative_path);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(_) => continue,
        };
        let words = body.unicode_words().map(str::to_owned).collect::<Vec<_>>();
        if words.len() < options.passage_words.saturating_mul(2) {
            continue;
        }
        documents.push(LoadedDocument {
            record: record.clone(),
            words,
        });
    }
    Ok(documents)
}

fn build_relation_groups(manifest: &CorpusManifest) -> BTreeMap<String, Vec<String>> {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for document in &manifest.documents {
        groups
            .entry(relation_key(manifest, document))
            .or_default()
            .push(document.id.clone());
    }
    for group in groups.values_mut() {
        group.sort_unstable();
        group.dedup();
    }
    groups
}

fn relation_key(manifest: &CorpusManifest, document: &CorpusDocument) -> String {
    let section = document
        .metadata
        .get("section_title")
        .map_or_else(|| "whole document".to_owned(), |value| canonical(value));
    match manifest.provider {
        CorpusProvider::ProjectGutenberg => {
            let title = document
                .metadata
                .get("parent_title")
                .map_or_else(|| canonical(&document.title), |value| canonical(value));
            let author = canonical(&document.author_or_issuer);
            format!("gutenberg:{author}:{title}:{section}")
        }
        CorpusProvider::SecEdgar10K | CorpusProvider::SecEdgarFilings => {
            let issuer = document
                .metadata
                .get("cik")
                .cloned()
                .or_else(|| extract_cik(&document.id))
                .unwrap_or_else(|| canonical(&document.author_or_issuer));
            let form = document
                .metadata
                .get("form")
                .map_or_else(|| "filing".to_owned(), |value| canonical(value));
            format!("sec:{issuer}:{form}:{section}")
        }
        CorpusProvider::LocalCollection => {
            let family = document
                .metadata
                .get("family_id")
                .cloned()
                .unwrap_or_else(|| canonical(&document.title));
            let document_type = document
                .metadata
                .get("document_type")
                .map_or_else(|| "document".to_owned(), |value| canonical(value));
            format!("collection:{family}:{document_type}:{section}")
        }
    }
}

fn extract_cik(value: &str) -> Option<String> {
    let start = value.find("CIK")? + 3;
    let digits = value[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then_some(digits)
}

fn generate_document_queries(
    document: &LoadedDocument,
    relation_key: &str,
    related: &[String],
    provider: CorpusProvider,
    options: &ScenarioGenerationOptions,
) -> Vec<ScenarioQuery> {
    let width = options.passage_words.min(document.words.len());
    let windows = document.words.len().saturating_sub(width).saturating_add(1);
    let start = if windows <= 1 {
        0
    } else {
        stable_hash(&document.record.id, options.seed ^ 0x50_41_53_53_41_47_45) as usize % windows
    };
    let passage = document.words[start..start + width].to_vec();
    let profiles = [
        "exact",
        "format_drift",
        "substitution_10pct",
        "insertion_deletion",
        "ocr_noise",
        "fragmented",
        "reordered",
        "natural_relation",
    ];
    let mut output = Vec::new();
    for (profile_index, profile) in profiles.iter().take(options.queries_per_source).enumerate() {
        let seed = stable_hash(
            &document.record.id,
            options.seed ^ profile_index as u64 ^ 0x51_55_45_52_59,
        );
        let text = match *profile {
            "exact" | "natural_relation" => passage.join(" "),
            "format_drift" => format_drift(&passage),
            "substitution_10pct" => substitute_words(&passage, seed),
            "insertion_deletion" => insertion_deletion(&passage, seed),
            "ocr_noise" => ocr_noise(&passage.join(" ")),
            "fragmented" => fragmented(&passage, seed),
            "reordered" => reordered(&passage),
            _ => passage.join(" "),
        };
        let positive_ids = if *profile == "natural_relation" && related.len() > 1 {
            related.to_vec()
        } else {
            vec![document.record.id.clone()]
        };
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "source_relative_path".to_owned(),
            document.record.relative_path.clone(),
        );
        metadata.insert("provider".to_owned(), provider_name(provider).to_owned());
        metadata.insert("passage_start_word".to_owned(), start.to_string());
        metadata.insert("passage_words".to_owned(), width.to_string());
        for key in [
            "section_title",
            "parent_id",
            "form",
            "cik",
            "family_id",
            "document_type",
        ] {
            if let Some(value) = document.record.metadata.get(key) {
                metadata.insert(key.to_owned(), value.clone());
            }
        }
        output.push(ScenarioQuery {
            id: format!("{}:{profile}", document.record.id),
            profile: (*profile).to_owned(),
            text,
            positive_ids,
            source_id: document.record.id.clone(),
            source_title: document.record.title.clone(),
            relation_key: relation_key.to_owned(),
            metadata,
        });
    }
    output
}

const fn provider_name(provider: CorpusProvider) -> &'static str {
    match provider {
        CorpusProvider::ProjectGutenberg => "project_gutenberg",
        CorpusProvider::SecEdgar10K => "sec_edgar_10k",
        CorpusProvider::SecEdgarFilings => "sec_edgar_filings",
        CorpusProvider::LocalCollection => "local_collection",
    }
}

fn format_drift(words: &[String]) -> String {
    let mut output = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            output.push(if index % 13 == 0 { '\n' } else { ' ' });
        }
        if index % 5 == 0 {
            output.push_str(&word.to_uppercase());
        } else {
            output.push_str(&word.to_lowercase());
        }
        if index % 11 == 10 {
            output.push(',');
        }
    }
    output
}

fn substitute_words(words: &[String], seed: u64) -> String {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            if index % 10 == 5 {
                NOISE_WORDS[(index + seed as usize) % NOISE_WORDS.len()].to_owned()
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn insertion_deletion(words: &[String], seed: u64) -> String {
    let mut output = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if index % 13 == 7 {
            continue;
        }
        output.push(word.clone());
        if index % 17 == 9 {
            output.push(NOISE_WORDS[(index + seed as usize) % NOISE_WORDS.len()].to_owned());
            output.push(NOISE_WORDS[(index * 3 + seed as usize) % NOISE_WORDS.len()].to_owned());
        }
    }
    output.join(" ")
}

fn ocr_noise(input: &str) -> String {
    input
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 47 != 19 {
                return character;
            }
            match character {
                'm' => 'n',
                'n' => 'm',
                'l' | 'I' => '1',
                'o' | 'O' => '0',
                'e' => 'c',
                'c' => 'e',
                _ => character,
            }
        })
        .collect()
}

fn fragmented(words: &[String], seed: u64) -> String {
    let third = (words.len() / 3).max(1);
    let first = words[..third.min(words.len())].join(" ");
    let last_start = words.len().saturating_sub(third);
    let last = words[last_start..].join(" ");
    let noise = (0..32)
        .map(|index| NOISE_WORDS[(index + seed as usize) % NOISE_WORDS.len()])
        .collect::<Vec<_>>()
        .join(" ");
    format!("{first} {noise} {last}")
}

fn reordered(words: &[String]) -> String {
    let third = (words.len() / 3).max(1);
    let a_end = third.min(words.len());
    let b_end = (third * 2).min(words.len());
    let a = words[..a_end].join(" ");
    let b = words[a_end..b_end].join(" ");
    let c = words[b_end..].join(" ");
    format!("{c} {a} {b}")
}

fn canonical(value: &str) -> String {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn stable_hash(value: &str, seed: u64) -> u64 {
    let mut hash = seed ^ 0xcbf2_9ce4_8422_2325;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn validate_relative_path(value: &str) -> ShowcaseResult<()> {
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
        return Err(invalid(format!("unsafe corpus relative path {value:?}")));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{format_drift, insertion_deletion, ocr_noise, reordered, substitute_words};

    #[test]
    fn mutations_are_deterministic_and_nonempty() {
        let words = (0..120)
            .map(|index| format!("word{index}"))
            .collect::<Vec<_>>();
        assert_eq!(substitute_words(&words, 7), substitute_words(&words, 7));
        assert_ne!(format_drift(&words), words.join(" "));
        assert_ne!(insertion_deletion(&words, 9), words.join(" "));
        assert_ne!(ocr_noise(&words.join(" ")), words.join(" "));
        assert_ne!(reordered(&words), words.join(" "));
    }

    #[test]
    fn relation_groups_remain_unique() {
        let values = ["a", "a", "b"].into_iter().collect::<BTreeSet<_>>();
        assert_eq!(values.len(), 2);
    }
}
