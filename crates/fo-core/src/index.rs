use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::{
    Feature, Fingerprint, FoError, IndexConfig, IndexStats, NormalizationProfile, NormalizedText,
    Posting, PunctuationMode, Result, normalize, qgram_hashes, winnow,
};

const MAGIC: &[u8; 8] = b"FROV0001";
const FORMAT_VERSION: u32 = 1;
const MAX_STRING_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_DOCUMENTS: u32 = 100_000_000;
const MAX_ENTRIES: u64 = 2_000_000_000;
const MAX_POSTINGS_PER_ENTRY: u32 = 2_000_000_000;

#[derive(Debug, Clone)]
pub struct Document {
    pub id: u32,
    pub path: String,
    pub normalized: NormalizedText,
}

impl Document {
    #[must_use]
    pub fn token_count(&self) -> usize {
        self.normalized.tokens.len()
    }
}

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub fingerprint: Fingerprint,
    pub document_frequency: u32,
    pub postings: Vec<Posting>,
}

impl IndexEntry {
    #[must_use]
    pub fn postings_for_document(&self, document_id: u32) -> &[Posting] {
        let start = self
            .postings
            .partition_point(|posting| posting.document_id < document_id);
        let end = self
            .postings
            .partition_point(|posting| posting.document_id <= document_id);
        &self.postings[start..end]
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub config: IndexConfig,
    pub(crate) documents: Vec<Document>,
    pub(crate) entries: Vec<IndexEntry>,
}

impl Index {
    #[must_use]
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    #[must_use]
    pub fn document(&self, document_id: u32) -> Option<&Document> {
        usize::try_from(document_id)
            .ok()
            .and_then(|index| self.documents.get(index))
    }

    #[must_use]
    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    #[must_use]
    pub fn lookup(&self, fingerprint: Fingerprint) -> Option<&IndexEntry> {
        self.entries
            .binary_search_by_key(&fingerprint, |entry| entry.fingerprint)
            .ok()
            .map(|index| &self.entries[index])
    }

    #[must_use]
    pub fn stats(&self) -> IndexStats {
        IndexStats {
            documents: self.documents.len(),
            normalized_tokens: self.documents.iter().map(Document::token_count).sum(),
            distinct_fingerprints: self.entries.len(),
            postings: self.entries.iter().map(|entry| entry.postings.len()).sum(),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| FoError::io(parent, error))?;
        }
        let temporary = temporary_path(path);
        let file = File::create(&temporary).map_err(|error| FoError::io(&temporary, error))?;
        let mut writer = BufWriter::new(file);

        writer
            .write_all(MAGIC)
            .and_then(|()| writer.write_all(&FORMAT_VERSION.to_le_bytes()))
            .map_err(|error| FoError::io(&temporary, error))?;
        write_u32(&mut writer, checked_u32(self.config.qgram_size, "q-gram size")?)
            .map_err(|error| FoError::io(&temporary, error))?;
        write_u32(
            &mut writer,
            checked_u32(self.config.winnow_window, "winnow window")?,
        )
        .map_err(|error| FoError::io(&temporary, error))?;

        let mut flags = 0u32;
        if self.config.normalization.nfkc {
            flags |= 1;
        }
        if self.config.normalization.lowercase {
            flags |= 1 << 1;
        }
        if self.config.normalization.collapse_whitespace {
            flags |= 1 << 2;
        }
        write_u32(&mut writer, flags).map_err(|error| FoError::io(&temporary, error))?;
        writer
            .write_all(&[
                self.config.normalization.punctuation.as_u8(),
                0,
                0,
                0,
            ])
            .map_err(|error| FoError::io(&temporary, error))?;

        write_u32(
            &mut writer,
            checked_u32(self.documents.len(), "document count")?,
        )
        .map_err(|error| FoError::io(&temporary, error))?;
        let entry_count = u64::try_from(self.entries.len())
            .map_err(|_| FoError::InvalidConfig("entry count exceeds u64".to_owned()))?;
        write_u64(&mut writer, entry_count)
            .map_err(|error| FoError::io(&temporary, error))?;

