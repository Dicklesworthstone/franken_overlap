use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CORPUS_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_FILENAME: &str = "manifest.json";

pub type Result<T> = std::result::Result<T, CorpusError>;

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error("invalid corpus configuration: {0}")]
    Invalid(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("CSV parsing failed: {0}")]
    Csv(#[from] csv::Error),
    #[error("JSON parsing failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("HTML extraction failed: {0}")]
    Html(String),
    #[error("downloaded object exceeded the {limit}-byte safety limit: {url}")]
    DownloadTooLarge { url: String, limit: u64 },
    #[error("remote server returned HTTP {status} for {url}")]
    HttpStatus { url: String, status: u16 },
    #[error("manifest verification failed: {0}")]
    Verification(String),
}

impl CorpusError {
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusManifest {
    pub schema_version: u32,
    pub corpus_id: String,
    pub provider: CorpusProvider,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub source_snapshot: BTreeMap<String, String>,
    pub documents: Vec<CorpusDocument>,
    pub failures: Vec<CorpusFailure>,
}

impl CorpusManifest {
    pub fn new(corpus_id: impl Into<String>, provider: CorpusProvider) -> Self {
        let now = unix_timestamp();
        Self {
            schema_version: CORPUS_MANIFEST_SCHEMA_VERSION,
            corpus_id: corpus_id.into(),
            provider,
            created_at_unix: now,
            updated_at_unix: now,
            source_snapshot: BTreeMap::new(),
            documents: Vec::new(),
            failures: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CORPUS_MANIFEST_SCHEMA_VERSION {
            return Err(CorpusError::Invalid(format!(
                "unsupported corpus manifest schema {}",
                self.schema_version
            )));
        }
        if self.corpus_id.trim().is_empty() {
            return Err(CorpusError::Invalid(
                "corpus_id must not be empty".to_owned(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        for document in &self.documents {
            document.validate()?;
            if !ids.insert(document.id.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate document id {}",
                    document.id
                )));
            }
            if !paths.insert(document.relative_path.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate document path {}",
                    document.relative_path
                )));
            }
        }
        Ok(())
    }

    pub fn document(&self, id: &str) -> Option<&CorpusDocument> {
        self.documents.iter().find(|document| document.id == id)
    }

    pub fn upsert_document(&mut self, document: CorpusDocument) {
        if let Some(existing) = self
            .documents
            .iter_mut()
            .find(|existing| existing.id == document.id)
        {
            *existing = document;
        } else {
            self.documents.push(document);
        }
        self.documents.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        self.updated_at_unix = unix_timestamp();
    }

    pub fn record_failure(&mut self, failure: CorpusFailure) {
        self.failures.push(failure);
        self.updated_at_unix = unix_timestamp();
    }

    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|error| CorpusError::io(root, error))?;
        let destination = root.join(MANIFEST_FILENAME);
        let bytes = serde_json::to_vec_pretty(self)?;
        atomic_write(&destination, &bytes)
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(MANIFEST_FILENAME);
        let bytes = fs::read(&path).map_err(|error| CorpusError::io(&path, error))?;
        let manifest = serde_json::from_slice::<Self>(&bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load_or_new(
        root: impl AsRef<Path>,
        corpus_id: impl Into<String>,
        provider: CorpusProvider,
    ) -> Result<Self> {
        let root = root.as_ref();
        let path = root.join(MANIFEST_FILENAME);
        if path.exists() {
            let manifest = Self::load(root)?;
            if manifest.provider != provider {
                return Err(CorpusError::Invalid(format!(
                    "existing manifest provider {:?} does not match requested {:?}",
                    manifest.provider, provider
                )));
            }
            Ok(manifest)
        } else {
            Ok(Self::new(corpus_id, provider))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusProvider {
    ProjectGutenberg,
    SecEdgar10K,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusDocument {
    pub id: String,
    pub relative_path: String,
    pub source_url: String,
    pub title: String,
    pub author_or_issuer: String,
    pub language: Option<String>,
    pub published_or_filed: Option<String>,
    pub sha256: String,
    pub bytes: u64,
    pub characters: usize,
    pub downloaded_at_unix: u64,
    pub metadata: BTreeMap<String, String>,
}

impl CorpusDocument {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty()
            || self.relative_path.trim().is_empty()
            || self.source_url.trim().is_empty()
            || self.sha256.len() != 64
        {
            return Err(CorpusError::Invalid(format!(
                "document {} has invalid identity, path, URL, or SHA-256",
                self.id
            )));
        }
        let path = Path::new(&self.relative_path);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(CorpusError::Invalid(format!(
                "document {} uses unsafe relative path {}",
                self.id, self.relative_path
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFailure {
    pub id: String,
    pub source_url: Option<String>,
    pub message: String,
    pub observed_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestVerificationReport {
    pub corpus_id: String,
    pub documents: usize,
    pub verified: usize,
    pub missing: Vec<String>,
    pub mismatched: Vec<String>,
    pub total_bytes: u64,
}

pub fn verify_manifest(root: impl AsRef<Path>) -> Result<ManifestVerificationReport> {
    let root = root.as_ref();
    let manifest = CorpusManifest::load(root)?;
    let mut report = ManifestVerificationReport {
        corpus_id: manifest.corpus_id.clone(),
        documents: manifest.documents.len(),
        verified: 0,
        missing: Vec::new(),
        mismatched: Vec::new(),
        total_bytes: 0,
    };
    for document in &manifest.documents {
        let path = root.join(&document.relative_path);
        if !path.is_file() {
            report.missing.push(document.relative_path.clone());
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| CorpusError::io(&path, error))?;
        report.total_bytes = report.total_bytes.saturating_add(bytes.len() as u64);
        let digest = sha256_hex(&bytes);
        if digest != document.sha256 || bytes.len() as u64 != document.bytes {
            report.mismatched.push(document.relative_path.clone());
        } else {
            report.verified += 1;
        }
    }
    if !report.missing.is_empty() || !report.mismatched.is_empty() {
        return Err(CorpusError::Verification(format!(
            "{} missing and {} mismatched documents",
            report.missing.len(),
            report.mismatched.len()
        )));
    }
    Ok(report)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| CorpusError::io(parent, error))?;
    }
    let mut temporary = path.to_path_buf();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("tmp");
    temporary.set_extension(format!("{extension}.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| CorpusError::io(&temporary, error))?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| CorpusError::io(path, error))?;
    }
    fs::rename(&temporary, path).map_err(|error| CorpusError::io(path, error))
}

#[cfg(test)]
mod tests {
    use super::{CorpusDocument, CorpusManifest, CorpusProvider, sha256_hex};

    #[test]
    fn sha256_is_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_rejects_unsafe_paths() {
        let mut manifest = CorpusManifest::new("test", CorpusProvider::ProjectGutenberg);
        manifest.documents.push(CorpusDocument {
            id: "1".to_owned(),
            relative_path: "../escape.txt".to_owned(),
            source_url: "https://example.invalid/1".to_owned(),
            title: "test".to_owned(),
            author_or_issuer: "test".to_owned(),
            language: None,
            published_or_filed: None,
            sha256: "0".repeat(64),
            bytes: 0,
            characters: 0,
            downloaded_at_unix: 0,
            metadata: Default::default(),
        });
        assert!(manifest.validate().is_err());
    }
}
