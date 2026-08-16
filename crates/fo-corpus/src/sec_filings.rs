use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    atomic_write, sha256_hex, unix_timestamp, CorpusDocument, CorpusError, CorpusFailure,
    CorpusManifest, CorpusProvider, DownloadClient, HttpOptions, Result, SEC_ARCHIVES_BASE,
    SEC_SUBMISSIONS_BASE, SEC_TICKERS_URL,
};

pub const INVESTOR_CORE_FORMS: &[&str] = &[
    "10-K", "10-K/A", "10-Q", "10-Q/A", "8-K", "8-K/A", "DEF 14A", "20-F",
    "20-F/A", "6-K", "40-F", "40-F/A",
];
pub const REGISTRATION_FORMS: &[&str] = &[
    "S-1", "S-1/A", "S-3", "S-3/A", "F-1", "F-1/A", "F-3", "F-3/A", "424B1",
    "424B2", "424B3", "424B4", "424B5", "424B7", "424B8",
];
pub const COMMENT_LETTER_FORMS: &[&str] = &["UPLOAD", "CORRESP"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecFilingCategory {
    AnnualReport,
    QuarterlyReport,
    CurrentReport,
    Proxy,
    Registration,
    Prospectus,
    ForeignReport,
    CommentLetter,
    Ownership,
    Other,
}

#[derive(Debug, Clone)]
pub struct SecFilingsOptions {
    pub output_dir: PathBuf,
    pub tickers: Vec<String>,
    pub ciks: Vec<u64>,
    pub sampled_companies: Option<usize>,
    pub seed: u64,
    pub forms: Vec<String>,
    pub filings_per_company: usize,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub include_historical_submission_files: bool,
    pub maximum_historical_files_per_company: usize,
    pub user_agent: String,
    pub requests_per_second: f64,
    pub maximum_attempts: usize,
    pub maximum_json_bytes: u64,
    pub maximum_document_bytes: u64,
    pub minimum_characters: usize,
    pub overwrite: bool,
}

impl Default for SecFilingsOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("corpora/sec-filings"),
            tickers: Vec::new(),
            ciks: Vec::new(),
            sampled_companies: None,
            seed: 0x73_65_63_2d_66_69_6c_65,
            forms: INVESTOR_CORE_FORMS.iter().map(|form| (*form).to_owned()).collect(),
            filings_per_company: 40,
            from_date: Some("2018-01-01".to_owned()),
            to_date: None,
            include_historical_submission_files: true,
            maximum_historical_files_per_company: 32,
            user_agent: String::new(),
            requests_per_second: 5.0,
            maximum_attempts: 5,
            maximum_json_bytes: 128 * 1024 * 1024,
            maximum_document_bytes: 128 * 1024 * 1024,
            minimum_characters: 500,
            overwrite: false,
        }
    }
}

