#![forbid(unsafe_code)]

mod chain;
mod error;
mod fingerprint;
mod index;
mod model;
mod normalize;
mod search;
pub mod spectral;
mod verify;
mod winnow;

pub use chain::{Anchor, AnchorChain, ChainOptions, chain_anchors};
pub use error::{FoError, Result};
pub use fingerprint::{Feature, Fingerprint, qgram_hashes};
pub use index::{Document, Index, IndexBuilder, IndexEntry};
pub use model::{
    IndexConfig, IndexStats, NormalizationProfile, Posting, PunctuationMode, SearchOptions,
    SearchResult,
};
pub use normalize::{NormalizedText, normalize};
pub use spectral::{SpectralOptions, SpectralPeak, spectral_scan};
pub use verify::{Alignment, global_levenshtein, semi_global_banded};
pub use winnow::winnow;
