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
}

impl Found {
    pub(super) fn merge(mut self, other: Found) -> Found {
        self.chars.extend(other.chars);
        self.styles.extend(other.styles);
        self.images.extend(other.images);
        self
    }
}

/// Collects what `page` draws and builds, and passes each module script to `start`. A
/// reference that fails to resolve is skipped here and reported by the rewrite.
pub(super) fn scan(optimizer: &Optimizer, page: &Page, start: &dyn Fn(PathBuf)) -> Found {
    let dir = page.source.parent().unwrap_or(Path::new(""));
    let depth = Rc::new(Cell::new(0));
    let found = RefCell::new(Found::default());
    let style = RefCell::new(String::new());
    let mut settings = RewriteStrSettings::new()
        .append_element_content_handler(picture_depth(Rc::clone(&depth)))
        .append_element_content_handler(element!("img[src], script[src], link[href]", |element| {
            let attr = if element.tag_name() == "link" { "href" } else { "src" };
            let kind = kind(optimizer, element, attr, depth.get());
            let value = element.get_attribute(attr).unwrap_or_default();
            if matches!(kind, Kind::Css | Kind::Module | Kind::Image)
                && let Ok(Target::File(file, _)) = optimizer.references.resolve(dir, &value)
            {
                let mut found = found.borrow_mut();
                match kind {
                    Kind::Module => start(file),
                    Kind::Css => drop(found.styles.insert(file)),
                    _ => drop(found.images.insert(file)),
                }
            }
            Ok(())
        }));
    // Only an optimized build subsets fonts, so only it needs the characters.
    if optimizer.mode == Mode::Optimized {
        settings = settings
            .append_element_content_handler(text!("style", |text| {
                style.borrow_mut().push_str(text.as_str());
                if text.last_in_text_node() {
                    found.borrow_mut().chars.extend(css::strings(&style.take()).chars());
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
    let settings = RewriteStrSettings::new()
        .append_element_content_handler(picture_depth(Rc::clone(&depth)))
        .append_element_content_handler(element!(
            r"[src], [href], [poster], [data], [xlink\:href], [srcset], [style]",
            |element| boxed(references(optimizer, dir, element, depth.get()))
        ))
        .append_element_content_handler(element!("meta[content]", |element| {
            boxed(preview_image(optimizer, dir, element))
        }))
        .append_element_content_handler(element!("script", |element| {
            let kind = element.get_attribute("type").unwrap_or_default().to_ascii_lowercase();
            inline.set(if element.has_attribute("src") { Kind::Raw } else { script_kind(&kind) });
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
            text.replace(&boxed(css)?.replace("</style", "<\\/style"), ContentType::Html);
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
            text.replace(&boxed(js)?, ContentType::Html);
            Ok(())
        }));
    lol_html::rewrite_str(&page.html, settings).map_err(|e| anyhow!("{e}"))
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

/// How a `<script>` of the type `kind` is built. One that is not JavaScript, such as JSON,
/// stays as written.
fn script_kind(kind: &str) -> Kind {
    match kind {
        "module" => Kind::Module,
        "" | "text/javascript" => Kind::Classic,
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
