//! A tiny fictional website for trying Sift offline.
//!
//! ```text
//! cargo run -p sift-engine --example demo_site          # serves http://127.0.0.1:8099
//! sift scrape http://127.0.0.1:8099 --fields name,email,phone,address,socials
//! ```
//!
//! Everything here is made-up demo data.

use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;

const HOME: &str = r#"<!doctype html><html><head><title>Demo Soft | Software Agency</title>
<meta name="description" content="A fictional software studio used to demo Sift.">
<script type="application/ld+json">{"@context":"https://schema.org","@type":"ProfessionalService","name":"Demo Soft Pvt. Ltd.",
"address":{"@type":"PostalAddress","streetAddress":"1 Example Marg","addressLocality":"Kathmandu","addressCountry":"NP"}}</script></head>
<body><h1>We build software</h1><nav><a href="/contact">Contact</a> <a href="/about">About</a> <a href="/blog/launch">Blog</a></nav>
<footer><a href="https://www.instagram.com/demosoft/">Instagram</a> <a href="https://www.linkedin.com/company/demo-soft">LinkedIn</a></footer></body></html>"#;

const CONTACT: &str = r#"<!doctype html><html><head><title>Contact | Demo Soft</title></head><body>
<h1>Contact us</h1><p>Email: <a href="mailto:hello@demosoft.com.np">hello@demosoft.com.np</a></p>
<p>Phone: <a href="tel:+97714000000">+977 1 4000000</a></p><p>Fax: +977 1 4000001</p>
<form action="/contact" method="post"><input type="email" name="email"><textarea name="message"></textarea></form></body></html>"#;

const ABOUT: &str = r#"<!doctype html><html><head><title>About | Demo Soft</title></head><body><h1>About</h1>
<div class="team-member"><h3>Asha Demo</h3><span class="job-title">Founder</span></div></body></html>"#;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(|| async { Html(HOME) }))
        .route("/contact", get(|| async { Html(CONTACT) }))
        .route("/about", get(|| async { Html(ABOUT) }))
        .route("/blog/launch", get(|| async { Html("<h1>We launched</h1><p>blog@demosoft.com.np</p>") }))
        .route(
            "/robots.txt",
            get(|| async {
                ([(header::CONTENT_TYPE, "text/plain")], "User-agent: *\nDisallow: /private\n").into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8099").await.expect("port 8099 is free");
    println!("demo site on http://127.0.0.1:8099 (Ctrl-C to stop)");
    axum::serve(listener, app).await.unwrap();
}
