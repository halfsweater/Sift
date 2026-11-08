//! Polite asynchronous HTTP fetching.
//!
//! * robots.txt is always honoured (including `Crawl-delay`), cached per origin.
//! * Requests to one host are spaced by a per-domain rate limiter.
//! * Transient failures are retried with exponential backoff (`Retry-After` aware).
//! * Response bodies are streamed and cut off at `max_body_bytes`.
//! * Redirects are followed manually so the full chain is recorded and every hop
//!   is checked against robots.txt and the host policy.
//! * Optionally refuses loopback / private / link-local targets (SSRF protection
//!   for the HTTP API), including hostnames that *resolve* to such addresses.

mod limiter;
mod robots;
mod ssrf;

use limiter::DomainLimiter;
use reqwest::{header, redirect::Policy, Client, StatusCode};
use robots::RobotsCache;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use url::Url;

pub use ssrf::is_private_host;

#[derive(Debug, Clone)]
pub struct FetcherConfig {
    pub user_agent: String,
    /// Product token matched against `User-agent:` groups in robots.txt.
    pub robots_agent: String,
    pub timeout: Duration,
    pub max_body_bytes: usize,
    pub max_retries: u32,
    pub retry_base_delay: Duration,
    /// Minimum spacing between requests to the same host.
    pub per_domain_delay: Duration,
    pub respect_robots: bool,
    pub max_redirects: usize,
    /// Refuse loopback/private/link-local destinations.
    pub block_private_hosts: bool,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            user_agent: format!("Sift/{} (+https://github.com/sift-rs/sift)", env!("CARGO_PKG_VERSION")),
            robots_agent: "Sift".into(),
            timeout: Duration::from_secs(15),
            max_body_bytes: 5 * 1024 * 1024,
            max_retries: 2,
            retry_base_delay: Duration::from_millis(500),
            per_domain_delay: Duration::from_millis(250),
            respect_robots: true,
            max_redirects: 8,
            block_private_hosts: false,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("invalid URL `{0}`")]
    InvalidUrl(String),
    #[error("unsupported URL scheme `{0}` (only http and https are allowed)")]
    UnsupportedScheme(String),
    #[error("host `{0}` is not allowed (private, loopback or link-local address)")]
    BlockedHost(String),
    #[error("disallowed by robots.txt: {0}")]
    RobotsDisallowed(String),
    #[error("robots.txt unavailable ({0}); not crawling because permission cannot be confirmed")]
    RobotsUnavailable(String),
    #[error("request timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("cancelled")]
    Cancelled,
}

impl FetchError {
    pub fn is_robots(&self) -> bool {
        matches!(self, FetchError::RobotsDisallowed(_))
    }
}

/// A completed HTTP exchange. Non-2xx statuses are *not* errors: the caller
/// decides what to do with a 404 or 503 that survived the retries.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub requested_url: Url,
    pub final_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    /// Selected response headers (lower-cased names): server, x-powered-by, x-generator, via.
    pub headers: Vec<(String, String)>,
    /// Decoded text body; `None` for non-textual content types or non-2xx without a body.
    pub body: Option<String>,
    pub body_bytes: usize,
    /// True if the body hit `max_body_bytes` and was cut.
    pub truncated: bool,
    /// Time spent on the final request (excluding rate-limit waits and earlier hops).
    pub elapsed: Duration,
    /// Every URL visited, starting with the requested one.
    pub redirect_chain: Vec<String>,
}

impl FetchedPage {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

struct RawResponse {
    status: StatusCode,
    headers: header::HeaderMap,
    body: Option<Vec<u8>>,
    truncated: bool,
    elapsed: Duration,
}

pub struct Fetcher {
    client: Client,
    robots_client: Client,
    cfg: FetcherConfig,
    limiter: DomainLimiter,
    robots: RobotsCache,
}

const KEPT_HEADERS: [&str; 4] = ["server", "x-powered-by", "x-generator", "via"];

impl Fetcher {
    pub fn new(cfg: FetcherConfig) -> Result<Self, FetchError> {
        let build = |policy: Policy| -> Result<Client, FetchError> {
            let mut b = Client::builder()
                .user_agent(cfg.user_agent.clone())
                .timeout(cfg.timeout)
                .connect_timeout(cfg.timeout.min(Duration::from_secs(10)))
                .redirect(policy);
            if cfg.block_private_hosts {
                b = b.dns_resolver(std::sync::Arc::new(ssrf::SafeResolver)).no_proxy();
            }
            b.build().map_err(|e| FetchError::Network(e.to_string()))
        };
        Ok(Self {
            client: build(Policy::none())?,
            robots_client: build(Policy::limited(5))?,
            limiter: DomainLimiter::default(),
            robots: RobotsCache::default(),
            cfg,
        })
    }

    pub fn config(&self) -> &FetcherConfig {
        &self.cfg
    }

