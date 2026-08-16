#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use fo_core::{
    DomainFeaturePolicy, DomainSearchOptions, HybridFilter, HybridIndex, HybridQueryMode,
    HybridSearchOptions, LexicalSearchOptions, SearchIntent, SearchOptions,
    SemanticCandidateSet, SemanticFusionOptions, TextDomain, fuse_semantic_candidates,
};
use serde::{Deserialize, Serialize};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const LATENCY_BUCKETS_MICROS: [u64; 10] = [
    1_000, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000, 5_000_000,
];
const MAX_HEADER_BYTES: usize = 32 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "fo-serve",
    version,
    about = "Serve one resident FrankenOverlap hybrid index with bounded concurrency and metrics"
)]
struct Cli {
    index: PathBuf,
    #[arg(long, default_value = "127.0.0.1:7070")]
    bind: SocketAddr,
    #[arg(long, default_value_t = 8)]
    threads: usize,
    #[arg(long, default_value_t = 128)]
    queue_capacity: usize,
    #[arg(long, default_value_t = 1_048_576)]
    maximum_request_bytes: usize,
    #[arg(long, default_value_t = 262_144)]
    maximum_query_bytes: usize,
    #[arg(long, default_value_t = 20)]
    default_limit: usize,
    #[arg(long, default_value_t = 8)]
    candidate_multiplier: usize,
    #[arg(long, default_value_t = 15_000)]
    read_timeout_ms: u64,
    #[arg(long, default_value_t = 30_000)]
    write_timeout_ms: u64,
    /// Environment variable containing a bearer token. Required for non-loopback binds unless explicitly overridden.
    #[arg(long)]
    api_key_env: Option<String>,
    /// Permit a non-loopback bind without authentication.
    #[arg(long)]
    allow_unauthenticated_public: bool,
    /// Optional Access-Control-Allow-Origin response value.
    #[arg(long)]
    allow_origin: Option<String>,
}

#[derive(Debug, Clone)]
struct ServiceConfig {
    bind: SocketAddr,
    threads: usize,
    queue_capacity: usize,
    maximum_request_bytes: usize,
    maximum_query_bytes: usize,
    default_limit: usize,
    candidate_multiplier: usize,
    read_timeout: Duration,
    write_timeout: Duration,
    api_key: Option<String>,
    allow_origin: Option<String>,
}

#[derive(Debug)]
struct AppState {
    index: Arc<HybridIndex>,
    config: ServiceConfig,
    metrics: ServiceMetrics,
    started: Instant,
}

#[derive(Debug)]
struct ServiceMetrics {
    accepted_connections: AtomicU64,
    queue_rejections: AtomicU64,
    requests: AtomicU64,
    completed: AtomicU64,
    failures: AtomicU64,
    unauthorized: AtomicU64,
    oversized: AtomicU64,
    in_flight: AtomicU64,
    search_requests: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    latency_sum_micros: AtomicU64,
    latency_buckets: [AtomicU64; LATENCY_BUCKETS_MICROS.len()],
}

