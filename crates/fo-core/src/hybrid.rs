use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    CompositeSearchOptions, CompositeSearchResult, FoError, Index, IndexBuilder, IndexConfig,
    LexicalDocumentInput, LexicalIndex, LexicalIndexBuilder, LexicalIndexConfig, LexicalQuery,
    LexicalSearchOptions, LexicalSearchResult, Result, SearchOptions, SearchResult,
};

pub const HYBRID_INDEX_SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const OVERLAP_FILE: &str = "overlap.foidx";
const LEXICAL_FILE: &str = "lexical.folex";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HybridIndexConfig {
    pub overlap: IndexConfig,
    pub lexical: LexicalIndexConfig,
}

impl Default for HybridIndexConfig {
    fn default() -> Self {
        Self {
            overlap: IndexConfig::default(),
            lexical: LexicalIndexConfig::default(),
        }
    }
}

impl HybridIndexConfig {
    pub fn validate(&self) -> Result<()> {
        self.overlap.validate()?;
        self.lexical.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridDocumentInput {
    pub external_id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct HybridIndexBuilder {
    config: HybridIndexConfig,
    documents: Vec<HybridDocumentInput>,
}

impl HybridIndexBuilder {
    pub fn new(config: HybridIndexConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            documents: Vec::new(),
        })
    }

    pub fn add_document(&mut self, document: HybridDocumentInput) -> Result<u32> {
        if document.external_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "hybrid external_id must not be empty".to_owned(),
            ));
        }
        if self.documents.len() >= u32::MAX as usize {
            return Err(FoError::TooManyDocuments);
        }
        if self
            .documents
            .iter()
            .any(|existing| existing.external_id == document.external_id)
        {
            return Err(FoError::InvalidConfig(format!(
                "duplicate hybrid external_id {}",
                document.external_id
            )));
        }
        let document_id = self.documents.len() as u32;
        self.documents.push(document);
        Ok(document_id)
    }

    pub fn build(self) -> Result<HybridIndex> {
        let mut overlap_builder = IndexBuilder::new(self.config.overlap.clone())?;
        let mut lexical_builder = LexicalIndexBuilder::new(self.config.lexical.clone())?;
        let mut document_ids = Vec::with_capacity(self.documents.len());
        for document in self.documents {
            let overlap_id = overlap_builder.add_document(&document.external_id, &document.body)?;
            let lexical_id = lexical_builder.add_document(LexicalDocumentInput {
                external_id: document.external_id.clone(),
                title: document.title,
                body: document.body,
                tags: document.tags,
                metadata: document.metadata,
            })?;
            if overlap_id != lexical_id {
                return Err(FoError::InvalidConfig(
                    "hybrid builders produced inconsistent document IDs".to_owned(),
                ));
            }
            document_ids.push(document.external_id);
        }
        let index = HybridIndex {
            config: self.config,
            overlap: overlap_builder.build()?,
            lexical: lexical_builder.build()?,
            document_ids,
        };
        index.validate()?;
        Ok(index)
    }
}

