//! Technical information: HTTP facts, detectable technologies, sitemap links.

use crate::Ctx;
use sift_core::{Extracted, FieldKind as F, Method};
use sift_fetcher::FetchedPage;

/// (technology, lower-case substrings looked for in resource URLs / generator meta).
const SIGNATURES: &[(&str, &[&str])] = &[
    ("WordPress", &["/wp-content/", "/wp-includes/", "wordpress"]),
    ("Shopify", &["cdn.shopify.com", "myshopify.com"]),
    ("Wix", &["static.wixstatic.com", "parastorage.com"]),
    ("Squarespace", &["squarespace.com", "squarespace-cdn.com"]),
    ("Webflow", &["assets.website-files.com", "webflow.com"]),
    ("Drupal", &["/sites/default/files", "drupal"]),
    ("Joomla", &["/media/jui/", "joomla"]),
    ("Next.js", &["/_next/"]),
    ("Nuxt", &["/_nuxt/"]),
    ("Gatsby", &["/page-data/", "gatsby"]),
    ("jQuery", &["jquery"]),
    ("Bootstrap", &["bootstrap"]),
    ("Tailwind CSS", &["tailwindcss", "tailwind.css"]),
    ("Google Analytics", &["google-analytics.com", "googletagmanager.com/gtag"]),
    ("Google Tag Manager", &["googletagmanager.com/gtm.js"]),
    ("Facebook Pixel", &["connect.facebook.net"]),
    ("Cloudflare", &["cdnjs.cloudflare.com", "cloudflareinsights.com"]),
    ("HubSpot", &["hs-scripts.com", "hubspot.com"]),
    ("Font Awesome", &["fontawesome"]),
];

/// Raw markers searched for in the HTML source.
const BODY_MARKERS: &[(&str, &str)] =
    &[("React", "data-reactroot"), ("Next.js", "id=\"__next\""), ("Angular", "ng-version="), ("Vue.js", "data-v-app")];

fn product_name(header: &str) -> Option<String> {
    let name: String =
        header.trim().chars().take_while(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ' ')).collect();
    let name = name.trim();
    // "nginx/1.25.3" -> "nginx", "PHP/8.1" -> "PHP", "Microsoft-IIS/10.0" -> "Microsoft-IIS"
    (!name.is_empty()).then(|| name.to_string())
}

pub(crate) fn http_fields(page: &FetchedPage, fields: &[F], source: &str, out: &mut Vec<Extracted>) {
    let mk = |f: F, v: String| Extracted::new(f, v, source, Method::HttpResponse, 1.0);
    if fields.contains(&F::StatusCode) {
        out.push(mk(F::StatusCode, page.status.to_string()));
    }
    if fields.contains(&F::ResponseTime) {
        out.push(mk(F::ResponseTime, format!("{} ms", page.elapsed.as_millis())));
    }
    if fields.contains(&F::ContentType) {
        if let Some(ct) = &page.content_type {
            out.push(mk(F::ContentType, ct.clone()));
        }
    }
    if fields.contains(&F::RedirectChain) && page.redirect_chain.len() > 1 {
        out.push(mk(F::RedirectChain, page.redirect_chain.join(" -> ")));
    }
    if fields.contains(&F::Technologies) {
        for h in ["server", "x-powered-by", "x-generator"] {
            if let Some(name) = page.header(h).and_then(product_name) {
                let mut e = mk(F::Technologies, name);
                e.confidence = 0.9;
                out.push(e);
            }
        }
    }
}

pub(crate) fn html_fields(ctx: &Ctx, page: &FetchedPage, out: &mut Vec<Extracted>) {
    if ctx.want(F::Technologies) {
        let mut blob = String::new();
        for el in ctx.doc.select(sel!("script[src], link[href], img[src], iframe[src]")) {
            let v = el.value();
            blob.push_str(v.attr("src").or_else(|| v.attr("href")).unwrap_or_default());
            blob.push('\n');
        }
        for m in ctx.doc.select(sel!(r#"meta[name="generator" i]"#)) {
            if let Some(c) = m.value().attr("content") {
                if let Some(name) = product_name(c.split(|ch: char| ch.is_ascii_digit()).next().unwrap_or(c)) {
                    out.push(ctx.ex(F::Technologies, name, Method::MetaTag, 0.9));
                }
                blob.push_str(c);
                blob.push('\n');
            }
        }
        let blob = blob.to_lowercase();
        for (name, needles) in SIGNATURES {
            if needles.iter().any(|n| blob.contains(n)) {
                out.push(ctx.ex(F::Technologies, *name, Method::Heuristic, 0.7));
            }
        }
        if let Some(body) = page.body.as_deref() {
            for (name, marker) in BODY_MARKERS {
                if body.contains(marker) {
                    out.push(ctx.ex(F::Technologies, *name, Method::Heuristic, 0.7));
                }
            }
        }
    }
    if ctx.want(F::Sitemaps) {
        for l in ctx.doc.select(sel!("link[rel~=sitemap i][href]")) {
            if let Some(u) = l.value().attr("href").and_then(|h| ctx.resolve(h)) {
                out.push(ctx.ex(F::Sitemaps, u.as_str(), Method::Sitemap, 0.9));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::testutil::{page, values};
    use sift_core::FieldKind as F;

    #[test]
    fn http_facts_even_without_body() {
        let mut p = page("https://acme.np/x", "");
        p.body = None;
        p.status = 503;
        p.content_type = Some("image/png".into());
        p.headers = vec![("server".into(), "nginx/1.25.3".into()), ("x-powered-by".into(), "PHP/8.1.2".into())];
        p.redirect_chain = vec!["http://acme.np/x".into(), "https://acme.np/x".into()];
        let a =
            analyze(&p, &[F::StatusCode, F::ResponseTime, F::ContentType, F::RedirectChain, F::Technologies, F::Email]);
        assert_eq!(values(&a, F::StatusCode), vec!["503"]);
        assert_eq!(values(&a, F::ResponseTime), vec!["42 ms"]);
        assert_eq!(values(&a, F::ContentType), vec!["image/png"]);
        assert_eq!(values(&a, F::RedirectChain), vec!["http://acme.np/x -> https://acme.np/x"]);
        assert_eq!(values(&a, F::Technologies), vec!["nginx", "PHP"]);
        assert!(values(&a, F::Email).is_empty());
        assert!(a.links.is_empty());
    }

    #[test]
    fn technologies_from_html() {
        let html = r#"<html><head><meta name="generator" content="WordPress 6.4.2">
        <script src="https://acme.np/wp-content/themes/x/app.js"></script>
        <script src="https://code.jquery.com/jquery-3.7.1.min.js"></script>
        <link rel="sitemap" href="/sitemap_index.xml"></head><body><div id="__next"></div></body></html>"#;
        let a = analyze(&page("https://acme.np/", html), &[F::Technologies, F::Sitemaps]);
        let t = values(&a, F::Technologies);
        for want in ["WordPress", "jQuery", "Next.js"] {
            assert!(t.iter().any(|x| x == want), "{want} missing in {t:?}");
        }
        assert!(!t.iter().any(|x| x == "Shopify"));
        assert_eq!(values(&a, F::Sitemaps), vec!["https://acme.np/sitemap_index.xml"]);
    }
}
