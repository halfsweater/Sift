//! Post-crawl stages: group → validate → normalize → deduplicate → records.
//!
//! Kept free of any I/O so it can be tested with hand-built crawl output.

use crate::RecordScope;
use sift_core::{Extracted, ExtractionSpec, FieldKind, Metadata, Method, Record, Validation};
use sift_crawler::{CrawlOutput, PageResult};
use sift_normalize::{dedupe, host_key, normalize, url_key, validate_all};
use std::collections::BTreeMap;

pub struct Built {
    pub records: Vec<Record>,
    pub duplicates_removed: usize,
    pub invalid_discarded: usize,
}

fn origin(url: &url::Url) -> String {
    let mut o = url.clone();
    o.set_path("/");
    o.set_query(None);
    o.set_fragment(None);
    sift_normalize::display_url(&o)
}

/// Run the validation, normalization and dedup stages over one record's raw extractions.
pub fn finish_record(
    website: String,
    pages_visited: usize,
    raw: Vec<Extracted>,
    spec: &ExtractionSpec,
) -> (Record, usize, usize) {
    // Validation (optionally drops invalid values), then normalization, then dedup.
    let (mut items, invalid) = if spec.validate { validate_all(raw, !spec.keep_invalid) } else { (raw, 0) };
    items.iter_mut().for_each(normalize);
    let (items, removed) = if spec.deduplicate { dedupe(items) } else { (items, 0) };

    let mut fields: BTreeMap<FieldKind, Vec<Extracted>> = spec.fields.iter().map(|f| (*f, Vec::new())).collect();
    for it in items {
        if let Some(list) = fields.get_mut(&it.field) {
            list.push(it);
        }
    }
    // Best candidate first: validated beats unverified, then higher confidence. Stable.
    for list in fields.values_mut() {
        if !spec.include_source {
            list.iter_mut().for_each(|e| e.source.clear());
        }
        list.sort_by(|a, b| {
            b.status.cmp(&a.status).then(b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });
    }
    (Record { website, pages_visited, fields }, removed, invalid)
}

pub fn build(crawl: &CrawlOutput, spec: &ExtractionSpec, scope: RecordScope) -> Built {
    // Only pages that were fetched and analysed contribute.
    let mut pages: Vec<&PageResult> = crawl.pages.iter().filter(|p| p.analysis.is_some()).collect();
    pages.sort_by(|a, b| (a.depth, a.url.as_str()).cmp(&(b.depth, b.url.as_str())));

    let mut groups: Vec<(String, Vec<&PageResult>)> = Vec::new();
    for p in pages {
        let key = match scope {
            RecordScope::Site => host_key(&p.url),
            RecordScope::Page => url_key(&p.url),
        };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(p),
            None => groups.push((key, vec![p])),
        }
    }

    let mut built = Built { records: Vec::new(), duplicates_removed: 0, invalid_discarded: 0 };
    for (key, group) in groups {
        let first = group[0];
        let website = match scope {
            RecordScope::Site => origin(&first.url),
            RecordScope::Page => sift_normalize::display_url(&first.url),
        };
        let mut raw: Vec<Extracted> =
            group.iter().filter_map(|p| p.analysis.as_ref()).flat_map(|a| a.extractions.iter().cloned()).collect();
        if spec.wants(FieldKind::Website) {
            raw.push(Extracted::new(FieldKind::Website, website.clone(), first.url.as_str(), Method::Seed, 1.0));
        }
        if spec.wants(FieldKind::Sitemaps) && scope == RecordScope::Site {
            for s in crawl.sitemaps.iter().filter(|s| s.site == key) {
                raw.push(Extracted::new(FieldKind::Sitemaps, s.url.clone(), s.url.clone(), Method::Sitemap, 0.95));
            }
        }
        let (rec, removed, invalid) = finish_record(website, group.len(), raw, spec);
        built.duplicates_removed += removed;
        built.invalid_discarded += invalid;
        built.records.push(rec);
    }
    built.records.sort_by(|a, b| a.website.cmp(&b.website));
    built
}

/// Human-readable problems worth surfacing (failed seeds/pages), deduplicated and capped.
pub fn collect_errors(crawl: &CrawlOutput) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    for p in &crawl.pages {
        let Some(e) = &p.error else { continue };
        if e == "cancelled" {
            continue;
        }
        let line = format!("{}: {}", p.requested, e);
        if !errs.contains(&line) {
            errs.push(line);
        }
    }
    errs.truncate(25);
    errs
}

pub fn metadata(crawl: &CrawlOutput, built: &Built, errors: Vec<String>) -> Metadata {
    Metadata {
        pages_crawled: crawl.stats.pages_fetched,
        pages_failed: crawl.stats.pages_failed,
        pages_blocked_by_robots: crawl.stats.blocked_by_robots,
        records_found: built.records.len(),
        duplicates_removed: built.duplicates_removed,
        invalid_values_discarded: built.invalid_discarded,
        elapsed_ms: crawl.stats.elapsed_ms,
        cancelled: crawl.cancelled,
        errors,
    }
}

