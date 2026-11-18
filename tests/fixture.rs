//! Builds the committed fixture site and checks what ships.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;
use std::path::Path;

use cinders::Content;
use common::{build, build_fast, fixture, text};

#[test]
fn build_ships_pages_and_assets() {
    let site = build(&fixture());
    let paths: Vec<_> = site.files.keys().filter(|path| !path.starts_with("assets/")).collect();
    let shipped = [
        "404.html",
        "about/index.html",
        "index.html",
        "notes/index.html",
        "robots.txt",
        "sitemap.xml",
    ];
    assert_eq!(paths, shipped, "routes, generated files, and public files");

    let home = text(&site, "index.html");
    assert!(home.starts_with("<!doctype html><html lang=en><meta charset=utf-8>"), "{home}");
    assert!(!home.contains("<!--") && !home.contains("</body>"), "{home}");
    assert!(home.contains("<pre>  kept\n    as written  </pre>"), "{home}");
    assert!(home.contains("<a href=/about/>about</a>, or <a href=/search/>"), "{home}");
    assert!(home.contains(r#"{ "@type": "WebSite" }"#), "other script types stay: {home}");
    assert!(home.contains(r#"style="background-image: url(/assets/mark-"#), "{home}");
    assert!(home.contains("<img src=/assets/mark-y0yc32kQ.svg alt=Mark>"), "{home}");
    assert!(home.contains("srcset=\"/assets/shot-omOWxS2b.png 1x, /assets/photo-"), "{home}");

    let notes = text(&site, "notes/index.html");
    let head = "<!doctype html><html lang=en><meta charset=utf-8><title>Notes</title>\
                <link rel=stylesheet href=/assets/site-";
    assert!(notes.starts_with(head), "{notes}");
    assert_eq!(notes.matches("<style>").count(), 1, "a component's style hoists once: {notes}");
    assert_eq!(notes.matches("<script").count(), 1, "and so does its script: {notes}");
    let mark = "<aside class=\"note note-tip\"><img src=/assets/mark-y0yc32kQ.svg alt>";
    assert!(notes.contains(mark), "a component's paths resolve from its file: {notes}");
    assert_eq!(notes.matches("<img src=/assets/mark-y0yc32kQ.svg alt>").count(), 2, "{notes}");

    let sitemap = text(&site, "sitemap.xml");
    assert!(sitemap.contains("<loc>https://fixture.example/about/</loc>"), "{sitemap}");
    assert!(!sitemap.contains("404"), "{sitemap}");
    assert_eq!(text(&site, "robots.txt"), "User-agent: *\nAllow: /\n", "public/ has its own");
}

#[test]
fn fast_build_keeps_source_paths() {
    let site = build_fast(&fixture());
    assert!(!site.files.keys().any(|path| path.starts_with("assets/")));
    let home = text(&site, "index.html");
    assert!(home.contains("<!-- A comment"), "pages are not minified: {home}");
    assert!(home.contains(r#"<img src="/_cinders/images/mark.svg" alt="Mark">"#), "{home}");
    assert!(home.contains(r#"<a href="/about/">"#), "{home}");
    assert!(matches!(site.files["_cinders/images/shot.png"], Content::File(_)));
}

#[test]
fn write_replaces_the_output_directory() {
    let site = build(&fixture());
    let dist = Path::new(env!("CARGO_TARGET_TMPDIR")).join("written-fixture");
    fs::remove_dir_all(&dist).unwrap_or_default();
    // What an earlier build left: stale directories, a longer file, a file where a directory
    // goes, and a directory where a file goes.
    fs::create_dir_all(dist.join("old/deep")).unwrap();
    fs::write(dist.join("old/deep/stale.html"), "stale").unwrap();
    fs::write(dist.join("index.html"), "x".repeat(100_000)).unwrap();
    fs::write(dist.join("about"), "a file").unwrap();
    fs::create_dir_all(dist.join("robots.txt/inner")).unwrap();
    site.write(&dist).unwrap();
    assert!(!dist.join("old").exists());
    for (path, content) in &site.files {
        assert_eq!(fs::read(dist.join(path)).unwrap(), *content.bytes().unwrap(), "{path}");
    }
}
