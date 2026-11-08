//! The vocabulary of things Sift can extract.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A single extractable piece of information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    // Contact
    Email,
    Phone,
    Fax,
    Whatsapp,
    ContactForm,
    // Business
    CompanyName,
    Website,
    Description,
    Category,
    Address,
    City,
    Region,
    Country,
    Hours,
    Team,
    Services,
    // Web
    Title,
    MetaDescription,
    Headings,
    Links,
    Canonical,
    Instagram,
    Facebook,
    Linkedin,
    Twitter,
    Youtube,
    Github,
    Tiktok,
    // Technical
    StatusCode,
    ResponseTime,
    ContentType,
    RedirectChain,
    Technologies,
    Sitemaps,
}

/// Broad grouping used for documentation and crawl prioritisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldGroup {
    Contact,
    Business,
    Web,
    Technical,
}

const TABLE: &[(FieldKind, &str)] = &[
    (FieldKind::Email, "email"),
    (FieldKind::Phone, "phone"),
    (FieldKind::Fax, "fax"),
    (FieldKind::Whatsapp, "whatsapp"),
    (FieldKind::ContactForm, "contact_form"),
    (FieldKind::CompanyName, "company_name"),
    (FieldKind::Website, "website"),
    (FieldKind::Description, "description"),
    (FieldKind::Category, "category"),
    (FieldKind::Address, "address"),
    (FieldKind::City, "city"),
    (FieldKind::Region, "region"),
    (FieldKind::Country, "country"),
    (FieldKind::Hours, "hours"),
    (FieldKind::Team, "team"),
    (FieldKind::Services, "services"),
    (FieldKind::Title, "title"),
    (FieldKind::MetaDescription, "meta_description"),
    (FieldKind::Headings, "headings"),
    (FieldKind::Links, "links"),
    (FieldKind::Canonical, "canonical"),
    (FieldKind::Instagram, "instagram"),
    (FieldKind::Facebook, "facebook"),
    (FieldKind::Linkedin, "linkedin"),
    (FieldKind::Twitter, "twitter"),
    (FieldKind::Youtube, "youtube"),
    (FieldKind::Github, "github"),
    (FieldKind::Tiktok, "tiktok"),
    (FieldKind::StatusCode, "status_code"),
    (FieldKind::ResponseTime, "response_time"),
    (FieldKind::ContentType, "content_type"),
    (FieldKind::RedirectChain, "redirect_chain"),
    (FieldKind::Technologies, "technologies"),
    (FieldKind::Sitemaps, "sitemaps"),
];

/// Every social-profile field.
pub const SOCIALS: [FieldKind; 7] = [
    FieldKind::Instagram,
    FieldKind::Facebook,
    FieldKind::Linkedin,
    FieldKind::Twitter,
    FieldKind::Youtube,
    FieldKind::Github,
    FieldKind::Tiktok,
];

impl FieldKind {
    /// All known fields, in canonical order.
    pub fn all() -> impl Iterator<Item = FieldKind> {
        TABLE.iter().map(|(k, _)| *k)
    }

    /// Canonical snake_case name (matches the serde representation).
    pub fn as_str(self) -> &'static str {
        TABLE.iter().find(|(k, _)| *k == self).map(|(_, s)| *s).expect("every FieldKind is in TABLE")
    }

    pub fn group(self) -> FieldGroup {
        use FieldKind::*;
        match self {
            Email | Phone | Fax | Whatsapp | ContactForm => FieldGroup::Contact,
            CompanyName | Website | Description | Category | Address | City | Region | Country | Hours | Team
            | Services => FieldGroup::Business,
            Title | MetaDescription | Headings | Links | Canonical | Instagram | Facebook | Linkedin | Twitter
            | Youtube | Github | Tiktok => FieldGroup::Web,
            StatusCode | ResponseTime | ContentType | RedirectChain | Technologies | Sitemaps => FieldGroup::Technical,
        }
    }

    pub fn is_social(self) -> bool {
        SOCIALS.contains(&self)
    }

    /// Parse a comma separated list such as `"name, email,socials"`.
    /// `socials` expands to every social-profile field.
    pub fn parse_list(input: &str) -> Result<Vec<FieldKind>, UnknownField> {
        let mut out = Vec::new();
        for raw in input.split(',') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let norm = normalize_token(raw);
            let parsed: Vec<FieldKind> = match norm.as_str() {
                "socials" | "social" | "social_media" | "social_profiles" => SOCIALS.to_vec(),
                _ => vec![norm.parse()?],
            };
            for f in parsed {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        Ok(out)
    }
}

fn normalize_token(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown field `{0}` (run `sift --help` for the list of fields)")]
pub struct UnknownField(pub String);

impl FromStr for FieldKind {
    type Err = UnknownField;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = normalize_token(s);
        if let Some((k, _)) = TABLE.iter().find(|(_, name)| *name == norm) {
            return Ok(*k);
        }
        use FieldKind::*;
        Ok(match norm.as_str() {
            "name" | "company" | "business_name" | "org" | "organization" => CompanyName,
            "url" | "site" | "domain" | "homepage" => Website,
            "e_mail" | "mail" | "emails" => Email,
            "phone_number" | "phones" | "tel" | "telephone" | "mobile" | "contact_number" => Phone,
            "form" | "contact_forms" => ContactForm,
            "desc" | "about" | "business_description" => Description,
            "industry" | "sector" => Category,
            "addr" | "street_address" | "location" => Address,
            "opening_hours" | "business_hours" | "operating_hours" => Hours,
            "people" | "staff" | "team_members" => Team,
            "products" | "offerings" => Services,
            "page_title" => Title,
            "meta" | "meta_desc" => MetaDescription,
            "h1" | "heading" => Headings,
            "link" => Links,
            "canonical_url" => Canonical,
            "ig" => Instagram,
            "fb" => Facebook,
            "x" => Twitter,
            "status" | "http_status" => StatusCode,
            "latency" | "response_time_ms" => ResponseTime,
            "mime" | "mime_type" => ContentType,
            "redirects" => RedirectChain,
            "tech" | "tech_stack" => Technologies,
            "sitemap" | "sitemap_urls" => Sitemaps,
            _ => return Err(UnknownField(s.trim().to_string())),
        })
    }
}

impl fmt::Display for FieldKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_through_serde_and_fromstr() {
        for k in FieldKind::all() {
            assert_eq!(k.as_str().parse::<FieldKind>().unwrap(), k);
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
    }

    #[test]
    fn aliases_and_lists() {
        let f = FieldKind::parse_list("name, E-mail ,tel,socials,email").unwrap();
        assert_eq!(&f[..3], &[FieldKind::CompanyName, FieldKind::Email, FieldKind::Phone]);
        assert_eq!(f.len(), 3 + SOCIALS.len());
        assert!(FieldKind::parse_list("bogus").is_err());
    }
}
