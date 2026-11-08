//! Concurrent, prioritised, polite crawling.
//!
//! The crawler keeps a priority queue of URLs scored by how likely they are to hold
//! the requested fields (see [`priority`]), fetches up to `concurrency` pages at once
//! through a shared [`Fetcher`] (robots.txt, per-domain rate limits, retries), parses
//! each page with `sift-extract`, and enqueues newly discovered in-scope links.

pub mod priority;
pub mod sitemap;

use serde::Serialize;
use sift_core::FieldKind;
use sift_extract::{analyze, PageAnalysis};
use sift_fetcher::Fetcher;
use sift_normalize::{host_key, url_key};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, Clone)]
pub struct CrawlConfig {
    /// Link depth from a seed (seed = 0). `0` fetches only the seeds.
    pub max_depth: u32,
    /// Total pages fetched across all seeds.
    pub max_pages: usize,
    /// Pages fetched per host.
    pub max_pages_per_host: usize,
    pub concurrency: usize,
    /// Also follow links to subdomains of a seed host.
    pub include_subdomains: bool,
    /// Only follow links whose URL/text suggests they hold the requested fields.
    pub relevant_only: bool,
    pub use_sitemaps: bool,
    pub fields: Vec<FieldKind>,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_pages: 50,
            max_pages_per_host: 50,
            concurrency: 8,
            include_subdomains: false,
            relevant_only: true,
            use_sitemaps: true,
            fields: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CrawlStats {
    /// Pages for which an HTTP response was received.
    pub pages_fetched: usize,
    pub pages_ok: usize,
    /// Network errors and non-2xx responses.
    pub pages_failed: usize,
    pub blocked_by_robots: usize,
    pub off_scope_redirects: usize,
    pub bytes_downloaded: u64,
    pub urls_discovered: usize,
    pub duplicates_skipped: usize,
    pub irrelevant_skipped: usize,
    pub queued: usize,
    pub in_flight: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PageResult {
    pub requested: Url,
    /// Final URL after redirects.
    pub url: Url,
    pub depth: u32,
    pub status: Option<u16>,
    pub analysis: Option<PageAnalysis>,
    pub error: Option<String>,
    pub robots_blocked: bool,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
pub struct SitemapFound {
    pub site: String,
    pub url: String,
}

#[derive(Debug)]
pub struct CrawlOutput {
    pub pages: Vec<PageResult>,
    pub stats: CrawlStats,
    pub sitemaps: Vec<SitemapFound>,
    pub cancelled: bool,
}

pub type ProgressFn = Arc<dyn Fn(&CrawlStats) + Send + Sync>;

struct QueueItem {
    score: i32,
    seq: u64,
    depth: u32,
    url: Url,
}

impl PartialEq for QueueItem {
    fn eq(&self, o: &Self) -> bool {
        self.score == o.score && self.seq == o.seq
    }
}
impl Eq for QueueItem {}
impl PartialOrd for QueueItem {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for QueueItem {
    /// Higher score first; among equals, earlier insertion first.
    fn cmp(&self, o: &Self) -> Ordering {
        self.score.cmp(&o.score).then_with(|| o.seq.cmp(&self.seq))
    }
}

const MAX_QUEUE: usize = 20_000;

pub struct Crawler {
    fetcher: Arc<Fetcher>,
    cfg: CrawlConfig,
}

struct State {
    heap: BinaryHeap<QueueItem>,
    visited: HashSet<String>,
    seq: u64,
    stats: CrawlStats,
    seed_hosts: Vec<String>,
    keywords: Vec<&'static str>,
}

impl Crawler {
    pub fn new(fetcher: Arc<Fetcher>, cfg: CrawlConfig) -> Self {
        Self { fetcher, cfg }
    }

    pub async fn run(&self, seeds: Vec<Url>, cancel: CancellationToken, progress: Option<ProgressFn>) -> CrawlOutput {
        let started = Instant::now();
        let mut st = State {
            heap: BinaryHeap::new(),
            visited: HashSet::new(),
            seq: 0,
            stats: CrawlStats::default(),
            seed_hosts: seeds
                .iter()
                .filter_map(|s| s.host_str())
                .map(|h| h.trim_start_matches("www.").to_ascii_lowercase())
                .collect(),
            keywords: priority::keywords_for(&self.cfg.fields),
        };
        let fields: Arc<[FieldKind]> = self.cfg.fields.clone().into();
        let mut pages: Vec<PageResult> = Vec::new();
        let mut sitemaps: Vec<SitemapFound> = Vec::new();

        for seed in &seeds {
            if st.visited.insert(url_key(seed)) {
                self.push(&mut st, seed.clone(), 0, i32::MAX / 2);
            }
        }

        if self.cfg.use_sitemaps && (self.cfg.max_depth > 0 || self.cfg.fields.contains(&FieldKind::Sitemaps)) {
            self.discover_sitemaps(&seeds, &cancel, &mut st, &mut sitemaps).await;
        }

        let mut join: JoinSet<PageResult> = JoinSet::new();
        let mut scheduled = 0usize;
        let mut per_host: HashMap<String, usize> = HashMap::new();

        loop {
            while join.len() < self.cfg.concurrency.max(1) && scheduled < self.cfg.max_pages && !cancel.is_cancelled() {
                let Some(item) = st.heap.pop() else { break };
                let hk = host_key(&item.url);
                let used = per_host.entry(hk).or_default();
                if *used >= self.cfg.max_pages_per_host {
                    continue;
                }
                *used += 1;
                scheduled += 1;
                let (fetcher, fields, cancel) = (self.fetcher.clone(), fields.clone(), cancel.clone());
                join.spawn(fetch_and_analyze(fetcher, fields, cancel, item.url, item.depth));
            }
            if join.is_empty() {
                break;
            }
            st.stats.queued = st.heap.len();
            st.stats.in_flight = join.len();
            match join.join_next().await {
                Some(Ok(result)) => self.absorb(result, &mut st, &mut pages),
                Some(Err(e)) => tracing::error!(error = %e, "crawl task failed"),
                None => break,
            }
            st.stats.queued = st.heap.len();
            st.stats.in_flight = join.len();
            st.stats.elapsed_ms = started.elapsed().as_millis() as u64;
            if let Some(p) = &progress {
                p(&st.stats);
            }
        }

        st.stats.queued = 0;
        st.stats.in_flight = 0;
        st.stats.elapsed_ms = started.elapsed().as_millis() as u64;
        if let Some(p) = &progress {
            p(&st.stats);
        }
        CrawlOutput { pages, stats: st.stats, sitemaps, cancelled: cancel.is_cancelled() }
    }

    fn push(&self, st: &mut State, url: Url, depth: u32, score: i32) {
        if st.heap.len() >= MAX_QUEUE {
            return;
        }
        st.seq += 1;
        st.heap.push(QueueItem { score, seq: st.seq, depth, url });
    }

    fn in_scope(&self, st: &State, url: &Url) -> bool {
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        let Some(h) = url.host_str() else { return false };
        let h = h.trim_start_matches("www.").to_ascii_lowercase();
        st.seed_hosts.iter().any(|s| h == *s || (self.cfg.include_subdomains && h.ends_with(&format!(".{s}"))))
    }

    async fn discover_sitemaps(
        &self,
        seeds: &[Url],
        cancel: &CancellationToken,
        st: &mut State,
        found: &mut Vec<SitemapFound>,
    ) {
        let mut origins: Vec<Url> = Vec::new();
        let mut seen = HashSet::new();
        for s in seeds {
            if seen.insert(host_key(s)) {
                origins.push(s.clone());
            }
        }
        let mut set = JoinSet::new();
        for origin in origins {
            let (f, c) = (self.fetcher.clone(), cancel.clone());
            set.spawn(async move {
                let d = sitemap::discover(&f, &origin, &c).await;
                (host_key(&origin), d)
            });
        }
        while let Some(Ok((site, d))) = set.join_next().await {
            for doc in d.documents {
                found.push(SitemapFound { site: site.clone(), url: doc });
            }
            // Sitemap documents are reported even at depth 0, but their pages are only queued when links may be followed.
            if self.cfg.max_depth == 0 {
                continue;
            }
            for page in d.pages {
                if !self.in_scope(st, &page) || priority::is_skipped_resource(&page) {
                    continue;
                }
                let score = priority::score_link(&page, "", &st.keywords);
                if self.cfg.relevant_only && score <= 0 {
                    continue;
                }
                if st.visited.insert(url_key(&page)) {
                    st.stats.urls_discovered += 1;
                    self.push(st, page, 1, score - 10);
                }
            }
        }
    }

    fn absorb(&self, mut result: PageResult, st: &mut State, pages: &mut Vec<PageResult>) {
        let stats = &mut st.stats;
        if result.robots_blocked {
            stats.blocked_by_robots += 1;
        } else if let Some(status) = result.status {
            stats.pages_fetched += 1;
            if (200..300).contains(&status) {
                stats.pages_ok += 1;
            } else {
                stats.pages_failed += 1;
                result.error.get_or_insert(format!("HTTP {status}"));
            }
        } else if result.error.as_deref() != Some("cancelled") {
            stats.pages_failed += 1;
        }

        // A redirect may have left the crawl scope; keep the record but don't use its content.
        if result.analysis.is_some() && !self.in_scope(st, &result.url) {
            st.stats.off_scope_redirects += 1;
            result.error = Some(format!("redirected outside crawl scope to {}", result.url));
            result.analysis = None;
        }
        st.visited.insert(url_key(&result.url));

        if let Some(analysis) = &result.analysis {
            if result.depth < self.cfg.max_depth {
                for link in &analysis.links {
                    if !self.in_scope(st, &link.url) || priority::is_skipped_resource(&link.url) {
                        continue;
                    }
                    let key = url_key(&link.url);
                    if st.visited.contains(&key) {
                        st.stats.duplicates_skipped += 1;
                        continue;
                    }
                    let score = priority::score_link(&link.url, &link.text, &st.keywords);
                    if self.cfg.relevant_only && score <= 0 {
                        st.stats.irrelevant_skipped += 1;
                        continue;
                    }
                    st.visited.insert(key);
                    st.stats.urls_discovered += 1;
                    let penalty = 10 * (result.depth as i32 + 1);
                    self.push(st, link.url.clone(), result.depth + 1, score - penalty);
                }
            }
        }
        st.stats.bytes_downloaded += result.bytes as u64;
        pages.push(result);
    }
}

async fn fetch_and_analyze(
    fetcher: Arc<Fetcher>,
    fields: Arc<[FieldKind]>,
    cancel: CancellationToken,
    url: Url,
    depth: u32,
) -> PageResult {
    match fetcher.fetch(&url, &cancel).await {
        Ok(page) => {
            let (status, bytes) = (page.status, page.body_bytes);
            let final_url = page.final_url.clone();
            let analysis = tokio::task::spawn_blocking(move || analyze(&page, &fields)).await.ok();
            PageResult {
                requested: url,
                url: final_url,
                depth,
                status: Some(status),
                analysis,
                error: None,
                robots_blocked: false,
                bytes,
            }
        }
        Err(e) => PageResult {
            requested: url.clone(),
            url,
            depth,
            status: None,
            analysis: None,
            robots_blocked: e.is_robots(),
            bytes: 0,
            error: Some(if matches!(e, sift_fetcher::FetchError::Cancelled) {
                "cancelled".into()
            } else {
                e.to_string()
            }),
        },
    }
}
