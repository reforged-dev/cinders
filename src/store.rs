//! The store: encoded images and subset fonts under `.cinders/`, keyed by content hash.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use std::{fs, process};

use anyhow::{Context, Result};
use blake3::Hasher;

use crate::paths;

pub struct Store {
    dir: PathBuf,
    /// The entries this build read or wrote, which pruning keeps.
    used: Mutex<HashSet<String>>,
}

/// The kinds of entry, each a directory in the store.
const KINDS: [&str; 2] = ["images", "fonts"];
/// The extension of an entry that is being written.
const PARTIAL: &str = "part";
/// How old a partial entry is when the build that wrote it must have died. A write takes
/// milliseconds.
const ABANDONED: Duration = Duration::from_secs(3600);

/// A key for an entry made from `parts`: the source bytes and every setting that shapes it.
pub fn key(parts: &[&[u8]]) -> String {
    let mut hasher = Hasher::new();
    for part in parts {
        // The length keeps two splits of the same bytes apart.
        hasher.update(&(part.len() as u64).to_le_bytes()).update(part);
    }
    hasher.finalize().to_hex()[..32].to_string()
}

impl Store {
    /// The store of the site in `root`, which need not exist yet.
    pub fn new(root: &Path) -> Store {
        Store { dir: root.join(".cinders"), used: Mutex::default() }
    }

    /// Reads the entry `name`, such as `fonts/<key>.woff2`.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let bytes = fs::read(self.dir.join(name)).ok()?;
        self.used().insert(name.to_string());
        Some(bytes)
    }

    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = self.dir.join(name);
        // Written aside and renamed, so a concurrent build never reads half an entry.
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let partial = path.with_extension(format!("{}-{number}.{PARTIAL}", process::id()));
        fs::create_dir_all(path.parent().unwrap_or(&self.dir))
            .and_then(|()| fs::write(&partial, bytes))
            .and_then(|()| fs::rename(&partial, &path))
            .with_context(|| path.display().to_string())?;
        self.used().insert(name.to_string());
        Ok(())
    }

    /// Deletes every entry this build neither read nor wrote. Another build may share the
    /// store, so an entry it is still writing stays, and one it has just deleted is no error.
    pub fn prune(&self) -> Result<()> {
        let used = self.used();
        for kind in KINDS {
            for entry in paths::entries(&self.dir.join(kind))? {
                let path = entry.path();
                let stale = match path.extension().is_some_and(|ext| ext == PARTIAL) {
                    true => {
                        let age = entry.metadata().and_then(|file| file.modified());
                        age.is_ok_and(|at| at.elapsed().is_ok_and(|age| age > ABANDONED))
                    }
                    false => {
                        !used.contains(&format!("{kind}/{}", entry.file_name().to_string_lossy()))
                    }
                };
                if stale
                    && let Err(e) = fs::remove_file(&path)
                    && e.kind() != ErrorKind::NotFound
                {
                    return Err(e).with_context(|| path.display().to_string());
                }
            }
        }
        Ok(())
    }

    /// The entries in use. A thread that panicked with the lock held left the set whole.
    fn used(&self) -> MutexGuard<'_, HashSet<String>> {
        self.used.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