impl ServiceMetrics {
    fn new() -> Self {
        Self {
            accepted_connections: AtomicU64::new(0),
            queue_rejections: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            unauthorized: AtomicU64::new(0),
            oversized: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            search_requests: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            latency_sum_micros: AtomicU64::new(0),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn observe_latency(&self, elapsed: Duration) {
        let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.latency_sum_micros
            .fetch_add(micros, Ordering::Relaxed);
        for (bucket, counter) in LATENCY_BUCKETS_MICROS
            .iter()
            .zip(&self.latency_buckets)
        {
            if micros <= *bucket {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    wire_bytes: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct SearchRequest {
    query: String,
    mode: HybridQueryMode,
    domain: Option<TextDomain>,
    limit: Option<usize>,
    candidate_multiplier: Option<usize>,
    minimum_score: f32,
    minimum_similarity: f32,
    minimum_matched_tokens: usize,
    minimum_query_coverage: f32,
    minimum_source_coverage: f32,
    maximum_postings_per_feature: usize,
    maximum_postings_per_term: usize,
    lexical_candidates: usize,
    filter: HybridFilter,
    domain_policy: Option<DomainFeaturePolicy>,
    semantic_candidates: Option<SemanticCandidateSet>,
    semantic_options: Option<SemanticFusionOptions>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: HybridQueryMode::Auto,
            domain: None,
            limit: None,
            candidate_multiplier: None,
            minimum_score: 0.0,
            minimum_similarity: 0.20,
            minimum_matched_tokens: 24,
            minimum_query_coverage: 0.10,
            minimum_source_coverage: 0.10,
            maximum_postings_per_feature: 50_000,
            maximum_postings_per_term: 1_000_000,
            lexical_candidates: 50_000,
            filter: HybridFilter::default(),
            domain_policy: None,
            semantic_candidates: None,
            semantic_options: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "report", rename_all = "snake_case")]
enum SearchResponse {
    Hybrid(fo_core::HybridSearchReport),
    Domain(fo_core::DomainSearchReport),
    Semantic(fo_core::SemanticFusionReport),
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    uptime_seconds: u64,
    documents: usize,
    overlap_fingerprints: usize,
    overlap_postings: usize,
    lexical_terms: usize,
    lexical_postings: usize,
    in_flight: u64,
    queue_capacity: usize,
    worker_threads: usize,
}

#[derive(Debug, Serialize)]
struct IndexResponse {
    stats: fo_core::HybridIndexStats,
    config: fo_core::HybridIndexConfig,
}

#[derive(Debug, Serialize)]
struct DocumentResponse {
    external_id: String,
    title: String,
    tags: Vec<String>,
    metadata: BTreeMap<String, String>,
    body_bytes: usize,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: u16,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fo-serve: {error}");
        std::process::exit(1);
    }
}

fn run() -> CliResult<()> {
    let command = Cli::parse();
    let config = service_config(&command)?;
    let index = HybridIndex::load(&command.index)?;
    index.validate()?;
    let listener = TcpListener::bind(config.bind)?;
    let local = listener.local_addr()?;
    let state = Arc::new(AppState {
        index: Arc::new(index),
        config: config.clone(),
        metrics: ServiceMetrics::new(),
        started: Instant::now(),
    });
    let (sender, receiver) = sync_channel::<TcpStream>(config.queue_capacity);
    spawn_workers(receiver, Arc::clone(&state), config.threads);

    println!("FrankenOverlap service listening on http://{local}");
    println!("  workers:       {}", config.threads);
    println!("  queue:         {}", config.queue_capacity);
    println!("  documents:     {}", state.index.stats().documents);
    println!(
        "  authentication:{}",
        if config.api_key.is_some() {
            " bearer token"
        } else {
            " none"
        }
    );

    for accepted in listener.incoming() {
        match accepted {
            Ok(stream) => {
                state
                    .metrics
                    .accepted_connections
                    .fetch_add(1, Ordering::Relaxed);
                match sender.try_send(stream) {
                    Ok(()) => {}
                    Err(TrySendError::Full(mut stream)) => {
                        state
                            .metrics
                            .queue_rejections
                            .fetch_add(1, Ordering::Relaxed);
                        let _ = write_error(
                            &mut stream,
                            ApiError::new(503, "queue_full", "request queue is full"),
                            &state,
                        );
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err("worker queue disconnected".into());
                    }
                }
            }
            Err(error) => eprintln!("fo-serve: accept error: {error}"),
        }
    }
    Ok(())
}

fn service_config(command: &Cli) -> CliResult<ServiceConfig> {
    if command.threads == 0
        || command.threads > 512
        || command.queue_capacity == 0
        || command.maximum_request_bytes == 0
        || command.maximum_query_bytes == 0
        || command.maximum_query_bytes > command.maximum_request_bytes
        || command.default_limit == 0
        || command.candidate_multiplier == 0
        || command.read_timeout_ms == 0
        || command.write_timeout_ms == 0
    {
        return Err("service count, byte, and timeout limits are invalid".into());
    }
    let api_key = match &command.api_key_env {
        Some(variable) => {
            let value = std::env::var(variable).map_err(|_| {
                format!("API key environment variable {variable:?} is not set")
            })?;
            if value.trim().is_empty() {
                return Err(format!("API key environment variable {variable:?} is empty").into());
            }
            Some(value)
        }
        None => None,
    };
    if !is_loopback(command.bind.ip())
        && api_key.is_none()
        && !command.allow_unauthenticated_public
    {
        return Err(
            "non-loopback binds require --api-key-env or --allow-unauthenticated-public".into(),
        );
    }
    if command
        .allow_origin
        .as_ref()
        .is_some_and(|value| value.contains(['\r', '\n']))
    {
        return Err("allow-origin must not contain CR or LF".into());
    }
    Ok(ServiceConfig {
        bind: command.bind,
        threads: command.threads,
        queue_capacity: command.queue_capacity,
        maximum_request_bytes: command.maximum_request_bytes,
        maximum_query_bytes: command.maximum_query_bytes,
        default_limit: command.default_limit,
        candidate_multiplier: command.candidate_multiplier,
        read_timeout: Duration::from_millis(command.read_timeout_ms),
        write_timeout: Duration::from_millis(command.write_timeout_ms),
        api_key,
        allow_origin: command.allow_origin.clone(),
    })
}

fn spawn_workers(receiver: Receiver<TcpStream>, state: Arc<AppState>, threads: usize) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..threads {
        let receiver = Arc::clone(&receiver);
        let state = Arc::clone(&state);
        thread::Builder::new()
            .name(format!("fo-http-{index}"))
            .spawn(move || loop {
                let stream = {
                    let receiver = receiver.lock().expect("worker receiver mutex poisoned");
                    receiver.recv()
                };
                match stream {
                    Ok(stream) => handle_connection(stream, &state),
                    Err(_) => break,
                }
            })
            .expect("could not spawn HTTP worker");
    }
}

fn handle_connection(mut stream: TcpStream, state: &AppState) {
    state.metrics.in_flight.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let _guard = InFlightGuard(&state.metrics.in_flight);
    if let Err(error) = stream.set_read_timeout(Some(state.config.read_timeout)) {
        eprintln!("fo-serve: could not set read timeout: {error}");
    }
    if let Err(error) = stream.set_write_timeout(Some(state.config.write_timeout)) {
        eprintln!("fo-serve: could not set write timeout: {error}");
    }
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);

    let result = read_request(&mut stream, state.config.maximum_request_bytes)
        .and_then(|request| {
            state
                .metrics
                .bytes_in
                .fetch_add(request.wire_bytes as u64, Ordering::Relaxed);
            route(&request, state)
        });
    let written = match result {
        Ok(response) => write_response(&mut stream, response, state),
        Err(error) => {
            state.metrics.failures.fetch_add(1, Ordering::Relaxed);
            if error.status == 401 {
                state.metrics.unauthorized.fetch_add(1, Ordering::Relaxed);
            }
            if error.status == 413 {
                state.metrics.oversized.fetch_add(1, Ordering::Relaxed);
            }
            write_error(&mut stream, error, state)
        }
    };
    match written {
        Ok(bytes) => {
            state
                .metrics
                .bytes_out
                .fetch_add(bytes as u64, Ordering::Relaxed);
            state.metrics.completed.fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.failures.fetch_add(1, Ordering::Relaxed);
            eprintln!("fo-serve: response write failed: {error}");
        }
    }
    state.metrics.observe_latency(started.elapsed());
}

struct InFlightGuard<'a>(&'a AtomicU64);

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn route(request: &HttpRequest, state: &AppState) -> Result<HttpResponse, ApiError> {
    if request.method == "GET" && request.path == "/health" {
        return json_response(200, &health(state));
    }
    authorize(request, state)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/metrics") => Ok(HttpResponse {
            status: 200,
            content_type: "text/plain; version=0.0.4; charset=utf-8",
            body: prometheus_metrics(state).into_bytes(),
        }),
        ("GET", "/v1/index") => json_response(
            200,
            &IndexResponse {
                stats: state.index.stats(),
                config: state.index.config().clone(),
            },
        ),
        ("POST", "/v1/search") => {
            state
                .metrics
                .search_requests
                .fetch_add(1, Ordering::Relaxed);
            let search = serde_json::from_slice::<SearchRequest>(&request.body)
                .map_err(|error| ApiError::new(400, "invalid_json", error.to_string()))?;
            let response = execute_search(search, state)?;
            json_response(200, &response)
        }
        ("GET", path) if path.starts_with("/v1/document/") => {
            let encoded = &path["/v1/document/".len()..];
            let id = percent_decode(encoded)?;
            let document = state
                .index
                .lexical_index()
                .documents()
                .iter()
                .find(|document| document.external_id == id)
                .ok_or_else(|| ApiError::new(404, "document_not_found", "unknown document id"))?;
            json_response(
                200,
                &DocumentResponse {
                    external_id: document.external_id.clone(),
                    title: document.title.clone(),
                    tags: document.tags.clone(),
                    metadata: document.metadata.clone(),
                    body_bytes: document.body.len(),
                },
            )
        }
        _ => Err(ApiError::new(404, "not_found", "unknown endpoint")),
    }
}

