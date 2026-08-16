use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    atomic_write, sha256_hex, unix_timestamp, verify_manifest, CorpusDocument, CorpusError,
    CorpusManifest, CorpusProvider, ManifestVerificationReport, Result,
};

pub const COLLECTION_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const COLLECTION_MANIFEST_FILENAME: &str = "collection.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionProfile {
    General,
    SecFilings,
    RetailLease,
    ProfessionalServices,
    Nda,
    Contract,
    Policy,
    SourceCode,
    Research,
}

impl CollectionProfile {
    #[must_use]
    pub const fn default_document_type(self) -> &'static str {
        match self {
            Self::General => "document",
            Self::SecFilings => "sec_filing",
            Self::RetailLease => "retail_lease",
            Self::ProfessionalServices => "professional_services_agreement",
            Self::Nda => "nda",
            Self::Contract => "contract",
            Self::Policy => "policy",
            Self::SourceCode => "source_code",
            Self::Research => "research_document",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionRelationKind {
    PreviousVersion,
    AmendmentOf,
    RestatementOf,
    Supersedes,
    ExhibitTo,
    IncorporatesByReference,
    Governs,
    TemplateFor,
    RelatedTo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRelation {
    pub from_id: String,
    pub to_id: String,
    pub kind: CollectionRelationKind,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDocumentRecord {
    pub id: String,
    pub source_path: String,
    pub stored_path: String,
    pub title: String,
    pub family_id: String,
    pub version_id: String,
    pub document_type: String,
    pub effective_date: Option<String>,
    pub executed_date: Option<String>,
    #[serde(default)]
    pub parties: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub sha256: String,
    pub bytes: u64,
    pub characters: usize,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl CollectionDocumentRecord {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("id", self.id.as_str()),
            ("source_path", self.source_path.as_str()),
            ("stored_path", self.stored_path.as_str()),
            ("title", self.title.as_str()),
            ("family_id", self.family_id.as_str()),
            ("version_id", self.version_id.as_str()),
            ("document_type", self.document_type.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(CorpusError::Invalid(format!(
                    "collection document {} has empty {name}",
                    self.id
                )));
            }
        }
        validate_relative_path(&self.source_path)?;
        validate_relative_path(&self.stored_path)?;
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CorpusError::Invalid(format!(
                "collection document {} has invalid SHA-256",
                self.id
            )));
        }
        for (name, date) in [
            ("effective_date", self.effective_date.as_deref()),
            ("executed_date", self.executed_date.as_deref()),
        ] {
            if let Some(date) = date
                && !valid_date(date)
            {
                return Err(CorpusError::Invalid(format!(
                    "collection document {} {name} must use YYYY-MM-DD",
                    self.id
                )));
            }
        }
        if self.parties.iter().any(|value| value.trim().is_empty())
            || self.tags.iter().any(|value| value.trim().is_empty())
            || self.metadata.keys().any(|value| value.trim().is_empty())
        {
            return Err(CorpusError::Invalid(format!(
                "collection document {} contains an empty party, tag, or metadata key",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionManifest {
    pub schema_version: u32,
    pub collection_id: String,
    pub profile: CollectionProfile,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub documents: Vec<CollectionDocumentRecord>,
    pub relations: Vec<CollectionRelation>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl CollectionManifest {
    #[must_use]
    pub fn new(collection_id: impl Into<String>, profile: CollectionProfile) -> Self {
        let now = unix_timestamp();
        Self {
            schema_version: COLLECTION_MANIFEST_SCHEMA_VERSION,
            collection_id: collection_id.into(),
            profile,
            created_at_unix: now,
            updated_at_unix: now,
            documents: Vec::new(),
            relations: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != COLLECTION_MANIFEST_SCHEMA_VERSION {
            return Err(CorpusError::Invalid(format!(
                "unsupported collection manifest schema {}",
                self.schema_version
            )));
        }
        if self.collection_id.trim().is_empty() {
            return Err(CorpusError::Invalid(
                "collection_id must not be empty".to_owned(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for document in &self.documents {
            document.validate()?;
            if !ids.insert(document.id.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate collection document ID {}",
                    document.id
                )));
            }
            if !paths.insert(document.stored_path.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate stored collection path {}",
                    document.stored_path
                )));
            }
        }
        let mut relations = BTreeSet::new();
        for relation in &self.relations {
            if relation.from_id == relation.to_id
                || !ids.contains(relation.from_id.as_str())
                || !ids.contains(relation.to_id.as_str())
            {
                return Err(CorpusError::Invalid(format!(
                    "invalid collection relation {} -> {}",
                    relation.from_id, relation.to_id
                )));
            }
            if !relations.insert((
                relation.from_id.as_str(),
                relation.to_id.as_str(),
                relation.kind,
            )) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate collection relation {} -> {} ({:?})",
                    relation.from_id, relation.to_id, relation.kind
                )));
            }
        }
        Ok(())
    }

    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let destination = root.as_ref().join(COLLECTION_MANIFEST_FILENAME);
        atomic_write(&destination, &serde_json::to_vec_pretty(self)?)
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(COLLECTION_MANIFEST_FILENAME);
        let manifest = serde_json::from_slice::<Self>(
            &fs::read(&path).map_err(|error| CorpusError::io(&path, error))?,
        )?;
        manifest.validate()?;
        Ok(manifest)
    }

    #[must_use]
    pub fn family(&self, family_id: &str) -> Vec<&CollectionDocumentRecord> {
        let mut documents = self
            .documents
            .iter()
            .filter(|document| document.family_id == family_id)
            .collect::<Vec<_>>();
        documents.sort_unstable_by(|left, right| version_key(left).cmp(&version_key(right)));
        documents
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionMetadataRow {
    pub source_path: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub family_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub document_type: Option<String>,
    #[serde(default)]
    pub effective_date: Option<String>,
    #[serde(default)]
    pub executed_date: Option<String>,
    #[serde(default)]
    pub parties: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub previous_version_id: Option<String>,
    #[serde(default)]
    pub amends_id: Option<String>,
    #[serde(default)]
    pub supersedes_id: Option<String>,
    #[serde(default)]
    pub related_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CollectionImportOptions {
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub collection_id: String,
    pub profile: CollectionProfile,
    pub metadata_jsonl: Option<PathBuf>,
    pub maximum_document_bytes: u64,
    pub all_files: bool,
    pub replace_output: bool,
    pub infer_previous_versions: bool,
}

impl Default for CollectionImportOptions {
    fn default() -> Self {
        Self {
            source_dir: PathBuf::from("documents"),
            output_dir: PathBuf::from("corpora/collection"),
            collection_id: "collection".to_owned(),
            profile: CollectionProfile::General,
            metadata_jsonl: None,
            maximum_document_bytes: 128 * 1024 * 1024,
            all_files: false,
            replace_output: false,
            infer_previous_versions: true,
        }
    }
}

impl CollectionImportOptions {
    pub fn validate(&self) -> Result<()> {
        if !self.source_dir.is_dir() {
            return Err(CorpusError::Invalid(format!(
                "collection source directory does not exist: {}",
                self.source_dir.display()
            )));
        }
        if self.collection_id.trim().is_empty() || self.maximum_document_bytes == 0 {
            return Err(CorpusError::Invalid(
                "collection ID and maximum document bytes must be positive/nonempty".to_owned(),
            ));
        }
        let source = absolute_path(&self.source_dir)?;
        let output = absolute_path(&self.output_dir)?;
        if output.starts_with(&source) {
            return Err(CorpusError::Invalid(
                "collection output directory must not be inside the source directory".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionImportReport {
    pub collection_id: String,
    pub profile: CollectionProfile,
    pub documents: usize,
    pub families: usize,
    pub relations: usize,
    pub total_bytes: u64,
    pub skipped_binary: usize,
    pub skipped_oversized: usize,
    pub unused_metadata_rows: Vec<String>,
    pub collection_manifest: String,
    pub corpus_manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionVerificationReport {
    pub collection_id: String,
    pub profile: CollectionProfile,
    pub documents: usize,
    pub families: usize,
    pub relations: usize,
    pub corpus: ManifestVerificationReport,
}

pub fn import_collection(options: CollectionImportOptions) -> Result<CollectionImportReport> {
    options.validate()?;
    if options.output_dir.exists() {
        if options.replace_output {
            fs::remove_dir_all(&options.output_dir)
                .map_err(|error| CorpusError::io(&options.output_dir, error))?;
        } else {
            return Err(CorpusError::Invalid(format!(
                "collection output already exists: {}",
                options.output_dir.display()
            )));
        }
    }
    fs::create_dir_all(options.output_dir.join("documents"))
        .map_err(|error| CorpusError::io(options.output_dir.join("documents"), error))?;

    let mut metadata = load_metadata(options.metadata_jsonl.as_deref())?;
    let mut paths = Vec::new();
    collect_files(&options.source_dir, options.all_files, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err(CorpusError::Invalid(
            "collection source contains no eligible files".to_owned(),
        ));
    }

    let mut collection = CollectionManifest::new(&options.collection_id, options.profile);
    collection.metadata.insert(
        "source_directory".to_owned(),
        options.source_dir.display().to_string(),
    );
    let mut corpus = CorpusManifest::new(&options.collection_id, CorpusProvider::LocalCollection);
    corpus.source_snapshot.insert(
        "collection_manifest".to_owned(),
        COLLECTION_MANIFEST_FILENAME.to_owned(),
    );
    corpus.source_snapshot.insert(
        "collection_profile".to_owned(),
        format!("{:?}", options.profile).to_ascii_lowercase(),
    );

    let mut used_ids = BTreeSet::new();
    let mut used_stored_paths = BTreeSet::new();
    let mut skipped_binary = 0usize;
    let mut skipped_oversized = 0usize;
    let mut explicit_relations = Vec::new();

    for path in paths {
        let size = fs::metadata(&path)
            .map_err(|error| CorpusError::io(&path, error))?
            .len();
        if size > options.maximum_document_bytes {
            skipped_oversized += 1;
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| CorpusError::io(&path, error))?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            skipped_binary += 1;
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let relative = path
            .strip_prefix(&options.source_dir)
            .map_err(|_| CorpusError::Invalid("source path escaped collection root".to_owned()))?;
        let relative_string = slash_path(relative);
        let row = metadata.remove(&relative_string);
        let digest = sha256_hex(&bytes);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("document");
        let effective_date = row
            .as_ref()
            .and_then(|value| value.effective_date.clone())
            .or_else(|| infer_date(stem));
        let family_id = row
            .as_ref()
            .and_then(|value| value.family_id.clone())
            .unwrap_or_else(|| infer_family_id(relative));
        let version_id = row
            .as_ref()
            .and_then(|value| value.version_id.clone())
            .or_else(|| effective_date.clone())
            .unwrap_or_else(|| stem.to_owned());
        let mut id = row
            .as_ref()
            .and_then(|value| value.id.clone())
            .unwrap_or_else(|| format!("{}--{}", slug(&family_id), slug(&version_id)));
        if !used_ids.insert(id.clone()) {
            id = format!("{}--{}", id, &digest[..12]);
            if !used_ids.insert(id.clone()) {
                return Err(CorpusError::Invalid(format!(
                    "could not derive unique ID for {}",
                    relative.display()
                )));
            }
        }
        let mut stored_path = format!("documents/{}.txt", slug(&id));
        if !used_stored_paths.insert(stored_path.clone()) {
            stored_path = format!("documents/{}-{}.txt", slug(&id), &digest[..12]);
            used_stored_paths.insert(stored_path.clone());
        }
        atomic_write(&options.output_dir.join(&stored_path), &bytes)?;

        let title = row
            .as_ref()
            .and_then(|value| value.title.clone())
            .unwrap_or_else(|| humanize(stem));
        let document_type = row
            .as_ref()
            .and_then(|value| value.document_type.clone())
            .unwrap_or_else(|| options.profile.default_document_type().to_owned());
        let parties = row
            .as_ref()
            .map(|value| value.parties.clone())
            .unwrap_or_default();
        let tags = row
            .as_ref()
            .map(|value| value.tags.clone())
            .unwrap_or_default();
        let executed_date = row
            .as_ref()
            .and_then(|value| value.executed_date.clone());
        let mut row_metadata = row
            .as_ref()
            .map(|value| value.metadata.clone())
            .unwrap_or_default();
        row_metadata.insert("collection_id".to_owned(), options.collection_id.clone());
        row_metadata.insert("family_id".to_owned(), family_id.clone());
        row_metadata.insert("version_id".to_owned(), version_id.clone());
        row_metadata.insert("document_type".to_owned(), document_type.clone());
        row_metadata.insert("source_path".to_owned(), relative_string.clone());
        row_metadata.insert(
            "collection_profile".to_owned(),
            format!("{:?}", options.profile).to_ascii_lowercase(),
        );
        if !parties.is_empty() {
            row_metadata.insert("parties".to_owned(), parties.join(" | "));
        }
        if !tags.is_empty() {
            row_metadata.insert("tags".to_owned(), tags.join(" | "));
        }
        if let Some(date) = &executed_date {
            row_metadata.insert("executed_date".to_owned(), date.clone());
        }

        collection.documents.push(CollectionDocumentRecord {
            id: id.clone(),
            source_path: relative_string.clone(),
            stored_path: stored_path.clone(),
            title: title.clone(),
            family_id,
            version_id,
            document_type,
            effective_date: effective_date.clone(),
            executed_date,
            parties: parties.clone(),
            tags: tags.clone(),
            sha256: digest.clone(),
            bytes: bytes.len() as u64,
            characters: text.chars().count(),
            metadata: row_metadata.clone(),
        });
        corpus.upsert_document(CorpusDocument {
            id: id.clone(),
            relative_path: stored_path,
            source_url: format!("file://{relative_string}"),
            title,
            author_or_issuer: if parties.is_empty() {
                "local collection".to_owned()
            } else {
                parties.join("; ")
            },
            language: Some("en".to_owned()),
            published_or_filed: effective_date,
            sha256: digest,
            bytes: bytes.len() as u64,
            characters: text.chars().count(),
            downloaded_at_unix: unix_timestamp(),
            metadata: row_metadata,
        });

        if let Some(row) = row {
            push_explicit_relations(&id, &row, &mut explicit_relations);
        }
    }

    collection.documents.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    collection.relations = explicit_relations;
    if options.infer_previous_versions {
        infer_previous_relations(&mut collection);
    }
    collection.relations.sort_unstable_by(|left, right| {
        left.from_id
            .cmp(&right.from_id)
            .then_with(|| left.to_id.cmp(&right.to_id))
            .then_with(|| format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)))
    });
    collection.updated_at_unix = unix_timestamp();
    collection.validate()?;
    corpus.validate()?;
    collection.save(&options.output_dir)?;
    corpus.save(&options.output_dir)?;

    let families = collection
        .documents
        .iter()
        .map(|document| document.family_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    Ok(CollectionImportReport {
        collection_id: options.collection_id,
        profile: options.profile,
        documents: collection.documents.len(),
        families,
        relations: collection.relations.len(),
        total_bytes: collection.documents.iter().map(|document| document.bytes).sum(),
        skipped_binary,
        skipped_oversized,
        unused_metadata_rows: metadata.into_keys().collect(),
        collection_manifest: options
            .output_dir
            .join(COLLECTION_MANIFEST_FILENAME)
            .display()
            .to_string(),
        corpus_manifest: options
            .output_dir
            .join(crate::MANIFEST_FILENAME)
            .display()
            .to_string(),
    })
}

pub fn verify_collection(root: impl AsRef<Path>) -> Result<CollectionVerificationReport> {
    let root = root.as_ref();
    let collection = CollectionManifest::load(root)?;
    let corpus = CorpusManifest::load(root)?;
    if corpus.provider != CorpusProvider::LocalCollection
        || corpus.corpus_id != collection.collection_id
        || corpus.documents.len() != collection.documents.len()
    {
        return Err(CorpusError::Verification(
            "collection and corpus manifests disagree".to_owned(),
        ));
    }
    for document in &collection.documents {
        let corpus_document = corpus.document(&document.id).ok_or_else(|| {
            CorpusError::Verification(format!(
                "collection document {} is missing from corpus manifest",
                document.id
            ))
        })?;
        if corpus_document.relative_path != document.stored_path
            || corpus_document.sha256 != document.sha256
            || corpus_document.bytes != document.bytes
        {
            return Err(CorpusError::Verification(format!(
                "collection document {} disagrees with corpus manifest",
                document.id
            )));
        }
    }
    let corpus_report = verify_manifest(root)?;
    let families = collection
        .documents
        .iter()
        .map(|document| document.family_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    Ok(CollectionVerificationReport {
        collection_id: collection.collection_id,
        profile: collection.profile,
        documents: collection.documents.len(),
        families,
        relations: collection.relations.len(),
        corpus: corpus_report,
    })
}

fn load_metadata(path: Option<&Path>) -> Result<BTreeMap<String, CollectionMetadataRow>> {
    let Some(path) = path else {
        return Ok(BTreeMap::new());
    };
    let file = File::open(path).map_err(|error| CorpusError::io(path, error))?;
    let mut rows = BTreeMap::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| CorpusError::io(path, error))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let row = serde_json::from_str::<CollectionMetadataRow>(line).map_err(|error| {
            CorpusError::Invalid(format!("{}:{}: {error}", path.display(), line_number + 1))
        })?;
        validate_relative_path(&row.source_path)?;
        let key = row.source_path.replace('\\', "/");
        if rows.insert(key.clone(), row).is_some() {
            return Err(CorpusError::Invalid(format!(
                "duplicate collection metadata row for {key}"
            )));
        }
    }
    Ok(rows)
}

fn collect_files(root: &Path, all_files: bool, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).map_err(|error| CorpusError::io(root, error))? {
        let entry = entry.map_err(|error| CorpusError::io(root, error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| CorpusError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_files(&path, all_files, output)?;
        } else if metadata.is_file() && (all_files || eligible_extension(&path)) {
            output.push(path);
        }
    }
    Ok(())
}

fn eligible_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "txt" | "md" | "markdown" | "html" | "htm" | "json" | "xml" | "csv" | "rtf"
            )
        })
}

fn infer_previous_relations(collection: &mut CollectionManifest) {
    let explicit = collection
        .relations
        .iter()
        .filter(|relation| relation.kind == CollectionRelationKind::PreviousVersion)
        .map(|relation| relation.from_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut families = BTreeMap::<String, Vec<&CollectionDocumentRecord>>::new();
    for document in &collection.documents {
        families
            .entry(document.family_id.clone())
            .or_default()
            .push(document);
    }
    let mut inferred = Vec::new();
    for documents in families.values_mut() {
        documents.sort_unstable_by(|left, right| version_key(left).cmp(&version_key(right)));
        for pair in documents.windows(2) {
            let older = pair[0];
            let newer = pair[1];
            if explicit.contains(newer.id.as_str()) {
                continue;
            }
            inferred.push(CollectionRelation {
                from_id: newer.id.clone(),
                to_id: older.id.clone(),
                kind: CollectionRelationKind::PreviousVersion,
                metadata: BTreeMap::from([("inferred".to_owned(), "true".to_owned())]),
            });
        }
    }
    collection.relations.extend(inferred);
}

fn push_explicit_relations(
    id: &str,
    row: &CollectionMetadataRow,
    output: &mut Vec<CollectionRelation>,
) {
    for (target, kind) in [
        (row.previous_version_id.as_ref(), CollectionRelationKind::PreviousVersion),
        (row.amends_id.as_ref(), CollectionRelationKind::AmendmentOf),
        (row.supersedes_id.as_ref(), CollectionRelationKind::Supersedes),
    ] {
        if let Some(target) = target {
            output.push(CollectionRelation {
                from_id: id.to_owned(),
                to_id: target.clone(),
                kind,
                metadata: BTreeMap::new(),
            });
        }
    }
    for target in &row.related_ids {
        output.push(CollectionRelation {
            from_id: id.to_owned(),
            to_id: target.clone(),
            kind: CollectionRelationKind::RelatedTo,
            metadata: BTreeMap::new(),
        });
    }
}

fn version_key(document: &CollectionDocumentRecord) -> (&str, &str, &str) {
    (
        document.effective_date.as_deref().unwrap_or(""),
        document.version_id.as_str(),
        document.id.as_str(),
    )
}

fn infer_family_id(relative: &Path) -> String {
    let parent = relative
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .map(slash_path)
        .unwrap_or_default();
    let stem = relative
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("document");
    let stem = strip_version_suffix(stem);
    if parent.is_empty() {
        slug(&stem)
    } else {
        format!("{}--{}", slug(&parent), slug(&stem))
    }
}

fn strip_version_suffix(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    for marker in ["__v", "-v", "_v", " version ", " amendment ", " amended "] {
        if let Some(index) = lower.rfind(marker) {
            return value[..index].trim_matches(['-', '_', ' ']).to_owned();
        }
    }
    if let Some(date) = infer_date(value)
        && let Some(index) = value.find(&date)
    {
        return value[..index].trim_matches(['-', '_', ' ']).to_owned();
    }
    value.to_owned()
}

fn infer_date(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    for start in 0..bytes.len() {
        if start + 10 <= bytes.len() {
            let candidate = &value[start..start + 10];
            if valid_date(candidate) {
                return Some(candidate.to_owned());
            }
        }
        if start + 8 <= bytes.len() {
            let candidate = &value[start..start + 8];
            if candidate.bytes().all(|byte| byte.is_ascii_digit()) {
                let normalized = format!(
                    "{}-{}-{}",
                    &candidate[..4],
                    &candidate[4..6],
                    &candidate[6..8]
                );
                if valid_date(&normalized) {
                    return Some(normalized);
                }
            }
        }
    }
    None
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
        && value[5..7].parse::<u8>().is_ok_and(|month| (1..=12).contains(&month))
        && value[8..10].parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(CorpusError::Invalid(format!(
            "unsafe collection relative path {value:?}"
        )));
    }
    Ok(())
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn slug(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !output.is_empty() {
            output.push('-');
            separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "document".to_owned()
    } else {
        output
    }
}

fn humanize(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| CorpusError::io(path, error))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        import_collection, verify_collection, CollectionImportOptions, CollectionMetadataRow,
        CollectionProfile, CollectionRelationKind,
    };

    #[test]
    fn imports_versions_and_infers_previous_relations() {
        let root = temp_root();
        let source = root.join("source");
        let output = root.join("output");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("lease-v1.txt"), "original rent and renewal language")
            .expect("v1");
        fs::write(source.join("lease-v2.txt"), "amended rent and renewal language")
            .expect("v2");
        let metadata = root.join("metadata.jsonl");
        let rows = [
            CollectionMetadataRow {
                source_path: "lease-v1.txt".to_owned(),
                id: Some("lease-2024".to_owned()),
                title: None,
                family_id: Some("store-lease".to_owned()),
                version_id: Some("2024".to_owned()),
                document_type: None,
                effective_date: Some("2024-01-01".to_owned()),
                executed_date: None,
                parties: vec!["Landlord".to_owned(), "Tenant".to_owned()],
                tags: vec!["lease".to_owned()],
                metadata: BTreeMap::new(),
                previous_version_id: None,
                amends_id: None,
                supersedes_id: None,
                related_ids: Vec::new(),
            },
            CollectionMetadataRow {
                source_path: "lease-v2.txt".to_owned(),
                id: Some("lease-2025".to_owned()),
                title: None,
                family_id: Some("store-lease".to_owned()),
                version_id: Some("2025".to_owned()),
                document_type: None,
                effective_date: Some("2025-01-01".to_owned()),
                executed_date: None,
                parties: vec!["Landlord".to_owned(), "Tenant".to_owned()],
                tags: vec!["lease".to_owned()],
                metadata: BTreeMap::new(),
                previous_version_id: None,
                amends_id: None,
                supersedes_id: None,
                related_ids: Vec::new(),
            },
        ];
        fs::write(
            &metadata,
            rows.iter()
                .map(|row| serde_json::to_string(row).expect("json"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("metadata");
        let report = import_collection(CollectionImportOptions {
            source_dir: source,
            output_dir: output.clone(),
            collection_id: "leases".to_owned(),
            profile: CollectionProfile::RetailLease,
            metadata_jsonl: Some(metadata),
            ..CollectionImportOptions::default()
        })
        .expect("import");
        assert_eq!(report.documents, 2);
        assert_eq!(report.families, 1);
        assert_eq!(report.relations, 1);
        let verified = verify_collection(&output).expect("verify");
        assert_eq!(verified.documents, 2);
        let manifest = super::CollectionManifest::load(output).expect("load");
        assert_eq!(manifest.relations[0].kind, CollectionRelationKind::PreviousVersion);
        assert_eq!(manifest.relations[0].from_id, "lease-2025");
        assert_eq!(manifest.relations[0].to_id, "lease-2024");
        fs::remove_dir_all(root).ok();
    }

    fn temp_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "franken-overlap-collection-{}-{nonce}",
            std::process::id()
        ))
    }
}