impl SecFilingsOptions {
    pub fn validate(&self) -> Result<()> {
        if self.output_dir.as_os_str().is_empty()
            || self.forms.is_empty()
            || self.forms.len() > 256
            || self.filings_per_company == 0
            || self.filings_per_company > 100_000
            || self.maximum_historical_files_per_company > 1_000
            || self.user_agent.trim().is_empty()
            || !self.user_agent.contains('@')
            || !self.requests_per_second.is_finite()
            || self.requests_per_second <= 0.0
            || self.requests_per_second > 10.0
            || self.maximum_attempts == 0
            || self.maximum_json_bytes == 0
            || self.maximum_document_bytes == 0
            || self.minimum_characters == 0
        {
            return Err(CorpusError::Invalid(
                "SEC filing acquisition options are invalid or omit a contact-bearing user agent"
                    .to_owned(),
            ));
        }
        if self
            .sampled_companies
            .is_some_and(|count| count == 0 || count > 100_000)
        {
            return Err(CorpusError::Invalid(
                "SEC sampled company count must lie in 1..=100000".to_owned(),
            ));
        }
        if self.forms.iter().any(|form| normalize_form(form).is_empty()) {
            return Err(CorpusError::Invalid(
                "SEC form filters must not contain empty values".to_owned(),
            ));
        }
        for date in [self.from_date.as_deref(), self.to_date.as_deref()].into_iter().flatten() {
            if !valid_iso_date(date) {
                return Err(CorpusError::Invalid(format!(
                    "SEC date filter {date:?} must use YYYY-MM-DD"
                )));
            }
        }
        if let (Some(from), Some(to)) = (&self.from_date, &self.to_date)
            && from > to
        {
            return Err(CorpusError::Invalid(
                "SEC from_date must not follow to_date".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFilingsFetchReport {
    pub manifest: CorpusManifest,
    pub companies: usize,
    pub requested_forms: Vec<String>,
    pub candidate_filings: usize,
    pub downloaded: usize,
    pub reused: usize,
    pub rejected_too_short: usize,
    pub rejected_binary: usize,
    pub historical_submission_files_read: usize,
    pub failed: usize,
    pub counts_by_form: BTreeMap<String, usize>,
    pub counts_by_category: BTreeMap<String, usize>,
}

#[derive(Debug, Deserialize)]
struct CompanyTickerRow {
    cik_str: u64,
    ticker: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct SecSubmission {
    cik: u64,
    name: String,
    #[serde(default)]
    tickers: Vec<String>,
    filings: SubmissionFilings,
}

#[derive(Debug, Deserialize)]
struct SubmissionFilings {
    recent: FilingColumns,
    #[serde(default)]
    files: Vec<SubmissionHistoryFile>,
}

#[derive(Debug, Deserialize)]
struct SubmissionHistoryFile {
    name: String,
    #[serde(default)]
    filing_from: String,
    #[serde(default)]
    filing_to: String,
    #[serde(default)]
    filing_count: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FilingColumns {
    accession_number: Vec<String>,
    filing_date: Vec<String>,
    report_date: Vec<String>,
    acceptance_date_time: Vec<String>,
    form: Vec<String>,
    primary_document: Vec<String>,
    primary_doc_description: Vec<String>,
    items: Vec<String>,
    size: Vec<u64>,
    is_xbrl: Vec<u8>,
    is_inline_xbrl: Vec<u8>,
}

#[derive(Debug, Clone)]
struct SelectedCompany {
    cik: u64,
    issuer: String,
    tickers: Vec<String>,
}

#[derive(Debug, Clone)]
struct FilingCandidate {
    cik: u64,
    issuer: String,
    tickers: Vec<String>,
    accession_number: String,
    filing_date: String,
    report_date: String,
    acceptance_date_time: String,
    form: String,
    primary_document: String,
    description: String,
    items: String,
    declared_size: Option<u64>,
    is_xbrl: bool,
    is_inline_xbrl: bool,
}

pub fn fetch_sec_filings(options: SecFilingsOptions) -> Result<SecFilingsFetchReport> {
    options.validate()?;
    fs::create_dir_all(options.output_dir.join("documents"))
        .map_err(|error| CorpusError::io(options.output_dir.join("documents"), error))?;
    let forms = normalized_form_set(&options.forms);
    let minimum_interval = Duration::from_secs_f64(1.0 / options.requests_per_second);
    let mut client = DownloadClient::new(HttpOptions {
        user_agent: options.user_agent.clone(),
        minimum_interval,
        maximum_attempts: options.maximum_attempts,
        timeout: Duration::from_secs(120),
    })?;
    let ticker_response = client.get(SEC_TICKERS_URL, options.maximum_json_bytes)?;
    let ticker_rows = serde_json::from_slice::<BTreeMap<String, CompanyTickerRow>>(
        &ticker_response.bytes,
    )?;
    let companies = select_companies(&ticker_rows, &options)?;
    let corpus_id = format!(
        "sec-filings-{}-{}",
        companies.len(),
        short_form_fingerprint(&forms)
    );
    let mut manifest = CorpusManifest::load_or_new(
        &options.output_dir,
        corpus_id,
        CorpusProvider::SecEdgarFilings,
    )?;
    manifest.source_snapshot.insert(
        "company_tickers_url".to_owned(),
        SEC_TICKERS_URL.to_owned(),
    );
    manifest.source_snapshot.insert(
        "company_tickers_sha256".to_owned(),
        sha256_hex(&ticker_response.bytes),
    );
    manifest.source_snapshot.insert(
        "requested_forms".to_owned(),
        forms.iter().cloned().collect::<Vec<_>>().join(","),
    );
    manifest.source_snapshot.insert(
        "from_date".to_owned(),
        options.from_date.clone().unwrap_or_default(),
    );
    manifest.source_snapshot.insert(
        "to_date".to_owned(),
        options.to_date.clone().unwrap_or_default(),
    );
    manifest.source_snapshot.insert(
        "historical_submission_files".to_owned(),
        options.include_historical_submission_files.to_string(),
    );

    let mut candidate_filings = 0usize;
    let mut downloaded = 0usize;
    let mut reused = 0usize;
    let mut rejected_too_short = 0usize;
    let mut rejected_binary = 0usize;
    let mut historical_submission_files_read = 0usize;
    let mut failed = 0usize;
    let mut counts_by_form = BTreeMap::new();
    let mut counts_by_category = BTreeMap::new();

    for company in &companies {
        let submission_url = format!(
            "{SEC_SUBMISSIONS_BASE}/CIK{:010}.json",
            company.cik
        );
        let submission_response = match client.get(&submission_url, options.maximum_json_bytes) {
            Ok(response) => response,
            Err(error) => {
                failed += 1;
                manifest.record_failure(CorpusFailure {
                    id: format!("CIK{:010}", company.cik),
                    source_url: Some(submission_url),
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        let submission = match serde_json::from_slice::<SecSubmission>(&submission_response.bytes) {
            Ok(submission) => submission,
            Err(error) => {
                failed += 1;
                manifest.record_failure(CorpusFailure {
                    id: format!("CIK{:010}", company.cik),
                    source_url: Some(submission_url),
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        let issuer = if submission.name.trim().is_empty() {
            company.issuer.clone()
        } else {
            submission.name.clone()
        };
        let tickers = if submission.tickers.is_empty() {
            company.tickers.clone()
        } else {
            submission.tickers.clone()
        };
        let mut candidates = candidates_from_columns(
            submission.cik,
            &issuer,
            &tickers,
            &submission.filings.recent,
            &forms,
            &options,
        );
        if options.include_historical_submission_files {
            for history in submission
                .filings
                .files
                .iter()
                .take(options.maximum_historical_files_per_company)
            {
                if history.name.trim().is_empty() || !history.name.ends_with(".json") {
                    continue;
                }
                if !history_overlaps_date_range(history, &options) {
                    continue;
                }
                let url = format!("{SEC_SUBMISSIONS_BASE}/{}", history.name);
                match client.get(&url, options.maximum_json_bytes) {
                    Ok(response) => match serde_json::from_slice::<FilingColumns>(&response.bytes) {
                        Ok(columns) => {
                            historical_submission_files_read += 1;
                            candidates.extend(candidates_from_columns(
                                submission.cik,
                                &issuer,
                                &tickers,
                                &columns,
                                &forms,
                                &options,
                            ));
                        }
                        Err(error) => {
                            failed += 1;
                            manifest.record_failure(CorpusFailure {
                                id: history.name.clone(),
                                source_url: Some(url),
                                message: error.to_string(),
                                observed_at_unix: unix_timestamp(),
                            });
                        }
                    },
                    Err(error) => {
                        failed += 1;
                        manifest.record_failure(CorpusFailure {
                            id: history.name.clone(),
                            source_url: Some(url),
                            message: error.to_string(),
                            observed_at_unix: unix_timestamp(),
                        });
                    }
                }
            }
        }
        candidates.sort_unstable_by(|left, right| {
            right
                .filing_date
                .cmp(&left.filing_date)
                .then_with(|| right.acceptance_date_time.cmp(&left.acceptance_date_time))
                .then_with(|| left.form.cmp(&right.form))
                .then_with(|| left.accession_number.cmp(&right.accession_number))
        });
        candidates.dedup_by(|left, right| left.accession_number == right.accession_number);
        candidates.truncate(options.filings_per_company);
        candidate_filings += candidates.len();

        for candidate in candidates {
            let accession_compact = candidate.accession_number.replace('-', "");
            let primary_document = candidate.primary_document.trim();
            let id = format!("CIK{:010}-{}", candidate.cik, candidate.accession_number);
            if primary_document.is_empty()
                || primary_document.contains('/')
                || primary_document.contains("..")
            {
                failed += 1;
                manifest.record_failure(CorpusFailure {
                    id,
                    source_url: None,
                    message: "unsafe or empty SEC primary document name".to_owned(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
            let relative_path = format!(
                "documents/{:010}/{}-{}-{}.txt",
                candidate.cik,
                candidate.filing_date,
                sanitize_component(&candidate.form),
                accession_compact
            );
            let destination = options.output_dir.join(&relative_path);
            if destination.is_file() && !options.overwrite {
                if let Some(existing) = manifest.document(&id)
                    && existing.relative_path == relative_path
                    && fs::metadata(&destination)
                        .map(|metadata| metadata.len() == existing.bytes)
                        .unwrap_or(false)
                {
                    reused += 1;
                    *counts_by_form.entry(candidate.form.clone()).or_insert(0usize) += 1;
                    *counts_by_category
                        .entry(category_name(classify_form(&candidate.form)).to_owned())
                        .or_insert(0usize) += 1;
                    continue;
                }
            }
            let url = format!(
                "{SEC_ARCHIVES_BASE}/edgar/data/{}/{}/{}",
                candidate.cik, accession_compact, primary_document
            );
            match client.get(&url, options.maximum_document_bytes) {
                Ok(response) => {
                    let text = match filing_to_text(&response.bytes) {
                        Ok(Some(text)) => text,
                        Ok(None) => {
                            rejected_binary += 1;
                            manifest.record_failure(CorpusFailure {
                                id,
                                source_url: Some(response.final_url),
                                message: "SEC primary document is binary or unsupported".to_owned(),
                                observed_at_unix: unix_timestamp(),
                            });
                            continue;
                        }
                        Err(error) => {
                            failed += 1;
                            manifest.record_failure(CorpusFailure {
                                id,
                                source_url: Some(response.final_url),
                                message: error.to_string(),
                                observed_at_unix: unix_timestamp(),
                            });
                            continue;
                        }
                    };
                    if text.chars().count() < options.minimum_characters {
                        rejected_too_short += 1;
                        manifest.record_failure(CorpusFailure {
                            id,
                            source_url: Some(response.final_url),
                            message: format!(
                                "extracted SEC filing has fewer than {} characters",
                                options.minimum_characters
                            ),
                            observed_at_unix: unix_timestamp(),
                        });
                        continue;
                    }
                    let bytes = text.into_bytes();
                    atomic_write(&destination, &bytes)?;
                    let category = classify_form(&candidate.form);
                    let mut metadata = BTreeMap::new();
                    metadata.insert("cik".to_owned(), candidate.cik.to_string());
                    metadata.insert(
                        "accession_number".to_owned(),
                        candidate.accession_number.clone(),
                    );
                    metadata.insert("filing_date".to_owned(), candidate.filing_date.clone());
                    metadata.insert("report_date".to_owned(), candidate.report_date.clone());
                    metadata.insert(
                        "acceptance_date_time".to_owned(),
                        candidate.acceptance_date_time.clone(),
                    );
                    metadata.insert("form".to_owned(), candidate.form.clone());
                    metadata.insert("filing_category".to_owned(), category_name(category).to_owned());
                    metadata.insert("primary_document".to_owned(), primary_document.to_owned());
                    metadata.insert("description".to_owned(), candidate.description.clone());
                    metadata.insert("items".to_owned(), candidate.items.clone());
                    metadata.insert("tickers".to_owned(), candidate.tickers.join(","));
                    metadata.insert("is_xbrl".to_owned(), candidate.is_xbrl.to_string());
                    metadata.insert(
                        "is_inline_xbrl".to_owned(),
                        candidate.is_inline_xbrl.to_string(),
                    );
                    if let Some(size) = candidate.declared_size {
                        metadata.insert("declared_size".to_owned(), size.to_string());
                    }
                    if let Some(etag) = response.etag {
                        metadata.insert("etag".to_owned(), etag);
                    }
                    if let Some(last_modified) = response.last_modified {
                        metadata.insert("last_modified".to_owned(), last_modified);
                    }
                    manifest.upsert_document(CorpusDocument {
                        id: id.clone(),
                        relative_path,
                        source_url: response.final_url,
                        title: format!(
                            "{} {} filed {}",
                            candidate.issuer, candidate.form, candidate.filing_date
                        ),
                        author_or_issuer: candidate.issuer,
                        language: Some("en".to_owned()),
                        published_or_filed: Some(candidate.filing_date),
                        sha256: sha256_hex(&bytes),
                        bytes: bytes.len() as u64,
                        characters: String::from_utf8_lossy(&bytes).chars().count(),
                        downloaded_at_unix: unix_timestamp(),
                        metadata,
                    });
                    downloaded += 1;
                    *counts_by_form.entry(candidate.form).or_insert(0usize) += 1;
                    *counts_by_category
                        .entry(category_name(category).to_owned())
                        .or_insert(0usize) += 1;
                    if downloaded % 10 == 0 {
                        manifest.save(&options.output_dir)?;
                    }
                }
                Err(error) => {
                    failed += 1;
                    manifest.record_failure(CorpusFailure {
                        id,
                        source_url: Some(url),
                        message: error.to_string(),
                        observed_at_unix: unix_timestamp(),
                    });
                }
            }
        }
    }
    manifest.save(&options.output_dir)?;
    Ok(SecFilingsFetchReport {
        manifest,
        companies: companies.len(),
        requested_forms: forms.into_iter().collect(),
        candidate_filings,
        downloaded,
        reused,
        rejected_too_short,
        rejected_binary,
        historical_submission_files_read,
        failed,
        counts_by_form,
        counts_by_category,
    })
}

fn select_companies(
    rows: &BTreeMap<String, CompanyTickerRow>,
    options: &SecFilingsOptions,
) -> Result<Vec<SelectedCompany>> {
    let mut by_ticker = BTreeMap::<String, &CompanyTickerRow>::new();
    let mut by_cik = BTreeMap::<u64, &CompanyTickerRow>::new();
    for row in rows.values() {
        by_ticker.insert(row.ticker.to_ascii_uppercase(), row);
        by_cik.entry(row.cik_str).or_insert(row);
    }
    let mut selected = BTreeMap::<u64, SelectedCompany>::new();
    for ticker in &options.tickers {
        let normalized = ticker.trim().to_ascii_uppercase();
        let row = by_ticker
            .get(&normalized)
            .ok_or_else(|| CorpusError::Invalid(format!("SEC ticker {normalized} was not found")))?;
        selected.insert(
            row.cik_str,
            SelectedCompany {
                cik: row.cik_str,
                issuer: row.title.clone(),
                tickers: vec![row.ticker.clone()],
            },
        );
    }
    for &cik in &options.ciks {
        let row = by_cik.get(&cik);
        selected.insert(
            cik,
            SelectedCompany {
                cik,
                issuer: row.map_or_else(|| format!("CIK{cik:010}"), |row| row.title.clone()),
                tickers: row.map_or_else(Vec::new, |row| vec![row.ticker.clone()]),
            },
        );
    }
    if selected.is_empty() {
        let count = options.sampled_companies.unwrap_or(25);
        let mut candidates = by_cik.values().copied().collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|row| stable_mix(row.cik_str ^ options.seed));
        for row in candidates.into_iter().take(count) {
            selected.insert(
                row.cik_str,
                SelectedCompany {
                    cik: row.cik_str,
                    issuer: row.title.clone(),
                    tickers: vec![row.ticker.clone()],
                },
            );
        }
    }
    Ok(selected.into_values().collect())
}

fn candidates_from_columns(
    cik: u64,
    issuer: &str,
    tickers: &[String],
    columns: &FilingColumns,
    forms: &BTreeSet<String>,
    options: &SecFilingsOptions,
) -> Vec<FilingCandidate> {
    let count = [
        columns.accession_number.len(),
        columns.filing_date.len(),
        columns.form.len(),
        columns.primary_document.len(),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    let mut output = Vec::new();
    for index in 0..count {
        let form = normalize_form(&columns.form[index]);
        if !forms.contains(&form) {
            continue;
        }
        let filing_date = columns.filing_date[index].clone();
        if !valid_iso_date(&filing_date)
            || options
                .from_date
                .as_ref()
                .is_some_and(|from| filing_date < *from)
            || options
                .to_date
                .as_ref()
                .is_some_and(|to| filing_date > *to)
        {
            continue;
        }
        output.push(FilingCandidate {
            cik,
            issuer: issuer.to_owned(),
            tickers: tickers.to_vec(),
            accession_number: columns.accession_number[index].clone(),
            filing_date,
            report_date: columns.report_date.get(index).cloned().unwrap_or_default(),
            acceptance_date_time: columns
                .acceptance_date_time
                .get(index)
                .cloned()
                .unwrap_or_default(),
            form,
            primary_document: columns.primary_document[index].clone(),
            description: columns
                .primary_doc_description
                .get(index)
                .cloned()
                .unwrap_or_default(),
            items: columns.items.get(index).cloned().unwrap_or_default(),
            declared_size: columns.size.get(index).copied(),
            is_xbrl: columns.is_xbrl.get(index).copied().unwrap_or(0) != 0,
            is_inline_xbrl: columns
                .is_inline_xbrl
                .get(index)
                .copied()
                .unwrap_or(0)
                != 0,
        });
    }
    output
}

fn history_overlaps_date_range(
    history: &SubmissionHistoryFile,
    options: &SecFilingsOptions,
) -> bool {
    if history.filing_count == 0 && history.filing_from.is_empty() && history.filing_to.is_empty() {
        return true;
    }
    if options
        .from_date
        .as_ref()
        .is_some_and(|from| !history.filing_to.is_empty() && history.filing_to < *from)
    {
        return false;
    }
    if options
        .to_date
        .as_ref()
        .is_some_and(|to| !history.filing_from.is_empty() && history.filing_from > *to)
    {
        return false;
    }
    true
}

fn normalized_form_set(forms: &[String]) -> BTreeSet<String> {
    forms
        .iter()
        .map(|form| normalize_form(form))
        .filter(|form| !form.is_empty())
        .collect()
}

fn normalize_form(form: &str) -> String {
    form.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

#[must_use]
pub fn classify_form(form: &str) -> SecFilingCategory {
    let form = normalize_form(form);
    match form.as_str() {
        "10-K" | "10-K/A" | "20-F" | "20-F/A" | "40-F" | "40-F/A" => {
            SecFilingCategory::AnnualReport
        }
        "10-Q" | "10-Q/A" => SecFilingCategory::QuarterlyReport,
        "8-K" | "8-K/A" => SecFilingCategory::CurrentReport,
        "DEF 14A" | "DEFA14A" | "PRE 14A" => SecFilingCategory::Proxy,
        "S-1" | "S-1/A" | "S-3" | "S-3/A" | "F-1" | "F-1/A" | "F-3"
        | "F-3/A" => SecFilingCategory::Registration,
        value if value.starts_with("424B") => SecFilingCategory::Prospectus,
        "6-K" => SecFilingCategory::ForeignReport,
        "UPLOAD" | "CORRESP" => SecFilingCategory::CommentLetter,
        "3" | "3/A" | "4" | "4/A" | "5" | "5/A" | "SC 13D" | "SC 13D/A"
        | "SC 13G" | "SC 13G/A" => SecFilingCategory::Ownership,
        _ => SecFilingCategory::Other,
    }
}

const fn category_name(category: SecFilingCategory) -> &'static str {
    match category {
        SecFilingCategory::AnnualReport => "annual_report",
        SecFilingCategory::QuarterlyReport => "quarterly_report",
        SecFilingCategory::CurrentReport => "current_report",
        SecFilingCategory::Proxy => "proxy",
        SecFilingCategory::Registration => "registration",
        SecFilingCategory::Prospectus => "prospectus",
        SecFilingCategory::ForeignReport => "foreign_report",
        SecFilingCategory::CommentLetter => "comment_letter",
        SecFilingCategory::Ownership => "ownership",
        SecFilingCategory::Other => "other",
    }
}

fn filing_to_text(bytes: &[u8]) -> Result<Option<String>> {
    if bytes.starts_with(b"%PDF-")
        || bytes.iter().take(4_096).any(|byte| *byte == 0)
        || std::str::from_utf8(bytes).is_err()
    {
        return Ok(None);
    }
    let source = String::from_utf8_lossy(bytes);
    let prefix = source
        .get(..source.len().min(8_192))
        .unwrap_or(source.as_ref())
        .to_ascii_lowercase();
    let extracted = if prefix.contains("<html")
        || prefix.contains("<body")
        || prefix.contains("<table")
        || prefix.contains("<div")
        || prefix.contains("<ix:")
    {
        html2text::from_read(bytes, 180).map_err(|error| CorpusError::Html(error.to_string()))?
    } else {
        source.into_owned()
    };
    Ok(Some(clean_filing_text(&extracted)))
}

fn clean_filing_text(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::with_capacity(normalized.len());
    let mut blank_lines = 0usize;
    for line in normalized.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !output.is_empty() {
                output.push('\n');
            }
            continue;
        }
        blank_lines = 0;
        output.push_str(trimmed);
        output.push('\n');
    }
    output.trim().to_owned()
}

fn short_form_fingerprint(forms: &BTreeSet<String>) -> String {
    let digest = sha256_hex(forms.iter().cloned().collect::<Vec<_>>().join("\n").as_bytes());
    digest[..12].to_owned()
}

fn sanitize_component(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !output.is_empty() {
            output.push('_');
            separator = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "filing".to_owned()
    } else {
        output
    }
}

fn valid_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn stable_mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        candidates_from_columns, classify_form, normalized_form_set, FilingColumns,
        SecFilingCategory, SecFilingsOptions,
    };

    #[test]
    fn classifies_investor_forms() {
        assert_eq!(classify_form("10-K"), SecFilingCategory::AnnualReport);
        assert_eq!(classify_form("8-k/a"), SecFilingCategory::CurrentReport);
        assert_eq!(classify_form("DEF 14A"), SecFilingCategory::Proxy);
        assert_eq!(classify_form("UPLOAD"), SecFilingCategory::CommentLetter);
    }

    #[test]
    fn filters_columns_by_form_and_date() {
        let columns = FilingColumns {
            accession_number: vec!["0001-24-000001".to_owned(), "0001-24-000002".to_owned()],
            filing_date: vec!["2024-02-01".to_owned(), "2024-03-01".to_owned()],
            form: vec!["10-K".to_owned(), "S-1".to_owned()],
            primary_document: vec!["annual.htm".to_owned(), "registration.htm".to_owned()],
            ..FilingColumns::default()
        };
        let forms = normalized_form_set(&["10-K".to_owned()]);
        let candidates = candidates_from_columns(
            1,
            "Issuer",
            &[],
            &columns,
            &forms,
            &SecFilingsOptions::default(),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].form, "10-K");
        assert_eq!(forms, BTreeSet::from(["10-K".to_owned()]));
    }
}
