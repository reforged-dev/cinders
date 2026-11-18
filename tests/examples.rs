//! Builds the example site and checks the outputs that matter to it.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;
use std::path::Path;

use common::{assets, build, copy_dir, text};

#[test]
fn landing() {
    // The build runs on a copy, so the tree is never written.
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/landing");
    let copy = Path::new(env!("CARGO_TARGET_TMPDIR")).join("examples/landing");
    fs::remove_dir_all(&copy).unwrap_or_default();
    copy_dir(&example, &copy, "dist");
    let site = build(&copy);

    let routes: Vec<_> = site.files.keys().filter(|path| !path.starts_with("assets/")).collect();
    let shipped = ["404.html", "cabins/index.html", "index.html", "robots.txt", "sitemap.xml"];
    assert_eq!(routes, shipped, "routes and generated files");
    assert!(!text(&site, "sitemap.xml").contains("404"), "the 404 page is no route to list");

    let home = text(&site, "index.html");
    assert!(home.contains("<a class=button href=/cabins/>View the cabins</a>"), "{home}");
    let preview = "<meta property=og:image content=https://tarn.example/assets/preview-";
    assert!(home.contains(preview), "a link preview names its image in full: {home}");
    assert_eq!(assets(&site, "png").len(), 1, "and the image ships as it is");
    assert!(home.contains("<source srcset=/assets/lake-dark-"), "{home}");
    assert!(home.contains("<img src=/assets/lake-light-"), "{home}");
    assert!(text(&site, "404.html").contains("<a class=button href=/>"), "the 404 page links home");

    let cabins = text(&site, "cabins/index.html");
    assert_eq!(cabins.matches("<li>").count(), 6, "one row for each cabin in [site]: {cabins}");
    let first = cabins.find("<span class=cabin-name>Heron</span>");
    let last = cabins.find("<span class=cabin-name>Juniper</span>");
    assert!(first.is_some() && first < last, "in the order of cinders.toml: {cabins}");
    assert!(cabins.contains("<span class=cabin-price>$210 a night</span>"), "{cabins}");
    assert!(cabins.contains("<span class=cabin-sleeps>Sleeps 6</span>"), "{cabins}");

    assert!(assets(&site, "avif").is_empty(), "the example encodes no image");
    assert!(!copy.join(".cinders").exists(), "so its build writes no store");
}