    /// Whether robots.txt permits fetching `url` (always true when robots handling is off).
    pub async fn robots_allows(&self, url: &Url) -> bool {
        if !self.cfg.respect_robots {
            return true;
        }
        self.robots.get(self, url).await.allows(url)
    }

    /// `Sitemap:` entries declared in the host's robots.txt.
    pub async fn robots_sitemaps(&self, url: &Url) -> Vec<String> {
        if !self.cfg.respect_robots {
            return Vec::new();
        }
        self.robots.get(self, url).await.sitemaps.clone()
    }

    pub async fn fetch(&self, url: &Url, cancel: &CancellationToken) -> Result<FetchedPage, FetchError> {
        let mut current = url.clone();
        let mut chain = vec![current.to_string()];
        for _ in 0..=self.cfg.max_redirects {
            self.check_target(&current)?;
            if self.cfg.respect_robots {
                let info = self.robots.get(self, &current).await;
                if let Some(reason) = &info.unavailable {
                    return Err(FetchError::RobotsUnavailable(reason.clone()));
                }
                if !info.allows(&current) {
                    return Err(FetchError::RobotsDisallowed(current.to_string()));
                }
            }
            let raw = self.request_with_retry(&current, cancel).await?;

            if raw.status.is_redirection() {
                if let Some(next) = raw
                    .headers
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|loc| current.join(loc).ok())
                {
                    chain.push(next.to_string());
                    current = next;
                    continue;
                }
            }

            let content_type = raw.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
            let headers = KEPT_HEADERS
                .iter()
                .filter_map(|n| {
                    raw.headers.get(*n).and_then(|v| v.to_str().ok()).map(|v| (n.to_string(), v.to_string()))
                })
                .collect();
            let body_bytes = raw.body.as_ref().map_or(0, Vec::len);
            let body = raw.body.map(|b| decode(&b, content_type.as_deref()));
            return Ok(FetchedPage {
                requested_url: url.clone(),
                final_url: current,
                status: raw.status.as_u16(),
                content_type,
                headers,
                body,
                body_bytes,
                truncated: raw.truncated,
                elapsed: raw.elapsed,
                redirect_chain: chain,
            });
        }
        Err(FetchError::TooManyRedirects)
    }

    fn check_target(&self, url: &Url) -> Result<(), FetchError> {
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(FetchError::UnsupportedScheme(other.to_string())),
        }
        let host = url.host_str().ok_or_else(|| FetchError::InvalidUrl(url.to_string()))?;
        if self.cfg.block_private_hosts && ssrf::is_private_host(host) {
            return Err(FetchError::BlockedHost(host.to_string()));
        }
        Ok(())
    }

    /// One logical request: rate limited, retried on transient failures.
    async fn request_with_retry(&self, url: &Url, cancel: &CancellationToken) -> Result<RawResponse, FetchError> {
        let host = host_key(url);
        let mut attempt = 0u32;
        loop {
            let delay = self.effective_delay(url).await;
            self.limiter.acquire(&host, delay, cancel).await?;

            let outcome = or_cancel(cancel, self.send_once(url)).await?;
            let retry_after = match &outcome {
                Ok(raw) if is_retryable_status(raw.status) => Some(parse_retry_after(&raw.headers)),
                Ok(_) => None,
                Err(e) if is_retryable_error(e) => Some(None),
                Err(_) => None,
            };

            match (outcome, retry_after) {
                (Ok(raw), Some(ra)) if attempt < self.cfg.max_retries => {
                    tracing::debug!(%url, status = %raw.status, attempt, "retrying");
                    self.backoff(attempt, ra, cancel).await?;
                }
                (Err(_), Some(ra)) if attempt < self.cfg.max_retries => {
                    tracing::debug!(%url, attempt, "retrying after network error");
                    self.backoff(attempt, ra, cancel).await?;
                }
                (Ok(raw), _) => return Ok(raw),
                (Err(e), _) => return Err(map_reqwest_error(&e)),
            }
            attempt += 1;
        }
    }

    async fn effective_delay(&self, url: &Url) -> Duration {
        let mut d = self.cfg.per_domain_delay;
        if self.cfg.respect_robots {
            if let Some(cd) = self.robots.get(self, url).await.crawl_delay {
                d = d.max(cd);
            }
        }
        d
    }

    async fn backoff(
        &self,
        attempt: u32,
        retry_after: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<(), FetchError> {
        let max = Duration::from_secs(30);
        let exp = self.cfg.retry_base_delay.saturating_mul(1u32 << attempt.min(10));
        let wait = retry_after.unwrap_or_else(|| jitter(exp)).min(max);
        sleep_or_cancel(wait, cancel).await
    }

    async fn send_once(&self, url: &Url) -> Result<RawResponse, reqwest::Error> {
        let started = Instant::now();
        let mut resp = self
            .client
            .get(url.clone())
            .header(header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5")
            .send()
            .await?;
        let status = resp.status();
        let headers = resp.headers().clone();

        let want_body = !status.is_redirection()
            && !is_retryable_status(status)
            && is_textual(headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()));
        let mut body = None;
        let mut truncated = false;
        if want_body {
            let max = self.cfg.max_body_bytes;
            let mut buf: Vec<u8> =
                Vec::with_capacity(resp.content_length().map_or(16 * 1024, |l| (l as usize).min(max)).min(1 << 20));
            while let Some(chunk) = resp.chunk().await? {
                if buf.len() + chunk.len() > max {
                    buf.extend_from_slice(&chunk[..max - buf.len()]);
                    truncated = true;
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            body = Some(buf);
        }
        Ok(RawResponse { status, headers, body, truncated, elapsed: started.elapsed() })
    }
}

fn host_key(url: &Url) -> String {
    match url.port() {
        Some(p) => format!("{}:{}", url.host_str().unwrap_or_default(), p),
        None => url.host_str().unwrap_or_default().to_string(),
    }
}

pub(crate) async fn or_cancel<T>(cancel: &CancellationToken, fut: impl Future<Output = T>) -> Result<T, FetchError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(FetchError::Cancelled),
        v = fut => Ok(v),
    }
}

