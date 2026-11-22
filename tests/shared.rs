//! The modules that both stages share: the config, the path rules, the store, and the write
//! of a built site.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, SystemTime};

use cinders::config::{Config, Prefetch};
use cinders::store::{self, Store};
use cinders::{Content, Site, paths};

/// The message of the error that loading `toml` as the config of the site `name` fails with.
fn config_error(name: &str, toml: &str) -> String {
    match Config::load(&common::site(name, &[("cinders.toml", toml)])) {
        Ok(_) => panic!("{name} should fail to load"),
        Err(e) => format!("{e:#}"),
    }
}

/// The names of the files and directories in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap();
    let mut names: Vec<String> =
        entries.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// A built site of `files`, each an output path and its content, with its store in `root`.
fn built(root: &Path, files: Vec<(&str, Content)>) -> Site {
    let files = files.into_iter().map(|(path, content)| (path.to_string(), content));
    Site {
        files: files.collect(),
        prefix: String::new(),
        store: Store::new(root),
        notes: Vec::new(),
    }
}

#[test]
fn config_rejects_what_it_does_not_know() {
    let load =
        |name: &str, toml: &str| Config::load(&common::site(name, &[("cinders.toml", toml)]));
    let defaults = load("c-none", "").unwrap();
    assert_eq!(defaults.images.avif, 59, "the default quality");
    assert_eq!(defaults.images.max_width, 2560, "the default width limit");
    assert!(defaults.navigation.client, "client-side navigation is the default");
    assert_eq!(defaults.navigation.prefetch, Prefetch::Hover, "hover is the default");
    assert_eq!(load("c-one", "[images]\navif = 50\n").unwrap().images.max_width, 2560);
    let all = load(
        "c-all",
        "browsers = [\"chrome 120\", \"safari 17.2.1\"]\nunchecked = [\"/x/\"]\n[site]\nurl = \"https://x.example\"\nnav = [\"a\"]\n[navigation]\nclient = false\nprefetch = \"visible\"\n[fonts]\nscan = [\"a.ts\"]\n[images]\navif = 1\nmax_width = 1600\n",
    );
    let all = all.unwrap();
    assert_eq!(all.browsers.unwrap(), ["chrome 120", "safari 17.2.1"]);
    assert_eq!(all.unchecked, ["/x/"]);
    assert!(!all.navigation.client, "client-side navigation can be disabled");
    assert_eq!(all.navigation.prefetch, Prefetch::Visible);
    assert!(all.site.contains_key("nav"), "[site] keeps the keys an author adds");
    let own = load("c-own", "[site]\nnavigation = \"full\"\nprefetch = \"visible\"\n").unwrap();
    assert!(own.site.contains_key("navigation"), "a key of that name is the author's");
    assert!(own.navigation.client, "and does not change [navigation] settings");
    let one = load("c-one-key", "[navigation]\nprefetch = \"visible\"\n").unwrap();
    assert!(one.navigation.client, "a table with one key keeps the other's default");
    assert_eq!(all.fonts.scan, ["a.ts"]);
    assert_eq!(all.images.avif, 1);
    assert_eq!(all.images.max_width, 1600);

    for (name, toml, expected) in [
        ("c-typo", "[images]\navf = 50\n", "unknown field `avf`"),
        ("c-top", "feed = []\n", "unknown field `feed`"),
        ("c-feeds", "feeds = [\"blog\"]\n", "unknown field `feeds`"),
        ("c-search", "search = [\"docs\"]\n", "unknown field `search`"),
        ("c-webp", "[images]\nwebp = 90\n", "unknown field `webp`"),
        ("c-range", "[images]\navif = 101\n", "from 0 to 100"),
        ("c-width", "[images]\nmax_width = 0\n", "max_width is at least 1"),
        ("c-prefix", "unchecked = [\"x\"]\n", "starts with `/`"),
        ("c-navigation", "[navigation]\nclient = \"full\"\n", "expected a boolean"),
        ("c-navigation-key", "[navigation]\nmode = \"client\"\n", "unknown field `mode`"),
        ("c-navigation-string", "navigation = \"full\"\n", "cinders.toml"),
        ("c-prefetch", "[navigation]\nprefetch = \"eager\"\n", "expected `hover` or `visible`"),
        ("c-prefetch-top", "prefetch = \"visible\"\n", "unknown field `prefetch`"),
        ("c-query", "browsers = \"last 2 versions\"\n", "expected a sequence"),
        ("c-syntax", "[site\n", "cinders.toml"),
    ] {
        let error = config_error(name, toml);
        assert!(error.contains(expected), "{name}: {error}");
    }
    for (name, url) in [
        ("c-url-bare", "\"example.com/docs\""),
        ("c-url-query", "\"https://x.example/?a\""),
        ("c-url-fragment", "\"https://x.example/#a\""),
        ("c-url-host", "\"https:///docs\""),
        ("c-url-space", "\"https://x .example\""),
        ("c-url-slashes", "\"https://x.example//docs\""),
        ("c-url-kind", "1"),
    ] {
        let error = config_error(name, &format!("[site]\nurl = {url}\n"));
        let expected = "[site] url is an address with no query, as in https://example.com/docs";
        assert!(error.contains(expected), "{name}: {error}");
    }
}

