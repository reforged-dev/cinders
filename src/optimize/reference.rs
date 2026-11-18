//! What a reference names: nothing the build changes, a rooted URL, or a source file.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};

use crate::config::Config;
use crate::paths;
use crate::site::Page;

/// What a reference names.
pub(super) enum Target {
    /// An external URL, or a query or a fragment alone, which ships as written.
    Unchanged,
    /// A rooted URL in the site, before the site's prefix: a route or a `public/` file as
    /// written, or the URL of the page whose source file the reference names.
    Rooted(String),
    /// A source file to process, and the query and fragment that followed it.
    File(PathBuf, String),
}

/// The places a reference may name.
pub(super) struct References<'a> {
    root: &'a Path,
    config: &'a Config,
    /// Every rooted URL a reference may name: routes, `public/` files, and generated files.
    urls: HashSet<String>,
    /// Page source files and their URLs, so a relative link to a page becomes its route.
    pages: HashMap<PathBuf, String>,
}

impl<'a> References<'a> {
    pub(super) fn new(root: &'a Path, config: &'a Config) -> Self {
        References { root, config, urls: HashSet::new(), pages: HashMap::new() }
    }

    /// Lets a rooted reference name the output file `path`, and its directory if the file is
    /// the directory's index.
    pub(super) fn add(&mut self, path: &str) {
        let dir = path.strip_suffix("index.html");
        if let Some(dir) = dir.filter(|dir| dir.is_empty() || dir.ends_with('/')) {
            self.urls.extend([format!("/{dir}"), format!("/{}", dir.trim_end_matches('/'))]);
        }
        self.urls.insert(format!("/{path}"));
    }

    /// Lets a reference name `page`, by its URL or by its source file.
    pub(super) fn add_page(&mut self, page: &Page) {
        self.add(&page.path);
        self.pages.insert(page.source.clone(), page.url());
    }

    /// Resolves `value`, a reference written in a file in `dir`.
    pub(super) fn resolve(&self, dir: &Path, value: &str) -> Result<Target> {
        let value = value.trim();
        let reference = paths::split(value);
        let raw = reference.path;
        if raw.is_empty() || raw.starts_with("//") || paths::has_scheme(raw) {
            return Ok(Target::Unchanged);
        }
        // A reference may write a space or a non-ASCII name percent-encoded, and HTML may use
        // character references.
        let path = paths::percent_decode(&html_escape::decode_html_entities(raw));
        if path.starts_with('/') {
            let unchecked = || self.config.unchecked.iter().any(|prefix| path.starts_with(prefix));
            if !self.urls.contains(&path) && !unchecked() {
                // The author writes the site as if it were at the root of its host.
                let prefix = self.config.prefix();
                let fix = match !prefix.is_empty() && Path::new(&path).starts_with(prefix) {
                    true => format!(": write it without `{prefix}`, which cinders adds"),
                    false => String::new(),
                };
                bail!("`{value}` is not a route or a file in public/{fix}");
            }
            return Ok(Target::Rooted(value.to_string()));
        }
        let file = paths::normalize(&dir.join(&path));
        if let Some(url) = self.pages.get(&file) {
            return Ok(Target::Rooted(format!("{url}{}", reference.suffix)));
        }
        match fs::metadata(&file) {
            Err(_) => bail!("`{value}` does not exist"),
            Ok(meta) if meta.is_dir() => bail!("`{value}` is a directory, not a file"),
            Ok(_) => {}
        }
        ensure!(file.starts_with(self.root), "`{value}` is outside the site directory");
        if file.starts_with(self.root.join("pages")) {
            match paths::extension(&file).as_str() {
                "html" => bail!(
                    "`{value}` is a page that is not built, such as one whose name starts with `_`"
                ),
                // The link keeps its meaning free for the page's route.
                "md" => bail!("`{value}` is a Markdown page, which is not yet supported"),
                _ => {}
            }
        }
        Ok(Target::File(file, reference.suffix.to_string()))
    }
}
