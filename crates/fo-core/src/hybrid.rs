use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    AdaptiveMatch, CompositeSearchOptions, FoError, Index, IndexBuilder, IndexConfig,
    LexicalDocumentInput, LexicalIndex, LexicalIndexBuilder, LexicalIndexConfig, LexicalQuery,
    LexicalSearchOptions, LexicalSearchResult, QueryPlan, QueryPlannerOptions, Result,
    SearchOptions, SearchResult,
};

pub const HYBRID_INDEX_SCHEMA_VERSION: u32 = 1;
const HYBRID_MANIFEST_FILENAME: &str = "manifest.json";
const HYBRID_OVERLAP_FILENAME: &str = "overlap.foidx";
const HYBRID_LEXICAL_FILENAME: &str = "lexical.folex";

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
    ids: BTreeSet<String>,
}

impl HybridIndexBuilder {
    pub fn new(config: HybridIndexConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            documents: Vec::new(),
            ids: BTreeSet::new(),
        })
    }

    pub fn add_document(&mut self, document: HybridDocumentInput) -> Result<u32> {
        if document.external_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "hybrid external_id must not be empty".to_owned(),
            ));
        }
        if !self.ids.insert(document.external_id.clone()) {
            return Err(FoError::InvalidConfig(format!(
                "duplicate hybrid external_id {}",
                document.external_id
            )));
        }
        if self.documents.len() >= u32::MAX as usize {
            return Err(FoError::TooManyDocuments);
        }
        let id = self.documents.len() as u32;
        self.documents.push(document);
        Ok(id)
    }

    pub fn build(self) -> Result<HybridIndex> {
        let mut overlap_builder = IndexBuilder::new(self.config.overlap.clone())?;
        let mut lexical_builder = LexicalIndexBuilder::new(self.config.lexical.clone())?;
        for document in self.documents {
            overlap_builder.add_document(document.external_id.clone(), &document.body)?;
            lexical_builder.add_document(LexicalDocumentInput {
                external_id: document.external_id,
                title: document.title,
                body: document.body,
                tags: document.tags,
                metadata: document.metadata,
            })?;
        }
        let index = HybridIndex {
            schema_version: HYBRID_INDEX_SCHEMA_VERSION,
            config: self.config,
            overlap: overlap_builder.build()?,
            lexical: lexical_builder.build()?,
        };
        index.validate()?;
        Ok(index)
    }
}

