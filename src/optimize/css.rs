//! How a URL is written in CSS.

use std::fmt::Write;

/// What `url()` holds for `url`: the URL as it is, or as a string if it could not stand bare.
pub(super) fn url(url: &str) -> String {
    match url.contains(|c: char| c.is_whitespace() || "\"'()\\".contains(c)) {
        true => format!("\"{}\"", string(url)),
        false => url.to_string(),
    }
}

/// Escapes `text` for a double-quoted CSS string, as a data URL may hold quotes.
fn string(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' | '"' => out.extend(['\\', c]),
            // A raw line break would end the string, and the rules after it would be lost.
            c if c.is_control() => write!(out, "\\{:x} ", u32::from(c)).unwrap_or_default(),
            c => out.push(c),
        }
    }
    out
}
