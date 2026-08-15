use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{
    FoError, Index, IndexBuilder, IndexConfig, IndexStats, NormalizedText, Result, SearchIntent,
    SearchOptions, SearchResult,
};

const SEGMENTED_FORMAT_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const WRITER_LOCK_FILE: &str = ".writer.lock";
const HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const HASH_PRIME: u64 = 0x0000_0100_0000_01b3;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDocumentInput {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDescriptor {
    pub id: u64,
    pub filename: String,
    pub file_bytes: u64,
    pub content_hash: u64,
    pub stats: IndexStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDocumentRecord {
    pub global_document_id: u64,
    pub path: String,
    pub segment_id: Option<u64>,
    pub local_document_id: Option<u32>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentedManifest {
    pub format_version: u32,
    pub generation: u64,
    pub next_segment_id: u64,
    pub next_document_id: u64,
    pub config: IndexConfig,
    pub segments: Vec<SegmentDescriptor>,
    pub documents: Vec<SegmentDocumentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentAppendReport {
    pub generation: u64,
    pub segment_id: u64,
    pub added_documents: usize,
    pub first_global_document_id: u64,
    pub last_global_document_id: u64,
    pub stats: IndexStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDeleteReport {
    pub generation: u64,
    pub deleted_document_ids: Vec<u64>,
    pub missing_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentCompactionReport {
    pub generation: u64,
    pub active_documents: usize,
    pub deleted_documents: usize,
    pub old_segments: usize,
    pub new_segments: usize,
    pub cleanup_failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentedIndexStats {
    pub generation: u64,
    pub segments: usize,
    pub active_documents: usize,
    pub deleted_documents: usize,
    pub physical_documents: usize,
    pub physical_normalized_tokens: usize,
    pub physical_postings: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentedSearchResult {
    pub global_document_id: u64,
    pub segment_id: u64,
    pub fused_score: f32,
    pub result: SearchResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentVerificationReport {
    pub generation: u64,
    pub verified_segments: usize,
    pub verified_active_documents: usize,
    pub file_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct SegmentedIndex {
    directory: PathBuf,
    manifest: SegmentedManifest,
}

impl SegmentedIndex {
    pub fn create(directory: impl AsRef<Path>, config: IndexConfig) -> Result<Self> {
        config.validate()?;
        let directory = directory.as_ref();
        fs::create_dir_all(directory).map_err(|error| FoError::io(directory, error))?;
        if directory.join(MANIFEST_FILE).exists() {
            return Err(FoError::InvalidConfig(format!(
                "segmented index already exists at {}",
                directory.display()
            )));
        }
        let _lock = WriterLock::acquire(directory)?;
        ensure_empty_directory_except_lock(directory)?;
        let manifest = SegmentedManifest {
            format_version: SEGMENTED_FORMAT_VERSION,
            generation: 0,
            next_segment_id: 0,
            next_document_id: 0,
            config,
            segments: Vec::new(),
            documents: Vec::new(),
        };
        validate_manifest(&manifest)?;
        write_manifest(directory, &manifest)?;
        Ok(Self {
            directory: directory.to_owned(),
            manifest,
        })
    }

    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        let manifest = read_manifest(directory)?;
        validate_manifest(&manifest)?;
        Ok(Self {
            directory: directory.to_owned(),
            manifest,
        })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn manifest(&self) -> &SegmentedManifest {
        &self.manifest
    }

    #[must_use]
    pub fn stats(&self) -> SegmentedIndexStats {
        let active_documents = self
            .manifest
            .documents
            .iter()
            .filter(|document| !document.deleted)
            .count();
        SegmentedIndexStats {
            generation: self.manifest.generation,
            segments: self.manifest.segments.len(),
            active_documents,
            deleted_documents: self.manifest.documents.len() - active_documents,
            physical_documents: self
                .manifest
                .segments
                .iter()
                .map(|segment| segment.stats.documents)
                .sum(),
            physical_normalized_tokens: self
                .manifest
                .segments
                .iter()
                .map(|segment| segment.stats.normalized_tokens)
                .sum(),
            physical_postings: self
                .manifest
                .segments
                .iter()
                .map(|segment| segment.stats.postings)
                .sum(),
        }
    }

    pub fn append_documents(
        &mut self,
        documents: Vec<SegmentDocumentInput>,
    ) -> Result<SegmentAppendReport> {
        if documents.is_empty() {
            return Err(FoError::InvalidConfig(
                "cannot append an empty document batch".to_owned(),
            ));
        }
        let _lock = WriterLock::acquire(&self.directory)?;
        self.ensure_current()?;
        validate_append_inputs(&self.manifest, &documents)?;

        let segment_id = self.manifest.next_segment_id;
        let filename = segment_filename(segment_id);
        let segment_path = self.directory.join(&filename);
        let mut builder = IndexBuilder::new(self.manifest.config.clone())?;
        for document in &documents {
            builder.add_document(document.path.clone(), &document.contents)?;
        }
        let index = builder.build()?;
        index.save(&segment_path)?;
        let descriptor = describe_segment(segment_id, filename, &segment_path, index.stats())?;

        let mut next = self.manifest.clone();
        let first_global_document_id = next.next_document_id;
        for (local_document_id, document) in documents.iter().enumerate() {
            let local_document_id = u32::try_from(local_document_id).map_err(|_| {
                FoError::InvalidConfig("segment contains more than u32::MAX documents".to_owned())
            })?;
            let global_document_id = next.next_document_id;
            next.next_document_id = next.next_document_id.checked_add(1).ok_or_else(|| {
                FoError::InvalidConfig("global document id space is exhausted".to_owned())
            })?;
            next.documents.push(SegmentDocumentRecord {
                global_document_id,
                path: document.path.clone(),
                segment_id: Some(segment_id),
                local_document_id: Some(local_document_id),
                deleted: false,
            });
        }
        next.next_segment_id = next.next_segment_id.checked_add(1).ok_or_else(|| {
            FoError::InvalidConfig("segment id space is exhausted".to_owned())
        })?;
        next.generation = next.generation.checked_add(1).ok_or_else(|| {
            FoError::InvalidConfig("manifest generation is exhausted".to_owned())
        })?;
        next.segments.push(descriptor.clone());
        validate_manifest(&next)?;
        if let Err(error) = write_manifest(&self.directory, &next) {
            let _ = fs::remove_file(&segment_path);
            return Err(error);
        }
        self.manifest = next;

        Ok(SegmentAppendReport {
            generation: self.manifest.generation,
            segment_id,
            added_documents: documents.len(),
            first_global_document_id,
            last_global_document_id: self.manifest.next_document_id - 1,
            stats: descriptor.stats,
        })
    }

    pub fn delete_paths(&mut self, paths: &[String]) -> Result<SegmentDeleteReport> {
        if paths.is_empty() {
            return Err(FoError::InvalidConfig(
                "at least one path is required for deletion".to_owned(),
            ));
        }
        let _lock = WriterLock::acquire(&self.directory)?;
        self.ensure_current()?;
        let requested = paths.iter().cloned().collect::<BTreeSet<_>>();
        let mut found = BTreeSet::new();
        let mut deleted_document_ids = Vec::new();
        let mut next = self.manifest.clone();
        for document in &mut next.documents {
            if document.deleted || !requested.contains(&document.path) {
                continue;
            }
            document.deleted = true;
            document.segment_id = None;
            document.local_document_id = None;
            found.insert(document.path.clone());
            deleted_document_ids.push(document.global_document_id);
        }
        deleted_document_ids.sort_unstable();
        let missing_paths = requested.difference(&found).cloned().collect::<Vec<_>>();
        if !deleted_document_ids.is_empty() {
            next.generation = next.generation.checked_add(1).ok_or_else(|| {
                FoError::InvalidConfig("manifest generation is exhausted".to_owned())
            })?;
            validate_manifest(&next)?;
            write_manifest(&self.directory, &next)?;
            self.manifest = next;
        }
        Ok(SegmentDeleteReport {
            generation: self.manifest.generation,
            deleted_document_ids,
            missing_paths,
        })
    }

    pub fn search(
        &self,
        specimen: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SegmentedSearchResult>> {
        options.validate()?;
        validate_manifest(&self.manifest)?;
        let active = active_documents_by_segment(&self.manifest);
        let mut results = Vec::new();
        let mut segment_options = options.clone();
        segment_options.max_results = options.max_candidates.max(options.max_results);

        for descriptor in &self.manifest.segments {
            let Some(documents) = active.get(&descriptor.id) else {
                continue;
            };
            let index = Index::load(self.directory.join(&descriptor.filename))?;
            if index.config != self.manifest.config {
                return Err(FoError::InvalidIndex(format!(
                    "segment {} uses a different index configuration",
                    descriptor.id
                )));
            }
            for result in index.search(specimen, &segment_options)? {
                let Some(record) = documents.get(&result.document_id) else {
                    continue;
                };
                if record.path != result.path {
                    return Err(FoError::InvalidIndex(format!(
                        "segment {} local document {} path disagrees with the manifest",
                        descriptor.id, result.document_id
                    )));
                }
                results.push(SegmentedSearchResult {
                    global_document_id: record.global_document_id,
                    segment_id: descriptor.id,
                    fused_score: stable_cross_segment_score(&result, options.intent),
                    result,
                });
            }
        }

        results.sort_unstable_by(|left, right| {
            right
                .fused_score
                .total_cmp(&left.fused_score)
                .then_with(|| {
                    right
                        .result
                        .combined_score
                        .total_cmp(&left.result.combined_score)
                })
                .then_with(|| left.global_document_id.cmp(&right.global_document_id))
                .then_with(|| left.result.corpus_start.cmp(&right.result.corpus_start))
        });
        results.truncate(options.max_results);
        Ok(results)
    }

    pub fn compact(&mut self) -> Result<SegmentCompactionReport> {
        let _lock = WriterLock::acquire(&self.directory)?;
        self.ensure_current()?;
        let old_descriptors = self.manifest.segments.clone();
        let mut active_documents = load_active_documents(&self.directory, &self.manifest)?;
        active_documents.sort_unstable_by_key(|document| document.global_document_id);

        let mut next = self.manifest.clone();
        let mut new_segment_path = None;
        let new_descriptor = if active_documents.is_empty() {
            None
        } else {
            let segment_id = next.next_segment_id;
            let filename = segment_filename(segment_id);
            let path = self.directory.join(&filename);
            let mut builder = IndexBuilder::new(next.config.clone())?;
            for document in &active_documents {
                builder.add_normalized_document(
                    document.path.clone(),
                    document.normalized.clone(),
                )?;
            }
            let index = builder.build()?;
            index.save(&path)?;
            let descriptor = describe_segment(segment_id, filename, &path, index.stats())?;
            new_segment_path = Some(path);
            next.next_segment_id = next.next_segment_id.checked_add(1).ok_or_else(|| {
                FoError::InvalidConfig("segment id space is exhausted".to_owned())
            })?;
            Some(descriptor)
        };

        let location_by_global = active_documents
            .iter()
            .enumerate()
            .map(|(local, document)| {
                let local = u32::try_from(local).map_err(|_| {
                    FoError::InvalidConfig(
                        "compacted segment contains more than u32::MAX documents".to_owned(),
                    )
                })?;
                Ok((document.global_document_id, local))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        for document in &mut next.documents {
            if document.deleted {
                document.segment_id = None;
                document.local_document_id = None;
                continue;
            }
            let local = location_by_global
                .get(&document.global_document_id)
                .copied()
                .ok_or_else(|| {
                    FoError::InvalidIndex(format!(
                        "active document {} disappeared during compaction",
                        document.global_document_id
                    ))
                })?;
            document.segment_id = new_descriptor.as_ref().map(|segment| segment.id);
            document.local_document_id = Some(local);
        }
        next.segments = new_descriptor.iter().cloned().collect();
        next.generation = next.generation.checked_add(1).ok_or_else(|| {
            FoError::InvalidConfig("manifest generation is exhausted".to_owned())
        })?;
        validate_manifest(&next)?;
        if let Err(error) = write_manifest(&self.directory, &next) {
            if let Some(path) = new_segment_path {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        self.manifest = next;

        let retained = new_descriptor.as_ref().map(|segment| segment.filename.as_str());
        let mut cleanup_failures = Vec::new();
        for descriptor in &old_descriptors {
            if Some(descriptor.filename.as_str()) == retained {
                continue;
            }
            let path = self.directory.join(&descriptor.filename);
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                cleanup_failures.push(format!("{}: {error}", path.display()));
            }
        }

        Ok(SegmentCompactionReport {
            generation: self.manifest.generation,
            active_documents: active_documents.len(),
            deleted_documents: self
                .manifest
                .documents
                .iter()
                .filter(|document| document.deleted)
                .count(),
            old_segments: old_descriptors.len(),
            new_segments: self.manifest.segments.len(),
            cleanup_failures,
        })
    }

    pub fn verify_storage(&self) -> Result<SegmentVerificationReport> {
        validate_manifest(&self.manifest)?;
        let active = active_documents_by_segment(&self.manifest);
        let mut verified_active_documents = 0usize;
        let mut file_bytes = 0u64;
        for descriptor in &self.manifest.segments {
            let path = self.directory.join(&descriptor.filename);
            let metadata = fs::metadata(&path).map_err(|error| FoError::io(&path, error))?;
            if metadata.len() != descriptor.file_bytes {
                return Err(FoError::InvalidIndex(format!(
                    "segment {} byte length changed from {} to {}",
                    descriptor.id,
                    descriptor.file_bytes,
                    metadata.len()
                )));
            }
            if hash_file(&path)? != descriptor.content_hash {
                return Err(FoError::InvalidIndex(format!(
                    "segment {} content hash mismatch",
                    descriptor.id
                )));
            }
            let index = Index::load(&path)?;
            if index.config != self.manifest.config || index.stats() != descriptor.stats {
                return Err(FoError::InvalidIndex(format!(
                    "segment {} metadata disagrees with the manifest",
                    descriptor.id
                )));
            }
            if let Some(documents) = active.get(&descriptor.id) {
                for (&local_document_id, record) in documents {
                    let document = index.document(local_document_id).ok_or_else(|| {
                        FoError::InvalidIndex(format!(
                            "segment {} is missing local document {}",
                            descriptor.id, local_document_id
                        ))
                    })?;
                    if document.path != record.path {
                        return Err(FoError::InvalidIndex(format!(
                            "segment {} local document {} path mismatch",
                            descriptor.id, local_document_id
                        )));
                    }
                    verified_active_documents = verified_active_documents.saturating_add(1);
                }
            }
            file_bytes = file_bytes.saturating_add(metadata.len());
        }
        Ok(SegmentVerificationReport {
            generation: self.manifest.generation,
            verified_segments: self.manifest.segments.len(),
            verified_active_documents,
            file_bytes,
        })
    }

    fn ensure_current(&self) -> Result<()> {
        let current = read_manifest(&self.directory)?;
        validate_manifest(&current)?;
        if current.generation != self.manifest.generation {
            return Err(FoError::InvalidConfig(format!(
                "segmented index generation changed from {} to {}; reopen before writing",
                self.manifest.generation, current.generation
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct LoadedDocument {
    global_document_id: u64,
    path: String,
    normalized: NormalizedText,
}

fn load_active_documents(
    directory: &Path,
    manifest: &SegmentedManifest,
) -> Result<Vec<LoadedDocument>> {
    let grouped = active_documents_by_segment(manifest);
    let mut documents = Vec::new();
    for descriptor in &manifest.segments {
        let Some(records) = grouped.get(&descriptor.id) else {
            continue;
        };
        let index = Index::load(directory.join(&descriptor.filename))?;
        if index.config != manifest.config {
            return Err(FoError::InvalidIndex(format!(
                "segment {} uses a different index configuration",
                descriptor.id
            )));
        }
        for (&local_document_id, record) in records {
            let document = index.document(local_document_id).ok_or_else(|| {
                FoError::InvalidIndex(format!(
                    "segment {} is missing local document {}",
                    descriptor.id, local_document_id
                ))
            })?;
            if document.path != record.path {
                return Err(FoError::InvalidIndex(format!(
                    "segment {} local document {} path mismatch",
                    descriptor.id, local_document_id
                )));
            }
            documents.push(LoadedDocument {
                global_document_id: record.global_document_id,
                path: record.path.clone(),
                normalized: document.normalized.clone(),
            });
        }
    }
    Ok(documents)
}

fn active_documents_by_segment(
    manifest: &SegmentedManifest,
) -> BTreeMap<u64, BTreeMap<u32, &SegmentDocumentRecord>> {
    let mut grouped = BTreeMap::<u64, BTreeMap<u32, &SegmentDocumentRecord>>::new();
    for document in &manifest.documents {
        if document.deleted {
            continue;
        }
        if let (Some(segment_id), Some(local_document_id)) =
            (document.segment_id, document.local_document_id)
        {
            grouped
                .entry(segment_id)
                .or_default()
                .insert(local_document_id, document);
        }
    }
    grouped
}

fn stable_cross_segment_score(result: &SearchResult, intent: SearchIntent) -> f32 {
    let length = 1.0 - (-(result.matched_tokens as f32) / 32.0).exp();
    let false_match_confidence =
        (1.0 / (1.0 + result.estimated_false_matches.max(0.0))) as f32;
    let evidence = (0.55 * result.edit_similarity
        + 0.16 * result.chain_consistency
        + 0.12 * length
        + 0.09 * result.anchor_coverage
        + 0.08 * false_match_confidence)
        .clamp(0.0, 1.0);
    let coverage = match intent {
        SearchIntent::AnyPassage => 1.0,
        SearchIntent::SourceAttribution => result.query_coverage.sqrt(),
        SearchIntent::NearDuplicate => {
            let denominator = result.query_coverage + result.source_coverage;
            if denominator > 0.0 {
                (2.0 * result.query_coverage * result.source_coverage / denominator).sqrt()
            } else {
                0.0
            }
        }
    };
    (0.35 * result.combined_score + 0.65 * evidence * coverage).clamp(0.0, 1.0)
}

fn describe_segment(
    id: u64,
    filename: String,
    path: &Path,
    stats: IndexStats,
) -> Result<SegmentDescriptor> {
    let file_bytes = fs::metadata(path)
        .map_err(|error| FoError::io(path, error))?
        .len();
    Ok(SegmentDescriptor {
        id,
        filename,
        file_bytes,
        content_hash: hash_file(path)?,
        stats,
    })
}

fn validate_append_inputs(
    manifest: &SegmentedManifest,
    documents: &[SegmentDocumentInput],
) -> Result<()> {
    let active_paths = manifest
        .documents
        .iter()
        .filter(|document| !document.deleted)
        .map(|document| document.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut batch_paths = BTreeSet::new();
    for document in documents {
        validate_document_path(&document.path)?;
        if document.contents.is_empty() {
            return Err(FoError::InvalidConfig(format!(
                "document {} is empty",
                document.path
            )));
        }
        if active_paths.contains(document.path.as_str()) {
            return Err(FoError::InvalidConfig(format!(
                "active document path {} already exists",
                document.path
            )));
        }
        if !batch_paths.insert(document.path.as_str()) {
            return Err(FoError::InvalidConfig(format!(
                "document path {} appears more than once in the append batch",
                document.path
            )));
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &SegmentedManifest) -> Result<()> {
    if manifest.format_version != SEGMENTED_FORMAT_VERSION {
        return Err(FoError::InvalidIndex(format!(
            "unsupported segmented-index format version {}",
            manifest.format_version
        )));
    }
    manifest.config.validate()?;
    let mut segment_ids = BTreeSet::new();
    for segment in &manifest.segments {
        if !segment_ids.insert(segment.id) {
            return Err(FoError::InvalidIndex(format!(
                "duplicate segment id {}",
                segment.id
            )));
        }
        validate_segment_filename(&segment.filename)?;
        if segment.stats.documents == 0 || segment.file_bytes == 0 {
            return Err(FoError::InvalidIndex(format!(
                "segment {} has impossible empty metadata",
                segment.id
            )));
        }
    }
    if manifest
        .segments
        .iter()
        .any(|segment| segment.id >= manifest.next_segment_id)
    {
        return Err(FoError::InvalidIndex(
            "next_segment_id does not exceed every segment id".to_owned(),
        ));
    }

    let mut global_ids = BTreeSet::new();
    let mut active_paths = BTreeSet::new();
    let mut local_locations = BTreeSet::new();
    for document in &manifest.documents {
        validate_document_path(&document.path)?;
        if !global_ids.insert(document.global_document_id) {
            return Err(FoError::InvalidIndex(format!(
                "duplicate global document id {}",
                document.global_document_id
            )));
        }
        if document.global_document_id >= manifest.next_document_id {
            return Err(FoError::InvalidIndex(
                "next_document_id does not exceed every document id".to_owned(),
            ));
        }
        if document.deleted {
            if document.segment_id.is_some() || document.local_document_id.is_some() {
                return Err(FoError::InvalidIndex(format!(
                    "deleted document {} retains a physical location",
                    document.global_document_id
                )));
            }
            continue;
        }
        if !active_paths.insert(document.path.as_str()) {
            return Err(FoError::InvalidIndex(format!(
                "active path {} appears more than once",
                document.path
            )));
        }
        let segment_id = document.segment_id.ok_or_else(|| {
            FoError::InvalidIndex(format!(
                "active document {} has no segment id",
                document.global_document_id
            ))
        })?;
        let local_document_id = document.local_document_id.ok_or_else(|| {
            FoError::InvalidIndex(format!(
                "active document {} has no local document id",
                document.global_document_id
            ))
        })?;
        let descriptor = manifest
            .segments
            .iter()
            .find(|segment| segment.id == segment_id)
            .ok_or_else(|| {
                FoError::InvalidIndex(format!(
                    "active document {} references missing segment {}",
                    document.global_document_id, segment_id
                ))
            })?;
        if local_document_id as usize >= descriptor.stats.documents {
            return Err(FoError::InvalidIndex(format!(
                "active document {} local id {} exceeds segment {} document count",
                document.global_document_id, local_document_id, segment_id
            )));
        }
        if !local_locations.insert((segment_id, local_document_id)) {
            return Err(FoError::InvalidIndex(format!(
                "segment {} local document {} has multiple active mappings",
                segment_id, local_document_id
            )));
        }
    }
    Ok(())
}

fn validate_document_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.len() > 1_048_576 {
        return Err(FoError::InvalidConfig(
            "document paths must contain between 1 and 1,048,576 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_segment_filename(filename: &str) -> Result<()> {
    let path = Path::new(filename);
    let mut components = path.components();
    let one_normal = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none();
    if !one_normal || !filename.ends_with(".foidx") {
        return Err(FoError::InvalidIndex(format!(
            "unsafe segment filename {filename:?}"
        )));
    }
    Ok(())
}

fn ensure_empty_directory_except_lock(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|error| FoError::io(directory, error))? {
        let entry = entry.map_err(|error| FoError::io(directory, error))?;
        if entry.file_name() != WRITER_LOCK_FILE {
            return Err(FoError::InvalidConfig(format!(
                "cannot create segmented index in nonempty directory {}",
                directory.display()
            )));
        }
    }
    Ok(())
}

fn segment_filename(segment_id: u64) -> String {
    format!("segment-{segment_id:016x}.foidx")
}

fn read_manifest(directory: &Path) -> Result<SegmentedManifest> {
    let path = directory.join(MANIFEST_FILE);
    let bytes = fs::read(&path).map_err(|error| FoError::io(&path, error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| FoError::InvalidIndex(format!("invalid segmented manifest: {error}")))
}

fn write_manifest(directory: &Path, manifest: &SegmentedManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
        FoError::InvalidConfig(format!("could not serialize segmented manifest: {error}"))
    })?;
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        ".manifest.tmp-{}-{sequence}",
        std::process::id()
    ));
    let destination = directory.join(MANIFEST_FILE);
    let file = File::create(&temporary).map_err(|error| FoError::io(&temporary, error))?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(&bytes)
        .and_then(|()| writer.flush())
        .map_err(|error| FoError::io(&temporary, error))?;
    let file = writer
        .into_inner()
        .map_err(|error| FoError::io(&temporary, error.into_error()))?;
    file.sync_all()
        .map_err(|error| FoError::io(&temporary, error))?;
    replace_file(&temporary, &destination)?;
    if let Ok(directory_file) = File::open(directory) {
        let _ = directory_file.sync_all();
    }
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error)
            if destination.exists()
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                ) =>
        {
            fs::remove_file(destination).map_err(|error| FoError::io(destination, error))?;
            fs::rename(source, destination).map_err(|error| FoError::io(destination, error))
        }
        Err(error) => Err(FoError::io(destination, error)),
    }
}

fn hash_file(path: &Path) -> Result<u64> {
    let file = File::open(path).map_err(|error| FoError::io(path, error))?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0u8; 64 * 1024];
    let mut hash = HASH_OFFSET;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| FoError::io(path, error))?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(HASH_PRIME);
        }
    }
    Ok(hash)
}

struct WriterLock {
    path: PathBuf,
}

impl WriterLock {
    fn acquire(directory: &Path) -> Result<Self> {
        let path = directory.join(WRITER_LOCK_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                FoError::InvalidConfig(format!(
                    "could not acquire segmented-index writer lock {}: {error}",
                    path.display()
                ))
            })?;
        writeln!(file, "pid={}", std::process::id())
            .map_err(|error| FoError::io(&path, error))?;
        file.sync_all().map_err(|error| FoError::io(&path, error))?;
        Ok(Self { path })
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SegmentDocumentInput, SegmentedIndex};
    use crate::{IndexConfig, SearchOptions};

    #[test]
    fn append_search_delete_and_compact_preserve_global_identity() {
        let root = temporary_root();
        let mut index = SegmentedIndex::create(&root, IndexConfig::default()).expect("create");
        let first = index
            .append_documents(vec![document(
                "first.txt",
                "the observatory released every raw measurement before interpretation",
            )])
            .expect("append first");
        let second = index
            .append_documents(vec![document(
                "second.txt",
                "winter vegetables and railway lanterns fill the cabinet",
            )])
            .expect("append second");
        assert_eq!(first.first_global_document_id, 0);
        assert_eq!(second.first_global_document_id, 1);

        let hits = index
            .search(
                "released every raw measurement before interpretation",
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(hits[0].global_document_id, 0);

        index
            .delete_paths(&["second.txt".to_owned()])
            .expect("delete");
        assert_eq!(index.stats().segments, 2);
        assert_eq!(index.stats().deleted_documents, 1);
        let compacted = index.compact().expect("compact");
        assert_eq!(compacted.active_documents, 1);
        assert_eq!(index.stats().segments, 1);
        assert_eq!(
            index
                .verify_storage()
                .expect("verify")
                .verified_active_documents,
            1
        );

        let hits = index
            .search(
                "released every raw measurement before interpretation",
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
            )
            .expect("search after compact");
        assert_eq!(hits[0].global_document_id, 0);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn active_paths_must_be_unique() {
        let root = temporary_root();
        let mut index = SegmentedIndex::create(&root, IndexConfig::default()).expect("create");
        index
            .append_documents(vec![document("same.txt", "first document contents")])
            .expect("append");
        let error = index
            .append_documents(vec![document("same.txt", "replacement contents")])
            .expect_err("duplicate active path");
        assert!(error.to_string().contains("already exists"));
        fs::remove_dir_all(root).ok();
    }

    fn document(path: &str, contents: &str) -> SegmentDocumentInput {
        SegmentDocumentInput {
            path: path.to_owned(),
            contents: contents.to_owned(),
        }
    }

    fn temporary_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "franken-overlap-segmented-{}-{nonce}",
            std::process::id()
        ))
    }
}
