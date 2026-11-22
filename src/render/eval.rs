//! Evaluates templates: values, expressions, components, and the paths they write.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Write};
use std::path::Component;

use anyhow::{Result, anyhow, bail};

use super::template::{AttrValue, Expr, Hole, Node, Tag, Template};
use crate::paths;

const MAX_DEPTH: usize = 32;

/// A template value.
#[derive(Clone, PartialEq)]
pub(super) enum Value {
    Missing,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

/// What every page's render reads.
pub(super) struct Shared {
    /// Components by file name, without the extension.
    pub(super) components: HashMap<String, Template>,
    /// The `[site]` table, which templates read as `site`.
    pub(super) site: Value,
}

/// Renders one page: its template, the components it uses, and the blocks they hoist.
struct Renderer<'a> {
    /// The page's template. Every path in the rendered HTML resolves against its directory.
    file: &'a Template,
    /// Component styles and scripts that move to the `<head>`, each once.
    head: Vec<String>,
    /// How many components deep the render is.
    depth: usize,
}

/// Where a run of nodes renders: its template and the names in scope.
#[derive(Clone, Copy)]
struct Env<'a> {
    shared: &'a Shared,
    /// What `page` names.
    page: &'a Value,
    template: &'a Template,
    /// Whether the template sits outside the page's directory, so the paths it writes move.
    moved: bool,
    /// A component's props. A page has none.
    props: Option<&'a [(&'a str, Cow<'a, Value>)]>,
    /// The innermost `{#each}` binding.
    binds: Option<&'a Bind<'a>>,
    /// What a component's `<slot />` renders. A page has none.
    slot: Option<Slot<'a>>,
}

struct Bind<'a> {
    name: &'a str,
    value: &'a Value,
    outer: Option<&'a Bind<'a>>,
}

/// A component's children, which render in the scope of the template that wrote them.
#[derive(Clone, Copy)]
struct Slot<'a> {
    children: &'a [Node],
    scope: &'a Env<'a>,
}

/// What a run of nodes renders into.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    /// Markup, which keeps text as written and escapes a value.
    Html,
    /// A string prop, which holds plain text: entities decode, and a value is not escaped.
    Prop,
}

impl Value {
    /// The value of a TOML table, such as `[site]`.
    pub(super) fn table(table: &toml::Table) -> Value {
        Value::Map(table.iter().map(|(key, value)| (key.clone(), value.into())).collect())
    }

    /// The text that printing the value writes, if it can be printed.
    fn text(&self) -> Option<Cow<'_, str>> {
        match self {
            Value::Str(text) => Some(Cow::Borrowed(text)),
            Value::Num(n) => Some(n.to_string().into()),
            Value::Bool(b) => Some(b.to_string().into()),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Missing => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Str(text) => !text.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Map(_) => true,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Value::Missing => "missing value",
            Value::Bool(_) => "boolean",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }
}

impl From<&toml::Value> for Value {
    fn from(value: &toml::Value) -> Value {
        match value {
            toml::Value::String(text) => Value::Str(text.clone()),
            toml::Value::Integer(n) => Value::Num(*n as f64),
            toml::Value::Float(n) => Value::Num(*n),
            toml::Value::Boolean(b) => Value::Bool(*b),
            toml::Value::Datetime(date) => Value::Str(date.to_string()),
            toml::Value::Array(items) => Value::List(items.iter().map(Value::from).collect()),
            toml::Value::Table(table) => Value::table(table),
        }
    }
}

/// Renders the page whose template is `file` and whose URL is `url`.
pub(super) fn page(shared: &Shared, file: &Template, url: String) -> Result<String> {
    let page = Value::Map(BTreeMap::from([("url".to_string(), Value::Str(url))]));
    let env = Env {
        shared,
        page: &page,
        template: file,
        moved: false,
        props: None,
        binds: None,
        slot: None,
    };
    let mut renderer = Renderer { file, head: Vec::new(), depth: 0 };
    let mut html = String::new();
    renderer.nodes(&file.nodes, &env, Target::Html, &mut html)?;
    // Hoisted blocks end the `<head>`. Without one they follow the doctype, which stays first.
    let at = html
        .as_bytes()
        .windows(7)
        .position(|window| window.eq_ignore_ascii_case(b"</head>"))
        .unwrap_or_else(|| paths::after_doctype(&html));
    html.insert_str(at, &renderer.head.concat());
    Ok(html)
}

