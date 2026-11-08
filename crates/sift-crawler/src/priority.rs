//! Link prioritisation: which pages are likely to contain the requested fields?

use sift_core::FieldKind as F;
use url::Url;

const CONTACT_KW: &[&str] = &[
    "contact",
    "contact-us",
    "contactus",
    "get-in-touch",
    "reach-us",
    "about",
    "about-us",
    "support",
    "team",
    "company",
    "location",
    "locations",
    "office",
    "offices",
    "find-us",
    "visit",
    "help",
    "imprint",
    "impressum",
];
const TEAM_KW: &[&str] =
    &["team", "our-team", "people", "staff", "leadership", "management", "about", "about-us", "company"];
const SERVICES_KW: &[&str] = &[
    "services",
    "service",
    "solutions",
    "what-we-do",
    "products",
    "offerings",
    "expertise",
    "capabilities",
    "portfolio",
];
const ABOUT_KW: &[&str] = &["about", "about-us", "company", "who-we-are"];
const SOCIAL_KW: &[&str] = &["contact", "about"];
const HOURS_KW: &[&str] = &["contact", "location", "visit", "hours"];

/// Path segments that almost never carry business/contact data.
const NEGATIVE: &[&str] = &[
    "blog",
    "news",
    "tag",
    "tags",
    "category",
    "categories",
    "author",
    "cart",
    "checkout",
    "login",
    "signin",
    "signup",
    "register",
    "wp-admin",
    "wp-json",
    "feed",
    "privacy",
    "terms",
    "cookie",
    "cookies",
    "search",
    "account",
    "download",
    "downloads",
    "page",
    "archive",
    "archives",
    "events",
];

const SKIPPED_EXTENSIONS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "gif", "svg", "webp", "ico", "css", "js", "mjs", "zip", "gz", "rar", "7z", "tar",
    "mp3", "mp4", "avi", "mov", "wmv", "webm", "woff", "woff2", "ttf", "eot", "xml", "json", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "exe", "dmg", "apk", "iso", "bin", "csv", "rss", "atom",
];

/// Keywords (most important first) for pages that should be visited to find `fields`.
/// Empty when the fields live on the page already being analysed (title, headings, ...).
pub fn keywords_for(fields: &[F]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let mut add = |list: &[&'static str]| {
        for k in list {
            if !out.contains(k) {
                out.push(k);
            }
        }
    };
    if fields.iter().any(|f| {
        matches!(
            f,
            F::Email | F::Phone | F::Fax | F::Whatsapp | F::ContactForm | F::Address | F::City | F::Region | F::Country
        )
    }) {
        add(CONTACT_KW);
    }
    if fields.contains(&F::Hours) {
        add(HOURS_KW);
    }
    if fields.contains(&F::Team) {
        add(TEAM_KW);
    }
    if fields.contains(&F::Services) {
        add(SERVICES_KW);
    }
    if fields.iter().any(|f| matches!(f, F::CompanyName | F::Description | F::Category)) {
        add(ABOUT_KW);
    }
    if fields.iter().any(|f| f.is_social()) {
        add(SOCIAL_KW);
    }
    out
}

/// True for URLs that point at files rather than pages.
pub fn is_skipped_resource(url: &Url) -> bool {
    url.path_segments()
        .and_then(|mut s| s.next_back())
        .and_then(|last| last.rsplit_once('.'))
        .is_some_and(|(_, ext)| SKIPPED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Relevance of a link: `> 0` means "worth visiting for these keywords".
/// Exact keyword segments (`/contact`) beat partial matches (`/contact-sales`) beat
/// matches in the link text only; earlier keywords weigh more. Blog/login/cart style
/// paths are pushed far below zero.
pub fn score_link(url: &Url, link_text: &str, keywords: &[&str]) -> i32 {
    let segments: Vec<String> = url
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).map(str::to_ascii_lowercase).collect())
        .unwrap_or_default();

    let mut best = 0;
    for (i, kw) in keywords.iter().enumerate() {
        let weight = 100 - (i as i32).min(40);
        for seg in &segments {
            let seg = seg.trim_end_matches(".html").trim_end_matches(".php").trim_end_matches(".htm");
            if seg == *kw {
                best = best.max(weight + 50);
            } else if seg.contains(kw) {
                best = best.max(weight);
            }
        }
    }
    if best == 0 {
        let text = link_text.to_ascii_lowercase();
        if let Some(i) = keywords.iter().position(|kw| text.contains(&kw.replace('-', " "))) {
            best = 40 - (i as i32).min(20);
        }
    }
    if segments.iter().any(|s| NEGATIVE.contains(&s.as_str())) {
        best -= 1000;
    }
    best
}
