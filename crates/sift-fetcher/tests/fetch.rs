//! Fetcher behaviour against a local in-process HTTP server (no external network).

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use sift_fetcher::{FetchError, Fetcher, FetcherConfig};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

type Hits = Arc<Mutex<HashMap<String, usize>>>;

async fn handler(State(hits): State<Hits>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let n = {
        let mut h = hits.lock().unwrap();
        let e = h.entry(path.clone()).or_default();
        *e += 1;
        *e
    };
    match path.as_str() {
        "/robots.txt" => {
            "User-agent: *\nDisallow: /private\nCrawl-delay: 0\nSitemap: http://example.invalid/sitemap.xml\n"
                .into_response()
        }
        "/flaky" if n <= 2 => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        "/flaky" => "finally".into_response(),
        "/down" => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        "/a" => (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "/b")]).into_response(),
        "/b" => (StatusCode::FOUND, [(header::LOCATION, "/c")]).into_response(),
        "/c" => "end of chain".into_response(),
        "/loop" => (StatusCode::FOUND, [(header::LOCATION, "/loop")]).into_response(),
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(3)).await;
            "slow".into_response()
        }
        "/big" => "x".repeat(100_000).into_response(),
        "/img" => ([(header::CONTENT_TYPE, "image/png")], vec![0u8; 64]).into_response(),
        "/latin1" => ([(header::CONTENT_TYPE, "text/html; charset=iso-8859-1")], b"caf\xe9".to_vec()).into_response(),
        "/missing" => (StatusCode::NOT_FOUND, "nope").into_response(),
        _ => "ok".into_response(),
    }
}

async fn serve() -> (SocketAddr, Hits) {
    let hits: Hits = Arc::default();
    let app = Router::new().fallback(handler).with_state(hits.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, hits)
}

fn cfg() -> FetcherConfig {
    FetcherConfig {
        per_domain_delay: Duration::from_millis(1),
        retry_base_delay: Duration::from_millis(10),
        timeout: Duration::from_secs(5),
        ..FetcherConfig::default()
    }
}

fn url(addr: SocketAddr, path: &str) -> Url {
    Url::parse(&format!("http://{addr}{path}")).unwrap()
}

fn hits_for(h: &Hits, p: &str) -> usize {
    h.lock().unwrap().get(p).copied().unwrap_or(0)
}

#[tokio::test]
async fn fetches_text_and_records_metadata() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let p = f.fetch(&url(addr, "/hello"), &CancellationToken::new()).await.unwrap();
    assert_eq!(p.status, 200);
    assert_eq!(p.body.as_deref(), Some("ok"));
    assert!(p.is_success());
    assert_eq!(p.redirect_chain.len(), 1);
}

#[tokio::test]
async fn retries_transient_errors_with_backoff_then_succeeds() {
    let (addr, hits) = serve().await;
    let f = Fetcher::new(FetcherConfig { max_retries: 2, ..cfg() }).unwrap();
    let p = f.fetch(&url(addr, "/flaky"), &CancellationToken::new()).await.unwrap();
    assert_eq!(p.status, 200);
    assert_eq!(p.body.as_deref(), Some("finally"));
    assert_eq!(hits_for(&hits, "/flaky"), 3);
}

#[tokio::test]
async fn gives_up_after_max_retries_and_reports_status() {
    let (addr, hits) = serve().await;
    let f = Fetcher::new(FetcherConfig { max_retries: 2, ..cfg() }).unwrap();
    let p = f.fetch(&url(addr, "/down"), &CancellationToken::new()).await.unwrap();
    assert_eq!(p.status, 503);
    assert!(p.body.is_none());
    assert_eq!(hits_for(&hits, "/down"), 3);
}

#[tokio::test]
async fn does_not_retry_404() {
    let (addr, hits) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let p = f.fetch(&url(addr, "/missing"), &CancellationToken::new()).await.unwrap();
    assert_eq!(p.status, 404);
    assert_eq!(hits_for(&hits, "/missing"), 1);
}

