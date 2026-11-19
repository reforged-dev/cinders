//! Parses the template language: HTML with `{expr}` holes, blocks, and components.

use std::fmt::{self, Display, Formatter};
use std::mem;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::eval::Value;
use crate::paths;

const SUPPORTED: &str = "expressions are literals, property paths, comparisons, &&, ||, and !. To \
                         print a brace, as in a code sample, write {\"{\"}";
/// Elements with no end tag, which open nothing.
const VOID: [&str; 13] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];
const BINARY: [&[&str]; 3] = [&["||"], &["&&"], &["==", "!=", "<=", ">=", "<", ">"]];

pub(super) struct Template {
    pub(super) nodes: Vec<Node>,
    /// The file's path relative to the site root, for messages.
    pub(super) name: String,
    /// The file's directory, which the relative paths it writes resolve against.
    pub(super) dir: PathBuf,
}

pub(super) enum Node {
    Text(String),
    Print(Hole),
    /// The arms of an `{#if}`, each a condition and a body. `{:else}` is an arm that is true.
    If(Vec<(Hole, Vec<Node>)>),
    Each {
        list: Hole,
        item: String,
        index: Option<String>,
        body: Vec<Node>,
    },
    Tag(Tag),
    /// A `<script>` or `<style>` element. `hoist` marks one at the top level of its file.
    Raw {
        tag: Tag,
        body: String,
        hoist: bool,
    },
    Component {
        tag: Tag,
        children: Vec<Node>,
        line: usize,
    },
    Slot,
}

pub(super) struct Tag {
    pub(super) name: String,
    pub(super) attrs: Vec<Attr>,
    pub(super) self_closing: bool,
}

pub(super) struct Attr {
    pub(super) name: String,
    pub(super) value: AttrValue,
}

pub(super) enum AttrValue {
    /// A name alone, as in `<input disabled>`.
    Bare,
    /// An expression, as in `href={link.url}`.
    Expr(Hole),
    /// Text, which may hold `Text`, `Print`, `If`, and `Each` nodes.
    Text(Vec<Node>),
}

/// An expression with its source and line, for messages.
pub(super) struct Hole {
    pub(super) expr: Expr,
    pub(super) source: String,
    pub(super) line: usize,
}

pub(super) enum Expr {
    Literal(Value),
    Path(Vec<String>),
    Not(Box<Expr>),
    /// `||`, `&&`, or a comparison.
    Binary {
        operator: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

/// What ended a run of nodes.
#[derive(PartialEq)]
enum End<'s> {
    Eof,
    /// `{:else}`, or `{:else if}` with its condition.
    Else(Option<&'s str>),
    If,
    Each,
    /// A component's end tag, with its name.
    Close(&'s str),
    /// The closing quote of an attribute value.
    Quote,
}

/// What the parser found at a special character.
enum Step<'s> {
    Text(&'s str),
    Node(Node),
    End(End<'s>),
}

struct Parser<'s> {
    source: &'s str,
    pos: usize,
    name: &'s str,
    /// How many elements, blocks, and components are open. Only a top-level `<style>` or
    /// `<script>` hoists.
    depth: usize,
}

impl Template {
    /// Parses `source`, the text of the template `file` of the site in `root`.
    pub(super) fn parse(root: &Path, file: &Path, source: &str) -> Result<Template> {
        let name = paths::name(root, file);
        let mut parser = Parser { source, pos: 0, name: &name, depth: 0 };
        let (nodes, end) = parser.nodes(None)?;
        if end != End::Eof {
            return Err(parser.error(parser.pos, format!("unexpected {end}")));
        }
        Ok(Template { nodes, name, dir: file.parent().unwrap_or(root).to_path_buf() })
    }
}

impl Display for End<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            End::Eof => f.write_str("end of file"),
            End::Else(_) => f.write_str("{:else}"),
            End::If => f.write_str("{/if}"),
            End::Each => f.write_str("{/each}"),
            End::Close(name) => write!(f, "</{name}>"),
            End::Quote => f.write_str("closing quote"),
        }
    }
}

impl<'s> Parser<'s> {
    fn line(&self, pos: usize) -> usize {
        self.source[..pos].matches('\n').count() + 1
    }

    fn error(&self, pos: usize, message: impl Display) -> anyhow::Error {
        anyhow!("{}:{}: {message}", self.name, self.line(pos))
    }