        for document in &self.documents {
            write_u32(&mut writer, document.id)
                .and_then(|()| write_string(&mut writer, &document.path))
                .and_then(|()| write_string(&mut writer, &document.normalized.text))
                .map_err(|error| FoError::io(&temporary, error))?;
        }
        for entry in &self.entries {
            write_u64(&mut writer, entry.fingerprint.hi)
                .and_then(|()| write_u64(&mut writer, entry.fingerprint.lo))
                .and_then(|()| write_u32(&mut writer, entry.document_frequency))
                .map_err(|error| FoError::io(&temporary, error))?;
            write_u32(
                &mut writer,
                checked_u32(entry.postings.len(), "posting count")?,
            )
            .map_err(|error| FoError::io(&temporary, error))?;
            for posting in &entry.postings {
                write_u32(&mut writer, posting.document_id)
                    .and_then(|()| write_u32(&mut writer, posting.position))
                    .map_err(|error| FoError::io(&temporary, error))?;
            }
        }

        writer.flush().map_err(|error| FoError::io(&temporary, error))?;
        let file = writer
            .into_inner()
            .map_err(|error| FoError::io(&temporary, error.into_error()))?;
        file.sync_all()
            .map_err(|error| FoError::io(&temporary, error))?;
        if path.exists() {
            fs::remove_file(path).map_err(|error| FoError::io(path, error))?;
        }
        fs::rename(&temporary, path).map_err(|error| FoError::io(path, error))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|error| FoError::io(path, error))?;
        let file_len = file
            .metadata()
            .map_err(|error| FoError::io(path, error))?
            .len();
        let mut reader = BufReader::new(file);
        let mut magic = [0u8; 8];
        reader
            .read_exact(&mut magic)
            .map_err(|error| FoError::io(path, error))?;
        if &magic != MAGIC {
            return Err(FoError::InvalidIndex("bad magic bytes".to_owned()));
        }
        let version = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
        if version != FORMAT_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "format version {version} is unsupported"
            )));
        }
        let qgram_size = read_u32(&mut reader).map_err(|error| FoError::io(path, error))? as usize;
        let winnow_window =
            read_u32(&mut reader).map_err(|error| FoError::io(path, error))? as usize;
        let flags = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
        if flags & !0b111 != 0 {
            return Err(FoError::InvalidIndex(format!(
                "unknown normalization flag bits {:#x}",
                flags & !0b111
            )));
        }
        let mut punctuation_and_reserved = [0u8; 4];
        reader
            .read_exact(&mut punctuation_and_reserved)
            .map_err(|error| FoError::io(path, error))?;
        if punctuation_and_reserved[1..].iter().any(|&byte| byte != 0) {
            return Err(FoError::InvalidIndex(
                "nonzero reserved header bytes".to_owned(),
            ));
        }
        let config = IndexConfig {
            normalization: NormalizationProfile {
                nfkc: flags & 1 != 0,
                lowercase: flags & 2 != 0,
                collapse_whitespace: flags & 4 != 0,
                punctuation: PunctuationMode::from_u8(punctuation_and_reserved[0])?,
            },
            qgram_size,
            winnow_window,
        };
        config.validate()?;

        let document_count = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
        if document_count > MAX_DOCUMENTS {
            return Err(FoError::InvalidIndex(format!(
                "document count {document_count} exceeds safety limit {MAX_DOCUMENTS}"
            )));
        }
        let entry_count_u64 = read_u64(&mut reader).map_err(|error| FoError::io(path, error))?;
        if entry_count_u64 > MAX_ENTRIES || entry_count_u64 > file_len {
            return Err(FoError::InvalidIndex(format!(
                "entry count {entry_count_u64} exceeds a safe bound"
            )));
        }
        let entry_count = usize::try_from(entry_count_u64)
            .map_err(|_| FoError::InvalidIndex("entry count does not fit this platform".to_owned()))?;

        let mut documents = Vec::with_capacity(document_count as usize);
        for expected_id in 0..document_count {
            let id = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
            if id != expected_id {
                return Err(FoError::InvalidIndex(format!(
                    "document id {id} appears where {expected_id} was expected"
                )));
            }
            let document_path = read_string(&mut reader, file_len)
                .map_err(|error| FoError::io(path, error))?;
            let normalized_text = read_string(&mut reader, file_len)
                .map_err(|error| FoError::io(path, error))?;
            let normalized = NormalizedText::from_stored(normalized_text);
            if normalized.tokens.len() > u32::MAX as usize {
                return Err(FoError::InvalidIndex(format!(
                    "document {id} exceeds the u32 position limit"
                )));
            }
            documents.push(Document {
                id,
                path: document_path,
                normalized,
            });
        }

        let mut entries = Vec::with_capacity(entry_count);
        let mut previous_fingerprint = None;
        for _ in 0..entry_count {
            let fingerprint = Fingerprint {
                hi: read_u64(&mut reader).map_err(|error| FoError::io(path, error))?,
                lo: read_u64(&mut reader).map_err(|error| FoError::io(path, error))?,
            };
            if previous_fingerprint.is_some_and(|previous| previous >= fingerprint) {
                return Err(FoError::InvalidIndex(
                    "fingerprint dictionary is not strictly sorted".to_owned(),
                ));
            }
            previous_fingerprint = Some(fingerprint);
            let document_frequency =
                read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
            let posting_count_u32 =
                read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
            if posting_count_u32 > MAX_POSTINGS_PER_ENTRY
                || u64::from(posting_count_u32).saturating_mul(8) > file_len
            {
                return Err(FoError::InvalidIndex(format!(
                    "posting count {posting_count_u32} exceeds a safe bound"
                )));
            }
            let posting_count = posting_count_u32 as usize;
            let mut postings = Vec::with_capacity(posting_count);
            let mut previous_posting = None;
            let mut observed_document_frequency = 0u32;
            let mut last_document = None;
            for _ in 0..posting_count {
                let posting = Posting {
                    document_id: read_u32(&mut reader)
                        .map_err(|error| FoError::io(path, error))?,
                    position: read_u32(&mut reader)
                        .map_err(|error| FoError::io(path, error))?,
                };
                let Some(document) = documents.get(posting.document_id as usize) else {
                    return Err(FoError::InvalidIndex(format!(
                        "posting references missing document {}",
                        posting.document_id
                    )));
                };
                if posting.position as usize >= document.normalized.tokens.len() {
                    return Err(FoError::InvalidIndex(format!(
                        "posting position {} is outside document {}",
                        posting.position, posting.document_id
                    )));
                }
                if previous_posting.is_some_and(|previous| previous >= posting) {
                    return Err(FoError::InvalidIndex(
                        "posting list is not strictly sorted".to_owned(),
                    ));
                }
                previous_posting = Some(posting);
                if last_document != Some(posting.document_id) {
                    observed_document_frequency = observed_document_frequency.saturating_add(1);
                    last_document = Some(posting.document_id);
                }
                postings.push(posting);
            }
            if observed_document_frequency != document_frequency {
                return Err(FoError::InvalidIndex(format!(
                    "document frequency {document_frequency} disagrees with observed {observed_document_frequency}"
                )));
            }
            entries.push(IndexEntry {
                fingerprint,
                document_frequency,
                postings,
            });
        }

        let mut trailing = [0u8; 1];
        match reader.read(&mut trailing) {
            Ok(0) => {}
            Ok(_) => {
                return Err(FoError::InvalidIndex(
                    "trailing bytes after the final posting list".to_owned(),
                ));
            }
            Err(error) => return Err(FoError::io(path, error)),
        }
        Ok(Self {
            config,
            documents,
            entries,
        })
    }
}

