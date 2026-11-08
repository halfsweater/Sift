//! The Sift engine: seeds + an [`ExtractionSpec`] in, a structured [`ResultSet`] out.
//!
//! Pipeline: URL input → (discovery) → crawl/fetch → parse/extract → validate →
//! normalize → deduplicate → records. Each stage lives in its own crate or module
//! and can be used on its own.

pub mod pipeline;
pub mod search;

use sift_core::{ExtractionSpec, ResultSet};
use sift_crawler::{CrawlConfig, Crawler};
use sift_fetcher::{FetchError, Fetcher, FetcherConfig};
use sift_normalize::{host_key, parse_lenient};
use std::collections::HashSet;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use url::Url;

pub use search::{build_search_query, SearxngProvider};
pub use sift_crawler::{CrawlStats, ProgressFn};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("`{0}` is not a valid http(s) URL")]
    InvalidSeed(String),
    #[error("no URLs to scrape: provide at least one URL")]
    NoSeeds,
    #[error("search failed: {0}")]
    Search(String),
    #[error(transparent)]
    Fetch(#[from] FetchError),
}

/// What one record represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordScope {
    /// One record per website, merging every crawled page (business directories, lead lists).
    Site,
    /// One record per crawled page (site audits, `sift crawl`).
    Page,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub max_depth: u32,
    /// Page budget **per site**.
    pub max_pages: usize,
    pub concurrency: usize,
    /// Only follow links likely to hold the requested fields.
    pub relevant_only: bool,
    pub use_sitemaps: bool,
    pub include_subdomains: bool,
    pub scope: RecordScope,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_pages: 15,
            concurrency: 8,
            relevant_only: true,
            use_sitemaps: true,
            include_subdomains: false,
            scope: RecordScope::Site,
        }
    }
}

/// Parse user-supplied seeds, adding `https://` when the scheme is missing.
pub fn parse_seeds<S: AsRef<str>>(inputs: &[S]) -> Result<Vec<Url>, EngineError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in inputs {
        let u = parse_lenient(i.as_ref()).ok_or_else(|| EngineError::InvalidSeed(i.as_ref().to_string()))?;
        if seen.insert(sift_normalize::url_key(&u)) {
            out.push(u);
        }
    }
    if out.is_empty() {
        return Err(EngineError::NoSeeds);
    }
    Ok(out)
}

pub struct Engine {
    fetcher: Arc<Fetcher>,
}

impl Engine {
    pub fn new(cfg: FetcherConfig) -> Result<Self, EngineError> {
        Ok(Self { fetcher: Arc::new(Fetcher::new(cfg)?) })
    }

    pub fn with_fetcher(fetcher: Arc<Fetcher>) -> Self {
        Self { fetcher }
    }

    pub fn fetcher(&self) -> &Arc<Fetcher> {
        &self.fetcher
    }

    /// Crawl `seeds`, extract the fields in `spec`, and assemble clean records.
    ///
    /// Never fails once seeds are parsed: unreachable sites, robots.txt blocks and HTTP
    /// errors are reported in `metadata.errors` / counters instead. When `cancel` fires,
    /// the partial results gathered so far are returned with `metadata.cancelled = true`.
    pub async fn run(
        &self,
        seeds: Vec<Url>,
        spec: ExtractionSpec,
        opts: RunOptions,
        cancel: CancellationToken,
        progress: Option<ProgressFn>,
    ) -> ResultSet {
        let sites = seeds.iter().map(host_key).collect::<HashSet<_>>().len().max(1);
        let cfg = CrawlConfig {
            max_depth: opts.max_depth,
            max_pages: opts.max_pages.saturating_mul(sites),
            max_pages_per_host: opts.max_pages,
            concurrency: opts.concurrency,
            include_subdomains: opts.include_subdomains,
            relevant_only: opts.relevant_only,
            use_sitemaps: opts.use_sitemaps,
            fields: spec.fields.clone(),
        };
        let crawl = Crawler::new(self.fetcher.clone(), cfg).run(seeds, cancel, progress).await;
        let built = pipeline::build(&crawl, &spec, opts.scope);
        let errors = pipeline::collect_errors(&crawl);
        let metadata = pipeline::metadata(&crawl, &built, errors);
        ResultSet { query: spec.query.clone(), spec, results: built.records, metadata }
    }
}
