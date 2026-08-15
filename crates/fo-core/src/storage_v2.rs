use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{
    Document, Fingerprint, FoError, Index, IndexConfig, IndexEntry, IndexStats,
    NormalizationProfile, NormalizedText, Posting, PunctuationMode, Result,
};

const MAGIC_V1: &[u8; 8] = b"FROV0001";
const MAGIC_V2: &[u8; 8] = b"FROV0002";
const FORMAT_VERSION_V2: u32 = 2;
const STORAGE_FLAGS_V2: u32 = 0b11;
const STORAGE_FLAG_DELTA_VARINT: u32 = 1;
const STORAGE_FLAG_CHECKSUM: u32 = 1 << 1;
const MAX_STRING_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_DOCUMENTS: u32 = 100_000_000;
const MAX_ENTRIES: u64 = 2_000_000_000;
const MAX_POSTINGS_PER_ENTRY: u32 = 2_000_000_000;
const MIN_DOCUMENT_RECORD_BYTES: u64 = 20;
const MIN_V2_ENTRY_RECORD_BYTES: u64 = 32;
const MIN_V2_FILE_BYTES: u64 = 56;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexStorageFormat {
    LegacyFixedV1,
    DeltaVarintV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSaveOptions {
    pub format: IndexStorageFormat,
}

impl Default for IndexSaveOptions {
    fn default() -> Self {
        Self {
            format: IndexStorageFormat::DeltaVarintV2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexFileStats {
    pub format: IndexStorageFormat,
    pub file_bytes: u64,
    pub documents: usize,
    pub normalized_tokens: usize,
    pub distinct_fingerprints: usize,
    pub postings: usize,
    pub posting_payload_bytes: u64,
    pub fixed_posting_bytes: u64,
    pub posting_compression_ratio: f64,
    pub checksum: Option<u64>,
}

impl Index {
    pub fn save_with_options(
        &self,
        path: impl AsRef<Path>,
        options: IndexSaveOptions,
    ) -> Result<IndexFileStats> {
        let path = path.as_ref();
        match options.format {
            IndexStorageFormat::LegacyFixedV1 => {
                self.save(path)?;
                legacy_file_stats(self, path)
            }
            IndexStorageFormat::DeltaVarintV2 => save_v2(self, path),
        }
    }

    pub fn save_compressed(&self, path: impl AsRef<Path>) -> Result<IndexFileStats> {
        self.save_with_options(path, IndexSaveOptions::default())
    }

    pub fn load_auto(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match read_magic(path)? {
            magic if &magic == MAGIC_V1 => Self::load(path),
            magic if &magic == MAGIC_V2 => load_v2(path).map(|(index, _)| index),
            _ => Err(FoError::InvalidIndex("bad magic bytes".to_owned())),
        }
    }

    pub fn inspect_storage(path: impl AsRef<Path>) -> Result<IndexFileStats> {
        let path = path.as_ref();
        match read_magic(path)? {
            magic if &magic == MAGIC_V1 => {
                let index = Self::load(path)?;
                legacy_file_stats(&index, path)
            }
            magic if &magic == MAGIC_V2 => load_v2(path).map(|(_, stats)| stats),
            _ => Err(FoError::InvalidIndex("bad magic bytes".to_owned())),
        }
    }
}

fn save_v2(index: &Index, path: &Path) -> Result<IndexFileStats> {
    validate_index_limits(index)?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| FoError::io(parent, error))?;
    }
    let temporary = temporary_path(path);
    let file = File::create(&temporary).map_err(|error| FoError::io(&temporary, error))?;
    let mut writer = ChecksumWriter::new(BufWriter::new(file));
    let qgram_size = checked_u32(index.config.qgram_size, "q-gram size")?;
    let winnow_window = checked_u32(index.config.winnow_window, "winnow window")?;
    let document_count = checked_u32(index.documents.len(), "document count")?;
    let entry_count = u64::try_from(index.entries.len())
        .map_err(|_| FoError::InvalidConfig("entry count exceeds u64".to_owned()))?;

    writer
        .write_all(MAGIC_V2)
        .and_then(|()| write_u32(&mut writer, FORMAT_VERSION_V2))
        .and_then(|()| write_u32(&mut writer, qgram_size))
        .and_then(|()| write_u32(&mut writer, winnow_window))
        .map_err(|error| FoError::io(&temporary, error))?;
    write_normalization_header(&mut writer, &index.config.normalization)
        .map_err(|error| FoError::io(&temporary, error))?;
    write_u32(&mut writer, document_count)
        .and_then(|()| write_u64(&mut writer, entry_count))
        .and_then(|()| write_u32(&mut writer, STORAGE_FLAGS_V2))
        .and_then(|()| write_u32(&mut writer, 0))
        .map_err(|error| FoError::io(&temporary, error))?;

    for document in &index.documents {
        write_u32(&mut writer, document.id)
            .and_then(|()| write_string(&mut writer, &document.path))
            .and_then(|()| write_string(&mut writer, &document.normalized.text))
            .map_err(|error| FoError::io(&temporary, error))?;
    }

    let mut posting_payload_bytes = 0u64;
    for entry in &index.entries {
        let posting_count = checked_u32(entry.postings.len(), "posting count")?;
        let encoded_len = encoded_postings_len(&entry.postings)?;
        posting_payload_bytes = posting_payload_bytes
            .checked_add(encoded_len)
            .ok_or_else(|| FoError::InvalidConfig("posting payload size overflows u64".to_owned()))?;
        write_u64(&mut writer, entry.fingerprint.hi)
            .and_then(|()| write_u64(&mut writer, entry.fingerprint.lo))
            .and_then(|()| write_u32(&mut writer, entry.document_frequency))
            .and_then(|()| write_u32(&mut writer, posting_count))
            .and_then(|()| write_u64(&mut writer, encoded_len))
            .map_err(|error| FoError::io(&temporary, error))?;
        write_delta_postings(&mut writer, &entry.postings)
            .map_err(|error| FoError::io(&temporary, error))?;
    }

    let checksum = writer.checksum();
    writer
        .write_unhashed(&checksum.to_le_bytes())
        .and_then(|()| writer.flush())
        .map_err(|error| FoError::io(&temporary, error))?;
    let buffered = writer.into_inner();
    let file = buffered
        .into_inner()
        .map_err(|error| FoError::io(&temporary, error.into_error()))?;
    file.sync_all()
        .map_err(|error| FoError::io(&temporary, error))?;
    replace_file(&temporary, path)?;

    let file_bytes = fs::metadata(path)
        .map_err(|error| FoError::io(path, error))?
        .len();
    Ok(file_stats(
        index.stats(),
        IndexStorageFormat::DeltaVarintV2,
        file_bytes,
        posting_payload_bytes,
        Some(checksum),
    ))
}

fn load_v2(path: &Path) -> Result<(Index, IndexFileStats)> {
    let file = File::open(path).map_err(|error| FoError::io(path, error))?;
    let file_len = file
        .metadata()
        .map_err(|error| FoError::io(path, error))?
        .len();
    if file_len < MIN_V2_FILE_BYTES {
        return Err(FoError::InvalidIndex("v2 index is too short".to_owned()));
    }
    let mut reader = ChecksumReader::new(BufReader::new(file));
    let mut magic = [0u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|error| FoError::io(path, error))?;
    if &magic != MAGIC_V2 {
        return Err(FoError::InvalidIndex("bad v2 magic bytes".to_owned()));
    }
    let version = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
    if version != FORMAT_VERSION_V2 {
        return Err(FoError::InvalidIndex(format!(
            "format version {version} is unsupported"
        )));
    }
    let qgram_size = read_u32(&mut reader).map_err(|error| FoError::io(path, error))? as usize;
    let winnow_window = read_u32(&mut reader).map_err(|error| FoError::io(path, error))? as usize;
    let normalization = read_normalization_header(&mut reader, path)?;
    let config = IndexConfig {
        normalization,
        qgram_size,
        winnow_window,
    };
    config.validate()?;

    let document_count = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
    let entry_count_u64 = read_u64(&mut reader).map_err(|error| FoError::io(path, error))?;
    let storage_flags = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
    let reserved = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
    if storage_flags != STORAGE_FLAGS_V2 || reserved != 0 {
        return Err(FoError::InvalidIndex(format!(
            "unsupported v2 storage flags {storage_flags:#x} or nonzero reserved header"
        )));
    }
    if storage_flags & STORAGE_FLAG_DELTA_VARINT == 0
        || storage_flags & STORAGE_FLAG_CHECKSUM == 0
    {
        return Err(FoError::InvalidIndex(
            "v2 index lacks required delta-varint/checksum flags".to_owned(),
        ));
    }
    if document_count > MAX_DOCUMENTS
        || u64::from(document_count).saturating_mul(MIN_DOCUMENT_RECORD_BYTES) > file_len
    {
        return Err(FoError::InvalidIndex(format!(
            "document count {document_count} exceeds a safe bound"
        )));
    }
    if entry_count_u64 > MAX_ENTRIES
        || entry_count_u64.saturating_mul(MIN_V2_ENTRY_RECORD_BYTES) > file_len
    {
        return Err(FoError::InvalidIndex(format!(
            "entry count {entry_count_u64} exceeds a safe bound"
        )));
    }
    let entry_count = usize::try_from(entry_count_u64).map_err(|_| {
        FoError::InvalidIndex("entry count does not fit this platform".to_owned())
    })?;

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
    let mut posting_payload_bytes = 0u64;
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
        let document_frequency = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
        let posting_count_u32 = read_u32(&mut reader).map_err(|error| FoError::io(path, error))?;
        let encoded_len = read_u64(&mut reader).map_err(|error| FoError::io(path, error))?;
        validate_entry_counts(
            document_frequency,
            posting_count_u32,
            document_count,
            encoded_len,
            file_len,
        )?;
        posting_payload_bytes = posting_payload_bytes
            .checked_add(encoded_len)
            .ok_or_else(|| FoError::InvalidIndex("posting payload size overflows u64".to_owned()))?;
        let postings = read_delta_postings(
            &mut reader,
            posting_count_u32 as usize,
            encoded_len,
            &documents,
            config.qgram_size,
        )
        .map_err(|error| FoError::InvalidIndex(format!("invalid posting payload: {error}")))?;
        let observed_document_frequency = postings
            .iter()
            .map(|posting| posting.document_id)
            .fold((None, 0u32), |(last, count), document_id| {
                if last == Some(document_id) {
                    (last, count)
                } else {
                    (Some(document_id), count.saturating_add(1))
                }
            })
            .1;
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

    let actual_checksum = reader.checksum();
    let mut checksum_bytes = [0u8; 8];
    reader
        .read_exact_unhashed(&mut checksum_bytes)
        .map_err(|error| FoError::io(path, error))?;
    let expected_checksum = u64::from_le_bytes(checksum_bytes);
    if actual_checksum != expected_checksum {
        return Err(FoError::InvalidIndex(format!(
            "v2 checksum mismatch: expected {expected_checksum:016x}, observed {actual_checksum:016x}"
        )));
    }
    let mut trailing = [0u8; 1];
    match reader.read_unhashed(&mut trailing) {
        Ok(0) => {}
        Ok(_) => {
            return Err(FoError::InvalidIndex(
                "trailing bytes after the v2 checksum".to_owned(),
            ));
        }
        Err(error) => return Err(FoError::io(path, error)),
    }

    let index = Index {
        config,
        documents,
        entries,
    };
    let stats = file_stats(
        index.stats(),
        IndexStorageFormat::DeltaVarintV2,
        file_len,
        posting_payload_bytes,
        Some(expected_checksum),
    );
    Ok((index, stats))
}

fn validate_index_limits(index: &Index) -> Result<()> {
    if index.documents.len() > MAX_DOCUMENTS as usize {
        return Err(FoError::InvalidConfig(format!(
            "document count {} exceeds the index safety limit {MAX_DOCUMENTS}",
            index.documents.len()
        )));
    }
    let entry_count = u64::try_from(index.entries.len())
        .map_err(|_| FoError::InvalidConfig("entry count exceeds u64".to_owned()))?;
    if entry_count > MAX_ENTRIES {
        return Err(FoError::InvalidConfig(format!(
            "entry count {} exceeds the index safety limit {MAX_ENTRIES}",
            index.entries.len()
        )));
    }
    if let Some(entry) = index
        .entries
        .iter()
        .find(|entry| entry.postings.len() > MAX_POSTINGS_PER_ENTRY as usize)
    {
        return Err(FoError::InvalidConfig(format!(
            "fingerprint {:?} has {} postings, exceeding {MAX_POSTINGS_PER_ENTRY}",
            entry.fingerprint,
            entry.postings.len()
        )));
    }
    Ok(())
}

fn validate_entry_counts(
    document_frequency: u32,
    posting_count: u32,
    document_count: u32,
    encoded_len: u64,
    file_len: u64,
) -> Result<()> {
    if document_frequency == 0
        || document_frequency > document_count
        || document_frequency > posting_count
    {
        return Err(FoError::InvalidIndex(format!(
            "document frequency {document_frequency} is impossible for {posting_count} postings across {document_count} documents"
        )));
    }
    if posting_count == 0 || posting_count > MAX_POSTINGS_PER_ENTRY {
        return Err(FoError::InvalidIndex(format!(
            "posting count {posting_count} exceeds a safe bound"
        )));
    }
    let maximum_encoded = u64::from(posting_count).saturating_mul(15);
    if encoded_len == 0 || encoded_len > maximum_encoded || encoded_len > file_len {
        return Err(FoError::InvalidIndex(format!(
            "encoded posting length {encoded_len} exceeds a safe bound"
        )));
    }
    Ok(())
}

fn encoded_postings_len(postings: &[Posting]) -> Result<u64> {
    let mut length = 0u64;
    let mut previous_document = 0u32;
    let mut previous_position = 0u32;
    for (index, posting) in postings.iter().enumerate() {
        if index > 0 && postings[index - 1] >= *posting {
            return Err(FoError::InvalidConfig(
                "posting lists must be strictly sorted before v2 serialization".to_owned(),
            ));
        }
        let document_delta = if index == 0 {
            posting.document_id
        } else {
            posting.document_id - previous_document
        };
        let position_code = if index == 0 || document_delta > 0 {
            posting.position
        } else {
            posting.position - previous_position
        };
        length = length
            .checked_add(varint_len(u64::from(document_delta)) as u64)
            .and_then(|value| value.checked_add(varint_len(u64::from(position_code)) as u64))
            .ok_or_else(|| FoError::InvalidConfig("encoded posting length overflows u64".to_owned()))?;
        previous_document = posting.document_id;
        previous_position = posting.position;
    }
    Ok(length)
}

fn write_delta_postings(writer: &mut impl Write, postings: &[Posting]) -> std::io::Result<()> {
    let mut previous_document = 0u32;
    let mut previous_position = 0u32;
    for (index, posting) in postings.iter().enumerate() {
        let document_delta = if index == 0 {
            posting.document_id
        } else {
            posting.document_id - previous_document
        };
        let position_code = if index == 0 || document_delta > 0 {
            posting.position
        } else {
            posting.position - previous_position
        };
        write_varint(writer, u64::from(document_delta))?;
        write_varint(writer, u64::from(position_code))?;
        previous_document = posting.document_id;
        previous_position = posting.position;
    }
    Ok(())
}

fn read_delta_postings(
    reader: &mut impl Read,
    posting_count: usize,
    encoded_len: u64,
    documents: &[Document],
    qgram_size: usize,
) -> std::result::Result<Vec<Posting>, String> {
    let mut remaining = encoded_len;
    let mut postings = Vec::with_capacity(posting_count);
    let mut previous_document = 0u32;
    let mut previous_position = 0u32;
    for index in 0..posting_count {
        let document_delta = read_varint_limited(reader, &mut remaining)?;
        let position_code = read_varint_limited(reader, &mut remaining)?;
        let document_id_u64 = if index == 0 {
            document_delta
        } else {
            u64::from(previous_document)
                .checked_add(document_delta)
                .ok_or_else(|| "document delta overflows u64".to_owned())?
        };
        let document_id = u32::try_from(document_id_u64)
            .map_err(|_| "decoded document id exceeds u32".to_owned())?;
        let position_u64 = if index == 0 || document_delta > 0 {
            position_code
        } else {
            u64::from(previous_position)
                .checked_add(position_code)
                .ok_or_else(|| "position delta overflows u64".to_owned())?
        };
        let position = u32::try_from(position_u64)
            .map_err(|_| "decoded position exceeds u32".to_owned())?;
        let posting = Posting {
            document_id,
            position,
        };
        if postings.last().is_some_and(|previous| *previous >= posting) {
            return Err("decoded posting list is not strictly sorted".to_owned());
        }
        let document = documents
            .get(document_id as usize)
            .ok_or_else(|| format!("posting references missing document {document_id}"))?;
        if (position as usize)
            .checked_add(qgram_size)
            .is_none_or(|end| end > document.normalized.tokens.len())
        {
            return Err(format!(
                "posting position {position} is outside document {document_id}"
            ));
        }
        postings.push(posting);
        previous_document = document_id;
        previous_position = position;
    }
    if remaining != 0 {
        return Err(format!(
            "posting payload has {remaining} unconsumed encoded bytes"
        ));
    }
    Ok(postings)
}

fn write_varint(writer: &mut impl Write, mut value: u64) -> std::io::Result<()> {
    let mut bytes = [0u8; 10];
    let mut length = 0usize;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        bytes[length] = byte;
        length += 1;
        if value == 0 {
            break;
        }
    }
    writer.write_all(&bytes[..length])
}

fn read_varint_limited(
    reader: &mut impl Read,
    remaining: &mut u64,
) -> std::result::Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        if *remaining == 0 {
            return Err("truncated varint".to_owned());
        }
        let mut byte = [0u8; 1];
        reader
            .read_exact(&mut byte)
            .map_err(|error| format!("truncated varint: {error}"))?;
        *remaining -= 1;
        if shift == 63 && byte[0] > 1 {
            return Err("varint overflows u64".to_owned());
        }
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("varint exceeds ten bytes".to_owned())
}