fn execute_search(request: SearchRequest, state: &AppState) -> Result<SearchResponse, ApiError> {
    if request.query.trim().is_empty() {
        return Err(ApiError::new(400, "empty_query", "query must not be empty"));
    }
    if request.query.len() > state.config.maximum_query_bytes {
        return Err(ApiError::new(
            413,
            "query_too_large",
            format!(
                "query has {} bytes; maximum is {}",
                request.query.len(),
                state.config.maximum_query_bytes
            ),
        ));
    }
    let limit = request.limit.unwrap_or(state.config.default_limit);
    let candidate_multiplier = request
        .candidate_multiplier
        .unwrap_or(state.config.candidate_multiplier);
    if limit == 0 || limit > 10_000 || candidate_multiplier == 0 || candidate_multiplier > 1_000 {
        return Err(ApiError::new(
            400,
            "invalid_search_limits",
            "limit or candidate_multiplier is outside safe bounds",
        ));
    }

    if let Some(domain) = request.domain {
        if request.semantic_candidates.is_some() {
            return Err(ApiError::new(
                400,
                "incompatible_lanes",
                "semantic fusion currently requires a hybrid search request, not domain-only overlap",
            ));
        }
        let policy = request
            .domain_policy
            .unwrap_or_else(|| DomainFeaturePolicy::for_domain(domain));
        let report = state
            .index
            .overlap_index()
            .search_domain_adaptive(
                &request.query,
                &DomainSearchOptions {
                    domain,
                    policy,
                    search: SearchOptions {
                        intent: SearchIntent::SourceAttribution,
                        max_results: limit,
                        max_candidates: limit.saturating_mul(candidate_multiplier).max(limit),
                        max_postings_per_feature: request.maximum_postings_per_feature,
                        maximum_document_frequency_fraction: policy
                            .maximum_document_frequency_fraction,
                        minimum_feature_idf: policy.minimum_feature_idf,
                        maximum_query_posting_pairs: policy.maximum_query_posting_pairs,
                        minimum_informative_feature_fraction: policy
                            .minimum_informative_feature_fraction,
                        minimum_similarity: request.minimum_similarity,
                        minimum_matched_tokens: request.minimum_matched_tokens,
                        minimum_query_coverage: request.minimum_query_coverage,
                        minimum_source_coverage: request.minimum_source_coverage,
                        ..SearchOptions::default()
                    },
                },
            )
            .map_err(api_core_error)?;
        return Ok(SearchResponse::Domain(report));
    }

    let hybrid = state
        .index
        .search(
            &request.query,
            &HybridSearchOptions {
                mode: request.mode,
                max_results: limit,
                candidate_multiplier,
                lexical: LexicalSearchOptions {
                    max_results: limit.saturating_mul(candidate_multiplier),
                    max_candidate_documents: request.lexical_candidates,
                    maximum_postings_per_term: request.maximum_postings_per_term,
                    minimum_score: 0.0,
                    ..LexicalSearchOptions::default()
                },
                overlap: SearchOptions {
                    max_results: limit.saturating_mul(candidate_multiplier),
                    max_candidates: limit
                        .saturating_mul(candidate_multiplier)
                        .saturating_mul(4)
                        .max(200),
                    max_postings_per_feature: request.maximum_postings_per_feature,
                    minimum_similarity: request.minimum_similarity,
                    minimum_matched_tokens: request.minimum_matched_tokens,
                    minimum_query_coverage: request.minimum_query_coverage,
                    minimum_source_coverage: request.minimum_source_coverage,
                    ..SearchOptions::default()
                },
                minimum_score: request.minimum_score,
                filter: request.filter,
                ..HybridSearchOptions::default()
            },
        )
        .map_err(api_core_error)?;
    match request.semantic_candidates {
        Some(semantic) => {
            let fused = fuse_semantic_candidates(
                &hybrid,
                &semantic,
                &request.semantic_options.unwrap_or_default(),
            )
            .map_err(api_core_error)?;
            Ok(SearchResponse::Semantic(fused))
        }
        None => Ok(SearchResponse::Hybrid(hybrid)),
    }
}

