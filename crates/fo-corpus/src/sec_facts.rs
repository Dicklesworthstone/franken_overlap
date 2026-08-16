use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    atomic_write, sha256_hex, unix_timestamp, CorpusError, DownloadClient, HttpOptions, Result,
    SEC_TICKERS_URL,
};

pub const SEC_COMPANYFACTS_BASE: &str = "https://data.sec.gov/api/xbrl/companyfacts";
pub const SEC_FACTS_MANIFEST_FILENAME: &str = "sec-facts-manifest.json";
pub const SEC_FACTS_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const SEC_NORMALIZED_FACTS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct SecFactsFetchOptions {
    pub output_dir: PathBuf,
    pub tickers: Vec<String>,
    pub ciks: Vec<u64>,
    pub sampled_companies: Option<usize>,
    pub seed: u64,
    pub user_agent: String,
    pub requests_per_second: f64,
    pub maximum_attempts: usize,
    pub maximum_ticker_json_bytes: u64,
    pub maximum_companyfacts_bytes: u64,
    pub overwrite: bool,
}

impl Default for SecFactsFetchOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("corpora/sec-companyfacts"),
            tickers: Vec::new(),
            ciks: Vec::new(),
            sampled_companies: None,
            seed: 0x78_62_72_6c_2d_66_61_63,
            user_agent: String::new(),
            requests_per_second: 5.0,
            maximum_attempts: 5,
            maximum_ticker_json_bytes: 64 * 1024 * 1024,
            maximum_companyfacts_bytes: 512 * 1024 * 1024,
            overwrite: false,
        }
    }
}

