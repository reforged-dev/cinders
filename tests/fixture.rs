//! Builds the committed fixture site and checks what ships.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cinders::Content;
use common::{asset, assets, build, build_fast, copy_dir, fixture, text};

/// A scratch copy of the fixture, including its store. Tests build the copy because a build
/// writes missing store entries, and the committed store must not change.
fn copied() -> &'static Path {
    static COPY: OnceLock<PathBuf> = OnceLock::new();
    COPY.get_or_init(|| {
        let copy = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fixture");
        fs::remove_dir_all(&copy).unwrap_or_default();
        copy_dir(&fixture(), &copy, "dist");
        copy
    })
}

/// The names of the entries of `kind` in the store of the site in `root`.
fn entries(root: &Path, kind: &str) -> BTreeSet<OsString> {
    let entries = fs::read_dir(root.join(".cinders").join(kind)).unwrap();
    entries.map(|entry| entry.unwrap().file_name()).collect()
}

#[test]
fn build_ships_pages_and_assets() {
    let site = build(copied());
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
    for route in shipped.iter().filter(|path| path.ends_with(".html")) {
        let html = text(&site, route);
        assert!(!html.contains("../"), "every relative reference is rewritten: {html}");
    }

    let home = text(&site, "index.html");
    let head = "<!doctype html><html lang=en><script type=module src=/assets/navigation-";
    assert!(home.starts_with(head) && home.contains("</script><meta charset=utf-8>"), "{home}");
    assert!(!home.contains("<!--") && !home.contains("</body>"), "{home}");
    assert!(home.contains("<pre>  kept\n    as written  </pre>"), "{home}");
    assert!(home.contains("<a href=/about/>about</a>, or <a href=/search/>"), "{home}");
    assert!(home.contains(r#"{ "@type": "WebSite" }"#), "other script types stay: {home}");
    assert!(home.contains("<script>var visits=1;"), "inline scripts are minified: {home}");
    assert!(home.contains("<style>.hero{background:url(/assets/mark-"), "{home}");
    assert!(home.contains(r#"style="background-image: url(/assets/mark-"#), "{home}");
    assert!(home.contains(".woff2 as=font type=font/woff2 crossorigin>"), "{home}");

    let notes = text(&site, "notes/index.html");
    let head = "</script><meta charset=utf-8><title>Notes</title>\
                <link rel=stylesheet href=/assets/site-";
    assert!(notes.starts_with("<!doctype html><html lang=en><script") && notes.contains(head));
    let hoisted = ".css><style>.note{border-left:4px solid}</style>\
                   <script type=module src=/assets/app-";
    assert!(notes.contains(hoisted), "a component's style hoists once: {notes}");
    assert_eq!(notes.matches("/assets/app-").count(), 1, "and so does its script: {notes}");
    let mark = "<aside class=\"note note-tip\"><img src=/assets/mark-y0yc32kQ.svg alt>";
    assert!(notes.contains(mark), "a component's paths resolve from its file: {notes}");
    assert_eq!(notes.matches("<img src=/assets/mark-y0yc32kQ.svg alt>").count(), 2, "{notes}");

    let sitemap = text(&site, "sitemap.xml");
    assert!(sitemap.contains("<loc>https://fixture.example/about/</loc>"), "{sitemap}");
    assert!(!sitemap.contains("404"), "{sitemap}");
    assert_eq!(text(&site, "robots.txt"), "User-agent: *\nAllow: /\n", "public/ has its own");
}

#[test]
fn build_encodes_images() {
    let site = build(copied());
    let home = text(&site, "index.html");
    let shot = "<img src=/assets/shot-FSVOwGIj.avif alt=\"A terminal\" width=64 height=48>";
    assert!(home.contains(shot), "an encoded image is a plain `<img>` with its size: {home}");
    assert_eq!(home.matches("<picture>").count(), 1, "and has no fallback around it: {home}");
    assert!(home.contains(".avif alt=\"A photo\" width=32 height=24>"), "{home}");
    assert!(home.contains("<img src=/assets/mark-y0yc32kQ.svg alt=Mark>"), "{home}");
    assert!(
        home.contains("<img src=/assets/shot-omOWxS2b.png alt=Hand-tuned>"),
        "a `<picture>` the author wrote keeps its sources, fingerprinted: {home}"
    );
    assert!(home.contains("srcset=\"/assets/shot-omOWxS2b.png 1x, /assets/photo-"), "{home}");
    assert_eq!(assets(&site, "avif").len(), 3);
}

#[test]
fn build_bundles_scripts_and_styles() {
    let site = build(copied());
    let css = asset(&site, "css");
    assert!(css.starts_with("body{margin:0;"), "imports are bundled and minified: {css}");
    assert!(css.contains(".hero h1{color:red}"), "nesting is lowered: {css}");
    assert!(
        css.contains(r#".woff2)format("woff2")"#) && css.contains("url(/assets/mark-"),
        "{css}"
    );
    assert_eq!(assets(&site, "woff2").len(), 1);

    let scripts = assets(&site, "js");
    assert_eq!(scripts.len(), 3, "one file per script, shared by the pages: {scripts:?}");
    assert!(scripts[1].starts_with("assets/navigation-"), "and the navigation script");
    let app = text(&site, scripts[0]);
    assert!(app.contains("toUpperCase") && app.contains("Hello, "), "imports are joined: {app}");
    assert!(!app.contains("tree shaking") && !app.contains("HTMLElement"), "{app}");
    let status = text(&site, scripts[2]);
    assert!(
        status.contains("var statusText=") && status.contains("function showStatus"),
        "{status}"
    );
}

#[test]
fn fast_build_skips_the_slow_steps() {
    let site = build_fast(&fixture());
    let fingerprinted: Vec<_> =
        site.files.keys().filter(|path| path.starts_with("assets/")).collect();
    assert_eq!(fingerprinted, ["assets/navigation.js"], "only the navigation script is there");
    let home = text(&site, "index.html");
    assert!(home.contains("<!-- A comment"), "pages are not minified: {home}");
    assert!(home.contains(r#"<img src="/_cinders/images/shot.png" alt="A terminal">"#), "{home}");
    assert!(home.contains(r#"<img src="/_cinders/images/mark.svg" alt="Mark">"#), "{home}");
    assert!(home.contains(r#"src="/_cinders/scripts/app.js""#), "{home}");
    assert!(home.contains("var visits = 1;"), "an inline script stays as written: {home}");
    assert!(home.contains(r#"href="/_cinders/fonts/GeistMono.ttf" as="font" type="font/ttf""#));
    assert!(home.contains(r#"<a href="/about/">"#), "{home}");
    assert!(home.contains(".hero {\n  background: url(/_cinders/images/mark.svg);"), "{home}");
    assert!(matches!(site.files["_cinders/images/shot.png"], Content::File(_)));
    let css = text(&site, "_cinders/styles/site.css");
    assert!(css.contains("margin: 0"), "imports are bundled: {css}");
    assert!(css.contains(r#"url(/_cinders/fonts/GeistMono.ttf) format("truetype")"#), "{css}");
    let app = text(&site, "_cinders/scripts/app.js");
    assert!(app.contains("function greet(name)"), "joined, with its types stripped: {app}");
    assert!(app.contains("toUpperCase"), "{app}");
    assert!(!fixture().join("dist").exists(), "a build writes nothing by itself");
}

/// The fixture's committed store holds what this platform encodes, and nothing else.
#[test]
fn store_matches_fresh_encodes() {
    let fresh = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fresh");
    fs::remove_dir_all(&fresh).unwrap_or_default();
    copy_dir(&fixture(), &fresh, ".cinders");
    let stored = build(copied());
    let encoded = build(&fresh);
    for (path, content) in &stored.files {
        let same = *content.bytes().unwrap() == *encoded.files[path].bytes().unwrap();
        assert!(same, "{path} differs between the committed store and a fresh encode");
    }
    assert_eq!(stored.files.len(), encoded.files.len());
    for kind in ["images", "fonts"] {
        let committed = entries(&fixture(), kind);
        let fresh = entries(&fresh, kind);
        assert_eq!(committed, fresh, "stale or missing entries in the fixture's .cinders/{kind}");
    }

    let junk = fresh.join(".cinders/images/junk.avif");
    fs::write(&junk, "junk").unwrap();
    encoded.store.prune().unwrap();
    assert!(!junk.exists(), "pruning deletes what the build did not use");
    assert_eq!(fs::read_dir(fresh.join(".cinders/images")).unwrap().count(), 6);
    assert_eq!(fs::read_dir(fresh.join(".cinders/fonts")).unwrap().count(), 1);
}

#[test]
fn write_replaces_the_output_directory() {
    let site = build(copied());
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