fn authorize(request: &HttpRequest, state: &AppState) -> Result<(), ApiError> {
    let Some(expected) = state.config.api_key.as_deref() else {
        return Ok(());
    };
    let observed = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "));
    if observed == Some(expected) {
        Ok(())
    } else {
        Err(ApiError::new(
            401,
            "unauthorized",
            "missing or invalid bearer token",
        ))
    }
}

fn health(state: &AppState) -> HealthResponse {
    let stats = state.index.stats();
    HealthResponse {
        status: "ok",
        uptime_seconds: state.started.elapsed().as_secs(),
        documents: stats.documents,
        overlap_fingerprints: stats.overlap.distinct_fingerprints,
        overlap_postings: stats.overlap.postings,
        lexical_terms: stats.lexical.distinct_terms,
        lexical_postings: stats.lexical.postings,
        in_flight: state.metrics.in_flight.load(Ordering::Relaxed),
        queue_capacity: state.config.queue_capacity,
        worker_threads: state.config.threads,
    }
}

fn prometheus_metrics(state: &AppState) -> String {
    let metrics = &state.metrics;
    let mut output = String::new();
    metric(&mut output, "fo_accepted_connections_total", metrics.accepted_connections.load(Ordering::Relaxed));
    metric(&mut output, "fo_queue_rejections_total", metrics.queue_rejections.load(Ordering::Relaxed));
    metric(&mut output, "fo_requests_total", metrics.requests.load(Ordering::Relaxed));
    metric(&mut output, "fo_completed_total", metrics.completed.load(Ordering::Relaxed));
    metric(&mut output, "fo_failures_total", metrics.failures.load(Ordering::Relaxed));
    metric(&mut output, "fo_unauthorized_total", metrics.unauthorized.load(Ordering::Relaxed));
    metric(&mut output, "fo_oversized_total", metrics.oversized.load(Ordering::Relaxed));
    metric(&mut output, "fo_in_flight", metrics.in_flight.load(Ordering::Relaxed));
    metric(&mut output, "fo_search_requests_total", metrics.search_requests.load(Ordering::Relaxed));
    metric(&mut output, "fo_bytes_in_total", metrics.bytes_in.load(Ordering::Relaxed));
    metric(&mut output, "fo_bytes_out_total", metrics.bytes_out.load(Ordering::Relaxed));
    for (boundary, counter) in LATENCY_BUCKETS_MICROS
        .iter()
        .zip(&metrics.latency_buckets)
    {
        output.push_str(&format!(
            "fo_request_latency_seconds_bucket{{le=\"{:.6}\"}} {}\n",
            *boundary as f64 / 1_000_000.0,
            counter.load(Ordering::Relaxed)
        ));
    }
    output.push_str(&format!(
        "fo_request_latency_seconds_bucket{{le=\"+Inf\"}} {}\n",
        metrics.completed.load(Ordering::Relaxed)
    ));
    output.push_str(&format!(
        "fo_request_latency_seconds_sum {:.6}\n",
        metrics.latency_sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    ));
    output.push_str(&format!(
        "fo_request_latency_seconds_count {}\n",
        metrics.completed.load(Ordering::Relaxed)
    ));
    output
}

