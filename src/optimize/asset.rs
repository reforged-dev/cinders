//! The files a build adds to the site: each referenced file once, processed by its kind, under
//! the name it ships with. Encoded images and subset fonts come from the store.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};

use super::image::Encoded;
use super::{Optimizer, css, font, image, script};
use crate::Mode;
use crate::config::Config;
use crate::paths;
use crate::site::{Content, Site};
use crate::store::{self, Store};

/// How a referenced file is processed.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Kind {
    /// Shipped as it is.
    Raw,
    /// A stylesheet, bundled with its imports.
    Css,
    /// A module script, joined with the modules it imports.
    Module,
    /// A classic script, minified alone.
    Classic,
    /// An `<img>` source, encoded to AVIF if it is a PNG or JPEG.
    Image,
    /// A file that CSS or a font preload names, subset to WOFF2 if it is a TTF or OTF font.
    Font,
}

/// A processed file.
#[derive(Clone)]
pub(super) struct Asset {
    pub(super) url: String,
    /// The width and height in pixels of an encoded image.
    pub(super) size: Option<(u32, u32)>,
}

/// A file is processed once per kind, by the first thread to ask. A failure is kept as its
/// message.
type Slot = Arc<OnceLock<Result<Asset, String>>>;

struct Output {
    /// The source that wrote the file, to name both sides of a collision.
    from: String,
    content: Content,
}

/// The site's files so far, and the source files that have joined them.
pub(super) struct Assets<'a> {
    root: &'a Path,
    config: &'a Config,
    mode: Mode,
    /// The characters the site draws, set once every page is scanned.
    pub(super) chars: OnceLock<String>,
    slots: Mutex<HashMap<(PathBuf, Kind), Slot>>,
    files: Mutex<BTreeMap<String, Output>>,
    store: Store,
}

impl Optimizer<'_> {
    /// Processes `file` as `kind`, once however many references name it.
    pub(super) fn asset(&self, file: &Path, kind: Kind) -> Result<Asset> {
        let optimized = self.mode == Mode::Optimized;
        let kind = match (kind, paths::extension(file).as_str()) {
            (Kind::Image, "png" | "jpg" | "jpeg") => Kind::Image,
            (Kind::Font, "ttf" | "otf") if optimized => Kind::Font,
            (Kind::Image | Kind::Font, _) => Kind::Raw,
            _ => kind,
        };
        let key = (file.to_path_buf(), kind);
        let slot = Arc::clone(super::lock(&self.assets.slots).entry(key).or_default());
        let process = || {
            self.process(file, kind).map_err(|e| match kind {
                // A module's error names the module and the line of the problem itself.
                Kind::Module => format!("{e:#}"),
                _ => format!("{}: {e:#}", paths::name(self.root, file)),
            })
        };
        slot.get_or_init(process).clone().map_err(|e| anyhow!(e))
    }

    fn process(&self, file: &Path, kind: Kind) -> Result<Asset> {
        let assets = &self.assets;
        let mut size = None;
        let (extension, content) = match kind {
            Kind::Raw => (paths::extension(file), Content::File(file.to_path_buf())),
            Kind::Css => ("css".to_string(), Content::Bytes(css::file(self, file)?.into_bytes())),
            Kind::Module => {
                ("js".to_string(), Content::Bytes(script::join(self.root, file, self.mode)?))
            }
            Kind::Classic => {
                let code = script::minify(&fs::read_to_string(file)?, kind, self.mode)?;
                ("js".to_string(), Content::Bytes(code.into_bytes()))
            }
            Kind::Font => ("woff2".to_string(), Content::Bytes(assets.subset(file)?)),
            Kind::Image => match assets.encode(file)? {
                Some(image) => {
                    size = Some((image.width, image.height));
                    ("avif".to_string(), Content::Bytes(image.avif))
                }
                None => (paths::extension(file), Content::File(file.to_path_buf())),
            },
        };
        Ok(Asset { url: assets.emit(file, &extension, content)?, size })
    }
}