#[derive(Debug, Clone)]
pub struct HybridIndex {
    config: HybridIndexConfig,
    overlap: Index,
    lexical: LexicalIndex,
    document_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HybridSearchMode {
    Auto,
    Lexical,
    Overlap,
    Composite,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HybridRoute {
    Lexical,
    Overlap,
    Composite,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridQueryAnalysis {
    pub route: HybridRoute,
    pub word_count: usize,
    pub unique_word_count: usize,
    pub unique_word_fraction: f32,
    pub character_count: usize,
    pub paragraph_count: usize,
    pub explicit_lexical_syntax: bool,
    pub quoted_phrase_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HybridSearchOptions {
    pub mode: HybridSearchMode,
    pub max_results: usize,
    pub candidate_multiplier: usize,
    pub lexical_weight: f32,
    pub overlap_weight: f32,
    pub reciprocal_rank_k: f32,
    pub auto_lexical_max_words: usize,
    pub auto_overlap_min_words: usize,
    pub auto_composite_min_words: usize,
    pub final_minimum_score: f32,
    pub required_metadata: BTreeMap<String, String>,
    pub required_tags: Vec<String>,
    pub excluded_tags: Vec<String>,
    pub lexical: LexicalSearchOptions,
    pub overlap: SearchOptions,
    pub composite: CompositeSearchOptions,
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            mode: HybridSearchMode::Auto,
            max_results: 20,
            candidate_multiplier: 8,
            lexical_weight: 1.0,
            overlap_weight: 1.0,
            reciprocal_rank_k: 60.0,
            auto_lexical_max_words: 12,
            auto_overlap_min_words: 48,
            auto_composite_min_words: 180,
            final_minimum_score: 0.0,
            required_metadata: BTreeMap::new(),
            required_tags: Vec::new(),
            excluded_tags: Vec::new(),
            lexical: LexicalSearchOptions::default(),
            overlap: SearchOptions::default(),
            composite: CompositeSearchOptions::default(),
        }
    }
}

impl HybridSearchOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0
            || self.candidate_multiplier == 0
            || self.auto_lexical_max_words == 0
            || self.auto_overlap_min_words == 0
            || self.auto_composite_min_words == 0
        {
            return Err(FoError::InvalidConfig(
                "hybrid result, candidate, and route thresholds must be positive".to_owned(),
            ));
        }
        if self.auto_lexical_max_words >= self.auto_overlap_min_words
            || self.auto_overlap_min_words >= self.auto_composite_min_words
        {
            return Err(FoError::InvalidConfig(
                "hybrid automatic word thresholds must be strictly increasing".to_owned(),
            ));
        }
        for (name, weight) in [
            ("lexical_weight", self.lexical_weight),
            ("overlap_weight", self.overlap_weight),
        ] {
            if !weight.is_finite() || weight < 0.0 || weight > 100.0 {
                return Err(FoError::InvalidConfig(format!(
                    "hybrid {name} must lie in [0, 100]"
                )));
            }
        }
        if self.lexical_weight + self.overlap_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "at least one hybrid lane weight must be positive".to_owned(),
            ));
        }
        if !self.reciprocal_rank_k.is_finite()
            || self.reciprocal_rank_k <= 0.0
            || self.reciprocal_rank_k > 10_000.0
        {
            return Err(FoError::InvalidConfig(
                "hybrid reciprocal_rank_k must lie in (0, 10000]".to_owned(),
            ));
        }
        if !self.final_minimum_score.is_finite()
            || !(0.0..=1.0).contains(&self.final_minimum_score)
        {
            return Err(FoError::InvalidConfig(
                "hybrid final_minimum_score must lie in [0, 1]".to_owned(),
            ));
        }
        self.lexical.validate()?;
        self.overlap.validate()?;
        self.composite.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HybridScoreExplanation {
    pub lexical_rank: Option<usize>,
    pub overlap_rank: Option<usize>,
    pub lexical_normalized_score: f32,
    pub overlap_score: f32,
    pub reciprocal_rank_component: f32,
    pub evidence_component: f32,
    pub cross_lane_support: bool,
    pub lexical_weight: f32,
    pub overlap_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResult {
    pub document_id: u32,
    pub external_id: String,
    pub title: String,
    pub route: HybridRoute,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub lexical: Option<LexicalSearchResult>,
    pub overlap: Option<SearchResult>,
    pub composite: Option<CompositeSearchResult>,
    pub explanation: HybridScoreExplanation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchReport {
    pub analysis: HybridQueryAnalysis,
    pub results: Vec<HybridSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridIndexStats {
    pub documents: usize,
    pub overlap: crate::IndexStats,
    pub lexical: crate::LexicalIndexStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HybridManifest {
    schema_version: u32,
    config: HybridIndexConfig,
    document_ids: Vec<String>,
    overlap_file: String,
    lexical_file: String,
}

impl HybridIndex {
    #[must_use]
    pub fn config(&self) -> &HybridIndexConfig {
        &self.config
    }

    #[must_use]
    pub fn overlap_index(&self) -> &Index {
        &self.overlap
    }

    #[must_use]
    pub fn lexical_index(&self) -> &LexicalIndex {
        &self.lexical
    }

    #[must_use]
    pub fn stats(&self) -> HybridIndexStats {
        HybridIndexStats {
            documents: self.document_ids.len(),
            overlap: self.overlap.stats(),
            lexical: self.lexical.stats(),
        }
    }

    pub fn analyze_query(
        &self,
        query: &str,
        options: &HybridSearchOptions,
    ) -> Result<HybridQueryAnalysis> {
        options.validate()?;
        let parsed = LexicalQuery::parse(query)?;
        let words = query
            .unicode_words()
            .map(|word| word.to_lowercase())
            .collect::<Vec<_>>();
        let unique = words.iter().collect::<BTreeSet<_>>().len();
        let word_count = words.len();
        let paragraph_count = query
            .split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
            .count()
            .max(1);
        let explicit_lexical_syntax = query.contains('"')
            || query.contains("title:")
            || query.contains("body:")
            || query.contains("tag:")
            || query
                .split_whitespace()
                .any(|token| token.starts_with('+') || token.starts_with('-'));
        let quoted_phrase_count = parsed
            .clauses
            .iter()
            .filter(|clause| clause.phrase)
            .count();
        let route = match options.mode {
            HybridSearchMode::Lexical => HybridRoute::Lexical,
            HybridSearchMode::Overlap => HybridRoute::Overlap,
            HybridSearchMode::Composite => HybridRoute::Composite,
            HybridSearchMode::Hybrid => HybridRoute::Hybrid,
            HybridSearchMode::Auto => {
                if explicit_lexical_syntax && word_count < options.auto_overlap_min_words * 2 {
                    HybridRoute::Lexical
                } else if word_count <= options.auto_lexical_max_words {
                    HybridRoute::Lexical
                } else if word_count >= options.auto_composite_min_words
                    && paragraph_count >= 2
                {
                    HybridRoute::Composite
                } else if word_count >= options.auto_overlap_min_words {
                    HybridRoute::Overlap
                } else {
                    HybridRoute::Hybrid
                }
            }
        };
        Ok(HybridQueryAnalysis {
            route,
            word_count,
            unique_word_count: unique,
            unique_word_fraction: unique as f32 / word_count.max(1) as f32,
            character_count: query.chars().count(),
            paragraph_count,
            explicit_lexical_syntax,
            quoted_phrase_count,
        })
    }

    pub fn search_text(
        &self,
        query: &str,
        options: &HybridSearchOptions,
    ) -> Result<HybridSearchReport> {
        self.validate()?;
        options.validate()?;
        let analysis = self.analyze_query(query, options)?;
        let candidate_limit = options
            .max_results
            .saturating_mul(options.candidate_multiplier)
            .max(options.max_results);
        let mut lexical_options = options.lexical.clone();
        lexical_options.max_results = candidate_limit;
        lexical_options.max_candidate_documents = lexical_options
            .max_candidate_documents
            .max(candidate_limit);
        let mut overlap_options = options.overlap.clone();
        overlap_options.max_results = candidate_limit;
        overlap_options.max_candidates = overlap_options.max_candidates.max(candidate_limit * 4);

        let mut results = match analysis.route {
            HybridRoute::Lexical => self
                .lexical
                .search_text(query, &lexical_options)?
                .into_iter()
                .enumerate()
                .filter_map(|(rank, result)| {
                    self.from_lexical(result, rank + 1, options, HybridRoute::Lexical)
                })
                .collect(),
            HybridRoute::Overlap => self
                .overlap
                .search(query, &overlap_options)?
                .into_iter()
                .enumerate()
                .filter_map(|(rank, result)| {
                    self.from_overlap(result, rank + 1, options, HybridRoute::Overlap)
                })
                .collect(),
            HybridRoute::Composite => self
                .overlap
                .search_composite(query, &overlap_options, options.composite)?
                .into_iter()
                .enumerate()
                .filter_map(|(rank, result)| {
                    self.from_composite(result, rank + 1, options)
                })
                .collect(),
            HybridRoute::Hybrid => {
                let lexical = self.lexical.search_text(query, &lexical_options)?;
                let overlap = self.overlap.search(query, &overlap_options)?;
                self.fuse(lexical, overlap, options)
            }
        };
        results.retain(|result| result.score >= options.final_minimum_score);
        results.sort_unstable_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.document_id.cmp(&right.document_id))
        });
        results.truncate(options.max_results);
        Ok(HybridSearchReport { analysis, results })
    }

    pub fn save(&self, directory: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let directory = directory.as_ref();
        let mut temporary = directory.to_path_buf();
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("hybrid");
        temporary.set_file_name(format!("{name}.tmp-{}", std::process::id()));
        if temporary.exists() {
            fs::remove_dir_all(&temporary).map_err(|error| FoError::io(&temporary, error))?;
        }
        fs::create_dir_all(&temporary).map_err(|error| FoError::io(&temporary, error))?;
        self.overlap.save_compressed(temporary.join(OVERLAP_FILE))?;
        self.lexical.save(temporary.join(LEXICAL_FILE))?;
        let manifest = HybridManifest {
            schema_version: HYBRID_INDEX_SCHEMA_VERSION,
            config: self.config.clone(),
            document_ids: self.document_ids.clone(),
            overlap_file: OVERLAP_FILE.to_owned(),
            lexical_file: LEXICAL_FILE.to_owned(),
        };
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            FoError::InvalidConfig(format!("could not serialize hybrid manifest: {error}"))
        })?;
        fs::write(temporary.join(MANIFEST_FILE), bytes)
            .map_err(|error| FoError::io(temporary.join(MANIFEST_FILE), error))?;
        if directory.exists() {
            fs::remove_dir_all(directory).map_err(|error| FoError::io(directory, error))?;
        }
        fs::rename(&temporary, directory).map_err(|error| FoError::io(directory, error))
    }

    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        let manifest_path = directory.join(MANIFEST_FILE);
        let manifest = serde_json::from_slice::<HybridManifest>(
            &fs::read(&manifest_path).map_err(|error| FoError::io(&manifest_path, error))?,
        )
        .map_err(|error| FoError::InvalidIndex(format!("invalid hybrid manifest: {error}")))?;
        if manifest.schema_version != HYBRID_INDEX_SCHEMA_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "unsupported hybrid schema version {}",
                manifest.schema_version
            )));
        }
        validate_filename(&manifest.overlap_file)?;
        validate_filename(&manifest.lexical_file)?;
        let index = Self {
            config: manifest.config,
            overlap: Index::load_auto(directory.join(&manifest.overlap_file))?,
            lexical: LexicalIndex::load(directory.join(&manifest.lexical_file))?,
            document_ids: manifest.document_ids,
        };
        index.validate()?;
        Ok(index)
    }

    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        self.lexical.validate()?;
        if self.document_ids.len() != self.overlap.documents().len()
            || self.document_ids.len() != self.lexical.documents().len()
        {
            return Err(FoError::InvalidIndex(
                "hybrid lane document counts disagree".to_owned(),
            ));
        }
        for (document_id, external_id) in self.document_ids.iter().enumerate() {
            let overlap = &self.overlap.documents()[document_id];
            let lexical = &self.lexical.documents()[document_id];
            if overlap.id as usize != document_id
                || lexical.document_id as usize != document_id
                || overlap.path != *external_id
                || lexical.external_id != *external_id
            {
                return Err(FoError::InvalidIndex(format!(
                    "hybrid document identity disagrees at position {document_id}"
                )));
            }
        }
        Ok(())
    }

    fn fuse(
        &self,
        lexical_results: Vec<LexicalSearchResult>,
        overlap_results: Vec<SearchResult>,
        options: &HybridSearchOptions,
    ) -> Vec<HybridSearchResult> {
        #[derive(Default)]
        struct Lanes {
            lexical: Option<(usize, LexicalSearchResult)>,
            overlap: Option<(usize, SearchResult)>,
        }

        let mut lanes = HashMap::<u32, Lanes>::new();
        for (rank, result) in lexical_results.into_iter().enumerate() {
            lanes.entry(result.document_id).or_default().lexical = Some((rank + 1, result));
        }
        for (rank, result) in overlap_results.into_iter().enumerate() {
            lanes.entry(result.document_id).or_default().overlap = Some((rank + 1, result));
        }
        lanes
            .into_iter()
            .filter_map(|(document_id, lanes)| {
                let document = self.lexical.documents().get(document_id as usize)?;
                if !passes_filters(document, options) {
                    return None;
                }
                let lexical_rank = lanes.lexical.as_ref().map(|(rank, _)| *rank);
                let overlap_rank = lanes.overlap.as_ref().map(|(rank, _)| *rank);
                let lexical_score = lanes
                    .lexical
                    .as_ref()
                    .map_or(0.0, |(_, result)| 1.0 - (-result.score.max(0.0)).exp());
                let overlap_score = lanes
                    .overlap
                    .as_ref()
                    .map_or(0.0, |(_, result)| result.combined_score.clamp(0.0, 1.0));
                let rrf_raw = lexical_rank.map_or(0.0, |rank| {
                    options.lexical_weight / (options.reciprocal_rank_k + rank as f32)
                }) + overlap_rank.map_or(0.0, |rank| {
                    options.overlap_weight / (options.reciprocal_rank_k + rank as f32)
                });
                let rrf_max = options.lexical_weight / (options.reciprocal_rank_k + 1.0)
                    + options.overlap_weight / (options.reciprocal_rank_k + 1.0);
                let reciprocal_rank_component = if rrf_max > 0.0 {
                    (rrf_raw / rrf_max).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let weight_sum = options.lexical_weight + options.overlap_weight;
                let evidence_component = if weight_sum > 0.0 {
                    (options.lexical_weight * lexical_score
                        + options.overlap_weight * overlap_score)
                        / weight_sum
                } else {
                    0.0
                };
                let cross_lane_support = lanes.lexical.is_some() && lanes.overlap.is_some();
                let support_bonus = if cross_lane_support { 0.05 } else { 0.0 };
                let score = (0.45 * reciprocal_rank_component
                    + 0.55 * evidence_component
                    + support_bonus)
                    .clamp(0.0, 1.0);
                let snippet = lanes
                    .lexical
                    .as_ref()
                    .map(|(_, result)| result.snippet.clone())
                    .or_else(|| {
                        lanes
                            .overlap
                            .as_ref()
                            .map(|(_, result)| compact(&result.matched_text, 420))
                    })
                    .unwrap_or_default();
                Some(HybridSearchResult {
                    document_id,
                    external_id: document.external_id.clone(),
                    title: document.title.clone(),
                    route: HybridRoute::Hybrid,
                    score,
                    snippet,
                    tags: document.tags.clone(),
                    metadata: document.metadata.clone(),
                    lexical: lanes.lexical.map(|(_, result)| result),
                    overlap: lanes.overlap.map(|(_, result)| result),
                    composite: None,
                    explanation: HybridScoreExplanation {
                        lexical_rank,
                        overlap_rank,
                        lexical_normalized_score: lexical_score,
                        overlap_score,
                        reciprocal_rank_component,
                        evidence_component,
                        cross_lane_support,
                        lexical_weight: options.lexical_weight,
                        overlap_weight: options.overlap_weight,
                    },
                })
            })
            .collect()
    }

    fn from_lexical(
        &self,
        result: LexicalSearchResult,
        rank: usize,
        options: &HybridSearchOptions,
        route: HybridRoute,
    ) -> Option<HybridSearchResult> {
        let document = self.lexical.documents().get(result.document_id as usize)?;
        if !passes_filters(document, options) {
            return None;
        }
        let normalized = 1.0 - (-result.score.max(0.0)).exp();
        Some(HybridSearchResult {
            document_id: result.document_id,
            external_id: result.external_id.clone(),
            title: result.title.clone(),
            route,
            score: normalized,
            snippet: result.snippet.clone(),
            tags: document.tags.clone(),
            metadata: document.metadata.clone(),
            lexical: Some(result),
            overlap: None,
            composite: None,
            explanation: HybridScoreExplanation {
                lexical_rank: Some(rank),
                lexical_normalized_score: normalized,
                lexical_weight: options.lexical_weight,
                ..HybridScoreExplanation::default()
            },
        })
    }

    fn from_overlap(
        &self,
        result: SearchResult,
        rank: usize,
        options: &HybridSearchOptions,
        route: HybridRoute,
    ) -> Option<HybridSearchResult> {
        let document = self.lexical.documents().get(result.document_id as usize)?;
        if !passes_filters(document, options) {
            return None;
        }
        let score = result.combined_score.clamp(0.0, 1.0);
        Some(HybridSearchResult {
            document_id: result.document_id,
            external_id: document.external_id.clone(),
            title: document.title.clone(),
            route,
            score,
            snippet: compact(&result.matched_text, 420),
            tags: document.tags.clone(),
            metadata: document.metadata.clone(),
            lexical: None,
            overlap: Some(result),
            composite: None,
            explanation: HybridScoreExplanation {
                overlap_rank: Some(rank),
                overlap_score: score,
                overlap_weight: options.overlap_weight,
                ..HybridScoreExplanation::default()
            },
        })
    }

    fn from_composite(
        &self,
        result: CompositeSearchResult,
        rank: usize,
        options: &HybridSearchOptions,
    ) -> Option<HybridSearchResult> {
        let document = self.lexical.documents().get(result.document_id as usize)?;
        if !passes_filters(document, options) {
            return None;
        }
        let score = result.aggregate_score.clamp(0.0, 1.0);
        let snippet = result
            .blocks
            .first()
            .map(|block| compact(&block.matched_text, 420))
            .unwrap_or_default();
        Some(HybridSearchResult {
            document_id: result.document_id,
            external_id: document.external_id.clone(),
            title: document.title.clone(),
            route: HybridRoute::Composite,
            score,
            snippet,
            tags: document.tags.clone(),
            metadata: document.metadata.clone(),
            lexical: None,
            overlap: None,
            composite: Some(result),
            explanation: HybridScoreExplanation {
                overlap_rank: Some(rank),
                overlap_score: score,
                overlap_weight: options.overlap_weight,
                ..HybridScoreExplanation::default()
            },
        })
    }
}

