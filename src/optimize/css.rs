//! Stylesheets: lightningcss resolves `@import`, lowers syntax, and minifies, and cinders
//! rewrites the `url()` references.

use std::fmt::Write;
use std::io::{self, ErrorKind};
use std::mem;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{Result, anyhow, bail};
use lightningcss::bundler::{
    BundleErrorKind, Bundler, FileProvider, ResolveResult, SourceProvider,
};
use lightningcss::dependencies::{Dependency, DependencyOptions};
use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::targets::{Browsers, Targets};

use super::Optimizer;
use super::asset::Kind;
use crate::Mode;
use crate::config::Config;
use crate::paths;

/// Baseline widely available, with Edge raised to its first version that decodes AVIF, so
/// that every default browser shows the images a build ships.
const BROWSERS: [&str; 4] = ["chrome 111", "edge 121", "firefox 114", "safari 16.4"];
const NAMES: &str = "chrome, edge, firefox, safari, opera, and samsung";

/// The browsers whose syntax stylesheets are lowered to, each a name and a minimum version.
pub(super) fn targets(config: &Config) -> Result<Targets> {
    let default = BROWSERS.map(String::from);
    let mut browsers = Browsers::default();
    for entry in config.browsers.as_deref().unwrap_or(&default) {
        let (name, number) = entry.split_once(' ').unwrap_or((entry, ""));
        let slot = match name {
            "chrome" => &mut browsers.chrome,
            "edge" => &mut browsers.edge,
            "firefox" => &mut browsers.firefox,
            "safari" => &mut browsers.safari,
            "opera" => &mut browsers.opera,
            "samsung" => &mut browsers.samsung,
            _ => bail!("cinders.toml: browsers: `{entry}` is not a browser cinders knows: {NAMES}"),
        };
        let Some(version) = version(number) else {
            bail!("cinders.toml: browsers: `{entry}` needs a version, as in `safari 16.4`");
        };
        *slot = Some(version);
    }
    // Safari on iOS shares Safari's versions.
    browsers.ios_saf = browsers.safari;
    Ok(Targets { browsers: Some(browsers), ..Targets::default() })
}

/// A version such as `16.4` in lightningcss's form, a byte each for minor and patch.
fn version(text: &str) -> Option<u32> {
    let mut parts = text.split('.');
    let major: u16 = parts.next()?.parse().ok()?;
    let mut next = || parts.next().map_or(Some(0u8), |part| part.parse().ok());
    let minor = next()?;
    let patch = next()?;
    let version = u32::from(major) << 16 | u32::from(minor) << 8 | u32::from(patch);
    parts.next().is_none().then_some(version)
}

/// Reads files from disk and leaves an `@import` of an external URL in place.
struct Files<'a> {
    /// The site directory. An import of a file outside it is an error.
    root: &'a Path,
    provider: FileProvider,
    /// The strings of every file read.
    strings: Mutex<String>,
}

impl Files<'_> {
    fn new(root: &Path) -> Files<'_> {
        Files { root, provider: FileProvider::new(), strings: Mutex::default() }
    }
}

impl SourceProvider for Files<'_> {
    type Error = io::Error;

    fn read<'a>(&'a self, file: &Path) -> io::Result<&'a str> {
        let css = self.provider.read(file)?;
        super::lock(&self.strings).push_str(&strings(css));
        Ok(css)
    }

    fn resolve(&self, specifier: &str, from: &Path) -> io::Result<ResolveResult> {
        if external(specifier) {
            return Ok(ResolveResult::External(specifier.to_string()));
        }
        let path = paths::percent_decode(paths::split(specifier).path);
        let file = paths::normalize(&from.with_file_name(path));
        let problem = match file.exists() {
            false => "does not exist",
            true if !file.starts_with(self.root) => "is outside the site directory",
            true => return Ok(file.into()),
        };
        Err(io::Error::new(ErrorKind::InvalidInput, format!("`{specifier}` {problem}")))
    }
}

/// Whether an import names no source file: a URL, or a rooted path to a route or `public/`.
fn external(url: &str) -> bool {
    url.starts_with('/') || paths::has_scheme(url)
}

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

