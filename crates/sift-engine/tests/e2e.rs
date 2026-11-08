//! End-to-end pipeline tests against local websites. No external network access.

use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Router;
use sift_core::{ExtractionSpec, ResultSet};
use sift_core::{FieldKind as F, RequestInterpreter, RuleBasedInterpreter, Validation};
use sift_engine::{parse_seeds, Engine, EngineError, RecordScope, RunOptions, SearxngProvider};
use sift_fetcher::FetcherConfig;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

type Hits = Arc<Mutex<Vec<String>>>;

const AGENCY_HOME: &str = r#"<html><head><title>Himalaya Soft | Software Agency</title>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"ProfessionalService",
"name":"Himalaya Soft Pvt. Ltd.","telephone":"+977-1-4412345",
"address":{"@type":"PostalAddress","streetAddress":"Thamel Marg 12","addressLocality":"Kathmandu","addressCountry":"NP"}}</script>
</head><body><h1>We build software</h1>
<a href="/contact">Contact</a> <a href="/about">About us</a> <a href="/blog/hello">Blog</a> <a href="/secret/x">Secret</a>
<footer><a href="https://www.instagram.com/himalayasoft/">IG</a> <a href="https://facebook.com/himalayasoft">FB</a>
<a href="https://twitter.com/intent/tweet?text=share">share</a></footer></body></html>"#;

const AGENCY_CONTACT: &str = r#"<html><head><title>Contact</title></head><body>
<a href="mailto:Hello@HimalayaSoft.com.np">Email us</a>
<p>Email: hello@himalayasoft.com.np</p>
<p>Phone: +977 1 4412345</p><a href="tel:+977-1-4412345">call</a>
<p>Fax: +977 1 4412346</p>
<a href="https://wa.me/9779801234567">WhatsApp</a>
<address>Himalaya Soft, Thamel Marg 12, Kathmandu</address>
<p>Image credit: logo@2x.png and placeholder user@example.com</p>
<form action="/contact/send" method="post"><input type="email" name="email"><textarea name="message"></textarea></form>
</body></html>"#;

