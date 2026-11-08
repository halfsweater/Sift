//! HTML parsing and field extraction.
//!
//! [`analyze`] parses one fetched page and runs only the extractors needed for the
//! requested fields. Extractors report *what they found and how* ([`Method`]) but do
//! not judge validity — that is the validation stage's job (`sift-normalize`).
//!
//! The HTML DOM is not `Send`, so [`analyze`] is a plain synchronous function that
//! returns owned data; async callers run it with `spawn_blocking`.

macro_rules! sel {
    ($s:expr) => {{
        static S: std::sync::LazyLock<scraper::Selector> =
            std::sync::LazyLock::new(|| scraper::Selector::parse($s).expect("valid selector"));
        &*S
    }};
}

mod business;
mod contact;
mod jsonld;
mod tech;
mod text;
mod web;

pub use web::classify_social;

use scraper::{ElementRef, Html};
use sift_core::{Extracted, FieldKind, Method};
use sift_fetcher::FetchedPage;
use std::cell::OnceCell;
use url::Url;

/// A hyperlink found on the page, resolved against the page's base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLink {
    pub url: Url,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct PageAnalysis {
    pub url: Url,
    pub extractions: Vec<Extracted>,
    /// Followable links (`rel=nofollow` anchors and pages marked `nofollow` yield none).
    pub links: Vec<DiscoveredLink>,
}

/// Per-page extraction context shared by all extractors.
pub(crate) struct Ctx<'a> {
    pub doc: &'a Html,
    pub base: Url,
    pub source: String,
    pub wants: &'a [FieldKind],
    pub jsonld: Vec<serde_json::Value>,
    text: OnceCell<String>,
}

impl<'a> Ctx<'a> {
    pub fn want(&self, f: FieldKind) -> bool {
        self.wants.contains(&f)
    }

    pub fn want_any(&self, fs: &[FieldKind]) -> bool {
        fs.iter().any(|f| self.want(f.to_owned()))
    }

    /// Visible text of the page (scripts/styles excluded), computed once.
    pub fn text(&self) -> &str {
        self.text.get_or_init(|| text::visible_text(self.doc))
    }

    pub fn ex(&self, field: FieldKind, value: impl Into<String>, method: Method, conf: f32) -> Extracted {
        Extracted::new(field, value, self.source.clone(), method, conf)
    }

    pub fn resolve(&self, href: &str) -> Option<Url> {
        self.base.join(href.trim()).ok()
    }
}

pub(crate) fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Analyze one fetched page for the requested `fields`.
///
/// Pages without a body (binary content, error statuses) still yield the technical
/// fields (status code, timing, content type, redirect chain, server headers).
pub fn analyze(page: &FetchedPage, fields: &[FieldKind]) -> PageAnalysis {
    let source = page.final_url.to_string();
    let mut extractions = Vec::new();
    tech::http_fields(page, fields, &source, &mut extractions);

    let mut links = Vec::new();
    if let (Some(body), true) = (page.body.as_deref(), page.is_success()) {
        let doc = Html::parse_document(body);
        let base = web::base_url(&doc, &page.final_url);
        let ctx = Ctx { jsonld: jsonld::collect(&doc), doc: &doc, base, source, wants: fields, text: OnceCell::new() };
        contact::extract(&ctx, &mut extractions);
        business::extract(&ctx, &mut extractions);
        web::extract(&ctx, &mut extractions);
        tech::html_fields(&ctx, page, &mut extractions);
        links = web::followable_links(&ctx);
    }
    PageAnalysis { url: page.final_url.clone(), extractions, links }
}

#[cfg(test)]
pub(crate) mod testutil {
    use sift_fetcher::FetchedPage;
    use std::time::Duration;
    use url::Url;

    pub fn page(url: &str, html: &str) -> FetchedPage {
        let u = Url::parse(url).unwrap();
        FetchedPage {
            requested_url: u.clone(),
            final_url: u.clone(),
            status: 200,
            content_type: Some("text/html; charset=utf-8".into()),
            headers: vec![],
            body: Some(html.to_string()),
            body_bytes: html.len(),
            truncated: false,
            elapsed: Duration::from_millis(42),
            redirect_chain: vec![u.to_string()],
        }
    }

    pub fn values(a: &crate::PageAnalysis, f: sift_core::FieldKind) -> Vec<String> {
        a.extractions.iter().filter(|e| e.field == f).map(|e| e.value.clone()).collect()
    }
}
