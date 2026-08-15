use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    atomic_write, sha256_hex, unix_timestamp, CorpusDocument, CorpusError, CorpusFailure,
    CorpusManifest, CorpusProvider, DownloadClient, HttpOptions, Result,
};

pub const SEC_TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
pub const SEC_SUBMISSIONS_BASE: &str = "https://data.sec.gov/submissions";
pub const SEC_ARCHIVES_BASE: &str = "https://www.sec.gov/Archives/edgar/data";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecPreset {
    Smoke,
    Standard,
    Large,
}

impl SecPreset {
    pub const fn company_count(self) -> usize {
        match self {
            Self::Smoke => 3,
            Self::Standard => 25,
            Self::Large => 100,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sec10KOptions {
    pub output_dir: PathBuf,
    pub preset: SecPreset,
    pub tickers: Vec<String>,
    pub ciks: Vec<u64>,
    pub sampled_companies: Option<usize>,
    pub filings_per_company: usize,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub include_amendments: bool,
    pub minimum_characters: usize,
    pub maximum_document_bytes: u64,
    pub user_agent: String,
    pub requests_per_second: f64,
    pub maximum_attempts: usize,
    pub overwrite: bool,
    pub seed: u64,
}

impl Default for Sec10KOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("corpora/sec-10k"),
            preset: SecPreset::Smoke,
            tickers: Vec::new(),
            ciks: Vec::new(),
            sampled_companies: None,
            filings_per_company: 3,
            from_date: None,
            to_date: None,
            include_amendments: false,
            minimum_characters: 25_000,
            maximum_document_bytes: 96 * 1024 * 1024,
            user_agent: std::env::var("SEC_USER_AGENT").unwrap_or_default(),
            requests_per_second: 5.0,
            maximum_attempts: 5,
            overwrite: false,
            seed: 0x53_45_43_2d_31_30_4b,
        }
    }
}

