//! Walks a rendered page twice: a scan for what the site draws and builds, then the rewrite
//! of each reference to what the site ships.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use lol_html::html_content::{ContentType, Element, TextType};
use lol_html::{
    ElementContentHandlers, RewriteStrSettings, Selector, doc_text, element, end_tag, text,
};

use super::asset::{Asset, Kind};
use super::reference::Target;
use super::{Optimizer, css, script};
use crate::Mode;
use crate::paths;
use crate::site::Page;

/// Attributes that hold a single reference.
const REFERENCES: [&str; 5] = ["src", "href", "poster", "data", "xlink:href"];
/// Attributes whose text a browser draws in the page's fonts.
const DRAWN: [&str; 3] = ["alt", "placeholder", "value"];

/// What a scan collects from a page.
#[derive(Default)]
pub(super) struct Found {
    /// The characters the page draws, for the font subset.
    pub(super) chars: BTreeSet<char>,
    /// The stylesheets to build before the rewrite.
    pub(super) styles: BTreeSet<PathBuf>,
    /// The images to encode before the rewrite.
    pub(super) images: BTreeSet<PathBuf>,
    /// Inline script bodies and non-bundled script `src` values (external URLs or files in
    /// `public/`), collected to fingerprint the site's scripts.
    pub(super) scripts: BTreeSet<String>,
    /// Executable scripts that do not carry `data-client-nav`, described for build notices.
    pub(super) unmarked: BTreeSet<String>,
}

impl Found {
    pub(super) fn merge(mut self, other: Found) -> Found {
        self.chars.extend(other.chars);
        self.styles.extend(other.styles);
        self.images.extend(other.images);
        self.scripts.extend(other.scripts);
        self.unmarked.extend(other.unmarked);
        self
    }
}

/// Collects the characters, stylesheets, images, and scripts `page` uses, and starts building
/// each local script file through `start`. Unresolved references are skipped here and reported
/// during `rewrite`.
pub(super) fn scan(optimizer: &Optimizer, page: &Page, start: &dyn Fn(PathBuf, Kind)) -> Found {
    let dir = page.source.parent().unwrap_or(Path::new(""));
    let depth = Rc::new(Cell::new(0));
    let found = RefCell::new(Found::default());
    let style = RefCell::new(String::new());
    let code = RefCell::new(String::new());
    let inline = Cell::new(Kind::Raw);
    let mut settings = RewriteStrSettings::new()
        .append_element_content_handler(picture_depth(Rc::clone(&depth)))
        .append_element_content_handler(element!("img[src], script[src], link[href]", |element| {
            let attr = if element.tag_name() == "link" { "href" } else { "src" };
            let kind = kind(optimizer, element, attr, depth.get());
            if matches!(kind, Kind::Raw | Kind::Font) {
                return Ok(());
            }
            let value = element.get_attribute(attr).unwrap_or_default();
            let mut found = found.borrow_mut();
            match (kind, optimizer.references.resolve(dir, &value)) {
                (Kind::Css, Ok(Target::File(file, _))) => drop(found.styles.insert(file)),
                (Kind::Image, Ok(Target::File(file, _))) => drop(found.images.insert(file)),
                (Kind::Module | Kind::Classic, Ok(Target::File(file, _))) => start(file, kind),
                (Kind::Module | Kind::Classic, Ok(_)) if !value.trim().is_empty() => {
                    found.scripts.insert(value);
                }
                _ => {}
            }
            Ok(())
        }))
        .append_element_content_handler(element!("script", |element| {
            let kind = element.get_attribute("type").unwrap_or_default().to_ascii_lowercase();
            let script = script_kind(&kind);
            let src = element.get_attribute("src");
            inline.set(if src.is_some() { Kind::Raw } else { script });
            if script != Kind::Raw
                && !element.has_attribute("data-client-nav")
                && !element.has_attribute("nomodule")
            {
                let label = match src {
                    Some(src) if src.trim().is_empty() => None,
                    Some(src) => Some(match optimizer.references.resolve(dir, &src) {
                        Ok(Target::File(file, _)) => paths::name(optimizer.root, &file),
                        _ => src,
                    }),
                    None => Some(format!(
                        "inline script in {}",
                        paths::name(optimizer.root, &page.source)
                    )),
                };
                if let Some(label) = label {
                    found.borrow_mut().unmarked.insert(label);
                }
            }
            Ok(())
        }));
    // Only an optimized build subsets fonts and fingerprints the site's scripts, so only it
    // needs the drawn characters and inline script bodies.
    if optimizer.mode == Mode::Optimized {
        settings = settings
            .append_element_content_handler(text!("style", |text| {
                style.borrow_mut().push_str(text.as_str());
                if text.last_in_text_node() {
                    found.borrow_mut().chars.extend(css::strings(&style.take()).chars());
                }
                Ok(())
            }))
            .append_element_content_handler(text!("script", |text| {
                if inline.get() != Kind::Raw {
                    code.borrow_mut().push_str(text.as_str());
                    if text.last_in_text_node() {
                        found.borrow_mut().scripts.insert(code.take());
                    }
                }
                Ok(())
            }))
            .append_element_content_handler(element!("[alt], [placeholder], [value]", |element| {
                for value in DRAWN.iter().filter_map(|attr| element.get_attribute(attr)) {
                    let drawn = html_escape::decode_html_entities(&value);
                    found.borrow_mut().chars.extend(drawn.chars());
                }
                Ok(())
            }))
            .append_document_content_handler(doc_text!(|text| {
                if matches!(text.text_type(), TextType::Data | TextType::RCData) {
                    let drawn = html_escape::decode_html_entities(text.as_str());
                    found.borrow_mut().chars.extend(drawn.chars());
                }
                Ok(())
            }));
    }
    lol_html::rewrite_str(&page.html, settings).map(drop).unwrap_or_default();
    found.into_inner()
}

