//! Crawler behaviour against a local in-process website.

use axum::extract::{Request, State};
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use sift_core::FieldKind;
use sift_crawler::{CrawlConfig, CrawlOutput, Crawler};
use sift_fetcher::{Fetcher, FetcherConfig};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Default)]
struct Site {
    hits: Mutex<Vec<String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

async fn handler(State(site): State<Arc<Site>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    site.hits.lock().unwrap().push(path.clone());
    let page = |body: &str| {
        Html(format!("<html><head><title>{path}</title></head><body>{body}</body></html>")).into_response()
    };
    match path.as_str() {
        "/" => page(
            r#"<a href="/blog/1">Blog</a> <a href="/products">Products</a> <a href="/contact">Contact</a>
                      <a href="/contact/">again</a> <a href="/contact#map">again</a> <a href="/contact?utm_source=x">again</a>
                      <a href="/about">About</a> <a href="/secret/plans">Plans</a> <a href="/files/brochure.pdf">pdf</a>
                      <a href="https://other.invalid/contact">elsewhere</a>"#,
        ),
        "/contact" => page(r#"<a href="mailto:hello@acme.np">mail</a> <a href="/team">Our team</a>"#),
        "/about" | "/team" | "/products" | "/blog/1" | "/hidden-contact" | "/unrelated" => page("plain"),
        "/robots.txt" => "User-agent: *\nDisallow: /secret\nSitemap: SITEMAP\n".into_response(),
        "/sitemap.xml" => {
            let host = req.headers().get("host").unwrap().to_str().unwrap().to_string();
            format!(
                "<urlset><url><loc>http://{host}/hidden-contact</loc></url><url><loc>http://{host}/unrelated</loc></url><url><loc>http://elsewhere.invalid/contact</loc></url></urlset>"
            )
            .into_response()
        }
        "/slow-index" => page(&(1..=8).map(|i| format!(r#"<a href="/slow/{i}">s{i}</a>"#)).collect::<String>()),
        p if p.starts_with("/slow/") => {
            let now = site.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            site.max_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(150)).await;
            site.in_flight.fetch_sub(1, Ordering::SeqCst);
            page("slow")
        }
        p if p.starts_with("/n/") => {
            let i: usize = p[3..].parse().unwrap_or(0);
            page(&format!(r#"<a href="/n/{}">a</a> <a href="/n/{}">b</a>"#, i * 2 + 1, i * 2 + 2))
        }
        p if p.starts_with("/d/") => {
            let i: usize = p[3..].parse().unwrap_or(0);
            page(&format!(r#"<a href="/d/{}">next</a>"#, i + 1))
        }
        p if p.starts_with("/endless/") => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let i: usize = p[9..].parse().unwrap_or(0);
            page(&format!(r#"<a href="/endless/{}">next</a>"#, i + 1))
        }
        _ => (axum::http::StatusCode::NOT_FOUND, "nf").into_response(),
    }
}

async fn serve() -> (SocketAddr, Arc<Site>) {
    let site = Arc::new(Site::default());
    let app = Router::new().fallback(handler).with_state(site.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, site)
}

fn fetcher() -> Arc<Fetcher> {
    Arc::new(
        Fetcher::new(FetcherConfig {
            per_domain_delay: Duration::from_millis(1),
            retry_base_delay: Duration::from_millis(5),
            max_retries: 0,
            timeout: Duration::from_secs(5),
            ..Default::default()
        })
        .unwrap(),
    )
}

fn cfg(fields: &[FieldKind]) -> CrawlConfig {
    CrawlConfig { fields: fields.to_vec(), concurrency: 4, ..Default::default() }
}

async fn crawl(addr: SocketAddr, start: &str, cfg: CrawlConfig) -> CrawlOutput {
    let url = Url::parse(&format!("http://{addr}{start}")).unwrap();
    Crawler::new(fetcher(), cfg).run(vec![url], CancellationToken::new(), None).await
}

fn paths(out: &CrawlOutput) -> Vec<String> {
    let mut p: Vec<String> = out.pages.iter().map(|p| p.url.path().to_string()).collect();
    p.sort();
    p
}

fn hit_count(site: &Site, path: &str) -> usize {
    site.hits.lock().unwrap().iter().filter(|p| *p == path).count()
}

#[tokio::test]
async fn relevant_only_visits_contact_like_pages_and_sitemap_hits() {
    let (addr, site) = serve().await;
    let out = crawl(addr, "/", cfg(&[FieldKind::Email])).await;
    let p = paths(&out);
    for want in ["/", "/contact", "/about", "/team", "/hidden-contact"] {
        assert!(p.contains(&want.to_string()), "{want} missing from {p:?}");
    }
    for unwanted in ["/blog/1", "/products", "/unrelated", "/files/brochure.pdf"] {
        assert_eq!(hit_count(&site, unwanted), 0, "{unwanted} should not have been fetched");
    }
    assert!(out.stats.irrelevant_skipped >= 2);
    assert_eq!(out.sitemaps.len(), 1);
    assert!(out.sitemaps[0].url.ends_with("/sitemap.xml"));
}

#[tokio::test]
async fn robots_disallowed_pages_are_never_requested() {
    let (addr, site) = serve().await;
    let out = crawl(addr, "/", CrawlConfig { relevant_only: false, ..cfg(&[FieldKind::Title]) }).await;
    assert_eq!(hit_count(&site, "/secret/plans"), 0);
    assert_eq!(out.stats.blocked_by_robots, 1);
    assert!(paths(&out).contains(&"/blog/1".to_string()), "full crawl follows non-relevant links");
}

#[tokio::test]
async fn duplicate_urls_and_off_site_links_are_skipped() {
    let (addr, site) = serve().await;
    let out = crawl(addr, "/", cfg(&[FieldKind::Email])).await;
    assert_eq!(hit_count(&site, "/contact"), 1, "/contact, /contact/, #frag and utm variants are one URL");
    assert!(out.stats.duplicates_skipped >= 1, "the trailing-slash variant is caught by the crawler");
    assert!(out.pages.iter().all(|p| p.url.host_str() == Some("127.0.0.1")));
}

#[tokio::test]
async fn max_pages_is_a_hard_limit() {
    let (addr, site) = serve().await;
    let out = crawl(
        addr,
        "/n/0",
        CrawlConfig {
            relevant_only: false,
            max_depth: 20,
            max_pages: 7,
            use_sitemaps: false,
            ..cfg(&[FieldKind::Title])
        },
    )
    .await;
    assert_eq!(out.pages.len(), 7);
    assert_eq!(site.hits.lock().unwrap().iter().filter(|p| p.starts_with("/n/")).count(), 7);
}

#[tokio::test]
async fn max_depth_is_respected() {
    let (addr, _) = serve().await;
    let out = crawl(
        addr,
        "/d/0",
        CrawlConfig { relevant_only: false, max_depth: 2, use_sitemaps: false, ..cfg(&[FieldKind::Title]) },
    )
    .await;
    assert_eq!(paths(&out), vec!["/d/0", "/d/1", "/d/2"]);
    let out = crawl(
        addr,
        "/d/0",
        CrawlConfig { relevant_only: false, max_depth: 0, use_sitemaps: false, ..cfg(&[FieldKind::Title]) },
    )
    .await;
    assert_eq!(paths(&out), vec!["/d/0"]);
}

#[tokio::test]
async fn fetches_concurrently_up_to_the_limit() {
    let (addr, site) = serve().await;
    let started = Instant::now();
    let out = crawl(
        addr,
        "/slow-index",
        CrawlConfig {
            relevant_only: false,
            max_depth: 1,
            use_sitemaps: false,
            concurrency: 4,
            ..cfg(&[FieldKind::Title])
        },
    )
    .await;
    assert_eq!(out.pages.len(), 9);
    let max = site.max_in_flight.load(Ordering::SeqCst);
    assert!((3..=4).contains(&max), "max in flight was {max}");
    assert!(
        started.elapsed() < Duration::from_millis(8 * 150),
        "should be faster than sequential: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn higher_scored_links_are_fetched_first() {
    let (addr, site) = serve().await;
    crawl(
        addr,
        "/",
        CrawlConfig {
            relevant_only: false,
            concurrency: 1,
            max_depth: 1,
            use_sitemaps: false,
            ..cfg(&[FieldKind::Email])
        },
    )
    .await;
    let hits = site.hits.lock().unwrap().clone();
    let pos = |p: &str| hits.iter().position(|h| h == p).unwrap_or(usize::MAX);
    assert!(pos("/contact") < pos("/products"), "{hits:?}");
    assert!(pos("/about") < pos("/blog/1"), "{hits:?}");
    assert!(pos("/products") < pos("/blog/1"), "blog is pushed to the back: {hits:?}");
}

#[tokio::test]
async fn cancellation_stops_the_crawl_gracefully() {
    let (addr, _) = serve().await;
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        c2.cancel();
    });
    let url = Url::parse(&format!("http://{addr}/endless/0")).unwrap();
    let started = Instant::now();
    let out = Crawler::new(
        fetcher(),
        CrawlConfig {
            relevant_only: false,
            max_depth: 1000,
            max_pages: 1000,
            use_sitemaps: false,
            ..cfg(&[FieldKind::Title])
        },
    )
    .run(vec![url], cancel, None)
    .await;
    assert!(out.cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!out.pages.is_empty() && out.pages.len() < 20, "got {} pages", out.pages.len());
}

#[tokio::test]
async fn progress_callback_reports_stats() {
    let (addr, _) = serve().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let url = Url::parse(&format!("http://{addr}/")).unwrap();
    let out = Crawler::new(fetcher(), cfg(&[FieldKind::Email]))
        .run(
            vec![url],
            CancellationToken::new(),
            Some(Arc::new(move |s| {
                c2.fetch_add(1, Ordering::SeqCst);
                assert!(s.pages_fetched <= 50);
            })),
        )
        .await;
    assert!(calls.load(Ordering::SeqCst) >= out.pages.len());
    assert!(out.stats.bytes_downloaded > 0);
    assert_eq!(out.stats.pages_fetched, out.pages.len());
}
