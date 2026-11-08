//! Benchmarks for URL normalisation and deduplication.
//!
//! Run with `cargo bench -p sift-normalize`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sift_core::{Extracted, FieldKind, Method};
use sift_normalize::{dedupe, url_key, validate_all};
use url::Url;

fn bench(c: &mut Criterion) {
    let urls: Vec<Url> = (0..1000)
        .map(|i| Url::parse(&format!("https://www.acme.np/section/{i}/page?utm_source=x&b=2&a={i}#frag")).unwrap())
        .collect();
    c.bench_function("url_key/1000_urls", |b| {
        b.iter(|| urls.iter().map(|u| url_key(black_box(u)).len()).sum::<usize>())
    });

    let items: Vec<Extracted> = (0..5000)
        .map(|i| {
            let (field, value) = match i % 3 {
                0 => (FieldKind::Email, format!("User{}@Acme.np", i % 700)),
                1 => (FieldKind::Phone, format!("+977 1 44{:05}", i % 900)),
                _ => (FieldKind::Facebook, format!("https://www.facebook.com/acme{}", i % 400)),
            };
            Extracted::new(field, value, "https://acme.np/contact", Method::TextPattern, 0.6)
        })
        .collect();
    c.bench_function("validate_and_dedupe/5000_values", |b| {
        b.iter(|| {
            let (valid, _) = validate_all(black_box(items.clone()), true);
            dedupe(valid)
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
