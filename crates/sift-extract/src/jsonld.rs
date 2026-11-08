//! JSON-LD (`<script type="application/ld+json">`) helpers.

use scraper::Html;
use serde_json::Value;

/// All top-level JSON-LD nodes of a document, with `@graph` members flattened.
pub fn collect(doc: &Html) -> Vec<Value> {
    let mut out = Vec::new();
    for s in doc.select(sel!(r#"script[type="application/ld+json" i]"#)) {
        let raw: String = s.text().collect();
        let raw = raw.trim().trim_start_matches('\u{feff}');
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            flatten(v, &mut out);
        }
    }
    out
}

fn flatten(v: Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(items) => items.into_iter().for_each(|i| flatten(i, out)),
        Value::Object(mut map) => {
            let graph = map.remove("@graph");
            if map.contains_key("@type") {
                out.push(Value::Object(map));
            }
            if let Some(g) = graph {
                flatten(g, out);
            }
        }
        _ => {}
    }
}

/// `@type` as a list of names (schema.org URL prefixes removed).
pub fn types(node: &Value) -> Vec<String> {
    let clean = |s: &str| s.rsplit(['/', '#']).next().unwrap_or(s).to_string();
    match node.get("@type") {
        Some(Value::String(s)) => vec![clean(s)],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(clean).collect(),
        _ => vec![],
    }
}

/// Text of a scalar, or of an object's `name` / `@value`.
pub fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Object(o) => return o.get("name").or_else(|| o.get("@value")).and_then(text),
        _ => return None,
    };
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// A property that may hold one value or an array of values.
pub fn list<'a>(node: &'a Value, key: &str) -> Vec<&'a Value> {
    match node.get(key) {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v) => vec![v],
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_graph_and_arrays_and_ignores_bad_json() {
        let doc = Html::parse_document(
            r#"<script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"Organization","name":"A"},{"@type":["WebSite","Thing"],"name":"B"}]}</script>
               <script type="application/ld+json">[{"@type":"LocalBusiness","name":"C"}]</script>
               <script type="application/ld+json">{ not json </script>"#,
        );
        let nodes = collect(&doc);
        let names: Vec<_> = nodes.iter().filter_map(|n| text(&n["name"])).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        assert_eq!(types(&nodes[1]), vec!["WebSite", "Thing"]);
    }

    #[test]
    fn type_urls_are_cleaned() {
        let n: Value = serde_json::json!({"@type": "https://schema.org/LocalBusiness"});
        assert_eq!(types(&n), vec!["LocalBusiness"]);
    }
}