#[tokio::test]
async fn records_redirect_chain() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let p = f.fetch(&url(addr, "/a"), &CancellationToken::new()).await.unwrap();
    assert_eq!(p.final_url.path(), "/c");
    assert_eq!(p.redirect_chain.len(), 3);
    assert!(p.redirect_chain[0].ends_with("/a") && p.redirect_chain[2].ends_with("/c"));
}

#[tokio::test]
async fn redirect_loops_are_bounded() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(FetcherConfig { max_redirects: 3, ..cfg() }).unwrap();
    let err = f.fetch(&url(addr, "/loop"), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, FetchError::TooManyRedirects));
}

#[tokio::test]
async fn robots_txt_is_enforced_and_cached() {
    let (addr, hits) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let cancel = CancellationToken::new();
    let err = f.fetch(&url(addr, "/private/page"), &cancel).await.unwrap_err();
    assert!(err.is_robots(), "{err}");
    assert_eq!(hits_for(&hits, "/private/page"), 0, "disallowed URL must never be requested");
    f.fetch(&url(addr, "/public"), &cancel).await.unwrap();
    f.fetch(&url(addr, "/public2"), &cancel).await.unwrap();
    assert_eq!(hits_for(&hits, "/robots.txt"), 1, "robots.txt is fetched once per origin");
    assert_eq!(f.robots_sitemaps(&url(addr, "/")).await, vec!["http://example.invalid/sitemap.xml"]);
}

#[tokio::test]
async fn concurrent_requests_fetch_robots_once() {
    let (addr, hits) = serve().await;
    let f = Arc::new(Fetcher::new(cfg()).unwrap());
    let mut tasks = Vec::new();
    for i in 0..8 {
        let f = f.clone();
        tasks.push(tokio::spawn(async move {
            f.fetch(&url(addr, &format!("/p{i}")), &CancellationToken::new()).await.unwrap().status
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), 200);
    }
    assert_eq!(hits_for(&hits, "/robots.txt"), 1);
}

#[tokio::test]
async fn times_out_slow_responses() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(FetcherConfig { timeout: Duration::from_millis(300), max_retries: 0, ..cfg() }).unwrap();
    let err = f.fetch(&url(addr, "/slow"), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, FetchError::Timeout), "{err}");
}

#[tokio::test]
async fn caps_body_size() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(FetcherConfig { max_body_bytes: 1000, ..cfg() }).unwrap();
    let p = f.fetch(&url(addr, "/big"), &CancellationToken::new()).await.unwrap();
    assert!(p.truncated);
    assert_eq!(p.body_bytes, 1000);
    assert_eq!(p.body.unwrap().len(), 1000);
}

#[tokio::test]
async fn skips_binary_bodies_and_decodes_charsets() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let c = CancellationToken::new();
    let img = f.fetch(&url(addr, "/img"), &c).await.unwrap();
    assert_eq!(img.content_type.as_deref(), Some("image/png"));
    assert!(img.body.is_none());
    let l1 = f.fetch(&url(addr, "/latin1"), &c).await.unwrap();
    assert_eq!(l1.body.as_deref(), Some("café"));
}

#[tokio::test]
async fn private_hosts_are_refused_when_blocking() {
    let (addr, hits) = serve().await;
    let f = Fetcher::new(FetcherConfig { block_private_hosts: true, ..cfg() }).unwrap();
    let err = f.fetch(&url(addr, "/x"), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, FetchError::BlockedHost(_)));
    assert!(hits.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_stops_a_pending_fetch() {
    let (addr, _) = serve().await;
    let f = Fetcher::new(cfg()).unwrap();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        c2.cancel();
    });
    let start = std::time::Instant::now();
    let err = f.fetch(&url(addr, "/slow"), &cancel).await.unwrap_err();
    assert!(matches!(err, FetchError::Cancelled));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn unreachable_robots_means_no_crawling() {
    // Port 9 (discard) on localhost is almost certainly closed -> connection refused.
    let f = Fetcher::new(FetcherConfig { max_retries: 0, ..cfg() }).unwrap();
    let u = Url::parse("http://127.0.0.1:9/page").unwrap();
    let err = f.fetch(&u, &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, FetchError::RobotsUnavailable(_)), "{err}");
    assert!(!err.is_robots(), "an unreachable robots.txt is not the same as a disallow");
}