/// Rewrites every reference in `page` to its output in the site, and builds its `<style>`
/// blocks and inline scripts.
pub(super) fn rewrite(optimizer: &Optimizer, page: &Page) -> Result<String> {
    let dir = page.source.parent().unwrap_or(Path::new(""));
    let depth = Rc::new(Cell::new(0));
    let style = RefCell::new(String::new());
    let code = RefCell::new(String::new());
    // How the current `<script>` is built, or `Kind::Raw` if it is not inline JavaScript.
    let inline = Cell::new(Kind::Raw);
    // The navigation `<script>` tag, consumed by the first `<head>` or inserted after the
    // doctype.
    let navigation = Cell::new(optimizer.navigation.as_deref());
    let settings = RewriteStrSettings::new()
        .append_element_content_handler(picture_depth(Rc::clone(&depth)))
        .append_element_content_handler(element!("head", |element| {
            // Prepend so the navigation script runs before any other module script.
            if let Some(tag) = navigation.take() {
                element.prepend(tag, ContentType::Html);
            }
            Ok(())
        }))
        .append_element_content_handler(element!(
            r"[src], [href], [poster], [data], [xlink\:href], [srcset], [style]",
            |element| boxed(references(optimizer, dir, element, depth.get()))
        ))
        .append_element_content_handler(element!("meta[content]", |element| {
            boxed(preview_image(optimizer, dir, element))
        }))
        .append_element_content_handler(element!("script", |element| {
            inline.set(inline_kind(element));
            Ok(())
        }))
        .append_element_content_handler(text!("style", |text| {
            style.borrow_mut().push_str(text.as_str());
            if !text.last_in_text_node() {
                text.remove();
                return Ok(());
            }
            let css = css::inline(optimizer, &style.take(), &page.source).context("<style>");
            // A `</style` inside a CSS string would end the element early.
            text.replace(&escape_close_tag(&boxed(css)?, "style"), ContentType::Html);
            Ok(())
        }))
        .append_element_content_handler(text!("script", |text| {
            if inline.get() == Kind::Raw {
                return Ok(());
            }
            code.borrow_mut().push_str(text.as_str());
            if !text.last_in_text_node() {
                text.remove();
                return Ok(());
            }
            let js = script::minify(&code.take(), inline.get(), optimizer.mode).context("<script>");
            // A `</script` inside a JS string or comment would end the element early.
            text.replace(&escape_close_tag(&boxed(js)?, "script"), ContentType::Html);
            Ok(())
        }));
    let mut html = lol_html::rewrite_str(&page.html, settings).map_err(|e| anyhow!("{e}"))?;
    // If the page does not have a `<head>` tag, insert the tag after the doctype (or at the
    // start).
    if let Some(tag) = navigation.take() {
        html.insert_str(paths::after_doctype(&html), tag);
    }
    Ok(html)
}

/// Keeps an inline `<style>` or `<script>` body from ending its element early by escaping
/// `</tag` as `<\/tag`. Matches only real close tags, not longer names such as `</script-editor>`.
fn escape_close_tag<'a>(text: &'a str, tag: &str) -> Cow<'a, str> {
    let bytes = text.as_bytes();
    let needle = tag.as_bytes();
    let len = needle.len() + 2;
    let is_end = |&b: &u8| b.is_ascii_whitespace() || matches!(b, b'/' | b'>');
    let mut out = String::new();
    let mut from = 0;
    let mut i = 0;
    while i + len <= bytes.len() {
        if &bytes[i..i + 2] == b"</"
            && bytes[i + 2..i + len].eq_ignore_ascii_case(needle)
            && bytes.get(i + len).is_none_or(is_end)
        {
            out.push_str(&text[from..i]);
            out.push_str("<\\/");
            from = i + 2;
            i += len;
        } else {
            i += 1;
        }
    }
    if out.is_empty() {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[from..]);
    Cow::Owned(out)
}

