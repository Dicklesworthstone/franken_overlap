use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT_ENCODING, ETAG, LAST_MODIFIED, USER_AGENT};
use reqwest::StatusCode;

use crate::{CorpusError, Result};

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub user_agent: String,
    pub minimum_interval: Duration,
    pub maximum_attempts: usize,
    pub timeout: Duration,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "FrankenOverlap/{} corpus-acquisition",
                env!("CARGO_PKG_VERSION")
            ),
            minimum_interval: Duration::from_secs(2),
            maximum_attempts: 4,
            timeout: Duration::from_secs(90),
        }
    }
}

impl HttpOptions {
    pub fn validate(&self) -> Result<()> {
        if self.user_agent.trim().is_empty() {
            return Err(CorpusError::Invalid(
                "HTTP user agent must not be empty".to_owned(),
            ));
        }
        if self.maximum_attempts == 0 || self.maximum_attempts > 32 {
            return Err(CorpusError::Invalid(
                "maximum HTTP attempts must be between 1 and 32".to_owned(),
            ));
        }
        if self.timeout.is_zero() {
            return Err(CorpusError::Invalid(
                "HTTP timeout must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub final_url: String,
}

pub struct DownloadClient {
    client: Client,
    options: HttpOptions,
    last_request: Option<Instant>,
}

impl DownloadClient {
    pub fn new(options: HttpOptions) -> Result<Self> {
        options.validate()?;
        let client = Client::builder()
            .timeout(options.timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self {
            client,
            options,
            last_request: None,
        })
    }

    pub fn get(&mut self, url: &str, maximum_bytes: u64) -> Result<FetchResponse> {
        if maximum_bytes == 0 {
            return Err(CorpusError::Invalid(
                "maximum download bytes must be positive".to_owned(),
            ));
        }
        let mut last_status = None;
        for attempt in 0..self.options.maximum_attempts {
            self.wait_for_rate_limit();
            let response = self
                .client
                .get(url)
                .header(USER_AGENT, &self.options.user_agent)
                .header(ACCEPT_ENCODING, "gzip, br, deflate")
                .send();
            self.last_request = Some(Instant::now());
            match response {
                Ok(response) if response.status().is_success() => {
                    return read_response(response, maximum_bytes);
                }
                Ok(response) => {
                    let status = response.status();
                    last_status = Some(status);
                    if !is_retryable_status(status)
                        || attempt + 1 >= self.options.maximum_attempts
                    {
                        return Err(CorpusError::HttpStatus {
                            url: url.to_owned(),
                            status: status.as_u16(),
                        });
                    }
                }
                Err(error) => {
                    if attempt + 1 >= self.options.maximum_attempts {
                        return Err(error.into());
                    }
                }
            }
            let exponent = u32::try_from(attempt.min(6)).unwrap_or(6);
            let delay = Duration::from_millis(250u64.saturating_mul(2u64.pow(exponent)));
            thread::sleep(delay);
        }
        Err(CorpusError::HttpStatus {
            url: url.to_owned(),
            status: last_status.map_or(599, |status| status.as_u16()),
        })
    }

    fn wait_for_rate_limit(&self) {
        let Some(last_request) = self.last_request else {
            return;
        };
        let elapsed = last_request.elapsed();
        if elapsed < self.options.minimum_interval {
            thread::sleep(self.options.minimum_interval - elapsed);
        }
    }
}

fn read_response(response: Response, maximum_bytes: u64) -> Result<FetchResponse> {
    let final_url = response.url().to_string();
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(CorpusError::DownloadTooLarge {
            url: final_url,
            limit: maximum_bytes,
        });
    }
    let etag = header_string(&response, ETAG);
    let last_modified = header_string(&response, LAST_MODIFIED);
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(64 * 1024),
    );
    let limit = maximum_bytes.saturating_add(1);
    response
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| CorpusError::io(final_url.clone(), error))?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(CorpusError::DownloadTooLarge {
            url: final_url,
            limit: maximum_bytes,
        });
    }
    Ok(FetchResponse {
        bytes,
        etag,
        last_modified,
        final_url,
    })
}

fn header_string(response: &Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

#[cfg(test)]
mod tests {
    use super::HttpOptions;

    #[test]
    fn rejects_empty_user_agent() {
        let options = HttpOptions {
            user_agent: " ".to_owned(),
            ..HttpOptions::default()
        };
        assert!(options.validate().is_err());
    }
}
