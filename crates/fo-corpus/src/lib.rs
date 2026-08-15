#![forbid(unsafe_code)]

mod gutenberg;
mod http;
mod model;
#[allow(unused_imports)]
mod sec;

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
