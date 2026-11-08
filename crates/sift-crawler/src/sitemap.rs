//! Sitemap discovery and parsing.

use regex::Regex;
use sift_fetcher::Fetcher;
use std::collections::HashSet;
use std::sync::LazyLock;
use tokio_util::sync::CancellationToken;
use url::Url;

static LOC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<loc>\s*(?:<!\[CDATA\[)?\s*([^<\]\s]+)").unwrap());

const MAX_SITEMAP_DOCS: usize = 5;
const MAX_URLS: usize = 2000;

#[derive(Debug, Default)]
pub struct SitemapDiscovery {
    /// Sitemap documents that were successfully fetched.
    pub documents: Vec<String>,
    /// Page URLs listed in them (same host only).
    pub pages: Vec<Url>,
}

/// Parse a sitemap body into `(is_index, locations)`.
pub fn parse_sitemap(body: &str) -> (bool, Vec<String>) {
    let is_index = body.contains("<sitemapindex");
    let locs = LOC_RE.captures_iter(body).map(|c| c[1].replace("&amp;", "&")).collect();
    (is_index, locs)
}

/// Look for sitemaps declared in robots.txt plus `/sitemap.xml`, following one level of
/// sitemap indexes. Respects robots.txt like every other fetch.
pub async fn discover(fetcher: &Fetcher, site: &Url, cancel: &CancellationToken) -> SitemapDiscovery {
    let mut queue: Vec<Url> = Vec::new();
    for s in fetcher.robots_sitemaps(site).await {
        if let Ok(u) = Url::parse(&s) {
            queue.push(u);
        }
    }
    if let Ok(default) = site.join("/sitemap.xml") {
        queue.push(default);
    }

    let mut result = SitemapDiscovery::default();
    let mut seen_docs: HashSet<String> = HashSet::new();
    let mut seen_pages: HashSet<String> = HashSet::new();
    let host = site.host_str().map(str::to_ascii_lowercase);
    let mut fetched = 0;
    let mut i = 0;
    while i < queue.len() && fetched < MAX_SITEMAP_DOCS && !cancel.is_cancelled() {
        let doc_url = queue[i].clone();
        i += 1;
        if !seen_docs.insert(doc_url.to_string()) {
            continue;
        }
        fetched += 1;
        let Ok(page) = fetcher.fetch(&doc_url, cancel).await else { continue };
        let ok = page.is_success();
        let Some(body) = page.body.filter(|_| ok) else { continue };
        let (is_index, locs) = parse_sitemap(&body);
        if locs.is_empty() && !body.contains("<urlset") && !is_index {
            continue; // 200 OK soft-404 page, not a sitemap
        }
        result.documents.push(doc_url.to_string());
        for loc in locs {
            let Ok(u) = Url::parse(&loc) else { continue };
            if u.host_str().map(str::to_ascii_lowercase) != host {
                continue;
            }
            if is_index {
                queue.push(u);
            } else if result.pages.len() < MAX_URLS && seen_pages.insert(u.to_string()) {
                result.pages.push(u);
            }
        }
    }
    result
}
