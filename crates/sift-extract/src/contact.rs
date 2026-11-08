//! Contact information: email, phone, fax, WhatsApp links, contact forms.

use crate::text::window_before;
use crate::{text_of, Ctx};
use regex::Regex;
use sift_core::{Extracted, FieldKind, Method};
use std::sync::LazyLock;

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}").unwrap());
static DEOBF_AT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\[\(\{]\s*at\s*[\]\)\}]\s*").unwrap());
static DEOBF_DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*[\[\(\{]\s*dot\s*[\]\)\}]\s*").unwrap());
static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\+?\(?\d(?:[ \u{a0}.\-()]{0,2}\d){6,}").unwrap());
static DATE_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\d{4}[-./]\d{1,2}[-./]\d{1,2}|\d{1,2}[-./]\d{1,2}[-./]\d{2,4}|(?:19|20)\d{2}[-–. ]+(?:19|20)\d{2})$",
    )
    .unwrap()
});

const PHONE_LABELS: [&str; 10] =
    ["tel", "phone", "call", "mobile", "cell", "whatsapp", "contact", "hotline", "landline", "reach"];

pub(crate) fn extract(ctx: &Ctx, out: &mut Vec<Extracted>) {
    if ctx.want(FieldKind::Email) {
        emails(ctx, out);
    }
    if ctx.want_any(&[FieldKind::Phone, FieldKind::Fax]) {
        phones(ctx, out);
    }
    if ctx.want(FieldKind::Whatsapp) {
        whatsapp(ctx, out);
    }
    if ctx.want(FieldKind::ContactForm) {
        contact_forms(ctx, out);
    }
}

fn percent_decode_basic(s: &str) -> String {
    s.replace("%40", "@").replace("%20", " ").replace("%2B", "+").replace("%2b", "+")
}

// ---------------------------------------------------------------- email

fn emails(ctx: &Ctx, out: &mut Vec<Extracted>) {
    // 1. mailto: links
    for a in ctx.doc.select(sel!("a[href]")) {
        let href = a.value().attr("href").unwrap_or_default().trim();
        if href.len() < 7 || !href[..7].eq_ignore_ascii_case("mailto:") {
            continue;
        }
        let addr_part = percent_decode_basic(href[7..].split('?').next().unwrap_or_default());
        for addr in addr_part.split(',') {
            let addr = addr.trim();
            if !addr.is_empty() {
                out.push(ctx.ex(FieldKind::Email, addr, Method::MailtoLink, 0.95));
            }
        }
    }
    // 2. Cloudflare "email protection" (public XOR obfuscation, decoded by every browser)
    for el in ctx.doc.select(sel!("[data-cfemail]")) {
        if let Some(addr) = el.value().attr("data-cfemail").and_then(decode_cfemail) {
            out.push(ctx.ex(FieldKind::Email, addr, Method::CfEmail, 0.9));
        }
    }
    // 3. Plain text
    let text = ctx.text();
    for m in EMAIL_RE.find_iter(text) {
        out.push(ctx.ex(FieldKind::Email, m.as_str(), Method::TextPattern, 0.6));
    }
    // 4. "name [at] host [dot] com"
    if text.to_ascii_lowercase().contains("at]") || text.to_ascii_lowercase().contains("at)") {
        let step = DEOBF_AT.replace_all(text, "@");
        let step = DEOBF_DOT.replace_all(&step, ".");
        for m in EMAIL_RE.find_iter(&step) {
            let addr = m.as_str();
            if !text.contains(addr) {
                out.push(ctx.ex(FieldKind::Email, addr, Method::Deobfuscated, 0.5));
            }
        }
    }
}

fn decode_cfemail(hex: &str) -> Option<String> {
    let bytes: Vec<u8> =
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
    let (&key, rest) = bytes.split_first()?;
    let decoded: Vec<u8> = rest.iter().map(|b| b ^ key).collect();
    String::from_utf8(decoded).ok().filter(|s| s.contains('@'))
}

// ---------------------------------------------------------------- phone / fax

#[derive(PartialEq)]
enum Label {
    Phone,
    Fax,
    None,
}

fn label_before(text: &str, start: usize) -> Label {
    let w = window_before(text, start, 22).to_lowercase();
    let fax = w.rfind("fax");
    let phone = PHONE_LABELS.iter().filter_map(|l| w.rfind(l)).max();
    match (fax, phone) {
        (Some(f), Some(p)) if p > f => Label::Phone,
        (Some(_), _) => Label::Fax,
        (None, Some(_)) => Label::Phone,
        _ => Label::None,
    }
}

