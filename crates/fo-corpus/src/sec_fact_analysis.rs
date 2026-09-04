use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CorpusError, Result, SecCompanyFacts, SecFactObservation};

pub const SEC_FACT_ANALYSIS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecInvestorMetric {
    Revenue,
    CostOfRevenue,
    GrossProfit,
    GrossMargin,
    ResearchAndDevelopment,
    SellingGeneralAndAdministrative,
    OperatingIncome,
    OperatingMargin,
    NetIncome,
    OperatingCashFlow,
    CapitalExpenditures,
    FreeCashFlow,
    Cash,
    CurrentDebt,
    LongTermDebt,
    TotalDebt,
    NetDebt,
    CurrentAssets,
    CurrentLiabilities,
    CurrentRatio,
    AccountsReceivable,
    Inventory,
    AccountsPayable,
    DeferredRevenue,
    Goodwill,
    IntangibleAssets,
    ShareholdersEquity,
    ShareRepurchases,
    DividendsPaid,
    StockBasedCompensation,
    DilutedShares,
    InterestExpense,
    IncomeTaxExpense,
    OperatingLeaseLiability,
}

impl SecInvestorMetric {
    #[must_use]
    pub const fn is_derived(self) -> bool {
        matches!(
            self,
            Self::GrossMargin
                | Self::OperatingMargin
                | Self::FreeCashFlow
                | Self::TotalDebt
                | Self::NetDebt
                | Self::CurrentRatio
        )
    }

