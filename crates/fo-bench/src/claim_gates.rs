use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::Path;

use fo_core::{
    GroupedEvaluationOptions, GroupedEvaluationReport, GroupedLabeledScore,
    grouped_evaluation_report,
};
use fo_corpus::{atomic_write, sha256_hex, unix_timestamp};
use serde::{Deserialize, Serialize};

pub type ClaimResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const CLAIM_GATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
struct ScoreRow {
    corpus_size: usize,
    query_id: String,
    profile: String,
    candidate_id: String,
    label: bool,
    scores: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProofReport {
    corpus_id: String,
    scales: Vec<ProofScale>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProofScale {
    corpus_size: usize,
    exhaustive: ExhaustiveCoverage,
    methods: Vec<ProofMethod>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExhaustiveCoverage {
    complete: bool,
    complete_queries: usize,
    partial_queries: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ProofMethod {
    name: String,
    timing: ProofTiming,
}

#[derive(Debug, Clone, Deserialize)]
struct ProofTiming {
    repeat_p95_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimGateManifest {
    pub schema_version: u32,
    pub corpus_size: Option<usize>,
    pub bootstrap_samples: usize,
    pub confidence_level: f64,
    pub seed: u64,
    pub minimum_queries: usize,
    pub minimum_profile_queries: usize,
    pub comparisons: Vec<ClaimComparison>,
}

impl Default for ClaimGateManifest {
    fn default() -> Self {
        Self {
            schema_version: CLAIM_GATE_SCHEMA_VERSION,
            corpus_size: None,
            bootstrap_samples: 2_000,
            confidence_level: 0.95,
            seed: 0x63_6c_61_69_6d_2d_67_61,
            minimum_queries: 20,
            minimum_profile_queries: 3,
            comparisons: vec![ClaimComparison::default()],
        }
    }
}

impl ClaimGateManifest {
    pub fn validate(&self) -> ClaimResult<()> {
        if self.schema_version != CLAIM_GATE_SCHEMA_VERSION {
            return Err(invalid(format!(
                "unsupported claim-gate schema {}",
                self.schema_version
            )));
        }
        if self.bootstrap_samples == 0
            || self.bootstrap_samples > 1_000_000
            || self.minimum_queries == 0
            || self.minimum_profile_queries == 0
            || self.comparisons.is_empty()
            || self.comparisons.len() > 128
        {
            return Err(invalid(
                "bootstrap, query, profile, and comparison counts are outside safe bounds",
            ));
        }
        if !self.confidence_level.is_finite()
            || self.confidence_level <= 0.5
            || self.confidence_level >= 1.0
        {
            return Err(invalid("confidence_level must lie in (0.5, 1)"));
        }
        let mut ids = BTreeSet::new();
        for comparison in &self.comparisons {
            comparison.validate()?;
            if !ids.insert(comparison.id.as_str()) {
                return Err(invalid(format!(
                    "duplicate claim comparison ID {:?}",
                    comparison.id
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimComparison {
    pub id: String,
    pub baseline_method: String,
    pub challenger_method: String,
    pub minimum_challenger_micro_auprc: f64,
    pub minimum_challenger_macro_auprc: f64,
    pub minimum_challenger_recall_at_1: f64,
    pub minimum_micro_auprc_delta: f64,
    pub minimum_macro_auprc_delta: f64,
    pub minimum_recall_at_1_delta: f64,
    pub minimum_mrr_delta: f64,
    pub minimum_micro_delta_lower_bound: f64,
    pub minimum_macro_delta_lower_bound: f64,
    pub minimum_recall_at_1_delta_lower_bound: f64,
    pub maximum_worst_profile_macro_regression: f64,
    pub maximum_challenger_p95_ms: Option<f64>,
    pub maximum_p95_ratio: Option<f64>,
    pub require_complete_baseline: bool,
}

impl Default for ClaimComparison {
    fn default() -> Self {
        Self {
            id: "hybrid-vs-bm25".to_owned(),
            baseline_method: "fielded_bm25_phrase_proximity".to_owned(),
            challenger_method: "franken_hybrid".to_owned(),
            minimum_challenger_micro_auprc: 0.0,
            minimum_challenger_macro_auprc: 0.0,
            minimum_challenger_recall_at_1: 0.0,
            minimum_micro_auprc_delta: 0.0,
            minimum_macro_auprc_delta: 0.0,
            minimum_recall_at_1_delta: -0.01,
            minimum_mrr_delta: -0.01,
            minimum_micro_delta_lower_bound: 0.0,
            minimum_macro_delta_lower_bound: 0.0,
            minimum_recall_at_1_delta_lower_bound: -0.01,
            maximum_worst_profile_macro_regression: 0.02,
            maximum_challenger_p95_ms: None,
            maximum_p95_ratio: Some(2.0),
            require_complete_baseline: true,
        }
    }
}

impl ClaimComparison {
    fn validate(&self) -> ClaimResult<()> {
        if self.id.trim().is_empty()
            || self.baseline_method.trim().is_empty()
            || self.challenger_method.trim().is_empty()
            || self.baseline_method == self.challenger_method
        {
            return Err(invalid(
                "claim comparison IDs and method names must be nonempty and distinct",
            ));
        }
        for (name, value) in [
            (
                "minimum_challenger_micro_auprc",
                self.minimum_challenger_micro_auprc,
            ),
            (
                "minimum_challenger_macro_auprc",
                self.minimum_challenger_macro_auprc,
            ),
            (
                "minimum_challenger_recall_at_1",
                self.minimum_challenger_recall_at_1,
            ),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(invalid(format!("{name} must lie in [0, 1]")));
            }
        }
        for (name, value) in [
            ("minimum_micro_auprc_delta", self.minimum_micro_auprc_delta),
            ("minimum_macro_auprc_delta", self.minimum_macro_auprc_delta),
            ("minimum_recall_at_1_delta", self.minimum_recall_at_1_delta),
            ("minimum_mrr_delta", self.minimum_mrr_delta),
            (
                "minimum_micro_delta_lower_bound",
                self.minimum_micro_delta_lower_bound,
            ),
            (
                "minimum_macro_delta_lower_bound",
                self.minimum_macro_delta_lower_bound,
            ),
            (
                "minimum_recall_at_1_delta_lower_bound",
                self.minimum_recall_at_1_delta_lower_bound,
            ),
        ] {
            if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
                return Err(invalid(format!("{name} must lie in [-1, 1]")));
            }
        }
        if !self.maximum_worst_profile_macro_regression.is_finite()
            || !(0.0..=1.0).contains(&self.maximum_worst_profile_macro_regression)
        {
            return Err(invalid(
                "maximum_worst_profile_macro_regression must lie in [0, 1]",
            ));
        }
        if self
            .maximum_challenger_p95_ms
            .is_some_and(|value| !value.is_finite() || value < 0.0)
            || self
                .maximum_p95_ratio
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err(invalid("latency gates must be finite and positive"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub micro_auprc: f64,
    pub macro_auprc: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_1: f64,
    pub ndcg_at_10: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub micro_auprc: f64,
    pub macro_auprc: f64,
    pub mean_reciprocal_rank: f64,
    pub recall_at_1: f64,
    pub ndcg_at_10: f64,
}

impl MetricDelta {
    fn between(baseline: MetricSnapshot, challenger: MetricSnapshot) -> Self {
        Self {
            micro_auprc: challenger.micro_auprc - baseline.micro_auprc,
            macro_auprc: challenger.macro_auprc - baseline.macro_auprc,
            mean_reciprocal_rank: challenger.mean_reciprocal_rank - baseline.mean_reciprocal_rank,
            recall_at_1: challenger.recall_at_1 - baseline.recall_at_1,
            ndcg_at_10: challenger.ndcg_at_10 - baseline.ndcg_at_10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeltaInterval {
    pub lower: f64,
    pub median: f64,
    pub upper: f64,
    pub nominal_confidence_level: f64,
    pub familywise_confidence_level: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapIntervals {
    pub micro_auprc: DeltaInterval,
    pub macro_auprc: DeltaInterval,
    pub mean_reciprocal_rank: DeltaInterval,
    pub recall_at_1: DeltaInterval,
    pub ndcg_at_10: DeltaInterval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileComparison {
    pub profile: String,
    pub queries: usize,
    pub baseline: MetricSnapshot,
    pub challenger: MetricSnapshot,
    pub delta: MetricDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimVerdict {
    Supported,
    Inconclusive,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimComparisonReport {
    pub id: String,
    pub verdict: ClaimVerdict,
    pub baseline_method: String,
    pub challenger_method: String,
    pub corpus_size: usize,
    pub eligible_queries: usize,
    pub excluded_incomplete_queries: usize,
    pub candidate_pairs: usize,
    pub baseline: MetricSnapshot,
    pub challenger: MetricSnapshot,
    pub delta: MetricDelta,
    pub bootstrap: BootstrapIntervals,
    pub profiles: Vec<ProfileComparison>,
    pub worst_profile_macro_delta: Option<f64>,
    pub worst_profile: Option<String>,
    pub baseline_p95_ms: f64,
    pub challenger_p95_ms: f64,
    pub p95_ratio: Option<f64>,
    pub baseline_complete: bool,
    pub failures: Vec<String>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimGateReport {
    pub schema_version: u32,
    pub generated_at_unix: u64,
    pub corpus_id: String,
    pub corpus_size: usize,
    pub score_sha256: String,
    pub proof_report_sha256: String,
    pub manifest_sha256: String,
    pub nominal_confidence_level: f64,
    pub familywise_confidence_level: f64,
    pub comparisons: Vec<ClaimComparisonReport>,
    pub all_supported: bool,
}

pub fn evaluate_claims(
    proof_report_path: &Path,
    score_path: &Path,
    manifest_path: &Path,
) -> ClaimResult<ClaimGateReport> {
    let proof_bytes = fs::read(proof_report_path)?;
    let score_bytes = fs::read(score_path)?;
    let manifest_bytes = fs::read(manifest_path)?;
    let proof = serde_json::from_slice::<ProofReport>(&proof_bytes)?;
    let manifest = serde_json::from_slice::<ClaimGateManifest>(&manifest_bytes)?;
    manifest.validate()?;
    let rows = read_score_rows(&score_bytes)?;
    let corpus_size = manifest.corpus_size.unwrap_or_else(|| {
        proof
            .scales
            .iter()
            .map(|scale| scale.corpus_size)
            .max()
            .unwrap_or(0)
    });
    let scale = proof
        .scales
        .iter()
        .find(|scale| scale.corpus_size == corpus_size)
        .ok_or_else(|| invalid(format!("proof report has no corpus size {corpus_size}")))?;
    let rows = rows
        .into_iter()
        .filter(|row| row.corpus_size == corpus_size)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Err(invalid(format!(
            "score file has no rows for corpus size {corpus_size}"
        )));
    }
    let familywise_confidence =
        1.0 - (1.0 - manifest.confidence_level) / manifest.comparisons.len().max(1) as f64;
    let mut comparisons = Vec::with_capacity(manifest.comparisons.len());
    for (index, comparison) in manifest.comparisons.iter().enumerate() {
        comparisons.push(evaluate_comparison(
            &rows,
            scale,
            comparison,
            &manifest,
            familywise_confidence,
            index,
        )?);
    }
    let all_supported = comparisons
        .iter()
        .all(|comparison| comparison.verdict == ClaimVerdict::Supported);
    Ok(ClaimGateReport {
        schema_version: CLAIM_GATE_SCHEMA_VERSION,
        generated_at_unix: unix_timestamp(),
        corpus_id: proof.corpus_id,
        corpus_size,
        score_sha256: sha256_hex(&score_bytes),
        proof_report_sha256: sha256_hex(&proof_bytes),
        manifest_sha256: sha256_hex(&manifest_bytes),
        nominal_confidence_level: manifest.confidence_level,
        familywise_confidence_level: familywise_confidence,
        comparisons,
        all_supported,
    })
}

pub fn write_default_manifest(path: &Path) -> ClaimResult<()> {
    atomic_write(
        path,
        &serde_json::to_vec_pretty(&ClaimGateManifest::default())?,
    )?;
    Ok(())
}

fn evaluate_comparison(
    rows: &[ScoreRow],
    scale: &ProofScale,
    comparison: &ClaimComparison,
    manifest: &ClaimGateManifest,
    familywise_confidence: f64,
    comparison_index: usize,
) -> ClaimResult<ClaimComparisonReport> {
    let grouped = group_rows(rows)?;
    let mut eligible = Vec::<QueryPair>::new();
    let mut excluded_incomplete_queries = 0usize;
    for (query_id, query_rows) in grouped {
        if query_rows.iter().all(|row| {
            row.scores.contains_key(&comparison.baseline_method)
                && row.scores.contains_key(&comparison.challenger_method)
        }) {
            eligible.push(QueryPair::new(
                query_id,
                query_rows,
                &comparison.baseline_method,
                &comparison.challenger_method,
            )?);
        } else {
            excluded_incomplete_queries += 1;
        }
    }
    eligible.sort_unstable_by(|left, right| left.query_id.cmp(&right.query_id));
    if eligible.is_empty() {
        return Err(invalid(format!(
            "comparison {} has no complete paired queries",
            comparison.id
        )));
    }
    let baseline = evaluate_query_pairs(&eligible, ScoreSide::Baseline)?;
    let challenger = evaluate_query_pairs(&eligible, ScoreSide::Challenger)?;
    let delta = MetricDelta::between(baseline, challenger);
    let bootstrap = bootstrap_intervals(
        &eligible,
        manifest.bootstrap_samples,
        familywise_confidence,
        manifest.confidence_level,
        manifest.seed ^ comparison_index as u64,
    )?;
    let profiles = profile_comparisons(&eligible, manifest.minimum_profile_queries)?;
    let (worst_profile, worst_profile_macro_delta) = profiles
        .iter()
        .min_by(|left, right| left.delta.macro_auprc.total_cmp(&right.delta.macro_auprc))
        .map(|profile| {
            (
                Some(profile.profile.clone()),
                Some(profile.delta.macro_auprc),
            )
        })
        .unwrap_or((None, None));
    let baseline_p95_ms = method_p95(scale, &comparison.baseline_method)?;
    let challenger_p95_ms = method_p95(scale, &comparison.challenger_method)?;
    let p95_ratio = (baseline_p95_ms > 0.0).then_some(challenger_p95_ms / baseline_p95_ms);
    let baseline_complete = baseline_completeness(scale, &comparison.baseline_method);
    let candidate_pairs = eligible.iter().map(|query| query.labels.len()).sum();

    let mut failures = Vec::new();
    let mut uncertainties = Vec::new();
    require_minimum(
        &mut failures,
        "challenger micro AUPRC",
        challenger.micro_auprc,
        comparison.minimum_challenger_micro_auprc,
    );
    require_minimum(
        &mut failures,
        "challenger macro AUPRC",
        challenger.macro_auprc,
        comparison.minimum_challenger_macro_auprc,
    );
    require_minimum(
        &mut failures,
        "challenger Recall@1",
        challenger.recall_at_1,
        comparison.minimum_challenger_recall_at_1,
    );
    require_minimum(
        &mut failures,
        "micro AUPRC delta",
        delta.micro_auprc,
        comparison.minimum_micro_auprc_delta,
    );
    require_minimum(
        &mut failures,
        "macro AUPRC delta",
        delta.macro_auprc,
        comparison.minimum_macro_auprc_delta,
    );
    require_minimum(
        &mut failures,
        "Recall@1 delta",
        delta.recall_at_1,
        comparison.minimum_recall_at_1_delta,
    );
    require_minimum(
        &mut failures,
        "MRR delta",
        delta.mean_reciprocal_rank,
        comparison.minimum_mrr_delta,
    );
    if eligible.len() < manifest.minimum_queries {
        uncertainties.push(format!(
            "only {} paired queries were eligible; at least {} were required",
            eligible.len(),
            manifest.minimum_queries
        ));
    }
    require_lower_bound(
        &mut uncertainties,
        "micro AUPRC delta",
        bootstrap.micro_auprc.lower,
        comparison.minimum_micro_delta_lower_bound,
    );
    require_lower_bound(
        &mut uncertainties,
        "macro AUPRC delta",
        bootstrap.macro_auprc.lower,
        comparison.minimum_macro_delta_lower_bound,
    );
    require_lower_bound(
        &mut uncertainties,
        "Recall@1 delta",
        bootstrap.recall_at_1.lower,
        comparison.minimum_recall_at_1_delta_lower_bound,
    );
    match worst_profile_macro_delta {
        Some(delta) if delta < -comparison.maximum_worst_profile_macro_regression => {
            failures.push(format!(
                "worst eligible profile {:?} macro AUPRC regressed by {:.6}, exceeding the {:.6} limit",
                worst_profile,
                -delta,
                comparison.maximum_worst_profile_macro_regression
            ));
        }
        None => uncertainties.push(format!(
            "no profile had at least {} complete paired queries",
            manifest.minimum_profile_queries
        )),
        _ => {}
    }
    if let Some(maximum) = comparison.maximum_challenger_p95_ms {
        if challenger_p95_ms > maximum {
            failures.push(format!(
                "challenger p95 {:.6} ms exceeded the {:.6} ms limit",
                challenger_p95_ms, maximum
            ));
        }
    }
    if let Some(maximum) = comparison.maximum_p95_ratio {
        match p95_ratio {
            Some(ratio) if ratio > maximum => failures.push(format!(
                "challenger/baseline p95 ratio {:.6} exceeded the {:.6} limit",
                ratio, maximum
            )),
            None => uncertainties.push(
                "baseline p95 was zero, so the latency ratio could not be established".to_owned(),
            ),
            _ => {}
        }
    }
    if comparison.require_complete_baseline && !baseline_complete {
        failures.push(format!(
            "baseline method {} was incomplete at corpus size {}",
            comparison.baseline_method, scale.corpus_size
        ));
    }
    let verdict = if !failures.is_empty() {
        ClaimVerdict::Unsupported
    } else if !uncertainties.is_empty() {
        ClaimVerdict::Inconclusive
    } else {
        ClaimVerdict::Supported
    };
    Ok(ClaimComparisonReport {
        id: comparison.id.clone(),
        verdict,
        baseline_method: comparison.baseline_method.clone(),
        challenger_method: comparison.challenger_method.clone(),
        corpus_size: scale.corpus_size,
        eligible_queries: eligible.len(),
        excluded_incomplete_queries,
        candidate_pairs,
        baseline,
        challenger,
        delta,
        bootstrap,
        profiles,
        worst_profile_macro_delta,
        worst_profile,
        baseline_p95_ms,
        challenger_p95_ms,
        p95_ratio,
        baseline_complete,
        failures,
        uncertainties,
    })
}

#[derive(Debug)]
struct QueryPair {
    query_id: String,
    profile: String,
    labels: Vec<bool>,
    baseline_scores: Vec<f64>,
    challenger_scores: Vec<f64>,
}

impl QueryPair {
    fn new(
        query_id: String,
        mut rows: Vec<&ScoreRow>,
        baseline_method: &str,
        challenger_method: &str,
    ) -> ClaimResult<Self> {
        rows.sort_unstable_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
        let profile = rows
            .first()
            .map(|row| row.profile.clone())
            .ok_or_else(|| invalid("empty query group"))?;
        if rows.iter().any(|row| row.profile != profile) {
            return Err(invalid(format!(
                "query {query_id} has inconsistent profile labels"
            )));
        }
        let labels = rows.iter().map(|row| row.label).collect::<Vec<_>>();
        if !labels.iter().any(|label| *label) {
            return Err(invalid(format!(
                "query {query_id} contains no positive candidate"
            )));
        }
        Ok(Self {
            query_id,
            profile,
            labels,
            baseline_scores: rows.iter().map(|row| row.scores[baseline_method]).collect(),
            challenger_scores: rows
                .iter()
                .map(|row| row.scores[challenger_method])
                .collect(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ScoreSide {
    Baseline,
    Challenger,
}

fn evaluate_query_pairs(queries: &[QueryPair], side: ScoreSide) -> ClaimResult<MetricSnapshot> {
    let examples = query_examples(queries, side, None);
    metric_snapshot(&grouped_evaluation_report(&examples, evaluation_options())?)
}

fn profile_comparisons(
    queries: &[QueryPair],
    minimum_queries: usize,
) -> ClaimResult<Vec<ProfileComparison>> {
    let mut grouped = BTreeMap::<&str, Vec<&QueryPair>>::new();
    for query in queries {
        grouped
            .entry(query.profile.as_str())
            .or_default()
            .push(query);
    }
    let mut output = Vec::new();
    for (profile, members) in grouped {
        if members.len() < minimum_queries {
            continue;
        }
        let owned = members.into_iter().collect::<Vec<_>>();
        let baseline = evaluate_query_refs(&owned, ScoreSide::Baseline)?;
        let challenger = evaluate_query_refs(&owned, ScoreSide::Challenger)?;
        output.push(ProfileComparison {
            profile: profile.to_owned(),
            queries: owned.len(),
            baseline,
            challenger,
            delta: MetricDelta::between(baseline, challenger),
        });
    }
    output.sort_unstable_by(|left, right| left.profile.cmp(&right.profile));
    Ok(output)
}

fn evaluate_query_refs(queries: &[&QueryPair], side: ScoreSide) -> ClaimResult<MetricSnapshot> {
    let mut examples = Vec::new();
    for query in queries {
        let scores = match side {
            ScoreSide::Baseline => &query.baseline_scores,
            ScoreSide::Challenger => &query.challenger_scores,
        };
        for (&score, &label) in scores.iter().zip(&query.labels) {
            examples.push(GroupedLabeledScore {
                query_id: query.query_id.clone(),
                score,
                label,
            });
        }
    }
    metric_snapshot(&grouped_evaluation_report(&examples, evaluation_options())?)
}

fn bootstrap_intervals(
    queries: &[QueryPair],
    samples: usize,
    familywise_confidence: f64,
    nominal_confidence: f64,
    seed: u64,
) -> ClaimResult<BootstrapIntervals> {
    let mut rng = DeterministicRng::new(seed);
    let mut micro = Vec::with_capacity(samples);
    let mut macro_values = Vec::with_capacity(samples);
    let mut mrr = Vec::with_capacity(samples);
    let mut recall = Vec::with_capacity(samples);
    let mut ndcg = Vec::with_capacity(samples);
    for sample in 0..samples {
        let draws = (0..queries.len())
            .map(|draw| (draw, rng.range(queries.len())))
            .collect::<Vec<_>>();
        let baseline = metric_snapshot(&grouped_evaluation_report(
            &query_examples(queries, ScoreSide::Baseline, Some((sample, &draws))),
            evaluation_options(),
        )?)?;
        let challenger = metric_snapshot(&grouped_evaluation_report(
            &query_examples(queries, ScoreSide::Challenger, Some((sample, &draws))),
            evaluation_options(),
        )?)?;
        let delta = MetricDelta::between(baseline, challenger);
        micro.push(delta.micro_auprc);
        macro_values.push(delta.macro_auprc);
        mrr.push(delta.mean_reciprocal_rank);
        recall.push(delta.recall_at_1);
        ndcg.push(delta.ndcg_at_10);
    }
    Ok(BootstrapIntervals {
        micro_auprc: interval(micro, nominal_confidence, familywise_confidence),
        macro_auprc: interval(macro_values, nominal_confidence, familywise_confidence),
        mean_reciprocal_rank: interval(mrr, nominal_confidence, familywise_confidence),
        recall_at_1: interval(recall, nominal_confidence, familywise_confidence),
        ndcg_at_10: interval(ndcg, nominal_confidence, familywise_confidence),
    })
}

fn query_examples(
    queries: &[QueryPair],
    side: ScoreSide,
    resample: Option<(usize, &[(usize, usize)])>,
) -> Vec<GroupedLabeledScore> {
    let mut examples = Vec::new();
    match resample {
        None => {
            for query in queries {
                append_query_examples(&mut examples, query, side, query.query_id.clone());
            }
        }
        Some((sample, draws)) => {
            for (draw, query_index) in draws {
                let query = &queries[*query_index];
                append_query_examples(
                    &mut examples,
                    query,
                    side,
                    format!("bootstrap-{sample}-{draw}-{}", query.query_id),
                );
            }
        }
    }
    examples
}

fn append_query_examples(
    output: &mut Vec<GroupedLabeledScore>,
    query: &QueryPair,
    side: ScoreSide,
    query_id: String,
) {
    let scores = match side {
        ScoreSide::Baseline => &query.baseline_scores,
        ScoreSide::Challenger => &query.challenger_scores,
    };
    output.extend(
        scores
            .iter()
            .copied()
            .zip(query.labels.iter().copied())
            .map(|(score, label)| GroupedLabeledScore {
                query_id: query_id.clone(),
                score,
                label,
            }),
    );
}

fn metric_snapshot(report: &GroupedEvaluationReport) -> ClaimResult<MetricSnapshot> {
    Ok(MetricSnapshot {
        micro_auprc: report.micro.average_precision,
        macro_auprc: report.macro_average_precision,
        mean_reciprocal_rank: report.mean_reciprocal_rank,
        recall_at_1: metric_at(&report.recall_at_k, 1)?,
        ndcg_at_10: metric_at(&report.ndcg_at_k, 10)?,
    })
}

fn metric_at(metrics: &[fo_core::AtKMetric], k: usize) -> ClaimResult<f64> {
    metrics
        .iter()
        .find(|metric| metric.k == k)
        .map(|metric| metric.value)
        .ok_or_else(|| invalid(format!("evaluation report has no metric at k={k}")))
}

fn interval(
    mut values: Vec<f64>,
    nominal_confidence: f64,
    familywise_confidence: f64,
) -> DeltaInterval {
    values.sort_by(f64::total_cmp);
    let alpha = 1.0 - familywise_confidence;
    DeltaInterval {
        lower: quantile(&values, alpha),
        median: quantile(&values, 0.5),
        upper: quantile(&values, 1.0 - alpha),
        nominal_confidence_level: nominal_confidence,
        familywise_confidence_level: familywise_confidence,
        samples: values.len(),
    }
}

fn quantile(values: &[f64], probability: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let position = ((values.len() - 1) as f64 * probability.clamp(0.0, 1.0)).round() as usize;
    values[position.min(values.len() - 1)]
}

fn method_p95(scale: &ProofScale, method: &str) -> ClaimResult<f64> {
    scale
        .methods
        .iter()
        .find(|candidate| candidate.name == method)
        .map(|method| method.timing.repeat_p95_ms)
        .ok_or_else(|| invalid(format!("proof report has no method {method:?}")))
}

fn baseline_completeness(scale: &ProofScale, method: &str) -> bool {
    if method != "exhaustive_levenshtein" {
        return true;
    }
    scale.exhaustive.complete
        && scale.exhaustive.partial_queries == 0
        && scale.exhaustive.complete_queries > 0
}

fn require_minimum(failures: &mut Vec<String>, name: &str, observed: f64, minimum: f64) {
    if observed < minimum {
        failures.push(format!(
            "{name} {:.6} was below the required {:.6}",
            observed, minimum
        ));
    }
}

fn require_lower_bound(uncertainties: &mut Vec<String>, name: &str, observed: f64, minimum: f64) {
    if observed < minimum {
        uncertainties.push(format!(
            "family-wise lower bound for {name} was {:.6}, below the required {:.6}",
            observed, minimum
        ));
    }
}

fn group_rows(rows: &[ScoreRow]) -> ClaimResult<BTreeMap<String, Vec<&ScoreRow>>> {
    let mut grouped = BTreeMap::<String, Vec<&ScoreRow>>::new();
    let mut keys = BTreeSet::new();
    for row in rows {
        if row.query_id.trim().is_empty()
            || row.profile.trim().is_empty()
            || row.candidate_id.trim().is_empty()
            || row.scores.values().any(|score| !score.is_finite())
            || !keys.insert((row.query_id.clone(), row.candidate_id.clone()))
        {
            return Err(invalid(
                "score rows contain empty fields, non-finite scores, or duplicate query/candidate pairs",
            ));
        }
        grouped.entry(row.query_id.clone()).or_default().push(row);
    }
    Ok(grouped)
}

fn read_score_rows(bytes: &[u8]) -> ClaimResult<Vec<ScoreRow>> {
    let input = std::str::from_utf8(bytes)?;
    input
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let value = line.trim();
            (!value.is_empty() && !value.starts_with('#')).then_some((index, value))
        })
        .map(|(index, value)| {
            serde_json::from_str(value)
                .map_err(|error| invalid(format!("score row {}: {error}", index + 1)))
        })
        .collect()
}

fn evaluation_options() -> GroupedEvaluationOptions {
    GroupedEvaluationOptions {
        recall_ks: vec![1, 5, 10],
        bootstrap_samples: 0,
        ..GroupedEvaluationOptions::default()
    }
}

struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn range(&mut self, upper: usize) -> usize {
        debug_assert!(upper > 0);
        (self.next() % upper as u64) as usize
    }
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}