fn phones(ctx: &Ctx, out: &mut Vec<Extracted>) {
    // tel: links
    if ctx.want(FieldKind::Phone) {
        for a in ctx.doc.select(sel!("a[href]")) {
            let href = a.value().attr("href").unwrap_or_default().trim();
            if href.len() > 4 && href[..4].eq_ignore_ascii_case("tel:") {
                let num = percent_decode_basic(href[4..].split([';', '?']).next().unwrap_or_default());
                let num = num.trim();
                if !num.is_empty() {
                    out.push(ctx.ex(FieldKind::Phone, num, Method::TelLink, 0.95));
                }
            }
        }
    }
    // free text
    let text = ctx.text();
    for m in PHONE_RE.find_iter(text) {
        let raw = m.as_str().trim_end_matches(['(', ')', '.', '-', ' ']);
        let raw = raw.trim_start_matches(')');
        let digits = raw.chars().filter(char::is_ascii_digit).count();
        if !(7..=15).contains(&digits) {
            continue;
        }
        // Part of a longer alphanumeric token (IDs, hashes, coordinates)?
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '.')
            || after.is_some_and(|c| c.is_alphanumeric())
        {
            continue;
        }
        if DATE_LIKE.is_match(raw) || before.is_some_and(|c| "$€£₹¥".contains(c)) {
            continue;
        }
        let international = raw.starts_with('+') || raw.starts_with("00");
        let label = label_before(text, m.start());
        let (field, conf) = match label {
            Label::Fax => (FieldKind::Fax, 0.6),
            Label::Phone if international => (FieldKind::Phone, 0.75),
            Label::Phone => (FieldKind::Phone, 0.6),
            Label::None if international => (FieldKind::Phone, 0.6),
            Label::None => {
                let formatted = raw.contains([' ', '-', '(', '.', '\u{a0}']);
                if digits >= 9 && formatted {
                    (FieldKind::Phone, 0.35)
                } else {
                    continue;
                }
            }
        };
        if ctx.want(field) {
            out.push(ctx.ex(field, raw, Method::TextPattern, conf));
        }
    }
}

// ---------------------------------------------------------------- whatsapp

fn whatsapp(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for a in ctx.doc.select(sel!("a[href]")) {
        let href = a.value().attr("href").unwrap_or_default().trim();
        let lower = href.to_ascii_lowercase();
        let phone: Option<String> = if let Some(rest) = lower.split("wa.me/").nth(1) {
            Some(rest.split(['?', '/', '#']).next().unwrap_or_default().to_string())
        } else if lower.contains("whatsapp.com/send") || lower.starts_with("whatsapp://send") {
            lower.split(['?', '&']).find_map(|kv| kv.strip_prefix("phone=")).map(str::to_string)
        } else {
            None
        };
        if let Some(p) = phone {
            let digits: String = p.chars().filter(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                out.push(ctx.ex(FieldKind::Whatsapp, format!("https://wa.me/{digits}"), Method::WhatsappLink, 0.9));
            }
        } else if lower.contains("chat.whatsapp.com/") {
            if let Some(u) = ctx.resolve(href) {
                out.push(ctx.ex(FieldKind::Whatsapp, u.as_str(), Method::WhatsappLink, 0.85));
            }
        }
    }
}

// ---------------------------------------------------------------- contact forms

