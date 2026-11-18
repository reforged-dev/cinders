//! The HTML minifier, a pass over lol_html: drops comments, collapses whitespace the way CSS
//! renders it, and shortens start tags. It leaves `<pre>`, `<textarea>`, `<script>`, and
//! `<style>` bodies alone.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use lol_html::html_content::{ContentType, Element, TextType};
use lol_html::{RewriteStrSettings, doc_comments, doc_text, element, end_tag};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Elements that are not inline boxes, so whitespace beside their tags never renders.
#[rustfmt::skip]
const BLOCK: &[&str] = &[
    "address", "article", "aside", "base", "blockquote", "body", "br", "caption", "col",
    "colgroup", "dd", "details", "dialog", "div", "dl", "dt", "fieldset", "figcaption", "figure",
    "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr",
    "html", "legend", "li", "link", "main", "menu", "meta", "nav", "ol", "optgroup", "option",
    "p", "pre", "search", "section", "summary", "table", "tbody", "td", "tfoot", "th", "thead",
    "title", "tr", "ul",
];
/// SVG and MathML elements whose content renders as text.
const FOREIGN_TEXT: &[&str] = &["text", "tspan", "textpath", "mi", "mn", "mo", "ms", "mtext"];

#[derive(Default)]
struct State {
    /// The depth inside `<pre>`, where text stays as written.
    pre: u32,
    /// Stripped whitespace that still renders: a newline if the run had one, else a space.
    pending: Option<char>,
    /// The output ends at a line start or a space, so leading whitespace is dropped.
    at_break: bool,
}

impl State {
    /// Handles a tag boundary and returns the pending whitespace to write before it, if any.
    /// `before` and `after` tell whether whitespace on that side of the tag can be dropped.
    /// Whitespace on each side of an inline tag stays, since an inline-block box keeps its
    /// edges.
    fn boundary(&mut self, before: bool, after: bool) -> Option<char> {
        let space = self.pending.take().filter(|_| !before && self.pre == 0);
        self.at_break = after;
        space
    }
}

/// Whether whitespace beside an element's tags can be dropped.
struct Sides {
    outside: bool,
    inside: bool,
}

fn sides(element: &Element) -> Sides {
    let name = element.tag_name();
    if element.namespace_uri() != HTML_NS {
        return match name.as_str() {
            "svg" | "math" => Sides { outside: false, inside: true },
            // SVG `a` can sit inside `text`, where the spaces beside it render.
            "tspan" | "textpath" | "a" => Sides { outside: false, inside: false },
            name if FOREIGN_TEXT.contains(&name) => Sides { outside: true, inside: false },
            _ => Sides { outside: true, inside: true },
        };
    }
    let block = BLOCK.contains(&name.as_str());
    Sides { outside: block, inside: block }
}

/// The start tag with each attribute in its shortest form.
fn start_tag(element: &Element) -> String {
    let foreign = element.namespace_uri() != HTML_NS;
    let mut tag = format!("<{}", element.tag_name_preserve_case());
    for attr in element.attributes() {
        // Inside `'` quotes a `"` is written as it is, which is shorter than its entity.
        let value = attr.value();
        let value = if value.contains('\'') { value } else { value.replace("&quot;", "\"") };
        tag.push(' ');
        tag.push_str(&attr.name_preserve_case());
        if value.is_empty() {
            continue; // `alt` parses the same as `alt=""`
        }
        tag.push('=');
        // Foreign tags keep their quotes, so a trailing `/>` cannot join an unquoted value.
        if !foreign && !value.bytes().any(|b| b" \t\n\x0c\r\"'=<>`".contains(&b)) {
            tag.push_str(&value);
        } else {
            let quote = if value.contains('"') { '\'' } else { '"' };
            tag.push(quote);
            tag.push_str(&value);
            tag.push(quote);
        }
    }
    tag.push_str(if foreign && element.is_self_closing() { "/>" } else { ">" });
    tag
}

pub(super) fn html(html: &str) -> Result<String> {
    // End tag handlers must own what they capture, so the state is shared by count.
    let state = Rc::new(RefCell::new(State { at_break: true, ..State::default() }));
    let text_state = state.clone();
    let settings = RewriteStrSettings::new()
        .append_element_content_handler(element!("*", move |element| {
            let Sides { outside, inside } = sides(element);
            let name = element.tag_name();
            let pre = u32::from(name == "pre");
            if let Some(space) = state.borrow_mut().boundary(outside, inside) {
                element.before(space.encode_utf8(&mut [0; 4]), ContentType::Text);
            }
            state.borrow_mut().pre += pre;
            // These tags are optional once the whitespace and comments beside them are gone.
            let optional_end = matches!(name.as_str(), "head" | "body" | "html");
            if name == "head" && element.attributes().is_empty() {
                element.start_tag().remove();
            } else {
                let tag = start_tag(element);
                element.start_tag().replace(&tag, ContentType::Html);
            }
            if element.can_have_content() {
                let state = state.clone();
                element.on_end_tag(end_tag!(move |end| {
                    let mut state = state.borrow_mut();
                    state.pre -= pre;
                    if let Some(space) = state.boundary(inside, outside) {
                        end.before(space.encode_utf8(&mut [0; 4]), ContentType::Text);
                    }
                    if optional_end {
                        end.remove();
                    }
                    Ok(())
                }))?;
            }
            Ok(())
        }))
        .append_document_content_handler(doc_comments!(|comment| {
            comment.remove();
            Ok(())
        }))
        .append_document_content_handler(doc_text!(move |text| {
            let mut state = text_state.borrow_mut();
            if state.pre > 0 || text.text_type() != TextType::Data {
                return Ok(());
            }
            let mut out = String::with_capacity(text.as_str().len());
            let mut space = state.pending;
            for c in text.as_str().chars() {
                if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                    space = Some(if c == '\n' || space == Some('\n') { '\n' } else { ' ' });
                    continue;
                }
                out.extend(space.take().filter(|_| !state.at_break));
                state.at_break = false;
                out.push(c);
            }
            state.pending = space.filter(|_| !state.at_break);
            text.set_str(out);
            Ok(())
        }));
    lol_html::rewrite_str(html, settings).map_err(|e| anyhow!("minifying HTML: {e}"))
}
