//! Optional REST API for Sift. See `docs/api.md` for the full reference.
//!
//! * `POST /api/v1/scrape`, `POST /api/v1/crawl` start asynchronous jobs (202 + job id).
//! * `POST /api/v1/extract` extracts from a single page synchronously.
//! * `GET /api/v1/jobs/{id}`, `GET /api/v1/jobs/{id}/results`, `DELETE /api/v1/jobs/{id}`.
//!
//! Every limit is enforced by rejecting the request (HTTP 400), never by silently clamping.
//! By default the fetcher refuses loopback/private/link-local targets.

mod error;
mod jobs;
mod requests;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use error::ApiError;
use jobs::{JobKind, JobStore};
use requests::{ExtractRequest, JobRequest};
use serde_json::json;
use sift_core::ResultSet;
use sift_engine::{parse_seeds, Engine, EngineError, RecordScope, RunOptions};
use sift_fetcher::FetcherConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub use error::ErrorBody;
pub use jobs::{JobStatus, JobSummary};

/// Hard limits applied to every request.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_urls: usize,
    pub max_pages: usize,
    pub max_depth: u32,
    pub max_concurrency: usize,
    pub max_running_jobs: usize,
    pub max_stored_jobs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_urls: 20,
            max_pages: 200,
            max_depth: 4,
            max_concurrency: 16,
            max_running_jobs: 8,
            max_stored_jobs: 200,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub fetcher: FetcherConfig,
    pub limits: Limits,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { fetcher: FetcherConfig { block_private_hosts: true, ..Default::default() }, limits: Limits::default() }
    }
}

pub(crate) struct AppState {
    engine: Engine,
    limits: Limits,
    block_private: bool,
    jobs: JobStore,
}

type Shared = Arc<AppState>;

pub fn router(cfg: ApiConfig) -> Result<Router, EngineError> {
    let state = Arc::new(AppState {
        block_private: cfg.fetcher.block_private_hosts,
        engine: Engine::new(cfg.fetcher)?,
        jobs: JobStore::new(cfg.limits.max_stored_jobs),
        limits: cfg.limits,
    });
    Ok(Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/docs", get(docs))
        .route("/api/v1/scrape", post(scrape))
        .route("/api/v1/crawl", post(crawl))
        .route("/api/v1/extract", post(extract))
        .route("/api/v1/jobs/{id}", get(job_status).delete(job_delete))
        .route("/api/v1/jobs/{id}/results", get(job_results))
        .with_state(state))
}

/// Serve until `shutdown` resolves; running jobs are cancelled on the way out.
pub async fn serve(
    addr: SocketAddr,
    cfg: ApiConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = router(cfg).map_err(|e| std::io::Error::other(e.to_string()))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "Sift API listening");
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn docs() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], include_str!("../../../docs/api.md"))
}

fn base_options(
    limits: &Limits,
    scope: RecordScope,
    relevant_only: bool,
    req: &JobRequest,
) -> Result<RunOptions, ApiError> {
    let depth = req.depth.unwrap_or(2);
    let max_pages = req.max_pages.unwrap_or(if relevant_only { 15 } else { 50 });
    let concurrency = req.concurrency.unwrap_or(8);
    if depth > limits.max_depth {
        return Err(ApiError::invalid(format!("`depth` must be at most {}", limits.max_depth)));
    }
    if max_pages == 0 || max_pages > limits.max_pages {
        return Err(ApiError::invalid(format!("`max_pages` must be between 1 and {}", limits.max_pages)));
    }
    if concurrency == 0 || concurrency > limits.max_concurrency {
        return Err(ApiError::invalid(format!("`concurrency` must be between 1 and {}", limits.max_concurrency)));
    }
    Ok(RunOptions {
        max_depth: depth,
        max_pages,
        concurrency,
        relevant_only,
        use_sitemaps: true,
        include_subdomains: req.include_subdomains.unwrap_or(false),
        scope,
    })
}