impl Renderer<'_> {
    fn error(&self, env: &Env, line: usize, message: impl Display) -> anyhow::Error {
        let file = &env.template.name;
        // An error in a component also names the page that used it.
        let page = match *file == self.file.name {
            true => String::new(),
            false => format!("{}: ", self.file.name),
        };
        anyhow!("{page}{file}:{line}: {message}")
    }

    /// Renders `nodes` into `out`.
    fn nodes<'v>(
        &mut self,
        nodes: &'v [Node],
        env: &Env<'v>,
        target: Target,
        out: &mut String,
    ) -> Result<()> {
        for node in nodes {
            match node {
                Node::Text(text) => match target {
                    Target::Html => out.push_str(text),
                    Target::Prop => out.push_str(&html_escape::decode_html_entities(text)),
                },
                Node::Print(hole) => {
                    let value = self.value(hole, env)?;
                    let text = self.text(&value, hole, env)?;
                    match target {
                        Target::Html => escape(out, &text),
                        Target::Prop => out.push_str(&text),
                    }
                }
                Node::If(arms) => {
                    for (condition, body) in arms {
                        if self.value(condition, env)?.truthy() {
                            self.nodes(body, env, target, out)?;
                            break;
                        }
                    }
                }
                Node::Each { list, item, index, body } => {
                    let value = self.value(list, env)?;
                    let items = match &*value {
                        Value::List(items) => &items[..],
                        Value::Missing => &[],
                        other => {
                            let kind = other.kind();
                            let problem = format!("cannot loop over a {kind}: {}", list.source);
                            return Err(self.error(env, list.line, problem));
                        }
                    };
                    for (i, value) in items.iter().enumerate() {
                        let position = Value::Num(i as f64);
                        let item = Bind { name: item, value, outer: env.binds };
                        let index = index.as_ref().map(|name| Bind {
                            name,
                            value: &position,
                            outer: Some(&item),
                        });
                        let binds = Some(index.as_ref().unwrap_or(&item));
                        self.nodes(body, &Env { binds, ..*env }, target, out)?;
                    }
                }
                Node::Tag(tag) => self.tag(tag, env, out)?,
                Node::Raw { tag, body, hoist } => {
                    let mut block = String::new();
                    self.tag(tag, env, &mut block)?;
                    if tag.name.eq_ignore_ascii_case("style") && env.moved {
                        block.push_str(&self.rebase_css(body, env));
                    } else {
                        block.push_str(body);
                    }
                    write!(block, "</{}>", tag.name)?;
                    // A page prints its own styles and scripts in place.
                    if !*hoist || env.props.is_none() {
                        out.push_str(&block);
                    } else if !self.head.contains(&block) {
                        self.head.push(block);
                    }
                }
                Node::Slot => {
                    if let Some(slot) = env.slot {
                        self.nodes(slot.children, slot.scope, target, out)?;
                    }
                }
                Node::Component { tag, children, line } => {
                    self.component(tag, children, *line, env, out)?;
                }
            }
        }
        Ok(())
    }

    /// Renders the component that `tag` names, with the tag's attributes as its props and
    /// `children` as its slot.
    fn component<'v>(
        &mut self,
        tag: &'v Tag,
        children: &'v [Node],
        line: usize,
        env: &Env<'v>,
        out: &mut String,
    ) -> Result<()> {
        let Some(template) = env.shared.components.get(&tag.name) else {
            return Err(self.error(env, line, format!("unknown component <{}>", tag.name)));
        };
        if self.depth == MAX_DEPTH {
            let problem = format!("<{}>: components nest deeper than {MAX_DEPTH} levels", tag.name);
            return Err(self.error(env, line, problem));
        }
        let mut props = Vec::new();
        for attr in &tag.attrs {
            let value = match &attr.value {
                AttrValue::Bare => Cow::Owned(Value::Bool(true)),
                AttrValue::Expr(hole) => self.value(hole, env)?,
                AttrValue::Text(nodes) => {
                    let mut text = String::new();
                    self.nodes(nodes, env, Target::Prop, &mut text)?;
                    // A literal that names a file beside this template is a path.
                    let literal = env.moved && matches!(nodes[..], [Node::Text(_)]);
                    if literal && env.template.dir.join(paths::split(&text).path).is_file() {
                        text = self.rebase(&text, env).into_owned();
                    }
                    Cow::Owned(Value::Str(text))
                }
            };
            props.push((attr.name.as_str(), value));
        }
        let moved = template.dir != self.file.dir;
        let slot = Some(Slot { children, scope: env });
        let inner = Env { template, moved, props: Some(&props), binds: None, slot, ..*env };
        self.depth += 1;
        let result = self.nodes(&template.nodes, &inner, Target::Html, out);
        self.depth -= 1;
        result
    }

    /// Writes a start tag, with each attribute as its value renders.
    fn tag<'v>(&mut self, tag: &'v Tag, env: &Env<'v>, out: &mut String) -> Result<()> {
        write!(out, "<{}", tag.name)?;
        // The `content` of a link preview's image is a path, where any other is text.
        let preview = tag.attrs.iter().any(|attr| match &attr.value {
            AttrValue::Text(nodes) if matches!(attr.name.as_str(), "property" | "name") => {
                matches!(&nodes[..], [Node::Text(text)] if paths::is_preview_image(text))
            }
            _ => false,
        });
        for attr in &tag.attrs {
            let name = &attr.name;
            match &attr.value {
                AttrValue::Bare => write!(out, " {name}")?,
                AttrValue::Expr(hole) => match &*self.value(hole, env)? {
                    // A boolean adds or omits the attribute, as `checked={done}` should.
                    Value::Bool(false) => {}
                    Value::Bool(true) => write!(out, " {name}")?,
                    value => {
                        write!(out, " {name}=\"")?;
                        escape(out, &self.text(value, hole, env)?);
                        out.push('"');
                    }
                },
                AttrValue::Text(nodes) => {
                    write!(out, " {name}=\"")?;
                    match &nodes[..] {
                        [Node::Text(text)] if env.moved => {
                            let path = preview && name.eq_ignore_ascii_case("content");
                            self.rebase_attr(if path { "src" } else { name }, text, env, out);
                        }
                        _ => self.nodes(nodes, env, Target::Html, out)?,
                    }
                    out.push('"');
                }
            }
        }
        out.push_str(if tag.self_closing { " />" } else { ">" });
        Ok(())
    }

    /// The text that printing the value of `hole` writes.
    fn text<'v>(&self, value: &'v Value, hole: &Hole, env: &Env) -> Result<Cow<'v, str>> {
        value.text().ok_or_else(|| {
            let problem = match value {
                Value::Missing => format!("{} is missing", hole.source),
                other => format!("cannot print a {}: {}", other.kind(), hole.source),
            };
            self.error(env, hole.line, problem)
        })
    }

    /// Evaluates a hole. A value that already exists is borrowed, not copied.
    fn value<'v>(&self, hole: &'v Hole, env: &Env<'v>) -> Result<Cow<'v, Value>> {
        eval(&hole.expr, env).map_err(|e| self.error(env, hole.line, e))
    }

    /// Writes the literal value of the attribute `name`, with the paths it holds rebased.
    fn rebase_attr(&self, name: &str, value: &str, env: &Env, out: &mut String) {
        match name.to_ascii_lowercase().as_str() {
            "src" | "href" | "xlink:href" | "poster" | "data" => {
                out.push_str(&self.rebase(value, env));
            }
            "srcset" => {
                for (i, candidate) in paths::srcset(value).iter().enumerate() {
                    out.push_str(if i > 0 { ", " } else { "" });
                    out.push_str(&self.rebase(candidate.url, env));
                    if !candidate.descriptor.is_empty() {
                        out.push(' ');
                        out.push_str(candidate.descriptor);
                    }
                }
            }
            // Entities decode first, so `url(&quot;a.png&quot;)` reads as a quoted path.
            "style" if value.contains("url(") => {
                let css = html_escape::decode_html_entities(value);
                escape(out, &self.rebase_css(&css, env));
            }
            _ => out.push_str(value),
        }
    }

    /// Rebases each `url()` in CSS written in the template.
    fn rebase_css(&self, css: &str, env: &Env) -> String {
        let mut out = String::new();
        let mut rest = css;
        while let Some(open) = rest.find("url(") {
            let inner = rest[open + 4..].trim_start();
            let quote = inner.chars().next().filter(|c| matches!(c, '"' | '\''));
            let start = rest.len() - inner.len() + quote.map_or(0, char::len_utf8);
            let end =
                rest[start..].find(quote.unwrap_or(')')).map_or(rest.len(), |end| start + end);
            out.push_str(&rest[..start]);
            out.push_str(&self.rebase(rest[start..end].trim(), env));
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    /// Rewrites a relative path written in the template so that it resolves from the page's
    /// directory.
    fn rebase<'v>(&self, value: &'v str, env: &Env) -> Cow<'v, str> {
        let reference = paths::split(value);
        let path = reference.path;
        if path.is_empty() || path.starts_with('/') || paths::has_scheme(path) {
            return Cow::Borrowed(value);
        }
        let target = paths::normalize(&env.template.dir.join(path));
        let page = &self.file.dir;
        let shared = target.components().zip(page.components()).take_while(|(a, b)| a == b).count();
        let up = page.components().skip(shared).map(|_| "..".into());
        // A space in a directory's name would end the path in a `srcset` or a `url()`.
        let name = |part: Component| part.as_os_str().to_string_lossy().replace(' ', "%20");
        let parts: Vec<String> = up.chain(target.components().skip(shared).map(name)).collect();
        // The page's own directory, which the optimizer reports as not a file.
        let path = if parts.is_empty() { ".".to_string() } else { parts.join("/") };
        Cow::Owned(format!("{path}{}", reference.suffix))
    }
}

