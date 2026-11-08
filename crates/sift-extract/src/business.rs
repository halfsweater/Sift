//! Business information: names, descriptions, categories, addresses, hours, team, services.
//!
//! Structured data (JSON-LD, microdata, meta tags) is preferred. Anything inferred from
//! page layout is marked [`Method::Heuristic`] with low confidence, so it can never be
//! reported as validated.

use crate::jsonld;
use crate::{text_of, Ctx};
use scraper::{ElementRef, Node};
use serde_json::Value;
use sift_core::{Extracted, FieldKind as F, Method};

const ORG_HINTS: [&str; 24] = [
    "Organization",
    "Business",
    "Corporation",
    "Company",
    "Agency",
    "Store",
    "Restaurant",
    "Hotel",
    "ProfessionalService",
    "Dentist",
    "Physician",
    "Attorney",
    "School",
    "University",
    "Library",
    "Museum",
    "NGO",
    "Clinic",
    "Hospital",
    "LegalService",
    "Accountant",
    "Bank",
    "Gym",
    "Salon",
];
const GENERIC_TYPES: [&str; 8] =
    ["Organization", "LocalBusiness", "Corporation", "Thing", "WebSite", "WebPage", "Brand", "OnlineBusiness"];

fn is_org(types: &[String]) -> bool {
    types.iter().any(|t| ORG_HINTS.iter().any(|h| t.contains(h)))
}