#[test]
fn config_reads_the_prefix_from_the_address() {
    for (name, url, base_url, prefix) in [
        ("p-root", "https://x.example", "https://x.example", ""),
        ("p-slash", "https://x.example/", "https://x.example", ""),
        ("p-path", "http://x.example/docs", "http://x.example/docs", "/docs"),
        ("p-deep", "https://x.example/a/b/", "https://x.example/a/b", "/a/b"),
    ] {
        let toml = format!("[site]\nurl = \"{url}\"\n");
        let config = Config::load(&common::site(name, &[("cinders.toml", toml.as_str())])).unwrap();
        assert_eq!(config.base_url(), Some(base_url), "{url}");
        assert_eq!(config.prefix(), prefix, "{url}");
    }
    let config = Config::load(&common::site("p-none", &[("pages/index.html", "x")])).unwrap();
    assert_eq!(config.base_url(), None, "a site with no cinders.toml has no address");
    assert_eq!(config.prefix(), "");
}

#[test]
fn paths_resolve_without_the_file_system() {
    let root = Path::new("/site");
    assert_eq!(paths::name(root, Path::new("/site/pages/a.html")), "pages/a.html");
    assert_eq!(paths::name(root, Path::new("/other/a.html")), "/other/a.html");
    for (path, expected) in [
        ("/site/pages/../public/./a.png", "/site/public/a.png"),
        ("/site/pages/a/b/../../c.css", "/site/pages/c.css"),
        ("a/../../b", "b"),
        ("./a", "a"),
    ] {
        assert_eq!(paths::normalize(Path::new(path)), PathBuf::from(expected), "{path}");
    }
}

#[test]
fn references_split_into_their_parts() {
    for (value, expected) in [
        ("https://x.example/a", true),
        ("mailto:a@x.example", true),
        ("data:image/png;base64,AAAA", true),
        ("view-source:a", true),
        ("a/b:c", false),
        (":a", false),
        ("about.html", false),
        ("//x.example/a", false),
    ] {
        assert_eq!(paths::has_scheme(value), expected, "{value}");
    }
    for (name, expected) in [
        ("og:image", true),
        ("OG:Image", true),
        ("twitter:image", true),
        ("og:image:secure_url", false),
        ("og:title", false),
    ] {
        assert_eq!(paths::is_preview_image(name), expected, "{name}");
    }
    for (value, expected) in [
        ("a.png", ("a.png", "")),
        ("a.png?v=1#top", ("a.png", "?v=1#top")),
        ("a.png#top?v=1", ("a.png", "#top?v=1")),
        ("#top", ("", "#top")),
        ("it&#39;s.svg", ("it&#39;s.svg", "")),
        ("it&#x27;s.svg#top", ("it&#x27;s.svg", "#top")),
        ("a&#b", ("a&", "#b")),
        ("a&?b", ("a&", "?b")),
    ] {
        let reference = paths::split(value);
        assert_eq!((reference.path, reference.suffix), expected, "{value}");
    }
    for (value, expected) in [
        ("a.png 1x, b.png 2x", vec![("a.png", "1x"), ("b.png", "2x")]),
        ("a,b.png 2x", vec![("a,b.png", "2x")]),
        ("a.png, b.png", vec![("a.png", ""), ("b.png", "")]),
        (" \n a.png  480w ,b.png", vec![("a.png", "480w"), ("b.png", "")]),
        ("a.png 480w 2x", vec![("a.png", "480w 2x")]),
        (" , ", vec![]),
        ("", vec![]),
    ] {
        let candidates = paths::srcset(value);
        let candidates: Vec<(&str, &str)> =
            candidates.iter().map(|candidate| (candidate.url, candidate.descriptor)).collect();
        assert_eq!(candidates, expected, "{value:?}");
    }
}

