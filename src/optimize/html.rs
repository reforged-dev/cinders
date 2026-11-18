//! Walks a rendered page and rewrites each reference to what the site ships.

use std::error::Error;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use lol_html::html_content::Element;
use lol_html::{RewriteStrSettings, element};

use super::{Optimizer, css};
use crate::paths;
use crate::site::Page;

/// Attributes that hold a single reference.
const REFERENCES: [&str; 5] = ["src", "href", "poster", "data", "xlink:href"];

/// Rewrites every reference in `page` to its output in the site.
pub(super) fn rewrite(optimizer: &Optimizer, page: &Page) -> Result<String> {
    let dir = page.source.parent().unwrap_or(Path::new(""));
    let settings = RewriteStrSettings::new()
        .append_element_content_handler(element!(
            r"[src], [href], [poster], [data], [xlink\:href], [srcset], [style]",
            |element| boxed(references(optimizer, dir, element))
        ))
        .append_element_content_handler(element!("meta[content]", |element| {
            boxed(preview_image(optimizer, dir, element))
        }));
    lol_html::rewrite_str(&page.html, settings).map_err(|e| anyhow!("{e}"))
}

fn references(optimizer: &Optimizer, dir: &Path, element: &mut Element) -> Result<()> {
    let tag = element.tag_name();
    for attr in REFERENCES {
        let Some(value) = element.get_attribute(attr) else { continue };
        let url = optimizer.reference(dir, &value).with_context(|| format!("<{tag} {attr}>"))?;
        if let Some(url) = url {
            element.set_attribute(attr, &url)?;
        }
    }
    if let Some(srcset) = element.get_attribute("srcset") {
        let mut candidates = Vec::new();
        for candidate in paths::srcset(&srcset) {
            let url = optimizer.reference(dir, candidate.url);
            let url = url.with_context(|| format!("<{tag} srcset>"))?;
            let url = url.as_deref().unwrap_or(candidate.url);
            candidates.push(format!("{url} {}", candidate.descriptor).trim().to_string());
        }
        element.set_attribute("srcset", &candidates.join(", "))?;
    }
    if let Some(style) = element.get_attribute("style") {
        let style = urls(optimizer, dir, &style).context("style attribute")?;
        element.set_attribute("style", &style)?;
    }
    Ok(())
}

/// Checks the image of a link preview, and writes its URL in full, as previews need.
fn preview_image(optimizer: &Optimizer, dir: &Path, element: &mut Element) -> Result<()> {
    let name = element.get_attribute("property").or_else(|| element.get_attribute("name"));
    let Some(name) = name.filter(|name| paths::is_preview_image(name)) else { return Ok(()) };
    let value = element.get_attribute("content").unwrap_or_default();
    // An external URL is already whole.
    let url = optimizer.rooted(dir, &value).with_context(|| format!("<meta {name}>"))?;
    let Some(url) = url else { return Ok(()) };
    let base = optimizer.config.base_url().with_context(|| {
        format!(
            "<meta {name}> needs [site] url in cinders.toml, since a link preview takes a full URL"
        )
    })?;
    element.set_attribute("content", &format!("{base}{url}"))?;
    Ok(())
}

/// Rewrites the `url()` references in a `style` attribute.
fn urls(optimizer: &Optimizer, dir: &Path, style: &str) -> Result<String> {
    let mut out = String::new();
    let mut rest = style;
    while let Some(start) = rest.find("url(").map(|i| i + 4) {
        let end = rest[start..].find(')').map_or(rest.len(), |i| start + i);
        // Attribute values arrive undecoded, so `&quot;` quotes must decode before trimming.
        let value = html_escape::decode_html_entities(rest[start..end].trim());
        let value = value.trim_matches(['"', '\'']);
        out.push_str(&rest[..start]);
        match optimizer.reference(dir, value)? {
            Some(url) => out.push_str(&css::url(&url)),
            None => out.push_str(&rest[start..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Keeps an error's whole context chain when lol_html boxes it.
fn boxed<T>(result: Result<T>) -> Result<T, Box<dyn Error + Send + Sync>> {
    result.map_err(|e| format!("{e:#}").into())
}