fn metric(output: &mut String, name: &str, value: u64) {
    output.push_str(name);
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn read_request(stream: &mut TcpStream, maximum_bytes: usize) -> Result<HttpRequest, ApiError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ApiError::new(400, "read_failed", error.to_string()))?;
        if read == 0 {
            return Err(ApiError::new(400, "incomplete_request", "connection closed before headers"));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > maximum_bytes || bytes.len() > MAX_HEADER_BYTES && find_header_end(&bytes).is_none() {
            return Err(ApiError::new(413, "request_too_large", "request headers exceed the configured limit"));
        }
        if let Some(end) = find_header_end(&bytes) {
            break end;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| ApiError::new(400, "invalid_headers", "headers are not UTF-8"))?;
    let (method, path, headers) = parse_head(head)?;
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(ApiError::new(400, "unsupported_transfer_encoding", "chunked transfer encoding is not supported"));
    }
    let content_length = match headers.get("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| {
            ApiError::new(400, "invalid_content_length", "invalid Content-Length")
        })?,
        None if method == "POST" => {
            return Err(ApiError::new(411, "length_required", "POST requires Content-Length"));
        }
        None => 0,
    };
    if header_end.saturating_add(content_length) > maximum_bytes {
        return Err(ApiError::new(413, "request_too_large", "request exceeds configured byte limit"));
    }
    let body_start = header_end + 4;
    while bytes.len() < body_start.saturating_add(content_length) {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ApiError::new(400, "read_failed", error.to_string()))?;
        if read == 0 {
            return Err(ApiError::new(400, "incomplete_body", "connection closed before request body"));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > maximum_bytes {
            return Err(ApiError::new(413, "request_too_large", "request exceeds configured byte limit"));
        }
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: bytes[body_start..body_start + content_length].to_vec(),
        wire_bytes: body_start + content_length,
    })
}

