//! The render stage: routes and templates in, plain HTML pages out.

mod eval;
mod template;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::config::Config;
use crate::paths;
use crate::site::{Page, Rendered};
use eval::{Shared, Value};
use template::Template;

/// Renders every page of the site in the canonical directory `root`.
pub fn render(root: &Path, config: &Config) -> Result<Rendered> {
    let mut components: HashMap<String, Template> = HashMap::new();
    for file in walk(&root.join("components"))? {
        if file.extension().is_none_or(|extension| extension != "html") {
            continue;
        }
        let template = Template::parse(root, &file, &read(&file)?)?;
        let name = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        match components.entry(name) {
            Entry::Occupied(other) => {
                let first = &other.get().name;
                bail!("{first} and {} are both the component <{}>", template.name, other.key());
            }
            Entry::Vacant(entry) => entry.insert(template),
        };
    }
    let shared = Shared { components, site: Value::table(&config.site) };

    let pages_dir = root.join("pages");
    let mut routes = walk(&pages_dir)?;
    // A file whose name starts with `_` or `.` is a partial, or one that an editor or the
    // system left behind.
    routes.retain(|file| {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        name.ends_with(".html") && !name.starts_with(['_', '.'])
    });
    let render = |file: PathBuf| {
        let text = read(&file)?;
        if text.starts_with("---\n") || text.starts_with("---\r\n") {
            let name = paths::name(root, &file);
            bail!("{name}:1: frontmatter is not yet supported for HTML pages");
        }
        let template = Template::parse(root, &file, &text)?;
        let mut page = Page { path: route(&pages_dir, &file), source: file, html: String::new() };
        page.html = eval::page(&shared, &template, page.url())?;
        anyhow::Ok(page)
    };
    let mut pages: Vec<Page> = routes.into_par_iter().map(render).collect::<Result<_>>()?;
    // The sort is stable, so two pages with one output path keep the order of their sources.
    pages.sort_by(|a, b| a.path.cmp(&b.path));

    let mut files = BTreeMap::new();
    let base = config.base_url();
    // A file of the same name in `public/` takes the generated one's place.
    if let Some(base) = base.filter(|_| !root.join("public/sitemap.xml").exists()) {
        let mut urls: Vec<String> = pages.iter().map(Page::url).collect();
        urls.sort();
        let mut sitemap = r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string();
        sitemap.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
        for url in urls.iter().filter(|url| *url != "/404.html") {
            write!(sitemap, "<url><loc>{}</loc></url>", xml(&format!("{base}{url}")))?;
        }
        sitemap.push_str("</urlset>\n");
        files.insert("sitemap.xml".to_string(), sitemap);
    }
    // Crawlers read `robots.txt` at the root of a host only, so a site under a path has none.
    let rooted = config.prefix().is_empty() && !root.join("public/robots.txt").exists();
    if let Some(base) = base.filter(|_| rooted) {
        let robots = format!("User-agent: *\nAllow: /\nSitemap: {base}/sitemap.xml\n");
        files.insert("robots.txt".to_string(), robots);
    }
    Ok(Rendered { pages, files })
}

/// The output path of a route file, such as `about/index.html` for `pages/about.html`.
fn route(pages_dir: &Path, source: &Path) -> String {
    let path = source.strip_prefix(pages_dir).unwrap_or(source).with_extension("");
    let stem = path.to_string_lossy();
    if stem == "404" || stem == "index" || stem.ends_with("/index") {
        format!("{stem}.html")
    } else {
        format!("{stem}/index.html")
    }
}

/// Escapes `text` for XML, without the control characters that XML cannot hold.
fn xml(text: &str) -> String {
    let text = text.replace(|c: char| c < ' ' && !"\t\n\r".contains(c), "");
    html_escape::encode_double_quoted_attribute(&text).into_owned()
}

/// The text of a template, without a byte order mark.
fn read(file: &Path) -> Result<String> {
    let mut text = fs::read_to_string(file).with_context(|| file.display().to_string())?;
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    Ok(text)
}

/// Every file under `dir`, sorted, or none if `dir` does not exist.
fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in paths::entries(dir)? {
        if entry.file_type()?.is_dir() {
            files.extend(walk(&entry.path())?);
        } else {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}