/// The text of every string in `css`. A string may be text that CSS draws, as `content`, a
/// list marker, and `quotes` do, so a font subset has to hold its characters.
pub(super) fn strings(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css.chars().peekable();
    while let Some(c) = rest.next() {
        if c == '/' && rest.next_if_eq(&'*').is_some() {
            // An apostrophe in a comment opens no string.
            let mut last = ' ';
            rest.by_ref().find(|&c| mem::replace(&mut last, c) == '*' && c == '/');
        }
        if c != '"' && c != '\'' {
            continue;
        }
        while let Some(inner) = rest.next().filter(|&inner| inner != c && inner != '\n') {
            if inner != '\\' {
                out.push(inner);
                continue;
            }
            // An escape is up to six hex digits and a space that ends them, or one character.
            let mut hex = String::new();
            while hex.len() < 6
                && let Some(digit) = rest.next_if(char::is_ascii_hexdigit)
            {
                hex.push(digit);
            }
            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                Some(escaped) => {
                    rest.next_if(char::is_ascii_whitespace);
                    out.push(escaped);
                }
                None => out.extend(rest.next().filter(|&c| c != '\n')),
            }
        }
    }
    out
}

/// The text of the strings in the stylesheet `path` and in the stylesheets it imports. An
/// error waits for the build of the stylesheet, which reports it.
pub(super) fn file_strings(root: &Path, path: &Path) -> String {
    let files = Files::new(root);
    Bundler::new(&files, None, ParserOptions::default()).bundle(path).map(drop).unwrap_or_default();
    files.strings.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// Builds a linked stylesheet with its local imports inlined.
pub(super) fn file(optimizer: &Optimizer, path: &Path) -> Result<String> {
    let files = Files::new(optimizer.root);
    let mut bundler = Bundler::new(&files, None, ParserOptions::default());
    let sheet = bundler.bundle(path).map_err(|e| match &e.kind {
        // The message already names the import, so the rule's location is left out.
        BundleErrorKind::ResolverError(refused) if refused.kind() == ErrorKind::InvalidInput => {
            anyhow!("{refused}")
        }
        _ => anyhow!("{e}"),
    })?;
    finish(optimizer, sheet)
}

/// Builds a `<style>` block, whose references resolve against the page's source file.
pub(super) fn inline(optimizer: &Optimizer, css: &str, source: &Path) -> Result<String> {
    let filename = source.to_string_lossy().into_owned();
    let options = ParserOptions { filename, ..ParserOptions::default() };
    finish(optimizer, StyleSheet::parse(css, options).map_err(|e| anyhow!("{e}"))?)
}

fn finish(optimizer: &Optimizer, mut sheet: StyleSheet) -> Result<String> {
    let targets = optimizer.targets;
    let optimized = optimizer.mode == Mode::Optimized;
    if optimized {
        let options = MinifyOptions { targets, ..MinifyOptions::default() };
        sheet.minify(options).map_err(|e| anyhow!("{e}"))?;
    }
    let options = PrinterOptions {
        minify: optimized,
        targets,
        analyze_dependencies: Some(DependencyOptions { remove_imports: false }),
        ..PrinterOptions::default()
    };
    let printed = sheet.to_css(options).map_err(|e| anyhow!("{e}"))?;
    let mut code = printed.code;
    for dependency in printed.dependencies.into_iter().flatten() {
        let named = match dependency {
            Dependency::Url(named) => named,
            // Only an external import survives bundling, and inline sheets are not bundled.
            Dependency::Import(import) if external(&import.url) => {
                // A rooted import is checked and written like any other rooted reference.
                let url = optimizer.reference(Path::new(""), &import.url, Kind::Raw)?;
                code = code.replace(&import.placeholder, &string(&url.unwrap_or(import.url)));
                continue;
            }
            Dependency::Import(import) => {
                let url = import.url;
                bail!("`@import \"{url}\"` in a <style> block: link the stylesheet instead")
            }
        };
        let dir = Path::new(&named.loc.file_path).parent().unwrap_or(Path::new(""));
        let url = optimizer.reference(dir, &named.url, Kind::Font)?.unwrap_or(named.url);
        // The printer quotes placeholders, but a plain URL in `url()` needs no quotes. A
        // string, as in `image-set()`, keeps them.
        let quoted = format!("\"{}\"", named.placeholder);
        code = code.replace(&format!("url({quoted})"), &format!("url({})", self::url(&url)));
        code = code.replace(&named.placeholder, &string(&url));
        // A subset font is WOFF2 whatever its source was, so its format hint follows.
        if optimized && url.ends_with(".woff2") {
            for format in [r#"format("truetype")"#, r#"format("opentype")"#] {
                let woff2 = format!(r#"{url})format("woff2")"#);
                code = code.replace(&format!("{url}){format}"), &woff2);
                code = code.replace(&format!("{url}) {format}"), &woff2);
            }
        }
    }
    Ok(code)
}
