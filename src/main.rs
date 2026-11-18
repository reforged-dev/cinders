use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use cinders::{Content, Mode, dev};

// musl's allocator takes one lock, which makes a parallel build slower with every thread.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE: &str = "usage: cinders build [DIR]
       cinders dev [DIR] [--optimized] [--port N] [--host]";

const HELP: &str = "
  build         write the optimized site to DIR/dist/
  dev           serve the site on http://localhost:3000 and rebuild it on each save
  DIR           the site directory, which defaults to the current one
  --optimized   run every step of a build in dev, to see what ships
  --port N      listen on port N, where 0 takes a free port
  --host        listen on every network interface, to open the site from a phone
  --help        print this help
  --version     print the version";

/// What a command line asks for.
struct Args {
    /// Whether the command is `dev`, where the other is `build`.
    dev: bool,
    /// The site directory, if the command line names one.
    dir: Option<String>,
    /// The mode that `dev` builds in.
    mode: Mode,
    port: u16,
    /// Whether `dev` listens on every network interface.
    network: bool,
}

impl Args {
    /// Reads the arguments that follow the program's name, or `None` if they are not a
    /// command.
    fn parse(mut args: impl Iterator<Item = String>) -> Option<Args> {
        let command = args.next()?;
        let dev = command == "dev";
        if !dev && command != "build" {
            return None;
        }
        let mut parsed = Args { dev, dir: None, mode: Mode::Fast, port: 3000, network: false };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--optimized" if dev => parsed.mode = Mode::Optimized,
                "--host" if dev => parsed.network = true,
                "--port" if dev => parsed.port = args.next()?.parse().ok()?,
                _ if parsed.dir.is_none() && !arg.starts_with('-') => parsed.dir = Some(arg),
                _ => return None,
            }
        }
        Some(parsed)
    }
}

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("--help" | "-h") => {
            println!("Cinders builds a static site from HTML templates and assets.\n");
            println!("{USAGE}\n{HELP}");
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            println!("cinders {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let Some(args) = Args::parse(env::args().skip(1)) else {
        eprintln!("{USAGE}\nrun cinders --help for what each part does");
        return ExitCode::from(2);
    };
    let dir = Path::new(args.dir.as_deref().unwrap_or("."));
    let result = match args.dev {
        true => dev::serve(dir, args.mode, args.port, args.network),
        false => build(dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the optimized site to `dist/` in `dir`, and prints what it wrote.
fn build(dir: &Path) -> Result<()> {
    let start = Instant::now();
    let site = cinders::build(dir, Mode::Optimized)?;
    let dist = dir.join("dist");
    site.write(&dist)?;
    site.store.prune()?;
    let pages = site.files.keys().filter(|path| path.ends_with(".html")).count();
    let sizes = site.files.values().map(|content| match content {
        Content::Bytes(bytes) => bytes.len() as u64,
        Content::File(path) => path.metadata().map_or(0, |file| file.len()),
    });
    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    eprintln!(
        "built {} and {} in {}, {} bytes in {} ms",
        count(pages, "page"),
        count(site.files.len() - pages, "other file"),
        dist.display(),
        sizes.sum::<u64>(),
        start.elapsed().as_millis()
    );
    Ok(())
}