    fn rest(&self) -> &'s str {
        &self.source[self.pos..]
    }

    /// Consumes the characters that `keep` accepts.
    fn take(&mut self, keep: impl Fn(char) -> bool) -> &'s str {
        let rest = self.rest();
        let end = rest.find(|c| !keep(c)).unwrap_or(rest.len());
        self.pos += end;
        &rest[..end]
    }

    /// Parses nodes up to the end of the file or a terminator, which it returns. Inside an
    /// attribute value quoted with `quote`, only braces and the closing quote are special.
    fn nodes(&mut self, quote: Option<char>) -> Result<(Vec<Node>, End<'s>)> {
        let mut nodes = Vec::new();
        let mut text = String::new();
        // Ends a run of text. Values are written in double quotes, whichever quotes the author
        // used.
        let flush = |text: &mut String, nodes: &mut Vec<Node>| {
            if quote == Some('\'') {
                *text = text.replace('"', "&quot;");
            }
            if !text.is_empty() {
                nodes.push(Node::Text(mem::take(text)));
            }
        };
        loop {
            let rest = self.rest();
            let special = rest.find([quote.unwrap_or('<'), '{']).unwrap_or(rest.len());
            text.push_str(&rest[..special]);
            self.pos += special;
            let step = match quote {
                _ if self.rest().is_empty() => Step::End(End::Eof),
                _ if self.rest().starts_with('{') => self.braces(quote)?,
                Some(_) => {
                    self.pos += 1;
                    Step::End(End::Quote)
                }
                None => self.markup()?,
            };
            match step {
                Step::Text(literal) => text.push_str(literal),
                Step::Node(node) => {
                    flush(&mut text, &mut nodes);
                    nodes.push(node);
                }
                Step::End(end) => {
                    flush(&mut text, &mut nodes);
                    return Ok((nodes, end));
                }
            }
        }
    }

    /// Parses a block's or a component's body, which opened at `start`, up to the terminator
    /// `end`.
    fn body(&mut self, quote: Option<char>, end: End, start: usize) -> Result<Vec<Node>> {
        let outer = self.depth;
        self.depth += 1;
        let (nodes, found) = self.nodes(quote)?;
        self.depth = outer;
        self.expect(found, end, start)?;
        Ok(nodes)
    }

    /// Checks that what opened at `start` ended with `end`.
    fn expect(&self, found: End, end: End, start: usize) -> Result<()> {
        let open = match end {
            End::If => "{#if}".to_string(),
            End::Each => "{#each}".to_string(),
            End::Close(name) => format!("<{name}>"),
            _ => "attribute value".to_string(),
        };
        match found {
            _ if found == end => Ok(()),
            End::Eof => Err(self.error(start, format!("unclosed {open}"))),
            _ => Err(self.error(self.pos, format!("unexpected {found}, expected {end}"))),
        }
    }

    /// Parses the `{...}` at the cursor: a hole, a block, or a block's terminator.
    fn braces(&mut self, quote: Option<char>) -> Result<Step<'s>> {
        let start = self.pos;
        let mut quoted = None;
        let mut chars = self.source[start + 1..].char_indices();
        let end = loop {
            match (quoted, chars.next()) {
                (_, None) => return Err(self.error(start, "unclosed `{`")),
                (Some(_), Some((_, '\\'))) => drop(chars.next()),
                (Some(quote), Some((_, c))) if c == quote => quoted = None,
                (None, Some((_, c @ ('"' | '\'')))) => quoted = Some(c),
                (None, Some((end, '}'))) => break start + 1 + end,
                _ => {}
            }
        };
        self.pos = end + 1;
        let inner = self.source[start + 1..end].trim();
        let unknown = || self.error(start, format!("unknown block `{{{inner}}}`"));
        Ok(match inner.split_at(usize::from(inner.starts_with(['#', ':', '/']))) {
            ("", _) => Step::Node(Node::Print(self.hole(inner, start)?)),
            (":", "else") => Step::End(End::Else(None)),
            (":", block) => {
                Step::End(End::Else(Some(block.strip_prefix("else if ").ok_or_else(unknown)?)))
            }
            ("/", "if") => Step::End(End::If),
            ("/", "each") => Step::End(End::Each),
            ("#", block) => {
                let (kind, header) = block.split_once(' ').ok_or_else(unknown)?;
                Step::Node(match kind {
                    "if" => self.if_block(header, start, quote)?,
                    "each" => self.each_block(header, start, quote)?,
                    _ => return Err(unknown()),
                })
            }
            _ => return Err(unknown()),
        })
    }

    fn hole(&self, source: &str, at: usize) -> Result<Hole> {
        let source = source.trim();
        let unsupported = || format!("unsupported expression `{source}`: {SUPPORTED}");
        let mut rest = source;
        let expr = binary(&mut rest, 0).filter(|_| rest.trim_start().is_empty());
        let expr = expr.ok_or_else(|| self.error(at, unsupported()))?;
        Ok(Hole { expr, source: source.to_string(), line: self.line(at) })
    }

    fn if_block(&mut self, condition: &str, start: usize, quote: Option<char>) -> Result<Node> {
        let mut arms = Vec::new();
        let mut condition = self.hole(condition, start)?;
        let outer = self.depth;
        self.depth += 1;
        // Whether the last arm was `{:else}`, which ends the arms.
        let mut last = false;
        loop {
            let (body, end) = self.nodes(quote)?;
            arms.push((condition, body));
            condition = match end {
                End::Else(next) if !last => {
                    last = next.is_none();
                    self.hole(next.unwrap_or("true"), self.pos)?
                }
                end => {
                    self.depth = outer;
                    self.expect(end, End::If, start)?;
                    return Ok(Node::If(arms));
                }
            };
        }
    }

    fn each_block(&mut self, header: &str, start: usize, quote: Option<char>) -> Result<Node> {
        let usage = "expected `{#each list as item}` or `{#each list as item, i}`";
        let (list, names) = header.rsplit_once(" as ").ok_or_else(|| self.error(start, usage))?;
        let (item, index) = match names.split_once(',') {
            Some((item, index)) => (item.trim(), Some(index.trim())),
            None => (names.trim(), None),
        };
        let digit = |name: &str| name.starts_with(|c: char| c.is_ascii_digit());
        let name = |name: &str| !name.is_empty() && word(name) == name.len() && !digit(name);
        if !name(item) || index.is_some_and(|index| !name(index)) {
            return Err(self.error(start, usage));
        }
        let list = self.hole(list, start)?;
        let body = self.body(quote, End::Each, start)?;
        Ok(Node::Each { list, item: item.to_string(), index: index.map(str::to_string), body })
    }

    /// Parses what starts with the `<` at the cursor.
    fn markup(&mut self) -> Result<Step<'s>> {
        let rest = self.rest();
        Ok(if rest.starts_with("<!--") {
            // Braces are literal in a comment.
            let end = rest.find("-->").map_or(rest.len(), |end| end + 3);
            self.pos += end;
            Step::Text(&rest[..end])
        } else if let Some(close) = rest.strip_prefix("</") {
            if close.starts_with(|c: char| c.is_ascii_uppercase()) {
                let end = close.find('>').ok_or_else(|| self.error(self.pos, "unclosed tag"))?;
                self.pos += end + 3;
                Step::End(End::Close(close[..end].trim()))
            } else {
                self.depth = self.depth.saturating_sub(1);
                self.pos += 2;
                Step::Text("</")
            }
        } else if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            Step::Node(self.element()?)
        } else {
            self.pos += 1;
            Step::Text("<")
        })
    }

    /// Parses the element whose start tag is at the cursor, with its body if it is a component,
    /// a script, or a style.
    fn element(&mut self) -> Result<Node> {
        let start = self.pos;
        let tag = self.tag()?;
        let lower = tag.name.to_ascii_lowercase();
        Ok(if tag.name.starts_with(|c: char| c.is_ascii_uppercase()) {
            let mut children = Vec::new();
            if !tag.self_closing {
                children = self.body(None, End::Close(&tag.name), start)?;
            }
            Node::Component { tag, children, line: self.line(start) }
        } else if lower == "slot" {
            if !tag.self_closing {
                return Err(self.error(start, "a slot takes no fallback content: write <slot />"));
            }
            Node::Slot
        } else if (lower == "script" || lower == "style") && !tag.self_closing {
            // Braces are literal in a script or a style, so the body is kept as written.
            let rest = self.rest();
            let lowered = rest.to_ascii_lowercase();
            let close = format!("</{lower}");
            // An end tag has whitespace, `/`, or `>` after its name, so `</scripture>` is not one.
            let after = |c: char| c.is_ascii_whitespace() || c == '/' || c == '>';
            let named = |at: &usize| lowered[at + close.len()..].starts_with(after);
            let end = lowered.match_indices(&close).map(|(at, _)| at).find(named);
            let end = end.ok_or_else(|| self.error(start, format!("unclosed <{lower}>")))?;
            self.pos += rest[end..].find('>').map_or(rest.len(), |close| end + close + 1);
            Node::Raw { tag, body: rest[..end].to_string(), hoist: self.depth == 0 }
        } else {
            if !tag.self_closing && !VOID.contains(&lower.as_str()) {
                self.depth += 1;
            }
            Node::Tag(tag)
        })
    }

    /// Parses the start tag at the cursor, up to its `>`.
    fn tag(&mut self) -> Result<Tag> {
        let start = self.pos;
        self.pos += 1;
        let name = self.take(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':').to_string();
        let mut attrs = Vec::new();
        loop {
            self.take(char::is_whitespace);
            let rest = self.rest();
            if let Some(end) = ["/>", ">"].iter().find(|end| rest.starts_with(**end)) {
                self.pos += end.len();
                return Ok(Tag { name, attrs, self_closing: end.len() == 2 });
            }
            let attr = self.take(|c| !c.is_whitespace() && !"=/>{\"'".contains(c));
            if attr.is_empty() {
                let problem =
                    if rest.is_empty() { "unclosed tag" } else { "expected an attribute" };
                return Err(self.error(start, problem));
            }
            self.take(char::is_whitespace);
            let mut value = AttrValue::Bare;
            if self.rest().starts_with('=') {
                self.pos += 1;
                self.take(char::is_whitespace);
                value = self.attr_value()?;
            }
            attrs.push(Attr { name: attr.to_string(), value });
        }
    }

    /// Parses the attribute value at the cursor, which follows an `=`.
    fn attr_value(&mut self) -> Result<AttrValue> {
        let at = self.pos;
        Ok(match self.rest().chars().next() {
            Some('{') => match self.braces(None)? {
                Step::Node(Node::Print(hole)) => AttrValue::Expr(hole),
                _ => return Err(self.error(at, "a block needs a quoted attribute value")),
            },
            Some(quote @ ('"' | '\'')) => {
                self.pos += 1;
                let (nodes, end) = self.nodes(Some(quote))?;
                self.expect(end, End::Quote, at)?;
                AttrValue::Text(nodes)
            }
            _ => {
                let text = self.take(|c| !c.is_whitespace() && c != '>');
                AttrValue::Text(vec![Node::Text(text.to_string())])
            }
        })
    }
}