/// Count of values per validation status — handy for summaries.
pub fn status_counts(records: &[Record]) -> (usize, usize, usize) {
    let mut c = (0, 0, 0);
    for e in records.iter().flat_map(|r| r.fields.values().flatten()) {
        match e.status {
            Validation::Valid => c.0 += 1,
            Validation::Unverified => c.1 += 1,
            Validation::Invalid => c.2 += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_core::Method;

    fn ex(f: FieldKind, v: &str, m: Method, c: f32) -> Extracted {
        Extracted::new(f, v, "https://acme.np/contact", m, c)
    }

    #[test]
    fn stages_run_in_order_and_best_value_comes_first() {
        let spec = ExtractionSpec::new(vec![FieldKind::Email, FieldKind::Phone, FieldKind::Fax]);
        let raw = vec![
            ex(FieldKind::Email, "Sales@Acme.np", Method::TextPattern, 0.6),
            ex(FieldKind::Email, "hello@acme.np", Method::MailtoLink, 0.95),
            ex(FieldKind::Email, "sales@acme.np", Method::MailtoLink, 0.95),
            ex(FieldKind::Email, "logo@2x.png", Method::TextPattern, 0.6),
            ex(FieldKind::Phone, "+977 1 4412345", Method::TextPattern, 0.7),
            ex(FieldKind::Phone, "+977-1-4412345", Method::TelLink, 0.95),
        ];
        let (rec, removed, invalid) = finish_record("https://acme.np".into(), 2, raw, &spec);
        assert_eq!((removed, invalid), (2, 1));
        let emails: Vec<_> = rec.values(FieldKind::Email).collect();
        // Equal rank -> order of first appearance; the text-pattern "Sales@" was upgraded
        // to the mailto-sourced duplicate.
        assert_eq!(emails, vec!["sales@acme.np", "hello@acme.np"]);
        assert_eq!(rec.fields[&FieldKind::Email][0].method, Method::MailtoLink);
        assert_eq!(rec.fields[&FieldKind::Phone].len(), 1);
        assert_eq!(rec.fields[&FieldKind::Phone][0].value, "+97714412345");
        assert_eq!(rec.fields[&FieldKind::Phone][0].status, Validation::Valid);
        assert!(rec.fields[&FieldKind::Fax].is_empty(), "requested but not found stays empty");
    }

    #[test]
    fn validation_and_dedup_can_be_turned_off() {
        let mut spec = ExtractionSpec::new(vec![FieldKind::Email]);
        spec.validate = false;
        spec.deduplicate = false;
        let raw = vec![
            ex(FieldKind::Email, "a@acme.np", Method::MailtoLink, 0.95),
            ex(FieldKind::Email, "A@acme.np", Method::MailtoLink, 0.95),
            ex(FieldKind::Email, "logo@2x.png", Method::TextPattern, 0.6),
        ];
        let (rec, removed, invalid) = finish_record("https://acme.np".into(), 1, raw, &spec);
        assert_eq!((removed, invalid), (0, 0));
        assert_eq!(rec.fields[&FieldKind::Email].len(), 3);
        assert!(rec.fields[&FieldKind::Email].iter().all(|e| e.status == Validation::Unverified));
    }

    #[test]
    fn unrequested_fields_are_dropped() {
        let spec = ExtractionSpec::new(vec![FieldKind::Email]);
        let raw = vec![ex(FieldKind::Phone, "+97714412345", Method::TelLink, 0.9)];
        let (rec, ..) = finish_record("https://acme.np".into(), 1, raw, &spec);
        assert!(!rec.fields.contains_key(&FieldKind::Phone));
    }
}

#[cfg(test)]
mod keep_invalid_tests {
    use super::*;

    #[test]
    fn keep_invalid_flags_instead_of_dropping() {
        let mut spec = ExtractionSpec::new(vec![FieldKind::Email]);
        spec.keep_invalid = true;
        let raw = vec![
            Extracted::new(FieldKind::Email, "ok@acme.np", "s", Method::MailtoLink, 0.9),
            Extracted::new(FieldKind::Email, "logo@2x.png", "s", Method::TextPattern, 0.6),
        ];
        let (rec, removed, invalid) = finish_record("https://acme.np".into(), 1, raw, &spec);
        assert_eq!((removed, invalid), (0, 0), "nothing is discarded");
        let e = &rec.fields[&FieldKind::Email];
        assert_eq!((e[0].value.as_str(), e[0].status), ("ok@acme.np", Validation::Valid));
        assert_eq!((e[1].value.as_str(), e[1].status), ("logo@2x.png", Validation::Invalid), "invalid sorts last");
    }
}