async fn agency(State(hits): State<Hits>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    hits.lock().unwrap().push(path.clone());
    match path.as_str() {
        "/" => Html(AGENCY_HOME).into_response(),
        "/contact" => Html(AGENCY_CONTACT).into_response(),
        "/about" => Html(
            r#"<html><head><meta name="description" content="Custom software studio."></head><body>
            <div class="team-member"><h3>Sita Rai</h3><span class="job-title">CTO</span></div></body></html>"#,
        )
        .into_response(),
        "/blog/hello" => Html("<p>blog@himalayasoft.com.np</p>").into_response(),
        "/secret/x" => Html("<p>leak@himalayasoft.com.np</p>").into_response(),
        "/robots.txt" => "User-agent: *\nDisallow: /secret\n".into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn startup(State(hits): State<Hits>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    hits.lock().unwrap().push(path.clone());
    match path.as_str() {
        "/" => Html(
            r#"<html><head><title>Everest Apps</title></head><body><a href="mailto:team@everestapps.io">mail</a>
                       <a href="/contact">Contact</a></body></html>"#,
        )
        .into_response(),
        "/contact" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve(router: Router<Hits>) -> (SocketAddr, Hits) {
    let hits: Hits = Arc::default();
    let app = router.with_state(hits.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, hits)
}

async fn agency_site() -> (SocketAddr, Hits) {
    serve(Router::new().fallback(agency)).await
}
async fn startup_site() -> (SocketAddr, Hits) {
    serve(Router::new().fallback(startup)).await
}

fn engine() -> Engine {
    Engine::new(FetcherConfig {
        per_domain_delay: Duration::from_millis(1),
        retry_base_delay: Duration::from_millis(5),
        max_retries: 1,
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .unwrap()
}

async fn run(seeds: &[String], spec: ExtractionSpec, opts: RunOptions) -> ResultSet {
    engine().run(parse_seeds(seeds).unwrap(), spec, opts, CancellationToken::new(), None).await
}

fn spec(fields: &[F]) -> ExtractionSpec {
    ExtractionSpec::new(fields.to_vec())
}

fn values(rs: &ResultSet, idx: usize, f: F) -> Vec<String> {
    rs.results[idx].values(f).map(str::to_string).collect()
}

#[tokio::test]
async fn scrape_one_agency_end_to_end() {
    let (addr, hits) = agency_site().await;
    let fields = [
        F::CompanyName,
        F::Website,
        F::Email,
        F::Phone,
        F::Fax,
        F::Whatsapp,
        F::Address,
        F::Instagram,
        F::Facebook,
        F::Twitter,
        F::ContactForm,
        F::Team,
    ];
    let rs = run(&[format!("http://{addr}")], spec(&fields), RunOptions::default()).await;

    assert_eq!(rs.results.len(), 1);
    let rec = &rs.results[0];
    assert_eq!(rec.website, format!("http://{addr}"));
    assert_eq!(values(&rs, 0, F::Website), vec![format!("http://{addr}")]);

    // Name: structured data wins over the title heuristic.
    assert_eq!(values(&rs, 0, F::CompanyName)[0], "Himalaya Soft Pvt. Ltd.");
    assert_eq!(rec.fields[&F::CompanyName][0].status, Validation::Valid);

    // Email: mailto + text + case variants collapse to one validated address; junk is discarded.
    assert_eq!(values(&rs, 0, F::Email), vec!["hello@himalayasoft.com.np"]);
    let email = &rec.fields[&F::Email][0];
    assert_eq!(email.status, Validation::Valid);
    assert_eq!(email.source, format!("http://{addr}/contact"));

    // Phone: JSON-LD, tel: link and plain text are the same number; the fax is not a phone.
    assert_eq!(values(&rs, 0, F::Phone), vec!["+97714412345"]);
    assert_eq!(values(&rs, 0, F::Fax), vec!["+97714412346"]);
    assert_eq!(values(&rs, 0, F::Whatsapp), vec!["https://wa.me/9779801234567"]);

    // Address: structured JSON-LD ranks above the <address> heuristic.
    let addr_vals = values(&rs, 0, F::Address);
    assert_eq!(addr_vals[0], "Thamel Marg 12, Kathmandu, NP");
    assert!(addr_vals.len() >= 2);
    assert_eq!(rec.fields[&F::Address][0].status, Validation::Valid);
    assert_eq!(rec.fields[&F::Address][1].status, Validation::Unverified);

    assert_eq!(values(&rs, 0, F::Instagram), vec!["https://www.instagram.com/himalayasoft"]);
    assert_eq!(values(&rs, 0, F::Facebook), vec!["https://www.facebook.com/himalayasoft"]);
    assert!(values(&rs, 0, F::Twitter).is_empty(), "a share intent is not a profile");
    assert_eq!(values(&rs, 0, F::ContactForm), vec![format!("http://{addr}/contact/send")]);
    assert_eq!(values(&rs, 0, F::Team), vec!["Sita Rai — CTO"]);

    // Bookkeeping.
    let m = &rs.metadata;
    assert_eq!(m.pages_crawled, 3, "/, /contact, /about only");
    // 1 duplicate email (mailto + text) + 2 duplicate phones (JSON-LD, tel: link, text).
    assert_eq!(m.duplicates_removed, 3);
    assert_eq!(m.invalid_values_discarded, 2, "logo@2x.png and user@example.com");
    assert_eq!(m.records_found, 1);
    // Never fetched: irrelevant blog page and the robots-disallowed path.
    let h = hits.lock().unwrap();
    assert!(!h.contains(&"/blog/hello".to_string()) && !h.contains(&"/secret/x".to_string()), "{h:?}");
}

#[tokio::test]
async fn no_value_is_ever_invented() {
    let (addr, _) = startup_site().await;
    let rs = run(
        &[format!("http://{addr}")],
        spec(&[F::Email, F::Phone, F::Address, F::Instagram, F::Fax]),
        RunOptions::default(),
    )
    .await;
    assert_eq!(rs.results.len(), 1);
    assert_eq!(values(&rs, 0, F::Email), vec!["team@everestapps.io"]);
    for f in [F::Phone, F::Address, F::Instagram, F::Fax] {
        assert!(rs.results[0].fields[&f].is_empty(), "{f} must stay missing");
    }
    // The 500 on /contact shows up as a failure, not as data.
    assert!(rs.metadata.pages_failed >= 1);
    assert!(rs.metadata.errors.iter().any(|e| e.contains("/contact") && e.contains("500")), "{:?}", rs.metadata.errors);
}

#[tokio::test]
async fn multiple_sites_duplicate_seeds_and_sorted_output() {
    let (a, _) = agency_site().await;
    let (b, _) = startup_site().await;
    let seeds = [format!("http://{b}/"), format!("http://{a}"), format!("http://{a}/")];
    assert_eq!(parse_seeds(&seeds).unwrap().len(), 2, "duplicate seeds collapse");
    let rs = run(&seeds, spec(&[F::CompanyName, F::Email]), RunOptions::default()).await;
    assert_eq!(rs.results.len(), 2);
    let sites: Vec<_> = rs.results.iter().map(|r| r.website.clone()).collect();
    let mut sorted = sites.clone();
    sorted.sort();
    assert_eq!(sites, sorted);
    assert!(rs.results.iter().all(|r| !r.fields[&F::Email].is_empty()));
}

#[tokio::test]
async fn validation_can_be_disabled_and_sources_hidden() {
    let (addr, _) = agency_site().await;
    let mut s = spec(&[F::Email]);
    s.validate = false;
    s.include_source = false;
    let rs = run(&[format!("http://{addr}")], s, RunOptions::default()).await;
    let emails = values(&rs, 0, F::Email);
    assert!(emails.contains(&"logo@2x.png".to_string()), "{emails:?}");
    assert!(rs.results[0].fields[&F::Email].iter().all(|e| e.status == Validation::Unverified));
    let json = sift_core::output::to_json(&rs).unwrap();
    assert!(!json.contains("\"source\""), "sources must be omitted");
    assert_eq!(rs.metadata.invalid_values_discarded, 0);
}

#[tokio::test]
async fn page_scope_yields_one_record_per_page() {
    let (addr, _) = agency_site().await;
    let opts = RunOptions { scope: RecordScope::Page, relevant_only: false, ..Default::default() };
    let rs =
        run(&[format!("http://{addr}")], spec(&[F::Title, F::StatusCode, F::ResponseTime, F::Headings]), opts).await;
    let sites: Vec<_> = rs.results.iter().map(|r| r.website.as_str()).collect();
    assert!(
        sites.contains(&format!("http://{addr}").as_str()) && sites.iter().any(|s| s.ends_with("/contact")),
        "{sites:?}"
    );
    assert!(sites.iter().any(|s| s.ends_with("/blog/hello")), "full crawl visits the blog");
    assert!(!sites.iter().any(|s| s.contains("/secret")), "robots.txt still applies");
    for r in &rs.results {
        assert_eq!(r.pages_visited, 1);
        assert_eq!(r.fields[&F::StatusCode][0].value, "200");
        assert!(r.fields[&F::ResponseTime][0].value.ends_with(" ms"));
    }
    assert_eq!(rs.metadata.pages_blocked_by_robots, 1);
}

#[tokio::test]
async fn unreachable_seed_is_reported_not_fabricated() {
    let rs = run(&["http://127.0.0.1:9/".to_string()], spec(&[F::Email]), RunOptions::default()).await;
    assert!(rs.results.is_empty());
    assert!(!rs.metadata.errors.is_empty());
    assert_eq!(rs.metadata.records_found, 0);
}

#[tokio::test]
async fn cancellation_returns_partial_results() {
    let (addr, _) = agency_site().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let rs = engine()
        .run(parse_seeds(&[format!("http://{addr}")]).unwrap(), spec(&[F::Email]), RunOptions::default(), cancel, None)
        .await;
    assert!(rs.metadata.cancelled);
    assert!(rs.results.is_empty());
}

#[tokio::test]
async fn natural_language_request_drives_the_engine() {
    let (addr, _) = agency_site().await;
    let spec = RuleBasedInterpreter.interpret("Get the email, phone number and Instagram account of every agency.");
    assert_eq!(spec.fields, vec![F::Email, F::Phone, F::Instagram]);
    let rs = run(&[format!("http://{addr}")], spec, RunOptions::default()).await;
    assert_eq!(rs.query.as_deref(), Some("Get the email, phone number and Instagram account of every agency."));
    let rec = &rs.results[0];
    assert_eq!(rec.fields.keys().copied().collect::<Vec<_>>(), vec![F::Email, F::Phone, F::Instagram]);
    assert_eq!(values(&rs, 0, F::Instagram).len(), 1);
}

#[tokio::test]
async fn output_formats_round_trip() {
    let (addr, _) = agency_site().await;
    let rs = run(&[format!("http://{addr}")], spec(&[F::CompanyName, F::Email, F::Phone]), RunOptions::default()).await;
    let json = sift_core::output::to_json(&rs).unwrap();
    let back: ResultSet = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rs, "JSON output deserialises back to the same data");
    let csv = sift_core::output::to_csv(&rs).unwrap();
    assert!(csv.starts_with("website,company_name,company_name_source,email,email_source,phone,phone_source\n"));
    assert!(csv.contains("hello@himalayasoft.com.np"));
    let table = sift_core::output::to_table(&rs, Default::default());
    assert!(table.contains("Himalaya Soft Pvt. Ltd.") && table.contains("Summary:"));
}

// ---------------------------------------------------------------- discovery

async fn searx(
    Query(q): Query<HashMap<String, String>>,
    State(urls): State<Arc<Vec<String>>>,
) -> Json<serde_json::Value> {
    assert_eq!(q.get("format").map(String::as_str), Some("json"));
    assert_eq!(q.get("q").map(String::as_str), Some("software agencies Kathmandu"));
    Json(
        serde_json::json!({ "results": urls.iter().map(|u| serde_json::json!({"url": u, "title": "x"})).collect::<Vec<_>>() }),
    )
}

#[tokio::test]
async fn search_discovers_sites_then_scrapes_them() {
    let (a, _) = agency_site().await;
    let (b, _) = startup_site().await;
    let urls = Arc::new(vec![
        format!("http://{a}/services/web-development"),
        "https://www.facebook.com/someagency".to_string(),
        format!("http://{b}/contact"),
        format!("http://{a}/about"),
    ]);
    let app = Router::new().route("/search", axum::routing::get(searx)).with_state(urls);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s_addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let spec = RuleBasedInterpreter.interpret("Find software agencies in Kathmandu. Collect their emails.");
    let q = sift_engine::build_search_query(&spec).unwrap();
    let provider = SearxngProvider::new(&format!("http://{s_addr}/"), "Sift-test").unwrap();
    let seeds = provider.search(&q, 10).await.unwrap();
    assert_eq!(seeds.len(), 2, "facebook skipped, duplicate host collapsed: {seeds:?}");

    let rs = engine().run(seeds, spec, RunOptions::default(), CancellationToken::new(), None).await;
    assert_eq!(rs.results.len(), 2);
    assert_eq!(rs.spec.location.as_deref(), Some("Kathmandu"));
}

#[tokio::test]
async fn search_errors_are_helpful() {
    let err = SearxngProvider::new("not a url", "x").err().unwrap();
    assert!(matches!(err, EngineError::Search(_)));
    let p = SearxngProvider::new("http://127.0.0.1:9/", "x").unwrap();
    assert!(p.search("q", 3).await.unwrap_err().to_string().contains("search request failed"));
    assert!(matches!(parse_seeds::<&str>(&[]), Err(EngineError::NoSeeds)));
    assert!(matches!(parse_seeds(&["ftp://x.org"]), Err(EngineError::InvalidSeed(_))));
}
