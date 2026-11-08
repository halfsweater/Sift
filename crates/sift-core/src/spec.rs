//! The extraction specification and the (optional, deterministic) request interpreter.

use crate::field::{FieldKind, SOCIALS};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

fn yes() -> bool {
    true
}

/// Everything the extraction engine needs to know about *what* to collect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionSpec {
    /// Original natural-language request, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// What kind of business/page is being looked for ("software agencies").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Geographic hint ("Kathmandu"). Only used to build discovery queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub fields: Vec<FieldKind>,
    #[serde(default = "yes")]
    pub deduplicate: bool,
    #[serde(default = "yes")]
    pub validate: bool,
    /// Keep values that fail validation, flagged `invalid`, instead of dropping them.
    #[serde(default)]
    pub keep_invalid: bool,
    #[serde(default = "yes")]
    pub include_source: bool,
}

impl ExtractionSpec {
    pub fn new(fields: Vec<FieldKind>) -> Self {
        Self {
            query: None,
            subject: None,
            location: None,
            fields,
            deduplicate: true,
            validate: true,
            keep_invalid: false,
            include_source: true,
        }
    }

    pub fn wants(&self, field: FieldKind) -> bool {
        self.fields.contains(&field)
    }
}

/// Fields collected when a request names none explicitly.
pub const DEFAULT_FIELDS: [FieldKind; 4] =
    [FieldKind::CompanyName, FieldKind::Website, FieldKind::Email, FieldKind::Phone];

/// Turns free text into an [`ExtractionSpec`]. Implement this to plug in an
/// LLM-backed interpreter; the core engine never requires one.
pub trait RequestInterpreter {
    fn interpret(&self, request: &str) -> ExtractionSpec;
}

/// Keyword/regex based interpreter. Deterministic, offline, no external services.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedInterpreter;

struct Rule {
    field: FieldKind,
    re: Regex,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rules: &[(FieldKind, &str)] = &[
        (FieldKind::Email, r"e-?mails?(?:\s+address(?:es)?)?|mail\s+address(?:es)?"),
        (FieldKind::ContactForm, r"contact\s+forms?"),
        (FieldKind::Phone, r"phones?(?:\s+numbers?)?|telephones?|mobiles?|contact\s+numbers?"),
        (FieldKind::Fax, r"fax(?:es)?(?:\s+numbers?)?"),
        (FieldKind::Whatsapp, r"whats\s?app"),
        (FieldKind::CompanyName, r"(?:company|business|agency|organi[sz]ation)\s+names?|names?"),
        (FieldKind::Website, r"web\s?sites?|urls?|domains?"),
        (FieldKind::Address, r"(?:physical\s+|street\s+|postal\s+)?address(?:es)?"),
        (FieldKind::City, r"cit(?:y|ies)"),
        (FieldKind::Country, r"countr(?:y|ies)"),
        (FieldKind::Hours, r"(?:opening|business|operating|working)\s+hours|timings"),
        (FieldKind::Description, r"descriptions?"),
        (FieldKind::Category, r"categor(?:y|ies)|industr(?:y|ies)"),
        (FieldKind::Team, r"team(?:\s+members?)?|staff|employees"),
        (FieldKind::Services, r"services|products"),
        (FieldKind::Instagram, r"instagram"),
        (FieldKind::Facebook, r"facebook"),
        (FieldKind::Linkedin, r"linked\s?in"),
        (FieldKind::Twitter, r"twitter|x\.com"),
        (FieldKind::Youtube, r"youtube"),
        (FieldKind::Github, r"github"),
        (FieldKind::Tiktok, r"tik\s?tok"),
        (FieldKind::Title, r"page\s+titles?"),
        (FieldKind::Headings, r"headings?"),
        (FieldKind::Sitemaps, r"sitemaps?"),
        (FieldKind::Technologies, r"technolog(?:y|ies)|tech\s+stack"),
    ];
    rules
        .iter()
        .map(|(field, pat)| Rule { field: *field, re: Regex::new(&format!(r"(?i)\b(?:{pat})\b")).unwrap() })
        .collect()
});

static SOCIAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsocial(?:s|\s+media)?(?:\s+(?:profiles?|accounts?|links?|handles?))?\b").unwrap()
});
static LOCATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[Ii]n\s+([A-Z][\p{L}'’-]+(?:(?:,\s*|\s+)[A-Z][\p{L}'’-]+)*)").unwrap());
static SUBJECT_OF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:of|for|from)\s+(?:(?:every|each|all|the)\s+)?([^.,;]+?)(?:\s+in\s+[A-Z]|\s*[.,;]|$)").unwrap()
});
static SUBJECT_VERB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:please\s+)?(?:find|search\s+for|list|discover|look\s+for)\s+(?:me\s+)?(?:all\s+|the\s+|some\s+)?([^.,;]+?)(?:\s+in\s+[A-Z]|\s*[.,;]|$)",
    )
    .unwrap()
});

impl RequestInterpreter for RuleBasedInterpreter {
    fn interpret(&self, request: &str) -> ExtractionSpec {
        let mut found: Vec<(usize, FieldKind)> = Vec::new();

        // Blank out "social media profiles" so "links"/"profiles" can't leak elsewhere,
        // and remember where it was so field order follows the sentence.
        let mut text = request.to_string();
        if let Some(m) = SOCIAL_RE.find(request) {
            // Don't treat an explicit network list ("social: instagram") as "all networks".
            for (i, s) in SOCIALS.iter().enumerate() {
                found.push((m.start() * 10 + i, *s));
            }
            text.replace_range(m.range(), &" ".repeat(m.len()));
        }
        for rule in RULES.iter() {
            if let Some(m) = rule.re.find(&text) {
                found.push((m.start() * 10, rule.field));
            }
        }
        found.sort_by_key(|(pos, _)| *pos);
        let mut fields: Vec<FieldKind> = Vec::new();
        for (_, f) in found {
            if !fields.contains(&f) {
                fields.push(f);
            }
        }
        // "agency names" matches Company name, but "address" alone etc. are fine.
        if fields.is_empty() {
            fields = DEFAULT_FIELDS.to_vec();
        }

        let location = LOCATION_RE.captures(request).map(|c| c[1].trim().trim_end_matches(',').to_string());
        let subject = SUBJECT_OF_RE
            .captures(request)
            .or_else(|| SUBJECT_VERB_RE.captures(request))
            .map(|c| c[1].trim().to_string())
            .filter(|s| !s.is_empty());

        let mut spec = ExtractionSpec::new(fields);
        spec.query = Some(request.trim().to_string());
        spec.subject = subject;
        spec.location = location;
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interpret(s: &str) -> ExtractionSpec {
        RuleBasedInterpreter.interpret(s)
    }

    #[test]
    fn spec_example_from_the_brief() {
        let s = interpret("Get the email, phone number, address and Instagram account of every agency.");
        assert_eq!(s.fields, vec![FieldKind::Email, FieldKind::Phone, FieldKind::Address, FieldKind::Instagram]);
        assert_eq!(s.subject.as_deref(), Some("agency"));
        assert!(s.deduplicate && s.validate && s.include_source);
    }

    #[test]
    fn long_request_with_location() {
        let s = interpret(
            "Find software development agencies in Kathmandu. Collect their company names, websites, email addresses, phone numbers, physical addresses, and social media profiles.",
        );
        assert_eq!(s.location.as_deref(), Some("Kathmandu"));
        assert_eq!(s.subject.as_deref(), Some("software development agencies"));
        for f in [
            FieldKind::CompanyName,
            FieldKind::Website,
            FieldKind::Email,
            FieldKind::Phone,
            FieldKind::Address,
            FieldKind::Instagram,
            FieldKind::Linkedin,
        ] {
            assert!(s.wants(f), "missing {f}");
        }
        assert!(!s.wants(FieldKind::Fax));
        assert_eq!(s.fields[0], FieldKind::CompanyName);
    }

    #[test]
    fn subject_without_determiner_and_location_not_swallowed() {
        assert_eq!(interpret("emails of dentists").subject.as_deref(), Some("dentists"));
        let s = interpret("phone number of the dental clinics in Pokhara, Nepal");
        assert_eq!(s.subject.as_deref(), Some("dental clinics"));
        assert_eq!(s.location.as_deref(), Some("Pokhara, Nepal"));
    }

    #[test]
    fn nothing_recognised_falls_back_to_defaults() {
        assert_eq!(interpret("hello there").fields, DEFAULT_FIELDS.to_vec());
    }

    #[test]
    fn spec_json_defaults() {
        let s: ExtractionSpec = serde_json::from_str(r#"{"fields":["email"]}"#).unwrap();
        assert!(s.deduplicate && s.validate && s.include_source);
    }
}