#[test]
fn url_paths_encode_and_decode() {
    for (path, encoded) in [
        ("a-b_c.d~e/f0", "a-b_c.d~e/f0"),
        ("my post/é.html", "my%20post/%C3%A9.html"),
        ("a,b(c).png", "a%2Cb%28c%29.png"),
        ("100%.svg", "100%25.svg"),
        ("it's?#.svg", "it%27s%3F%23.svg"),
    ] {
        assert_eq!(paths::percent_encode(path), encoded, "{path}");
        assert_eq!(paths::percent_decode(encoded), path, "{encoded}");
    }
    for (text, decoded) in [
        ("%2f%2F", "//"),
        ("100%", "100%"),
        ("a%2", "a%2"),
        ("%zz%", "%zz%"),
        ("%E2%82%AC", "€"),
        ("%FF", "\u{FFFD}"),
        ("a+b", "a+b"),
        ("a%+Ab.png", "a%+Ab.png"),
        ("%-1%1+", "%-1%1+"),
    ] {
        assert_eq!(paths::percent_decode(text), decoded, "{text}");
    }
}

#[test]
fn entries_of_a_missing_directory_are_none() {
    let root = common::site("entries", &[("pages/a.html", "a"), ("pages/b/c.html", "c")]);
    let entries = paths::entries(&root.join("pages")).unwrap();
    let mut found: Vec<_> = entries.iter().map(|entry| entry.file_name()).collect();
    found.sort();
    assert_eq!(found, ["a.html", "b"]);
    assert!(paths::entries(&root.join("components")).unwrap().is_empty());
    let error = paths::entries(&root.join("pages/a.html")).unwrap_err();
    assert!(format!("{error:#}").contains("pages/a.html"), "the error names the path: {error:#}");
}

#[test]
fn store_keys_cover_every_part() {
    let key = store::key(&[b"ab", b"c"]);
    assert_eq!(key.len(), 32);
    assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()), "{key}");
    assert_eq!(key, store::key(&[b"ab", b"c"]), "a key depends on its parts alone");
    assert_ne!(key, store::key(&[b"a", b"bc"]), "two splits of the same bytes differ");
    assert_ne!(key, store::key(&[b"abc"]));
    assert_ne!(key, store::key(&[b"ab", b"d"]));
}

#[test]
fn store_keeps_what_a_build_used() {
    let files = [
        (".cinders/images/read.avif", "read"),
        (".cinders/images/stale.avif", "stale"),
        (".cinders/images/stale.size", "1 1"),
        (".cinders/fonts/stale.woff2", "stale"),
        (".cinders/notes.txt", "not an entry"),
    ];
    let root = common::site("store", &files);
    let store = Store::new(&root);
    assert_eq!(store.read("images/missing.avif"), None);
    assert_eq!(store.read("images/read.avif").unwrap(), b"read");
    store.write("fonts/new.woff2", b"new").unwrap();
    store.write("images/read.avif", b"again").unwrap();
    assert_eq!(store.read("fonts/new.woff2").unwrap(), b"new");
    assert_eq!(store.read("images/read.avif").unwrap(), b"again", "a write replaces an entry");

    store.prune().unwrap();
    assert_eq!(names(&root.join(".cinders/images")), ["read.avif"]);
    assert_eq!(names(&root.join(".cinders/fonts")), ["new.woff2"], "a write leaves no partial");
    assert!(root.join(".cinders/notes.txt").exists(), "pruning deletes entries alone");

    Store::new(&root).prune().unwrap();
    for kind in ["images", "fonts"] {
        let left = names(&root.join(".cinders").join(kind));
        assert!(left.is_empty(), "a build that uses nothing deletes every entry: {left:?}");
    }
}

