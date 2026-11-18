use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use cinders::{Content, Mode};

// musl's allocator takes one lock, which makes a parallel build slower with every thread.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE: &str = "usage: cinders build [DIR]";

const HELP: &str = "
  build         write the optimized site to DIR/dist/
  DIR           the site directory, which defaults to the current one
  --help        print this help
  --version     print the version";

/// What a command line asks for.
struct Args {
    /// The site directory.
    dir: String,
}

impl Args {
    /// Reads the arguments that follow the program's name, or `None` if they are not a
    /// command.
    fn parse(mut args: impl Iterator<Item = String>) -> Option<Args> {
        if args.next()? != "build" {
            return None;
        }
        let dir = args.next().unwrap_or_else(|| ".".to_string());
        (args.next().is_none() && !dir.starts_with('-')).then_some(Args { dir })
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
    match build(Path::new(&args.dir)) {
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