fn check_urls(state: &AppState, urls: &[String]) -> Result<Vec<url::Url>, ApiError> {
    if urls.is_empty() || urls.len() > state.limits.max_urls {
        return Err(ApiError::invalid(format!("`urls` must contain between 1 and {} entries", state.limits.max_urls)));
    }
    let seeds = parse_seeds(urls).map_err(|e| ApiError::invalid(e.to_string()))?;
    if state.block_private {
        for s in &seeds {
            if s.host_str().is_some_and(sift_fetcher::is_private_host) {
                return Err(ApiError::invalid(format!(
                    "`{s}` points at a private or loopback address, which is not allowed"
                )));
            }
        }
    }
    Ok(seeds)
}

async fn start_job(state: Shared, body: Bytes, kind: JobKind) -> Result<Response, ApiError> {
    let req: JobRequest = requests::parse(&body)?;
    let spec = req.spec()?;
    let (scope, relevant) = match kind {
        JobKind::Scrape => (RecordScope::Site, true),
        JobKind::Crawl => (RecordScope::Page, false),
    };
    let opts = base_options(&state.limits, scope, relevant, &req)?;
    let seeds = check_urls(&state, &req.urls)?;

    let cancel = CancellationToken::new();
    let job = state.jobs.create(kind, cancel.clone(), state.limits.max_running_jobs)?;
    let (id, runner) = (job.id, job.clone());
    let progress_job = job.clone();
    let engine_state = state.clone();
    tokio::spawn(async move {
        let progress: sift_engine::ProgressFn = Arc::new(move |s| progress_job.set_stats(s.clone()));
        let run = tokio::spawn(async move { engine_state.engine.run(seeds, spec, opts, cancel, Some(progress)).await });
        match run.await {
            Ok(rs) => runner.finish(rs),
            Err(e) => runner.fail(format!("internal error: {e}")),
        }
    });

    let location = format!("/api/v1/jobs/{id}");
    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location.clone())],
        Json(json!({
            "id": id,
            "kind": kind,
            "status": JobStatus::Running,
            "links": { "self": location, "results": format!("{location}/results") }
        })),
    )
        .into_response())
}

async fn scrape(State(state): State<Shared>, body: Bytes) -> Result<Response, ApiError> {
    start_job(state, body, JobKind::Scrape).await
}

async fn crawl(State(state): State<Shared>, body: Bytes) -> Result<Response, ApiError> {
    start_job(state, body, JobKind::Crawl).await
}

async fn extract(State(state): State<Shared>, body: Bytes) -> Result<Json<ResultSet>, ApiError> {
    let req: ExtractRequest = requests::parse(&body)?;
    let spec = req.spec()?;
    let seeds = check_urls(&state, std::slice::from_ref(&req.url))?;
    let opts = RunOptions { max_depth: 0, max_pages: 1, concurrency: 1, use_sitemaps: false, ..RunOptions::default() };
    Ok(Json(state.engine.run(seeds, spec, opts, CancellationToken::new(), None).await))
}

fn parse_id(id: &str) -> Result<uuid::Uuid, ApiError> {
    id.parse().map_err(|_| ApiError::not_found(id))
}

async fn job_status(State(state): State<Shared>, Path(id): Path<String>) -> Result<Json<JobSummary>, ApiError> {
    let job = state.jobs.get(parse_id(&id)?).ok_or_else(|| ApiError::not_found(&id))?;
    Ok(Json(job.summary()))
}

async fn job_results(State(state): State<Shared>, Path(id): Path<String>) -> Result<Json<Arc<ResultSet>>, ApiError> {
    let job = state.jobs.get(parse_id(&id)?).ok_or_else(|| ApiError::not_found(&id))?;
    match job.results() {
        Ok(rs) => Ok(Json(rs)),
        Err(e) => Err(e),
    }
}

async fn job_delete(State(state): State<Shared>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let job = state.jobs.remove(parse_id(&id)?).ok_or_else(|| ApiError::not_found(&id))?;
    job.cancel();
    Ok(StatusCode::NO_CONTENT)
}
