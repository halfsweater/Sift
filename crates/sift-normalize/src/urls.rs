//! URL normalisation used for crawl de-duplication and output cleanliness.

use url::Url;

fn is_tracking_param(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("utm_")
        || matches!(
            n.as_str(),
            "fbclid" | "gclid" | "dclid" | "msclkid" | "mc_cid" | "mc_eid" | "igshid" | "yclid" | "_ga" | "_gl"
        )
}

/// Clean a URL without changing what it points at: drops the fragment and
/// tracking parameters, sorts remaining query parameters, removes empty queries.
/// (The `url` crate already lower-cases scheme/host, strips default ports and
/// resolves `.`/`..` segments.)
pub fn normalize_url(url: &Url) -> Url {
    let mut u = url.clone();
    u.set_fragment(None);
    if u.query().is_some() {
        let mut pairs: Vec<(String, String)> = u
            .query_pairs()
            .filter(|(k, _)| !is_tracking_param(k))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        pairs.sort();
        if pairs.is_empty() {
            u.set_query(None);
        } else {
            u.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    u
}

/// Host without a leading `www.`, lower-cased. Two URLs on `www.acme.np` and
/// `acme.np` are considered the same site.
pub fn host_key(url: &Url) -> String {
    let h = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h).to_string();
    match url.port() {
        Some(p) => format!("{h}:{p}"),
        None => h,
    }
}

/// Identity of a page for duplicate detection. Ignores scheme (http/https),
/// `www.`, fragment, tracking params, query order and a trailing slash.
pub fn url_key(url: &Url) -> String {
    let n = normalize_url(url);
    let path = n.path().trim_end_matches('/');
    let mut key = format!("{}{}", host_key(&n), path);
    if let Some(q) = n.query() {
        key.push('?');
        key.push_str(q);
    }
    key
}

/// Canonical string form for presenting a URL in results: normalised, and
/// without the trailing slash of a bare origin (`https://acme.np`).
pub fn display_url(url: &Url) -> String {
    let n = normalize_url(url);
    let s = n.as_str();
    if n.path() == "/" && n.query().is_none() {
        s.trim_end_matches('/').to_string()
    } else {
        s.to_string()
    }
}

/// Parse text that may lack a scheme (`acme.np/contact`) into an http(s) URL.
pub fn parse_lenient(input: &str) -> Option<Url> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    let with_scheme = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let u = Url::parse(&with_scheme).ok()?;
    matches!(u.scheme(), "http" | "https").then_some(u).filter(|u| u.host_str().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn strips_fragment_and_tracking() {
        let n = normalize_url(&u("https://Acme.NP:443/a/../contact?utm_source=x&b=2&a=1&fbclid=z#team"));
        assert_eq!(n.as_str(), "https://acme.np/contact?a=1&b=2");
        assert_eq!(normalize_url(&u("https://acme.np/?utm_medium=mail")).as_str(), "https://acme.np/");
    }

    #[test]
    fn keys_unify_equivalent_urls() {
        let a = url_key(&u("http://www.acme.np/contact/"));
        let b = url_key(&u("https://acme.np/contact?utm_campaign=q#x"));
        assert_eq!(a, b);
        assert_ne!(a, url_key(&u("https://acme.np/contact-us")));
        assert_ne!(url_key(&u("https://acme.np/?page=1")), url_key(&u("https://acme.np/?page=2")));
        assert_eq!(url_key(&u("https://acme.np/?b=1&a=2")), url_key(&u("https://acme.np/?a=2&b=1")));
    }

    #[test]
    fn host_key_keeps_ports_distinct() {
        assert_ne!(host_key(&u("http://127.0.0.1:8000/")), host_key(&u("http://127.0.0.1:8001/")));
        assert_eq!(host_key(&u("https://WWW.Acme.np/")), "acme.np");
    }

    #[test]
    fn display_and_lenient_parse() {
        assert_eq!(display_url(&u("https://acme.np/")), "https://acme.np");
        assert_eq!(display_url(&u("https://acme.np/team/")), "https://acme.np/team/");
        assert_eq!(parse_lenient("acme.np/contact").unwrap().as_str(), "https://acme.np/contact");
        assert!(parse_lenient("ftp://acme.np").is_none());
        assert!(parse_lenient("").is_none());
    }
}
