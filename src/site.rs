//! What crosses the stage boundary: the pages that render hands to optimize, and the built
//! site that optimize hands back.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::{self, DirEntry, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::paths;
use crate::store::Store;

/// What the render stage hands to the optimizer.
pub struct Rendered {
    pub pages: Vec<Page>,
    /// The text of each generated file, such as the sitemap, by its output path.
    pub files: BTreeMap<String, String>,
}

/// A rendered page. Relative references in `html` resolve against the directory of `source`.
pub struct Page {
    /// The absolute path of the page's source file, under the canonical site root.
    pub source: PathBuf,
    /// The output path, such as `about/index.html`.
    pub path: String,
    pub html: String,
}

/// A file of a built site.
pub enum Content {
    /// Bytes the build produced.
    Bytes(Vec<u8>),
    /// A source file that ships unchanged, which is read only when it is written or served.
    File(PathBuf),
}

/// A built site, held in memory.
pub struct Site {
    /// Every file by output path, such as `about/index.html`.
    pub files: BTreeMap<String, Content>,
    /// The path the site is deployed under, such as `/docs`, which `files` leaves out.
    pub prefix: String,
    /// The site's store, which knows the entries this build used.
    pub store: Store,
    /// Build notices printed by `cinders build` and `cinders dev`, such as scripts missing
    /// `data-client-nav`.
    pub notes: Vec<String>,
}

impl Page {
    /// The page's URL, such as `/about/` for `about/index.html`.
    pub fn url(&self) -> String {
        let path = self.path.strip_suffix("index.html").unwrap_or(&self.path);
        format!("/{}", paths::percent_encode(path))
    }
}

impl Site {
    /// Writes the site to `dir`, replacing what was there.
    pub fn write(&self, dir: &Path) -> Result<()> {
        // Files are replaced in place, since deleting a large site takes longer than writing it.
        if dir.exists() {
            self.remove_stale(dir, "").with_context(|| dir.display().to_string())?;
        }
        self.files.par_iter().try_for_each(|(path, content)| {
            let to = dir.join(path);
            // Most directories exist already, so one is created only when a write misses it.
            // A read-only file, as copied from a read-only source, is replaced.
            let written = content.write(&to).or_else(|_| {
                fs::create_dir_all(to.parent().unwrap_or(dir))?;
                fs::remove_file(&to).unwrap_or_default();
                content.write(&to)
            });
            written.with_context(|| to.display().to_string())
        })
    }

    /// Deletes what an earlier build left under `dir` that this site does not write, and
    /// returns whether anything is left. `prefix` is the output path of `dir`.
    fn remove_stale(&self, dir: &Path, prefix: &str) -> io::Result<bool> {
        let entries: Vec<DirEntry> = fs::read_dir(dir)?.collect::<io::Result<_>>()?;
        let kept = entries.par_iter().map(|entry| {
            let path = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let is_dir = entry.file_type()?.is_dir();
            let kept = match is_dir {
                true => self.remove_stale(&entry.path(), &format!("{path}/"))?,
                false => self.files.contains_key(&path),
            };
            if !kept && is_dir {
                fs::remove_dir(entry.path())?;
            } else if !kept {
                fs::remove_file(entry.path())?;
            }
            Ok(kept)
        });
        kept.try_reduce(|| false, |a, b| Ok(a || b))
    }
}

impl Content {
    /// The file's bytes, read from disk for a source file.
    pub fn bytes(&self) -> Result<Cow<'_, [u8]>> {
        Ok(match self {
            Content::Bytes(bytes) => bytes.into(),
            Content::File(path) => {
                fs::read(path).with_context(|| path.display().to_string())?.into()
            }
        })
    }

    /// Writes the file to `to`, whose directory has to exist.
    fn write(&self, to: &Path) -> io::Result<()> {
        match self {
            Content::Bytes(bytes) => {
                // Truncating first frees the file's blocks, which is slower than writing over
                // them.
                let mut file = File::options().write(true).create(true).truncate(false).open(to)?;
                file.write_all(bytes)?;
                file.set_len(bytes.len() as u64)
            }
            Content::File(from) => fs::copy(from, to).map(drop),
        }
    }
}
