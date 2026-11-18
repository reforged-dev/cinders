//! Helpers shared by the test binaries.
#![allow(dead_code, reason = "each test binary uses only some of the helpers")]

use std::fs;
use std::path::{Path, PathBuf};

use cinders::Site;

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