fn parse_head(head: &str) -> Result<(String, String, BTreeMap<String, String>), ApiError> {
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ApiError::new(400, "invalid_request_line", "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || !matches!(method, "GET" | "POST")
        || !path.starts_with('/')
        || !version.starts_with("HTTP/1.")
        || path.contains(['\r', '\n'])
    {
        return Err(ApiError::new(400, "invalid_request_line", "invalid HTTP request line"));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ApiError::new(400, "invalid_header", "malformed HTTP header"));
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
            return Err(ApiError::new(400, "invalid_header", "invalid HTTP header name"));
        }
        if headers.insert(name, value.trim().to_owned()).is_some() {
            return Err(ApiError::new(400, "duplicate_header", "duplicate HTTP header"));
        }
    }
    Ok((method.to_owned(), path.to_owned(), headers))
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_error(stream: &mut TcpStream, error: ApiError, state: &AppState) -> std::io::Result<usize> {
    let body = serde_json::to_vec(&ErrorBody {
        error: ErrorDetail {
            code: error.code,
            message: error.message,
        },
    })
    .unwrap_or_else(|_| b"{\"error\":{\"code\":\"serialization_failed\",\"message\":\"error serialization failed\"}}".to_vec());
    write_response(
        stream,
        HttpResponse {
            status: error.status,
            content_type: "application/json; charset=utf-8",
            body,
        },
        state,
    )
}

