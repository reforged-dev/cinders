//! Cinders builds a static site from HTML templates and assets.

pub mod config;
pub mod paths;
mod site;
pub mod store;

pub use site::{Content, Page, Site};

/// Which optimizer steps a build runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every step, as `cinders build` and `cinders dev --optimized` run.
    Optimized,
    /// Scripts and stylesheets are bundled, and nothing is minified, fingerprinted, encoded, or
    /// subset. Assets keep their source paths under `/_cinders/`.
    Fast,
}