fn contact_forms(ctx: &Ctx, out: &mut Vec<Extracted>) {
    for form in ctx.doc.select(sel!("form")) {
        let v = form.value();
        let marks_search = v.attr("role").is_some_and(|r| r.eq_ignore_ascii_case("search"))
            || form.select(sel!("input[type=search]")).next().is_some()
            || v.attr("action").is_some_and(|a| a.to_ascii_lowercase().contains("search"));
        if marks_search {
            continue;
        }
        let has_message = form.select(sel!("textarea")).next().is_some();
        let has_email = form.select(sel!("input[type=email]")).next().is_some()
            || form
                .select(sel!("input[name]"))
                .any(|i| i.value().attr("name").is_some_and(|n| n.to_ascii_lowercase().contains("email")));
        let named_contact = [v.attr("id"), v.attr("class"), v.attr("action")]
            .into_iter()
            .flatten()
            .any(|s| s.to_ascii_lowercase().contains("contact"));
        let newsletter = text_of(form).to_ascii_lowercase().contains("subscribe") && !has_message;
        if (has_message && (has_email || named_contact)) || (has_email && named_contact && !newsletter) {
            let action = v.attr("action").map(str::trim).filter(|a| !a.is_empty() && !a.starts_with('#'));
            let target = action.and_then(|a| ctx.resolve(a)).unwrap_or_else(|| ctx.base.clone());
            out.push(ctx.ex(FieldKind::ContactForm, target.as_str(), Method::HtmlElement, 0.75));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analyze;
    use crate::testutil::{page, values};
    use sift_core::{FieldKind as F, Method};

    fn run(html: &str, fields: &[F]) -> crate::PageAnalysis {
        analyze(&page("https://acme.np/contact", html), fields)
    }

    #[test]
    fn emails_from_mailto_text_and_obfuscation() {
        let a = run(
            r#"<body><a href="mailto:Hello@acme.np?subject=Hi">mail</a>
            <p>Sales: sales@acme.np.</p><p>Jobs: jobs [at] acme [dot] np</p>
            <script>var x="tracker@sentry.io"</script></body>"#,
            &[F::Email],
        );
        let v = values(&a, F::Email);
        assert!(v.contains(&"Hello@acme.np".to_string()));
        assert!(v.contains(&"sales@acme.np".to_string()), "{v:?}");
        assert!(v.contains(&"jobs@acme.np".to_string()), "{v:?}");
        assert!(!v.iter().any(|e| e.contains("sentry")), "script text must be ignored");
        let jobs = a.extractions.iter().find(|e| e.value == "jobs@acme.np").unwrap();
        assert_eq!(jobs.method, Method::Deobfuscated);
        assert!(jobs.confidence < 0.6);
        let mailto = a.extractions.iter().find(|e| e.method == Method::MailtoLink).unwrap();
        assert!(mailto.confidence > 0.9);
    }

    #[test]
    fn cloudflare_protected_email_is_decoded() {
        // "info@acme.np" XOR key 0x5a
        let key = 0x5au8;
        let hex: String =
            std::iter::once(key).chain("info@acme.np".bytes().map(|b| b ^ key)).map(|b| format!("{b:02x}")).collect();
        let a =
            run(&format!(r#"<a class="__cf_email__" data-cfemail="{hex}">[email&#160;protected]</a>"#), &[F::Email]);
        assert_eq!(values(&a, F::Email), vec!["info@acme.np"]);
    }

    #[test]
    fn phones_tel_links_labels_and_international() {
        let a = run(
            r#"<body><a href="tel:+977-1-4412345">call</a>
            <p>Mobile: 9801234567</p><p>Fax: +977 1 4412346</p>
            <p>Founded 2015-2024. Order #20240115 on 2024-01-15 for $1,234,567.</p>
            <p>International: +44 20 7946 0958</p></body>"#,
            &[F::Phone, F::Fax],
        );
        let phones = values(&a, F::Phone);
        assert!(phones.contains(&"+977-1-4412345".to_string()), "{phones:?}");
        assert!(phones.contains(&"9801234567".to_string()), "{phones:?}");
        assert!(phones.contains(&"+44 20 7946 0958".to_string()), "{phones:?}");
        assert_eq!(phones.len(), 3, "dates, years and prices are not phones: {phones:?}");
        assert_eq!(values(&a, F::Fax), vec!["+977 1 4412346"]);
        assert!(!phones.iter().any(|p| p.contains("4412346")), "fax number must not be listed as phone");
    }

    #[test]
    fn unlabeled_national_numbers_are_low_confidence() {
        let a = run("<p>Reach us on (01) 441-2345 anytime. Version 12345678 shipped.</p>", &[F::Phone]);
        let phones = values(&a, F::Phone);
        assert_eq!(phones, vec!["(01) 441-2345"]);
        assert!(a.extractions[0].confidence <= 0.6);
    }

    #[test]
    fn whatsapp_links() {
        let a = run(
            r#"<a href="https://wa.me/9779801234567?text=hi">w</a>
               <a href="https://api.whatsapp.com/send?phone=+977 980-1111111">x</a>
               <a href="https://example.org">no</a>"#,
            &[F::Whatsapp],
        );
        assert_eq!(values(&a, F::Whatsapp), vec!["https://wa.me/9779801234567", "https://wa.me/9779801111111"]);
    }

    #[test]
    fn contact_forms_exclude_search_and_newsletter() {
        let a = run(
            r#"<form action="/search"><input type="search" name="q"></form>
               <form action="/newsletter"><input type="email" name="email"><button>Subscribe</button></form>
               <form action="/send-message" method="post"><input type="email" name="email"><textarea name="msg"></textarea></form>"#,
            &[F::ContactForm],
        );
        assert_eq!(values(&a, F::ContactForm), vec!["https://acme.np/send-message"]);
    }

    #[test]
    fn only_requested_fields_are_extracted() {
        let a = run(r#"<a href="mailto:a@acme.np">a</a><a href="tel:+9779801234567">t</a>"#, &[F::Email]);
        assert!(a.extractions.iter().all(|e| e.field == F::Email));
    }
}
