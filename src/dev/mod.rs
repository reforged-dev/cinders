//! `cinders dev`: serves the site from memory, rebuilds it on every save, and reloads the
//! browser.

mod http;
mod watch;

use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::{Mode, Site};
use watch::Saves;

/// One build and its number. A request takes both at once, so it never sees half a site.
struct Build {
    number: u64,
    site: Result<Site>,
}

/// The build that requests are answered from.
struct Dev {
    build: Mutex<Arc<Build>>,
    /// Wakes the event streams when a new build is swapped in.
    swapped: Condvar,
}

/// Serves the site in `root` on `port` until the process ends: on the loopback interface, or
/// on every interface if `network` is set, which shows the site to every machine on the
/// network.
pub fn serve(root: &Path, mode: Mode, port: u16, network: bool) -> Result<()> {
    let root = root.canonicalize().with_context(|| root.display().to_string())?;
    // Listening before the first build lets a browser connect while it runs.
    let interface = if network { Ipv4Addr::UNSPECIFIED } else { Ipv4Addr::LOCALHOST };
    let listener = TcpListener::bind((interface, port))
        .with_context(|| format!("cannot listen on port {port}"))?;
    let port = listener.local_addr()?.port();
    eprintln!("serving http://localhost:{port}");
    // Connecting a UDP socket sends nothing, and tells which address reaches the network.
    let reach = |socket: UdpSocket| socket.connect("192.0.2.1:9").and(socket.local_addr());
    if network && let Ok(address) = UdpSocket::bind("0.0.0.0:0").and_then(reach) {
        eprintln!("on the network: http://{}:{port}", address.ip());
    }
    // Watching before the first build catches a save that arrives during it.
    let saves = Saves::watch(&root)?;
    // The clock seeds the build number, so a page from before a restart sees a new one.
    let number = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let first = Arc::new(build(&root, mode, number));
    let dev = Arc::new(Dev { build: Mutex::new(first), swapped: Condvar::new() });
    let rebuilt = Arc::clone(&dev);
    thread::spawn(move || {
        // The number goes up by one for each build that is shown, failed ones included.
        for number in number + 1.. {
            let Some(next) = saves.rebuild(|| build(&root, mode, number)) else { break };
            rebuilt.swap(next);
        }
    });
    for stream in listener.incoming().flatten() {
        let dev = Arc::clone(&dev);
        thread::spawn(move || http::respond(&dev, &stream).unwrap_or_default());
    }
    Ok(())
}

/// Builds the site as the build `number`, and prints how it went.
fn build(root: &Path, mode: Mode, number: u64) -> Build {
    let start = Instant::now();
    let site = crate::build(root, mode);
    match &site {
        Ok(_) => eprintln!("built in {} ms", start.elapsed().as_millis()),
        Err(e) => eprintln!("error: {e:#}"),
    }
    Build { number, site }
}

impl Dev {
    /// The build to answer a request from.
    fn current(&self) -> Arc<Build> {
        Arc::clone(&self.lock())
    }

    /// Answers from `build` from now on, and tells the event streams.
    fn swap(&self, build: Build) {
        *self.lock() = Arc::new(build);
        self.swapped.notify_all();
    }

    /// Waits for a build whose number is not `sent`, and returns its number. After `limit`
    /// with no such build, it returns `sent`.
    fn wait(&self, sent: u64, limit: Duration) -> u64 {
        let same = |build: &mut Arc<Build>| build.number == sent;
        let waited = self.swapped.wait_timeout_while(self.lock(), limit, same);
        let (build, _) = waited.unwrap_or_else(PoisonError::into_inner);
        build.number
    }

    /// A poisoned lock is taken anyway, since a swap leaves a whole build behind.
    fn lock(&self) -> MutexGuard<'_, Arc<Build>> {
        self.build.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
