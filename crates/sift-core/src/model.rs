//! Data model shared by every Sift stage.

use crate::field::FieldKind;
use crate::spec::ExtractionSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a value was found. Used to decide how much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    MailtoLink,
    TelLink,
    WhatsappLink,
    /// Decoded from a Cloudflare `data-cfemail` attribute.
    CfEmail,
    JsonLd,
    Microdata,
    MetaTag,
    /// A dedicated HTML element (`<title>`, `<address>`, `<h1>`, `<form>`, ...).
    HtmlElement,
    /// An `<a href>` pointing at a recognisable resource.
    Anchor,
    /// Regex match in visible text.
    TextPattern,
    /// Regex match after undoing "name [at] host [dot] com" style obfuscation.
    Deobfuscated,
    /// Derived from HTTP response data.
    HttpResponse,
    /// Value listed in a robots.txt / sitemap file.
    Sitemap,
    /// The URL the user supplied.
    Seed,
    /// A best-effort guess based on page structure or naming.
    Heuristic,
}

impl Method {
    /// Explicit machine-readable markup or protocol data, as opposed to a guess
    /// from free text. Only explicit values can ever be marked [`Validation::Valid`].
    pub fn is_explicit(self) -> bool {
        !matches!(self, Method::TextPattern | Method::Deobfuscated | Method::Heuristic)
    }
}

/// Outcome of the validation stage.
///
/// `Valid` means *syntactically valid and found in explicit markup* — Sift never
/// contacts mail servers or dials numbers, so nothing is ever "deliverable-verified".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validation {
    Invalid,
    Unverified,
    Valid,
}

/// One extracted value together with its provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extracted {
    pub field: FieldKind,
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub method: Method,
    pub status: Validation,
    pub confidence: f32,
}

impl Extracted {
    /// New values start `Unverified`; the validation stage decides the final status.
    pub fn new(
        field: FieldKind,
        value: impl Into<String>,
        source: impl Into<String>,
        method: Method,
        confidence: f32,
    ) -> Self {
        Self {
            field,
            value: value.into(),
            source: source.into(),
            method,
            status: Validation::Unverified,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

/// All data collected for one website (or one page, depending on scope).
///
/// A field that was requested but not found maps to an empty list — "missing"
/// is never represented by a made-up value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub website: String,
    pub pages_visited: usize,
    pub fields: BTreeMap<FieldKind, Vec<Extracted>>,
}

impl Record {
    pub fn values(&self, field: FieldKind) -> impl Iterator<Item = &str> {
        self.fields.get(&field).into_iter().flatten().map(|e| e.value.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub pages_crawled: usize,
    pub pages_failed: usize,
    pub pages_blocked_by_robots: usize,
    pub records_found: usize,
    pub duplicates_removed: usize,
    pub invalid_values_discarded: usize,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultSet {
    pub query: Option<String>,
    pub spec: ExtractionSpec,
    pub results: Vec<Record>,
    pub metadata: Metadata,
}
