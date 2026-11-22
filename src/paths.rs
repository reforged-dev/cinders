//! Path and URL rules that the render and optimize stages share.

use std::fmt::Write;
use std::fs::{self, DirEntry};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

/// A file's path relative to the site root, for messages and dev URLs.
pub fn name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).unwrap_or(file).to_string_lossy().into_owned()
}

/// A file's extension in lowercase, or nothing if it has none.
pub fn extension(file: &Path) -> String {
    file.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}

/// Resolves `.` and `..` without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => drop(out.pop()),
            Component::CurDir => {}
            part => out.push(part),
        }
    }
    out
}

/// Whether a reference starts with a URL scheme, such as `https:` or `mailto:`.
pub fn has_scheme(value: &str) -> bool {
    let end = value.find(':').unwrap_or(0);
    end > 0 && value[..end].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
}

/// Whether a `<meta>` of this `property` or `name` holds the image of a link preview, which
/// is a reference like any other.
pub fn is_preview_image(name: &str) -> bool {
    ["og:image", "twitter:image"].iter().any(|image| name.eq_ignore_ascii_case(image))
}

/// Decodes HTML character references and `%XX` percent-encoding in a reference path.
pub fn decode(raw: &str) -> String {
    percent_decode(&html_escape::decode_html_entities(raw))
}

/// Returns the byte offset just past the doctype, or 0 if there is not one. Only ASCII
/// whitespace and comments may precede a doctype without putting the browser in quirks mode.
pub fn after_doctype(html: &str) -> usize {
    let mut rest = html.trim_ascii_start();
    while let Some((_, after)) = rest.strip_prefix("<!--").and_then(|s| s.split_once("-->")) {
        rest = after.trim_ascii_start();
    }
    if !rest.get(..9).is_some_and(|start| start.eq_ignore_ascii_case("<!doctype")) {
        return 0;
    }
    rest.find('>').map_or(0, |end| html.len() - rest.len() + end + 1)
}

/// A reference in its two parts.
pub struct Reference<'a> {
    pub path: &'a str,
    /// The query and fragment, from their `?` or `#` on, or nothing.
    pub suffix: &'a str,
}

/// Splits a reference into its path and the query and fragment that follow it.
pub fn split(value: &str) -> Reference<'_> {
    // The `#` of a character reference such as `&#39;` starts no fragment.
    let reference = |i: usize| {
        value[..i].ends_with('&')
            && value[i + 1..].starts_with(|c: char| c.is_ascii_digit() || "xX".contains(c))
    };
    let mut ends = value.match_indices(['?', '#']).map(|(i, _)| i);
    let end = ends.find(|&i| value.as_bytes()[i] == b'?' || !reference(i));
    let (path, suffix) = value.split_at(end.unwrap_or(value.len()));
    Reference { path, suffix }
}

/// One candidate of a `srcset`.
pub struct Candidate<'a> {
    pub url: &'a str,
    /// What follows the URL, such as `2x` or `480w`, or nothing.
    pub descriptor: &'a str,
}

/// Splits a `srcset` into its candidates. A URL may hold commas, as whitespace ends it, so a
/// comma ends a candidate only after that.
pub fn srcset(value: &str) -> Vec<Candidate<'_>> {
    let between = |c: char| c.is_ascii_whitespace() || c == ',';
    let mut candidates = Vec::new();
    let mut rest = value.trim_start_matches(between);
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_ascii_whitespace()).unwrap_or(rest.len());
        let (url, after) = rest.split_at(end);
        // A comma at the end of the URL leaves the candidate no descriptor.
        let end = if url.ends_with(',') { 0 } else { after.find(',').unwrap_or(after.len()) };
        let (descriptor, after) = after.split_at(end);
        let url = url.trim_end_matches(',');
        candidates.push(Candidate { url, descriptor: descriptor.trim() });
        rest = after.trim_start_matches(between);
    }
    candidates
}

/// The entries of the directory `dir`, or none if it does not exist.
pub fn entries(dir: &Path) -> Result<Vec<DirEntry>> {
    match fs::read_dir(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        entries => entries.and_then(Iterator::collect).with_context(|| dir.display().to_string()),
    }
}

/// Escapes what a URL path cannot hold, and what would end a `srcset` or `url()` entry.
pub fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' | b'~' => {
                out.push(byte as char);
            }
            _ => write!(out, "%{byte:02X}").unwrap_or_default(),
        }
    }
    out
}

/// Decodes each `%XX` in a URL path. Anything else stays as written.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Both must be hex digits, since `from_str_radix` alone takes a sign, as in `%+A`.
        let hex = bytes.get(i + 1..i + 3).filter(|hex| hex.iter().all(u8::is_ascii_hexdigit));
        let hex = hex.and_then(|hex| str::from_utf8(hex).ok());
        match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            Some(byte) if bytes[i] == b'%' => {
                out.push(byte);
                i += 3;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