pub(crate) fn extract(ctx: &Ctx, out: &mut Vec<Extracted>) {
    let wants_business = ctx.want_any(&[
        F::CompanyName,
        F::Description,
        F::Category,
        F::Address,
        F::City,
        F::Region,
        F::Country,
        F::Hours,
        F::Team,
        F::Services,
        F::Email,
        F::Phone,
        F::Fax,
    ]) || ctx.want_any(&crate::web::SOCIAL_FIELDS);
    if !wants_business {
        return;
    }
    for node in &ctx.jsonld {
        let types = jsonld::types(node);
        if is_org(&types) {
            org_node(ctx, node, &types, out);
        } else if types.iter().any(|t| t == "WebSite") && ctx.want(F::CompanyName) {
            if let Some(n) = node.get("name").and_then(jsonld::text) {
                out.push(ctx.ex(F::CompanyName, n, Method::JsonLd, 0.7));
            }
        }
    }
    meta_names(ctx, out);
    microdata(ctx, out);
    if ctx.want(F::Address) {
        address_elements(ctx, out);
    }
    if ctx.want(F::Description) {
        for (sel_, conf) in
            [(sel!(r#"meta[property="og:description"]"#), 0.7), (sel!(r#"meta[name="description" i]"#), 0.7)]
        {
            if let Some(c) = ctx.doc.select(sel_).next().and_then(|m| m.value().attr("content")) {
                let c = c.split_whitespace().collect::<Vec<_>>().join(" ");
                if !c.is_empty() {
                    out.push(ctx.ex(F::Description, c, Method::MetaTag, conf));
                }
            }
        }
    }
    if ctx.want(F::Category) {
        for m in ctx.doc.select(sel!(r#"meta[name="category" i], meta[name="classification" i]"#)) {
            if let Some(c) = m.value().attr("content").map(str::trim).filter(|c| !c.is_empty()) {
                out.push(ctx.ex(F::Category, c, Method::MetaTag, 0.6));
            }
        }
    }
    if ctx.want(F::Team) {
        team_elements(ctx, out);
    }
    if ctx.want(F::Services) {
        services(ctx, out);
    }
}

// ---------------------------------------------------------------- JSON-LD

fn org_node(ctx: &Ctx, node: &Value, types: &[String], out: &mut Vec<Extracted>) {
    let push = |out: &mut Vec<Extracted>, f: F, v: String, conf: f32| {
        if ctx.want(f) {
            out.push(ctx.ex(f, v, Method::JsonLd, conf));
        }
    };
    if let Some(n) = node.get("name").and_then(jsonld::text) {
        push(out, F::CompanyName, n, 0.9);
    }
    if let Some(n) = node.get("legalName").and_then(jsonld::text) {
        push(out, F::CompanyName, n, 0.85);
    }
    if let Some(d) = node.get("description").and_then(jsonld::text) {
        push(out, F::Description, d, 0.85);
    }
    if let Some(t) = types.iter().find(|t| !GENERIC_TYPES.contains(&t.as_str())) {
        push(out, F::Category, split_camel(t), 0.8);
    }
    contact_props(ctx, node, out);
    for cp in jsonld::list(node, "contactPoint") {
        contact_props(ctx, cp, out);
    }
    for a in jsonld::list(node, "address") {
        postal_address(ctx, a, Method::JsonLd, out);
    }
    for h in jsonld::list(node, "openingHours") {
        if let Some(s) = jsonld::text(h) {
            push(out, F::Hours, s, 0.85);
        }
    }
    for spec in jsonld::list(node, "openingHoursSpecification") {
        if let Some(s) = hours_spec(spec) {
            push(out, F::Hours, s, 0.85);
        }
    }
    for same in jsonld::list(node, "sameAs") {
        if let Some(u) = same.as_str().and_then(|s| ctx.resolve(s)) {
            if let Some((field, canon)) = crate::web::classify_social(&u) {
                push(out, field, canon, 0.85);
            }
        }
    }
    for key in ["employee", "member", "founder", "founders"] {
        for p in jsonld::list(node, key) {
            let Some(name) = p.get("name").and_then(jsonld::text) else { continue };
            let role = p.get("jobTitle").and_then(jsonld::text);
            push(out, F::Team, person_value(&name, role.as_deref()), 0.85);
        }
    }
}

fn contact_props(ctx: &Ctx, node: &Value, out: &mut Vec<Extracted>) {
    let mut push = |f: F, v: &Value, conf: f32| {
        for item in match v {
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        } {
            if let (true, Some(s)) = (ctx.want(f), jsonld::text(item)) {
                let s = s.strip_prefix("mailto:").or_else(|| s.strip_prefix("tel:")).unwrap_or(&s).to_string();
                out.push(ctx.ex(f, s, Method::JsonLd, conf));
            }
        }
    };
    if let Some(v) = node.get("telephone") {
        push(F::Phone, v, 0.9);
    }
    if let Some(v) = node.get("faxNumber") {
        push(F::Fax, v, 0.9);
    }
    if let Some(v) = node.get("email") {
        push(F::Email, v, 0.9);
    }
}

fn postal_address(ctx: &Ctx, a: &Value, method: Method, out: &mut Vec<Extracted>) {
    if let Some(s) = a.as_str() {
        if ctx.want(F::Address) {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if !s.is_empty() {
                out.push(ctx.ex(F::Address, s, method, 0.85));
            }
        }
        return;
    }
    let get = |k: &str| a.get(k).and_then(jsonld::text);
    let (street, city, region, postal, country) =
        (get("streetAddress"), get("addressLocality"), get("addressRegion"), get("postalCode"), get("addressCountry"));
    emit_address(ctx, [street.clone(), city.clone(), region.clone(), postal, country.clone()], method, 0.9, out);
    for (f, v) in [(F::City, city), (F::Region, region), (F::Country, country)] {
        if let (true, Some(v)) = (ctx.want(f), v) {
            out.push(ctx.ex(f, v, method, 0.9));
        }
    }
}

fn emit_address(ctx: &Ctx, parts: [Option<String>; 5], method: Method, conf: f32, out: &mut Vec<Extracted>) {
    if !ctx.want(F::Address) {
        return;
    }
    let joined = parts.into_iter().flatten().collect::<Vec<_>>().join(", ");
    if !joined.is_empty() {
        out.push(ctx.ex(F::Address, joined, method, conf));
    }
}

fn hours_spec(spec: &Value) -> Option<String> {
    let days: Vec<String> = jsonld::list(spec, "dayOfWeek")
        .into_iter()
        .filter_map(jsonld::text)
        .map(|d| d.rsplit('/').next().unwrap_or(&d).to_string())
        .collect();
    let opens = spec.get("opens").and_then(jsonld::text)?;
    let closes = spec.get("closes").and_then(jsonld::text)?;
    Some(if days.is_empty() { format!("{opens}-{closes}") } else { format!("{}: {opens}-{closes}", days.join(", ")) })
}

fn person_value(name: &str, role: Option<&str>) -> String {
    match role {
        Some(r) if !r.is_empty() => format!("{name} — {r}"),
        _ => name.to_string(),
    }
}

/// `ProfessionalService` -> `Professional Service`.
fn split_camel(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- meta / microdata / elements

fn meta_names(ctx: &Ctx, out: &mut Vec<Extracted>) {
    if !ctx.want(F::CompanyName) {
        return;
    }
    if let Some(c) =
        ctx.doc.select(sel!(r#"meta[property="og:site_name"]"#)).next().and_then(|m| m.value().attr("content"))
    {
        if !c.trim().is_empty() {
            out.push(ctx.ex(F::CompanyName, c.trim(), Method::MetaTag, 0.8));
        }
    }
    if let Some(c) =
        ctx.doc.select(sel!(r#"meta[name="application-name" i]"#)).next().and_then(|m| m.value().attr("content"))
    {
        if !c.trim().is_empty() {
            out.push(ctx.ex(F::CompanyName, c.trim(), Method::MetaTag, 0.6));
        }
    }
    if let Some(t) = ctx.doc.select(sel!("title")).next().map(text_of) {
        if let Some(n) = name_from_title(&t) {
            out.push(ctx.ex(F::CompanyName, n, Method::Heuristic, 0.4));
        }
    }
}

const GENERIC_TITLE_WORDS: [&str; 14] = [
    "home",
    "homepage",
    "welcome",
    "contact",
    "contact us",
    "about",
    "about us",
    "services",
    "official site",
    "official website",
    "index",
    "main",
    "blog",
    "untitled",
];

/// "Contact Us | Acme Technologies" -> "Acme Technologies". Returns `None` when no
/// segment looks like a name.
pub(crate) fn name_from_title(title: &str) -> Option<String> {
    let segments: Vec<&str> = title
        .split(['|', '–', '—', '·', '•', '«', '»'])
        .flat_map(|s| s.split(" - "))
        .flat_map(|s| s.split(": "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    segments
        .into_iter()
        .filter(|s| !GENERIC_TITLE_WORDS.contains(&s.to_lowercase().as_str()))
        .filter(|s| s.chars().count() <= 60)
        .min_by_key(|s| s.chars().count())
        .map(str::to_string)
}

fn microdata(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for addr in ctx.doc.select(sel!(r#"[itemtype*="PostalAddress"]"#)) {
        let prop = |name: &str| {
            addr.descendants()
                .filter_map(ElementRef::wrap)
                .find(|e| e.value().attr("itemprop") == Some(name))
                .map(|e| e.value().attr("content").map(str::to_string).unwrap_or_else(|| text_of(e)))
                .filter(|s| !s.is_empty())
        };
        let (street, city, region, postal, country) = (
            prop("streetAddress"),
            prop("addressLocality"),
            prop("addressRegion"),
            prop("postalCode"),
            prop("addressCountry"),
        );
        emit_address(
            ctx,
            [street, city.clone(), region.clone(), postal, country.clone()],
            Method::Microdata,
            0.85,
            out,
        );
        for (f, v) in [(F::City, city), (F::Region, region), (F::Country, country)] {
            if let (true, Some(v)) = (ctx.want(f), v) {
                out.push(ctx.ex(f, v, Method::Microdata, 0.85));
            }
        }
    }
    let itemprop = |name: &'static str, f: F, out: &mut Vec<Extracted>| {
        if !ctx.want(f) {
            return;
        }
        for el in ctx.doc.select(sel!("[itemprop]")) {
            if el.value().attr("itemprop") == Some(name) {
                let v = el.value().attr("content").map(str::to_string).unwrap_or_else(|| text_of(el));
                let v = v.trim().trim_start_matches("mailto:").trim_start_matches("tel:").to_string();
                if !v.is_empty() {
                    out.push(ctx.ex(f, v, Method::Microdata, 0.85));
                }
            }
        }
    };
    itemprop("telephone", F::Phone, out);
    itemprop("faxNumber", F::Fax, out);
    itemprop("email", F::Email, out);
    itemprop("openingHours", F::Hours, out);
}

/// `<address>` blocks: keep only text nodes that are not email/phone/label fragments.
fn address_elements(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for el in ctx.doc.select(sel!("address")) {
        if el.select(sel!(r#"[itemtype*="PostalAddress"]"#)).next().is_some() {
            continue; // microdata handled above
        }
        let parts: Vec<String> = el
            .descendants()
            .filter_map(|n| match n.value() {
                Node::Text(t) => Some(t.split_whitespace().collect::<Vec<_>>().join(" ")),
                _ => None,
            })
            .map(|s| s.trim_matches(|c: char| c == ',' || c.is_whitespace()).to_string())
            .filter(|s| {
                let digits = s.chars().filter(char::is_ascii_digit).count();
                let l = s.to_lowercase();
                !s.is_empty()
                    && !s.contains('@')
                    && !(digits >= 7 && digits * 2 > s.len())
                    && !matches!(
                        l.trim_end_matches(':'),
                        "phone" | "tel" | "email" | "e-mail" | "mobile" | "fax" | "address"
                    )
            })
            .collect();
        let joined = parts.join(", ");
        if !joined.is_empty() && joined.chars().count() <= 250 {
            out.push(ctx.ex(F::Address, joined, Method::Heuristic, 0.55));
        }
    }
}

const TEAM_CLASSES: [&str; 9] = [
    "team-member",
    "team_member",
    "teammember",
    "staff-member",
    "team-card",
    "member-card",
    "employee",
    "staff",
    "person",
];

fn team_elements(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for el in ctx.doc.select(sel!("[class], [itemtype]")) {
        let by_class = el.value().classes().any(|c| TEAM_CLASSES.contains(&c.to_lowercase().as_str()));
        let by_schema = el.value().attr("itemtype").is_some_and(|t| t.ends_with("/Person"));
        if !(by_class || by_schema) {
            continue;
        }
        let name = first_text(el, sel!(r#"[itemprop="name"], h2, h3, h4, h5"#));
        let role = first_text(
            el,
            sel!(
                r#"[itemprop="jobTitle"], [class*="role"], [class*="position"], [class*="job"], [class*="designation"]"#
            ),
        );
        if let Some(name) = name.filter(|n| n.chars().count() <= 60) {
            let method = if by_schema { Method::Microdata } else { Method::Heuristic };
            out.push(ctx.ex(
                F::Team,
                person_value(&name, role.as_deref().filter(|r| *r != name)),
                method,
                if by_schema { 0.8 } else { 0.5 },
            ));
        }
    }
}

fn first_text(el: ElementRef<'_>, sel_: &scraper::Selector) -> Option<String> {
    el.select(sel_).map(text_of).find(|t| !t.is_empty())
}

fn heading_level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
        _ => None,
    }
}

const SERVICE_HEADINGS: [&str; 8] =
    ["services", "our services", "solutions", "what we do", "products", "our products", "offerings", "what we offer"];

/// Items listed under a "Services"/"What we do" style heading.
fn services(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for h in ctx.doc.select(sel!("h1, h2, h3")) {
        let level = heading_level(h.value().name()).unwrap_or(2);
        let title = text_of(h).to_lowercase();
        let title = title.trim_matches(|c: char| !c.is_alphanumeric() && c != ' ');
        if !SERVICE_HEADINGS.contains(&title) {
            continue;
        }
        let mut count = 0;
        for sib in h.next_siblings().filter_map(ElementRef::wrap) {
            if heading_level(sib.value().name()).is_some_and(|l| l <= level) {
                break;
            }
            let mut items: Vec<ElementRef> = Vec::new();
            if heading_level(sib.value().name()).is_some() || sib.value().name() == "li" {
                items.push(sib);
            }
            items.extend(sib.select(sel!("h3, h4, h5, li")));
            for item in items {
                let t = text_of(item);
                if t.is_empty() || t.chars().count() > 120 {
                    continue;
                }
                let desc = if heading_level(item.value().name()).is_some() {
                    item.next_siblings()
                        .filter_map(ElementRef::wrap)
                        .next()
                        .filter(|n| n.value().name() == "p")
                        .map(text_of)
                        .filter(|d| !d.is_empty())
                } else {
                    None
                };
                let value = match desc {
                    Some(d) => format!("{t}: {}", d.chars().take(160).collect::<String>()),
                    None => t,
                };
                out.push(ctx.ex(F::Services, value, Method::Heuristic, 0.5));
                count += 1;
                if count >= 30 {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::name_from_title;
    use crate::analyze;
    use crate::testutil::{page, values};
    use sift_core::{FieldKind as F, Method};

    #[test]
    fn titles() {
        assert_eq!(name_from_title("Contact Us | Acme Technologies").as_deref(), Some("Acme Technologies"));
        assert_eq!(name_from_title("Acme – Home").as_deref(), Some("Acme"));
        assert_eq!(name_from_title("Home"), None);
    }

    const JSONLD_PAGE: &str = r#"<html><head><script type="application/ld+json">
    {"@context":"https://schema.org","@type":"ProfessionalService","name":"Himalaya Soft Pvt. Ltd.",
     "description":"Custom software studio.","telephone":"+977-1-4412345","email":"mailto:hello@himalayasoft.com.np",
     "address":{"@type":"PostalAddress","streetAddress":"Thamel Marg 12","addressLocality":"Kathmandu","addressCountry":"NP"},
     "openingHoursSpecification":[{"dayOfWeek":["https://schema.org/Monday","Tuesday"],"opens":"09:00","closes":"17:00"}],
     "sameAs":["https://www.facebook.com/himalayasoft","https://twitter.com/intent/tweet","https://example.org/x"],
     "employee":[{"@type":"Person","name":"Sita Rai","jobTitle":"CTO"}]}
    </script></head><body></body></html>"#;

    #[test]
    fn json_ld_organisation() {
        let all = [
            F::CompanyName,
            F::Description,
            F::Category,
            F::Address,
            F::City,
            F::Country,
            F::Hours,
            F::Team,
            F::Phone,
            F::Email,
            F::Facebook,
            F::Twitter,
        ];
        let a = analyze(&page("https://himalayasoft.com.np/", JSONLD_PAGE), &all);
        assert_eq!(values(&a, F::CompanyName), vec!["Himalaya Soft Pvt. Ltd."]);
        assert_eq!(values(&a, F::Category), vec!["Professional Service"]);
        assert_eq!(values(&a, F::Address), vec!["Thamel Marg 12, Kathmandu, NP"]);
        assert_eq!(values(&a, F::City), vec!["Kathmandu"]);
        assert_eq!(values(&a, F::Hours), vec!["Monday, Tuesday: 09:00-17:00"]);
        assert_eq!(values(&a, F::Team), vec!["Sita Rai — CTO"]);
        assert_eq!(values(&a, F::Phone), vec!["+977-1-4412345"]);
        assert_eq!(values(&a, F::Email), vec!["hello@himalayasoft.com.np"]);
        assert_eq!(values(&a, F::Facebook), vec!["https://www.facebook.com/himalayasoft"]);
        assert!(values(&a, F::Twitter).is_empty(), "share/intent URLs are not profiles");
        assert!(a.extractions.iter().filter(|e| e.field == F::Phone).all(|e| e.method == Method::JsonLd));
    }

    #[test]
    fn heuristics_are_labelled_as_such() {
        let html = r#"<html><head><title>About | Acme Labs</title></head><body>
        <address>Acme Labs<br>Baneshwor, Kathmandu<br>Phone: <a href="tel:+97714000000">+977 1 4000000</a></address>
        <div class="team-member"><h3>Ram Thapa</h3><span class="job-title">Founder</span></div>
        <div class="team-member"><h3>Gita Shrestha</h3></div>
        </body></html>"#;
        let a = analyze(&page("https://acme.np/about", html), &[F::CompanyName, F::Address, F::Team]);
        let name = a.extractions.iter().find(|e| e.field == F::CompanyName).unwrap();
        assert_eq!((name.value.as_str(), name.method), ("Acme Labs", Method::Heuristic));
        let addr = a.extractions.iter().find(|e| e.field == F::Address).unwrap();
        assert_eq!(addr.value, "Acme Labs, Baneshwor, Kathmandu");
        assert_eq!(addr.method, Method::Heuristic);
        assert_eq!(values(&a, F::Team), vec!["Ram Thapa — Founder", "Gita Shrestha"]);
    }

    #[test]
    fn microdata_address_and_services_section() {
        let html = r#"<body>
        <div itemscope itemtype="https://schema.org/PostalAddress"><span itemprop="streetAddress">Lazimpat</span>,
        <span itemprop="addressLocality">Kathmandu</span><meta itemprop="addressCountry" content="Nepal"></div>
        <h2>Our Services</h2>
        <div class="grid"><div><h3>Web Development</h3><p>Fast sites.</p></div><div><h3>Mobile Apps</h3></div></div>
        <h2>Contact</h2><ul><li>should not be a service</li></ul></body>"#;
        let a = analyze(&page("https://acme.np/", html), &[F::Address, F::City, F::Country, F::Services]);
        assert_eq!(values(&a, F::Address), vec!["Lazimpat, Kathmandu, Nepal"]);
        assert_eq!(values(&a, F::Country), vec!["Nepal"]);
        assert_eq!(values(&a, F::Services), vec!["Web Development: Fast sites.", "Mobile Apps"]);
    }

    #[test]
    fn description_and_site_name_from_meta() {
        let html = r#"<head><meta property="og:site_name" content="Acme"><meta name="description" content="  We build   things. "></head>"#;
        let a = analyze(&page("https://acme.np/", html), &[F::CompanyName, F::Description]);
        assert_eq!(values(&a, F::CompanyName), vec!["Acme"]);
        assert_eq!(values(&a, F::Description), vec!["We build things."]);
    }
}