impl Sec10KOptions {
    pub fn validate(&self) -> Result<()> {
        if self.user_agent.trim().is_empty() || !self.user_agent.contains('@') {
            return Err(CorpusError::Invalid(
                "SEC downloads require a declared user agent containing an organization and \
                 contact email, supplied by --user-agent or SEC_USER_AGENT"
                    .to_owned(),
            ));
        }
        if self.filings_per_company == 0
            || self.filings_per_company > 100
            || self.minimum_characters == 0
            || self.maximum_document_bytes == 0
        {
            return Err(CorpusError::Invalid(
                "SEC filing and size limits are outside safe bounds".to_owned(),
            ));
        }
        if !self.requests_per_second.is_finite()
            || self.requests_per_second <= 0.0
            || self.requests_per_second > 10.0
        {
            return Err(CorpusError::Invalid(
                "SEC requests_per_second must lie in (0, 10]".to_owned(),
            ));
        }
        if self.maximum_attempts == 0 || self.maximum_attempts > 32 {
            return Err(CorpusError::Invalid(
                "SEC maximum attempts must be between 1 and 32".to_owned(),
            ));
        }
        for (name, date) in [
            ("from_date", self.from_date.as_deref()),
            ("to_date", self.to_date.as_deref()),
        ] {
            if let Some(date) = date
                && !valid_iso_date(date)
            {
                return Err(CorpusError::Invalid(format!(
                    "{name} must use YYYY-MM-DD"
                )));
            }
        }
        if self
            .from_date
            .as_ref()
            .zip(self.to_date.as_ref())
            .is_some_and(|(from, to)| from > to)
        {
            return Err(CorpusError::Invalid(
                "from_date must not be later than to_date".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sec10KFetchReport {
    pub manifest: CorpusManifest,
    pub companies: usize,
    pub candidate_filings: usize,
    pub downloaded: usize,
    pub reused: usize,
    pub rejected_too_short: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct CompanyTickerRow {
    cik_str: u64,
    ticker: String,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SecSubmission {
    #[serde(deserialize_with = "deserialize_cik")]
    cik: u64,
    name: String,
    #[serde(default)]
    tickers: Vec<String>,
    filings: SecFilings,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CikRepresentation {
    Number(u64),
    String(String),
}

fn deserialize_cik<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;

    match CikRepresentation::deserialize(deserializer)? {
        CikRepresentation::Number(cik) => Ok(cik),
        CikRepresentation::String(cik) => cik
            .trim()
            .parse::<u64>()
            .map_err(|error| D::Error::custom(format!("invalid SEC CIK {cik:?}: {error}"))),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SecFilings {
    recent: SecRecentFilings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecRecentFilings {
    accession_number: Vec<String>,
    filing_date: Vec<String>,
    report_date: Vec<String>,
    form: Vec<String>,
    primary_document: Vec<String>,
    #[serde(default)]
    primary_doc_description: Vec<String>,
}

#[derive(Debug, Clone)]
struct SecFilingCandidate {
    cik: u64,
    issuer: String,
    accession_number: String,
    filing_date: String,
    report_date: String,
    form: String,
    primary_document: String,
    description: String,
}

pub fn fetch_sec_10k(options: Sec10KOptions) -> Result<Sec10KFetchReport> {
    options.validate()?;
    fs::create_dir_all(options.output_dir.join("documents"))
        .map_err(|error| CorpusError::io(options.output_dir.join("documents"), error))?;
    fs::create_dir_all(options.output_dir.join("metadata"))
        .map_err(|error| CorpusError::io(options.output_dir.join("metadata"), error))?;

    let interval = Duration::from_secs_f64(1.0 / options.requests_per_second);
    let mut client = DownloadClient::new(HttpOptions {
        user_agent: options.user_agent.clone(),
        minimum_interval: interval,
        maximum_attempts: options.maximum_attempts,
        ..HttpOptions::default()
    })?;
    let ticker_response = client.get(SEC_TICKERS_URL, 64 * 1024 * 1024)?;
    let ticker_path = options.output_dir.join("metadata/company_tickers.json");
    atomic_write(&ticker_path, &ticker_response.bytes)?;
    let ticker_rows =
        serde_json::from_slice::<BTreeMap<String, CompanyTickerRow>>(&ticker_response.bytes)?;
    let selected_companies = select_companies(&ticker_rows, &options)?;

    let mut manifest = CorpusManifest::load_or_new(
        &options.output_dir,
        format!(
            "sec-10k-{}-{}",
            selected_companies.len(),
            options.filings_per_company
        ),
        CorpusProvider::SecEdgar10K,
    )?;
    manifest
        .source_snapshot
        .insert("company_tickers_url".to_owned(), ticker_response.final_url);
    manifest.source_snapshot.insert(
        "company_tickers_sha256".to_owned(),
        sha256_hex(&ticker_response.bytes),
    );
    manifest.source_snapshot.insert(
        "requests_per_second".to_owned(),
        options.requests_per_second.to_string(),
    );
    manifest
        .source_snapshot
        .insert("user_agent".to_owned(), options.user_agent.clone());

    let mut candidate_filings = 0usize;
    let mut downloaded = 0usize;
    let mut reused = 0usize;
    let mut rejected_too_short = 0usize;
    let mut failed = 0usize;

    for (requested_cik, fallback_name, fallback_ticker) in &selected_companies {
        let submissions_url = format!("{SEC_SUBMISSIONS_BASE}/CIK{requested_cik:010}.json");
        let submission = match client.get(&submissions_url, 64 * 1024 * 1024) {
            Ok(response) => match serde_json::from_slice::<SecSubmission>(&response.bytes) {
                Ok(submission) => submission,
                Err(error) => {
                    failed += 1;
                    manifest.record_failure(CorpusFailure {
                        id: format!("CIK{requested_cik:010}"),
                        source_url: Some(response.final_url),
                        message: error.to_string(),
                        observed_at_unix: unix_timestamp(),
                    });
                    continue;
                }
            },
            Err(error) => {
                failed += 1;
                manifest.record_failure(CorpusFailure {
                    id: format!("CIK{requested_cik:010}"),
                    source_url: Some(submissions_url),
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        if submission.cik != *requested_cik {
            failed += 1;
            manifest.record_failure(CorpusFailure {
                id: format!("CIK{requested_cik:010}"),
                source_url: Some(format!(
                    "{SEC_SUBMISSIONS_BASE}/CIK{requested_cik:010}.json"
                )),
                message: format!(
                    "submission CIK {} did not match requested CIK {}",
                    submission.cik, requested_cik
                ),
                observed_at_unix: unix_timestamp(),
            });
            continue;
        }
        let issuer = if submission.name.trim().is_empty() {
            fallback_name.clone()
        } else {
            submission.name.clone()
        };
        let tickers = if submission.tickers.is_empty() {
            fallback_ticker.iter().cloned().collect()
        } else {
            submission.tickers.clone()
        };
        let filings = select_filings(&submission, &issuer, &options);
        candidate_filings = candidate_filings.saturating_add(filings.len());

        for filing in filings {
            let accession_compact = filing.accession_number.replace('-', "");
            let cik_directory = filing.cik.to_string();
            let url = format!(
                "{SEC_ARCHIVES_BASE}/{cik_directory}/{accession_compact}/{}",
                filing.primary_document
            );
            let id = format!("CIK{:010}-{}", filing.cik, filing.accession_number);
            let relative_path = format!(
                "documents/CIK{:010}/{}_{}.txt",
                filing.cik,
                filing.filing_date,
                sanitize_component(&filing.accession_number)
            );
            let destination = options.output_dir.join(&relative_path);
            if !options.overwrite {
                if let Some(existing) = manifest.document(&id)
                    && destination.is_file()
                {
                    let bytes = fs::read(&destination)
                        .map_err(|error| CorpusError::io(&destination, error))?;
                    if sha256_hex(&bytes) == existing.sha256 {
                        reused += 1;
                        continue;
                    }
                }
            }

            match client.get(&url, options.maximum_document_bytes) {
                Ok(response) => {
                    let text = filing_to_text(&response.bytes)?;
                    if text.chars().count() < options.minimum_characters {
                        rejected_too_short += 1;
                        manifest.record_failure(CorpusFailure {
                            id,
                            source_url: Some(response.final_url),
                            message: format!(
                                "filing contained fewer than {} characters after HTML extraction",
                                options.minimum_characters
                            ),
                            observed_at_unix: unix_timestamp(),
                        });
                        continue;
                    }
                    let bytes = text.into_bytes();
                    atomic_write(&destination, &bytes)?;
                    let mut metadata = BTreeMap::new();
                    metadata.insert("cik".to_owned(), filing.cik.to_string());
                    metadata.insert("accession_number".to_owned(), filing.accession_number);
                    metadata.insert("report_date".to_owned(), filing.report_date);
                    metadata.insert("form".to_owned(), filing.form);
                    metadata.insert("primary_document".to_owned(), filing.primary_document);
                    metadata.insert("description".to_owned(), filing.description);
                    metadata.insert("tickers".to_owned(), tickers.join(","));
                    let form = metadata["form"].clone();
                    manifest.upsert_document(CorpusDocument {
                        id,
                        relative_path,
                        source_url: response.final_url,
                        title: format!("{} {} filed {}", filing.issuer, form, filing.filing_date),
                        author_or_issuer: filing.issuer,
                        language: Some("en".to_owned()),
                        published_or_filed: Some(filing.filing_date),
                        sha256: sha256_hex(&bytes),
                        bytes: bytes.len() as u64,
                        characters: String::from_utf8_lossy(&bytes).chars().count(),
                        downloaded_at_unix: unix_timestamp(),
                        metadata,
                    });
                    downloaded += 1;
                    if downloaded % 5 == 0 {
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
    Ok(Sec10KFetchReport {
        manifest,
        companies: selected_companies.len(),
        candidate_filings,
        downloaded,
        reused,
        rejected_too_short,
        failed,
    })
}

fn select_companies(
    rows: &BTreeMap<String, CompanyTickerRow>,
    options: &Sec10KOptions,
) -> Result<Vec<(u64, String, Option<String>)>> {
    let mut by_ticker = BTreeMap::<String, &CompanyTickerRow>::new();
    let mut by_cik = BTreeMap::<u64, &CompanyTickerRow>::new();
    for row in rows.values() {
        by_ticker.insert(row.ticker.to_ascii_uppercase(), row);
        by_cik.entry(row.cik_str).or_insert(row);
    }

    let mut selected = BTreeMap::<u64, (String, Option<String>)>::new();
    for ticker in &options.tickers {
        let normalized = ticker.trim().to_ascii_uppercase();
        let row = by_ticker
            .get(&normalized)
            .ok_or_else(|| CorpusError::Invalid(format!("SEC ticker {normalized} was not found")))?;
        selected.insert(row.cik_str, (row.title.clone(), Some(row.ticker.clone())));
    }
    for &cik in &options.ciks {
        let row = by_cik.get(&cik);
        selected.insert(
            cik,
            (
                row.map_or_else(|| format!("CIK{cik:010}"), |row| row.title.clone()),
                row.map(|row| row.ticker.clone()),
            ),
        );
    }

    if selected.is_empty() {
        let count = options
            .sampled_companies
            .unwrap_or_else(|| options.preset.company_count());
        if count == 0 {
            return Err(CorpusError::Invalid(
                "SEC sampled company count must be positive".to_owned(),
            ));
        }
        let mut candidates = by_cik.values().copied().collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|row| stable_mix(row.cik_str ^ options.seed));
        for row in candidates.into_iter().take(count) {
            selected.insert(row.cik_str, (row.title.clone(), Some(row.ticker.clone())));
        }
    }
    Ok(selected
        .into_iter()
        .map(|(cik, (name, ticker))| (cik, name, ticker))
        .collect())
}

fn select_filings(
    submission: &SecSubmission,
    issuer: &str,
    options: &Sec10KOptions,
) -> Vec<SecFilingCandidate> {
    let recent = &submission.filings.recent;
    let count = [
        recent.accession_number.len(),
        recent.filing_date.len(),
        recent.report_date.len(),
        recent.form.len(),
        recent.primary_document.len(),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    let mut candidates = Vec::new();
    for index in 0..count {
        let form = &recent.form[index];
        let accepted_form = form == "10-K" || (options.include_amendments && form == "10-K/A");
        if !accepted_form {
            continue;
        }
        let filing_date = &recent.filing_date[index];
        if options
            .from_date
            .as_ref()
            .is_some_and(|from| filing_date < from)
            || options
                .to_date
                .as_ref()
                .is_some_and(|to| filing_date > to)
        {
            continue;
        }
        let primary_document = recent.primary_document[index].trim();
        if primary_document.is_empty() || primary_document.contains('/') {
            continue;
        }
        candidates.push(SecFilingCandidate {
            cik: submission.cik,
            issuer: issuer.to_owned(),
            accession_number: recent.accession_number[index].clone(),
            filing_date: filing_date.clone(),
            report_date: recent.report_date[index].clone(),
            form: form.clone(),
            primary_document: primary_document.to_owned(),
            description: recent
                .primary_doc_description
                .get(index)
                .cloned()
                .unwrap_or_default(),
        });
    }
    candidates.sort_unstable_by(|left, right| {
        right
            .filing_date
            .cmp(&left.filing_date)
            .then_with(|| right.accession_number.cmp(&left.accession_number))
    });
    candidates.truncate(options.filings_per_company);
    candidates
}

fn filing_to_text(bytes: &[u8]) -> Result<String> {
    let source = String::from_utf8_lossy(bytes);
    let looks_like_html = source
        .get(..source.len().min(4_096))
        .is_some_and(|prefix| {
            let lowercase = prefix.to_ascii_lowercase();
            lowercase.contains("<html")
                || lowercase.contains("<body")
                || lowercase.contains("<table")
                || lowercase.contains("<div")
        });
    let extracted = if looks_like_html {
        html2text::from_read(bytes, 160).map_err(|error| CorpusError::Html(error.to_string()))?
    } else {
        source.into_owned()
    };
    Ok(clean_filing_text(&extracted))
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

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
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
    use serde::Deserialize;

    use super::{clean_filing_text, valid_iso_date, SecSubmission};

    #[test]
    fn validates_iso_dates() {
        assert!(valid_iso_date("2025-12-31"));
        assert!(!valid_iso_date("2025-1-1"));
        assert!(!valid_iso_date("2025/12/31"));
    }

    #[test]
    fn collapses_excess_blank_lines() {
        assert_eq!(clean_filing_text("a\n\n\n\nb\n"), "a\n\nb");
    }

    #[test]
    fn accepts_string_cik_in_submission_json() {
        let submission = serde_json::from_str::<SecSubmission>(
            r#"{
                "cik":"0000320193",
                "name":"Apple Inc.",
                "tickers":["AAPL"],
                "filings":{"recent":{
                    "accessionNumber":[],
                    "filingDate":[],
                    "reportDate":[],
                    "form":[],
                    "primaryDocument":[],
                    "primaryDocDescription":[]
                }}
            }"#,
        )
        .expect("submission");
        assert_eq!(submission.cik, 320_193);
    }
}