fn json_response<T: Serialize>(status: u16, value: &T) -> Result<HttpResponse, ApiError> {
    let body = serde_json::to_vec(value)
        .map_err(|error| ApiError::new(500, "serialization_failed", error.to_string()))?;
    Ok(HttpResponse {
        status,
        content_type: "application/json; charset=utf-8",
        body,
    })
}

fn write_response(
    stream: &mut TcpStream,
    response: HttpResponse,
    state: &AppState,
) -> std::io::Result<usize> {
    let reason = status_reason(response.status);
    let cors = state.config.allow_origin.as_ref().map_or_else(String::new, |origin| {
        format!("Access-Control-Allow-Origin: {origin}\r\n")
    });
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        cors,
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(head.len() + response.body.len())
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn percent_decode(value: &str) -> Result<String, ApiError> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(ApiError::new(400, "invalid_path_encoding", "truncated percent encoding"));
            }
            let high = hex(bytes[index + 1])?;
            let low = hex(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output)
        .map_err(|_| ApiError::new(400, "invalid_path_encoding", "document id is not UTF-8"))
}

fn hex(value: u8) -> Result<u8, ApiError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ApiError::new(400, "invalid_path_encoding", "invalid percent encoding")),
    }
}

fn api_core_error(error: fo_core::FoError) -> ApiError {
    ApiError::new(400, "search_failed", error.to_string())
}

const fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::{SearchRequest, parse_head, percent_decode, service_config};
    use crate::Cli;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    #[test]
    fn parses_strict_http_headers() {
        let (method, path, headers) = parse_head(
            "POST /v1/search HTTP/1.1\r\nHost: localhost\r\nContent-Length: 13",
        )
        .expect("head");
        assert_eq!(method, "POST");
        assert_eq!(path, "/v1/search");
        assert_eq!(headers["content-length"], "13");
        assert!(parse_head("POST / HTTP/1.1\r\nX: 1\r\nX: 2").is_err());
    }

    #[test]
    fn decodes_document_ids_without_form_url_semantics() {
        assert_eq!(percent_decode("CIK0001%23section-1").expect("decode"), "CIK0001#section-1");
        assert_eq!(percent_decode("a+b").expect("decode"), "a+b");
        assert!(percent_decode("bad%2").is_err());
    }

    #[test]
    fn rejects_public_unauthenticated_binding_by_default() {
        let command = Cli {
            index: PathBuf::from("index"),
            bind: "0.0.0.0:7070".parse::<SocketAddr>().expect("address"),
            threads: 1,
            queue_capacity: 1,
            maximum_request_bytes: 1024,
            maximum_query_bytes: 512,
            default_limit: 10,
            candidate_multiplier: 4,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
            api_key_env: None,
            allow_unauthenticated_public: false,
            allow_origin: None,
        };
        assert!(service_config(&command).is_err());
    }

    #[test]
    fn search_request_defaults_are_bounded() {
        let request = serde_json::from_str::<SearchRequest>("{\"query\":\"alpha\"}")
            .expect("request");
        assert_eq!(request.query, "alpha");
        assert_eq!(request.minimum_matched_tokens, 24);
        assert_eq!(request.maximum_postings_per_feature, 50_000);
    }
}