/// How the file that `attr` of `element` names is processed. `depth` counts the `<picture>`
/// elements around it.
fn kind(optimizer: &Optimizer, element: &Element, attr: &str, depth: u32) -> Kind {
    let get = |name| element.get_attribute(name).unwrap_or_default().to_ascii_lowercase();
    match (element.tag_name().as_str(), attr) {
        ("link", "href") => match get("rel").as_str() {
            "modulepreload" => Kind::Module,
            "preload" if get("as") == "style" => Kind::Css,
            "preload" if get("as") == "font" => Kind::Font,
            rel if rel.split_whitespace().any(|rel| rel == "stylesheet") => Kind::Css,
            _ => Kind::Raw,
        },
        ("script", "src") => script_kind(&get("type")),
        // A `<picture>` or a `srcset` the author wrote is a hand-tuned encode, and stays.
        ("img", "src") if depth == 0 && !element.has_attribute("srcset") => match optimizer.mode {
            Mode::Optimized => Kind::Image,
            Mode::Fast => Kind::Raw,
        },
        _ => Kind::Raw,
    }
}

/// Returns how an inline `<script>` element should be processed, or `Kind::Raw` if it has a
/// `src` attribute or is not JavaScript.
fn inline_kind(element: &Element) -> Kind {
    let kind = element.get_attribute("type").unwrap_or_default().to_ascii_lowercase();
    if element.has_attribute("src") { Kind::Raw } else { script_kind(&kind) }
}

/// How a `<script>` of the type `kind` is built. One that is not JavaScript, such as JSON,
/// stays as written.
fn script_kind(kind: &str) -> Kind {
    match kind.trim() {
        "module" => Kind::Module,
        ""
        | "text/javascript"
        | "application/javascript"
        | "text/ecmascript"
        | "application/ecmascript"
        | "text/x-javascript"
        | "application/x-javascript"
        | "text/x-ecmascript"
        | "application/x-ecmascript" => Kind::Classic,
        _ => Kind::Raw,
    }
}

fn references(optimizer: &Optimizer, dir: &Path, element: &mut Element, depth: u32) -> Result<()> {
    let tag = element.tag_name();
    for attr in REFERENCES {
        let Some(value) = element.get_attribute(attr) else { continue };
        let context = || format!("<{tag} {attr}>");
        let url = match optimizer.references.resolve(dir, &value).with_context(context)? {
            Target::Unchanged => continue,
            Target::Rooted(url) => url,
            Target::File(file, suffix) => {
                let kind = kind(optimizer, element, attr, depth);
                let asset = optimizer.asset(&file, kind).with_context(context)?;
                describe(element, &asset)?;
                format!("{}{suffix}", asset.url)
            }
        };
        let url = format!("{}{url}", optimizer.config.prefix());
        if url != value {
            element.set_attribute(attr, &url)?;
        }
    }
    if let Some(srcset) = element.get_attribute("srcset") {
        let mut candidates = Vec::new();
        for candidate in paths::srcset(&srcset) {
            let url = optimizer.reference(dir, candidate.url, Kind::Raw);
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

/// Makes `element` describe the processed file it names, where the file's format or size is
/// not the source's.
fn describe(element: &mut Element, asset: &Asset) -> Result<()> {
    // A subset font is WOFF2 whatever its source was, so a preload's type hint follows.
    if asset.url.ends_with(".woff2") && element.has_attribute("type") {
        element.set_attribute("type", "font/woff2")?;
    }
    if let Some((width, height)) = asset.size
        && !element.has_attribute("width")
        && !element.has_attribute("height")
    {
        element.set_attribute("width", &width.to_string())?;
        element.set_attribute("height", &height.to_string())?;
    }
    Ok(())
}

/// Checks the image of a link preview, and writes its URL in full, as previews need.
fn preview_image(optimizer: &Optimizer, dir: &Path, element: &mut Element) -> Result<()> {
    let name = element.get_attribute("property").or_else(|| element.get_attribute("name"));
    let Some(name) = name.filter(|name| paths::is_preview_image(name)) else { return Ok(()) };
    let value = element.get_attribute("content").unwrap_or_default();
    // Previews do not reliably decode AVIF, so the image ships as it is. An external URL is
    // already whole.
    let url = optimizer.rooted(dir, &value, Kind::Raw).with_context(|| format!("<meta {name}>"))?;
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
        match optimizer.reference(dir, value, Kind::Raw)? {
            Some(url) => out.push_str(&css::url(&url)),
            None => out.push_str(&rest[start..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Counts in `depth` the `<picture>` elements that enclose the current element.
fn picture_depth<'h>(depth: Rc<Cell<u32>>) -> (Cow<'static, Selector>, ElementContentHandlers<'h>) {
    element!("picture", move |element| {
        depth.set(depth.get() + 1);
        let depth = Rc::clone(&depth);
        if let Some(handlers) = element.end_tag_handlers() {
            handlers.push(end_tag!(move |_| {
                depth.set(depth.get() - 1);
                Ok(())
            }));
        }
        Ok(())
    })
}

/// Keeps an error's whole context chain when lol_html boxes it.
fn boxed<T>(result: Result<T>) -> Result<T, Box<dyn Error + Send + Sync>> {
    result.map_err(|e| format!("{e:#}").into())
}
