//! Cinders builds a static site from HTML templates and assets.

pub mod config;
mod optimize;
pub mod paths;
pub mod render;
mod site;
pub mod store;

use std::path::Path;

use anyhow::{Context, Result};

use config::Config;
pub use site::{Content, Page, Rendered, Site};

/// Which optimizer steps a build runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every step, as `cinders build` and `cinders dev --optimized` run.
    Optimized,
    /// Scripts and stylesheets are bundled, and nothing is minified, fingerprinted, encoded, or
    /// subset. Assets keep their source paths under `/_cinders/`.
    Fast,
}

/// Renders and optimizes the site in the directory `root`.
pub fn build(root: &Path, mode: Mode) -> Result<Site> {
    let root = root.canonicalize().with_context(|| root.display().to_string())?;
    let config = Config::load(&root)?;
    let rendered = render::render(&root, &config)?;
    optimize::optimize(&root, &config, rendered, mode)
}
