//! The optimize stage: rendered pages in, the shipped site out. It reads only the rendered HTML
//! and the files it references, and knows nothing about templates.

mod asset;
mod css;
mod html;
mod minify;
mod reference;

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::Mode;
use crate::config::Config;
use crate::paths;
use crate::site::{Content, Page, Rendered, Site};
use crate::store::Store;
use asset::Assets;
use reference::{References, Target};

/// What the passes over a page share.
struct Optimizer<'a> {
    root: &'a Path,
    config: &'a Config,
    mode: Mode,
    references: References<'a>,
    assets: Assets<'a>,
}

/// Builds the site for the rendered pages of the site in the canonical directory `root`.
pub(crate) fn optimize(
    root: &Path,
    config: &Config,
    rendered: Rendered,
    mode: Mode,
) -> Result<Site> {
    let references = References::new(root, config);
    let mut optimizer =
        Optimizer { root, config, mode, references, assets: Assets::new(root, mode) };
    optimizer.add_public(&root.join("public"), "")?;
    for (path, text) in rendered.files {
        optimizer.references.add(&path);
        let from = format!("the generated {path}");
        optimizer.assets.put(path, from, Content::Bytes(text.into_bytes()))?;
    }
    for page in &rendered.pages {
        optimizer.references.add_page(page);
        // Claims the path, so a collision fails before any slow work.
        let from = paths::name(root, &page.source);
        optimizer.assets.put(page.path.clone(), from, Content::Bytes(Vec::new()))?;
    }

    let pages = rendered.pages.par_iter().map(|page| optimizer.page(page));
    let pages: Vec<String> = pages.collect::<Result<_>>()?;
    let mut files = optimizer.assets.into_files();
    for (page, html) in rendered.pages.into_iter().zip(pages) {
        files.insert(page.path, Content::Bytes(html.into_bytes()));
    }
    Ok(Site { files, prefix: config.prefix().to_string(), store: Store::new(root) })
}

impl Optimizer<'_> {
    /// Adds the files under `dir` in `public/`, which ship as they are. `prefix` is the output
    /// path of `dir`.
    fn add_public(&mut self, dir: &Path, prefix: &str) -> Result<()> {
        for entry in paths::entries(dir)? {
            let name = entry.file_name();
            let path = format!("{prefix}{}", name.to_string_lossy());
            if entry.file_type()?.is_dir() {
                self.add_public(&entry.path(), &format!("{path}/"))?;
            // macOS leaves a `.DS_Store` in any directory its file manager has shown.
            } else if name != ".DS_Store" {
                self.references.add(&path);
                let from = format!("public/{path}");
                self.assets.put(path, from, Content::File(entry.path()))?;
            }
        }
        Ok(())
    }

    /// The HTML that `page` ships.
    fn page(&self, page: &Page) -> Result<String> {
        let html = html::rewrite(self, page);
        let html = html.with_context(|| paths::name(self.root, &page.source))?;
        match self.mode {
            Mode::Optimized => minify::html(&html),
            Mode::Fast => Ok(html),
        }
    }

    /// The rooted URL that `value` names in the built site, before the site's prefix, or
    /// `None` if it ships as written. `value` is a reference written in a file in `dir`.
    fn rooted(&self, dir: &Path, value: &str) -> Result<Option<String>> {
        Ok(match self.references.resolve(dir, value)? {
            Target::Unchanged => None,
            Target::Rooted(url) => Some(url),
            Target::File(file, suffix) => Some(format!("{}{suffix}", self.assets.asset(&file)?)),
        })
    }

    /// The URL to write in place of `value`, a reference written in a file in `dir`, or `None`
    /// to leave it as it is.
    fn reference(&self, dir: &Path, value: &str) -> Result<Option<String>> {
        let url = self.rooted(dir, value)?.map(|url| format!("{}{url}", self.config.prefix()));
        Ok(url.filter(|url| url != value))
    }
}
