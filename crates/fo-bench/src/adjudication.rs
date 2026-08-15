use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Component, Path};

use fo_core::{NormalizationProfile, normalize};
use fo_corpus::{CorpusManifest, atomic_write, sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub type AdjudicationResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const ADJUDICATION_SCHEMA_VERSION: u32 = 1;

const METHOD_HYBRID: &str = "franken_hybrid";
const METHOD_OVERLAP: &str = "franken_overlap";
const METHOD_EXHAUSTIVE: &str = "exhaustive_levenshtein";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioQuery {
    pub id: String,
    pub profile: String,
    pub text: String,
    pub positive_ids: Vec<String>,
    pub source_id: String,
    #[serde(default)]
    pub source_title: String,
    #[serde(default)]
    pub relation_key: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
}

impl TokenSpan {
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.start < self.end
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ScoreRow {
    corpus_size: usize,
    query_id: String,
    candidate_id: String,
    candidate_title: String,
    label: bool,
    scores: BTreeMap<String, f64>,
    #[serde(default)]
    predicted_spans: BTreeMap<String, Vec<TokenSpan>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    NaturalRelation,
    TopOneDisagreement,
    GeneratedPositiveMissingFromHybridTopK,
    LowHybridMargin,
    NoHybridResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodLeader {
    pub method: String,
    pub candidate_id: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateReview {
    pub candidate_id: String,
    pub candidate_title: String,
    pub generated_positive: bool,
    pub scores: BTreeMap<String, f64>,
    pub ranks: BTreeMap<String, usize>,
    pub predicted_spans: BTreeMap<String, Vec<TokenSpan>>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionTemplate {
    pub query_id: String,
    pub status: AdjudicationStatus,
    pub positive_ids: Vec<String>,
    pub graded_relevance: BTreeMap<String, u8>,
    pub acceptable_spans: BTreeMap<String, Vec<TokenSpan>>,
    pub reviewer: String,
    pub notes: String,
    pub reviewed_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewTask {
    pub schema_version: u32,
    pub query: ScenarioQuery,
    pub corpus_size: usize,
    pub reasons: Vec<ReviewReason>,
    pub method_leaders: Vec<MethodLeader>,
    pub hybrid_margin: Option<f64>,
    pub candidates: Vec<CandidateReview>,
    pub decision_template: DecisionTemplate,
}

#[derive(Debug, Clone)]
pub struct QueueOptions {
    pub corpus_size: Option<usize>,
    pub top_candidates: usize,
    pub hybrid_top_k: usize,
    pub low_margin: f64,
    pub snippet_tokens: usize,
    pub include_all: bool,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            corpus_size: None,
            top_candidates: 12,
            hybrid_top_k: 5,
            low_margin: 0.05,
            snippet_tokens: 160,
            include_all: false,
        }
    }
}

impl QueueOptions {
    pub fn validate(&self) -> AdjudicationResult<()> {
        if self.top_candidates == 0 || self.hybrid_top_k == 0 || self.snippet_tokens == 0 {
            return Err(invalid("queue candidate, rank, and snippet limits must be positive"));
        }
        if !self.low_margin.is_finite() || self.low_margin < 0.0 || self.low_margin > 1.0 {
            return Err(invalid("queue low-margin threshold must lie in [0, 1]"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_size: usize,
    pub input_queries: usize,
    pub queued_queries: usize,
    pub natural_relation_queries: usize,
    pub disagreement_queries: usize,
    pub query_sha256: String,
    pub score_sha256: String,
    pub output_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjudicationStatus {
    AcceptGenerated,
    Replace,
    Exclude,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjudicationDecision {
    pub query_id: String,
    pub status: AdjudicationStatus,
    #[serde(default)]
    pub positive_ids: Vec<String>,
    #[serde(default)]
    pub graded_relevance: BTreeMap<String, u8>,
    #[serde(default)]
    pub acceptable_spans: BTreeMap<String, Vec<TokenSpan>>,
    pub reviewer: String,
    #[serde(default)]
    pub notes: String,
    pub reviewed_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldAnnotation {
    pub status: String,
    pub reviewer: String,
    pub notes: String,
    pub reviewed_at_unix: u64,
    pub graded_relevance: BTreeMap<String, u8>,
    pub acceptable_spans: BTreeMap<String, Vec<TokenSpan>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldQuery {
    pub id: String,
    pub profile: String,
    pub text: String,
    pub positive_ids: Vec<String>,
    pub source_id: String,
    pub source_title: String,
    pub relation_key: String,
    pub metadata: BTreeMap<String, String>,
    pub gold: GoldAnnotation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub input_queries: usize,
    pub decisions: usize,
    pub accepted_generated: usize,
    pub replaced: usize,
    pub excluded: usize,
    pub retained_controlled_without_review: usize,
    pub output_queries: usize,
    pub input_query_sha256: String,
    pub decision_sha256: String,
    pub output_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldValidationReport {
    pub schema_version: u32,
    pub corpus_id: String,
    pub queries: usize,
    pub controlled_queries: usize,
    pub natural_queries: usize,
    pub multi_positive_queries: usize,
    pub relevance_labels: usize,
    pub acceptable_spans: usize,
    pub query_sha256: String,
}

pub fn create_review_queue(
    corpus_root: &Path,
    query_path: &Path,
    score_path: &Path,
    output_path: &Path,
    options: &QueueOptions,
) -> AdjudicationResult<QueueReport> {
    options.validate()?;
    let manifest = CorpusManifest::load(corpus_root)?;
    let queries = read_jsonl::<ScenarioQuery>(query_path)?;
    validate_scenario_queries(&queries)?;
    let all_rows = read_jsonl::<ScoreRow>(score_path)?;
    if all_rows.is_empty() {
        return Err(invalid("score file contains no rows"));
    }
    let corpus_size = options
        .corpus_size
        .unwrap_or_else(|| all_rows.iter().map(|row| row.corpus_size).max().unwrap_or(0));
    let rows = all_rows
        .into_iter()
        .filter(|row| row.corpus_size == corpus_size)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Err(invalid(format!(
            "score file contains no rows for corpus size {corpus_size}"
        )));
    }
    let grouped = group_score_rows(rows)?;
    let documents = manifest
        .documents
        .iter()
        .map(|document| (document.id.as_str(), document))
        .collect::<BTreeMap<_, _>>();
    let mut tasks = Vec::new();
    let mut natural_relation_queries = 0usize;
    let mut disagreement_queries = 0usize;
    for query in &queries {
        let query_rows = grouped.get(query.id.as_str()).ok_or_else(|| {
            invalid(format!(
                "query {} has no score rows at corpus size {corpus_size}",
                query.id
            ))
        })?;
        let analysis = analyze_query(query, query_rows, options);
        if query.profile == "natural_relation" {
            natural_relation_queries += 1;
        }
        if analysis.reasons.contains(&ReviewReason::TopOneDisagreement) {
            disagreement_queries += 1;
        }
        if !options.include_all && analysis.reasons.is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        for row in select_candidate_rows(query, query_rows, &analysis, options) {
            let document = documents.get(row.candidate_id.as_str()).ok_or_else(|| {
                invalid(format!(
                    "candidate {} is absent from corpus manifest",
                    row.candidate_id
                ))
            })?;
            candidates.push(CandidateReview {
                candidate_id: row.candidate_id.clone(),
                candidate_title: row.candidate_title.clone(),
                generated_positive: row.label,
                scores: row.scores.clone(),
                ranks: analysis
                    .ranks
                    .iter()
                    .filter_map(|(method, ranks)| {
                        ranks
                            .get(row.candidate_id.as_str())
                            .copied()
                            .map(|rank| (method.clone(), rank))
                    })
                    .collect(),
                predicted_spans: row.predicted_spans.clone(),
                snippet: candidate_snippet(
                    corpus_root,
                    document,
                    &row.predicted_spans,
                    options.snippet_tokens,
                )?,
            });
        }
        let graded_relevance = query
            .positive_ids
            .iter()
            .map(|id| (id.clone(), 3))
            .collect();
        tasks.push(ReviewTask {
            schema_version: ADJUDICATION_SCHEMA_VERSION,
            query: query.clone(),
            corpus_size,
            reasons: analysis.reasons,
            method_leaders: analysis.method_leaders,
            hybrid_margin: analysis.hybrid_margin,
            candidates,
            decision_template: DecisionTemplate {
                query_id: query.id.clone(),
                status: AdjudicationStatus::AcceptGenerated,
                positive_ids: query.positive_ids.clone(),
                graded_relevance,
                acceptable_spans: BTreeMap::new(),
                reviewer: String::new(),
                notes: String::new(),
                reviewed_at_unix: 0,
            },
        });
    }
    let output_bytes = write_jsonl_bytes(&tasks)?;
    atomic_write(output_path, &output_bytes)?;
    Ok(QueueReport {
        schema_version: ADJUDICATION_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: manifest.corpus_id,
        corpus_size,
        input_queries: queries.len(),
        queued_queries: tasks.len(),
        natural_relation_queries,
        disagreement_queries,
        query_sha256: sha256_hex(&fs::read(query_path)?),
        score_sha256: sha256_hex(&fs::read(score_path)?),
        output_sha256: sha256_hex(&output_bytes),
    })
}

pub fn apply_decisions(
    query_path: &Path,
    decision_path: &Path,
    output_path: &Path,
    allow_unreviewed_natural: bool,
) -> AdjudicationResult<ApplyReport> {
    let queries = read_jsonl::<ScenarioQuery>(query_path)?;
    validate_scenario_queries(&queries)?;
    let decisions = read_jsonl::<AdjudicationDecision>(decision_path)?;
    let decision_map = validate_decisions(&decisions)?;
    let mut gold = Vec::new();
    let mut accepted_generated = 0usize;
    let mut replaced = 0usize;
    let mut excluded = 0usize;
    let mut retained_controlled_without_review = 0usize;
    for query in &queries {
        match decision_map.get(query.id.as_str()) {
            Some(decision) => match decision.status {
                AdjudicationStatus::Exclude => {
                    excluded += 1;
                }
                AdjudicationStatus::AcceptGenerated => {
                    accepted_generated += 1;
                    gold.push(gold_query(
                        query,
                        query.positive_ids.clone(),
                        decision,
                        "reviewed_accept_generated",
                    )?);
                }
                AdjudicationStatus::Replace => {
                    if decision.positive_ids.is_empty() {
                        return Err(invalid(format!(
                            "replacement decision for {} has no positive_ids",
                            query.id
                        )));
                    }
                    replaced += 1;
                    gold.push(gold_query(
                        query,
                        decision.positive_ids.clone(),
                        decision,
                        "reviewed_replacement",
                    )?);
                }
            },
            None if query.profile == "natural_relation" && !allow_unreviewed_natural => {
                return Err(invalid(format!(
                    "natural-relation query {} has no adjudication decision",
                    query.id
                )));
            }
            None => {
                retained_controlled_without_review += 1;
                let generated = AdjudicationDecision {
                    query_id: query.id.clone(),
                    status: AdjudicationStatus::AcceptGenerated,
                    positive_ids: query.positive_ids.clone(),
                    graded_relevance: query
                        .positive_ids
                        .iter()
                        .map(|id| (id.clone(), 3))
                        .collect(),
                    acceptable_spans: BTreeMap::new(),
                    reviewer: "deterministic_generator".to_owned(),
                    notes: "controlled source provenance retained without human review".to_owned(),
                    reviewed_at_unix: 0,
                };
                gold.push(gold_query(
                    query,
                    query.positive_ids.clone(),
                    &generated,
                    if query.profile == "natural_relation" {
                        "unreviewed_natural_relation"
                    } else {
                        "generated_controlled"
                    },
                )?);
            }
        }
    }
    let output_bytes = write_jsonl_bytes(&gold)?;
    atomic_write(output_path, &output_bytes)?;
    Ok(ApplyReport {
        schema_version: ADJUDICATION_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        input_queries: queries.len(),
        decisions: decisions.len(),
        accepted_generated,
        replaced,
        excluded,
        retained_controlled_without_review,
        output_queries: gold.len(),
        input_query_sha256: sha256_hex(&fs::read(query_path)?),
        decision_sha256: sha256_hex(&fs::read(decision_path)?),
        output_sha256: sha256_hex(&output_bytes),
    })
}

pub fn validate_gold(
    corpus_root: &Path,
    gold_path: &Path,
) -> AdjudicationResult<GoldValidationReport> {
    let manifest = CorpusManifest::load(corpus_root)?;
    let queries = read_jsonl::<GoldQuery>(gold_path)?;
    if queries.is_empty() {
        return Err(invalid("gold query file contains no queries"));
    }
    let documents = manifest
        .documents
        .iter()
        .map(|document| (document.id.as_str(), document))
        .collect::<BTreeMap<_, _>>();
    let mut ids = BTreeSet::new();
    let mut controlled_queries = 0usize;
    let mut natural_queries = 0usize;
    let mut multi_positive_queries = 0usize;
    let mut relevance_labels = 0usize;
    let mut acceptable_spans = 0usize;
    let mut normalized_lengths = BTreeMap::<String, usize>::new();
    for query in &queries {
        if query.id.trim().is_empty()
            || query.text.trim().is_empty()
            || query.positive_ids.is_empty()
            || !ids.insert(query.id.as_str())
        {
            return Err(invalid(format!(
                "gold query {:?} has empty fields, no positives, or a duplicate ID",
                query.id
            )));
        }
        if query.profile == "natural_relation" {
            natural_queries += 1;
        } else {
            controlled_queries += 1;
        }
        multi_positive_queries += usize::from(query.positive_ids.len() > 1);
        let positives = query.positive_ids.iter().collect::<BTreeSet<_>>();
        for positive in &query.positive_ids {
            if !documents.contains_key(positive.as_str()) {
                return Err(invalid(format!(
                    "gold query {} references missing positive document {positive}",
                    query.id
                )));
            }
            let grade = query.gold.graded_relevance.get(positive).copied().unwrap_or(3);
            if !(1..=3).contains(&grade) {
                return Err(invalid(format!(
                    "gold query {} has invalid relevance grade {grade} for {positive}",
                    query.id
                )));
            }
            relevance_labels += 1;
        }
        if query
            .gold
            .graded_relevance
            .keys()
            .any(|id| !positives.contains(id))
        {
            return Err(invalid(format!(
                "gold query {} grades a document outside positive_ids",
                query.id
            )));
        }
        for (document_id, spans) in &query.gold.acceptable_spans {
            if !positives.contains(document_id) || spans.is_empty() {
                return Err(invalid(format!(
                    "gold query {} has spans for a non-positive document or an empty span list",
                    query.id
                )));
            }
            let length = match normalized_lengths.get(document_id) {
                Some(length) => *length,
                None => {
                    let record = documents[document_id.as_str()];
                    validate_relative_path(&record.relative_path)?;
                    let body = fs::read_to_string(corpus_root.join(&record.relative_path))?;
                    let length = normalize(&body, &NormalizationProfile::default()).len();
                    normalized_lengths.insert(document_id.clone(), length);
                    length
                }
            };
            for span in spans {
                if !span.is_valid() || span.end > length {
                    return Err(invalid(format!(
                        "gold query {} has out-of-range span {:?} for {} tokens in {document_id}",
                        query.id, span, length
                    )));
                }
                acceptable_spans += 1;
            }
        }
    }
    Ok(GoldValidationReport {
        schema_version: ADJUDICATION_SCHEMA_VERSION,
        corpus_id: manifest.corpus_id,
        queries: queries.len(),
        controlled_queries,
        natural_queries,
        multi_positive_queries,
        relevance_labels,
        acceptable_spans,
        query_sha256: sha256_hex(&fs::read(gold_path)?),
    })
}

#[derive(Debug)]
struct QueryAnalysis {
    reasons: Vec<ReviewReason>,
    method_leaders: Vec<MethodLeader>,
    ranks: BTreeMap<String, BTreeMap<String, usize>>,
    hybrid_margin: Option<f64>,
}

fn analyze_query(
    query: &ScenarioQuery,
    rows: &[ScoreRow],
    options: &QueueOptions,
) -> QueryAnalysis {
    let methods = rows
        .iter()
        .flat_map(|row| row.scores.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mut method_leaders = Vec::new();
    let mut ranks = BTreeMap::new();
    for method in methods {
        let mut ranked = rows.iter().collect::<Vec<_>>();
        ranked.sort_unstable_by(|left, right| {
            score_for(right, &method)
                .total_cmp(&score_for(left, &method))
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        if let Some(leader) = ranked.first() {
            method_leaders.push(MethodLeader {
                method: method.clone(),
                candidate_id: leader.candidate_id.clone(),
                score: score_for(leader, &method),
            });
        }
        ranks.insert(
            method,
            ranked
                .into_iter()
                .enumerate()
                .map(|(index, row)| (row.candidate_id.clone(), index + 1))
                .collect(),
        );
    }
    method_leaders.sort_unstable_by(|left, right| left.method.cmp(&right.method));
    let leader_ids = method_leaders
        .iter()
        .map(|leader| leader.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut reasons = BTreeSet::new();
    if query.profile == "natural_relation" {
        reasons.insert(ReviewReason::NaturalRelation);
    }
    if leader_ids.len() > 1 {
        reasons.insert(ReviewReason::TopOneDisagreement);
    }
    let hybrid_ranked = rank_rows(rows, METHOD_HYBRID);
    if hybrid_ranked.is_empty() || score_for(hybrid_ranked[0], METHOD_HYBRID) <= 0.0 {
        reasons.insert(ReviewReason::NoHybridResult);
    }
    if !query.positive_ids.iter().any(|positive| {
        hybrid_ranked
            .iter()
            .take(options.hybrid_top_k)
            .any(|row| &row.candidate_id == positive)
    }) {
        reasons.insert(ReviewReason::GeneratedPositiveMissingFromHybridTopK);
    }
    let hybrid_margin = hybrid_ranked.first().map(|first| {
        score_for(first, METHOD_HYBRID)
            - hybrid_ranked
                .get(1)
                .map_or(0.0, |second| score_for(second, METHOD_HYBRID))
    });
    if hybrid_margin.is_some_and(|margin| margin < options.low_margin) {
        reasons.insert(ReviewReason::LowHybridMargin);
    }
    QueryAnalysis {
        reasons: reasons.into_iter().collect(),
        method_leaders,
        ranks,
        hybrid_margin,
    }
}

fn select_candidate_rows<'a>(
    query: &ScenarioQuery,
    rows: &'a [ScoreRow],
    analysis: &QueryAnalysis,
    options: &QueueOptions,
) -> Vec<&'a ScoreRow> {
    let mut ids = query.positive_ids.iter().cloned().collect::<BTreeSet<_>>();
    ids.extend(
        analysis
            .method_leaders
            .iter()
            .map(|leader| leader.candidate_id.clone()),
    );
    ids.extend(
        rank_rows(rows, METHOD_HYBRID)
            .into_iter()
            .take(options.top_candidates)
            .map(|row| row.candidate_id.clone()),
    );
    let mut selected = rows
        .iter()
        .filter(|row| ids.contains(&row.candidate_id))
        .collect::<Vec<_>>();
    selected.sort_unstable_by(|left, right| {
        let left_rank = analysis
            .ranks
            .get(METHOD_HYBRID)
            .and_then(|ranks| ranks.get(&left.candidate_id))
            .copied()
            .unwrap_or(usize::MAX);
        let right_rank = analysis
            .ranks
            .get(METHOD_HYBRID)
            .and_then(|ranks| ranks.get(&right.candidate_id))
            .copied()
            .unwrap_or(usize::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| right.label.cmp(&left.label))
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    selected
}

fn candidate_snippet(
    corpus_root: &Path,
    document: &fo_corpus::CorpusDocument,
    predicted_spans: &BTreeMap<String, Vec<TokenSpan>>,
    snippet_tokens: usize,
) -> AdjudicationResult<String> {
    validate_relative_path(&document.relative_path)?;
    let body = fs::read_to_string(corpus_root.join(&document.relative_path))?;
    let normalized = normalize(&body, &NormalizationProfile::default());
    let span = [METHOD_HYBRID, METHOD_OVERLAP, METHOD_EXHAUSTIVE]
        .into_iter()
        .find_map(|method| predicted_spans.get(method).and_then(|spans| spans.first()))
        .copied();
    let text = match span {
        Some(span) if span.is_valid() => {
            let context = snippet_tokens / 2;
            normalized
                .slice_tokens(
                    span.start.saturating_sub(context),
                    span.end.saturating_add(context).min(normalized.len()),
                )
                .to_owned()
        }
        _ => normalized
            .slice_tokens(0, snippet_tokens.min(normalized.len()))
            .to_owned(),
    };
    Ok(one_line(&text, 1_200))
}

fn gold_query(
    query: &ScenarioQuery,
    mut positive_ids: Vec<String>,
    decision: &AdjudicationDecision,
    status: &str,
) -> AdjudicationResult<GoldQuery> {
    positive_ids.sort_unstable();
    positive_ids.dedup();
    if positive_ids.is_empty() {
        return Err(invalid(format!(
            "gold query {} has no positive IDs",
            query.id
        )));
    }
    let positive_set = positive_ids.iter().collect::<BTreeSet<_>>();
    if decision
        .graded_relevance
        .keys()
        .any(|id| !positive_set.contains(id))
        || decision
            .acceptable_spans
            .keys()
            .any(|id| !positive_set.contains(id))
    {
        return Err(invalid(format!(
            "decision for {} grades or spans a document outside its final positives",
            query.id
        )));
    }
    let mut graded_relevance = decision.graded_relevance.clone();
    for positive in &positive_ids {
        graded_relevance.entry(positive.clone()).or_insert(3);
    }
    if graded_relevance.values().any(|grade| !(1..=3).contains(grade)) {
        return Err(invalid(format!(
            "decision for {} contains a relevance grade outside 1..=3",
            query.id
        )));
    }
    for spans in decision.acceptable_spans.values() {
        if spans.is_empty() || spans.iter().any(|span| !span.is_valid()) {
            return Err(invalid(format!(
                "decision for {} contains an empty or invalid span",
                query.id
            )));
        }
    }
    Ok(GoldQuery {
        id: query.id.clone(),
        profile: query.profile.clone(),
        text: query.text.clone(),
        positive_ids,
        source_id: query.source_id.clone(),
        source_title: query.source_title.clone(),
        relation_key: query.relation_key.clone(),
        metadata: query.metadata.clone(),
        gold: GoldAnnotation {
            status: status.to_owned(),
            reviewer: decision.reviewer.clone(),
            notes: decision.notes.clone(),
            reviewed_at_unix: decision.reviewed_at_unix,
            graded_relevance,
            acceptable_spans: decision.acceptable_spans.clone(),
        },
    })
}

fn validate_scenario_queries(queries: &[ScenarioQuery]) -> AdjudicationResult<()> {
    if queries.is_empty() {
        return Err(invalid("query file contains no queries"));
    }
    let mut ids = BTreeSet::new();
    for query in queries {
        if query.id.trim().is_empty()
            || query.profile.trim().is_empty()
            || query.text.trim().is_empty()
            || query.positive_ids.is_empty()
            || !ids.insert(query.id.as_str())
        {
            return Err(invalid(format!(
                "query {:?} has empty required fields, no positives, or a duplicate ID",
                query.id
            )));
        }
    }
    Ok(())
}

fn validate_decisions(
    decisions: &[AdjudicationDecision],
) -> AdjudicationResult<BTreeMap<&str, &AdjudicationDecision>> {
    let mut map = BTreeMap::new();
    for decision in decisions {
        if decision.query_id.trim().is_empty()
            || decision.reviewer.trim().is_empty()
            || decision.reviewed_at_unix == 0
        {
            return Err(invalid(
                "every decision requires query_id, reviewer, and reviewed_at_unix",
            ));
        }
        if map.insert(decision.query_id.as_str(), decision).is_some() {
            return Err(invalid(format!(
                "duplicate decision for query {}",
                decision.query_id
            )));
        }
    }
    Ok(map)
}

fn group_score_rows(rows: Vec<ScoreRow>) -> AdjudicationResult<BTreeMap<String, Vec<ScoreRow>>> {
    let mut grouped = BTreeMap::<String, Vec<ScoreRow>>::new();
    let mut keys = BTreeSet::new();
    for row in rows {
        if row.query_id.trim().is_empty()
            || row.candidate_id.trim().is_empty()
            || row.scores.values().any(|score| !score.is_finite())
            || !keys.insert((row.query_id.clone(), row.candidate_id.clone()))
        {
            return Err(invalid(
                "score rows contain empty IDs, non-finite scores, or duplicate query/candidate pairs",
            ));
        }
        grouped.entry(row.query_id.clone()).or_default().push(row);
    }
    Ok(grouped)
}

fn rank_rows<'a>(rows: &'a [ScoreRow], method: &str) -> Vec<&'a ScoreRow> {
    let mut ranked = rows.iter().collect::<Vec<_>>();
    ranked.sort_unstable_by(|left, right| {
        score_for(right, method)
            .total_cmp(&score_for(left, method))
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    ranked
}

fn score_for(row: &ScoreRow, method: &str) -> f64 {
    row.scores.get(method).copied().unwrap_or(0.0)
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> AdjudicationResult<Vec<T>> {
    let input = fs::read_to_string(path)?;
    input
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let value = line.trim();
            (!value.is_empty() && !value.starts_with('#')).then_some((index, value))
        })
        .map(|(index, value)| {
            serde_json::from_str(value).map_err(|error| {
                invalid(format!("{}:{}: {error}", path.display(), index + 1))
            })
        })
        .collect()
}

fn write_jsonl_bytes<T: Serialize>(values: &[T]) -> AdjudicationResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for value in values {
        serde_json::to_writer(&mut bytes, value)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn one_line(value: &str, maximum_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= maximum_chars {
        return compact;
    }
    let mut output = compact.chars().take(maximum_chars).collect::<String>();
    output.push('…');
    output
}

fn validate_relative_path(value: &str) -> AdjudicationResult<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid(format!("unsafe corpus path {value:?}")));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
