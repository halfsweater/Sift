//! Pure-function tests for link prioritisation and sitemap parsing.

use sift_core::FieldKind as F;
use sift_crawler::priority::{is_skipped_resource, keywords_for, score_link};
use sift_crawler::sitemap::parse_sitemap;
use url::Url;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn keyword_sets_follow_requested_fields() {
    assert!(keywords_for(&[F::Title, F::Headings]).is_empty());
    assert_eq!(keywords_for(&[F::Email])[0], "contact");
    let k = keywords_for(&[F::Team, F::Services]);
    assert!(k.contains(&"leadership") && k.contains(&"solutions"));
    assert!(!k.contains(&"contact"));
}

#[test]
fn contact_pages_outrank_about_outrank_noise() {
    let kws = keywords_for(&[F::Email]);
    let contact = score_link(&u("https://a.np/contact"), "", &kws);
    let about = score_link(&u("https://a.np/about-us"), "", &kws);
    let partial = score_link(&u("https://a.np/contact-sales-team"), "", &kws);
    let blog = score_link(&u("https://a.np/blog/contact-tips"), "", &kws);
    let random = score_link(&u("https://a.np/pricing"), "", &kws);
    let text_only = score_link(&u("https://a.np/p/77"), "Get in touch", &kws);
    assert!(contact > about && about > 0, "{contact} {about}");
    assert!(contact > partial && partial > 0);
    assert!(blog < 0);
    assert_eq!(random, 0);
    assert!(text_only > 0 && text_only < partial);
}

#[test]
fn resources_are_skipped() {
    assert!(is_skipped_resource(&u("https://a.np/files/brochure.PDF")));
    assert!(is_skipped_resource(&u("https://a.np/logo.svg?x=1")));
    assert!(!is_skipped_resource(&u("https://a.np/contact")));
    assert!(!is_skipped_resource(&u("https://a.np/team.html")));
    assert!(!is_skipped_resource(&u("https://a.np/")));
}

#[test]
fn parses_urlset_and_index_and_cdata() {
    let (idx, locs) = parse_sitemap(
        r#"<?xml version="1.0"?><urlset><url><loc>https://a.np/contact</loc></url>
        <url><loc> https://a.np/q?a=1&amp;b=2 </loc></url><url><loc><![CDATA[https://a.np/about]]></loc></url></urlset>"#,
    );
    assert!(!idx);
    assert_eq!(locs, vec!["https://a.np/contact", "https://a.np/q?a=1&b=2", "https://a.np/about"]);
    let (idx, locs) = parse_sitemap("<sitemapindex><sitemap><loc>https://a.np/s1.xml</loc></sitemap></sitemapindex>");
    assert!(idx);
    assert_eq!(locs, vec!["https://a.np/s1.xml"]);
}
