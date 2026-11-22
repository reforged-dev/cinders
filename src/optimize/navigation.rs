//! Builds and injects the client-side navigation script.

use std::borrow::Cow;
use std::collections::BTreeSet;

use anyhow::Result;

use super::asset::{self, Kind};
use super::{Optimizer, script};
use crate::Mode;
use crate::config::Prefetch;
use crate::site::Content;
use crate::{paths, store};

const SOURCE: &str = include_str!("navigation.js");

impl Optimizer<'_> {
    /// Adds the navigation script to the site and returns its `<script>` tag. `scripts`
    /// identifies the site's scripts so any script change gives the file a new URL.
    pub(super) fn add_navigation(&self, scripts: &BTreeSet<String>) -> Result<String> {
        let fast = self.mode == Mode::Fast;
        let visible = self.config.navigation.prefetch == Prefetch::Visible;
        let mut code = format!("const DEV = {fast};\nconst VISIBLE = {visible};\n{SOURCE}");
        // Fast builds put user assets under `/_cinders/`, so `/assets/navigation.js` cannot collide.
        let mut path = "assets/navigation.js".to_string();
        if !fast {
            // Embed a hash of every script in the site so any script change gives this file a
            // new name, forcing a full load across deploys. Scripts in `public/` keep the same
            // URL when edited, so include their content fingerprint alongside their URL.
            let entries: Vec<Cow<'_, str>> = scripts
                .iter()
                .map(|script| {
                    let path = paths::decode(paths::split(script.trim()).path);
                    let public = path.strip_prefix('/').filter(|p| !p.starts_with("assets/"));
                    match public.and_then(|p| self.assets.fingerprint_at(p)) {
                        Some(content) => Cow::Owned(format!("{script} {content}")),
                        None => Cow::Borrowed(script.as_str()),
                    }
                })
                .collect();
            let parts: Vec<&[u8]> = entries.iter().map(|entry| entry.as_bytes()).collect();
            code = script::minify(&code, Kind::Module, self.mode)?;
            code = format!("{code}//{}\n", store::key(&parts));
            path = format!("assets/navigation-{}.js", asset::fingerprint(code.as_bytes()));
        }
        let tag =
            format!(r#"<script type="module" src="{}/{path}"></script>"#, self.config.prefix());
        self.assets.put(path, "cinders".to_string(), Content::Bytes(code.into_bytes()))?;
        Ok(tag)
    }
}

/// Formats a build notice when pages contain scripts missing `data-client-nav`.
pub(super) fn unmarked_note(unmarked: &BTreeSet<String>) -> Option<String> {
    if unmarked.is_empty() {
        return None;
    }
    let shown: Vec<&str> = unmarked.iter().take(3).map(String::as_str).collect();
    let list = match unmarked.len().saturating_sub(3) {
        0 => shown.join(", "),
        more => format!("{}, and {more} more", shown.join(", ")),
    };
    Some(format!(
        "pages with {list} will use full page loads \
         (add data-client-nav to opt in, or set [navigation] client = false)"
    ))
}