#[derive(Debug, Clone)]
pub struct HybridIndex {
    schema_version: u32,
    config: HybridIndexConfig,
    overlap: Index,
    lexical: LexicalIndex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HybridManifest {
    schema_version: u32,
    config: HybridIndexConfig,
    overlap_file: String,
    lexical_file: String,
    external_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridIndexStats {
    pub schema_version: u32,
    pub documents: usize,
    pub overlap: crate::IndexStats,
    pub lexical: crate::LexicalIndexStats,
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
            schema_version: self.schema_version,
            documents: self.lexical.documents().len(),
            overlap: self.overlap.stats(),
            lexical: self.lexical.stats(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != HYBRID_INDEX_SCHEMA_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "unsupported hybrid index schema {}",
                self.schema_version
            )));
        }
        self.config.validate()?;
        if self.overlap.config != self.config.overlap {
            return Err(FoError::InvalidIndex(
                "hybrid overlap configuration disagrees with manifest".to_owned(),
            ));
        }
        if self.lexical.config != self.config.lexical {
            return Err(FoError::InvalidIndex(
                "hybrid lexical configuration disagrees with manifest".to_owned(),
            ));
        }
        if self.overlap.documents().len() != self.lexical.documents().len() {
            return Err(FoError::InvalidIndex(
                "hybrid component document counts disagree".to_owned(),
            ));
        }
        for (overlap, lexical) in self
            .overlap
            .documents()
            .iter()
            .zip(self.lexical.documents())
        {
            if overlap.id != lexical.document_id || overlap.path != lexical.external_id {
                return Err(FoError::InvalidIndex(format!(
                    "hybrid document identity disagreement at overlap id {}",
                    overlap.id
                )));
            }
        }
        Ok(())
    }

    pub fn save(&self, directory: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let directory = directory.as_ref();
        fs::create_dir_all(directory).map_err(|error| FoError::io(directory, error))?;
        self.overlap
            .save_compressed(directory.join(HYBRID_OVERLAP_FILENAME))?;
        self.lexical
            .save(directory.join(HYBRID_LEXICAL_FILENAME))?;
        let manifest = HybridManifest {
            schema_version: self.schema_version,
            config: self.config.clone(),
            overlap_file: HYBRID_OVERLAP_FILENAME.to_owned(),
            lexical_file: HYBRID_LEXICAL_FILENAME.to_owned(),
            external_ids: self
                .lexical
                .documents()
                .iter()
                .map(|document| document.external_id.clone())
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            FoError::InvalidConfig(format!("could not serialize hybrid manifest: {error}"))
        })?;
        atomic_write(&directory.join(HYBRID_MANIFEST_FILENAME), &bytes)
    }

    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        let manifest_path = directory.join(HYBRID_MANIFEST_FILENAME);
        let bytes = fs::read(&manifest_path).map_err(|error| FoError::io(&manifest_path, error))?;
        let manifest = serde_json::from_slice::<HybridManifest>(&bytes).map_err(|error| {
            FoError::InvalidIndex(format!("invalid hybrid manifest: {error}"))
        })?;
        if manifest.schema_version != HYBRID_INDEX_SCHEMA_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "unsupported hybrid manifest schema {}",
                manifest.schema_version
            )));
        }
        validate_component_filename(&manifest.overlap_file)?;
        validate_component_filename(&manifest.lexical_file)?;
        let overlap = Index::load_auto(directory.join(&manifest.overlap_file))?;
        let lexical = LexicalIndex::load(directory.join(&manifest.lexical_file))?;
        let index = Self {
            schema_version: manifest.schema_version,
            config: manifest.config,
            overlap,
            lexical,
        };
        index.validate()?;
        let observed = index
            .lexical
            .documents()
            .iter()
            .map(|document| document.external_id.as_str())
            .collect::<Vec<_>>();
        let expected = manifest
            .external_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if observed != expected {
            return Err(FoError::InvalidIndex(
                "hybrid manifest external IDs disagree with component indexes".to_owned(),
            ));
        }
        Ok(index)
    }

    pub fn search(
        &self,
        query_text: &str,
        options: &HybridSearchOptions,
    ) -> Result<HybridSearchReport> {
        self.validate()?;
        options.validate()?;
        let lexical_query = LexicalQuery::parse(query_text)?;
        let positive_terms = lexical_query.positive_terms().len();
        let positive_term_occurrences = lexical_query
            .clauses
            .iter()
            .filter(|clause| clause.occur != crate::LexicalOccur::MustNot)
            .map(|clause| clause.terms.len())
            .sum::<usize>();
        let selected_mode = select_mode(query_text, positive_term_occurrences, options.mode);
        let overlap_specimen = overlap_specimen(&lexical_query);
        let expanded_limit = options
            .max_results
            .saturating_mul(options.candidate_multiplier)
            .max(options.max_results);

        let mut lexical_results = Vec::new();
        if selected_mode != HybridQueryMode::Overlap {
            let mut lexical_options = options.lexical.clone();
            lexical_options.max_results = expanded_limit;
            lexical_options.max_candidate_documents = lexical_options
                .max_candidate_documents
                .max(expanded_limit);
            lexical_results = self.lexical.search(&lexical_query, &lexical_options)?;
        }

        let mut overlap_plan = None;
        let mut overlap_results = Vec::new();
        if selected_mode != HybridQueryMode::Lexical {
            let mut overlap_options = options.overlap.clone();
            overlap_options.max_results = expanded_limit;
            overlap_options.max_candidates = overlap_options
                .max_candidates
                .max(expanded_limit.saturating_mul(4));
            overlap_options.minimum_similarity = overlap_options
                .minimum_similarity
                .min(options.overlap_candidate_floor);
            let report = self.overlap.search_adaptive(
                &overlap_specimen,
                &overlap_options,
                options.planner,
                options.composite,
            )?;
            overlap_plan = Some(report.plan);
            overlap_results = report.matches;
        }

        let mut candidates = BTreeMap::<String, HybridCandidate>::new();
        for (offset, result) in lexical_results.iter().cloned().enumerate() {
            let entry = candidates
                .entry(result.external_id.clone())
                .or_insert_with(|| HybridCandidate::new(result.external_id.clone()));
            entry.lexical = Some((offset + 1, result));
        }
        for (offset, evidence) in overlap_results.iter().cloned().enumerate() {
            let evidence = HybridOverlapEvidence::from(evidence);
            let external_id = overlap_external_id(&evidence).to_owned();
            let entry = candidates
                .entry(external_id.clone())
                .or_insert_with(|| HybridCandidate::new(external_id));
            entry.overlap = Some((offset + 1, evidence));
        }

        let mut results = Vec::with_capacity(candidates.len());
        for candidate in candidates.into_values() {
            let Some(document) = self.lexical_document(&candidate.external_id) else {
                continue;
            };
            if !options.filter.matches(document) {
                continue;
            }
            let lexical_rank = candidate.lexical.as_ref().map(|(rank, _)| *rank);
            let overlap_rank = candidate.overlap.as_ref().map(|(rank, _)| *rank);
            let lexical_raw_score = candidate
                .lexical
                .as_ref()
                .map_or(0.0, |(_, result)| result.score.max(0.0));
            let lexical_score = 1.0 - (-lexical_raw_score / options.lexical_saturation).exp();
            let overlap_score = candidate
                .overlap
                .as_ref()
                .map_or(0.0, |(_, evidence)| overlap_score(evidence));
            let reciprocal_rank_score =
                reciprocal_rank_fusion(lexical_rank, overlap_rank, options.rrf_constant);
            let phrase_signal = candidate
                .lexical
                .as_ref()
                .map_or(0.0, |(_, result)| {
                    (result.explanation.exact_phrase_matches as f32).ln_1p() / 2.0f32.ln()
                })
                .clamp(0.0, 1.0);
            let agreement = candidate.lexical.is_some() && candidate.overlap.is_some();
            let weighted_sum = options.lexical_weight * lexical_score
                + options.overlap_weight * overlap_score
                + options.rrf_weight * reciprocal_rank_score;
            let active_weight = (candidate.lexical.is_some() as u8 as f32)
                * options.lexical_weight
                + (candidate.overlap.is_some() as u8 as f32) * options.overlap_weight
                + options.rrf_weight;
            let base_score = if active_weight > 0.0 {
                weighted_sum / active_weight
            } else {
                0.0
            };
            let agreement_bonus = if agreement {
                options.agreement_bonus
            } else {
                0.0
            };
            let phrase_bonus = options.phrase_bonus * phrase_signal;
            let bonus = (agreement_bonus + phrase_bonus).clamp(0.0, 0.75);
            let final_score =
                (1.0 - (1.0 - base_score.clamp(0.0, 1.0)) * (1.0 - bonus)).clamp(0.0, 1.0);
            if final_score < options.minimum_score {
                continue;
            }
            let snippet = candidate
                .lexical
                .as_ref()
                .map(|(_, result)| result.snippet.clone())
                .or_else(|| {
                    candidate
                        .overlap
                        .as_ref()
                        .map(|(_, evidence)| overlap_snippet(evidence))
                })
                .unwrap_or_default();
            results.push(HybridSearchResult {
                external_id: candidate.external_id,
                title: document.title.clone(),
                score: final_score,
                snippet,
                tags: document.tags.clone(),
                metadata: document.metadata.clone(),
                lexical: candidate.lexical.map(|(_, result)| result),
                overlap: candidate.overlap.map(|(_, evidence)| evidence),
                explanation: HybridScoreExplanation {
                    selected_mode,
                    lexical_rank,
                    overlap_rank,
                    lexical_raw_score,
                    lexical_score,
                    overlap_score,
                    reciprocal_rank_score,
                    agreement,
                    agreement_bonus,
                    phrase_signal,
                    phrase_bonus,
                    base_score,
                    final_score,
                },
            });
        }
        results.sort_unstable_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.external_id.cmp(&right.external_id))
        });
        results.truncate(options.max_results);
        Ok(HybridSearchReport {
            requested_mode: options.mode,
            selected_mode,
            positive_terms,
            positive_term_occurrences,
            overlap_plan,
            lexical_candidates: lexical_results.len(),
            overlap_candidates: overlap_results.len(),
            results,
        })
    }

    fn lexical_document(&self, external_id: &str) -> Option<&crate::LexicalDocument> {
        self.lexical
            .documents()
            .iter()
            .find(|document| document.external_id == external_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HybridQueryMode {
    Auto,
    Hybrid,
    Overlap,
    Lexical,
}

impl Default for HybridQueryMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HybridFilter {
    pub external_id_prefix: Option<String>,
    pub required_tags: Vec<String>,
    pub metadata_equals: BTreeMap<String, String>,
}

impl Default for HybridFilter {
    fn default() -> Self {
        Self {
            external_id_prefix: None,
            required_tags: Vec::new(),
            metadata_equals: BTreeMap::new(),
        }
    }
}

impl HybridFilter {
    fn validate(&self) -> Result<()> {
        if self
            .external_id_prefix
            .as_ref()
            .is_some_and(|prefix| prefix.is_empty())
        {
            return Err(FoError::InvalidConfig(
                "hybrid external_id_prefix must not be empty".to_owned(),
            ));
        }
        if self.required_tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(FoError::InvalidConfig(
                "hybrid required tags must not be empty".to_owned(),
            ));
        }
        Ok(())
    }

    fn matches(&self, document: &crate::LexicalDocument) -> bool {
        if self
            .external_id_prefix
            .as_ref()
            .is_some_and(|prefix| !document.external_id.starts_with(prefix))
        {
            return false;
        }
        if self.required_tags.iter().any(|required| {
            !document
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(required))
        }) {
            return false;
        }
        self.metadata_equals.iter().all(|(key, value)| {
            document
                .metadata
                .get(key)
                .is_some_and(|observed| observed == value)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HybridSearchOptions {
    pub mode: HybridQueryMode,
    pub max_results: usize,
    pub candidate_multiplier: usize,
    pub lexical: LexicalSearchOptions,
    pub overlap: SearchOptions,
    pub planner: QueryPlannerOptions,
    pub composite: CompositeSearchOptions,
    pub lexical_weight: f32,
    pub overlap_weight: f32,
    pub rrf_weight: f32,
    pub rrf_constant: f32,
    pub lexical_saturation: f32,
    pub agreement_bonus: f32,
    pub phrase_bonus: f32,
    pub overlap_candidate_floor: f32,
    pub minimum_score: f32,
    pub filter: HybridFilter,
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            mode: HybridQueryMode::Auto,
            max_results: 20,
            candidate_multiplier: 8,
            lexical: LexicalSearchOptions::default(),
            overlap: SearchOptions {
                minimum_similarity: 0.20,
                ..SearchOptions::default()
            },
            planner: QueryPlannerOptions::default(),
            composite: CompositeSearchOptions::default(),
            lexical_weight: 0.40,
            overlap_weight: 0.45,
            rrf_weight: 0.15,
            rrf_constant: 60.0,
            lexical_saturation: 4.0,
            agreement_bonus: 0.12,
            phrase_bonus: 0.08,
            overlap_candidate_floor: 0.10,
            minimum_score: 0.0,
            filter: HybridFilter::default(),
        }
    }
}

impl HybridSearchOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0 || self.candidate_multiplier == 0 {
            return Err(FoError::InvalidConfig(
                "hybrid result and candidate limits must be positive".to_owned(),
            ));
        }
        self.lexical.validate()?;
        self.overlap.validate()?;
        self.planner.validate()?;
        self.composite.validate()?;
        self.filter.validate()?;
        for (name, value) in [
            ("lexical_weight", self.lexical_weight),
            ("overlap_weight", self.overlap_weight),
            ("rrf_weight", self.rrf_weight),
            ("agreement_bonus", self.agreement_bonus),
            ("phrase_bonus", self.phrase_bonus),
        ] {
            if !value.is_finite() || value < 0.0 || value > 10.0 {
                return Err(FoError::InvalidConfig(format!(
                    "hybrid {name} must lie in [0, 10]"
                )));
            }
        }
        if self.lexical_weight + self.overlap_weight + self.rrf_weight <= 0.0 {
            return Err(FoError::InvalidConfig(
                "at least one hybrid scoring weight must be positive".to_owned(),
            ));
        }
        if !self.rrf_constant.is_finite() || self.rrf_constant <= 0.0 {
            return Err(FoError::InvalidConfig(
                "hybrid rrf_constant must be positive".to_owned(),
            ));
        }
        if !self.lexical_saturation.is_finite() || self.lexical_saturation <= 0.0 {
            return Err(FoError::InvalidConfig(
                "hybrid lexical_saturation must be positive".to_owned(),
            ));
        }
        for (name, value) in [
            ("overlap_candidate_floor", self.overlap_candidate_floor),
            ("minimum_score", self.minimum_score),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "hybrid {name} must lie in [0, 1]"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "evidence", rename_all = "snake_case")]
pub enum HybridOverlapEvidence {
    Passage(SearchResult),
    Composite(crate::CompositeSearchResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridScoreExplanation {
    pub selected_mode: HybridQueryMode,
    pub lexical_rank: Option<usize>,
    pub overlap_rank: Option<usize>,
    pub lexical_raw_score: f32,
    pub lexical_score: f32,
    pub overlap_score: f32,
    pub reciprocal_rank_score: f32,
    pub agreement: bool,
    pub agreement_bonus: f32,
    pub phrase_signal: f32,
    pub phrase_bonus: f32,
    pub base_score: f32,
    pub final_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResult {
    pub external_id: String,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub lexical: Option<LexicalSearchResult>,
    pub overlap: Option<HybridOverlapEvidence>,
    pub explanation: HybridScoreExplanation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchReport {
    pub requested_mode: HybridQueryMode,
    pub selected_mode: HybridQueryMode,
    pub positive_terms: usize,
    pub positive_term_occurrences: usize,
    pub overlap_plan: Option<QueryPlan>,
    pub lexical_candidates: usize,
    pub overlap_candidates: usize,
    pub results: Vec<HybridSearchResult>,
}

#[derive(Debug)]
struct HybridCandidate {
    external_id: String,
    lexical: Option<(usize, LexicalSearchResult)>,
    overlap: Option<(usize, HybridOverlapEvidence)>,
}

impl HybridCandidate {
    fn new(external_id: String) -> Self {
        Self {
            external_id,
            lexical: None,
            overlap: None,
        }
    }
}

fn select_mode(query: &str, positive_terms: usize, requested: HybridQueryMode) -> HybridQueryMode {
    if requested != HybridQueryMode::Auto {
        return requested;
    }
    let has_lexical_syntax = query.contains('"')
        || query.split_whitespace().any(|token| {
            token.starts_with('+')
                || token.starts_with('-')
                || token.starts_with("title:")
                || token.starts_with("body:")
                || token.starts_with("tag:")
        });
    if has_lexical_syntax || positive_terms <= 8 {
        HybridQueryMode::Lexical
    } else if positive_terms >= 160 {
        HybridQueryMode::Overlap
    } else {
        HybridQueryMode::Hybrid
    }
}

fn overlap_specimen(query: &LexicalQuery) -> String {
    query
        .clauses
        .iter()
        .filter(|clause| clause.occur != crate::LexicalOccur::MustNot)
        .flat_map(|clause| clause.terms.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn overlap_external_id(evidence: &HybridOverlapEvidence) -> &str {
    match evidence {
        HybridOverlapEvidence::Passage(result) => &result.path,
        HybridOverlapEvidence::Composite(result) => &result.path,
    }
}

fn overlap_score(evidence: &HybridOverlapEvidence) -> f32 {
    match evidence {
        HybridOverlapEvidence::Passage(result) => result.combined_score,
        HybridOverlapEvidence::Composite(result) => result.aggregate_score,
    }
    .clamp(0.0, 1.0)
}

fn overlap_snippet(evidence: &HybridOverlapEvidence) -> String {
    match evidence {
        HybridOverlapEvidence::Passage(result) => result.matched_text.clone(),
        HybridOverlapEvidence::Composite(result) => result
            .blocks
            .first()
            .map_or_else(String::new, |block| block.matched_text.clone()),
    }
}

fn reciprocal_rank_fusion(
    lexical_rank: Option<usize>,
    overlap_rank: Option<usize>,
    constant: f32,
) -> f32 {
    let score = [lexical_rank, overlap_rank]
        .into_iter()
        .flatten()
        .map(|rank| (constant + 1.0) / (constant + rank as f32))
        .sum::<f32>();
    let lanes = (lexical_rank.is_some() as usize) + (overlap_rank.is_some() as usize);
    if lanes == 0 {
        0.0
    } else {
        (score / lanes as f32).clamp(0.0, 1.0)
    }
}

fn validate_component_filename(filename: &str) -> Result<()> {
    let path = Path::new(filename);
    if filename.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || filename == "."
        || filename == ".."
    {
        return Err(FoError::InvalidIndex(format!(
            "unsafe hybrid component filename {filename:?}"
        )));
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| FoError::io(parent, error))?;
    }
    let mut temporary = PathBuf::from(path);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("tmp");
    temporary.set_extension(format!("{extension}.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| FoError::io(&temporary, error))?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| FoError::io(path, error))?;
    }
    fs::rename(&temporary, path).map_err(|error| FoError::io(path, error))
}

impl From<AdaptiveMatch> for HybridOverlapEvidence {
    fn from(value: AdaptiveMatch) -> Self {
        match value {
            AdaptiveMatch::Passage(result) => Self::Passage(result),
            AdaptiveMatch::Composite(result) => Self::Composite(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        HybridDocumentInput, HybridIndex, HybridIndexBuilder, HybridIndexConfig, HybridQueryMode,
        HybridSearchOptions,
    };

    fn build_index() -> HybridIndex {
        let mut builder = HybridIndexBuilder::new(HybridIndexConfig::default()).expect("builder");
        builder
            .add_document(HybridDocumentInput {
                external_id: "observatory.txt".to_owned(),
                title: "Copper Shutter Observatory".to_owned(),
                body: "Before dawn the observatory opened its copper shutters. The team checked every detector twice and published the raw measurements before comparing causal models.".to_owned(),
                tags: vec!["astronomy".to_owned(), "science".to_owned()],
                metadata: BTreeMap::from([("domain".to_owned(), "science".to_owned())]),
            })
            .expect("document");
        builder
            .add_document(HybridDocumentInput {
                external_id: "kitchen.txt".to_owned(),
                title: "Winter Kitchen".to_owned(),
                body: "The cooks prepared winter vegetables beside a railway timetable and a brass lantern.".to_owned(),
                tags: vec!["cooking".to_owned()],
                metadata: BTreeMap::from([("domain".to_owned(), "food".to_owned())]),
            })
            .expect("document");
        builder.build().expect("index")
    }

    #[test]
    fn auto_uses_lexical_search_for_short_queries() {
        let report = build_index()
            .search(
                "title:observatory detector",
                &HybridSearchOptions::default(),
            )
            .expect("search");
        assert_eq!(report.selected_mode, HybridQueryMode::Lexical);
        assert_eq!(report.results[0].external_id, "observatory.txt");
    }

    #[test]
    fn hybrid_fuses_edited_passage_and_keyword_evidence() {
        let report = build_index()
            .search(
                "the observatory opened copper shutters before sunrise and the team checked every detector twice before publishing raw measurements",
                &HybridSearchOptions {
                    mode: HybridQueryMode::Hybrid,
                    overlap: crate::SearchOptions {
                        minimum_similarity: 0.05,
                        minimum_matched_tokens: 8,
                        ..crate::SearchOptions::default()
                    },
                    ..HybridSearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.results[0].external_id, "observatory.txt");
        assert!(report.results[0].explanation.agreement);
    }

    #[test]
    fn persistence_preserves_results_and_filters() {
        let index = build_index();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "franken-overlap-hybrid-{}-{nonce}",
            std::process::id()
        ));
        index.save(&directory).expect("save");
        let loaded = HybridIndex::load(&directory).expect("load");
        let report = loaded
            .search(
                "winter vegetables",
                &HybridSearchOptions {
                    filter: super::HybridFilter {
                        metadata_equals: BTreeMap::from([(
                            "domain".to_owned(),
                            "food".to_owned(),
                        )]),
                        ..super::HybridFilter::default()
                    },
                    ..HybridSearchOptions::default()
                },
            )
            .expect("search");
        assert_eq!(report.results[0].external_id, "kitchen.txt");
        fs::remove_dir_all(directory).ok();
    }
}
