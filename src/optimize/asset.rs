//! The files a build adds to the site: each referenced file once, under the name it ships
//! with.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use anyhow::{Result, anyhow, bail};

use crate::Mode;
use crate::paths;
use crate::site::Content;

/// A file is processed once, by the first thread to ask, and gives its URL. A failure is kept
/// as its message.
type Slot = Arc<OnceLock<Result<String, String>>>;

struct Output {
    /// The source that wrote the file, to name both sides of a collision.
    from: String,
    content: Content,
}

/// The site's files so far, and the source files that have joined them.
pub(super) struct Assets<'a> {
    root: &'a Path,
    mode: Mode,
    slots: Mutex<HashMap<PathBuf, Slot>>,
    files: Mutex<BTreeMap<String, Output>>,
}

impl<'a> Assets<'a> {
    pub(super) fn new(root: &'a Path, mode: Mode) -> Self {
        Assets { root, mode, slots: Mutex::default(), files: Mutex::default() }
    }

    /// Every file of the site, by its output path.
    pub(super) fn into_files(self) -> BTreeMap<String, Content> {
        let files = self.files.into_inner().unwrap_or_else(PoisonError::into_inner);
        files.into_iter().map(|(path, output)| (path, output.content)).collect()
    }

    /// Adds the source file `file` to the site, once however many references name it, and
    /// returns its URL.
    pub(super) fn asset(&self, file: &Path) -> Result<String> {
        let slot = lock(&self.slots).entry(file.to_path_buf()).or_default().clone();
        let process = || {
            let content = Content::File(file.to_path_buf());
            let url = self.emit(file, &paths::extension(file), content);
            url.map_err(|e| format!("{}: {e:#}", paths::name(self.root, file)))
        };
        slot.get_or_init(process).clone().map_err(|e| anyhow!(e))
    }

    /// Adds a file built from `file` to the site and returns its URL: fingerprinted under
    /// `/assets/` in an optimized build, and at its source path under `/_cinders/` in a fast
    /// one.
    fn emit(&self, file: &Path, extension: &str, content: Content) -> Result<String> {
        let from = paths::name(self.root, file);
        if self.mode == Mode::Fast {
            let source = paths::name(self.root, &file.with_extension(extension));
            let path = format!("_cinders/{source}");
            let url = format!("/{}", paths::percent_encode(&path));
            self.put(path, from, content)?;
            return Ok(url);
        }
        // Only URL-safe characters, so the URL needs no escaping in `srcset` or `url()`.
        let stem: String = (file.file_stem().unwrap_or_default().to_string_lossy().chars())
            .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
            .collect();
        let dot = if extension.is_empty() { "" } else { "." };
        let fingerprint = fingerprint(&content.bytes()?);
        let path = format!("assets/{stem}-{fingerprint}{dot}{extension}");
        let url = format!("/{path}");
        // Two files with the same name and bytes share one output, which is no collision.
        lock(&self.files).insert(path, Output { from, content });
        Ok(url)
    }

    /// Adds a file to the site. Two sources writing one path is an error.
    pub(super) fn put(&self, path: String, from: String, content: Content) -> Result<()> {
        let mut files = lock(&self.files);
        if let Some(other) = files.get(&path)
            && other.from != from
        {
            bail!("{} and {from} both write {path}", other.from);
        }
        files.insert(path, Output { from, content });
        Ok(())
    }
}

/// Locks `mutex`, poisoned or not, since a panic on any thread ends the build.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Eight URL-safe base64 characters of the content's BLAKE3 hash.
fn fingerprint(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let hash = blake3::hash(bytes);
    let mut out = String::with_capacity(8);
    let (triples, _) = hash.as_bytes().as_chunks::<3>();
    for &[a, b, c] in &triples[..2] {
        let bits = u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c);
        for shift in [18, 12, 6, 0] {
            out.push(char::from(ALPHABET[(bits >> shift & 63) as usize]));
        }
    }
    out
}
