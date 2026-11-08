//! Normalization stage: canonical spelling for each value.

use crate::urls::{display_url, parse_lenient};
use crate::validate::phone_digits;
use sift_core::{Extracted, FieldKind};

/// Collapse all runs of whitespace to single spaces and trim.
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// International numbers become compact E.164 (`+9779801234567`); `00` prefixes are
/// converted to `+`. National numbers (no country code visible) are left in their
/// original grouping with whitespace collapsed — Sift never guesses a country code.
pub fn normalize_phone(raw: &str) -> String {
    let t = raw.trim();
    let digits = phone_digits(t);
    if t.starts_with('+') {
        format!("+{digits}")
    } else if let Some(rest) = digits.strip_prefix("00").filter(|_| t.starts_with("00")) {
        format!("+{rest}")
    } else {
        collapse_ws(t)
    }
}

pub(crate) fn is_url_field(f: FieldKind) -> bool {
    use FieldKind::*;
    matches!(
        f,
        Website
            | Canonical
            | Links
            | ContactForm
            | Whatsapp
            | Instagram
            | Facebook
            | Linkedin
            | Twitter
            | Youtube
            | Github
            | Tiktok
            | Sitemaps
    )
}

pub fn normalize_value(field: FieldKind, value: &str) -> String {
    use FieldKind::*;
    match field {
        Email => value.trim().to_ascii_lowercase(),
        Phone | Fax => normalize_phone(value),
        f if is_url_field(f) => parse_lenient(value).map_or_else(|| value.trim().to_string(), |u| display_url(&u)),
        Address => collapse_ws(value).trim_matches(|c: char| c == ',' || c.is_whitespace()).to_string(),
        StatusCode | ResponseTime | ContentType | RedirectChain => value.trim().to_string(),
        _ => collapse_ws(value),
    }
}

pub fn normalize(e: &mut Extracted) {
    e.value = normalize_value(e.field, &e.value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones() {
        assert_eq!(normalize_phone("+977 (1) 441-2345"), "+97714412345");
        assert_eq!(normalize_phone("00977 9801234567"), "+9779801234567");
        assert_eq!(normalize_phone("  (01)   441 2345 "), "(01) 441 2345");
    }

    #[test]
    fn values() {
        assert_eq!(normalize_value(FieldKind::Email, " Hello@Acme.NP "), "hello@acme.np");
        assert_eq!(
            normalize_value(FieldKind::Instagram, "https://www.instagram.com/acme/?utm_source=x"),
            "https://www.instagram.com/acme/"
        );
        assert_eq!(normalize_value(FieldKind::Website, "https://Acme.np/"), "https://acme.np");
        assert_eq!(normalize_value(FieldKind::Address, "  Thamel,\n  Kathmandu , "), "Thamel, Kathmandu");
        assert_eq!(normalize_value(FieldKind::CompanyName, "Acme \t Tech"), "Acme Tech");
    }
}