pub(crate) async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> Result<(), FetchError> {
    or_cancel(cancel, tokio::time::sleep(d)).await
}

fn is_retryable_status(s: StatusCode) -> bool {
    matches!(s.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn is_retryable_error(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request() || e.is_body()
}

fn map_reqwest_error(e: &reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else {
        // Walk the source chain so users see "dns error" / "connection refused", not just "error sending request".
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            msg = format!("{msg}: {s}");
            src = s.source();
        }
        FetchError::Network(msg)
    }
}

fn parse_retry_after(h: &header::HeaderMap) -> Option<Duration> {
    h.get(header::RETRY_AFTER)?.to_str().ok()?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// +/-20% jitter derived from the clock; avoids pulling in an RNG crate.
fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.subsec_nanos());
    let factor = 0.8 + (nanos % 400) as f64 / 1000.0;
    d.mul_f64(factor)
}

fn is_textual(content_type: Option<&str>) -> bool {
    match content_type {
        None => true,
        Some(ct) => {
            let ct = ct.to_ascii_lowercase();
            ct.starts_with("text/") || ct.contains("xml")
        }
    }
}

/// Decode using the HTTP charset, then a `<meta charset>` sniff, then UTF-8.
fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    let from_header = content_type.and_then(|ct| {
        let lower = ct.to_ascii_lowercase();
        let idx = lower.find("charset=")?;
        let label = lower[idx + 8..].split(';').next()?.trim().trim_matches('"').to_string();
        encoding_rs::Encoding::for_label(label.as_bytes())
    });
    let enc = from_header.or_else(|| {
        let head = &bytes[..bytes.len().min(2048)];
        let head = String::from_utf8_lossy(head).to_ascii_lowercase();
        let idx = head.find("charset=")?;
        let label: String = head[idx + 8..]
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
            .collect();
        encoding_rs::Encoding::for_label(label.as_bytes())
    });
    enc.unwrap_or(encoding_rs::UTF_8).decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_latin1_from_header_and_meta() {
        let bytes = b"caf\xe9";
        assert_eq!(decode(bytes, Some("text/html; charset=ISO-8859-1")), "café");
        let html = b"<meta charset=\"iso-8859-1\">caf\xe9";
        assert!(decode(html, Some("text/html")).ends_with("café"));
        assert_eq!(decode("héllo".as_bytes(), None), "héllo");
    }

    #[test]
    fn textual_content_types() {
        assert!(is_textual(None));
        assert!(is_textual(Some("text/html; charset=utf-8")));
        assert!(is_textual(Some("application/xml")));
        assert!(!is_textual(Some("image/png")));
        assert!(!is_textual(Some("application/pdf")));
    }

    #[test]
    fn rejects_non_http_schemes() {
        let f = Fetcher::new(FetcherConfig::default()).unwrap();
        let err = f.check_target(&Url::parse("ftp://example.org/x").unwrap()).unwrap_err();
        assert!(matches!(err, FetchError::UnsupportedScheme(_)));
    }

    #[test]
    fn private_hosts_blocked_when_configured() {
        let f = Fetcher::new(FetcherConfig { block_private_hosts: true, ..Default::default() }).unwrap();
        for u in [
            "http://127.0.0.1/",
            "http://localhost:8080/",
            "http://10.1.2.3/",
            "http://169.254.169.254/",
            "http://[::1]/",
        ] {
            assert!(matches!(f.check_target(&Url::parse(u).unwrap()), Err(FetchError::BlockedHost(_))), "{u}");
        }
        assert!(f.check_target(&Url::parse("https://example.org/").unwrap()).is_ok());
    }
}
