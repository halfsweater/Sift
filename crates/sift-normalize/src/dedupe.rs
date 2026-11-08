//! Deduplication stage.

use crate::normalize::{collapse_ws, is_url_field};
use crate::urls::parse_lenient;
use crate::urls::url_key;
use crate::validate::phone_digits;
use sift_core::{Extracted, FieldKind};
use std::collections::HashMap;

/// Phones are compared by their last 9 digits so `+977 980-123-4567` and the national form
/// `9801234567` collapse into one, without needing a country-code table.
fn phone_key(v: &str) -> String {
    let d = phone_digits(v);
    let start = d.len().saturating_sub(9);
    d[start..].to_string()
}

fn key(field: FieldKind, value: &str) -> String {
    match field {
        FieldKind::Email => value.trim().to_ascii_lowercase(),
        FieldKind::Phone | FieldKind::Fax => phone_key(value),
        f if is_url_field(f) => parse_lenient(value).map_or_else(|| value.to_string(), |u| url_key(&u)),
        _ => collapse_ws(value).to_lowercase(),
    }
}

fn better(a: &Extracted, b: &Extracted) -> bool {
    // Is `a` strictly better than `b`?
    (a.status, a.confidence, a.value.starts_with('+')).partial_cmp(&(b.status, b.confidence, b.value.starts_with('+')))
        == Some(std::cmp::Ordering::Greater)
}

/// Remove duplicate values per field, keeping the best-validated, highest-confidence
/// representative (first one wins ties). Order of first appearance is preserved.
/// Returns the unique items and how many were removed.
pub fn dedupe(items: Vec<Extracted>) -> (Vec<Extracted>, usize) {
    let mut out: Vec<Extracted> = Vec::with_capacity(items.len());
    let mut seen: HashMap<(FieldKind, String), usize> = HashMap::new();
    let mut removed = 0;
    for item in items {
        let k = (item.field, key(item.field, &item.value));
        match seen.get(&k) {
            Some(&idx) => {
                removed += 1;
                if better(&item, &out[idx]) {
                    out[idx] = item;
                }
            }
            None => {
                seen.insert(k, out.len());
                out.push(item);
            }
        }
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::{Method, Validation};

    fn e(f: FieldKind, v: &str, m: Method, c: f32) -> Extracted {
        let mut x = Extracted::new(f, v, "https://acme.np/", m, c);
        crate::validate::validate(&mut x);
        x
    }

    #[test]
    fn emails_are_case_insensitive_and_best_wins() {
        let (out, removed) = dedupe(vec![
            e(FieldKind::Email, "Hello@Acme.np", Method::TextPattern, 0.6),
            e(FieldKind::Email, "hello@acme.np", Method::MailtoLink, 0.9),
            e(FieldKind::Email, "other@acme.np", Method::TextPattern, 0.6),
        ]);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].value, "hello@acme.np");
        assert_eq!(out[0].status, Validation::Valid);
    }

    #[test]
    fn phones_match_across_formats_and_prefer_international() {
        let (out, removed) = dedupe(vec![
            e(FieldKind::Phone, "980-123-4567", Method::TextPattern, 0.6),
            e(FieldKind::Phone, "+9779801234567", Method::TextPattern, 0.6),
            e(FieldKind::Phone, "+977 1 4412345", Method::TelLink, 0.9),
        ]);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].value, "+9779801234567");
    }

    #[test]
    fn urls_dedupe_by_key_and_fields_do_not_mix() {
        let (out, removed) = dedupe(vec![
            e(FieldKind::Facebook, "https://facebook.com/acme/", Method::Anchor, 0.9),
            e(FieldKind::Facebook, "http://www.facebook.com/acme", Method::Anchor, 0.9),
            e(FieldKind::Linkedin, "https://facebook.com/acme", Method::Anchor, 0.9),
        ]);
        assert_eq!((out.len(), removed), (2, 1));
    }

    #[test]
    fn text_dedupes_ignoring_case_and_whitespace() {
        let (out, removed) = dedupe(vec![
            e(FieldKind::Headings, "Our  Services", Method::HtmlElement, 0.8),
            e(FieldKind::Headings, "our services", Method::HtmlElement, 0.8),
        ]);
        assert_eq!((out.len(), removed), (1, 1));
    }
}
