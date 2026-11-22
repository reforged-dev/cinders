//! Watches the site directory, and decides when a save is built and which build is shown.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// How long a save's events must stop for before it is built.
const SETTLE: Duration = Duration::from_millis(1);

/// The saves in a site directory, as its watcher reports them.
pub(super) struct Saves {
    events: Receiver<notify::Result<Event>>,
    /// The directories that a build or git writes, where a change is no save.
    ignored: [PathBuf; 3],
    /// Held because the events end when the watcher is dropped.
    _watcher: RecommendedWatcher,
}

impl Saves {
    /// Starts watching the site in the canonical directory `root`.
    pub(super) fn watch(root: &Path) -> Result<Saves> {
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender)?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        let ignored = ["dist", ".cinders", ".git"].map(|dir| root.join(dir));
        Ok(Saves { events, ignored, _watcher: watcher })
    }

    /// Waits for a save, then calls `build` until no save arrives while it runs, and returns
    /// that last build. It returns `None` once the watcher has stopped.
    pub(super) fn rebuild<T>(&self, mut build: impl FnMut() -> T) -> Option<T> {
        while !self.is_save(&self.events.recv().ok()?) {}
        loop {
            // One save arrives as several events, so the build waits until they stop.
            while self.events.recv_timeout(SETTLE).is_ok() {}
            let built = build();
            // A save during the build makes it stale, so the site is built again before it
            // is shown.
            if !self.events.try_iter().any(|event| self.is_save(&event)) {
                return Some(built);
            }
        }
    }

    /// Whether `event` reports a change to a file that a build may read.
    fn is_save(&self, event: &notify::Result<Event>) -> bool {
        let outside = |path: &PathBuf| !self.ignored.iter().any(|dir| path.starts_with(dir));
        event.as_ref().is_ok_and(|event| {
            // A rescan means the system dropped events. On Linux it names no path.
            let rescan = event.need_rescan() && event.paths.is_empty();
            // inotify also reports reads, and the build's own would rebuild without end.
            !event.kind.is_access() && (rescan || event.paths.iter().any(outside))
        })
    }
}
