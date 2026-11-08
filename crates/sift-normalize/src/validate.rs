//! Validation stage: decides `Valid` / `Unverified` / `Invalid` for each extracted value.
//!
//! Rule: a value that fails syntax checks is `Invalid`; one that passes *and* came from
//! explicit machine-readable markup is `Valid`; one that passes but was pulled out of
//! free text is `Unverified`. Sift performs no network checks (no MX lookups, no dialing).

use crate::urls::parse_lenient;
use sift_core::{Extracted, FieldKind, Validation};

const FILE_LIKE_TLDS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "avif", "ico", "css", "js", "mjs", "woff", "woff2", "ttf", "eot",
    "pdf", "zip", "mp4", "webm", "map",
];
/// Domains and TLDs reserved by RFC 2606 / 6761: never real contact addresses.
const RESERVED_DOMAINS: &[&str] = &["example.com", "example.net", "example.org"];
const RESERVED_TLDS: &[&str] = &["test", "example", "invalid", "localhost"];
/// Telemetry pseudo-addresses that show up in bundled JS (e.g. `abc123@sentry.io`).
const NOISE_DOMAINS: &[&str] = &["sentry.io", "sentry-next.wixpress.com", "wixpress.com"];

pub fn is_valid_email(s: &str) -> bool {
    if s.len() > 254 || s.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else { return false };
    if local.is_empty() || local.len() > 64 || domain.contains('@') {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    if !local.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(c)) {
        return false;
    }
    let domain = domain.to_ascii_lowercase();
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    for l in &labels {
        if l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-') {
            return false;
        }
        if !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    let tld = labels[labels.len() - 1];
    let tld_ok = (tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())) || tld.starts_with("xn--");
    if !tld_ok || FILE_LIKE_TLDS.contains(&tld) || RESERVED_TLDS.contains(&tld) {
        return false;
    }
    if RESERVED_DOMAINS.contains(&domain.as_str()) {
        return false;
    }
    !NOISE_DOMAINS.iter().any(|n| domain == *n || domain.ends_with(&format!(".{n}")))
}

/// Digits of a phone-like string, ignoring formatting.
pub fn phone_digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

pub fn is_valid_phone(s: &str) -> bool {
    let s = s.trim();
    if !s.chars().all(|c| c.is_ascii_digit() || " +-().".contains(c)) {
        return false;
    }
    if s.matches('+').count() > 1 || (s.contains('+') && !s.starts_with('+')) {
        return false;
    }
    let digits = phone_digits(s);
    if !(7..=15).contains(&digits.len()) {
        return false;
    }
    if digits.chars().all(|c| c == digits.as_bytes()[0] as char) {
        return false; // 0000000000, 1111111111
    }
    // E.164 country codes never start with 0.
    !(s.starts_with('+') && digits.starts_with('0'))
}

fn is_valid_url(s: &str) -> bool {
    s.contains("://")
        && parse_lenient(s).is_some_and(|u| {
            u.host_str().is_some_and(|h| h.contains('.') || h.parse::<std::net::IpAddr>().is_ok() || h == "localhost")
        })
}

fn syntax_ok(field: FieldKind, value: &str) -> bool {
    use FieldKind::*;
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    match field {
        Email => is_valid_email(v),
        Phone | Fax => is_valid_phone(v),
        Website | Canonical | Links | ContactForm | Whatsapp | Instagram | Facebook | Linkedin | Twitter | Youtube
        | Github | Tiktok | Sitemaps => is_valid_url(v),
        StatusCode => v.parse::<u16>().is_ok_and(|c| (100..=599).contains(&c)),
        ResponseTime => v.strip_suffix(" ms").is_some_and(|n| n.parse::<u64>().is_ok()),
        _ => v.chars().count() <= 2000,
    }
}

/// Set `status` on one extraction.
pub fn validate(e: &mut Extracted) {
    e.status = if !syntax_ok(e.field, &e.value) {
        Validation::Invalid
    } else if e.method.is_explicit() {
        Validation::Valid
    } else {
        Validation::Unverified
    };
}

/// Validate a batch. With `drop_invalid`, invalid values are removed and counted;
/// otherwise they are kept with `status = invalid`.
pub fn validate_all(mut items: Vec<Extracted>, drop_invalid: bool) -> (Vec<Extracted>, usize) {
    for e in &mut items {
        validate(e);
    }
    if !drop_invalid {
        return (items, 0);
    }
    let before = items.len();
    items.retain(|e| e.status != Validation::Invalid);
    let dropped = before - items.len();
    (items, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::Method;

    #[test]
    fn email_syntax() {
        for ok in ["hello@kathmandu-soft.com.np", "a.b+tag@sub.domain.io", "x_y@xn--80ak6aa92e.com"] {
            assert!(is_valid_email(ok), "{ok}");
        }
        for bad in [
            "plain",
            "a@b",
            "a@@b.com",
            "@b.com",
            "a@.com",
            "a@b..com",
            ".a@b.com",
            "a..b@c.com",
            "a b@c.com",
            "logo@2x.png",
            "user@example.com",
            "x@host.test",
            "a@-b.com",
            "a@b.c",
            "1a2b@sentry.io",
            "x@o1.ingest.sentry.io",
        ] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn phone_syntax() {
        for ok in ["+977-9800000001", "+977 1 4412345", "(01) 441-2345", "9801234567", "00977 9801234567"] {
            assert!(is_valid_phone(ok), "{ok}");
        }
        for bad in ["12345", "0000000000", "+0123456789", "+1234567890123456", "98-01-xx", "12+345678", "call us"] {
            assert!(!is_valid_phone(bad), "{bad}");
        }
    }

    #[test]
    fn status_depends_on_syntax_and_method() {
        let mk = |f, v: &str, m| {
            let mut e = Extracted::new(f, v, "https://a.np/", m, 0.5);
            validate(&mut e);
            e.status
        };
        assert_eq!(mk(FieldKind::Email, "a@acme.np", Method::MailtoLink), Validation::Valid);
        assert_eq!(mk(FieldKind::Email, "a@acme.np", Method::TextPattern), Validation::Unverified);
        assert_eq!(mk(FieldKind::Email, "nope", Method::MailtoLink), Validation::Invalid);
        assert_eq!(mk(FieldKind::Instagram, "https://instagram.com/acme", Method::Anchor), Validation::Valid);
        assert_eq!(mk(FieldKind::Website, "not a url", Method::Seed), Validation::Invalid);
        assert_eq!(mk(FieldKind::StatusCode, "200", Method::HttpResponse), Validation::Valid);
        assert_eq!(mk(FieldKind::StatusCode, "999", Method::HttpResponse), Validation::Invalid);
        assert_eq!(mk(FieldKind::CompanyName, "Acme", Method::Heuristic), Validation::Unverified);
        assert_eq!(mk(FieldKind::CompanyName, "  ", Method::JsonLd), Validation::Invalid);
    }

    #[test]
    fn validate_all_drops_or_flags() {
        let items = vec![
            Extracted::new(FieldKind::Email, "ok@acme.np", "s", Method::MailtoLink, 0.9),
            Extracted::new(FieldKind::Email, "bad", "s", Method::TextPattern, 0.9),
        ];
        let (kept, dropped) = validate_all(items.clone(), true);
        assert_eq!((kept.len(), dropped), (1, 1));
        let (kept, dropped) = validate_all(items, false);
        assert_eq!((kept.len(), dropped), (2, 0));
        assert_eq!(kept[1].status, Validation::Invalid);
    }
}