#[derive(Debug)]
pub struct IndexBuilder {
    config: IndexConfig,
    documents: Vec<Document>,
}

impl IndexBuilder {
    pub fn new(config: IndexConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            documents: Vec::new(),
        })
    }

    pub fn add_document(&mut self, path: impl Into<String>, contents: &str) -> Result<u32> {
        let path = path.into();
        let normalized = normalize(contents, &self.config.normalization);
        self.add_normalized_document(path, normalized)
    }

    pub fn add_normalized_document(
        &mut self,
        path: impl Into<String>,
        normalized: NormalizedText,
    ) -> Result<u32> {
        if self.documents.len() >= u32::MAX as usize {
            return Err(FoError::TooManyDocuments);
        }
        let path = path.into();
        if normalized.tokens.len() > u32::MAX as usize {
            return Err(FoError::DocumentTooLarge {
                path,
                tokens: normalized.tokens.len(),
            });
        }
        let id = self.documents.len() as u32;
        self.documents.push(Document {
            id,
            path,
            normalized,
        });
        Ok(id)
    }

    pub fn build(self) -> Result<Index> {
        let mut posting_map = HashMap::<Fingerprint, Vec<Posting>>::new();
        for document in &self.documents {
            let hashes = qgram_hashes(&document.normalized.tokens, self.config.qgram_size)?;
            for Feature {
                fingerprint,
                position,
            } in winnow(&hashes, self.config.winnow_window)
            {
                posting_map
                    .entry(fingerprint)
                    .or_default()
                    .push(Posting {
                        document_id: document.id,
                        position,
                    });
            }
        }
        let mut entries = Vec::with_capacity(posting_map.len());
        for (fingerprint, mut postings) in posting_map {
            postings.sort_unstable();
            postings.dedup();
            let mut last_document = None;
            let mut document_frequency = 0u32;
            for posting in &postings {
                if last_document != Some(posting.document_id) {
                    document_frequency = document_frequency.saturating_add(1);
                    last_document = Some(posting.document_id);
                }
            }
            entries.push(IndexEntry {
                fingerprint,
                document_frequency,
                postings,
            });
        }
        entries.sort_unstable_by_key(|entry| entry.fingerprint);
        Ok(Index {
            config: self.config,
            documents: self.documents,
            entries,
        })
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "index".into(), |name| name.to_os_string());
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn checked_u32(value: usize, what: &str) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| FoError::InvalidConfig(format!("{what} exceeds the u32 format limit")))
}

