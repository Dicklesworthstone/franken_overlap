use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use csv::StringRecord;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

use crate::{
    atomic_write, sha256_hex, unix_timestamp, CorpusDocument, CorpusError, CorpusFailure,
    CorpusManifest, CorpusProvider, DownloadClient, HttpOptions, Result,
};

pub const GUTENBERG_CATALOG_URL: &str =
    "https://www.gutenberg.org/cache/epub/feeds/pg_catalog.csv.gz";
pub const DEFAULT_GUTENBERG_MIRROR: &str = "https://www.gutenberg.org/cache/epub";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GutenbergPreset {
    Smoke,
    Standard,
    Large,
}

impl GutenbergPreset {
    pub const fn document_limit(self) -> usize {
        match self {
            Self::Smoke => 25,
            Self::Standard => 250,
            Self::Large => 2_500,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GutenbergOptions {
    pub output_dir: PathBuf,
    pub preset: GutenbergPreset,
    pub document_limit: Option<usize>,
    pub explicit_ids: Vec<u64>,
    pub language: String,
    pub minimum_characters: usize,
    pub maximum_document_bytes: u64,
    pub mirror_base: String,
    pub catalog_url: String,
    pub seed: u64,
    pub overwrite: bool,
    pub refresh_catalog: bool,
    pub user_agent: String,
    pub request_interval: Duration,
    pub maximum_attempts: usize,
}

impl GutenbergOptions {
    pub fn validate(&self) -> Result<()> {
        if self.document_limit == Some(0)
            || self.minimum_characters == 0
            || self.maximum_document_bytes == 0
        {
            return Err(CorpusError::Invalid(
                "Gutenberg limits must be positive".to_owned(),
            ));
        }
        if self.language.trim().is_empty()
            || self.mirror_base.trim().is_empty()
            || self.catalog_url.trim().is_empty()
        {
            return Err(CorpusError::Invalid(
                "Gutenberg language, mirror, and catalog URL must not be empty".to_owned(),
            ));
        }
        let effective_limit = self
            .document_limit
            .unwrap_or_else(|| self.preset.document_limit())
            .max(self.explicit_ids.len());
        if effective_limit > 100
            && self.mirror_base.trim_end_matches('/') == DEFAULT_GUTENBERG_MIRROR
        {
            return Err(CorpusError::Invalid(
                "downloads above 100 books require an explicit Project Gutenberg mirror via \
                 --mirror-base or GUTENBERG_MIRROR; the main site must not be used as a bulk mirror"
                    .to_owned(),
            ));
        }
        if self.maximum_attempts == 0 {
            return Err(CorpusError::Invalid(
                "maximum attempts must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Default for GutenbergOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("corpora/gutenberg"),
            preset: GutenbergPreset::Smoke,
            document_limit: None,
            explicit_ids: Vec::new(),
            language: "en".to_owned(),
            minimum_characters: 10_000,
            maximum_document_bytes: 32 * 1024 * 1024,
            mirror_base: std::env::var("GUTENBERG_MIRROR")
                .unwrap_or_else(|_| DEFAULT_GUTENBERG_MIRROR.to_owned()),
            catalog_url: GUTENBERG_CATALOG_URL.to_owned(),
            seed: 0x67_75_74_65_6e_62_65_72,
            overwrite: false,
            refresh_catalog: false,
            user_agent: format!(
                "FrankenOverlap/{} Gutenberg corpus acquisition",
                env!("CARGO_PKG_VERSION")
            ),
            request_interval: Duration::from_secs(2),
            maximum_attempts: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GutenbergFetchReport {
    pub manifest: CorpusManifest,
    pub requested: usize,
    pub downloaded: usize,
    pub reused: usize,
    pub rejected_too_short: usize,
    pub failed: usize,
    pub catalog_candidates: usize,
}

#[derive(Debug, Clone)]
struct GutenbergCandidate {
    id: u64,
    title: String,
    authors: String,
    language: String,
    subjects: String,
    issued: String,
}

pub fn fetch_gutenberg(options: GutenbergOptions) -> Result<GutenbergFetchReport> {
    options.validate()?;
    fs::create_dir_all(options.output_dir.join("documents"))
        .map_err(|error| CorpusError::io(options.output_dir.join("documents"), error))?;
    fs::create_dir_all(options.output_dir.join("metadata"))
        .map_err(|error| CorpusError::io(options.output_dir.join("metadata"), error))?;

    let mut manifest = CorpusManifest::load_or_new(
        &options.output_dir,
        format!(
            "gutenberg-{}-{}",
            options.language,
            options
                .document_limit
                .unwrap_or_else(|| options.preset.document_limit())
                .max(options.explicit_ids.len())
        ),
        CorpusProvider::ProjectGutenberg,
    )?;
    let mut client = DownloadClient::new(HttpOptions {
        user_agent: options.user_agent.clone(),
        minimum_interval: options.request_interval,
        maximum_attempts: options.maximum_attempts,
        ..HttpOptions::default()
    })?;

    let catalog_path = options.output_dir.join("metadata/pg_catalog.csv.gz");
    let catalog_bytes = if catalog_path.exists() && !options.refresh_catalog {
        fs::read(&catalog_path).map_err(|error| CorpusError::io(&catalog_path, error))?
    } else {
        let response = client.get(&options.catalog_url, 64 * 1024 * 1024)?;
        atomic_write(&catalog_path, &response.bytes)?;
        manifest
            .source_snapshot
            .insert("catalog_final_url".to_owned(), response.final_url);
        if let Some(etag) = response.etag {
            manifest
                .source_snapshot
                .insert("catalog_etag".to_owned(), etag);
        }
        if let Some(last_modified) = response.last_modified {
            manifest
                .source_snapshot
                .insert("catalog_last_modified".to_owned(), last_modified);
        }
        response.bytes
    };
    manifest
        .source_snapshot
        .insert("catalog_sha256".to_owned(), sha256_hex(&catalog_bytes));
    manifest
        .source_snapshot
        .insert("mirror_base".to_owned(), options.mirror_base.clone());
    manifest
        .source_snapshot
        .insert("language".to_owned(), options.language.clone());

    let catalog_candidates = parse_catalog(&catalog_bytes, &options.language)?;
    let selected = select_candidates(
        &catalog_candidates,
        &options.explicit_ids,
        options
            .document_limit
            .unwrap_or_else(|| options.preset.document_limit()),
        options.seed,
    );
    let requested = selected.len();
    let mut downloaded = 0usize;
    let mut reused = 0usize;
    let mut rejected_too_short = 0usize;
    let mut failed = 0usize;

    for candidate in selected {
        let id = candidate.id.to_string();
        let relative_path = format!(
            "documents/{}/pg{}.txt",
            candidate.id / 1_000,
            candidate.id
        );
        let destination = options.output_dir.join(&relative_path);
        if !options.overwrite {
            if let Some(existing) = manifest.document(&id)
                && destination.is_file()
            {
                let bytes =
                    fs::read(&destination).map_err(|error| CorpusError::io(&destination, error))?;
                if sha256_hex(&bytes) == existing.sha256 {
                    reused += 1;
                    continue;
                }
            }
        }

        let url = format!(
            "{}/{}/pg{}.txt",
            options.mirror_base.trim_end_matches('/'),
            candidate.id,
            candidate.id
        );
        match client.get(&url, options.maximum_document_bytes) {
            Ok(response) => {
                let decoded = String::from_utf8_lossy(&response.bytes);
                let cleaned = strip_gutenberg_boilerplate(&decoded);
                if cleaned.chars().count() < options.minimum_characters {
                    rejected_too_short += 1;
                    manifest.record_failure(CorpusFailure {
                        id,
                        source_url: Some(response.final_url),
                        message: format!(
                            "document contained fewer than {} characters after cleanup",
                            options.minimum_characters
                        ),
                        observed_at_unix: unix_timestamp(),
                    });
                    continue;
                }
                let bytes = cleaned.into_bytes();
                atomic_write(&destination, &bytes)?;
                let mut metadata = BTreeMap::new();
                metadata.insert("gutenberg_id".to_owned(), candidate.id.to_string());
                metadata.insert("subjects".to_owned(), candidate.subjects.clone());
                manifest.upsert_document(CorpusDocument {
                    id,
                    relative_path,
                    source_url: response.final_url,
                    title: candidate.title,
                    author_or_issuer: candidate.authors,
                    language: Some(candidate.language),
                    published_or_filed: nonempty(candidate.issued),
                    sha256: sha256_hex(&bytes),
                    bytes: bytes.len() as u64,
                    characters: String::from_utf8_lossy(&bytes).chars().count(),
                    downloaded_at_unix: unix_timestamp(),
                    metadata,
                });
                downloaded += 1;
                if downloaded % 10 == 0 {
                    manifest.save(&options.output_dir)?;
                }
            }
            Err(error) => {
                failed += 1;
                manifest.record_failure(CorpusFailure {
                    id,
                    source_url: Some(url),
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
            }
        }
    }

    manifest.save(&options.output_dir)?;
    Ok(GutenbergFetchReport {
        manifest,
        requested,
        downloaded,
        reused,
        rejected_too_short,
        failed,
        catalog_candidates: catalog_candidates.len(),
    })
}

fn parse_catalog(bytes: &[u8], language: &str) -> Result<Vec<GutenbergCandidate>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut csv_bytes = Vec::new();
    decoder
        .read_to_end(&mut csv_bytes)
        .map_err(|error| CorpusError::io("pg_catalog.csv.gz", error))?;
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(csv_bytes.as_slice());
    let headers = reader.headers()?.clone();
    let id_index = header_index(&headers, &["Text#", "Text", "ebook_id"])?;
    let type_index = header_index(&headers, &["Type", "Category"])?;
    let language_index = header_index(&headers, &["Language", "Languages"])?;
    let title_index = header_index(&headers, &["Title"])?;
    let author_index = header_index_optional(&headers, &["Authors", "Author"]);
    let subject_index = header_index_optional(&headers, &["Subjects", "Subject"]);
    let issued_index = header_index_optional(&headers, &["Issued", "Release Date"]);

    let language = language.to_ascii_lowercase();
    let mut candidates = Vec::new();
    for row in reader.records() {
        let row = row?;
        let category = row.get(type_index).unwrap_or_default();
        if !category.eq_ignore_ascii_case("text") {
            continue;
        }
        let row_language = row.get(language_index).unwrap_or_default();
        if !row_language
            .split([';', ',', ' '])
            .any(|value| value.eq_ignore_ascii_case(&language))
        {
            continue;
        }
        let Some(id) = row
            .get(id_index)
            .and_then(|value| value.trim().parse::<u64>().ok())
        else {
            continue;
        };
        let title = row.get(title_index).unwrap_or_default().trim();
        if title.is_empty() {
            continue;
        }
        candidates.push(GutenbergCandidate {
            id,
            title: title.to_owned(),
            authors: author_index
                .and_then(|index| row.get(index))
                .unwrap_or_default()
                .trim()
                .to_owned(),
            language: row_language.trim().to_owned(),
            subjects: subject_index
                .and_then(|index| row.get(index))
                .unwrap_or_default()
                .trim()
                .to_owned(),
            issued: issued_index
                .and_then(|index| row.get(index))
                .unwrap_or_default()
                .trim()
                .to_owned(),
        });
    }
    candidates.sort_unstable_by_key(|candidate| candidate.id);
    Ok(candidates)
}

fn select_candidates(
    catalog: &[GutenbergCandidate],
    explicit_ids: &[u64],
    limit: usize,
    seed: u64,
) -> Vec<GutenbergCandidate> {
    if !explicit_ids.is_empty() {
        let requested = explicit_ids.iter().copied().collect::<BTreeSet<_>>();
        let mut selected = catalog
            .iter()
            .filter(|candidate| requested.contains(&candidate.id))
            .cloned()
            .collect::<Vec<_>>();
        let present = selected
            .iter()
            .map(|candidate| candidate.id)
            .collect::<BTreeSet<_>>();
        for id in requested.difference(&present) {
            selected.push(GutenbergCandidate {
                id: *id,
                title: format!("Project Gutenberg eBook #{id}"),
                authors: String::new(),
                language: String::new(),
                subjects: String::new(),
                issued: String::new(),
            });
        }
        selected.sort_unstable_by_key(|candidate| candidate.id);
        selected.truncate(limit.max(explicit_ids.len()));
        return selected;
    }
    let mut selected = catalog.to_vec();
    selected.sort_unstable_by_key(|candidate| stable_mix(candidate.id ^ seed));
    selected.truncate(limit);
    selected.sort_unstable_by_key(|candidate| candidate.id);
    selected
}

fn header_index(headers: &StringRecord, names: &[&str]) -> Result<usize> {
    header_index_optional(headers, names).ok_or_else(|| {
        CorpusError::Invalid(format!(
            "Project Gutenberg catalog is missing one of required headers: {}",
            names.join(", ")
        ))
    })
}

fn header_index_optional(headers: &StringRecord, names: &[&str]) -> Option<usize> {
    headers.iter().position(|header| {
        names
            .iter()
            .any(|name| header.trim().eq_ignore_ascii_case(name))
    })
}

fn strip_gutenberg_boilerplate(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| {
            let uppercase = line.to_ascii_uppercase();
            uppercase.contains("*** START OF THE PROJECT GUTENBERG")
                || uppercase.contains("***START OF THE PROJECT GUTENBERG")
        })
        .map_or(0, |index| index + 1);
    let end = lines[start..]
        .iter()
        .position(|line| {
            let uppercase = line.to_ascii_uppercase();
            uppercase.contains("*** END OF THE PROJECT GUTENBERG")
                || uppercase.contains("***END OF THE PROJECT GUTENBERG")
        })
        .map_or(lines.len(), |offset| start + offset);
    let mut output = lines[start..end].join("\n");
    while output.contains("\n\n\n") {
        output = output.replace("\n\n\n", "\n\n");
    }
    output.trim().to_owned()
}

fn stable_mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{select_candidates, strip_gutenberg_boilerplate, GutenbergCandidate};

    #[test]
    fn removes_standard_header_and_footer() {
        let input = "header\n*** START OF THE PROJECT GUTENBERG EBOOK TEST ***\nbody\ntext\n*** END OF THE PROJECT GUTENBERG EBOOK TEST ***\nfooter";
        assert_eq!(strip_gutenberg_boilerplate(input), "body\ntext");
    }

    #[test]
    fn deterministic_selection_is_seeded() {
        let catalog = (1..100)
            .map(|id| GutenbergCandidate {
                id,
                title: id.to_string(),
                authors: String::new(),
                language: "en".to_owned(),
                subjects: String::new(),
                issued: String::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            select_candidates(&catalog, &[], 10, 7)
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            select_candidates(&catalog, &[], 10, 7)
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>()
        );
    }
}