fn varint_len(mut value: u64) -> usize {
    let mut length = 1usize;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

fn legacy_file_stats(index: &Index, path: &Path) -> Result<IndexFileStats> {
    let file_bytes = fs::metadata(path)
        .map_err(|error| FoError::io(path, error))?
        .len();
    let fixed = fixed_posting_bytes(index.stats().postings);
    Ok(file_stats(
        index.stats(),
        IndexStorageFormat::LegacyFixedV1,
        file_bytes,
        fixed,
        None,
    ))
}

fn file_stats(
    stats: IndexStats,
    format: IndexStorageFormat,
    file_bytes: u64,
    posting_payload_bytes: u64,
    checksum: Option<u64>,
) -> IndexFileStats {
    let fixed_posting_bytes = fixed_posting_bytes(stats.postings);
    IndexFileStats {
        format,
        file_bytes,
        documents: stats.documents,
        normalized_tokens: stats.normalized_tokens,
        distinct_fingerprints: stats.distinct_fingerprints,
        postings: stats.postings,
        posting_payload_bytes,
        fixed_posting_bytes,
        posting_compression_ratio: if fixed_posting_bytes == 0 {
            1.0
        } else {
            posting_payload_bytes as f64 / fixed_posting_bytes as f64
        },
        checksum,
    }
}

fn fixed_posting_bytes(postings: usize) -> u64 {
    (postings as u128)
        .saturating_mul(8)
        .min(u64::MAX as u128) as u64
}

fn read_magic(path: &Path) -> Result<[u8; 8]> {
    let mut file = File::open(path).map_err(|error| FoError::io(path, error))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|error| FoError::io(path, error))?;
    Ok(magic)
}

