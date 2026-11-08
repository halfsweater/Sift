//! Optional website discovery through a SearxNG-compatible metasearch endpoint.
//!
//! Sift ships no search engine and does not scrape search result pages. Point it at
//! your own (or a trusted) SearxNG instance with JSON output enabled, or pass seed URLs.

use crate::EngineError;
use serde::Deserialize;
use sift_core::ExtractionSpec;
use sift_normalize::host_key;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

/// Hosts that are platforms rather than the business's own website.
const PLATFORM_HOSTS: &[&str] = &[
    "facebook.com",
    "instagram.com",
    "linkedin.com",
    "twitter.com",
    "x.com",
    "youtube.com",
    "tiktok.com",
    "pinterest.com",
    "reddit.com",
    "wikipedia.org",
    "google.com",
    "bing.com",
    "duckduckgo.com",
];

/// "software development agencies" + "Kathmandu" -> "software development agencies Kathmandu".
pub fn build_search_query(spec: &ExtractionSpec) -> Option<String> {
    match (&spec.subject, &spec.location) {
        (Some(s), Some(l)) => Some(format!("{s} {l}")),
        (Some(s), None) => Some(s.clone()),
        (None, _) => spec.query.clone(),
    }
}

#[derive(Debug, Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}

#[derive(Debug, Deserialize)]
struct SearxResult {
    url: String,
}

pub struct SearxngProvider {
    endpoint: Url,
    client: reqwest::Client,
}

impl SearxngProvider {
    pub fn new(endpoint: &str, user_agent: &str) -> Result<Self, EngineError> {
        let endpoint = Url::parse(endpoint)
            .map_err(|e| EngineError::Search(format!("invalid search endpoint `{endpoint}`: {e}")))?;
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| EngineError::Search(e.to_string()))?;
        Ok(Self { endpoint, client })
    }

    /// Return up to `limit` distinct site home pages for `query`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Url>, EngineError> {
        let mut url = self.endpoint.join("search").map_err(|e| EngineError::Search(e.to_string()))?;
        url.query_pairs_mut().append_pair("q", query).append_pair("format", "json");
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| EngineError::Search(format!("search request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(EngineError::Search(format!(
                "search endpoint returned HTTP {} (does the instance enable `format=json`?)",
                resp.status()
            )));
        }
        let bytes = resp.bytes().await.map_err(|e| EngineError::Search(format!("search request failed: {e}")))?;
        let body: SearxResponse = serde_json::from_slice(&bytes)
            .map_err(|e| EngineError::Search(format!("unexpected search response: {e}")))?;
        Ok(origins(body.results.iter().map(|r| r.url.as_str()), limit))
    }
}

/// Reduce result URLs to distinct business home pages, skipping social/search platforms.
pub fn origins<'a>(urls: impl Iterator<Item = &'a str>, limit: usize) -> Vec<Url> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in urls {
        let Ok(mut u) = Url::parse(raw) else { continue };
        if !matches!(u.scheme(), "http" | "https") {
            continue;
        }
        let host = host_key(&u);
        if PLATFORM_HOSTS.iter().any(|p| host == *p || host.ends_with(&format!(".{p}"))) {
            continue;
        }
        if seen.insert(host) {
            u.set_path("/");
            u.set_query(None);
            u.set_fragment(None);
            out.push(u);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::{RequestInterpreter, RuleBasedInterpreter};

    #[test]
    fn query_from_spec() {
        let spec = RuleBasedInterpreter.interpret("Find software development agencies in Kathmandu. Collect emails.");
        assert_eq!(build_search_query(&spec).as_deref(), Some("software development agencies Kathmandu"));
        let spec = RuleBasedInterpreter.interpret("emails of dentists");
        assert_eq!(build_search_query(&spec).as_deref(), Some("dentists"));
    }

    #[test]
    fn origins_dedupe_and_skip_platforms() {
        let urls = [
            "https://www.acme.np/services/web?x=1",
            "https://acme.np/contact",
            "https://facebook.com/acme",
            "https://m.facebook.com/other",
            "not a url",
            "ftp://files.np/x",
            "https://beta.np/about",
        ];
        let o = origins(urls.into_iter(), 10);
        assert_eq!(o.iter().map(Url::as_str).collect::<Vec<_>>(), vec!["https://www.acme.np/", "https://beta.np/"]);
        assert_eq!(origins(urls.into_iter(), 1).len(), 1);
    }
}