impl SecFactsFetchOptions {
    pub fn validate(&self) -> Result<()> {
        if self.output_dir.as_os_str().is_empty()
            || self.user_agent.trim().is_empty()
            || !self.user_agent.contains('@')
            || !self.requests_per_second.is_finite()
            || self.requests_per_second <= 0.0
            || self.requests_per_second > 10.0
            || self.maximum_attempts == 0
            || self.maximum_attempts > 32
            || self.maximum_ticker_json_bytes == 0
            || self.maximum_companyfacts_bytes == 0
        {
            return Err(CorpusError::Invalid(
                "SEC facts options are invalid or omit a contact-bearing user agent".to_owned(),
            ));
        }
        if self
            .sampled_companies
            .is_some_and(|count| count == 0 || count > 100_000)
        {
            return Err(CorpusError::Invalid(
                "sampled company count must lie in 1..=100000".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactsCompanyRecord {
    pub cik: u64,
    pub entity_name: String,
    pub tickers: Vec<String>,
    pub raw_path: String,
    pub raw_sha256: String,
    pub raw_bytes: u64,
    pub normalized_path: String,
    pub normalized_sha256: String,
    pub normalized_bytes: u64,
    pub observations: usize,
    pub taxonomies: usize,
    pub concepts: usize,
    pub fetched_at_unix: u64,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl SecFactsCompanyRecord {
    fn validate(&self) -> Result<()> {
        if self.cik == 0
            || self.entity_name.trim().is_empty()
            || self.raw_sha256.len() != 64
            || self.normalized_sha256.len() != 64
            || self.raw_bytes == 0
            || self.normalized_bytes == 0
        {
            return Err(CorpusError::Invalid(format!(
                "SEC facts company record CIK{:010} has invalid identity or receipts",
                self.cik
            )));
        }
        validate_relative_path(&self.raw_path)?;
        validate_relative_path(&self.normalized_path)?;
        if self.tickers.iter().any(|ticker| ticker.trim().is_empty())
            || self.metadata.keys().any(|key| key.trim().is_empty())
        {
            return Err(CorpusError::Invalid(format!(
                "SEC facts company record CIK{:010} contains empty metadata",
                self.cik
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactsFailure {
    pub cik: u64,
    pub source_url: String,
    pub message: String,
    pub observed_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactsManifest {
    pub schema_version: u32,
    pub corpus_id: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub company_tickers_url: String,
    pub company_tickers_sha256: String,
    pub companies: Vec<SecFactsCompanyRecord>,
    pub failures: Vec<SecFactsFailure>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl SecFactsManifest {
    #[must_use]
    pub fn new(corpus_id: impl Into<String>) -> Self {
        let now = unix_timestamp();
        Self {
            schema_version: SEC_FACTS_MANIFEST_SCHEMA_VERSION,
            corpus_id: corpus_id.into(),
            created_at_unix: now,
            updated_at_unix: now,
            company_tickers_url: SEC_TICKERS_URL.to_owned(),
            company_tickers_sha256: String::new(),
            companies: Vec::new(),
            failures: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SEC_FACTS_MANIFEST_SCHEMA_VERSION
            || self.corpus_id.trim().is_empty()
            || self.company_tickers_url.trim().is_empty()
            || self.company_tickers_sha256.len() != 64
        {
            return Err(CorpusError::Invalid(
                "SEC facts manifest has an invalid schema, identity, or ticker snapshot"
                    .to_owned(),
            ));
        }
        let mut ciks = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for company in &self.companies {
            company.validate()?;
            if !ciks.insert(company.cik)
                || !paths.insert(company.raw_path.as_str())
                || !paths.insert(company.normalized_path.as_str())
            {
                return Err(CorpusError::Invalid(format!(
                    "SEC facts manifest contains duplicate CIK or path for CIK{:010}",
                    company.cik
                )));
            }
        }
        Ok(())
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let path = root.as_ref().join(SEC_FACTS_MANIFEST_FILENAME);
        let manifest = serde_json::from_slice::<Self>(
            &fs::read(&path).map_err(|error| CorpusError::io(&path, error))?,
        )?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        atomic_write(
            &root.as_ref().join(SEC_FACTS_MANIFEST_FILENAME),
            &serde_json::to_vec_pretty(self)?,
        )
    }

    pub fn upsert_company(&mut self, company: SecFactsCompanyRecord) {
        if let Some(existing) = self
            .companies
            .iter_mut()
            .find(|existing| existing.cik == company.cik)
        {
            *existing = company;
        } else {
            self.companies.push(company);
        }
        self.companies.sort_unstable_by_key(|company| company.cik);
        self.updated_at_unix = unix_timestamp();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactObservation {
    pub id: String,
    pub taxonomy: String,
    pub concept: String,
    pub label: String,
    pub description: String,
    pub unit: String,
    pub value: Value,
    pub start: Option<String>,
    pub end: Option<String>,
    pub accession_number: String,
    pub fiscal_year: Option<i64>,
    pub fiscal_period: Option<String>,
    pub form: String,
    pub filed: String,
    pub frame: Option<String>,
}

impl SecFactObservation {
    fn validate(&self) -> Result<()> {
        if self.id.len() != 64
            || self.taxonomy.trim().is_empty()
            || self.concept.trim().is_empty()
            || self.unit.trim().is_empty()
            || self.accession_number.trim().is_empty()
            || self.form.trim().is_empty()
            || self.filed.trim().is_empty()
        {
            return Err(CorpusError::Invalid(format!(
                "normalized SEC fact {} has invalid identity or fields",
                self.id
            )));
        }
        Ok(())
    }

    #[must_use]
    pub fn numeric_value(&self) -> Option<f64> {
        match &self.value {
            Value::Number(value) => value.as_f64(),
            Value::String(value) => value.replace(',', "").parse::<f64>().ok(),
            _ => None,
        }
        .filter(|value| value.is_finite())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecCompanyFacts {
    pub schema_version: u32,
    pub cik: u64,
    pub entity_name: String,
    pub tickers: Vec<String>,
    pub source_url: String,
    pub raw_sha256: String,
    pub normalized_at_unix: u64,
    pub observations: Vec<SecFactObservation>,
}

impl SecCompanyFacts {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SEC_NORMALIZED_FACTS_SCHEMA_VERSION
            || self.cik == 0
            || self.entity_name.trim().is_empty()
            || self.source_url.trim().is_empty()
            || self.raw_sha256.len() != 64
        {
            return Err(CorpusError::Invalid(
                "normalized SEC Company Facts file has invalid identity or schema".to_owned(),
            ));
        }
        let mut ids = BTreeSet::new();
        for observation in &self.observations {
            observation.validate()?;
            if !ids.insert(observation.id.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate normalized SEC fact ID {}",
                    observation.id
                )));
            }
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let facts = serde_json::from_slice::<Self>(
            &fs::read(path).map_err(|error| CorpusError::io(path, error))?,
        )?;
        facts.validate()?;
        Ok(facts)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactsFetchReport {
    pub manifest: SecFactsManifest,
    pub selected_companies: usize,
    pub downloaded: usize,
    pub reused: usize,
    pub failed: usize,
    pub observations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecFactsVerificationReport {
    pub corpus_id: String,
    pub companies: usize,
    pub verified: usize,
    pub raw_bytes: u64,
    pub normalized_bytes: u64,
    pub observations: usize,
}

#[derive(Debug, Deserialize)]
struct CompanyTickerRow {
    cik_str: u64,
    ticker: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct RawCompanyFacts {
    cik: u64,
    #[serde(rename = "entityName")]
    entity_name: String,
    facts: BTreeMap<String, BTreeMap<String, RawConcept>>, 
}

#[derive(Debug, Deserialize)]
struct RawConcept {
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    units: BTreeMap<String, Vec<RawObservation>>,
}

#[derive(Debug, Deserialize)]
struct RawObservation {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    val: Value,
    accn: String,
    #[serde(default)]
    fy: Option<i64>,
    #[serde(default)]
    fp: Option<String>,
    form: String,
    filed: String,
    #[serde(default)]
    frame: Option<String>,
}

#[derive(Debug, Clone)]
struct SelectedCompany {
    cik: u64,
    entity_name: String,
    tickers: Vec<String>,
}

pub fn fetch_sec_companyfacts(options: SecFactsFetchOptions) -> Result<SecFactsFetchReport> {
    options.validate()?;
    fs::create_dir_all(options.output_dir.join("raw"))
        .map_err(|error| CorpusError::io(options.output_dir.join("raw"), error))?;
    fs::create_dir_all(options.output_dir.join("normalized"))
        .map_err(|error| CorpusError::io(options.output_dir.join("normalized"), error))?;
    let mut client = DownloadClient::new(HttpOptions {
        user_agent: options.user_agent.clone(),
        minimum_interval: Duration::from_secs_f64(1.0 / options.requests_per_second),
        maximum_attempts: options.maximum_attempts,
        timeout: Duration::from_secs(120),
    })?;
    let ticker_response = client.get(SEC_TICKERS_URL, options.maximum_ticker_json_bytes)?;
    let rows = serde_json::from_slice::<BTreeMap<String, CompanyTickerRow>>(&ticker_response.bytes)?;
    let companies = select_companies(&rows, &options)?;
    let corpus_id = format!("sec-companyfacts-{}", companies.len());
    let manifest_path = options.output_dir.join(SEC_FACTS_MANIFEST_FILENAME);
    let mut manifest = if manifest_path.is_file() {
        SecFactsManifest::load(&options.output_dir)?
    } else {
        SecFactsManifest::new(corpus_id)
    };
    manifest.company_tickers_sha256 = sha256_hex(&ticker_response.bytes);
    manifest.metadata.insert(
        "companyfacts_base".to_owned(),
        SEC_COMPANYFACTS_BASE.to_owned(),
    );

    let mut downloaded = 0usize;
    let mut reused = 0usize;
    let mut failed = 0usize;
    let mut observations = 0usize;
    for selected in &companies {
        let raw_path = format!("raw/CIK{:010}.json", selected.cik);
        let normalized_path = format!("normalized/CIK{:010}.json", selected.cik);
        let raw_destination = options.output_dir.join(&raw_path);
        let normalized_destination = options.output_dir.join(&normalized_path);
        if !options.overwrite
            && raw_destination.is_file()
            && normalized_destination.is_file()
            && manifest.companies.iter().find(|company| company.cik == selected.cik).is_some_and(
                |company| {
                    fs::metadata(&raw_destination)
                        .map(|metadata| metadata.len() == company.raw_bytes)
                        .unwrap_or(false)
                        && fs::metadata(&normalized_destination)
                            .map(|metadata| metadata.len() == company.normalized_bytes)
                            .unwrap_or(false)
                },
            )
        {
            reused += 1;
            if let Some(company) = manifest.companies.iter().find(|company| company.cik == selected.cik) {
                observations += company.observations;
            }
            continue;
        }
        let url = format!("{SEC_COMPANYFACTS_BASE}/CIK{:010}.json", selected.cik);
        let response = match client.get(&url, options.maximum_companyfacts_bytes) {
            Ok(response) => response,
            Err(error) => {
                failed += 1;
                manifest.failures.push(SecFactsFailure {
                    cik: selected.cik,
                    source_url: url,
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        let raw = match serde_json::from_slice::<RawCompanyFacts>(&response.bytes) {
            Ok(raw) => raw,
            Err(error) => {
                failed += 1;
                manifest.failures.push(SecFactsFailure {
                    cik: selected.cik,
                    source_url: response.final_url,
                    message: error.to_string(),
                    observed_at_unix: unix_timestamp(),
                });
                continue;
            }
        };
        if raw.cik != selected.cik {
            failed += 1;
            manifest.failures.push(SecFactsFailure {
                cik: selected.cik,
                source_url: response.final_url,
                message: format!(
                    "Company Facts response CIK {} does not match requested CIK {}",
                    raw.cik, selected.cik
                ),
                observed_at_unix: unix_timestamp(),
            });
            continue;
        }
        let raw_sha256 = sha256_hex(&response.bytes);
        atomic_write(&raw_destination, &response.bytes)?;
        let normalized = normalize_companyfacts(raw, selected, &response.final_url, &raw_sha256)?;
        let normalized_bytes = serde_json::to_vec_pretty(&normalized)?;
        atomic_write(&normalized_destination, &normalized_bytes)?;
        let taxonomies = normalized
            .observations
            .iter()
            .map(|observation| observation.taxonomy.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let concepts = normalized
            .observations
            .iter()
            .map(|observation| (observation.taxonomy.as_str(), observation.concept.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let observation_count = normalized.observations.len();
        manifest.upsert_company(SecFactsCompanyRecord {
            cik: selected.cik,
            entity_name: normalized.entity_name.clone(),
            tickers: selected.tickers.clone(),
            raw_path,
            raw_sha256,
            raw_bytes: response.bytes.len() as u64,
            normalized_path,
            normalized_sha256: sha256_hex(&normalized_bytes),
            normalized_bytes: normalized_bytes.len() as u64,
            observations: observation_count,
            taxonomies,
            concepts,
            fetched_at_unix: unix_timestamp(),
            metadata: BTreeMap::from([(
                "selected_entity_name".to_owned(),
                selected.entity_name.clone(),
            )]),
        });
        downloaded += 1;
        observations += observation_count;
        manifest.save(&options.output_dir)?;
    }
    manifest.save(&options.output_dir)?;
    Ok(SecFactsFetchReport {
        manifest,
        selected_companies: companies.len(),
        downloaded,
        reused,
        failed,
        observations,
    })
}

pub fn verify_sec_companyfacts(root: impl AsRef<Path>) -> Result<SecFactsVerificationReport> {
    let root = root.as_ref();
    let manifest = SecFactsManifest::load(root)?;
    let mut raw_bytes = 0u64;
    let mut normalized_bytes = 0u64;
    let mut observations = 0usize;
    for company in &manifest.companies {
        let raw_path = root.join(&company.raw_path);
        let normalized_path = root.join(&company.normalized_path);
        let raw = fs::read(&raw_path).map_err(|error| CorpusError::io(&raw_path, error))?;
        let normalized = fs::read(&normalized_path)
            .map_err(|error| CorpusError::io(&normalized_path, error))?;
        if raw.len() as u64 != company.raw_bytes
            || normalized.len() as u64 != company.normalized_bytes
            || sha256_hex(&raw) != company.raw_sha256
            || sha256_hex(&normalized) != company.normalized_sha256
        {
            return Err(CorpusError::Verification(format!(
                "SEC Company Facts receipt mismatch for CIK{:010}",
                company.cik
            )));
        }
        let facts = serde_json::from_slice::<SecCompanyFacts>(&normalized)?;
        facts.validate()?;
        if facts.cik != company.cik || facts.observations.len() != company.observations {
            return Err(CorpusError::Verification(format!(
                "normalized SEC Company Facts content mismatch for CIK{:010}",
                company.cik
            )));
        }
        raw_bytes += raw.len() as u64;
        normalized_bytes += normalized.len() as u64;
        observations += facts.observations.len();
    }
    Ok(SecFactsVerificationReport {
        corpus_id: manifest.corpus_id,
        companies: manifest.companies.len(),
        verified: manifest.companies.len(),
        raw_bytes,
        normalized_bytes,
        observations,
    })
}

fn normalize_companyfacts(
    raw: RawCompanyFacts,
    selected: &SelectedCompany,
    source_url: &str,
    raw_sha256: &str,
) -> Result<SecCompanyFacts> {
    let mut observations = Vec::new();
    for (taxonomy, concepts) in raw.facts {
        for (concept, facts) in concepts {
            for (unit, units) in facts.units {
                for observation in units {
                    let identity = serde_json::json!({
                        "taxonomy": taxonomy,
                        "concept": concept,
                        "unit": unit,
                        "start": observation.start,
                        "end": observation.end,
                        "value": observation.val,
                        "accession": observation.accn,
                        "form": observation.form,
                        "filed": observation.filed,
                        "frame": observation.frame,
                    });
                    let id = sha256_hex(&serde_json::to_vec(&identity)?);
                    observations.push(SecFactObservation {
                        id,
                        taxonomy: taxonomy.clone(),
                        concept: concept.clone(),
                        label: facts.label.clone(),
                        description: facts.description.clone(),
                        unit: unit.clone(),
                        value: observation.val,
                        start: observation.start,
                        end: observation.end,
                        accession_number: observation.accn,
                        fiscal_year: observation.fy,
                        fiscal_period: observation.fp,
                        form: observation.form,
                        filed: observation.filed,
                        frame: observation.frame,
                    });
                }
            }
        }
    }
    observations.sort_unstable_by(|left, right| {
        left.taxonomy
            .cmp(&right.taxonomy)
            .then_with(|| left.concept.cmp(&right.concept))
            .then_with(|| left.unit.cmp(&right.unit))
            .then_with(|| left.end.cmp(&right.end))
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.filed.cmp(&right.filed))
            .then_with(|| left.accession_number.cmp(&right.accession_number))
            .then_with(|| left.id.cmp(&right.id))
    });
    observations.dedup_by(|left, right| left.id == right.id);
    let facts = SecCompanyFacts {
        schema_version: SEC_NORMALIZED_FACTS_SCHEMA_VERSION,
        cik: raw.cik,
        entity_name: if raw.entity_name.trim().is_empty() {
            selected.entity_name.clone()
        } else {
            raw.entity_name
        },
        tickers: selected.tickers.clone(),
        source_url: source_url.to_owned(),
        raw_sha256: raw_sha256.to_owned(),
        normalized_at_unix: unix_timestamp(),
        observations,
    };
    facts.validate()?;
    Ok(facts)
}

fn select_companies(
    rows: &BTreeMap<String, CompanyTickerRow>,
    options: &SecFactsFetchOptions,
) -> Result<Vec<SelectedCompany>> {
    let mut by_ticker = BTreeMap::<String, &CompanyTickerRow>::new();
    let mut by_cik = BTreeMap::<u64, Vec<&CompanyTickerRow>>::new();
    for row in rows.values() {
        by_ticker.insert(row.ticker.to_ascii_uppercase(), row);
        by_cik.entry(row.cik_str).or_default().push(row);
    }
    let make = |cik: u64, rows: &[&CompanyTickerRow]| SelectedCompany {
        cik,
        entity_name: rows
            .first()
            .map_or_else(|| format!("CIK{cik:010}"), |row| row.title.clone()),
        tickers: rows.iter().map(|row| row.ticker.clone()).collect(),
    };
    let mut selected = BTreeMap::<u64, SelectedCompany>::new();
    for ticker in &options.tickers {
        let normalized = ticker.trim().to_ascii_uppercase();
        let row = by_ticker
            .get(&normalized)
            .ok_or_else(|| CorpusError::Invalid(format!("SEC ticker {normalized} was not found")))?;
        selected.insert(row.cik_str, make(row.cik_str, &by_cik[&row.cik_str]));
    }
    for &cik in &options.ciks {
        let rows = by_cik.get(&cik).map_or(&[][..], Vec::as_slice);
        selected.insert(cik, make(cik, rows));
    }
    if selected.is_empty() {
        let count = options.sampled_companies.unwrap_or(25);
        let mut ciks = by_cik.keys().copied().collect::<Vec<_>>();
        ciks.sort_unstable_by_key(|cik| stable_mix(*cik ^ options.seed));
        for cik in ciks.into_iter().take(count) {
            selected.insert(cik, make(cik, &by_cik[&cik]));
        }
    }
    for company in selected.values_mut() {
        company.tickers.sort_unstable();
        company.tickers.dedup();
    }
    Ok(selected.into_values().collect())
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CorpusError::Invalid(format!(
            "unsafe SEC facts relative path {value:?}"
        )));
    }
    Ok(())
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
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{normalize_companyfacts, RawCompanyFacts, SecFactObservation, SelectedCompany};

    #[test]
    fn numeric_values_accept_numbers_and_numeric_strings() {
        let make = |value| SecFactObservation {
            id: "a".repeat(64),
            taxonomy: "us-gaap".to_owned(),
            concept: "Revenues".to_owned(),
            label: String::new(),
            description: String::new(),
            unit: "USD".to_owned(),
            value,
            start: None,
            end: Some("2025-12-31".to_owned()),
            accession_number: "0001".to_owned(),
            fiscal_year: Some(2025),
            fiscal_period: Some("FY".to_owned()),
            form: "10-K".to_owned(),
            filed: "2026-02-01".to_owned(),
            frame: None,
        };
        assert_eq!(make(json!(42.5)).numeric_value(), Some(42.5));
        assert_eq!(make(json!("1,250")).numeric_value(), Some(1250.0));
    }

    #[test]
    fn normalizes_and_deduplicates_companyfacts() {
        let raw: RawCompanyFacts = serde_json::from_value(json!({
            "cik":1,
            "entityName":"Issuer",
            "facts":{"us-gaap":{"Revenues":{
                "label":"Revenue",
                "description":"Revenue",
                "units":{"USD":[{
                    "start":"2024-01-01","end":"2024-12-31","val":100,
                    "accn":"0001","fy":2024,"fp":"FY","form":"10-K","filed":"2025-02-01"
                }]}
            }}}
        }))
        .expect("raw");
        let facts = normalize_companyfacts(
            raw,
            &SelectedCompany {
                cik: 1,
                entity_name: "Issuer".to_owned(),
                tickers: vec!["TEST".to_owned()],
            },
            "https://example.invalid",
            &"0".repeat(64),
        )
        .expect("normalize");
        assert_eq!(facts.observations.len(), 1);
        assert_eq!(facts.observations[0].numeric_value(), Some(100.0));
        assert_eq!(facts.observations[0].concept, "Revenues");
        assert!(BTreeMap::<String, String>::new().is_empty());
    }
}
