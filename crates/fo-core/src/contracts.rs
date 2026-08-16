use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{FoError, Result};

pub const CONTRACT_ANALYSIS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractProfile {
    General,
    RetailLease,
    ProfessionalServices,
    Nda,
}

impl Default for ContractProfile {
    fn default() -> Self {
        Self::General
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseKind {
    Parties,
    Recitals,
    Definitions,
    Term,
    Renewal,
    Scope,
    Deliverables,
    Milestones,
    Acceptance,
    ChangeControl,
    Fees,
    Invoicing,
    Payment,
    Expenses,
    Taxes,
    Confidentiality,
    ConfidentialInformation,
    ConfidentialityExclusions,
    UseRestrictions,
    PermittedRecipients,
    CompelledDisclosure,
    ReturnOrDestruction,
    Residuals,
    NoLicense,
    IntellectualProperty,
    WorkProduct,
    BackgroundIp,
    OpenSource,
    DataProtection,
    DataSecurity,
    ServiceLevels,
    Staffing,
    Subcontracting,
    Audit,
    Compliance,
    Representations,
    Warranties,
    Indemnification,
    LimitationOfLiability,
    Insurance,
    Assignment,
    ChangeOfControl,
    Termination,
    TransitionAssistance,
    ForceMajeure,
    GoverningLaw,
    DisputeResolution,
    Notices,
    EntireAgreement,
    Amendment,
    Waiver,
    Severability,
    Survival,
    Premises,
    PermittedUse,
    BaseRent,
    PercentageRent,
    CommonAreaMaintenance,
    Utilities,
    MaintenanceAndRepair,
    Alterations,
    Signage,
    AssignmentAndSubletting,
    CoTenancy,
    Exclusivity,
    GoDark,
    KickOut,
    RadiusRestriction,
    RenewalOption,
    SecurityDeposit,
    TenantImprovement,
    DeliveryCondition,
    OpeningCovenant,
    OperatingHours,
    Casualty,
    Condemnation,
    SubordinationNondisturbance,
    Estoppel,
    Holdover,
    Surrender,
    Guaranty,
    Standstill,
    NonSolicit,
    NoHire,
    NonCircumvention,
    InjunctiveRelief,
    Miscellaneous,
    Unclassified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClauseClassification {
    pub kind: ClauseKind,
    pub confidence: f32,
    pub matched_signals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractClause {
    pub index: usize,
    pub heading: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub text: String,
    pub classifications: Vec<ClauseClassification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinedTerm {
    pub term: String,
    pub definition: String,
    pub clause_index: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationModality {
    Shall,
    ShallNot,
    Must,
    MustNot,
    Will,
    May,
    MayNot,
    Should,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractObligation {
    pub clause_index: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub subject: String,
    pub modality: ObligationModality,
    pub action: String,
    pub trigger: Option<String>,
    pub deadline: Option<String>,
    pub remedy: Option<String>,
    pub confidence: f32,
    pub sentence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EconomicTermKind {
    Money,
    Percentage,
    Duration,
    NoticePeriod,
    PaymentTerm,
    BaseRent,
    RentEscalation,
    PercentageRent,
    SecurityDeposit,
    TenantImprovementAllowance,
    LiabilityCap,
    InsuranceLimit,
    ServiceLevel,
    InterestRate,
    RenewalTerm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicTerm {
    pub kind: EconomicTermKind,
    pub clause_index: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub raw_value: String,
    pub normalized_value: Option<f64>,
    pub unit: Option<String>,
    pub currency: Option<String>,
    pub context: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractWarning {
    pub code: String,
    pub message: String,
    pub clause_indexes: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContractAnalysisOptions {
    pub minimum_clause_characters: usize,
    pub maximum_clause_characters: usize,
    pub maximum_clauses: usize,
    pub maximum_definitions: usize,
    pub maximum_obligations: usize,
    pub maximum_economic_terms: usize,
    pub maximum_classifications_per_clause: usize,
}

impl Default for ContractAnalysisOptions {
    fn default() -> Self {
        Self {
            minimum_clause_characters: 24,
            maximum_clause_characters: 64_000,
            maximum_clauses: 10_000,
            maximum_definitions: 20_000,
            maximum_obligations: 100_000,
            maximum_economic_terms: 100_000,
            maximum_classifications_per_clause: 4,
        }
    }
}

impl ContractAnalysisOptions {
    pub fn validate(&self) -> Result<()> {
        if self.minimum_clause_characters == 0
            || self.maximum_clause_characters < self.minimum_clause_characters
            || self.maximum_clauses == 0
            || self.maximum_definitions == 0
            || self.maximum_obligations == 0
            || self.maximum_economic_terms == 0
            || self.maximum_classifications_per_clause == 0
            || self.maximum_classifications_per_clause > 32
        {
            return Err(FoError::InvalidConfig(
                "contract analysis limits are outside safe bounds".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractAnalysis {
    pub schema_version: u32,
    pub profile: ContractProfile,
    pub bytes: usize,
    pub characters: usize,
    pub clauses: Vec<ContractClause>,
    pub definitions: Vec<DefinedTerm>,
    pub obligations: Vec<ContractObligation>,
    pub economic_terms: Vec<EconomicTerm>,
    pub clause_counts: BTreeMap<ClauseKind, usize>,
    pub warnings: Vec<ContractWarning>,
}

pub fn analyze_contract(
    text: &str,
    profile: ContractProfile,
    options: &ContractAnalysisOptions,
) -> Result<ContractAnalysis> {
    options.validate()?;
    if text.trim().is_empty() {
        return Err(FoError::InvalidConfig(
            "contract text must not be empty".to_owned(),
        ));
    }
    let mut clauses = segment_contract(text, profile, options);
    for clause in &mut clauses {
        clause.classifications = classify_clause(
            &clause.heading,
            &clause.text,
            profile,
            options.maximum_classifications_per_clause,
        );
    }
    let definitions = extract_definitions(text, &clauses, options.maximum_definitions);
    let obligations = extract_obligations(text, &clauses, options.maximum_obligations);
    let economic_terms = extract_economic_terms(text, &clauses, options.maximum_economic_terms);
    let mut clause_counts = BTreeMap::new();
    for clause in &clauses {
        let kind = clause
            .classifications
            .first()
            .map_or(ClauseKind::Unclassified, |classification| classification.kind);
        *clause_counts.entry(kind).or_insert(0usize) += 1;
    }
    let warnings = build_warnings(profile, &clauses, &definitions, &economic_terms);
    Ok(ContractAnalysis {
        schema_version: CONTRACT_ANALYSIS_SCHEMA_VERSION,
        profile,
        bytes: text.len(),
        characters: text.chars().count(),
        clauses,
        definitions,
        obligations,
        economic_terms,
        clause_counts,
        warnings,
    })
}

fn segment_contract(
    text: &str,
    profile: ContractProfile,
    options: &ContractAnalysisOptions,
) -> Vec<ContractClause> {
    let mut starts = Vec::<(usize, String)>::new();
    for (line_start, line) in line_offsets(text) {
        let trimmed = line.trim();
        if is_heading(trimmed, profile) {
            starts.push((line_start, clean_heading(trimmed)));
            if starts.len() >= options.maximum_clauses {
                break;
            }
        }
    }
    if starts.is_empty() {
        starts.push((0, "Document".to_owned()));
    } else if starts[0].0 > 0 && text[..starts[0].0].trim().len() >= options.minimum_clause_characters {
        starts.insert(0, (0, "Preamble".to_owned()));
    }

    let mut clauses = Vec::new();
    for (index, (start, heading)) in starts.iter().enumerate() {
        let mut end = starts.get(index + 1).map_or(text.len(), |next| next.0);
        if end.saturating_sub(*start) > options.maximum_clause_characters {
            end = start.saturating_add(options.maximum_clause_characters);
            while end < text.len() && !text.is_char_boundary(end) {
                end -= 1;
            }
        }
        let body = text.get(*start..end).unwrap_or_default().trim();
        if body.len() < options.minimum_clause_characters {
            continue;
        }
        let leading = text[*start..end].find(body).unwrap_or(0);
        let actual_start = start.saturating_add(leading);
        let actual_end = actual_start.saturating_add(body.len());
        clauses.push(ContractClause {
            index: clauses.len(),
            heading: heading.clone(),
            start_byte: actual_start,
            end_byte: actual_end,
            text: body.to_owned(),
            classifications: Vec::new(),
        });
    }
    if clauses.is_empty() {
        clauses.push(ContractClause {
            index: 0,
            heading: "Document".to_owned(),
            start_byte: 0,
            end_byte: text.len(),
            text: text.to_owned(),
            classifications: Vec::new(),
        });
    }
    clauses
}

fn line_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for line in text.split_inclusive('\n') {
        lines.push((start, line.trim_end_matches(['\r', '\n'])));
        start += line.len();
    }
    if start < text.len() {
        lines.push((start, &text[start..]));
    }
    lines
}

fn is_heading(line: &str, profile: ContractProfile) -> bool {
    if line.is_empty() || line.chars().count() > 180 || line.split_whitespace().count() > 20 {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("article ")
        || lower.starts_with("section ")
        || lower.starts_with("schedule ")
        || lower.starts_with("exhibit ")
        || lower.starts_with("appendix ")
    {
        return true;
    }
    if numbered_heading(line) {
        return true;
    }
    let letters = line.chars().filter(|character| character.is_alphabetic()).count();
    let uppercase = line.chars().filter(|character| character.is_uppercase()).count();
    if letters >= 3 && uppercase.saturating_mul(100) / letters.max(1) >= 75 {
        return true;
    }
    if line.ends_with(':') && line.split_whitespace().count() <= 12 {
        return true;
    }
    let profile_words: &[&str] = match profile {
        ContractProfile::RetailLease => &[
            "base rent", "percentage rent", "common area", "co-tenancy", "exclusive use",
            "tenant improvements", "security deposit", "renewal option", "permitted use",
        ],
        ContractProfile::ProfessionalServices => &[
            "scope of services", "deliverables", "acceptance", "change order", "service levels",
            "fees and expenses", "work product", "project schedule",
        ],
        ContractProfile::Nda => &[
            "confidential information", "exclusions", "permitted disclosure", "return or destruction",
            "residuals", "standstill", "non-solicitation",
        ],
        ContractProfile::General => &[
            "definitions", "term and termination", "confidentiality", "indemnification",
            "limitation of liability", "governing law", "notices",
        ],
    };
    profile_words.iter().any(|word| lower == *word)
}

fn numbered_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut seen_digit = false;
    let mut index = 0usize;
    for (offset, character) in trimmed.char_indices() {
        if character.is_ascii_digit() {
            seen_digit = true;
            index = offset + character.len_utf8();
        } else if matches!(character, '.' | ')' | '(' | '-' | ' ') && index < 16 {
            index = offset + character.len_utf8();
        } else {
            break;
        }
    }
    seen_digit
        && index > 0
        && index < trimmed.len()
        && trimmed[index..].trim().chars().any(char::is_alphabetic)
}

fn clean_heading(line: &str) -> String {
    let trimmed = line.trim().trim_end_matches(':');
    let mut cut = 0usize;
    for (offset, character) in trimmed.char_indices() {
        if character.is_ascii_digit() || matches!(character, '.' | ')' | '(' | '-' | ' ') {
            cut = offset + character.len_utf8();
        } else {
            break;
        }
    }
    let candidate = trimmed[cut.min(trimmed.len())..].trim();
    if candidate.is_empty() {
        trimmed.to_owned()
    } else {
        candidate.to_owned()
    }
}

fn classify_clause(
    heading: &str,
    text: &str,
    profile: ContractProfile,
    maximum: usize,
) -> Vec<ClauseClassification> {
    let heading_lower = heading.to_ascii_lowercase();
    let body_lower = text
        .chars()
        .take(8_000)
        .collect::<String>()
        .to_ascii_lowercase();
    let mut scored = Vec::new();
    for rule in classification_rules(profile) {
        let mut score = 0.0f32;
        let mut signals = Vec::new();
        for signal in rule.heading_signals {
            if heading_lower.contains(signal) {
                score += 0.72;
                signals.push(format!("heading:{signal}"));
                break;
            }
        }
        let mut body_hits = 0usize;
        for signal in rule.body_signals {
            if body_lower.contains(signal) {
                body_hits += 1;
                signals.push(format!("body:{signal}"));
                if body_hits >= 4 {
                    break;
                }
            }
        }
        score += (body_hits as f32 * 0.12).min(0.42);
        if score >= 0.30 {
            scored.push(ClauseClassification {
                kind: rule.kind,
                confidence: score.min(1.0),
                matched_signals: signals,
            });
        }
    }
    scored.sort_unstable_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    scored.dedup_by_key(|classification| classification.kind);
    scored.truncate(maximum);
    if scored.is_empty() {
        scored.push(ClauseClassification {
            kind: ClauseKind::Unclassified,
            confidence: 0.0,
            matched_signals: Vec::new(),
        });
    }
    scored
}

struct Rule {
    kind: ClauseKind,
    heading_signals: &'static [&'static str],
    body_signals: &'static [&'static str],
}

fn classification_rules(profile: ContractProfile) -> Vec<Rule> {
    let mut rules = vec![
        rule(ClauseKind::Parties, &["parties"], &["between", "party", "hereinafter"]),
        rule(ClauseKind::Recitals, &["recitals", "whereas"], &["whereas", "background"]),
        rule(ClauseKind::Definitions, &["definitions", "defined terms"], &[" means ", "shall mean", "defined as"]),
        rule(ClauseKind::Term, &["term", "duration"], &["effective date", "commence", "expire"]),
        rule(ClauseKind::Renewal, &["renewal", "extension"], &["renew", "extended term"]),
        rule(ClauseKind::Fees, &["fees", "compensation"], &["fee", "compensation", "rate"]),
        rule(ClauseKind::Invoicing, &["invoice", "billing"], &["invoice", "billing"]),
        rule(ClauseKind::Payment, &["payment", "payment terms"], &["pay", "net 30", "late payment"]),
        rule(ClauseKind::Taxes, &["taxes", "tax"], &["tax", "withholding"]),
        rule(ClauseKind::Confidentiality, &["confidentiality"], &["confidential", "non-public information"]),
        rule(ClauseKind::IntellectualProperty, &["intellectual property", "ownership"], &["intellectual property", "copyright", "patent"]),
        rule(ClauseKind::DataProtection, &["data protection", "privacy"], &["personal data", "privacy", "data protection"]),
        rule(ClauseKind::DataSecurity, &["security", "information security"], &["security controls", "breach", "cybersecurity"]),
        rule(ClauseKind::Audit, &["audit", "records"], &["audit", "books and records"]),
        rule(ClauseKind::Compliance, &["compliance", "laws"], &["comply with", "applicable law"]),
        rule(ClauseKind::Representations, &["representations"], &["represents", "representation"]),
        rule(ClauseKind::Warranties, &["warranties", "warranty"], &["warrants", "warranty"]),
        rule(ClauseKind::Indemnification, &["indemnification", "indemnity"], &["indemnify", "hold harmless", "defend"]),
        rule(ClauseKind::LimitationOfLiability, &["limitation of liability", "liability"], &["aggregate liability", "consequential damages", "liability shall not exceed"]),
        rule(ClauseKind::Insurance, &["insurance"], &["insurance", "coverage", "policy limit"]),
        rule(ClauseKind::Assignment, &["assignment"], &["assign", "transfer"]),
        rule(ClauseKind::ChangeOfControl, &["change of control"], &["change in control", "change of control"]),
        rule(ClauseKind::Termination, &["termination", "default"], &["terminate", "material breach", "cure period"]),
        rule(ClauseKind::ForceMajeure, &["force majeure"], &["force majeure", "beyond its reasonable control"]),
        rule(ClauseKind::GoverningLaw, &["governing law"], &["governed by", "laws of the state"]),
        rule(ClauseKind::DisputeResolution, &["dispute", "arbitration"], &["arbitration", "venue", "jurisdiction"]),
        rule(ClauseKind::Notices, &["notices", "notice"], &["notice shall", "delivered to"]),
        rule(ClauseKind::EntireAgreement, &["entire agreement"], &["entire agreement", "complete understanding"]),
        rule(ClauseKind::Amendment, &["amendment", "modification"], &["amended", "modified", "writing signed"]),
        rule(ClauseKind::Waiver, &["waiver"], &["waiver", "failure to enforce"]),
        rule(ClauseKind::Severability, &["severability"], &["invalid or unenforceable", "severed"]),
        rule(ClauseKind::Survival, &["survival"], &["survive termination", "survival"]),
    ];
    match profile {
        ContractProfile::RetailLease => rules.extend(retail_lease_rules()),
        ContractProfile::ProfessionalServices => rules.extend(professional_services_rules()),
        ContractProfile::Nda => rules.extend(nda_rules()),
        ContractProfile::General => {}
    }
    rules
}

const fn rule(
    kind: ClauseKind,
    heading_signals: &'static [&'static str],
    body_signals: &'static [&'static str],
) -> Rule {
    Rule {
        kind,
        heading_signals,
        body_signals,
    }
}

fn retail_lease_rules() -> Vec<Rule> {
    vec![
        rule(ClauseKind::Premises, &["premises", "demised premises"], &["premises", "square feet", "shopping center"]),
        rule(ClauseKind::PermittedUse, &["use", "permitted use"], &["permitted use", "use the premises"]),
        rule(ClauseKind::BaseRent, &["base rent", "minimum rent"], &["base rent", "minimum annual rent"]),
        rule(ClauseKind::PercentageRent, &["percentage rent"], &["gross sales", "percentage rent", "breakpoint"]),
        rule(ClauseKind::CommonAreaMaintenance, &["common area", "operating expenses", "cam"], &["common area maintenance", "operating expenses", "pro rata share"]),
        rule(ClauseKind::Utilities, &["utilities"], &["electricity", "water", "utilities"]),
        rule(ClauseKind::MaintenanceAndRepair, &["maintenance", "repairs"], &["maintain", "repair", "hvac"]),
        rule(ClauseKind::Alterations, &["alterations"], &["alteration", "landlord consent"]),
        rule(ClauseKind::Signage, &["signs", "signage"], &["signage", "sign criteria"]),
        rule(ClauseKind::AssignmentAndSubletting, &["assignment and subletting", "subletting"], &["sublease", "assignee", "recapture"]),
        rule(ClauseKind::CoTenancy, &["co-tenancy", "cotenancy"], &["co-tenancy", "occupancy threshold", "anchor tenant"]),
        rule(ClauseKind::Exclusivity, &["exclusive use", "exclusivity"], &["exclusive", "competing use"]),
        rule(ClauseKind::GoDark, &["go dark", "continuous operation"], &["continuously operate", "go dark"]),
        rule(ClauseKind::KickOut, &["kick-out", "termination right"], &["sales threshold", "terminate this lease"]),
        rule(ClauseKind::RadiusRestriction, &["radius restriction"], &["radius", "competing store"]),
        rule(ClauseKind::RenewalOption, &["renewal option", "option to extend"], &["option term", "renewal rent"]),
        rule(ClauseKind::SecurityDeposit, &["security deposit"], &["security deposit", "letter of credit"]),
        rule(ClauseKind::TenantImprovement, &["tenant improvement", "allowance"], &["tenant improvement allowance", "build-out"]),
        rule(ClauseKind::DeliveryCondition, &["delivery", "delivery condition"], &["deliver the premises", "substantial completion"]),
        rule(ClauseKind::OpeningCovenant, &["opening", "opening covenant"], &["open for business", "opening date"]),
        rule(ClauseKind::OperatingHours, &["hours of operation", "operating hours"], &["business hours", "remain open"]),
        rule(ClauseKind::Casualty, &["casualty", "damage"], &["fire or other casualty", "restore"]),
        rule(ClauseKind::Condemnation, &["condemnation", "eminent domain"], &["condemnation", "taking"]),
        rule(ClauseKind::SubordinationNondisturbance, &["subordination", "non-disturbance", "snda"], &["subordinate", "mortgagee", "non-disturbance"]),
        rule(ClauseKind::Estoppel, &["estoppel"], &["estoppel certificate"]),
        rule(ClauseKind::Holdover, &["holdover"], &["holdover", "month-to-month"]),
        rule(ClauseKind::Surrender, &["surrender"], &["surrender the premises", "remove"]),
        rule(ClauseKind::Guaranty, &["guaranty", "guarantee"], &["guarantor", "guarantees"]),
    ]
}

fn professional_services_rules() -> Vec<Rule> {
    vec![
        rule(ClauseKind::Scope, &["scope", "services"], &["services", "statement of work", "scope"]),
        rule(ClauseKind::Deliverables, &["deliverables"], &["deliverable", "work product"]),
        rule(ClauseKind::Milestones, &["milestones", "schedule"], &["milestone", "project plan"]),
        rule(ClauseKind::Acceptance, &["acceptance"], &["acceptance criteria", "deemed accepted", "reject"]),
        rule(ClauseKind::ChangeControl, &["change control", "change order"], &["change order", "scope change"]),
        rule(ClauseKind::Expenses, &["expenses"], &["reimbursable", "travel expenses"]),
        rule(ClauseKind::ServiceLevels, &["service levels", "sla"], &["service level", "uptime", "service credit"]),
        rule(ClauseKind::Staffing, &["personnel", "staffing"], &["key personnel", "replace personnel"]),
        rule(ClauseKind::Subcontracting, &["subcontracting", "subcontractors"], &["subcontractor", "delegate"]),
        rule(ClauseKind::WorkProduct, &["work product", "deliverable ownership"], &["work product", "work made for hire"]),
        rule(ClauseKind::BackgroundIp, &["background intellectual property", "pre-existing materials"], &["background ip", "pre-existing"]),
        rule(ClauseKind::OpenSource, &["open source"], &["open source", "copyleft"]),
        rule(ClauseKind::TransitionAssistance, &["transition assistance"], &["transition services", "knowledge transfer"]),
    ]
}

fn nda_rules() -> Vec<Rule> {
    vec![
        rule(ClauseKind::ConfidentialInformation, &["confidential information", "definition"], &["confidential information means", "designated confidential"]),
        rule(ClauseKind::ConfidentialityExclusions, &["exclusions", "exceptions"], &["publicly available", "already known", "independently developed"]),
        rule(ClauseKind::UseRestrictions, &["use", "non-use"], &["solely for", "not use", "purpose"]),
        rule(ClauseKind::PermittedRecipients, &["representatives", "permitted recipients"], &["employees", "advisers", "need to know"]),
        rule(ClauseKind::CompelledDisclosure, &["required disclosure", "compelled disclosure"], &["subpoena", "court order", "legally required"]),
        rule(ClauseKind::ReturnOrDestruction, &["return or destruction", "return of information"], &["return", "destroy", "certify destruction"]),
        rule(ClauseKind::Residuals, &["residuals"], &["unaided memory", "residual knowledge"]),
        rule(ClauseKind::NoLicense, &["no license"], &["no license", "no transfer of ownership"]),
        rule(ClauseKind::Standstill, &["standstill"], &["acquire securities", "proxy solicitation"]),
        rule(ClauseKind::NonSolicit, &["non-solicitation"], &["solicit customers", "solicit employees"]),
        rule(ClauseKind::NoHire, &["no-hire", "no hire"], &["hire any employee", "employ"]),
        rule(ClauseKind::NonCircumvention, &["non-circumvention"], &["circumvent", "business opportunity"]),
        rule(ClauseKind::InjunctiveRelief, &["injunctive relief", "equitable relief"], &["irreparable harm", "injunction", "specific performance"]),
    ]
}

fn extract_definitions(
    source: &str,
    clauses: &[ContractClause],
    maximum: usize,
) -> Vec<DefinedTerm> {
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for clause in clauses {
        for (sentence_start, sentence) in sentence_offsets(&clause.text) {
            let lower = sentence.to_ascii_lowercase();
            let marker = [" shall mean ", " means ", " has the meaning ", " is defined as "]
                .iter()
                .filter_map(|marker| lower.find(marker).map(|index| (index, *marker)))
                .min_by_key(|(index, _)| *index);
            let Some((index, marker)) = marker else {
                continue;
            };
            let left = sentence[..index].trim();
            let term = left
                .rsplit(['.', ';', ':', '\n'])
                .next()
                .unwrap_or(left)
                .trim_matches(|character: char| {
                    character.is_whitespace()
                        || matches!(character, '"' | '\'' | '(' | ')' | '[' | ']')
                });
            if term.is_empty() || term.chars().count() > 100 {
                continue;
            }
            let definition = sentence[index + marker.len()..].trim();
            if definition.len() < 3 || !seen.insert(term.to_ascii_lowercase()) {
                continue;
            }
            let absolute_start = clause.start_byte + sentence_start;
            output.push(DefinedTerm {
                term: term.to_owned(),
                definition: definition.to_owned(),
                clause_index: clause.index,
                start_byte: absolute_start,
                end_byte: absolute_start + sentence.len(),
            });
            if output.len() >= maximum {
                return output;
            }
        }
    }
    let _ = source;
    output
}

fn extract_obligations(
    _source: &str,
    clauses: &[ContractClause],
    maximum: usize,
) -> Vec<ContractObligation> {
    let mut output = Vec::new();
    for clause in clauses {
        for (sentence_start, sentence) in sentence_offsets(&clause.text) {
            let lower = sentence.to_ascii_lowercase();
            let Some((modal_start, modal_end, modality)) = find_modality(&lower) else {
                continue;
            };
            let subject_source = sentence[..modal_start].trim();
            let subject = subject_source
                .rsplit([',', ';', ':', '\n'])
                .next()
                .unwrap_or(subject_source)
                .trim()
                .chars()
                .rev()
                .take(160)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            let action = sentence[modal_end..].trim().chars().take(320).collect::<String>();
            if subject.is_empty() || action.is_empty() {
                continue;
            }
            let trigger = extract_trigger(sentence);
            let deadline = extract_deadline(sentence);
            let remedy = extract_remedy(sentence);
            let confidence = (0.58
                + if subject.split_whitespace().count() <= 12 { 0.10 } else { 0.0 }
                + if deadline.is_some() { 0.08 } else { 0.0 }
                + if trigger.is_some() { 0.06 } else { 0.0 }
                + if remedy.is_some() { 0.06 } else { 0.0 })
                .min(0.95);
            let absolute_start = clause.start_byte + sentence_start;
            output.push(ContractObligation {
                clause_index: clause.index,
                start_byte: absolute_start,
                end_byte: absolute_start + sentence.len(),
                subject,
                modality,
                action,
                trigger,
                deadline,
                remedy,
                confidence,
                sentence: sentence.trim().to_owned(),
            });
            if output.len() >= maximum {
                return output;
            }
        }
    }
    output
}

fn find_modality(lower: &str) -> Option<(usize, usize, ObligationModality)> {
    let patterns = [
        (" shall not ", ObligationModality::ShallNot),
        (" must not ", ObligationModality::MustNot),
        (" may not ", ObligationModality::MayNot),
        (" shall ", ObligationModality::Shall),
        (" must ", ObligationModality::Must),
        (" will ", ObligationModality::Will),
        (" should ", ObligationModality::Should),
        (" may ", ObligationModality::May),
    ];
    patterns
        .iter()
        .filter_map(|(pattern, modality)| {
            lower
                .find(pattern)
                .map(|index| (index, index + pattern.len(), *modality))
        })
        .min_by_key(|(index, _, _)| *index)
}

fn extract_trigger(sentence: &str) -> Option<String> {
    let lower = sentence.to_ascii_lowercase();
    for marker in ["in the event that", "provided that", "subject to", "upon ", "if ", "when "] {
        if let Some(index) = lower.find(marker) {
            return Some(
                sentence[index..]
                    .split([',', ';'])
                    .next()
                    .unwrap_or(&sentence[index..])
                    .trim()
                    .chars()
                    .take(200)
                    .collect(),
            );
        }
    }
    None
}

fn extract_deadline(sentence: &str) -> Option<String> {
    let lower = sentence.to_ascii_lowercase();
    for marker in ["within ", "no later than ", "at least ", "not less than ", "not more than "] {
        if let Some(index) = lower.find(marker) {
            let candidate = sentence[index..]
                .split([',', ';', '.'])
                .next()
                .unwrap_or(&sentence[index..])
                .trim();
            if candidate.to_ascii_lowercase().contains("day")
                || candidate.to_ascii_lowercase().contains("month")
                || candidate.to_ascii_lowercase().contains("year")
            {
                return Some(candidate.chars().take(160).collect());
            }
        }
    }
    None
}

fn extract_remedy(sentence: &str) -> Option<String> {
    let lower = sentence.to_ascii_lowercase();
    for marker in [
        "terminate", "indemnify", "damages", "service credit", "cure", "injunctive relief",
        "specific performance", "withhold payment",
    ] {
        if let Some(index) = lower.find(marker) {
            return Some(
                sentence[index..]
                    .split([',', ';', '.'])
                    .next()
                    .unwrap_or(&sentence[index..])
                    .trim()
                    .chars()
                    .take(180)
                    .collect(),
            );
        }
    }
    None
}

fn extract_economic_terms(
    source: &str,
    clauses: &[ContractClause],
    maximum: usize,
) -> Vec<EconomicTerm> {
    let mut output = Vec::new();
    for clause in clauses {
        scan_money_and_percentages(source, clause, &mut output, maximum);
        if output.len() >= maximum {
            return output;
        }
        scan_number_units(source, clause, &mut output, maximum);
        if output.len() >= maximum {
            return output;
        }
    }
    output.sort_unstable_by_key(|term| (term.start_byte, term.end_byte));
    output.dedup_by(|left, right| {
        left.start_byte == right.start_byte && left.end_byte == right.end_byte && left.kind == right.kind
    });
    output.truncate(maximum);
    output
}

fn scan_money_and_percentages(
    source: &str,
    clause: &ContractClause,
    output: &mut Vec<EconomicTerm>,
    maximum: usize,
) {
    let bytes = clause.text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() && output.len() < maximum {
        if bytes[index] == b'$' {
            let end = scan_numeric_end(bytes, index + 1);
            if end > index + 1 {
                let raw = &clause.text[index..end];
                let value = parse_number(&raw[1..]);
                push_term(source, clause, index, end, EconomicTermKind::Money, raw, value, Some("currency"), Some("USD"), output);
                index = end;
                continue;
            }
        }
        if bytes[index].is_ascii_digit() {
            let number_end = scan_numeric_end(bytes, index);
            if number_end < bytes.len() && bytes[number_end] == b'%' {
                let end = number_end + 1;
                let raw = &clause.text[index..end];
                let value = parse_number(&raw[..raw.len() - 1]);
                let kind = classify_percentage_context(&context(&clause.text, index, end));
                push_term(source, clause, index, end, kind, raw, value, Some("percent"), None, output);
                index = end;
                continue;
            }
        }
        index += 1;
    }
}

fn scan_number_units(
    source: &str,
    clause: &ContractClause,
    output: &mut Vec<EconomicTerm>,
    maximum: usize,
) {
    let tokens = word_tokens(&clause.text);
    for window in tokens.windows(2) {
        if output.len() >= maximum {
            return;
        }
        let number = window[0].text.trim_matches([',', '$']);
        let Ok(value) = number.parse::<f64>() else {
            continue;
        };
        let unit_lower = window[1].text.to_ascii_lowercase();
        if matches!(unit_lower.as_str(), "day" | "days" | "month" | "months" | "year" | "years") {
            let start = window[0].start;
            let end = window[1].end;
            let term_context = context(&clause.text, start, end);
            let kind = classify_duration_context(&term_context);
            let raw = &clause.text[start..end];
            push_term(
                source,
                clause,
                start,
                end,
                kind,
                raw,
                Some(value),
                Some(unit_lower.trim_end_matches('s')),
                None,
                output,
            );
        }
    }
    for window in tokens.windows(2) {
        if output.len() >= maximum {
            return;
        }
        if window[0].text.eq_ignore_ascii_case("net")
            && window[1].text.parse::<f64>().is_ok()
        {
            let start = window[0].start;
            let end = window[1].end;
            let raw = &clause.text[start..end];
            push_term(
                source,
                clause,
                start,
                end,
                EconomicTermKind::PaymentTerm,
                raw,
                window[1].text.parse::<f64>().ok(),
                Some("day"),
                None,
                output,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_term(
    source: &str,
    clause: &ContractClause,
    local_start: usize,
    local_end: usize,
    kind: EconomicTermKind,
    raw: &str,
    normalized_value: Option<f64>,
    unit: Option<&str>,
    currency: Option<&str>,
    output: &mut Vec<EconomicTerm>,
) {
    let start = clause.start_byte + local_start;
    let end = clause.start_byte + local_end;
    let surrounding = context(source, start, end);
    output.push(EconomicTerm {
        kind: refine_money_kind(kind, &surrounding),
        clause_index: clause.index,
        start_byte: start,
        end_byte: end,
        raw_value: raw.to_owned(),
        normalized_value,
        unit: unit.map(str::to_owned),
        currency: currency.map(str::to_owned),
        context: surrounding,
        confidence: 0.78,
    });
}

fn refine_money_kind(kind: EconomicTermKind, context: &str) -> EconomicTermKind {
    if kind != EconomicTermKind::Money {
        return kind;
    }
    let lower = context.to_ascii_lowercase();
    if lower.contains("base rent") || lower.contains("minimum rent") {
        EconomicTermKind::BaseRent
    } else if lower.contains("security deposit") || lower.contains("letter of credit") {
        EconomicTermKind::SecurityDeposit
    } else if lower.contains("tenant improvement") || lower.contains("allowance") {
        EconomicTermKind::TenantImprovementAllowance
    } else if lower.contains("liability") && (lower.contains("cap") || lower.contains("exceed")) {
        EconomicTermKind::LiabilityCap
    } else if lower.contains("insurance") || lower.contains("coverage") {
        EconomicTermKind::InsuranceLimit
    } else {
        EconomicTermKind::Money
    }
}

fn classify_percentage_context(context: &str) -> EconomicTermKind {
    let lower = context.to_ascii_lowercase();
    if lower.contains("percentage rent") || lower.contains("gross sales") {
        EconomicTermKind::PercentageRent
    } else if lower.contains("increase") || lower.contains("escalat") || lower.contains("annual rent") {
        EconomicTermKind::RentEscalation
    } else if lower.contains("interest") || lower.contains("late payment") {
        EconomicTermKind::InterestRate
    } else if lower.contains("service level") || lower.contains("uptime") {
        EconomicTermKind::ServiceLevel
    } else {
        EconomicTermKind::Percentage
    }
}

fn classify_duration_context(context: &str) -> EconomicTermKind {
    let lower = context.to_ascii_lowercase();
    if lower.contains("notice") || lower.contains("notify") {
        EconomicTermKind::NoticePeriod
    } else if lower.contains("renewal") || lower.contains("option term") || lower.contains("extend") {
        EconomicTermKind::RenewalTerm
    } else if lower.contains("invoice") || lower.contains("payment") || lower.contains("pay") {
        EconomicTermKind::PaymentTerm
    } else {
        EconomicTermKind::Duration
    }
}

fn scan_numeric_end(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len()
        && (bytes[index].is_ascii_digit() || matches!(bytes[index], b',' | b'.' | b'_'))
    {
        index += 1;
    }
    index
}

fn parse_number(value: &str) -> Option<f64> {
    value.replace([',', '_'], "").parse::<f64>().ok()
}

struct WordToken<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn word_tokens(text: &str) -> Vec<WordToken<'_>> {
    let mut output = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() || matches!(character, '$' | '%' | '.' | ',') {
            start.get_or_insert(index);
        } else if let Some(token_start) = start.take() {
            output.push(WordToken {
                text: &text[token_start..index],
                start: token_start,
                end: index,
            });
        }
    }
    if let Some(token_start) = start {
        output.push(WordToken {
            text: &text[token_start..],
            start: token_start,
            end: text.len(),
        });
    }
    output
}

fn sentence_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut output = Vec::new();
    let mut start = 0usize;
    for (index, character) in text.char_indices() {
        if matches!(character, '.' | ';' | '\n') {
            let end = index + character.len_utf8();
            if text[start..end].trim().len() >= 8 {
                let leading = text[start..end]
                    .find(|character: char| !character.is_whitespace())
                    .unwrap_or(0);
                let trailing = text[start..end].trim_end().len();
                output.push((start + leading, &text[start + leading..start + trailing]));
            }
            start = end;
        }
    }
    if start < text.len() && text[start..].trim().len() >= 8 {
        let leading = text[start..]
            .find(|character: char| !character.is_whitespace())
            .unwrap_or(0);
        output.push((start + leading, text[start + leading..].trim_end()));
    }
    output
}

fn context(text: &str, start: usize, end: usize) -> String {
    let mut left = start.saturating_sub(140);
    while left > 0 && !text.is_char_boundary(left) {
        left -= 1;
    }
    let mut right = end.saturating_add(180).min(text.len());
    while right < text.len() && !text.is_char_boundary(right) {
        right += 1;
    }
    text[left..right].split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build_warnings(
    profile: ContractProfile,
    clauses: &[ContractClause],
    definitions: &[DefinedTerm],
    economic_terms: &[EconomicTerm],
) -> Vec<ContractWarning> {
    let present = clauses
        .iter()
        .flat_map(|clause| clause.classifications.iter().map(|classification| classification.kind))
        .collect::<BTreeSet<_>>();
    let mut warnings = Vec::new();
    for kind in required_clause_kinds(profile) {
        if !present.contains(&kind) {
            warnings.push(ContractWarning {
                code: "missing_expected_clause".to_owned(),
                message: format!("expected {:?} clause was not identified", kind),
                clause_indexes: Vec::new(),
            });
        }
    }
    let mut definition_locations = BTreeMap::<String, Vec<usize>>::new();
    for definition in definitions {
        definition_locations
            .entry(definition.term.to_ascii_lowercase())
            .or_default()
            .push(definition.clause_index);
    }
    for (term, locations) in definition_locations {
        if locations.len() > 1 {
            warnings.push(ContractWarning {
                code: "duplicate_definition".to_owned(),
                message: format!("defined term {term:?} appears more than once"),
                clause_indexes: locations,
            });
        }
    }
    if profile == ContractProfile::RetailLease
        && !economic_terms.iter().any(|term| term.kind == EconomicTermKind::BaseRent)
    {
        warnings.push(ContractWarning {
            code: "base_rent_not_extracted".to_owned(),
            message: "no explicit base-rent amount was extracted".to_owned(),
            clause_indexes: Vec::new(),
        });
    }
    warnings
}

fn required_clause_kinds(profile: ContractProfile) -> &'static [ClauseKind] {
    match profile {
        ContractProfile::General => &[
            ClauseKind::Term,
            ClauseKind::Payment,
            ClauseKind::Termination,
            ClauseKind::GoverningLaw,
        ],
        ContractProfile::RetailLease => &[
            ClauseKind::Premises,
            ClauseKind::Term,
            ClauseKind::BaseRent,
            ClauseKind::PermittedUse,
            ClauseKind::MaintenanceAndRepair,
            ClauseKind::AssignmentAndSubletting,
            ClauseKind::Casualty,
            ClauseKind::Condemnation,
        ],
        ContractProfile::ProfessionalServices => &[
            ClauseKind::Scope,
            ClauseKind::Deliverables,
            ClauseKind::Fees,
            ClauseKind::Payment,
            ClauseKind::IntellectualProperty,
            ClauseKind::Confidentiality,
            ClauseKind::Termination,
        ],
        ContractProfile::Nda => &[
            ClauseKind::ConfidentialInformation,
            ClauseKind::ConfidentialityExclusions,
            ClauseKind::UseRestrictions,
            ClauseKind::CompelledDisclosure,
            ClauseKind::ReturnOrDestruction,
            ClauseKind::Term,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_contract, ClauseKind, ContractAnalysisOptions, ContractProfile, EconomicTermKind,
        ObligationModality,
    };

    #[test]
    fn analyzes_retail_lease_terms_and_obligations() {
        let text = r#"
ARTICLE 1. PREMISES
Landlord leases to Tenant approximately 4,000 square feet in the Shopping Center.

ARTICLE 2. BASE RENT
Tenant shall pay Base Rent of $120,000 per year, increasing by 3% each year.

ARTICLE 3. RENEWAL OPTION
Tenant may renew this Lease for 5 years by giving at least 180 days notice.

ARTICLE 4. ASSIGNMENT AND SUBLETTING
Tenant shall not assign this Lease without Landlord's prior written consent.
"#;
        let analysis = analyze_contract(
            text,
            ContractProfile::RetailLease,
            &ContractAnalysisOptions::default(),
        )
        .expect("analysis");
        assert!(analysis.clause_counts.contains_key(&ClauseKind::BaseRent));
        assert!(analysis
            .economic_terms
            .iter()
            .any(|term| term.kind == EconomicTermKind::BaseRent));
        assert!(analysis
            .economic_terms
            .iter()
            .any(|term| term.kind == EconomicTermKind::RentEscalation));
        assert!(analysis
            .obligations
            .iter()
            .any(|obligation| obligation.modality == ObligationModality::ShallNot));
    }

    #[test]
    fn analyzes_nda_definitions_and_restrictions() {
        let text = r#"
1. Confidential Information
"Confidential Information" means non-public business, technical, and financial information.

2. Use Restrictions
Recipient shall use Confidential Information solely to evaluate the Transaction.

3. Exclusions
Confidential Information does not include information that is publicly available or independently developed.

4. Required Disclosure
Recipient may disclose information if legally required by subpoena, provided that Recipient gives prompt notice.

5. Return or Destruction
Recipient shall destroy all copies within 10 days after request.
"#;
        let analysis = analyze_contract(
            text,
            ContractProfile::Nda,
            &ContractAnalysisOptions::default(),
        )
        .expect("analysis");
        assert!(!analysis.definitions.is_empty());
        assert!(analysis.clause_counts.contains_key(&ClauseKind::UseRestrictions));
        assert!(analysis
            .economic_terms
            .iter()
            .any(|term| term.kind == EconomicTermKind::NoticePeriod || term.kind == EconomicTermKind::Duration));
    }
}