#[test]
fn store_is_made_by_its_first_write() {
    let root = common::site("store-new", &[("pages/index.html", "x")]);
    let store = Store::new(&root);
    assert_eq!(store.read("fonts/a.woff2"), None);
    store.prune().unwrap();
    assert!(!root.join(".cinders").exists(), "reading and pruning make no store");
    store.write("fonts/a.woff2", b"a").unwrap();
    assert_eq!(fs::read(root.join(".cinders/fonts/a.woff2")).unwrap(), b"a");
}

/// Two builds of one site share its store, as `cinders build` and `cinders dev --optimized` do.
#[test]
fn pruning_leaves_what_another_build_is_writing() {
    let root = common::site("shared-store", &[(".cinders/fonts/used.woff2", "used")]);
    let fonts = root.join(".cinders/fonts");
    let builds = [Store::new(&root), Store::new(&root)];
    for store in &builds {
        assert!(store.read("fonts/used.woff2").is_some());
    }

    // An entry is written aside and renamed, so pruning it would fail the build that writes it.
    let partial = fonts.join("0123.456-7.part");
    fs::write(&partial, "half an entry").unwrap();
    builds[0].prune().unwrap();
    assert!(partial.exists(), "a partial entry stays while its build may be alive");
    let long_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    fs::File::options().write(true).open(&partial).unwrap().set_modified(long_ago).unwrap();
    builds[0].prune().unwrap();
    assert!(!partial.exists(), "a build that died left it");

    // Both builds delete the same stale entries, and neither fails on one the other took.
    for i in 0..4000 {
        fs::write(fonts.join(format!("stale-{i}.woff2")), "").unwrap();
    }
    let start = Barrier::new(2);
    thread::scope(|threads| {
        for store in &builds {
            let start = &start;
            threads.spawn(move || {
                start.wait();
                store.prune().unwrap_or_else(|e| panic!("{e:#}"));
            });
        }
    });
    assert_eq!(names(&fonts), ["used.woff2"]);
}

#[test]
fn write_replaces_the_output_directory() {
    let root = common::site("written", &[("public/logo.svg", "<svg/>")]);
    let files = vec![
        ("index.html", Content::Bytes(b"home".to_vec())),
        ("about/index.html", Content::Bytes(b"about".to_vec())),
        ("robots.txt", Content::Bytes(b"robots".to_vec())),
        ("assets/deep/logo.svg", Content::File(root.join("public/logo.svg"))),
    ];
    let site = built(&root, files);
    let dist = root.join("dist");
    // What an earlier build left: stale directories, a longer file, a file where a directory
    // goes, and a directory where a file goes.
    fs::create_dir_all(dist.join("old/deep")).unwrap();
    fs::write(dist.join("old/deep/stale.html"), "stale").unwrap();
    fs::write(dist.join("index.html"), "x".repeat(100_000)).unwrap();
    fs::write(dist.join("about"), "a file").unwrap();
    fs::create_dir_all(dist.join("robots.txt/inner")).unwrap();
    site.write(&dist).unwrap();
    assert_eq!(names(&dist), ["about", "assets", "index.html", "robots.txt"]);
    for (path, content) in &site.files {
        assert_eq!(fs::read(dist.join(path)).unwrap(), *content.bytes().unwrap(), "{path}");
    }

    site.write(&dist).unwrap();
    let again = names(&dist);
    assert_eq!(again, ["about", "assets", "index.html", "robots.txt"], "a second write keeps all");
    assert_eq!(fs::read(dist.join("index.html")).unwrap(), b"home", "and changes no file");
}

#[test]
fn write_replaces_read_only_files() {
    let root = common::site("read-only", &[("public/x.txt", "x")]);
    let source = root.join("public/x.txt");
    fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
    let site = built(&root, vec![("x.txt", Content::File(source))]);
    for _ in 0..2 {
        site.write(&root.join("dist")).unwrap();
    }
    assert_eq!(fs::read(root.join("dist/x.txt")).unwrap(), b"x");
}

#[test]
fn write_names_the_file_it_cannot_write() {
    let root = common::site("unwritten", &[("pages/index.html", "x")]);
    let site = built(&root, vec![("gone.mp4", Content::File(root.join("public/gone.mp4")))]);
    let error = site.write(&root.join("dist")).unwrap_err();
    assert!(format!("{error:#}").contains("dist/gone.mp4"), "{error:#}");
}