fn passes_filters(document: &crate::LexicalDocument, options: &HybridSearchOptions) -> bool {
    if options
        .required_metadata
        .iter()
        .any(|(key, value)| document.metadata.get(key) != Some(value))
    {
        return false;
    }
    let tags = document
        .tags
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect::<BTreeSet<_>>();
    if options
        .required_tags
        .iter()
        .any(|tag| !tags.contains(&tag.to_lowercase()))
    {
        return false;
    }
    !options
        .excluded_tags
        .iter()
        .any(|tag| tags.contains(&tag.to_lowercase()))
}

fn validate_filename(filename: &str) -> Result<()> {
    let path = Path::new(filename);
    if filename.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || filename == "."
        || filename == ".."
    {
        return Err(FoError::InvalidIndex(format!(
            "invalid hybrid component filename {filename:?}"
        )));
    }
    Ok(())
}

fn compact(value: &str, maximum_characters: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_characters {
        return compact;
    }
    let mut output = compact
        .chars()
        .take(maximum_characters)
        .collect::<String>();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        HybridDocumentInput, HybridIndex, HybridIndexBuilder, HybridIndexConfig, HybridRoute,
        HybridSearchMode, HybridSearchOptions,
    };

    fn fixture() -> HybridIndex {
        let mut builder =
            HybridIndexBuilder::new(HybridIndexConfig::default()).expect("builder");
        builder
            .add_document(HybridDocumentInput {
                external_id: "observatory".to_owned(),
                title: "Copper Shutter Observatory".to_owned(),
                body: "The observatory opened its copper shutters before dawn and calibrated every detector before publishing the raw observations.".to_owned(),
                tags: vec!["astronomy".to_owned()],
                metadata: BTreeMap::from([("year".to_owned(), "2026".to_owned())]),
            })
            .expect("document");
        builder
            .add_document(HybridDocumentInput {
                external_id: "finance".to_owned(),
                title: "Liquidity Risk Review".to_owned(),
                body: "The portfolio review measured issuer liquidity risk and covenant exposure.".to_owned(),
                tags: vec!["finance".to_owned()],
                metadata: BTreeMap::from([("year".to_owned(), "2025".to_owned())]),
            })
            .expect("document");
        builder.build().expect("index")
    }

    #[test]
    fn auto_routes_short_queries_to_lexical_search() {
        let report = fixture()
            .search_text("issuer liquidity", &HybridSearchOptions::default())
            .expect("search");
        assert_eq!(report.analysis.route, HybridRoute::Lexical);
        assert_eq!(report.results[0].external_id, "finance");
    }

    #[test]
    fn auto_routes_long_specimens_to_overlap() {
        let specimen = "The observatory opened its copper shutters before dawn and calibrated every detector before publishing the raw observations. This sentence adds enough surrounding language to make the specimen a passage rather than a keyword query and continues with several ordinary words about measurement, evidence, and careful review.";
        let report = fixture()
            .search_text(
                specimen,
                &HybridSearchOptions {
                    auto_overlap_min_words: 20,
                    auto_composite_min_words: 80,
                    ..HybridSearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.analysis.route, HybridRoute::Overlap);
        assert_eq!(report.results[0].external_id, "observatory");
    }

    #[test]
    fn hybrid_route_rewards_cross_lane_support() {
        let report = fixture()
            .search_text(
                "copper shutters calibrated detector observatory",
                &HybridSearchOptions {
                    mode: HybridSearchMode::Hybrid,
                    ..HybridSearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.analysis.route, HybridRoute::Hybrid);
        assert_eq!(report.results[0].external_id, "observatory");
        assert!(report.results[0].explanation.cross_lane_support);
    }

    #[test]
    fn filters_apply_to_every_route() {
        let report = fixture()
            .search_text(
                "risk liquidity",
                &HybridSearchOptions {
                    required_metadata: BTreeMap::from([(
                        "year".to_owned(),
                        "2025".to_owned(),
                    )]),
                    required_tags: vec!["finance".to_owned()],
                    ..HybridSearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].external_id, "finance");
    }

    #[test]
    fn persistence_preserves_document_identity() {
        let index = fixture();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("franken-overlap-{nonce}.fohybrid"));
        index.save(&path).expect("save");
        let loaded = HybridIndex::load(&path).expect("load");
        fs::remove_dir_all(path).ok();
        let report = loaded
            .search_text("issuer liquidity", &HybridSearchOptions::default())
            .expect("search");
        assert_eq!(report.results[0].external_id, "finance");
    }
}
