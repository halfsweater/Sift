//! Benchmarks for the per-page hot path: parse + run extractors.
//!
//! Run with `cargo bench -p sift-extract`. Numbers depend on your machine; compare before/after
//! changes rather than quoting them as absolutes.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use sift_core::FieldKind as F;
use sift_extract::analyze;
use sift_fetcher::FetchedPage;
use std::time::Duration;
use url::Url;

/// A synthetic but realistic business page: JSON-LD, nav, many links, a contact block and filler text.
fn sample_html(paragraphs: usize) -> String {
    let mut h = String::from(
        r#"<!doctype html><html><head><title>Acme Technologies | Software Agency</title>
<meta name="description" content="Custom software studio in Kathmandu.">
<script type="application/ld+json">{"@type":"ProfessionalService","name":"Acme Technologies","telephone":"+977-1-4412345",
"address":{"@type":"PostalAddress","streetAddress":"Thamel Marg 12","addressLocality":"Kathmandu","addressCountry":"NP"},
"sameAs":["https://www.facebook.com/acme","https://www.linkedin.com/company/acme"]}</script></head><body><nav>"#,
    );
    for i in 0..60 {
        h.push_str(&format!(r#"<a href="/section/{i}/page">Section {i}</a>"#));
    }
    h.push_str("</nav><main>");
    for i in 0..paragraphs {
        h.push_str(&format!(
            "<p>Paragraph {i}: we build web and mobile products for customers across Nepal and beyond, \
             with a focus on reliability, accessibility and long-term maintainability. Call 2024-01-{:02} for details.</p>",
            i % 28 + 1
        ));
    }
    h.push_str(
        r#"</main><footer><a href="mailto:hello@acme.np">hello@acme.np</a> Phone: +977 1 4412345 Fax: +977 1 4412346
<a href="https://www.instagram.com/acme/">IG</a></footer></body></html>"#,
    );
    h
}

fn page(html: String) -> FetchedPage {
    let u = Url::parse("https://acme.np/").unwrap();
    FetchedPage {
        requested_url: u.clone(),
        final_url: u.clone(),
        status: 200,
        content_type: Some("text/html".into()),
        headers: vec![],
        body_bytes: html.len(),
        body: Some(html),
        truncated: false,
        elapsed: Duration::from_millis(10),
        redirect_chain: vec![u.to_string()],
    }
}

fn bench(c: &mut Criterion) {
    let all_contact = [F::CompanyName, F::Email, F::Phone, F::Fax, F::Address, F::Instagram, F::Facebook, F::Linkedin];
    let mut group = c.benchmark_group("analyze");
    for (name, paragraphs) in [("small_10kb", 40), ("large_200kb", 800)] {
        let p = page(sample_html(paragraphs));
        group.throughput(Throughput::Bytes(p.body_bytes as u64));
        group.bench_function(format!("{name}/contact_fields"), |b| {
            b.iter(|| analyze(black_box(&p), black_box(&all_contact)))
        });
        group.bench_function(format!("{name}/links_only"), |b| {
            b.iter(|| analyze(black_box(&p), black_box(&[F::Title])))
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
