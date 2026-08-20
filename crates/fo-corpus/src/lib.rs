#![forbid(unsafe_code)]

mod collection;
mod collection_metadata_serialize;
mod collection_ordering;
mod gutenberg;
mod http;
mod model;
mod sec;
mod sec_filings;
mod section;

pub use collection::{
    COLLECTION_MANIFEST_FILENAME, COLLECTION_MANIFEST_SCHEMA_VERSION, CollectionDocumentRecord,
    CollectionImportOptions, CollectionImportReport, CollectionManifest, CollectionMetadataRow,
    CollectionProfile, CollectionRelation, CollectionRelationKind, CollectionVerificationReport,
    import_collection, verify_collection,
};
pub use gutenberg::{
    DEFAULT_GUTENBERG_MIRROR, GUTENBERG_CATALOG_URL, GutenbergFetchReport, GutenbergOptions,
    GutenbergPreset, fetch_gutenberg,
};
pub use http::{DownloadClient, FetchResponse, HttpOptions};
pub use model::{
    CORPUS_MANIFEST_SCHEMA_VERSION, CorpusDocument, CorpusError, CorpusFailure, CorpusManifest,
    CorpusProvider, MANIFEST_FILENAME, ManifestVerificationReport, Result, atomic_write,
    sha256_hex, unix_timestamp, verify_manifest,
};
pub use sec::{
    SEC_ARCHIVES_BASE, SEC_SUBMISSIONS_BASE, SEC_TICKERS_URL, Sec10KFetchReport, Sec10KOptions,
    SecPreset, fetch_sec_10k,
};
pub use sec_filings::{
    COMMENT_LETTER_FORMS, INVESTOR_CORE_FORMS, REGISTRATION_FORMS, SecFilingCategory,
    SecFilingsFetchReport, SecFilingsOptions, classify_form, fetch_sec_filings,
};
pub use section::{SectionCorpusOptions, SectionCorpusReport, SectionStrategy, section_corpus};
