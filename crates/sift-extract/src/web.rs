//! Web information: titles, meta, headings, links, canonical URL, social profiles.

use crate::{text_of, Ctx, DiscoveredLink};
use scraper::Html;
use sift_core::{Extracted, FieldKind as F, Method};
use url::Url;

pub(crate) const SOCIAL_FIELDS: [F; 7] = sift_core::SOCIALS;

/// Document base URL: `<base href>` if present and valid, else the page's final URL.
pub(crate) fn base_url(doc: &Html, page_url: &Url) -> Url {
    doc.select(sel!("base[href]"))
        .next()
        .and_then(|b| b.value().attr("href"))
        .and_then(|h| page_url.join(h).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or_else(|| page_url.clone())
}

pub(crate) fn extract(ctx: &Ctx, out: &mut Vec<Extracted>) {
    if ctx.want(F::Title) {
        if let Some(t) = ctx.doc.select(sel!("title")).next().map(text_of).filter(|t| !t.is_empty()) {
            out.push(ctx.ex(F::Title, t, Method::HtmlElement, 0.95));
        }
    }
    if ctx.want(F::MetaDescription) {
        if let Some(c) =
            ctx.doc.select(sel!(r#"meta[name="description" i]"#)).next().and_then(|m| m.value().attr("content"))
        {
            let c = c.split_whitespace().collect::<Vec<_>>().join(" ");
            if !c.is_empty() {
                out.push(ctx.ex(F::MetaDescription, c, Method::MetaTag, 0.95));
            }
        }
    }
    if ctx.want(F::Headings) {
        for h in ctx.doc.select(sel!("h1, h2, h3")).take(60) {
            let t = text_of(h);
            if !t.is_empty() && t.chars().count() <= 200 {
                out.push(ctx.ex(F::Headings, t, Method::HtmlElement, 0.9));
            }
        }
    }
    if ctx.want(F::Canonical) {
        if let Some(u) = ctx
            .doc
            .select(sel!("link[rel~=canonical i][href]"))
            .next()
            .and_then(|l| l.value().attr("href"))
            .and_then(|h| ctx.resolve(h))
        {
            out.push(ctx.ex(F::Canonical, u.as_str(), Method::HtmlElement, 0.95));
        }
    }
    if ctx.want(F::Links) {
        for a in ctx.doc.select(sel!("a[href]")).take(2000) {
            if let Some(u) = http_link(ctx, a.value().attr("href").unwrap_or_default()) {
                out.push(ctx.ex(F::Links, u.as_str(), Method::Anchor, 1.0));
            }
        }
    }
    if ctx.want_any(&SOCIAL_FIELDS) {
        for a in ctx.doc.select(sel!("a[href]")) {
            if let Some((field, canon)) =
                http_link(ctx, a.value().attr("href").unwrap_or_default()).and_then(|u| classify_social(&u))
            {
                if ctx.want(field) {
                    out.push(ctx.ex(field, canon, Method::Anchor, 0.85));
                }
            }
        }
    }
}

fn http_link(ctx: &Ctx, href: &str) -> Option<Url> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if href.is_empty() || ["#", "javascript:", "mailto:", "tel:", "data:", "sms:"].iter().any(|p| lower.starts_with(p))
    {
        return None;
    }
    let u = ctx.resolve(href)?;
    matches!(u.scheme(), "http" | "https").then(|| sift_normalize::normalize_url(&u))
}

/// Links the crawler may follow. Empty when the page declares `nofollow`.
pub(crate) fn followable_links(ctx: &Ctx) -> Vec<DiscoveredLink> {
    let nofollow_page = ctx
        .doc
        .select(sel!(r#"meta[name="robots" i], meta[name="sift" i]"#))
        .filter_map(|m| m.value().attr("content"))
        .any(|c| c.to_ascii_lowercase().contains("nofollow"));
    if nofollow_page {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut links = Vec::new();
    for a in ctx.doc.select(sel!("a[href]")) {
        let v = a.value();
        if v.attr("rel").is_some_and(|r| r.to_ascii_lowercase().split_whitespace().any(|t| t == "nofollow")) {
            continue;
        }
        let Some(url) = http_link(ctx, v.attr("href").unwrap_or_default()) else { continue };
        if !seen.insert(url.to_string()) {
            continue;
        }
        let mut text = text_of(a);
        if text.is_empty() {
            text = v.attr("title").or_else(|| v.attr("aria-label")).unwrap_or_default().trim().to_string();
        }
        links.push(DiscoveredLink { url, text: text.chars().take(100).collect() });
        if links.len() >= 2000 {
            break;
        }
    }
    links
}

const IG_RESERVED: [&str; 11] =
    ["p", "reel", "reels", "explore", "accounts", "stories", "tv", "direct", "about", "legal", "web"];
const FB_RESERVED: [&str; 17] = [
    "sharer",
    "sharer.php",
    "share",
    "share.php",
    "dialog",
    "plugins",
    "tr",
    "login",
    "login.php",
    "policies",
    "help",
    "privacy",
    "legal",
    "watch",
    "marketplace",
    "events",
    "hashtag",
];
const TW_RESERVED: [&str; 12] = [
    "intent",
    "share",
    "home",
    "search",
    "i",
    "hashtag",
    "login",
    "explore",
    "settings",
    "tos",
    "privacy",
    "widgets.js",
];
const GH_RESERVED: [&str; 16] = [
    "features",
    "about",
    "pricing",
    "login",
    "join",
    "sponsors",
    "orgs",
    "topics",
    "marketplace",
    "explore",
    "settings",
    "notifications",
    "pulls",
    "issues",
    "enterprise",
    "security",
];

/// Recognise a *profile* link on a supported social network and return its canonical form.
/// Share buttons, posts, intents and login pages are rejected.
pub fn classify_social(url: &Url) -> Option<(F, String)> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let h = ["www.", "m.", "mobile."].iter().find_map(|p| host.strip_prefix(p)).unwrap_or(&host);
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let s0 = segs.first()?.to_ascii_lowercase();
    let simple = |s: &str| !s.is_empty() && s.len() <= 100;

    match h {
        "instagram.com" if !IG_RESERVED.contains(&s0.as_str()) && simple(segs[0]) => {
            Some((F::Instagram, format!("https://www.instagram.com/{}", segs[0])))
        }
        "facebook.com" | "fb.com" | "fb.me" if !FB_RESERVED.contains(&s0.as_str()) => {
            let path = match s0.as_str() {
                "pages" if segs.len() >= 3 => segs[..3].join("/"),
                "pages" => return None,
                "profile.php" => {
                    let id = url.query_pairs().find(|(k, _)| k == "id")?.1.into_owned();
                    format!("profile.php?id={id}")
                }
                _ if simple(segs[0]) => segs[0].to_string(),
                _ => return None,
            };
            Some((F::Facebook, format!("https://www.facebook.com/{path}")))
        }
        "linkedin.com" if matches!(s0.as_str(), "company" | "in" | "school") && segs.len() >= 2 => {
            Some((F::Linkedin, format!("https://www.linkedin.com/{}/{}", s0, segs[1])))
        }
        "twitter.com" | "x.com" if !TW_RESERVED.contains(&s0.as_str()) && simple(segs[0]) => {
            Some((F::Twitter, format!("https://{h}/{}", segs[0])))
        }
        "youtube.com" if segs[0].starts_with('@') => Some((F::Youtube, format!("https://www.youtube.com/{}", segs[0]))),
        "youtube.com" if matches!(s0.as_str(), "channel" | "c" | "user") && segs.len() >= 2 => {
            Some((F::Youtube, format!("https://www.youtube.com/{}/{}", s0, segs[1])))
        }
        "github.com" if segs.len() == 1 && !GH_RESERVED.contains(&s0.as_str()) => {
            Some((F::Github, format!("https://github.com/{}", segs[0])))
        }
        "tiktok.com" if segs[0].starts_with('@') => Some((F::Tiktok, format!("https://www.tiktok.com/{}", segs[0]))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::classify_social;
    use crate::analyze;
    use crate::testutil::{page, values};
    use sift_core::FieldKind as F;
    use url::Url;

    fn c(u: &str) -> Option<(F, String)> {
        classify_social(&Url::parse(u).unwrap())
    }

    #[test]
    fn social_profiles_vs_noise() {
        assert_eq!(
            c("https://instagram.com/acme.np/?hl=en"),
            Some((F::Instagram, "https://www.instagram.com/acme.np".into()))
        );
        assert_eq!(c("https://www.instagram.com/p/Cabc123/"), None);
        assert_eq!(c("https://m.facebook.com/acmenp/"), Some((F::Facebook, "https://www.facebook.com/acmenp".into())));
        assert_eq!(c("https://www.facebook.com/sharer/sharer.php?u=x"), None);
        assert_eq!(
            c("https://facebook.com/profile.php?id=1000123"),
            Some((F::Facebook, "https://www.facebook.com/profile.php?id=1000123".into()))
        );
        assert_eq!(
            c("https://www.linkedin.com/company/acme-tech/about/"),
            Some((F::Linkedin, "https://www.linkedin.com/company/acme-tech".into()))
        );
        assert_eq!(c("https://linkedin.com/shareArticle?url=x"), None);
        assert_eq!(c("https://twitter.com/acme"), Some((F::Twitter, "https://twitter.com/acme".into())));
        assert_eq!(c("https://x.com/acme"), Some((F::Twitter, "https://x.com/acme".into())));
        assert_eq!(c("https://twitter.com/intent/tweet?text=hi"), None);
        assert_eq!(c("https://www.youtube.com/@acme"), Some((F::Youtube, "https://www.youtube.com/@acme".into())));
        assert_eq!(c("https://www.youtube.com/watch?v=abc"), None);
        assert_eq!(c("https://github.com/acme"), Some((F::Github, "https://github.com/acme".into())));
        assert_eq!(c("https://github.com/acme/repo"), None);
        assert_eq!(c("https://www.tiktok.com/@acme"), Some((F::Tiktok, "https://www.tiktok.com/@acme".into())));
        assert_eq!(c("https://example.org/acme"), None);
        assert_eq!(c("https://instagram.com/"), None);
    }

    #[test]
    fn page_level_web_fields() {
        let html = r##"<html><head><title> Acme  | Home </title>
        <meta name="description" content="Software   studio">
        <link rel="canonical" href="/home"><base href="https://acme.np/en/"></head>
        <body><h1>Welcome</h1><h2>Services</h2>
        <a href="team">Team</a> <a href="mailto:x@y.np">m</a> <a href="#top">t</a>
        <a href="https://instagram.com/acme">ig</a> <a href="https://acme.np/en/team?utm_source=a#x">dup</a></body></html>"##;
        let a = analyze(
            &page("https://acme.np/", html),
            &[F::Title, F::MetaDescription, F::Headings, F::Canonical, F::Links, F::Instagram],
        );
        assert_eq!(values(&a, F::Title), vec!["Acme | Home"]);
        assert_eq!(values(&a, F::MetaDescription), vec!["Software studio"]);
        assert_eq!(values(&a, F::Headings), vec!["Welcome", "Services"]);
        assert_eq!(values(&a, F::Canonical), vec!["https://acme.np/home"]);
        assert_eq!(values(&a, F::Instagram), vec!["https://www.instagram.com/acme"]);
        let links = values(&a, F::Links);
        assert!(links.contains(&"https://acme.np/en/team".to_string()), "{links:?}");
        assert!(!links.iter().any(|l| l.starts_with("mailto") || l.contains('#')));
        // followable links are deduplicated after normalisation
        let team = a.links.iter().filter(|l| l.url.path() == "/en/team").count();
        assert_eq!(team, 1);
    }

    #[test]
    fn nofollow_is_respected() {
        let a = analyze(
            &page("https://acme.np/", r#"<a href="/a" rel="nofollow noopener">a</a><a href="/b">b</a>"#),
            &[F::Title],
        );
        assert_eq!(a.links.len(), 1);
        let a = analyze(
            &page("https://acme.np/", r#"<meta name="robots" content="index, NOFOLLOW"><a href="/b">b</a>"#),
            &[F::Title],
        );
        assert!(a.links.is_empty());
    }
}
