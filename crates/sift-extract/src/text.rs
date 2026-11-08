//! Visible-text extraction.

use scraper::{Html, Node};

fn hidden_tag(name: &str) -> bool {
    matches!(name, "script" | "style" | "noscript" | "template" | "svg" | "head")
}

/// All human-visible text nodes, separated by single spaces. `<script>`, `<style>`,
/// `<noscript>`, `<template>`, `<svg>` and `<head>` content is skipped.
pub fn visible_text(doc: &Html) -> String {
    let mut out = String::new();
    for node in doc.tree.root().descendants() {
        let Node::Text(t) = node.value() else { continue };
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        let hidden = node.ancestors().any(|a| a.value().as_element().is_some_and(|e| hidden_tag(e.name())));
        if !hidden {
            out.push_str(t);
            out.push(' ');
        }
    }
    out
}

/// Up to `n` chars of `text` ending at byte offset `end`, respecting char boundaries.
pub fn window_before(text: &str, end: usize, n: usize) -> &str {
    let head = &text[..end];
    let start = head.char_indices().rev().nth(n.saturating_sub(1)).map_or(0, |(i, _)| i);
    &head[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_scripts_and_styles() {
        let d = Html::parse_document(
            "<html><head><title>T</title><style>.a{}</style></head><body><p>Hello <b>wor</b>ld</p><script>var email='x@y.com'</script><noscript>no</noscript></body></html>",
        );
        let t = visible_text(&d);
        assert!(t.contains("Hello"));
        assert!(!t.contains("x@y.com") && !t.contains(".a{}") && !t.contains("no ") && !t.contains("T "));
    }

    #[test]
    fn window_is_char_boundary_safe() {
        let s = "héllo wörld tel: 123";
        let end = s.len();
        assert_eq!(window_before(s, end, 8), "tel: 123");
        assert_eq!(window_before("ab", 2, 10), "ab");
    }
}
