//! API behaviour, exercised in-process against a local target website.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use sift_api::{router, ApiConfig, Limits};
use sift_fetcher::FetcherConfig;
use std::net::SocketAddr;
use std::time::Duration;
use tower::ServiceExt;

async fn target() -> SocketAddr {
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                Html(
                    r#"<html><head><title>Acme Studio | Home</title></head><body>
                <a href="mailto:hello@acme-studio.com.np">mail</a> <a href="/contact">Contact</a></body></html>"#,
                )
            }),
        )
        .route("/contact", get(|| async { Html(r#"<p>Phone: +977 1 4412345</p>"#) }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                "slow".into_response()
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

fn test_app(limits: Limits) -> Router {
    router(ApiConfig {
        fetcher: FetcherConfig {
            per_domain_delay: Duration::from_millis(1),
            max_retries: 0,
            timeout: Duration::from_secs(10),
            block_private_hosts: false,
            ..Default::default()
        },
        limits,
    })
    .unwrap()
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, json)
}

async fn wait_for(app: &Router, id: &str, status: &str) -> Value {
    for _ in 0..200 {
        let (code, job) = call(app, Method::GET, &format!("/api/v1/jobs/{id}"), None).await;
        assert_eq!(code, StatusCode::OK);
        if job["status"] == status {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} never reached {status}");
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn health_and_docs() {
    let app = test_app(Limits::default());
    let (s, v) = call(&app, Method::GET, "/api/v1/health", None).await;
    assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("ok")));
    let (s, v) = call(&app, Method::GET, "/api/v1/docs", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.as_str().unwrap().contains("# Sift HTTP API"));
}

#[tokio::test]
async fn scrape_job_lifecycle() {
    let addr = target().await;
    let app = test_app(Limits::default());
    let (s, v) = call(
        &app,
        Method::POST,
        "/api/v1/scrape",
        Some(json!({ "urls": [format!("http://{addr}")], "fields": ["company_name", "email", "phone", "fax"] })),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["links"]["results"], format!("/api/v1/jobs/{id}/results"));

    let job = wait_for(&app, &id, "completed").await;
    assert_eq!(job["kind"], "scrape");
    assert!(job["stats"]["pages_ok"].as_u64().unwrap() >= 2);
    assert!(job["finished_at"].is_u64());

    let (s, rs) = call(&app, Method::GET, &format!("/api/v1/jobs/{id}/results"), None).await;
    assert_eq!(s, StatusCode::OK);
    let rec = &rs["results"][0];
    assert_eq!(rec["fields"]["email"][0]["value"], "hello@acme-studio.com.np");
    assert_eq!(rec["fields"]["email"][0]["status"], "valid");
    assert_eq!(rec["fields"]["phone"][0]["value"], "+97714412345");
    assert_eq!(rec["fields"]["phone"][0]["status"], "unverified");
    assert_eq!(rec["fields"]["fax"], json!([]), "missing stays empty");
    assert_eq!(rs["metadata"]["records_found"], 1);

    let (s, _) = call(&app, Method::DELETE, &format!("/api/v1/jobs/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, Method::GET, &format!("/api/v1/jobs/{id}"), None).await;
    assert_eq!((s, code(&v)), (StatusCode::NOT_FOUND, "job_not_found"));
}

#[tokio::test]
async fn natural_language_query_and_crawl_scope() {
    let addr = target().await;
    let app = test_app(Limits::default());
    let (s, v) = call(
        &app,
        Method::POST,
        "/api/v1/crawl",
        Some(json!({ "urls": [format!("http://{addr}")], "query": "page titles", "depth": 1 })),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_str().unwrap();
    wait_for(&app, id, "completed").await;
    let (_, rs) = call(&app, Method::GET, &format!("/api/v1/jobs/{id}/results"), None).await;
    assert_eq!(rs["spec"]["fields"], json!(["title"]));
    assert_eq!(rs["query"], "page titles");
    assert_eq!(rs["results"].as_array().unwrap().len(), 2, "crawl = one record per page");
}

#[tokio::test]
async fn extract_is_synchronous_and_single_page() {
    let addr = target().await;
    let app = test_app(Limits::default());
    let (s, rs) = call(
        &app,
        Method::POST,
        "/api/v1/extract",
        Some(json!({ "url": format!("http://{addr}/"), "fields": ["email", "title"], "include_source": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{rs}");
    assert_eq!(rs["metadata"]["pages_crawled"], 1);
    assert_eq!(rs["results"][0]["fields"]["title"][0]["value"], "Acme Studio | Home");
    assert!(rs["results"][0]["fields"]["email"][0].get("source").is_none());
}

#[tokio::test]
async fn invalid_requests_are_rejected_with_clear_errors() {
    let app = test_app(Limits::default());
    let post = |body: Value| {
        let app = app.clone();
        async move { call(&app, Method::POST, "/api/v1/scrape", Some(body)).await }
    };
    for (body, needle) in [
        (json!({ "urls": [], "fields": ["email"] }), "urls"),
        (json!({ "urls": ["ftp://x.org"], "fields": ["email"] }), "not a valid http(s) URL"),
        (json!({ "urls": ["https://x.org"] }), "fields"),
        (json!({ "urls": ["https://x.org"], "fields": ["bogus"] }), "unknown field `bogus`"),
        (json!({ "urls": ["https://x.org"], "fields": ["email"], "max_pages": 100000 }), "max_pages"),
        (json!({ "urls": ["https://x.org"], "fields": ["email"], "depth": 99 }), "depth"),
        (json!({ "urls": ["https://x.org"], "fields": ["email"], "concurrency": 0 }), "concurrency"),
        (json!({ "urls": ["https://x.org"], "fields": ["email"], "nope": 1 }), "unknown field"),
        (
            json!({ "urls": (0..21).map(|i| format!("https://s{i}.org")).collect::<Vec<_>>(), "fields": ["email"] }),
            "urls",
        ),
    ] {
        let (s, v) = post(body.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(code(&v), "invalid_request");
        assert!(v["error"]["message"].as_str().unwrap().contains(needle), "{v} should mention {needle}");
    }
    let req = Request::builder().method(Method::POST).uri("/api/v1/scrape").body(Body::from("{not json")).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn private_targets_are_refused_by_default() {
    let app = router(ApiConfig::default()).unwrap();
    for url in [
        "http://127.0.0.1:8080/",
        "http://localhost/",
        "http://169.254.169.254/latest/meta-data",
        "http://10.0.0.1/",
        "http://[::1]/",
    ] {
        let (s, v) =
            call(&app, Method::POST, "/api/v1/scrape", Some(json!({ "urls": [url], "fields": ["email"] }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{url}");
        assert!(v["error"]["message"].as_str().unwrap().contains("private or loopback"), "{v}");
    }
    let (s, _) =
        call(&app, Method::POST, "/api/v1/extract", Some(json!({ "url": "http://127.0.0.1/", "fields": ["email"] })))
            .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn results_before_completion_conflict_and_delete_cancels() {
    let addr = target().await;
    let app = test_app(Limits::default());
    let (_, v) = call(
        &app,
        Method::POST,
        "/api/v1/scrape",
        Some(json!({ "urls": [format!("http://{addr}/slow")], "fields": ["title"] })),
    )
    .await;
    let id = v["id"].as_str().unwrap();
    let (s, v) = call(&app, Method::GET, &format!("/api/v1/jobs/{id}/results"), None).await;
    assert_eq!((s, code(&v)), (StatusCode::CONFLICT, "job_not_finished"));
    let (_, job) = call(&app, Method::GET, &format!("/api/v1/jobs/{id}"), None).await;
    assert_eq!(job["status"], "running");
    let (s, _) = call(&app, Method::DELETE, &format!("/api/v1/jobs/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn running_job_limit_returns_429() {
    let addr = target().await;
    let app = test_app(Limits { max_running_jobs: 1, ..Limits::default() });
    let body = json!({ "urls": [format!("http://{addr}/slow")], "fields": ["title"] });
    let (s, first) = call(&app, Method::POST, "/api/v1/scrape", Some(body.clone())).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, v) = call(&app, Method::POST, "/api/v1/scrape", Some(body)).await;
    assert_eq!((s, code(&v)), (StatusCode::TOO_MANY_REQUESTS, "too_many_jobs"));
    call(&app, Method::DELETE, &format!("/api/v1/jobs/{}", first["id"].as_str().unwrap()), None).await;
}

#[tokio::test]
async fn unknown_job_ids_are_404() {
    let app = test_app(Limits::default());
    for uri in ["/api/v1/jobs/not-a-uuid", "/api/v1/jobs/00000000-0000-0000-0000-000000000000/results"] {
        let (s, v) = call(&app, Method::GET, uri, None).await;
        assert_eq!((s, code(&v)), (StatusCode::NOT_FOUND, "job_not_found"), "{uri}");
    }
    let (s, _) = call(&app, Method::DELETE, "/api/v1/jobs/00000000-0000-0000-0000-000000000000", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn finished_jobs_are_evicted_when_the_store_is_full() {
    let addr = target().await;
    let app = test_app(Limits { max_stored_jobs: 2, ..Limits::default() });
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (s, v) = call(
            &app,
            Method::POST,
            "/api/v1/extract",
            Some(json!({ "url": format!("http://{addr}/"), "fields": ["title"] })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, v) = call(
            &app,
            Method::POST,
            "/api/v1/scrape",
            Some(json!({ "urls": [format!("http://{addr}/")], "fields": ["title"], "depth": 0 })),
        )
        .await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
        let id = v["id"].as_str().unwrap().to_string();
        wait_for(&app, &id, "completed").await;
        ids.push(id);
        tokio::time::sleep(Duration::from_millis(1100)).await; // created_at has 1s resolution
    }
    let (s, _) = call(&app, Method::GET, &format!("/api/v1/jobs/{}", ids[0]), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "oldest finished job evicted");
    let (s, _) = call(&app, Method::GET, &format!("/api/v1/jobs/{}", ids[2]), None).await;
    assert_eq!(s, StatusCode::OK);
}
