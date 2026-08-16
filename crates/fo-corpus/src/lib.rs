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
    import_collection, verify_collection, CollectionDocumentRecord, CollectionImportOptions,
    CollectionImportReport, CollectionManifest, CollectionMetadataRow, CollectionProfile,
    CollectionRelation, CollectionRelationKind, CollectionVerificationReport,
    COLLECTION_MANIFEST_FILENAME, COLLECTION_MANIFEST_SCHEMA_VERSION,
};
pub use gutenberg::{
    fetch_gutenberg, GutenbergFetchReport, GutenbergOptions, GutenbergPreset,
    DEFAULT_GUTENBERG_MIRROR, GUTENBERG_CATALOG_URL,
};
pub use http::{DownloadClient, FetchResponse, HttpOptions};
pub use model::{
    atomic_write, sha256_hex, unix_timestamp, verify_manifest, CorpusDocument, CorpusError,
    CorpusFailure, CorpusManifest, CorpusProvider, ManifestVerificationReport, Result,
    CORPUS_MANIFEST_SCHEMA_VERSION, MANIFEST_FILENAME,
};
pub use sec::{
    fetch_sec_10k, Sec10KFetchReport, Sec10KOptions, SecPreset, SEC_ARCHIVES_BASE,
    SEC_SUBMISSIONS_BASE, SEC_TICKERS_URL,
};
pub use sec_filings::{
    classify_form, fetch_sec_filings, SecFilingCategory, SecFilingsFetchReport,
    SecFilingsOptions, COMMENT_LETTER_FORMS, INVESTOR_CORE_FORMS, REGISTRATION_FORMS,
};
pub use section::{
    section_corpus, SectionCorpusOptions, SectionCorpusReport, SectionStrategy,
};
