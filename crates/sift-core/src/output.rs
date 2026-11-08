//! Renderers for [`ResultSet`]: JSON, JSONL, CSV and a terminal table.

use crate::field::FieldKind;
use crate::model::{Record, ResultSet};
use std::fmt::Write as _;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Jsonl,
    Csv,
    Table,
}

impl OutputFormat {
    /// Guess a format from a file extension (`results.csv` -> Csv).
    pub fn from_path(path: &str) -> Option<Self> {
        let ext = path.rsplit('.').next()?.to_ascii_lowercase();
        ext.parse().ok()
    }
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "jsonl" | "ndjson" => Ok(Self::Jsonl),
            "csv" => Ok(Self::Csv),
            "table" | "txt" => Ok(Self::Table),
            other => Err(format!("unknown format `{other}` (expected json, jsonl, csv or table)")),
        }
    }
}

/// Table pagination settings.
#[derive(Debug, Clone, Copy)]
pub struct Pager {
    /// 1-based page number.
    pub page: usize,
    pub page_size: usize,
}

impl Default for Pager {
    fn default() -> Self {
        Self { page: 1, page_size: 25 }
    }
}

pub fn render(rs: &ResultSet, format: OutputFormat, pager: Pager) -> Result<String, RenderError> {
    match format {
        OutputFormat::Json => to_json(rs),
        OutputFormat::Jsonl => to_jsonl(rs),
        OutputFormat::Csv => to_csv(rs),
        OutputFormat::Table => Ok(to_table(rs, pager)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),
}

fn strip_sources(rs: &ResultSet) -> ResultSet {
    let mut out = rs.clone();
    for rec in &mut out.results {
        for list in rec.fields.values_mut() {
            for e in list {
                e.source.clear();
            }
        }
    }
    out
}

fn prepared(rs: &ResultSet) -> std::borrow::Cow<'_, ResultSet> {
    if rs.spec.include_source {
        std::borrow::Cow::Borrowed(rs)
    } else {
        std::borrow::Cow::Owned(strip_sources(rs))
    }
}

pub fn to_json(rs: &ResultSet) -> Result<String, RenderError> {
    Ok(serde_json::to_string_pretty(&*prepared(rs))?)
}

/// One record per line. Large crawls can be streamed with this format.
pub fn to_jsonl(rs: &ResultSet) -> Result<String, RenderError> {
    let rs = prepared(rs);
    let mut out = String::new();
    for rec in &rs.results {
        out.push_str(&serde_json::to_string(rec)?);
        out.push('\n');
    }
    Ok(out)
}