    #[must_use]
    pub const fn expected_unit(self) -> &'static str {
        match self {
            Self::GrossMargin | Self::OperatingMargin | Self::CurrentRatio => "ratio",
            Self::DilutedShares => "shares",
            _ => "USD",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactPeriodType {
    Annual,
    Quarterly,
    Instant,
    OtherDuration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactPoint {
    pub metric: SecInvestorMetric,
    pub value: f64,
    pub unit: String,
    pub start: Option<String>,
    pub end: String,
    pub fiscal_year: Option<i64>,
    pub fiscal_period: Option<String>,
    pub period_type: FactPeriodType,
    pub form: String,
    pub filed: String,
    pub accession_number: String,
    pub taxonomy: String,
    pub concept: String,
    pub fact_id: String,
    pub derived_from_fact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDelta {
    pub current: FactPoint,
    pub previous: FactPoint,
    pub absolute_change: f64,
    pub relative_change: Option<f64>,
    pub comparison_basis: String,
    pub robust_z_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactRestatement {
    pub metric: SecInvestorMetric,
    pub unit: String,
    pub start: Option<String>,
    pub end: String,
    pub earliest_value: f64,
    pub latest_value: f64,
    pub absolute_revision: f64,
    pub relative_revision: Option<f64>,
    pub earliest_filed: String,
    pub latest_filed: String,
    pub accession_numbers: Vec<String>,
    pub fact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSeries {
    pub metric: SecInvestorMetric,
    pub taxonomy: String,
    pub concept: String,
    pub label: String,
    pub unit: String,
    pub points: Vec<FactPoint>,
    pub latest_delta: Option<MetricDelta>,
    pub restatements: Vec<FactRestatement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactAlertKind {
    MaterialChange,
    StatisticalAnomaly,
    Restatement,
    MarginCompression,
    MarginExpansion,
    LeverageIncrease,
    LeverageReduction,
    CashConversionDeterioration,
    CashConversionImprovement,
    Dilution,
    WorkingCapitalBuild,
    StockCompensationAcceleration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactAlert {
    pub kind: FactAlertKind,
    pub code: String,
    pub title: String,
    pub severity: f32,
    pub metric: SecInvestorMetric,
    pub period_end: String,
    pub rationale: String,
    pub current_value: f64,
    pub previous_value: Option<f64>,
    pub relative_change: Option<f64>,
    pub robust_z_score: Option<f64>,
    pub accession_numbers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecFactAnalysisOptions {
    pub allowed_forms: Vec<String>,
    pub earliest_filed_date: Option<String>,
    pub maximum_points_per_metric: usize,
    pub minimum_points_for_anomaly: usize,
    pub material_change_fraction: f64,
    pub anomaly_mad_threshold: f64,
    pub restatement_absolute_tolerance: f64,
    pub restatement_relative_tolerance: f64,
    pub margin_alert_points: f64,
    pub dilution_alert_fraction: f64,
    pub working_capital_spread_fraction: f64,
    pub maximum_alerts: usize,
}

impl Default for SecFactAnalysisOptions {
    fn default() -> Self {
        Self {
            allowed_forms: vec![
                "10-K".to_owned(),
                "10-K/A".to_owned(),
                "10-Q".to_owned(),
                "10-Q/A".to_owned(),
                "20-F".to_owned(),
                "20-F/A".to_owned(),
                "40-F".to_owned(),
                "40-F/A".to_owned(),
            ],
            earliest_filed_date: None,
            maximum_points_per_metric: 256,
            minimum_points_for_anomaly: 5,
            material_change_fraction: 0.15,
            anomaly_mad_threshold: 3.5,
            restatement_absolute_tolerance: 1.0e-9,
            restatement_relative_tolerance: 1.0e-6,
            margin_alert_points: 0.02,
            dilution_alert_fraction: 0.03,
            working_capital_spread_fraction: 0.15,
            maximum_alerts: 1_000,
        }
    }
}

impl SecFactAnalysisOptions {
    pub fn validate(&self) -> Result<()> {
        if self.allowed_forms.is_empty()
            || self.allowed_forms.len() > 256
            || self.maximum_points_per_metric == 0
            || self.minimum_points_for_anomaly < 3
            || !self.material_change_fraction.is_finite()
            || self.material_change_fraction < 0.0
            || !self.anomaly_mad_threshold.is_finite()
            || self.anomaly_mad_threshold <= 0.0
            || !self.restatement_absolute_tolerance.is_finite()
            || self.restatement_absolute_tolerance < 0.0
            || !self.restatement_relative_tolerance.is_finite()
            || self.restatement_relative_tolerance < 0.0
            || !self.margin_alert_points.is_finite()
            || self.margin_alert_points < 0.0
            || !self.dilution_alert_fraction.is_finite()
            || self.dilution_alert_fraction < 0.0
            || !self.working_capital_spread_fraction.is_finite()
            || self.working_capital_spread_fraction < 0.0
            || self.maximum_alerts == 0
        {
            return Err(CorpusError::Invalid(
                "SEC fact-analysis thresholds or limits are invalid".to_owned(),
            ));
        }
        if self
            .earliest_filed_date
            .as_ref()
            .is_some_and(|date| !valid_date(date))
        {
            return Err(CorpusError::Invalid(
                "earliest_filed_date must use YYYY-MM-DD".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactAnalysis {
    pub schema_version: u32,
    pub cik: u64,
    pub entity_name: String,
    pub tickers: Vec<String>,
    pub source_sha256: String,
    pub observations_considered: usize,
    pub metric_series: Vec<MetricSeries>,
    pub alerts: Vec<FactAlert>,
    pub missing_metrics: Vec<SecInvestorMetric>,
}

pub fn analyze_sec_companyfacts(
    facts: &SecCompanyFacts,
    options: &SecFactAnalysisOptions,
) -> Result<SecFactAnalysis> {
    facts.validate()?;
    options.validate()?;
    let forms = options
        .allowed_forms
        .iter()
        .map(|form| normalize_form(form))
        .collect::<BTreeSet<_>>();
    let filtered = facts
        .observations
        .iter()
        .filter(|observation| forms.contains(&normalize_form(&observation.form)))
        .filter(|observation| {
            options
                .earliest_filed_date
                .as_ref()
                .is_none_or(|minimum| observation.filed >= *minimum)
        })
        .filter(|observation| observation.numeric_value().is_some())
        .collect::<Vec<_>>();

    let base_metrics = [
        SecInvestorMetric::Revenue,
        SecInvestorMetric::CostOfRevenue,
        SecInvestorMetric::GrossProfit,
        SecInvestorMetric::ResearchAndDevelopment,
        SecInvestorMetric::SellingGeneralAndAdministrative,
        SecInvestorMetric::OperatingIncome,
        SecInvestorMetric::NetIncome,
        SecInvestorMetric::OperatingCashFlow,
        SecInvestorMetric::CapitalExpenditures,
        SecInvestorMetric::Cash,
        SecInvestorMetric::CurrentDebt,
        SecInvestorMetric::LongTermDebt,
        SecInvestorMetric::CurrentAssets,
        SecInvestorMetric::CurrentLiabilities,
        SecInvestorMetric::AccountsReceivable,
        SecInvestorMetric::Inventory,
        SecInvestorMetric::AccountsPayable,
        SecInvestorMetric::DeferredRevenue,
        SecInvestorMetric::Goodwill,
        SecInvestorMetric::IntangibleAssets,
        SecInvestorMetric::ShareholdersEquity,
        SecInvestorMetric::ShareRepurchases,
        SecInvestorMetric::DividendsPaid,
        SecInvestorMetric::StockBasedCompensation,
        SecInvestorMetric::DilutedShares,
        SecInvestorMetric::InterestExpense,
        SecInvestorMetric::IncomeTaxExpense,
        SecInvestorMetric::OperatingLeaseLiability,
    ];
    let mut series = BTreeMap::<SecInvestorMetric, MetricSeries>::new();
    for metric in base_metrics {
        if let Some(metric_series) = build_metric_series(metric, &filtered, options) {
            series.insert(metric, metric_series);
        }
    }
    derive_difference_metric(
        SecInvestorMetric::FreeCashFlow,
        SecInvestorMetric::OperatingCashFlow,
        SecInvestorMetric::CapitalExpenditures,
        &mut series,
        options,
    );
    derive_sum_metric(
        SecInvestorMetric::TotalDebt,
        SecInvestorMetric::CurrentDebt,
        SecInvestorMetric::LongTermDebt,
        &mut series,
        options,
    );
    derive_difference_metric(
        SecInvestorMetric::NetDebt,
        SecInvestorMetric::TotalDebt,
        SecInvestorMetric::Cash,
        &mut series,
        options,
    );
    derive_ratio_metric(
        SecInvestorMetric::GrossMargin,
        SecInvestorMetric::GrossProfit,
        SecInvestorMetric::Revenue,
        &mut series,
        options,
    );
    derive_ratio_metric(
        SecInvestorMetric::OperatingMargin,
        SecInvestorMetric::OperatingIncome,
        SecInvestorMetric::Revenue,
        &mut series,
        options,
    );
    derive_ratio_metric(
        SecInvestorMetric::CurrentRatio,
        SecInvestorMetric::CurrentAssets,
        SecInvestorMetric::CurrentLiabilities,
        &mut series,
        options,
    );

    let mut alerts = build_alerts(&series, options);
    alerts.sort_unstable_by(|left, right| {
        right
            .severity
            .total_cmp(&left.severity)
            .then_with(|| left.period_end.cmp(&right.period_end))
            .then_with(|| left.code.cmp(&right.code))
    });
    alerts.truncate(options.maximum_alerts);
    let all_metrics = base_metrics
        .into_iter()
        .chain([
            SecInvestorMetric::FreeCashFlow,
            SecInvestorMetric::TotalDebt,
            SecInvestorMetric::NetDebt,
            SecInvestorMetric::GrossMargin,
            SecInvestorMetric::OperatingMargin,
            SecInvestorMetric::CurrentRatio,
        ])
        .collect::<BTreeSet<_>>();
    let missing_metrics = all_metrics
        .into_iter()
        .filter(|metric| !series.contains_key(metric))
        .collect::<Vec<_>>();
    Ok(SecFactAnalysis {
        schema_version: SEC_FACT_ANALYSIS_SCHEMA_VERSION,
        cik: facts.cik,
        entity_name: facts.entity_name.clone(),
        tickers: facts.tickers.clone(),
        source_sha256: facts.raw_sha256.clone(),
        observations_considered: filtered.len(),
        metric_series: series.into_values().collect(),
        alerts,
        missing_metrics,
    })
}

/// One alias's candidate series, before the best alias is chosen.
///
/// Aliases are ranked by how many distinct periods they cover, then by their
/// position in [`metric_aliases`] (earlier aliases are the preferred concept).
struct AliasCandidate<'a> {
    unique_periods: usize,
    /// Higher is better, so declaration order is inverted here.
    alias_rank: usize,
    taxonomy: String,
    concept: String,
    unit: String,
    observations: Vec<&'a SecFactObservation>,
}

fn build_metric_series(
    metric: SecInvestorMetric,
    observations: &[&SecFactObservation],
    options: &SecFactAnalysisOptions,
) -> Option<MetricSeries> {
    let aliases = metric_aliases(metric);
    let mut best: Option<AliasCandidate<'_>> = None;
    for (priority, alias) in aliases.iter().enumerate() {
        let matching = observations
            .iter()
            .copied()
            .filter(|observation| observation.concept == *alias)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            continue;
        }
        // A unitless alias is skipped, not fatal: a later alias may still
        // carry a usable series.
        let Some(unit) = choose_unit(metric, &matching) else {
            continue;
        };
        let matching = matching
            .into_iter()
            .filter(|observation| observation.unit == unit)
            .collect::<Vec<_>>();
        // `choose_unit` only ever returns a unit it observed, so the filter
        // cannot empty the set; guard anyway rather than index blindly below.
        let Some(first) = matching.first() else {
            continue;
        };
        let taxonomy = first.taxonomy.clone();
        let unique_periods = matching
            .iter()
            .map(|observation| {
                (
                    observation.start.as_deref(),
                    observation.end.as_deref(),
                    observation.fiscal_period.as_deref(),
                )
            })
            .collect::<BTreeSet<_>>()
            .len();
        let candidate = AliasCandidate {
            unique_periods,
            alias_rank: aliases.len() - priority,
            taxonomy,
            concept: (*alias).to_owned(),
            unit,
            observations: matching,
        };
        if best.as_ref().is_none_or(|current| {
            (candidate.unique_periods, candidate.alias_rank)
                > (current.unique_periods, current.alias_rank)
        }) {
            best = Some(candidate);
        }
    }
    let AliasCandidate {
        taxonomy,
        concept,
        unit,
        observations: raw_points,
        ..
    } = best?;
    let label = raw_points
        .first()
        .map_or_else(|| concept.clone(), |observation| observation.label.clone());
    let restatements = detect_restatements(metric, &unit, &raw_points, options);
    let mut points = raw_points
        .into_iter()
        .filter_map(|observation| observation_to_point(metric, observation))
        .collect::<Vec<_>>();
    points.sort_unstable_by(|left, right| {
        left.end
            .cmp(&right.end)
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.period_type.cmp(&right.period_type))
            .then_with(|| left.filed.cmp(&right.filed))
            .then_with(|| left.accession_number.cmp(&right.accession_number))
    });
    let mut latest_by_period = BTreeMap::<PeriodKey, FactPoint>::new();
    for point in points {
        let key = PeriodKey::from_point(&point);
        let replace = latest_by_period.get(&key).is_none_or(|current| {
            (point.filed.as_str(), point.accession_number.as_str())
                > (current.filed.as_str(), current.accession_number.as_str())
        });
        if replace {
            latest_by_period.insert(key, point);
        }
    }
    let mut points = latest_by_period.into_values().collect::<Vec<_>>();
    points.sort_unstable_by(|left, right| {
        left.end
            .cmp(&right.end)
            .then_with(|| left.period_type.cmp(&right.period_type))
            .then_with(|| left.start.cmp(&right.start))
    });
    if points.len() > options.maximum_points_per_metric {
        points.drain(..points.len() - options.maximum_points_per_metric);
    }
    let latest_delta = latest_delta(&points, options);
    Some(MetricSeries {
        metric,
        taxonomy,
        concept,
        label,
        unit,
        points,
        latest_delta,
        restatements,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PeriodKey {
    start: Option<String>,
    end: String,
    period_type: FactPeriodType,
    fiscal_period: Option<String>,
}

impl PeriodKey {
    fn from_point(point: &FactPoint) -> Self {
        Self {
            start: point.start.clone(),
            end: point.end.clone(),
            period_type: point.period_type,
            fiscal_period: point.fiscal_period.clone(),
        }
    }
}

fn observation_to_point(
    metric: SecInvestorMetric,
    observation: &SecFactObservation,
) -> Option<FactPoint> {
    let value = observation.numeric_value()?;
    let end = observation.end.clone()?;
    Some(FactPoint {
        metric,
        value,
        unit: observation.unit.clone(),
        start: observation.start.clone(),
        end,
        fiscal_year: observation.fiscal_year,
        fiscal_period: observation.fiscal_period.clone(),
        period_type: classify_period(observation),
        form: observation.form.clone(),
        filed: observation.filed.clone(),
        accession_number: observation.accession_number.clone(),
        taxonomy: observation.taxonomy.clone(),
        concept: observation.concept.clone(),
        fact_id: observation.id.clone(),
        derived_from_fact_ids: Vec::new(),
    })
}

fn classify_period(observation: &SecFactObservation) -> FactPeriodType {
    if observation.start.is_none() {
        return FactPeriodType::Instant;
    }
    match observation
        .fiscal_period
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .as_str()
    {
        "FY" => FactPeriodType::Annual,
        "Q1" | "Q2" | "Q3" | "Q4" => FactPeriodType::Quarterly,
        _ if matches!(
            normalize_form(&observation.form).as_str(),
            "10-K" | "10-K/A" | "20-F" | "20-F/A" | "40-F" | "40-F/A"
        ) =>
        {
            FactPeriodType::Annual
        }
        _ if matches!(
            normalize_form(&observation.form).as_str(),
            "10-Q" | "10-Q/A"
        ) =>
        {
            FactPeriodType::Quarterly
        }
        _ => FactPeriodType::OtherDuration,
    }
}

fn choose_unit(metric: SecInvestorMetric, observations: &[&SecFactObservation]) -> Option<String> {
    let preferred: &[&str] = match metric {
        SecInvestorMetric::DilutedShares => &["shares"],
        _ => &["USD"],
    };
    let counts =
        observations
            .iter()
            .fold(BTreeMap::<&str, usize>::new(), |mut counts, observation| {
                *counts.entry(observation.unit.as_str()).or_default() += 1;
                counts
            });
    for unit in preferred {
        if counts.contains_key(unit) {
            return Some((*unit).to_owned());
        }
    }
    counts
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(unit, _)| unit.to_owned())
}

fn detect_restatements(
    metric: SecInvestorMetric,
    unit: &str,
    observations: &[&SecFactObservation],
    options: &SecFactAnalysisOptions,
) -> Vec<FactRestatement> {
    let mut groups =
        BTreeMap::<(Option<String>, Option<String>, String), Vec<&SecFactObservation>>::new();
    for observation in observations {
        let Some(value) = observation.numeric_value() else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        groups
            .entry((
                observation.start.clone(),
                observation.end.clone(),
                observation.fiscal_period.clone().unwrap_or_default(),
            ))
            .or_default()
            .push(observation);
    }
    let mut output = Vec::new();
    for ((start, end, _), mut group) in groups {
        let Some(end) = end else {
            continue;
        };
        group.sort_unstable_by(|left, right| {
            left.filed
                .cmp(&right.filed)
                .then_with(|| left.accession_number.cmp(&right.accession_number))
        });
        let Some(first) = group.first() else {
            continue;
        };
        let Some(last) = group.last() else {
            continue;
        };
        let Some(earliest) = first.numeric_value() else {
            continue;
        };
        let Some(latest) = last.numeric_value() else {
            continue;
        };
        let absolute_revision = latest - earliest;
        let relative_revision = (earliest.abs() > options.restatement_absolute_tolerance)
            .then_some(absolute_revision / earliest.abs());
        let materially_distinct = absolute_revision.abs() > options.restatement_absolute_tolerance
            && relative_revision
                .is_none_or(|value| value.abs() > options.restatement_relative_tolerance);
        let distinct_accessions = group
            .iter()
            .map(|observation| observation.accession_number.as_str())
            .collect::<BTreeSet<_>>();
        if materially_distinct && distinct_accessions.len() > 1 {
            output.push(FactRestatement {
                metric,
                unit: unit.to_owned(),
                start,
                end,
                earliest_value: earliest,
                latest_value: latest,
                absolute_revision,
                relative_revision,
                earliest_filed: first.filed.clone(),
                latest_filed: last.filed.clone(),
                accession_numbers: distinct_accessions.into_iter().map(str::to_owned).collect(),
                fact_ids: group
                    .iter()
                    .map(|observation| observation.id.clone())
                    .collect(),
            });
        }
    }
    output.sort_unstable_by(|left, right| {
        right
            .end
            .cmp(&left.end)
            .then_with(|| left.start.cmp(&right.start))
    });
    output
}

fn latest_delta(points: &[FactPoint], options: &SecFactAnalysisOptions) -> Option<MetricDelta> {
    let current = points.last()?.clone();
    let previous_index = comparable_previous_index(points, points.len() - 1)?;
    let previous = points[previous_index].clone();
    let absolute_change = current.value - previous.value;
    let relative_change =
        (previous.value.abs() > 1.0e-12).then_some(absolute_change / previous.value.abs());
    let comparison_basis = comparison_basis(&current, &previous).to_owned();
    let history = historical_relative_changes(points, current.period_type);
    let robust_z_score = if history.len() >= options.minimum_points_for_anomaly {
        relative_change.and_then(|latest| robust_z(latest, &history))
    } else {
        None
    };
    Some(MetricDelta {
        current,
        previous,
        absolute_change,
        relative_change,
        comparison_basis,
        robust_z_score,
    })
}

fn comparable_previous_index(points: &[FactPoint], current_index: usize) -> Option<usize> {
    let current = &points[current_index];
    let candidates = &points[..current_index];
    match current.period_type {
        FactPeriodType::Annual => candidates
            .iter()
            .rposition(|point| point.period_type == FactPeriodType::Annual),
        FactPeriodType::Quarterly => {
            let same_period = candidates.iter().rposition(|point| {
                point.period_type == FactPeriodType::Quarterly
                    && point.fiscal_period == current.fiscal_period
                    && point.fiscal_year != current.fiscal_year
            });
            same_period.or_else(|| {
                candidates
                    .iter()
                    .rposition(|point| point.period_type == FactPeriodType::Quarterly)
            })
        }
        FactPeriodType::Instant => candidates
            .iter()
            .rposition(|point| point.period_type == FactPeriodType::Instant),
        FactPeriodType::OtherDuration => candidates
            .iter()
            .rposition(|point| point.period_type == FactPeriodType::OtherDuration),
    }
}

fn comparison_basis(current: &FactPoint, previous: &FactPoint) -> &'static str {
    match current.period_type {
        FactPeriodType::Annual => "year_over_year",
        FactPeriodType::Quarterly
            if current.fiscal_period == previous.fiscal_period
                && current.fiscal_year != previous.fiscal_year =>
        {
            "year_over_year_quarter"
        }
        FactPeriodType::Quarterly => "sequential_quarter",
        FactPeriodType::Instant => "prior_reported_instant",
        FactPeriodType::OtherDuration => "prior_duration",
    }
}

fn historical_relative_changes(points: &[FactPoint], period_type: FactPeriodType) -> Vec<f64> {
    let filtered = points
        .iter()
        .filter(|point| point.period_type == period_type)
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for index in 1..filtered.len() {
        let previous = filtered[index - 1].value;
        if previous.abs() > 1.0e-12 {
            output.push((filtered[index].value - previous) / previous.abs());
        }
    }
    output
}

fn robust_z(value: f64, history: &[f64]) -> Option<f64> {
    if history.is_empty() {
        return None;
    }
    let mut sorted = history.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    // Deliberately not named `median`: shadowing the free function here makes
    // the median-absolute-deviation call below a call on an `f64`.
    let center = median(&sorted);
    let mut deviations = sorted
        .iter()
        .map(|observation| (observation - center).abs())
        .collect::<Vec<_>>();
    deviations.sort_unstable_by(f64::total_cmp);
    let mad = median(&deviations);
    if mad <= 1.0e-12 {
        return None;
    }
    Some(0.674_489_75 * (value - center) / mad)
}

fn derive_difference_metric(
    metric: SecInvestorMetric,
    left_metric: SecInvestorMetric,
    right_metric: SecInvestorMetric,
    series: &mut BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
) {
    let Some(left) = series.get(&left_metric).cloned() else {
        return;
    };
    let Some(right) = series.get(&right_metric).cloned() else {
        return;
    };
    let points = combine_points(metric, &left.points, &right.points, |left, right| {
        left - right
    });
    insert_derived_series(metric, points, series, options);
}

fn derive_sum_metric(
    metric: SecInvestorMetric,
    left_metric: SecInvestorMetric,
    right_metric: SecInvestorMetric,
    series: &mut BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
) {
    let Some(left) = series.get(&left_metric).cloned() else {
        return;
    };
    let Some(right) = series.get(&right_metric).cloned() else {
        return;
    };
    let points = combine_points(metric, &left.points, &right.points, |left, right| {
        left + right
    });
    insert_derived_series(metric, points, series, options);
}

fn derive_ratio_metric(
    metric: SecInvestorMetric,
    numerator_metric: SecInvestorMetric,
    denominator_metric: SecInvestorMetric,
    series: &mut BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
) {
    let Some(numerator) = series.get(&numerator_metric).cloned() else {
        return;
    };
    let Some(denominator) = series.get(&denominator_metric).cloned() else {
        return;
    };
    let points = combine_points(
        metric,
        &numerator.points,
        &denominator.points,
        |left, right| {
            if right.abs() <= 1.0e-12 {
                f64::NAN
            } else {
                left / right
            }
        },
    )
    .into_iter()
    .filter(|point| point.value.is_finite())
    .collect();
    insert_derived_series(metric, points, series, options);
}

fn combine_points(
    metric: SecInvestorMetric,
    left: &[FactPoint],
    right: &[FactPoint],
    operation: impl Fn(f64, f64) -> f64,
) -> Vec<FactPoint> {
    let right_map = right
        .iter()
        .map(|point| (PeriodKey::from_point(point), point))
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    for left_point in left {
        let key = PeriodKey::from_point(left_point);
        let Some(right_point) = right_map.get(&key) else {
            continue;
        };
        let value = operation(left_point.value, right_point.value);
        if !value.is_finite() {
            continue;
        }
        // A derived point is only as fresh as its stalest input, so attribute
        // it to whichever side was filed later.
        let later = if left_point.filed >= right_point.filed {
            left_point
        } else {
            *right_point
        };
        let filed = later.filed.clone();
        let accession_number = later.accession_number.clone();
        output.push(FactPoint {
            metric,
            value,
            unit: metric.expected_unit().to_owned(),
            start: left_point.start.clone(),
            end: left_point.end.clone(),
            fiscal_year: left_point.fiscal_year.or(right_point.fiscal_year),
            fiscal_period: left_point
                .fiscal_period
                .clone()
                .or_else(|| right_point.fiscal_period.clone()),
            period_type: left_point.period_type,
            form: left_point.form.clone(),
            filed,
            accession_number,
            taxonomy: "derived".to_owned(),
            concept: format!("{:?}", metric),
            fact_id: format!("derived:{}:{}", left_point.fact_id, right_point.fact_id),
            derived_from_fact_ids: vec![left_point.fact_id.clone(), right_point.fact_id.clone()],
        });
    }
    output
}

fn insert_derived_series(
    metric: SecInvestorMetric,
    mut points: Vec<FactPoint>,
    series: &mut BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
) {
    if points.is_empty() {
        return;
    }
    points.sort_unstable_by(|left, right| {
        left.end
            .cmp(&right.end)
            .then_with(|| left.period_type.cmp(&right.period_type))
    });
    if points.len() > options.maximum_points_per_metric {
        points.drain(..points.len() - options.maximum_points_per_metric);
    }
    let latest_delta = latest_delta(&points, options);
    series.insert(
        metric,
        MetricSeries {
            metric,
            taxonomy: "derived".to_owned(),
            concept: format!("{:?}", metric),
            label: format!("{:?}", metric),
            unit: metric.expected_unit().to_owned(),
            points,
            latest_delta,
            restatements: Vec::new(),
        },
    );
}

fn build_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
) -> Vec<FactAlert> {
    let mut alerts = Vec::new();
    for metric_series in series.values() {
        for restatement in &metric_series.restatements {
            let magnitude = restatement
                .relative_revision
                .map_or(0.5, |value| (value.abs() / 0.25).clamp(0.25, 1.0));
            alerts.push(FactAlert {
                kind: FactAlertKind::Restatement,
                code: "fact_restatement".to_owned(),
                title: format!(
                    "{:?}: {}",
                    metric_series.metric,
                    alert_kind_title(FactAlertKind::Restatement)
                ),
                severity: (0.55 + 0.45 * magnitude as f32).clamp(0.0, 1.0),
                metric: metric_series.metric,
                period_end: restatement.end.clone(),
                rationale: format!(
                    "earliest reported value {} changed to {} across {} accessions",
                    restatement.earliest_value,
                    restatement.latest_value,
                    restatement.accession_numbers.len()
                ),
                current_value: restatement.latest_value,
                previous_value: Some(restatement.earliest_value),
                relative_change: restatement.relative_revision,
                robust_z_score: None,
                accession_numbers: restatement.accession_numbers.clone(),
            });
        }
        let Some(delta) = &metric_series.latest_delta else {
            continue;
        };
        if delta
            .relative_change
            .is_some_and(|value| value.abs() >= options.material_change_fraction)
        {
            alerts.push(delta_alert(
                FactAlertKind::MaterialChange,
                "material_fact_change",
                metric_series.metric,
                delta,
                (delta.relative_change.unwrap_or_default().abs() / 0.50) as f32,
                format!(
                    "{:?} changed {:+.1}% on a {} basis",
                    metric_series.metric,
                    delta.relative_change.unwrap_or_default() * 100.0,
                    delta.comparison_basis
                ),
            ));
        }
        if delta
            .robust_z_score
            .is_some_and(|value| value.abs() >= options.anomaly_mad_threshold)
        {
            alerts.push(delta_alert(
                FactAlertKind::StatisticalAnomaly,
                "fact_change_anomaly",
                metric_series.metric,
                delta,
                (delta.robust_z_score.unwrap_or_default().abs() / 8.0) as f32,
                format!(
                    "latest change has robust z-score {:.2}",
                    delta.robust_z_score.unwrap_or_default()
                ),
            ));
        }
    }
    add_margin_alerts(series, options, &mut alerts);
    add_balance_sheet_alerts(series, options, &mut alerts);
    add_cash_conversion_alerts(series, options, &mut alerts);
    add_dilution_alerts(series, options, &mut alerts);
    add_working_capital_alerts(series, options, &mut alerts);
    add_stock_comp_alerts(series, options, &mut alerts);
    alerts
}

/// Human-readable headline for each alert kind.
///
/// Every delta-derived alert previously shared one "changed materially"
/// headline, which read wrong on margin expansion, leverage reduction, and
/// the other directional kinds.
const fn alert_kind_title(kind: FactAlertKind) -> &'static str {
    match kind {
        FactAlertKind::MaterialChange => "material change",
        FactAlertKind::StatisticalAnomaly => "statistically anomalous change",
        FactAlertKind::Restatement => "prior-period value changed",
        FactAlertKind::MarginCompression => "margin compression",
        FactAlertKind::MarginExpansion => "margin expansion",
        FactAlertKind::LeverageIncrease => "leverage increase",
        FactAlertKind::LeverageReduction => "leverage reduction",
        FactAlertKind::CashConversionDeterioration => "cash conversion deteriorated",
        FactAlertKind::CashConversionImprovement => "cash conversion improved",
        FactAlertKind::Dilution => "share dilution",
        FactAlertKind::WorkingCapitalBuild => "working-capital build",
        FactAlertKind::StockCompensationAcceleration => "stock-compensation acceleration",
    }
}

fn delta_alert(
    kind: FactAlertKind,
    code: &str,
    metric: SecInvestorMetric,
    delta: &MetricDelta,
    severity: f32,
    rationale: String,
) -> FactAlert {
    FactAlert {
        kind,
        code: code.to_owned(),
        title: format!("{metric:?}: {}", alert_kind_title(kind)),
        severity: severity.clamp(0.0, 1.0),
        metric,
        period_end: delta.current.end.clone(),
        rationale,
        current_value: delta.current.value,
        previous_value: Some(delta.previous.value),
        relative_change: delta.relative_change,
        robust_z_score: delta.robust_z_score,
        accession_numbers: vec![
            delta.previous.accession_number.clone(),
            delta.current.accession_number.clone(),
        ],
    }
}

fn add_margin_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    for metric in [
        SecInvestorMetric::GrossMargin,
        SecInvestorMetric::OperatingMargin,
    ] {
        let Some(delta) = series
            .get(&metric)
            .and_then(|series| series.latest_delta.as_ref())
        else {
            continue;
        };
        if delta.absolute_change.abs() < options.margin_alert_points {
            continue;
        }
        let kind = if delta.absolute_change < 0.0 {
            FactAlertKind::MarginCompression
        } else {
            FactAlertKind::MarginExpansion
        };
        alerts.push(delta_alert(
            kind,
            if delta.absolute_change < 0.0 {
                "margin_compression"
            } else {
                "margin_expansion"
            },
            metric,
            delta,
            (delta.absolute_change.abs() as f32 / 0.10).clamp(0.0, 1.0),
            format!(
                "margin changed {:+.1} percentage points",
                delta.absolute_change * 100.0
            ),
        ));
    }
}

fn add_balance_sheet_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    let Some(delta) = series
        .get(&SecInvestorMetric::NetDebt)
        .and_then(|series| series.latest_delta.as_ref())
    else {
        return;
    };
    let Some(relative) = delta.relative_change else {
        return;
    };
    if relative.abs() < options.material_change_fraction {
        return;
    }
    alerts.push(delta_alert(
        if relative > 0.0 {
            FactAlertKind::LeverageIncrease
        } else {
            FactAlertKind::LeverageReduction
        },
        if relative > 0.0 {
            "net_debt_increase"
        } else {
            "net_debt_reduction"
        },
        SecInvestorMetric::NetDebt,
        delta,
        (relative.abs() as f32 / 0.50).clamp(0.0, 1.0),
        format!("net debt changed {:+.1}%", relative * 100.0),
    ));
}

fn add_cash_conversion_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    let Some(revenue) = latest_relative(series, SecInvestorMetric::Revenue) else {
        return;
    };
    let Some(cash_flow) = latest_relative(series, SecInvestorMetric::OperatingCashFlow) else {
        return;
    };
    let spread = cash_flow - revenue;
    if spread.abs() < options.working_capital_spread_fraction {
        return;
    }
    let metric_series = &series[&SecInvestorMetric::OperatingCashFlow];
    let delta = metric_series.latest_delta.as_ref().expect("delta exists");
    alerts.push(delta_alert(
        if spread < 0.0 {
            FactAlertKind::CashConversionDeterioration
        } else {
            FactAlertKind::CashConversionImprovement
        },
        if spread < 0.0 {
            "cash_conversion_deterioration"
        } else {
            "cash_conversion_improvement"
        },
        SecInvestorMetric::OperatingCashFlow,
        delta,
        (spread.abs() as f32 / 0.50).clamp(0.0, 1.0),
        if spread < 0.0 {
            format!(
                "operating cash-flow growth trailed revenue growth by {:.1} percentage points",
                -spread * 100.0
            )
        } else {
            format!(
                "operating cash-flow growth outpaced revenue growth by {:.1} percentage points",
                spread * 100.0
            )
        },
    ));
}

fn add_dilution_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    let Some(delta) = series
        .get(&SecInvestorMetric::DilutedShares)
        .and_then(|series| series.latest_delta.as_ref())
    else {
        return;
    };
    if delta
        .relative_change
        .is_none_or(|change| change < options.dilution_alert_fraction)
    {
        return;
    }
    alerts.push(delta_alert(
        FactAlertKind::Dilution,
        "diluted_share_growth",
        SecInvestorMetric::DilutedShares,
        delta,
        (delta.relative_change.unwrap_or_default() as f32 / 0.15).clamp(0.0, 1.0),
        format!(
            "diluted shares increased {:.1}%",
            delta.relative_change.unwrap_or_default() * 100.0
        ),
    ));
}

fn add_working_capital_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    let Some(revenue) = latest_relative(series, SecInvestorMetric::Revenue) else {
        return;
    };
    for metric in [
        SecInvestorMetric::AccountsReceivable,
        SecInvestorMetric::Inventory,
    ] {
        let Some(change) = latest_relative(series, metric) else {
            continue;
        };
        let spread = change - revenue;
        if spread < options.working_capital_spread_fraction {
            continue;
        }
        let delta = series[&metric].latest_delta.as_ref().expect("delta exists");
        alerts.push(delta_alert(
            FactAlertKind::WorkingCapitalBuild,
            "working_capital_build",
            metric,
            delta,
            (spread as f32 / 0.50).clamp(0.0, 1.0),
            format!(
                "{:?} growth exceeded revenue growth by {:.1} percentage points",
                metric,
                spread * 100.0
            ),
        ));
    }
}

fn add_stock_comp_alerts(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    options: &SecFactAnalysisOptions,
    alerts: &mut Vec<FactAlert>,
) {
    let Some(revenue) = latest_relative(series, SecInvestorMetric::Revenue) else {
        return;
    };
    let Some(stock_comp) = latest_relative(series, SecInvestorMetric::StockBasedCompensation)
    else {
        return;
    };
    let spread = stock_comp - revenue;
    if spread < options.working_capital_spread_fraction {
        return;
    }
    let delta = series[&SecInvestorMetric::StockBasedCompensation]
        .latest_delta
        .as_ref()
        .expect("delta exists");
    alerts.push(delta_alert(
        FactAlertKind::StockCompensationAcceleration,
        "stock_compensation_acceleration",
        SecInvestorMetric::StockBasedCompensation,
        delta,
        (spread as f32 / 0.75).clamp(0.0, 1.0),
        format!(
            "stock-based compensation growth exceeded revenue growth by {:.1} percentage points",
            spread * 100.0
        ),
    ));
}

fn latest_relative(
    series: &BTreeMap<SecInvestorMetric, MetricSeries>,
    metric: SecInvestorMetric,
) -> Option<f64> {
    series.get(&metric)?.latest_delta.as_ref()?.relative_change
}

fn metric_aliases(metric: SecInvestorMetric) -> &'static [&'static str] {
    match metric {
        SecInvestorMetric::Revenue => &[
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "Revenues",
            "SalesRevenueNet",
        ],
        SecInvestorMetric::CostOfRevenue => &[
            "CostOfRevenue",
            "CostOfGoodsAndServicesSold",
            "CostOfGoodsSold",
        ],
        SecInvestorMetric::GrossProfit => &["GrossProfit"],
        SecInvestorMetric::ResearchAndDevelopment => &["ResearchAndDevelopmentExpense"],
        SecInvestorMetric::SellingGeneralAndAdministrative => {
            &["SellingGeneralAndAdministrativeExpense"]
        }
        SecInvestorMetric::OperatingIncome => &["OperatingIncomeLoss"],
        SecInvestorMetric::NetIncome => &["NetIncomeLoss", "ProfitLoss"],
        SecInvestorMetric::OperatingCashFlow => &[
            "NetCashProvidedByUsedInOperatingActivities",
            "NetCashProvidedByUsedInOperatingActivitiesContinuingOperations",
        ],
        SecInvestorMetric::CapitalExpenditures => &[
            "PaymentsToAcquirePropertyPlantAndEquipment",
            "PaymentsForAdditionsToPropertyPlantAndEquipment",
        ],
        SecInvestorMetric::Cash => &[
            "CashAndCashEquivalentsAtCarryingValue",
            "CashCashEquivalentsRestrictedCashAndRestrictedCashEquivalents",
        ],
        SecInvestorMetric::CurrentDebt => &[
            "LongTermDebtCurrent",
            "ShortTermBorrowings",
            "ShortTermDebtCurrent",
        ],
        SecInvestorMetric::LongTermDebt => &[
            "LongTermDebtNoncurrent",
            "LongTermDebtAndFinanceLeaseObligationsNoncurrent",
        ],
        SecInvestorMetric::CurrentAssets => &["AssetsCurrent"],
        SecInvestorMetric::CurrentLiabilities => &["LiabilitiesCurrent"],
        SecInvestorMetric::AccountsReceivable => &[
            "AccountsReceivableNetCurrent",
            "AccountsNotesAndLoansReceivableNetCurrent",
        ],
        SecInvestorMetric::Inventory => &["InventoryNet"],
        SecInvestorMetric::AccountsPayable => &["AccountsPayableCurrent"],
        SecInvestorMetric::DeferredRevenue => &[
            "ContractWithCustomerLiabilityCurrent",
            "DeferredRevenueCurrent",
        ],
        SecInvestorMetric::Goodwill => &["Goodwill"],
        SecInvestorMetric::IntangibleAssets => &[
            "FiniteLivedIntangibleAssetsNet",
            "IndefiniteLivedIntangibleAssetsExcludingGoodwill",
        ],
        SecInvestorMetric::ShareholdersEquity => &[
            "StockholdersEquity",
            "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        ],
        SecInvestorMetric::ShareRepurchases => &["PaymentsForRepurchaseOfCommonStock"],
        SecInvestorMetric::DividendsPaid => {
            &["PaymentsOfDividends", "PaymentsOfDividendsCommonStock"]
        }
        SecInvestorMetric::StockBasedCompensation => &["ShareBasedCompensation"],
        SecInvestorMetric::DilutedShares => &["WeightedAverageNumberOfDilutedSharesOutstanding"],
        SecInvestorMetric::InterestExpense => {
            &["InterestExpenseNonOperating", "InterestAndDebtExpense"]
        }
        SecInvestorMetric::IncomeTaxExpense => &["IncomeTaxExpenseBenefit"],
        SecInvestorMetric::OperatingLeaseLiability => &[
            "OperatingLeaseLiability",
            "OperatingLeaseLiabilityNoncurrent",
        ],
        metric if metric.is_derived() => &[],
        _ => &[],
    }
}

fn normalize_form(form: &str) -> String {
    form.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        FactAlertKind, SecFactAnalysisOptions, SecInvestorMetric, alert_kind_title,
        analyze_sec_companyfacts,
    };
    use crate::{SEC_NORMALIZED_FACTS_SCHEMA_VERSION, SecCompanyFacts, SecFactObservation};

    struct Period<'a> {
        start: Option<&'a str>,
        end: &'a str,
        fiscal_year: i64,
        fiscal_period: &'a str,
        filed: &'a str,
    }

    fn observation(id: &str, concept: &str, value: f64, period: &Period<'_>) -> SecFactObservation {
        SecFactObservation {
            id: format!("{id:0<64}"),
            taxonomy: "us-gaap".to_owned(),
            concept: concept.to_owned(),
            label: concept.to_owned(),
            description: String::new(),
            unit: if concept.contains("Shares") {
                "shares"
            } else {
                "USD"
            }
            .to_owned(),
            value: json!(value),
            start: period.start.map(str::to_owned),
            end: Some(period.end.to_owned()),
            accession_number: format!("accn-{id}"),
            fiscal_year: Some(period.fiscal_year),
            fiscal_period: Some(period.fiscal_period.to_owned()),
            form: if period.fiscal_period == "FY" {
                "10-K"
            } else {
                "10-Q"
            }
            .to_owned(),
            filed: period.filed.to_owned(),
            frame: None,
        }
    }

    #[test]
    fn derives_margins_and_flags_cash_conversion() {
        let mut observations = Vec::new();
        for (index, (year, revenue, gross, operating_cash)) in [
            (2021, 100.0, 50.0, 20.0),
            (2022, 120.0, 58.0, 23.0),
            (2023, 140.0, 64.0, 24.0),
            (2024, 170.0, 70.0, 18.0),
            (2025, 220.0, 80.0, 10.0),
        ]
        .into_iter()
        .enumerate()
        {
            let start = format!("{year}-01-01");
            let end = format!("{year}-12-31");
            let filed = format!("{}-02-01", year + 1);
            let period = Period {
                start: Some(&start),
                end: &end,
                fiscal_year: year,
                fiscal_period: "FY",
                filed: &filed,
            };
            observations.push(observation(
                &format!("r{index}"),
                "RevenueFromContractWithCustomerExcludingAssessedTax",
                revenue,
                &period,
            ));
            observations.push(observation(
                &format!("g{index}"),
                "GrossProfit",
                gross,
                &period,
            ));
            observations.push(observation(
                &format!("c{index}"),
                "NetCashProvidedByUsedInOperatingActivities",
                operating_cash,
                &period,
            ));
        }
        let facts = SecCompanyFacts {
            schema_version: SEC_NORMALIZED_FACTS_SCHEMA_VERSION,
            cik: 1,
            entity_name: "Issuer".to_owned(),
            tickers: vec!["TEST".to_owned()],
            source_url: "https://example.invalid".to_owned(),
            raw_sha256: "0".repeat(64),
            normalized_at_unix: 0,
            observations,
        };
        let analysis =
            analyze_sec_companyfacts(&facts, &SecFactAnalysisOptions::default()).expect("analysis");
        assert!(
            analysis
                .metric_series
                .iter()
                .any(|series| series.metric == SecInvestorMetric::GrossMargin)
        );
        assert!(
            analysis
                .alerts
                .iter()
                .any(|alert| { alert.kind == FactAlertKind::CashConversionDeterioration })
        );
    }

    #[test]
    fn detects_prior_period_value_revision() {
        let start = "2024-01-01";
        let end = "2024-12-31";
        let period = |filed| Period {
            start: Some(start),
            end,
            fiscal_year: 2024,
            fiscal_period: "FY",
            filed,
        };
        let mut first = observation("first", "NetIncomeLoss", 100.0, &period("2025-02-01"));
        first.accession_number = "original".to_owned();
        let mut revised = observation("revised", "NetIncomeLoss", 80.0, &period("2025-04-01"));
        revised.accession_number = "amendment".to_owned();
        let facts = SecCompanyFacts {
            schema_version: SEC_NORMALIZED_FACTS_SCHEMA_VERSION,
            cik: 1,
            entity_name: "Issuer".to_owned(),
            tickers: Vec::new(),
            source_url: "https://example.invalid".to_owned(),
            raw_sha256: "0".repeat(64),
            normalized_at_unix: 0,
            observations: vec![first, revised],
        };
        let analysis =
            analyze_sec_companyfacts(&facts, &SecFactAnalysisOptions::default()).expect("analysis");
        let net_income = analysis
            .metric_series
            .iter()
            .find(|series| series.metric == SecInvestorMetric::NetIncome)
            .expect("net income");
        assert_eq!(net_income.restatements.len(), 1);
    }

    /// Guards the median-absolute-deviation path in `robust_z`: a local named
    /// `median` used to shadow the `median` function, so the MAD term could
    /// never be computed. A steady growth history followed by one violent jump
    /// must score far outside the MAD threshold.
    #[test]
    fn flags_a_statistical_anomaly_against_a_steady_history() {
        let growth = [1.10, 1.12, 1.08, 1.11, 1.09, 1.10, 1.12, 1.09];
        let mut value = 100.0_f64;
        let mut observations = Vec::new();
        let push = |index: usize, year: i64, value: f64, observations: &mut Vec<_>| {
            let start = format!("{year}-01-01");
            let end = format!("{year}-12-31");
            let filed = format!("{}-02-01", year + 1);
            observations.push(observation(
                &format!("r{index}"),
                "Revenues",
                value,
                &Period {
                    start: Some(&start),
                    end: &end,
                    fiscal_year: year,
                    fiscal_period: "FY",
                    filed: &filed,
                },
            ));
        };
        push(0, 2016, value, &mut observations);
        for (index, factor) in growth.iter().enumerate() {
            value *= factor;
            push(index + 1, 2017 + index as i64, value, &mut observations);
        }
        // One order-of-magnitude jump, far outside the historical spread.
        push(growth.len() + 1, 2025, value * 5.0, &mut observations);

        let facts = SecCompanyFacts {
            schema_version: SEC_NORMALIZED_FACTS_SCHEMA_VERSION,
            cik: 1,
            entity_name: "Issuer".to_owned(),
            tickers: Vec::new(),
            source_url: "https://example.invalid".to_owned(),
            raw_sha256: "0".repeat(64),
            normalized_at_unix: 0,
            observations,
        };
        let analysis =
            analyze_sec_companyfacts(&facts, &SecFactAnalysisOptions::default()).expect("analysis");
        let revenue = analysis
            .metric_series
            .iter()
            .find(|series| series.metric == SecInvestorMetric::Revenue)
            .expect("revenue");
        let z = revenue
            .latest_delta
            .as_ref()
            .expect("latest delta")
            .robust_z_score
            .expect("robust z-score");
        assert!(z > 3.5, "expected a large positive robust z-score, got {z}");
        assert!(
            analysis
                .alerts
                .iter()
                .any(|alert| alert.kind == FactAlertKind::StatisticalAnomaly)
        );
    }

    /// A derived point is only as fresh as its stalest input, so it must carry
    /// the later-filed side's filing date and accession number.
    #[test]
    fn derived_points_are_attributed_to_the_later_filing() {
        let start = "2024-01-01";
        let end = "2024-12-31";
        let mut cash_flow = observation(
            "ocf",
            "NetCashProvidedByUsedInOperatingActivities",
            500.0,
            &Period {
                start: Some(start),
                end,
                fiscal_year: 2024,
                fiscal_period: "FY",
                filed: "2025-02-01",
            },
        );
        cash_flow.accession_number = "earlier".to_owned();
        let mut capex = observation(
            "capex",
            "PaymentsToAcquirePropertyPlantAndEquipment",
            200.0,
            &Period {
                start: Some(start),
                end,
                fiscal_year: 2024,
                fiscal_period: "FY",
                filed: "2025-05-01",
            },
        );
        capex.accession_number = "later".to_owned();
        let facts = SecCompanyFacts {
            schema_version: SEC_NORMALIZED_FACTS_SCHEMA_VERSION,
            cik: 1,
            entity_name: "Issuer".to_owned(),
            tickers: Vec::new(),
            source_url: "https://example.invalid".to_owned(),
            raw_sha256: "0".repeat(64),
            normalized_at_unix: 0,
            observations: vec![cash_flow, capex],
        };
        let analysis =
            analyze_sec_companyfacts(&facts, &SecFactAnalysisOptions::default()).expect("analysis");
        let free_cash_flow = analysis
            .metric_series
            .iter()
            .find(|series| series.metric == SecInvestorMetric::FreeCashFlow)
            .expect("free cash flow");
        let point = free_cash_flow.points.first().expect("derived point");
        assert!((point.value - 300.0).abs() < 1.0e-9);
        assert_eq!(point.filed, "2025-05-01");
        assert_eq!(point.accession_number, "later");
        assert_eq!(point.derived_from_fact_ids.len(), 2);
    }

    /// Directional alert kinds must not all announce themselves as a generic
    /// "changed materially".
    #[test]
    fn alert_titles_name_their_kind() {
        assert_eq!(
            alert_kind_title(FactAlertKind::MarginExpansion),
            "margin expansion"
        );
        assert_eq!(
            alert_kind_title(FactAlertKind::LeverageReduction),
            "leverage reduction"
        );
    }
}