/// The length of the name at the start of `text`.
fn word(text: &str) -> usize {
    text.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(text.len())
}

/// Consumes `token` from the start of `rest`, after any spaces.
fn eat(rest: &mut &str, token: &str) -> bool {
    let trimmed = rest.trim_start();
    let found = trimmed.starts_with(token);
    if found {
        *rest = &trimmed[token.len()..];
    }
    found
}

/// Parses an expression whose operators bind at `level` or tighter, or returns `None` for an
/// unsupported form.
fn binary(rest: &mut &str, level: usize) -> Option<Expr> {
    let Some(operators) = BINARY.get(level) else { return unary(rest) };
    let mut left = binary(rest, level + 1)?;
    while let Some(operator) = operators.iter().find(|operator| eat(rest, operator)) {
        let right = Box::new(binary(rest, level + 1)?);
        left = Expr::Binary { operator, left: Box::new(left), right };
    }
    Some(left)
}

fn unary(rest: &mut &str) -> Option<Expr> {
    if eat(rest, "!") {
        return Some(Expr::Not(Box::new(unary(rest)?)));
    } else if eat(rest, "(") {
        return binary(rest, 0).filter(|_| eat(rest, ")"));
    }
    let text = rest.trim_start();
    let first = text.chars().next()?;
    if first == '"' || first == '\'' {
        let mut value = String::new();
        let mut chars = text[1..].char_indices();
        loop {
            match chars.next()? {
                (_, '\\') => value.push(chars.next()?.1),
                (end, c) if c == first => {
                    *rest = &text[end + 2..];
                    return Some(Expr::Literal(Value::Str(value)));
                }
                (_, c) => value.push(c),
            }
        }
    } else if first.is_ascii_digit() {
        let end = text.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(text.len());
        *rest = &text[end..];
        return Some(Expr::Literal(Value::Num(text[..end].parse().ok()?)));
    }
    let name = &text[..word(text)];
    *rest = &text[name.len()..];
    match name {
        "" => None,
        "true" | "false" => Some(Expr::Literal(Value::Bool(name == "true"))),
        // A call leaves its `(` unread, so the caller finds the expression unsupported.
        _ => {
            let mut path = vec![name.to_string()];
            while eat(rest, ".") {
                let key = &rest[..word(rest)];
                *rest = &rest[key.len()..];
                path.push(Some(key).filter(|key| !key.is_empty())?.to_string());
            }
            Some(Expr::Path(path))
        }
    }
}
