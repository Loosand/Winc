//! Local article archives and RSS/Atom caches. The existing data lives in the `reader` directory beside the application state file.

/// RSS and Atom subscriptions and cached entries.
pub mod feed;
/// Saved articles, original content, and reading positions.
pub mod store;

pub use crate::web::{HttpFailure, fetch_http_with_referer};

fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation()
            && !matches!(c, ',' | ':' | ';' | '?' | '\'' | '"' | '/' | '=' | '@')
        {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Format selected text as a Markdown quotation with its source link.
pub fn quote_markdown(text: &str, url: &str, source_label: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let quoted = text
        .trim_matches(['\r', '\n'])
        .lines()
        .map(|line| format!("> {}", literal(line.trim_end())))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{quoted}\n\n{source_label}: <{url}>\n"))
}

#[cfg(test)]
mod tests;
