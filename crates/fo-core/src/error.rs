use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, FoError>;

#[derive(Debug, Error)]
pub enum FoError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid or unsupported index: {0}")]
    InvalidIndex(String),
    #[error("specimen is empty after normalization")]
    EmptySpecimen,
    #[error("document count exceeds the u32 index format limit")]
    TooManyDocuments,
    #[error("document {path} has {tokens} normalized tokens, exceeding the u32 position limit")]
    DocumentTooLarge { path: String, tokens: usize },
    #[error("spectral backend failed: {0}")]
    Spectral(String),
}

impl FoError {
    pub fn io(path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}
