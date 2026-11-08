//! robots.txt retrieval and caching.

use crate::{host_key, Fetcher};
use reqwest::header;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use texting_robots::Robot;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use url::Url;

const MAX_ROBOTS_BYTES: usize = 512 * 1024;
const MAX_CRAWL_DELAY: Duration = Duration::from_secs(10);

enum Policy {
    Rules(Box<Robot>),
    AllowAll,
    DisallowAll,
}

pub struct RobotsInfo {
    policy: Policy,
    pub sitemaps: Vec<String>,
    pub crawl_delay: Option<Duration>,
    /// Set when robots.txt could not be retrieved (network error / 5xx): we then refuse to crawl.
    pub unavailable: Option<String>,
}

impl RobotsInfo {
    pub fn allows(&self, url: &Url) -> bool {
        match &self.policy {
            Policy::AllowAll => true,
            Policy::DisallowAll => false,
            Policy::Rules(r) => r.allowed(url.as_str()),
        }
    }

    fn simple(policy: Policy) -> Self {
        Self { policy, sitemaps: Vec::new(), crawl_delay: None, unavailable: None }
    }

    fn unavailable(reason: String) -> Self {
        Self { unavailable: Some(reason), ..Self::simple(Policy::DisallowAll) }
    }
}

#[derive(Default)]
pub struct RobotsCache {
    map: Mutex<HashMap<String, Arc<OnceCell<Arc<RobotsInfo>>>>>,
}

impl RobotsCache {
    /// Fetch (once per origin, even under concurrency) and cache robots.txt.
    ///
    /// Following RFC 9309: 2xx -> parse; 4xx -> everything allowed;
    /// 5xx / unreachable -> everything disallowed (we must not guess).
    pub async fn get(&self, fetcher: &Fetcher, url: &Url) -> Arc<RobotsInfo> {
        let key = format!("{}://{}", url.scheme(), host_key(url));
        let cell = {
            let mut map = self.map.lock().expect("robots mutex poisoned");
            map.entry(key.clone()).or_default().clone()
        };
        cell.get_or_init(|| async { Arc::new(load(fetcher, url, &key).await) }).await.clone()
    }
}

async fn load(fetcher: &Fetcher, url: &Url, origin: &str) -> RobotsInfo {
    let robots_url = match Url::parse(&format!("{origin}/robots.txt")) {
        Ok(u) => u,
        Err(_) => return RobotsInfo::simple(Policy::AllowAll),
    };
    let cancel = CancellationToken::new();
    let _ = fetcher.limiter.acquire(&host_key(url), fetcher.cfg.per_domain_delay, &cancel).await;

    let resp =
        match fetcher.robots_client.get(robots_url.clone()).header(header::ACCEPT, "text/plain,*/*;q=0.1").send().await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(%robots_url, error = %e, "robots.txt unreachable; refusing to crawl");
                return RobotsInfo::unavailable(format!("{robots_url} is unreachable"));
            }
        };
    let status = resp.status();
    if status.is_client_error() {
        return RobotsInfo::simple(Policy::AllowAll);
    }
    if !status.is_success() {
        tracing::debug!(%robots_url, %status, "robots.txt server error; refusing to crawl");
        return RobotsInfo::unavailable(format!("{robots_url} returned HTTP {status}"));
    }

    let mut resp = resp;
    let mut buf = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => {
                buf.extend_from_slice(&c);
                if buf.len() >= MAX_ROBOTS_BYTES {
                    buf.truncate(MAX_ROBOTS_BYTES);
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => return RobotsInfo::unavailable(format!("{robots_url} could not be read")),
        }
    }
    match Robot::new(&fetcher.cfg.robots_agent, &buf) {
        Ok(r) => {
            let sitemaps = r.sitemaps.clone();
            let crawl_delay = r.delay.filter(|d| *d > 0.0).map(|d| Duration::from_secs_f32(d).min(MAX_CRAWL_DELAY));
            RobotsInfo { policy: Policy::Rules(Box::new(r)), sitemaps, crawl_delay, unavailable: None }
        }
        Err(_) => RobotsInfo::simple(Policy::AllowAll),
    }
}
