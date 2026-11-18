//! Helpers shared by the test binaries.
#![allow(dead_code, reason = "each test binary uses only some of the helpers")]

use std::fs;
use std::path::{Path, PathBuf};

use cinders::config::Config;
use cinders::{Mode, Rendered, Site};

/// The committed fixture site.
pub(crate) fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/site")
}

/// Writes `files` into a fresh site directory under the target directory and returns it.
pub(crate) fn site(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::remove_dir_all(&root).unwrap_or_default();
    for (path, text) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    root
}

/// Copies the directory `from` to `to`, leaving out entries named `skip`.
pub(crate) fn copy_dir(from: &Path, to: &Path, skip: &str) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_name() == skip {
            continue;
        } else if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target, skip);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Builds the site as `cinders build` does, without writing it.
pub(crate) fn build(root: &Path) -> Site {
    cinders::build(root, Mode::Optimized).unwrap_or_else(|e| panic!("{e:#}"))
}

/// Builds the site as `cinders dev` does.
pub(crate) fn build_fast(root: &Path) -> Site {
    cinders::build(root, Mode::Fast).unwrap_or_else(|e| panic!("{e:#}"))
}

/// The message of the error that building the site fails with.
pub(crate) fn error(root: &Path) -> String {
    match cinders::build(root, Mode::Optimized) {
        Ok(_) => panic!("the build should fail"),
        Err(e) => format!("{e:#}"),
    }
}

/// The text of the built file at `path`.
pub(crate) fn text(site: &Site, path: &str) -> String {
    let content = site.files.get(path).unwrap_or_else(|| panic!("{path} is not in the site"));
    String::from_utf8(content.bytes().unwrap().into_owned()).unwrap()
}

/// The output paths of the fingerprinted assets with the extension `extension`.
pub(crate) fn assets<'a>(site: &'a Site, extension: &str) -> Vec<&'a str> {
    let suffix = format!(".{extension}");
    let assets = site.files.keys().filter(|path| path.starts_with("assets/"));
    assets.filter(|path| path.ends_with(&suffix)).map(String::as_str).collect()
}

/// The text of the only fingerprinted asset with the extension `extension`.
pub(crate) fn asset(site: &Site, extension: &str) -> String {
    let paths = assets(site, extension);
    assert_eq!(paths.len(), 1, "one .{extension} asset: {paths:?}");
    text(site, paths[0])
}

/// Runs the render stage alone, so a test sees the HTML before the optimizer rewrites it.
pub(crate) fn render(root: &Path) -> anyhow::Result<Rendered> {
    let root = root.canonicalize()?;
    cinders::render::render(&root, &Config::load(&root)?)
}

/// The rendered HTML of the page at the output path `path`.
pub(crate) fn rendered(root: &Path, path: &str) -> String {
    let pages = render(root).unwrap_or_else(|e| panic!("{e:#}")).pages;
    let page = pages.into_iter().find(|page| page.path == path);
    page.unwrap_or_else(|| panic!("{path} is not a page")).html
}

/// The message of the error that rendering the site fails with.
pub(crate) fn render_error(root: &Path) -> String {
    match render(root) {
        Ok(_) => panic!("the render should fail"),
        Err(e) => format!("{e:#}"),
    }
}