/// Evaluates an expression in the scope `env`.
fn eval<'v>(expr: &'v Expr, env: &Env<'v>) -> Result<Cow<'v, Value>> {
    Ok(match expr {
        Expr::Literal(value) => Cow::Borrowed(value),
        Expr::Path(path) => {
            let mut value = lookup(&path[0], env);
            for key in &path[1..] {
                value = match value {
                    Some(Value::Map(fields)) => fields.get(key),
                    _ => None,
                };
            }
            Cow::Borrowed(value.unwrap_or(&Value::Missing))
        }
        Expr::Not(inner) => Cow::Owned(Value::Bool(!eval(inner, env)?.truthy())),
        Expr::Binary { operator, left, right } => {
            let left = eval(left, env)?;
            Cow::Owned(Value::Bool(match *operator {
                "&&" => left.truthy() && eval(right, env)?.truthy(),
                "||" => left.truthy() || eval(right, env)?.truthy(),
                operator => compare(operator, &left, &*eval(right, env)?)?,
            }))
        }
    })
}

/// Finds `name` among the `{#each}` bindings, then the props, then `page` and `site`.
fn lookup<'v>(name: &str, env: &Env<'v>) -> Option<&'v Value> {
    let mut bind = env.binds;
    while let Some(Bind { name: bound, value, outer }) = bind {
        if *bound == name {
            return Some(*value);
        }
        bind = *outer;
    }
    let props: &'v [(&str, Cow<Value>)] = env.props.unwrap_or_default();
    match (props.iter().find(|(prop, _)| *prop == name), name) {
        (Some((_, value)), _) => Some(&**value),
        (None, "page") => Some(env.page),
        (None, "site") => Some(&env.shared.site),
        _ => None,
    }
}

/// Compares two values. Types never convert, so `3 == "3"` is false.
fn compare(operator: &str, a: &Value, b: &Value) -> Result<bool> {
    let order = match (a, b) {
        (Value::Num(a), Value::Num(b)) => a.partial_cmp(b),
        (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
        _ => None,
    };
    Ok(match (operator, order) {
        ("==", _) => a == b,
        ("!=", _) => a != b,
        _ if [a, b].contains(&&Value::Missing) => false,
        ("<", Some(order)) => order.is_lt(),
        ("<=", Some(order)) => order.is_le(),
        (">", Some(order)) => order.is_gt(),
        (_, Some(order)) => order.is_ge(),
        _ => bail!("cannot compare a {} with a {}", a.kind(), b.kind()),
    })
}

/// Appends `text` to `out`, escaped for HTML text and for a double-quoted attribute.
fn escape(out: &mut String, text: &str) {
    html_escape::encode_double_quoted_attribute_to_string(text, out);
}