fn write_normalization_header(
    writer: &mut impl Write,
    normalization: &NormalizationProfile,
) -> std::io::Result<()> {
    let mut flags = 0u32;
    if normalization.nfkc {
        flags |= 1;
    }
    if normalization.lowercase {
        flags |= 1 << 1;
    }
    if normalization.collapse_whitespace {
        flags |= 1 << 2;
    }
    write_u32(writer, flags)?;
    writer.write_all(&[normalization.punctuation.as_u8(), 0, 0, 0])
}

fn read_normalization_header(
    reader: &mut impl Read,
    path: &Path,
) -> Result<NormalizationProfile> {
    let flags = read_u32(reader).map_err(|error| FoError::io(path, error))?;
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
            "nonzero reserved normalization bytes".to_owned(),
        ));
    }
    Ok(NormalizationProfile {
        nfkc: flags & 1 != 0,
        lowercase: flags & 2 != 0,
        collapse_whitespace: flags & 4 != 0,
        punctuation: PunctuationMode::from_u8(punctuation_and_reserved[0])?,
    })
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
    let length = u64::try_from(value.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "string length does not fit the index format",
        )
    })?;
    if length > MAX_STRING_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("string length {length} exceeds the {MAX_STRING_BYTES}-byte limit"),
        ));
    }
    write_u64(writer, length)?;
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

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "index".into(), |name| name.to_os_string());
    let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    path.with_file_name(name)
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

