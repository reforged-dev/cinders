//! The optimize stage: rendered pages in, the shipped site out. It reads only the rendered HTML
//! and the files it references, and knows nothing about templates.

mod asset;
mod css;
mod font;
mod html;
mod image;
mod minify;
mod reference;

use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use anyhow::{Context, Result};
use lightningcss::targets::Targets;
use rayon::prelude::*;

use crate::Mode;
use crate::config::Config;
use crate::paths;
use crate::site::{Content, Page, Rendered, Site};
use asset::{Assets, Kind};
use html::Found;
use reference::{References, Target};

/// What the passes over a page share.
struct Optimizer<'a> {
    root: &'a Path,
    config: &'a Config,
    mode: Mode,
    /// The browsers that stylesheets are lowered for.
    targets: Targets,
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
    let mut optimizer = Optimizer {
        root,
        config,
        mode,
        targets: css::targets(config)?,
        references: References::new(root, config),
        assets: Assets::new(root, config, mode),
    };
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

    optimizer.prepare(&rendered.pages)?;
    let pages = rendered.pages.par_iter().map(|page| optimizer.page(page));
    let pages: Vec<String> = pages.collect::<Result<_>>()?;
    let mut site = optimizer.assets.into_site();
    for (page, html) in rendered.pages.into_iter().zip(pages) {
        site.files.insert(page.path, Content::Bytes(html.into_bytes()));
    }
    Ok(site)
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

    /// Scans `pages`, then builds what the scan found: the characters that fonts are subset
    /// to, the stylesheets, and the images. The pass that rewrites each page's references
    /// then finds every result ready.
    fn prepare(&self, pages: &[Page]) -> Result<()> {
        let found = pages.par_iter().map(|page| html::scan(self, page));
        let found = found.reduce(Found::default, Found::merge);
        let mut chars = found.chars;
        if self.mode == Mode::Optimized {
            for file in &self.config.fonts.scan {
                let text = fs::read_to_string(self.root.join(file))
                    .with_context(|| format!("cinders.toml: [fonts] scan: {file}"))?;
                chars.extend(text.chars());
            }
            // CSS draws text that no page holds: its own strings, and the other case of a
            // letter under `text-transform`.
            for file in &found.styles {
                chars.extend(css::file_strings(file).chars());
            }
        }
        let cased: Vec<char> =
            chars.iter().flat_map(|c| c.to_uppercase().chain(c.to_lowercase())).collect();
        chars.extend(cased);
        self.assets.chars.set(chars.into_iter().collect()).unwrap_or_default();
        // A failure is kept, and the rewrite reports it with the page that asked.
        thread::scope(|threads| {
            // A stylesheet's build waits on the thread pool, so each runs on its own thread,
            // where it cannot pick up a second one.
            for file in &found.styles {
                threads.spawn(move || self.asset(file, Kind::Css));
            }
            found.images.par_iter().for_each(|file| {
                self.asset(file, Kind::Image).map(drop).unwrap_or_default();
            });
        });
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
    /// `None` if it ships as written. `value` is a reference written in a file in `dir`, and
    /// a file it names is processed as `kind`.
    fn rooted(&self, dir: &Path, value: &str, kind: Kind) -> Result<Option<String>> {
        Ok(match self.references.resolve(dir, value)? {
            Target::Unchanged => None,
            Target::Rooted(url) => Some(url),
            Target::File(file, suffix) => Some(format!("{}{suffix}", self.asset(&file, kind)?.url)),
        })
    }

    /// The URL to write in place of `value`, a reference written in a file in `dir`, or `None`
    /// to leave it as it is. A file it names is processed as `kind`.
    fn reference(&self, dir: &Path, value: &str, kind: Kind) -> Result<Option<String>> {
        let url = self.rooted(dir, value, kind)?;
        let url = url.map(|url| format!("{}{url}", self.config.prefix()));
        Ok(url.filter(|url| url != value))
    }
}

/// Locks `mutex`, poisoned or not, since a panic on any thread ends the build.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