/// Columns: `website`, then each requested field (values joined with `; `), and
/// `<field>_source` columns when sources are included. Column order is the order of
/// `spec.fields`, so it is stable across runs.
pub fn to_csv(rs: &ResultSet) -> Result<String, RenderError> {
    let mut w = csv::Writer::from_writer(Vec::new());
    let mut header = vec!["website".to_string()];
    for f in &rs.spec.fields {
        header.push(f.to_string());
        if rs.spec.include_source {
            header.push(format!("{f}_source"));
        }
    }
    w.write_record(&header)?;
    for rec in &rs.results {
        let mut row = vec![rec.website.clone()];
        for f in &rs.spec.fields {
            let list = rec.fields.get(f).map(Vec::as_slice).unwrap_or(&[]);
            row.push(list.iter().map(|e| e.value.as_str()).collect::<Vec<_>>().join("; "));
            if rs.spec.include_source {
                row.push(list.iter().map(|e| e.source.as_str()).collect::<Vec<_>>().join("; "));
            }
        }
        w.write_record(&row)?;
    }
    let bytes = w.into_inner().map_err(|e| csv::Error::from(e.into_error()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn cell(rec: &Record, field: FieldKind, max: usize) -> String {
    let joined = rec.values(field).collect::<Vec<_>>().join("; ");
    if joined.is_empty() {
        return "-".to_string();
    }
    let single_line = joined.replace(['\n', '\r'], " ");
    if single_line.chars().count() > max {
        let cut: String = single_line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        single_line
    }
}

/// Human-readable table with a one-line summary and pagination footer.
pub fn to_table(rs: &ResultSet, pager: Pager) -> String {
    const MAX_CELL: usize = 44;
    let page_size = pager.page_size.max(1);
    let total = rs.results.len();
    let pages = total.div_ceil(page_size).max(1);
    let page = pager.page.clamp(1, pages);
    let start = (page - 1) * page_size;
    let slice = &rs.results[start.min(total)..(start + page_size).min(total)];

    // Columns with no data anywhere are summarised in the footer instead of wasting width.
    let wanted: Vec<FieldKind> = rs.spec.fields.iter().copied().filter(|f| *f != FieldKind::Website).collect();
    let has_data = |f: &FieldKind| rs.results.iter().any(|r| r.fields.get(f).is_some_and(|v| !v.is_empty()));
    let (fields, empty): (Vec<FieldKind>, Vec<FieldKind>) =
        if rs.results.is_empty() { (wanted, vec![]) } else { wanted.into_iter().partition(has_data) };
    let mut headers = vec!["website".to_string()];
    headers.extend(fields.iter().map(|f| f.to_string()));

    let rows: Vec<Vec<String>> = slice
        .iter()
        .map(|rec| {
            let mut row = vec![truncate(&rec.website, MAX_CELL)];
            row.extend(fields.iter().map(|f| cell(rec, *f, MAX_CELL)));
            row
        })
        .collect();

    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }

    let mut out = String::new();
    let line = |out: &mut String, cells: &[String]| {
        let parts: Vec<String> = cells.iter().enumerate().map(|(i, c)| format!("{c:<w$}", w = widths[i])).collect();
        let _ = writeln!(out, "│ {} │", parts.join(" │ "));
    };
    let rule = |l: &str, m: &str, r: &str| {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{l}{}{r}", segs.join(m))
    };
    let _ = writeln!(out, "{}", rule("┌", "┬", "┐"));
    line(&mut out, &headers);
    let _ = writeln!(out, "{}", rule("├", "┼", "┤"));
    if rows.is_empty() {
        let _ = writeln!(out, "│ {:<w$} │", "(no records)", w = widths.iter().sum::<usize>() + 3 * (widths.len() - 1));
    }
    for row in &rows {
        line(&mut out, row);
    }
    let _ = writeln!(out, "{}", rule("└", "┴", "┘"));
    if total > 0 {
        let _ = writeln!(
            out,
            "Showing {}-{} of {} records (page {}/{})",
            start + 1,
            start + slice.len(),
            total,
            page,
            pages
        );
    }
    if !empty.is_empty() {
        let names: Vec<String> = empty.iter().map(|f| f.to_string()).collect();
        let _ = writeln!(out, "No values found for: {}", names.join(", "));
    }
    out.push_str(&summary(rs));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// Extraction summary: crawl totals plus per-field coverage.
pub fn summary(rs: &ResultSet) -> String {
    let m = &rs.metadata;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Summary: {} record(s) · {} page(s) crawled · {} failed · {} blocked by robots.txt · {} duplicate(s) removed · {} invalid value(s) discarded · {:.1}s{}",
        m.records_found,
        m.pages_crawled,
        m.pages_failed,
        m.pages_blocked_by_robots,
        m.duplicates_removed,
        m.invalid_values_discarded,
        m.elapsed_ms as f64 / 1000.0,
        if m.cancelled { " · CANCELLED (partial results)" } else { "" }
    );
    let total = rs.results.len();
    if total > 0 {
        let cov: Vec<String> = rs
            .spec
            .fields
            .iter()
            .map(|f| {
                let n = rs.results.iter().filter(|r| r.fields.get(f).is_some_and(|v| !v.is_empty())).count();
                format!("{f} {n}/{total}")
            })
            .collect();
        let _ = writeln!(out, "Coverage: {}", cov.join(" · "));
    }
    for e in m.errors.iter().take(5) {
        let _ = writeln!(out, "Note: {e}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extracted, Metadata, Method};
    use crate::spec::ExtractionSpec;
    use std::collections::BTreeMap;

    fn sample() -> ResultSet {
        let mut fields = BTreeMap::new();
        fields.insert(
            FieldKind::Email,
            vec![
                Extracted::new(FieldKind::Email, "a@acme.np", "https://acme.np/contact", Method::MailtoLink, 0.9),
                Extracted::new(FieldKind::Email, "b@acme.np", "https://acme.np/", Method::TextPattern, 0.6),
            ],
        );
        fields.insert(FieldKind::Phone, vec![]);
        ResultSet {
            query: Some("q".into()),
            spec: ExtractionSpec::new(vec![FieldKind::Email, FieldKind::Phone]),
            results: vec![Record { website: "https://acme.np".into(), pages_visited: 2, fields }],
            metadata: Metadata { records_found: 1, pages_crawled: 2, ..Default::default() },
        }
    }

    #[test]
    fn csv_has_predictable_columns() {
        let csv = to_csv(&sample()).unwrap();
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "website,email,email_source,phone,phone_source");
        assert_eq!(
            lines.next().unwrap(),
            "https://acme.np,a@acme.np; b@acme.np,https://acme.np/contact; https://acme.np/,,"
        );
    }

    #[test]
    fn jsonl_is_one_record_per_line() {
        let out = to_jsonl(&sample()).unwrap();
        assert_eq!(out.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(v["fields"]["email"][0]["value"], "a@acme.np");
        assert_eq!(v["fields"]["phone"], serde_json::json!([]));
    }

    #[test]
    fn sources_can_be_omitted() {
        let mut rs = sample();
        rs.spec.include_source = false;
        assert!(!to_json(&rs).unwrap().contains("acme.np/contact"));
        assert_eq!(to_csv(&rs).unwrap().lines().next().unwrap(), "website,email,phone");
    }

    #[test]
    fn table_paginates_and_marks_missing() {
        let mut rs = sample();
        let rec = rs.results[0].clone();
        for i in 0..4 {
            let mut r = rec.clone();
            r.website = format!("https://s{i}.np");
            rs.results.push(r);
        }
        let t = to_table(&rs, Pager { page: 2, page_size: 2 });
        assert!(t.contains("Showing 3-4 of 5 records (page 2/3)"), "{t}");
        assert!(t.contains("No values found for: phone"), "{t}");
        rs.results[3].fields.insert(
            FieldKind::Phone,
            vec![Extracted::new(FieldKind::Phone, "+97714412345", "s", Method::TelLink, 0.9)],
        );
        let t = to_table(&rs, Pager { page: 1, page_size: 10 });
        assert!(t.contains("│ -"), "partially missing values render as -: {t}");
        let t = to_table(&rs, Pager { page: 2, page_size: 2 });
        assert!(t.contains("Coverage: email 5/5 · phone 1/5"));
    }

    #[test]
    fn format_detection() {
        assert_eq!(OutputFormat::from_path("a/b/out.CSV"), Some(OutputFormat::Csv));
        assert_eq!(OutputFormat::from_path("noext"), None);
    }
}