fn write_u32(writer: &mut impl Write, value: u32) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_u64(writer: &mut impl Write, value: u64) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_string(writer: &mut impl Write, value: &str) -> std::io::Result<()> {
    write_u64(writer, value.len() as u64)?;
    writer.write_all(value.as_bytes())
}

fn read_string(reader: &mut impl Read, file_len: u64) -> std::io::Result<String> {
    let length = read_u64(reader)?;
    if length > MAX_STRING_BYTES || length > file_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("string length {length} exceeds a safe bound"),
        ));
    }
    let length = usize::try_from(length).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "string length does not fit this platform",
        )
    })?;
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("stored string is not UTF-8: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Index, IndexBuilder};
    use crate::IndexConfig;

    #[test]
    fn save_load_roundtrip_preserves_dictionary() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document("a.txt", "The quick brown fox jumps over the lazy dog.")
            .expect("document");
        builder
            .add_document("b.txt", "Something entirely different lives here.")
            .expect("document");
        let index = builder.build().expect("index");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("franken-overlap-{nonce}.foidx"));
        index.save(&path).expect("save");
        let loaded = Index::load(&path).expect("load");
        fs::remove_file(path).expect("remove");
        assert_eq!(index.config, loaded.config);
        assert_eq!(index.stats().documents, loaded.stats().documents);
        assert_eq!(
            index.stats().distinct_fingerprints,
            loaded.stats().distinct_fingerprints
        );
    }
}
