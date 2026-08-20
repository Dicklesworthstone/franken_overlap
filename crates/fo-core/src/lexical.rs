use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::{FoError, Result};

pub const LEXICAL_INDEX_SCHEMA_VERSION: u32 = 1;

const FIELD_TITLE: u8 = 1;
const FIELD_BODY: u8 = 1 << 1;
const FIELD_TAGS: u8 = 1 << 2;
const FIELD_ALL: u8 = FIELD_TITLE | FIELD_BODY | FIELD_TAGS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LexicalField {
    Any,
    Title,
    Body,
    Tags,
}

impl LexicalField {
    const fn mask(self) -> u8 {
        match self {
            Self::Any => FIELD_ALL,
            Self::Title => FIELD_TITLE,
            Self::Body => FIELD_BODY,
            Self::Tags => FIELD_TAGS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LexicalOccur {
    Should,
    Must,
    MustNot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalClause {
    pub occur: LexicalOccur,
    pub field: LexicalField,
    pub terms: Vec<String>,
    pub phrase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalQuery {
    pub original: String,
    pub clauses: Vec<LexicalClause>,
}

impl LexicalQuery {
    pub fn parse(input: &str) -> Result<Self> {
        let mut cursor = 0usize;
        let mut clauses = Vec::new();
        while cursor < input.len() {
            skip_whitespace(input, &mut cursor);
            if cursor >= input.len() {
                break;
            }
            let occur = match input.as_bytes()[cursor] {
                b'+' => {
                    cursor += 1;
                    LexicalOccur::Must
                }
                b'-' => {
                    cursor += 1;
                    LexicalOccur::MustNot
                }
                _ => LexicalOccur::Should,
            };
            skip_whitespace(input, &mut cursor);
            if cursor >= input.len() {
                break;
            }
            let field = parse_field_prefix(input, &mut cursor);
            let phrase = input.as_bytes().get(cursor) == Some(&b'"');
            let raw = if phrase {
                cursor += 1;
                let start = cursor;
                while cursor < input.len() && input.as_bytes()[cursor] != b'"' {
                    cursor += next_char_len(input, cursor);
                }
                let value = &input[start..cursor.min(input.len())];
                if cursor < input.len() {
                    cursor += 1;
                }
                value
            } else {
                let start = cursor;
                while cursor < input.len() {
                    let character = input[cursor..].chars().next().expect("character boundary");
                    if character.is_whitespace() {
                        break;
                    }
                    cursor += character.len_utf8();
                }
                &input[start..cursor]
            };
            let terms = tokenize_query_fragment(raw);
            if terms.is_empty() {
                continue;
            }
            clauses.push(LexicalClause {
                occur,
                field,
                phrase: phrase && terms.len() > 1,
                terms,
            });
        }
        if clauses.is_empty()
            || clauses
                .iter()
                .all(|clause| clause.occur == LexicalOccur::MustNot)
        {
            return Err(FoError::InvalidConfig(
                "lexical query must contain at least one positive term".to_owned(),
            ));
        }
        Ok(Self {
            original: input.to_owned(),
            clauses,
        })
    }

    #[must_use]
    pub fn positive_terms(&self) -> BTreeSet<&str> {
        self.clauses
            .iter()
            .filter(|clause| clause.occur != LexicalOccur::MustNot)
            .flat_map(|clause| clause.terms.iter().map(String::as_str))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LexicalIndexConfig {
    pub k1: f32,
    pub length_normalization: f32,
    pub title_weight: f32,
    pub body_weight: f32,
    pub tags_weight: f32,
}

impl Default for LexicalIndexConfig {
    fn default() -> Self {
        Self {
            k1: 1.2,
            length_normalization: 0.75,
            title_weight: 2.5,
            body_weight: 1.0,
            tags_weight: 3.0,
        }
    }
}

impl LexicalIndexConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.k1.is_finite() || self.k1 <= 0.0 || self.k1 > 10.0 {
            return Err(FoError::InvalidConfig(
                "lexical k1 must lie in (0, 10]".to_owned(),
            ));
        }
        if !self.length_normalization.is_finite()
            || !(0.0..=1.0).contains(&self.length_normalization)
        {
            return Err(FoError::InvalidConfig(
                "lexical length_normalization must lie in [0, 1]".to_owned(),
            ));
        }
        for (name, weight) in [
            ("title_weight", self.title_weight),
            ("body_weight", self.body_weight),
            ("tags_weight", self.tags_weight),
        ] {
            if !weight.is_finite() || weight <= 0.0 || weight > 100.0 {
                return Err(FoError::InvalidConfig(format!(
                    "lexical {name} must lie in (0, 100]"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LexicalSearchOptions {
    pub max_results: usize,
    pub max_candidate_documents: usize,
    pub candidate_term_limit: usize,
    pub maximum_postings_per_term: usize,
    pub minimum_should_match: f32,
    pub phrase_boost: f32,
    pub proximity_boost: f32,
    pub coverage_boost: f32,
    pub proximity_window: usize,
    pub snippet_words: usize,
    pub require_phrases: bool,
    pub minimum_score: f32,
}

impl Default for LexicalSearchOptions {
    fn default() -> Self {
        Self {
            max_results: 20,
            max_candidate_documents: 50_000,
            candidate_term_limit: 8,
            maximum_postings_per_term: 1_000_000,
            minimum_should_match: 0.0,
            phrase_boost: 2.0,
            proximity_boost: 1.25,
            coverage_boost: 0.75,
            proximity_window: 64,
            snippet_words: 48,
            require_phrases: false,
            minimum_score: 0.0,
        }
    }
}

impl LexicalSearchOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0
            || self.max_candidate_documents == 0
            || self.candidate_term_limit == 0
            || self.maximum_postings_per_term == 0
            || self.proximity_window == 0
            || self.snippet_words == 0
        {
            return Err(FoError::InvalidConfig(
                "lexical result, candidate, posting, proximity, and snippet limits must be positive"
                    .to_owned(),
            ));
        }
        if self.max_results > self.max_candidate_documents {
            return Err(FoError::InvalidConfig(
                "lexical max_results must not exceed max_candidate_documents".to_owned(),
            ));
        }
        for (name, value) in [
            ("minimum_should_match", self.minimum_should_match),
            ("minimum_score", self.minimum_score),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(FoError::InvalidConfig(format!(
                    "lexical {name} must lie in [0, 1]"
                )));
            }
        }
        for (name, value) in [
            ("phrase_boost", self.phrase_boost),
            ("proximity_boost", self.proximity_boost),
            ("coverage_boost", self.coverage_boost),
        ] {
            if !value.is_finite() || value < 0.0 || value > 100.0 {
                return Err(FoError::InvalidConfig(format!(
                    "lexical {name} must lie in [0, 100]"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalDocumentInput {
    pub external_id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalByteSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalDocument {
    pub document_id: u32,
    pub external_id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub title_length: u32,
    pub body_length: u32,
    pub tags_length: u32,
    body_word_spans: Vec<LexicalByteSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalPosting {
    pub document_id: u32,
    pub title_positions: Vec<u32>,
    pub body_positions: Vec<u32>,
    pub tag_positions: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalTermEntry {
    pub term: String,
    pub document_frequency: u32,
    pub postings: Vec<LexicalPosting>,
}

impl LexicalTermEntry {
    #[must_use]
    pub fn posting(&self, document_id: u32) -> Option<&LexicalPosting> {
        self.postings
            .binary_search_by_key(&document_id, |posting| posting.document_id)
            .ok()
            .map(|index| &self.postings[index])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalIndex {
    pub schema_version: u32,
    pub config: LexicalIndexConfig,
    documents: Vec<LexicalDocument>,
    terms: Vec<LexicalTermEntry>,
    average_title_length: f32,
    average_body_length: f32,
    average_tags_length: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalIndexStats {
    pub documents: usize,
    pub distinct_terms: usize,
    pub postings: usize,
    pub title_tokens: u64,
    pub body_tokens: u64,
    pub tag_tokens: u64,
    pub average_title_length: f32,
    pub average_body_length: f32,
    pub average_tags_length: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LexicalScoreExplanation {
    pub title_bm25: f32,
    pub body_bm25: f32,
    pub tags_bm25: f32,
    pub phrase_boost: f32,
    pub proximity_boost: f32,
    pub coverage_boost: f32,
    pub matched_terms: usize,
    pub total_positive_terms: usize,
    pub matched_should_clauses: usize,
    pub total_should_clauses: usize,
    pub exact_phrase_matches: usize,
    pub minimum_proximity_span: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalSearchResult {
    pub document_id: u32,
    pub external_id: String,
    pub title: String,
    pub score: f32,
    pub matched_terms: Vec<String>,
    pub matched_fields: Vec<LexicalField>,
    pub snippet: String,
    pub metadata: BTreeMap<String, String>,
    pub explanation: LexicalScoreExplanation,
}

#[derive(Debug)]
pub struct LexicalIndexBuilder {
    config: LexicalIndexConfig,
    inputs: Vec<LexicalDocumentInput>,
}

impl LexicalIndexBuilder {
    pub fn new(config: LexicalIndexConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            inputs: Vec::new(),
        })
    }

    pub fn add_document(&mut self, document: LexicalDocumentInput) -> Result<u32> {
        if document.external_id.trim().is_empty() {
            return Err(FoError::InvalidConfig(
                "lexical external_id must not be empty".to_owned(),
            ));
        }
        if self.inputs.len() >= u32::MAX as usize {
            return Err(FoError::TooManyDocuments);
        }
        if self
            .inputs
            .iter()
            .any(|existing| existing.external_id == document.external_id)
        {
            return Err(FoError::InvalidConfig(format!(
                "duplicate lexical external_id {}",
                document.external_id
            )));
        }
        let document_id = self.inputs.len() as u32;
        self.inputs.push(document);
        Ok(document_id)
    }

    pub fn build(self) -> Result<LexicalIndex> {
        let mut documents = Vec::with_capacity(self.inputs.len());
        let mut dictionary = BTreeMap::<String, BTreeMap<u32, PostingBuilder>>::new();
        let mut title_total = 0u64;
        let mut body_total = 0u64;
        let mut tags_total = 0u64;

        for (index, input) in self.inputs.into_iter().enumerate() {
            let document_id = u32::try_from(index).map_err(|_| FoError::TooManyDocuments)?;
            let title_tokens = tokenize_field(&input.title)?;
            let body_tokens = tokenize_field(&input.body)?;
            let tags_text = input.tags.join(" ");
            let tag_tokens = tokenize_field(&tags_text)?;
            add_field_postings(
                &mut dictionary,
                document_id,
                &title_tokens,
                LexicalField::Title,
            );
            add_field_postings(
                &mut dictionary,
                document_id,
                &body_tokens,
                LexicalField::Body,
            );
            add_field_postings(
                &mut dictionary,
                document_id,
                &tag_tokens,
                LexicalField::Tags,
            );
            let title_length = checked_u32(title_tokens.len(), "title token count")?;
            let body_length = checked_u32(body_tokens.len(), "body token count")?;
            let tags_length = checked_u32(tag_tokens.len(), "tag token count")?;
            title_total = title_total.saturating_add(u64::from(title_length));
            body_total = body_total.saturating_add(u64::from(body_length));
            tags_total = tags_total.saturating_add(u64::from(tags_length));
            documents.push(LexicalDocument {
                document_id,
                external_id: input.external_id,
                title: input.title,
                body: input.body,
                tags: input.tags,
                metadata: input.metadata,
                title_length,
                body_length,
                tags_length,
                body_word_spans: body_tokens
                    .iter()
                    .map(|token| LexicalByteSpan {
                        start: token.byte_start,
                        end: token.byte_end,
                    })
                    .collect(),
            });
        }

        let terms = dictionary
            .into_iter()
            .map(|(term, postings)| LexicalTermEntry {
                document_frequency: postings.len() as u32,
                term,
                postings: postings
                    .into_iter()
                    .map(|(document_id, posting)| LexicalPosting {
                        document_id,
                        title_positions: posting.title_positions,
                        body_positions: posting.body_positions,
                        tag_positions: posting.tag_positions,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let denominator = documents.len().max(1) as f32;
        let index = LexicalIndex {
            schema_version: LEXICAL_INDEX_SCHEMA_VERSION,
            config: self.config,
            documents,
            terms,
            average_title_length: title_total as f32 / denominator,
            average_body_length: body_total as f32 / denominator,
            average_tags_length: tags_total as f32 / denominator,
        };
        index.validate()?;
        Ok(index)
    }
}

impl LexicalIndex {
    #[must_use]
    pub fn documents(&self) -> &[LexicalDocument] {
        &self.documents
    }

    #[must_use]
    pub fn terms(&self) -> &[LexicalTermEntry] {
        &self.terms
    }

    #[must_use]
    pub fn stats(&self) -> LexicalIndexStats {
        LexicalIndexStats {
            documents: self.documents.len(),
            distinct_terms: self.terms.len(),
            postings: self.terms.iter().map(|term| term.postings.len()).sum(),
            title_tokens: self
                .documents
                .iter()
                .map(|document| u64::from(document.title_length))
                .sum(),
            body_tokens: self
                .documents
                .iter()
                .map(|document| u64::from(document.body_length))
                .sum(),
            tag_tokens: self
                .documents
                .iter()
                .map(|document| u64::from(document.tags_length))
                .sum(),
            average_title_length: self.average_title_length,
            average_body_length: self.average_body_length,
            average_tags_length: self.average_tags_length,
        }
    }

    #[must_use]
    pub fn lookup(&self, term: &str) -> Option<&LexicalTermEntry> {
        self.terms
            .binary_search_by(|entry| entry.term.as_str().cmp(term))
            .ok()
            .map(|index| &self.terms[index])
    }

    pub fn search_text(
        &self,
        query: &str,
        options: &LexicalSearchOptions,
    ) -> Result<Vec<LexicalSearchResult>> {
        let query = LexicalQuery::parse(query)?;
        self.search(&query, options)
    }

    pub fn search(
        &self,
        query: &LexicalQuery,
        options: &LexicalSearchOptions,
    ) -> Result<Vec<LexicalSearchResult>> {
        self.validate()?;
        options.validate()?;
        let term_specs = build_query_term_specs(query);
        let mut candidate_terms = term_specs
            .iter()
            .filter_map(|spec| {
                let entry = self.lookup(&spec.term)?;
                (entry.postings.len() <= options.maximum_postings_per_term).then_some((spec, entry))
            })
            .collect::<Vec<_>>();
        candidate_terms.sort_unstable_by(|(left_spec, left), (right_spec, right)| {
            left.document_frequency
                .cmp(&right.document_frequency)
                .then_with(|| right_spec.count.cmp(&left_spec.count))
                .then_with(|| left.term.cmp(&right.term))
        });
        if candidate_terms.is_empty() {
            return Ok(Vec::new());
        }

        let document_count = self.documents.len().max(1) as f32;
        let mut preliminary = HashMap::<u32, f32>::new();
        for (spec, entry) in candidate_terms.iter().take(options.candidate_term_limit) {
            let idf = inverse_document_frequency(document_count, entry.document_frequency as f32);
            for posting in &entry.postings {
                if posting_has_mask(posting, spec.field_mask) {
                    *preliminary.entry(posting.document_id).or_default() +=
                        idf * query_frequency_weight(spec.count);
                }
            }
        }
        let mut candidates = preliminary.into_iter().collect::<Vec<_>>();
        candidates.sort_unstable_by(|(left_id, left_score), (right_id, right_score)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left_id.cmp(right_id))
        });
        candidates.truncate(options.max_candidate_documents);

        let positive_terms = query.positive_terms();
        let total_positive_terms = positive_terms.len().max(1);
        let total_should_clauses = query
            .clauses
            .iter()
            .filter(|clause| clause.occur == LexicalOccur::Should)
            .count();
        let required_should = ((total_should_clauses as f32 * options.minimum_should_match).ceil()
            as usize)
            .min(total_should_clauses);
        let mut results = Vec::with_capacity(candidates.len().min(options.max_results * 4));

        for (document_id, _) in candidates {
            let Some(document) = self.documents.get(document_id as usize) else {
                continue;
            };
            if query
                .clauses
                .iter()
                .filter(|clause| clause.occur == LexicalOccur::MustNot)
                .any(|clause| self.clause_matches(document_id, clause))
            {
                continue;
            }
            if query
                .clauses
                .iter()
                .filter(|clause| clause.occur == LexicalOccur::Must)
                .any(|clause| !self.clause_matches(document_id, clause))
            {
                continue;
            }
            if options.require_phrases
                && query
                    .clauses
                    .iter()
                    .filter(|clause| clause.occur != LexicalOccur::MustNot && clause.phrase)
                    .any(|clause| !self.clause_matches(document_id, clause))
            {
                continue;
            }
            let matched_should_clauses = query
                .clauses
                .iter()
                .filter(|clause| clause.occur == LexicalOccur::Should)
                .filter(|clause| self.clause_matches(document_id, clause))
                .count();
            if matched_should_clauses < required_should {
                continue;
            }

            let mut explanation = LexicalScoreExplanation {
                total_positive_terms,
                matched_should_clauses,
                total_should_clauses,
                ..LexicalScoreExplanation::default()
            };
            let mut matched_terms = Vec::new();
            let mut matched_fields = BTreeSet::new();
            let mut best_body_position = None;
            let mut best_body_idf = -1.0f32;

            for spec in &term_specs {
                let Some(entry) = self.lookup(&spec.term) else {
                    continue;
                };
                let Some(posting) = entry.posting(document_id) else {
                    continue;
                };
                let idf =
                    inverse_document_frequency(document_count, entry.document_frequency as f32)
                        * query_frequency_weight(spec.count);
                let mut matched = false;
                if spec.field_mask & FIELD_TITLE != 0 && !posting.title_positions.is_empty() {
                    let contribution = bm25_field(
                        posting.title_positions.len(),
                        document.title_length as usize,
                        self.average_title_length,
                        self.config.k1,
                        self.config.length_normalization,
                    ) * idf
                        * self.config.title_weight;
                    explanation.title_bm25 += contribution;
                    matched_fields.insert(LexicalField::Title);
                    matched = true;
                }
                if spec.field_mask & FIELD_BODY != 0 && !posting.body_positions.is_empty() {
                    let contribution = bm25_field(
                        posting.body_positions.len(),
                        document.body_length as usize,
                        self.average_body_length,
                        self.config.k1,
                        self.config.length_normalization,
                    ) * idf
                        * self.config.body_weight;
                    explanation.body_bm25 += contribution;
                    matched_fields.insert(LexicalField::Body);
                    matched = true;
                    if idf > best_body_idf {
                        best_body_idf = idf;
                        best_body_position = posting.body_positions.first().copied();
                    }
                }
                if spec.field_mask & FIELD_TAGS != 0 && !posting.tag_positions.is_empty() {
                    let contribution = bm25_field(
                        posting.tag_positions.len(),
                        document.tags_length as usize,
                        self.average_tags_length,
                        self.config.k1,
                        self.config.length_normalization,
                    ) * idf
                        * self.config.tags_weight;
                    explanation.tags_bm25 += contribution;
                    matched_fields.insert(LexicalField::Tags);
                    matched = true;
                }
                if matched {
                    matched_terms.push(spec.term.clone());
                }
            }
            matched_terms.sort_unstable();
            matched_terms.dedup();
            explanation.matched_terms = matched_terms.len();
            if matched_terms.is_empty() {
                continue;
            }

            for clause in query
                .clauses
                .iter()
                .filter(|clause| clause.occur != LexicalOccur::MustNot && clause.phrase)
            {
                let occurrences = self.phrase_occurrences(document_id, clause);
                if occurrences > 0 {
                    explanation.exact_phrase_matches += occurrences;
                    let phrase_idf = clause
                        .terms
                        .iter()
                        .filter_map(|term| self.lookup(term))
                        .map(|entry| {
                            inverse_document_frequency(
                                document_count,
                                entry.document_frequency as f32,
                            )
                        })
                        .sum::<f32>()
                        / clause.terms.len().max(1) as f32;
                    explanation.phrase_boost +=
                        options.phrase_boost * (1.0 + phrase_idf) * (occurrences as f32).ln_1p();
                }
            }

            if let Some(span) = self.minimum_proximity_span(document_id, &term_specs) {
                explanation.minimum_proximity_span = Some(span);
                if span <= options.proximity_window {
                    let tightness = term_specs.len().max(1) as f32 / span.max(1) as f32;
                    explanation.proximity_boost = options.proximity_boost * tightness.min(1.0);
                }
            }
            explanation.coverage_boost = options.coverage_boost * explanation.matched_terms as f32
                / total_positive_terms as f32;
            let score = explanation.title_bm25
                + explanation.body_bm25
                + explanation.tags_bm25
                + explanation.phrase_boost
                + explanation.proximity_boost
                + explanation.coverage_boost;
            let normalized_floor_score = 1.0 - (-score.max(0.0)).exp();
            if normalized_floor_score < options.minimum_score {
                continue;
            }
            results.push(LexicalSearchResult {
                document_id,
                external_id: document.external_id.clone(),
                title: document.title.clone(),
                score,
                matched_terms,
                matched_fields: matched_fields.into_iter().collect(),
                snippet: document.snippet(best_body_position, options.snippet_words),
                metadata: document.metadata.clone(),
                explanation,
            });
        }

        results.sort_unstable_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.document_id.cmp(&right.document_id))
        });
        results.truncate(options.max_results);
        Ok(results)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| FoError::io(parent, error))?;
        }
        let bytes = serde_json::to_vec(self).map_err(|error| {
            FoError::InvalidConfig(format!("could not serialize lexical index: {error}"))
        })?;
        atomic_write(path, &bytes)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|error| FoError::io(path, error))?;
        let index = serde_json::from_slice::<Self>(&bytes).map_err(|error| {
            FoError::InvalidIndex(format!("invalid lexical index JSON: {error}"))
        })?;
        index.validate()?;
        Ok(index)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LEXICAL_INDEX_SCHEMA_VERSION {
            return Err(FoError::InvalidIndex(format!(
                "unsupported lexical schema version {}",
                self.schema_version
            )));
        }
        self.config.validate()?;
        for (expected, document) in self.documents.iter().enumerate() {
            if document.document_id as usize != expected {
                return Err(FoError::InvalidIndex(format!(
                    "lexical document id {} appears at position {expected}",
                    document.document_id
                )));
            }
            if document.body_word_spans.len() != document.body_length as usize {
                return Err(FoError::InvalidIndex(format!(
                    "lexical document {} has {} body spans for {} tokens",
                    document.document_id,
                    document.body_word_spans.len(),
                    document.body_length
                )));
            }
            let mut previous_end = 0u32;
            for span in &document.body_word_spans {
                if span.start > span.end
                    || span.end as usize > document.body.len()
                    || span.start < previous_end
                    || document
                        .body
                        .get(span.start as usize..span.end as usize)
                        .is_none()
                {
                    return Err(FoError::InvalidIndex(format!(
                        "lexical document {} contains an invalid body span",
                        document.document_id
                    )));
                }
                previous_end = span.end;
            }
        }
        let mut previous_term: Option<&str> = None;
        for entry in &self.terms {
            if entry.term.is_empty()
                || previous_term.is_some_and(|previous| previous >= entry.term.as_str())
            {
                return Err(FoError::InvalidIndex(
                    "lexical term dictionary is not strictly sorted".to_owned(),
                ));
            }
            previous_term = Some(&entry.term);
            if entry.document_frequency as usize != entry.postings.len()
                || entry.postings.is_empty()
            {
                return Err(FoError::InvalidIndex(format!(
                    "lexical term {} has inconsistent document frequency",
                    entry.term
                )));
            }
            let mut previous_document = None;
            for posting in &entry.postings {
                if previous_document.is_some_and(|previous| previous >= posting.document_id) {
                    return Err(FoError::InvalidIndex(format!(
                        "lexical postings for {} are not strictly sorted",
                        entry.term
                    )));
                }
                previous_document = Some(posting.document_id);
                let Some(document) = self.documents.get(posting.document_id as usize) else {
                    return Err(FoError::InvalidIndex(format!(
                        "lexical posting references missing document {}",
                        posting.document_id
                    )));
                };
                validate_positions(&posting.title_positions, document.title_length, &entry.term)?;
                validate_positions(&posting.body_positions, document.body_length, &entry.term)?;
                validate_positions(&posting.tag_positions, document.tags_length, &entry.term)?;
            }
        }
        for (name, value) in [
            ("average_title_length", self.average_title_length),
            ("average_body_length", self.average_body_length),
            ("average_tags_length", self.average_tags_length),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(FoError::InvalidIndex(format!("lexical {name} is invalid")));
            }
        }
        Ok(())
    }

    fn clause_matches(&self, document_id: u32, clause: &LexicalClause) -> bool {
        if clause.phrase {
            return self.phrase_occurrences(document_id, clause) > 0;
        }
        clause.terms.iter().all(|term| {
            self.lookup(term)
                .and_then(|entry| entry.posting(document_id))
                .is_some_and(|posting| posting_has_mask(posting, clause.field.mask()))
        })
    }

    fn phrase_occurrences(&self, document_id: u32, clause: &LexicalClause) -> usize {
        if clause.terms.len() < 2 {
            return usize::from(self.clause_matches(
                document_id,
                &LexicalClause {
                    phrase: false,
                    ..clause.clone()
                },
            ));
        }
        let postings = clause
            .terms
            .iter()
            .map(|term| {
                self.lookup(term)
                    .and_then(|entry| entry.posting(document_id))
            })
            .collect::<Option<Vec<_>>>();
        let Some(postings) = postings else {
            return 0;
        };
        let mut total = 0usize;
        for field in fields_for_mask(clause.field.mask()) {
            let position_lists = postings
                .iter()
                .map(|posting| positions_for_field(posting, field))
                .collect::<Vec<_>>();
            if position_lists.iter().any(|positions| positions.is_empty()) {
                continue;
            }
            for &start in position_lists[0] {
                if position_lists
                    .iter()
                    .enumerate()
                    .skip(1)
                    .all(|(offset, positions)| {
                        positions
                            .binary_search(&start.saturating_add(offset as u32))
                            .is_ok()
                    })
                {
                    total += 1;
                }
            }
        }
        total
    }

    fn minimum_proximity_span(
        &self,
        document_id: u32,
        term_specs: &[QueryTermSpec],
    ) -> Option<usize> {
        if term_specs.len() < 2 || term_specs.len() > 64 {
            return None;
        }
        let mut best = None;
        for field in [LexicalField::Title, LexicalField::Body, LexicalField::Tags] {
            let mut events = Vec::<(u32, usize)>::new();
            for (term_index, spec) in term_specs.iter().enumerate() {
                if spec.field_mask & field.mask() == 0 {
                    continue;
                }
                let posting = self
                    .lookup(&spec.term)
                    .and_then(|entry| entry.posting(document_id));
                let Some(posting) = posting else {
                    continue;
                };
                events.extend(
                    positions_for_field(posting, field)
                        .iter()
                        .copied()
                        .map(|position| (position, term_index)),
                );
            }
            if events.is_empty() {
                continue;
            }
            events.sort_unstable();
            let mut counts = vec![0usize; term_specs.len()];
            let mut covered = 0usize;
            let mut left = 0usize;
            for right in 0..events.len() {
                let term = events[right].1;
                if counts[term] == 0 {
                    covered += 1;
                }
                counts[term] += 1;
                while covered == term_specs.len() && left <= right {
                    let span = events[right].0.saturating_sub(events[left].0) as usize + 1;
                    best = Some(best.map_or(span, |current: usize| current.min(span)));
                    let left_term = events[left].1;
                    counts[left_term] -= 1;
                    if counts[left_term] == 0 {
                        covered -= 1;
                    }
                    left += 1;
                }
            }
        }
        best
    }
}

impl LexicalDocument {
    fn snippet(&self, preferred_position: Option<u32>, words: usize) -> String {
        if self.body_word_spans.is_empty() {
            return compact_snippet(&self.title, words.saturating_mul(12));
        }
        let center = preferred_position
            .map(|position| position as usize)
            .unwrap_or(0)
            .min(self.body_word_spans.len() - 1);
        let half = words / 2;
        let start_word = center.saturating_sub(half);
        let end_word = start_word
            .saturating_add(words)
            .min(self.body_word_spans.len());
        let start = self.body_word_spans[start_word].start as usize;
        let end = self.body_word_spans[end_word - 1].end as usize;
        compact_snippet(
            self.body.get(start..end).unwrap_or_default(),
            words.saturating_mul(12),
        )
    }
}

#[derive(Debug, Clone)]
struct TokenizedTerm {
    term: String,
    position: u32,
    byte_start: u32,
    byte_end: u32,
}

#[derive(Debug, Default)]
struct PostingBuilder {
    title_positions: Vec<u32>,
    body_positions: Vec<u32>,
    tag_positions: Vec<u32>,
}

#[derive(Debug, Clone)]
struct QueryTermSpec {
    term: String,
    field_mask: u8,
    count: usize,
}

fn build_query_term_specs(query: &LexicalQuery) -> Vec<QueryTermSpec> {
    let mut specs = BTreeMap::<String, (u8, usize)>::new();
    for clause in query
        .clauses
        .iter()
        .filter(|clause| clause.occur != LexicalOccur::MustNot)
    {
        for term in &clause.terms {
            let entry = specs.entry(term.clone()).or_default();
            entry.0 |= clause.field.mask();
            entry.1 += 1;
        }
    }
    specs
        .into_iter()
        .map(|(term, (field_mask, count))| QueryTermSpec {
            term,
            field_mask,
            count,
        })
        .collect()
}

fn tokenize_field(text: &str) -> Result<Vec<TokenizedTerm>> {
    if text.len() > u32::MAX as usize {
        return Err(FoError::InvalidConfig(
            "lexical field exceeds the u32 byte-offset limit".to_owned(),
        ));
    }
    let mut tokens = Vec::new();
    for (byte_start, word) in text.unicode_word_indices() {
        let term = normalize_term(word);
        if term.is_empty() {
            continue;
        }
        let position = checked_u32(tokens.len(), "lexical token position")?;
        tokens.push(TokenizedTerm {
            term,
            position,
            byte_start: checked_u32(byte_start, "lexical byte start")?,
            byte_end: checked_u32(byte_start + word.len(), "lexical byte end")?,
        });
    }
    Ok(tokens)
}

fn tokenize_query_fragment(text: &str) -> Vec<String> {
    text.unicode_words()
        .map(normalize_term)
        .filter(|term| !term.is_empty())
        .collect()
}

fn normalize_term(term: &str) -> String {
    term.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric() || *character == '_')
        .collect()
}

fn add_field_postings(
    dictionary: &mut BTreeMap<String, BTreeMap<u32, PostingBuilder>>,
    document_id: u32,
    tokens: &[TokenizedTerm],
    field: LexicalField,
) {
    for token in tokens {
        let posting = dictionary
            .entry(token.term.clone())
            .or_default()
            .entry(document_id)
            .or_default();
        match field {
            LexicalField::Title => posting.title_positions.push(token.position),
            LexicalField::Body => posting.body_positions.push(token.position),
            LexicalField::Tags => posting.tag_positions.push(token.position),
            LexicalField::Any => unreachable!("concrete index field required"),
        }
    }
}

fn bm25_field(
    term_frequency: usize,
    document_length: usize,
    average_length: f32,
    k1: f32,
    b: f32,
) -> f32 {
    if term_frequency == 0 {
        return 0.0;
    }
    let tf = term_frequency as f32;
    let average = average_length.max(1.0);
    let normalization = 1.0 - b + b * document_length as f32 / average;
    tf * (k1 + 1.0) / (tf + k1 * normalization)
}

fn inverse_document_frequency(document_count: f32, document_frequency: f32) -> f32 {
    (1.0 + (document_count - document_frequency + 0.5) / (document_frequency + 0.5)).ln()
}

fn query_frequency_weight(count: usize) -> f32 {
    1.0 + (count.max(1) as f32).ln()
}

fn posting_has_mask(posting: &LexicalPosting, mask: u8) -> bool {
    (mask & FIELD_TITLE != 0 && !posting.title_positions.is_empty())
        || (mask & FIELD_BODY != 0 && !posting.body_positions.is_empty())
        || (mask & FIELD_TAGS != 0 && !posting.tag_positions.is_empty())
}

fn fields_for_mask(mask: u8) -> impl Iterator<Item = LexicalField> {
    [LexicalField::Title, LexicalField::Body, LexicalField::Tags]
        .into_iter()
        .filter(move |field| mask & field.mask() != 0)
}

fn positions_for_field(posting: &LexicalPosting, field: LexicalField) -> &[u32] {
    match field {
        LexicalField::Title => &posting.title_positions,
        LexicalField::Body => &posting.body_positions,
        LexicalField::Tags => &posting.tag_positions,
        LexicalField::Any => &[],
    }
}

fn validate_positions(positions: &[u32], length: u32, term: &str) -> Result<()> {
    let mut previous = None;
    for &position in positions {
        if position >= length || previous.is_some_and(|prior| prior >= position) {
            return Err(FoError::InvalidIndex(format!(
                "lexical positions for term {term} are invalid"
            )));
        }
        previous = Some(position);
    }
    Ok(())
}

fn checked_u32(value: usize, name: &str) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| FoError::InvalidConfig(format!("{name} exceeds the u32 limit")))
}

fn skip_whitespace(input: &str, cursor: &mut usize) {
    while *cursor < input.len() {
        let character = input[*cursor..].chars().next().expect("character boundary");
        if !character.is_whitespace() {
            break;
        }
        *cursor += character.len_utf8();
    }
}

fn parse_field_prefix(input: &str, cursor: &mut usize) -> LexicalField {
    let start = *cursor;
    let mut probe = start;
    while probe < input.len() {
        let character = input[probe..].chars().next().expect("character boundary");
        if character == ':' {
            let prefix = input[start..probe].to_ascii_lowercase();
            let field = match prefix.as_str() {
                "title" => Some(LexicalField::Title),
                "body" => Some(LexicalField::Body),
                "tag" | "tags" => Some(LexicalField::Tags),
                _ => None,
            };
            if let Some(field) = field {
                *cursor = probe + 1;
                return field;
            }
            break;
        }
        if character.is_whitespace() || character == '"' || matches!(character, '+' | '-') {
            break;
        }
        probe += character.len_utf8();
    }
    LexicalField::Any
}

fn next_char_len(input: &str, cursor: usize) -> usize {
    input[cursor..].chars().next().map_or(1, char::len_utf8)
}

fn compact_snippet(text: &str, maximum_characters: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_characters {
        return compact;
    }
    let mut output = compact.chars().take(maximum_characters).collect::<String>();
    output.push('…');
    output
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary = PathBuf::from(path);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("folex");
    temporary.set_extension(format!("{extension}.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| FoError::io(&temporary, error))?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            fs::remove_file(path).map_err(|remove_error| FoError::io(path, remove_error))?;
            fs::rename(&temporary, path).map_err(|rename_error| FoError::io(path, rename_error))
        }
        Err(error) => Err(FoError::io(path, error)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        LexicalDocumentInput, LexicalIndex, LexicalIndexBuilder, LexicalIndexConfig, LexicalQuery,
        LexicalSearchOptions,
    };

    fn document(id: &str, title: &str, body: &str, tags: &[&str]) -> LexicalDocumentInput {
        LexicalDocumentInput {
            external_id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            metadata: BTreeMap::new(),
        }
    }

    fn fixture() -> LexicalIndex {
        let mut builder = LexicalIndexBuilder::new(LexicalIndexConfig::default()).expect("builder");
        builder
            .add_document(document(
                "observatory",
                "Copper Shutter Observatory",
                "The observatory opened the copper shutters before dawn and calibrated every detector.",
                &["astronomy", "instrumentation"],
            ))
            .expect("document");
        builder
            .add_document(document(
                "cooking",
                "Copper Cookware",
                "The kitchen displayed copper pans before the winter festival began.",
                &["cooking"],
            ))
            .expect("document");
        builder
            .add_document(document(
                "finance",
                "Market Risk Review",
                "The portfolio review measured liquidity risk and issuer concentration.",
                &["finance", "risk"],
            ))
            .expect("document");
        builder.build().expect("index")
    }

    #[test]
    fn exact_phrase_ranks_above_bag_of_words() {
        let results = fixture()
            .search_text("\"copper shutters\"", &LexicalSearchOptions::default())
            .expect("search");
        assert_eq!(results[0].external_id, "observatory");
        assert!(results[0].explanation.exact_phrase_matches > 0);
    }

    #[test]
    fn field_scope_and_negative_clause_work() {
        let results = fixture()
            .search_text(
                "+title:copper -body:kitchen",
                &LexicalSearchOptions::default(),
            )
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].external_id, "observatory");
    }

    #[test]
    fn rare_term_identifies_the_source() {
        let results = fixture()
            .search_text(
                "detector calibration observatory",
                &LexicalSearchOptions::default(),
            )
            .expect("search");
        assert_eq!(results[0].external_id, "observatory");
        assert!(results[0].snippet.contains("observatory"));
    }

    #[test]
    fn persistence_preserves_results() {
        let index = fixture();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("franken-overlap-{nonce}.folex"));
        index.save(&path).expect("save");
        let loaded = LexicalIndex::load(&path).expect("load");
        fs::remove_file(path).ok();
        let options = LexicalSearchOptions::default();
        let before = index
            .search_text("liquidity issuer", &options)
            .expect("before");
        let after = loaded
            .search_text("liquidity issuer", &options)
            .expect("after");
        assert_eq!(before[0].external_id, after[0].external_id);
        assert_eq!(before[0].score, after[0].score);
    }

    #[test]
    fn parses_phrases_and_fields() {
        let query =
            LexicalQuery::parse("+title:\"market risk\" -tag:cooking issuer").expect("query");
        assert_eq!(query.clauses.len(), 3);
        assert!(query.clauses[0].phrase);
    }
}
