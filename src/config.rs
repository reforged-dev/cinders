//! The site's `cinders.toml`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Exposed to templates as `site`. `url` is the site's address, which the sitemap needs,
    /// and whose path every rooted reference is written under.
    pub site: toml::Table,
    /// The oldest browsers that CSS syntax is lowered for, such as `safari 16.4`.
    pub browsers: Option<Vec<String>>,
    /// Rooted URL prefixes that exist only after the build, so references under them are not
    /// checked.
    pub unchecked: Vec<String>,
    pub fonts: Fonts,
    pub images: Images,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Fonts {
    /// Files whose characters join the font subset, for text that scripts insert.
    pub scan: Vec<String>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Images {
    /// The AVIF encoder's quality, from 0 to 100.
    pub avif: u8,
    /// The most pixels across that an encoded image keeps. A wider one is scaled down.
    pub max_width: u32,
}

impl Default for Images {
    fn default() -> Self {
        Images { avif: 59, max_width: 2560 }
    }
}

impl Config {
    /// Reads `cinders.toml` in `root`. A site without one gets the defaults.
    pub fn load(root: &Path) -> Result<Config> {
        let path = root.join("cinders.toml");
        let text = match fs::read_to_string(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            text => text.with_context(|| path.display().to_string())?,
        };
        let config: Config = toml::from_str(&text).context("cinders.toml")?;
        ensure!(config.images.avif <= 100, "cinders.toml: [images] avif runs from 0 to 100");
        ensure!(config.images.max_width > 0, "cinders.toml: [images] max_width is at least 1");
        ensure!(
            config.unchecked.iter().all(|prefix| prefix.starts_with('/')),
            "cinders.toml: each `unchecked` prefix starts with `/`"
        );
        ensure!(
            config.site.get("url").is_none_or(|url| url.as_str().is_some_and(is_address)),
            "cinders.toml: [site] url is an address with no query, as in https://example.com/docs"
        );
        Ok(config)
    }

    /// The base URL without a trailing slash, if `[site] url` is set.
    pub fn base_url(&self) -> Option<&str> {
        self.site.get("url")?.as_str().map(|url| url.trim_end_matches('/'))
    }

    /// The path of `[site] url`, such as `/docs`, which goes before every rooted reference. It
    /// is empty for a site at the root of its host.
    pub fn prefix(&self) -> &str {
        let url = self.base_url().unwrap_or_default();
        let host = url.find("://").map_or(0, |scheme| scheme + 3);
        url[host..].find('/').map_or("", |path| &url[host + path..])
    }
}

/// Whether `url` is whole, with a scheme and a host, since the prefix is read from it.
fn is_address(url: &str) -> bool {
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    host.is_some_and(|host| {
        let malformed = |c: char| c.is_whitespace() || "?#".contains(c);
        !host.is_empty()
            && !host.starts_with('/')
            && !host.contains("//")
            && !host.contains(malformed)
    })
}