impl<'a> Assets<'a> {
    pub(super) fn new(root: &'a Path, config: &'a Config, mode: Mode) -> Self {
        Assets {
            root,
            config,
            mode,
            chars: OnceLock::new(),
            slots: Mutex::default(),
            files: Mutex::default(),
            store: Store::new(root),
        }
    }

    /// The built site, which holds every file added so far and the store they came from.
    pub(super) fn into_site(self) -> Site {
        let files = self.files.into_inner().unwrap_or_else(PoisonError::into_inner);
        let files = files.into_iter().map(|(path, output)| (path, output.content)).collect();
        Site {
            files,
            prefix: self.config.prefix().to_string(),
            store: self.store,
            notes: Vec::new(),
        }
    }

    /// Encodes the image `file`, or reads the encode from the store. Returns `None` for an
    /// animated PNG, which ships unchanged, because the encoder writes only one frame.
    fn encode(&self, file: &Path) -> Result<Option<Encoded>> {
        let source = fs::read(file)?;
        if image::is_animated(&source) {
            return Ok(None);
        }
        let images = &self.config.images;
        let scaling = image::scaling(&source, images.max_width);
        let settings = image::SETTINGS.as_bytes();
        let key = store::key(&[&source, settings, &[images.avif], scaling.as_bytes()]);
        let avif = format!("images/{key}.avif");
        let size = format!("images/{key}.size");
        let stored = || {
            let size = String::from_utf8(self.store.read(&size)?).ok()?;
            let (width, height) = size.split_once(' ')?;
            let (width, height) = (width.parse().ok()?, height.parse().ok()?);
            Some(Encoded { avif: self.store.read(&avif)?, width, height })
        };
        if let Some(image) = stored() {
            return Ok(Some(image));
        }
        let start = Instant::now();
        let image = image::encode(&source, images.avif, images.max_width)?;
        let name = paths::name(self.root, file);
        eprintln!("encoded {name} in {:.1} s", start.elapsed().as_secs_f32());
        self.store.write(&avif, &image.avif)?;
        self.store.write(&size, format!("{} {}", image.width, image.height).as_bytes())?;
        Ok(Some(image))
    }

    /// Subsets the font `file` to the site's characters, or takes the subset from the store.
    fn subset(&self, file: &Path) -> Result<Vec<u8>> {
        let source = fs::read(file)?;
        let chars = self.chars.get().map_or("", String::as_str);
        let key = store::key(&[&source, chars.as_bytes(), font::SETTINGS.as_bytes()]);
        let name = format!("fonts/{key}.woff2");
        if let Some(font) = self.store.read(&name) {
            return Ok(font);
        }
        let font = font::subset(&source, chars)?;
        self.store.write(&name, &font)?;
        Ok(font)
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
        super::lock(&self.files).insert(path, Output { from, content });
        Ok(url)
    }

    /// Returns the content fingerprint of the output file at `path`, if one exists.
    pub(super) fn fingerprint_at(&self, path: &str) -> Option<String> {
        let files = super::lock(&self.files);
        files.get(path)?.content.bytes().ok().map(|bytes| fingerprint(&bytes))
    }

    /// Adds a file to the site. Two sources writing one path is an error.
    pub(super) fn put(&self, path: String, from: String, content: Content) -> Result<()> {
        let mut files = super::lock(&self.files);
        if let Some(other) = files.get(&path)
            && other.from != from
        {
            bail!("{} and {from} both write {path}", other.from);
        }
        // In a fast build, a plain link to a stylesheet or a script must not replace its bundle.
        if matches!(content, Content::File(_)) && files.contains_key(&path) {
            return Ok(());
        }
        files.insert(path, Output { from, content });
        Ok(())
    }
}

/// Eight URL-safe base64 characters of the content's BLAKE3 hash.
pub(super) fn fingerprint(bytes: &[u8]) -> String {
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