struct ChecksumWriter<W> {
    inner: W,
    checksum: u64,
}

impl<W> ChecksumWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            checksum: FNV_OFFSET,
        }
    }

    fn checksum(&self) -> u64 {
        self.checksum
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> ChecksumWriter<W> {
    fn write_unhashed(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(bytes)
    }
}

impl<W: Write> Write for ChecksumWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        update_checksum(&mut self.checksum, &buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }

    fn write_all(&mut self, buffer: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(buffer)?;
        update_checksum(&mut self.checksum, buffer);
        Ok(())
    }
}

struct ChecksumReader<R> {
    inner: R,
    checksum: u64,
}

impl<R> ChecksumReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            checksum: FNV_OFFSET,
        }
    }

    fn checksum(&self) -> u64 {
        self.checksum
    }
}

impl<R: Read> ChecksumReader<R> {
    fn read_exact_unhashed(&mut self, buffer: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_exact(buffer)
    }

    fn read_unhashed(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl<R: Read> Read for ChecksumReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        update_checksum(&mut self.checksum, &buffer[..read]);
        Ok(read)
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_exact(buffer)?;
        update_checksum(&mut self.checksum, buffer);
        Ok(())
    }
}

fn update_checksum(checksum: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *checksum ^= u64::from(*byte);
        *checksum = checksum.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{IndexSaveOptions, IndexStorageFormat};
    use crate::{Index, IndexBuilder, IndexConfig, SearchOptions};

    #[test]
    fn v2_roundtrip_preserves_search_and_reduces_posting_payload() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        for document in 0..32 {
            builder
                .add_document(
                    format!("document-{document}.txt"),
                    format!(
                        "common repeated passage number {document} with measurements and observations \
                         common repeated passage number {document} with measurements and observations"
                    ),
                )
                .expect("document");
        }
        let index = builder.build().expect("index");
        let path = temporary_file("v2");
        let stats = index
            .save_with_options(
                &path,
                IndexSaveOptions {
                    format: IndexStorageFormat::DeltaVarintV2,
                },
            )
            .expect("save v2");
        assert_eq!(stats.format, IndexStorageFormat::DeltaVarintV2);
        assert!(stats.posting_payload_bytes < stats.fixed_posting_bytes);
        let loaded = Index::load_auto(&path).expect("load v2");
        assert_eq!(loaded.stats(), index.stats());
        let hits = loaded
            .search(
                "passage number 7 with measurements and observations",
                &SearchOptions {
                    minimum_similarity: 0.10,
                    minimum_matched_tokens: 8,
                    ..SearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(hits[0].path, "document-7.txt");
        fs::remove_file(path).ok();
    }

    #[test]
    fn auto_loader_keeps_v1_compatibility() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document("legacy.txt", "legacy fixed index remains readable")
            .expect("document");
        let index = builder.build().expect("index");
        let path = temporary_file("v1");
        index.save(&path).expect("save v1");
        let loaded = Index::load_auto(&path).expect("load auto");
        assert_eq!(loaded.stats(), index.stats());
        let stats = Index::inspect_storage(&path).expect("inspect");
        assert_eq!(stats.format, IndexStorageFormat::LegacyFixedV1);
        fs::remove_file(path).ok();
    }

    #[test]
    fn checksum_detects_payload_corruption() {
        let mut builder = IndexBuilder::new(IndexConfig::default()).expect("builder");
        builder
            .add_document("a.txt", "checksum protected compressed postings")
            .expect("document");
        let index = builder.build().expect("index");
        let path = temporary_file("corrupt");
        index.save_compressed(&path).expect("save");
        let mut bytes = fs::read(&path).expect("read");
        let position = bytes.len() / 2;
        bytes[position] ^= 0x40;
        fs::write(&path, bytes).expect("write corruption");
        let error = Index::load_auto(&path).expect_err("corruption must fail");
        assert!(
            error.to_string().contains("checksum")
                || error.to_string().contains("posting")
                || error.to_string().contains("UTF-8")
        );
        fs::remove_file(path).ok();
    }

    fn temporary_file(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "franken-overlap-{label}-{}-{nonce}.foidx",
            std::process::id()
        ))
    }
}
