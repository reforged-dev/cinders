//! Tests for client-side navigation script generation, injection, and configuration.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::path::Path;

use cinders::Site;
use common::{assets, build, build_fast, error, site, text};

const HOME: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
                    <title>Home</title>\n</head>\n<body><p>Home</p></body>\n</html>";

/// Returns the output path of the navigation script referenced by `path`.
fn script<'a>(site: &'a Site, path: &str) -> &'a str {
    let html = text(site, path);
    let name = html.split("/assets/navigation").nth(1).unwrap_or_else(|| panic!("{html}"));
    let name = format!("assets/navigation{}", name.split(['>', '"']).next().unwrap());
    let (path, _) = site.files.get_key_value(&name).unwrap_or_else(|| panic!("no {name}"));
    path
}

#[test]
fn every_page_of_a_site_with_two_routes_loads_the_script() {
    let files = [
        ("pages/index.html", HOME),
        ("pages/about.html", "<!doctype html><title>About</title><p>About</p>"),
        ("pages/docs/setup.html", "<p>Setup</p>"),
        (
            "pages/late.html",
            "\n<!-- First. -->\n<!-- Second. -->\n<!DOCTYPE html><title>Late</title>",
        ),
        ("pages/404.html", "<p>Not found</p>"),
    ];
    let root = site("navigation", &files);
    let built = build(&root);
    let name = script(&built, "index.html");
    let tag = format!("<script type=module src=/{name}></script>");
    let home = text(&built, "index.html");
    let first = format!("<!doctype html><html lang=en>{tag}<meta charset=utf-8>");
    assert!(home.starts_with(&first), "the tag opens the head: {home}");
    let about = text(&built, "about/index.html");
    let after = format!("<!doctype html>{tag}<title>About</title>");
    assert!(about.starts_with(&after), "without a <head> tag, it follows the doctype: {about}");
    let setup = text(&built, "docs/setup/index.html");
    assert_eq!(setup, format!("{tag}<p>Setup</p>"), "without <head> or doctype, it comes first");
    let late = text(&built, "late/index.html");
    let after = format!("<!DOCTYPE html>{tag}<title>Late</title>");
    assert_eq!(late, after, "a doctype preceded by a comment still comes first");
    assert_eq!(text(&built, "404.html"), format!("{tag}<p>Not found</p>"), "the 404 page too");
    assert_eq!(assets(&built, "js"), [name], "every page names the one file");
    let code = text(&built, name);
    assert_eq!(code.lines().count(), 1, "it ships minified: {code}");
    assert!(
        !code.contains("[cinders]") && !code.contains("full load"),
        "without dev log messages: {code}"
    );
    assert!(
        !root.join(".cinders").exists(),
        "the navigation script does not create a .cinders store"
    );

    let fast = build_fast(&root);
    let tag = r#"<script type="module" src="/assets/navigation.js"></script>"#;
    let home = text(&fast, "index.html");
    assert!(home.contains(&format!("<head>{tag}\n<meta charset")), "{home}");
    assert!(text(&fast, "404.html").starts_with(tag), "the dev form is on every page too");
    let late = text(&fast, "late/index.html");
    let after =
        format!("\n<!-- First. -->\n<!-- Second. -->\n<!DOCTYPE html>{tag}<title>Late</title>");
    assert_eq!(late, after, "and follows a doctype preceded by comments");
    let code = text(&fast, "assets/navigation.js");
    let head = "const DEV = true;\nconst VISIBLE = false;\n// Client-side navigation";
    assert!(code.starts_with(head), "served as written");
    assert!(code.contains("[cinders] full load: "), "and says why a load was full: {code}");
}

#[test]
fn a_site_with_one_route_or_without_client_navigation_does_not_ship_a_script() {
    let one = [("pages/index.html", HOME), ("pages/404.html", "<a href=\"/\">Home</a>")];
    let full = [
        ("cinders.toml", "[navigation]\nclient = false\n"),
        ("pages/index.html", HOME),
        ("pages/about.html", "<p>About</p>"),
    ];
    for (name, files) in [("navigation-one", &one[..]), ("navigation-full", &full[..])] {
        let root = site(name, files);
        for site in [build(&root), build_fast(&root)] {
            let pages = site.files.keys().filter(|path| path.ends_with(".html"));
            for page in pages {
                let html = text(&site, page);
                assert!(!html.contains("<script"), "{name}: {page} should not have a tag: {html}");
            }
            let scripts = site.files.keys().filter(|path| path.ends_with(".js"));
            assert_eq!(scripts.count(), 0, "{name}: should not ship a script");
        }
    }
    assert_eq!(text(&build(&site("navigation-one", &one)), "404.html"), "<a href=/>Home</a>");

    let client = [("cinders.toml", "[navigation]\nclient = true\n"), full[1], full[2]];
    assert_eq!(assets(&build(&site("navigation-client", &client)), "js").len(), 1, "the default");
    let other = [("cinders.toml", "[navigation]\nclient = \"full\"\n"), full[1], full[2]];
    let other = error(&site("navigation-other", &other));
    assert!(other.contains("invalid type: string \"full\", expected a boolean"), "{other}");
}

#[test]
fn the_prefetch_key_adds_the_fetch_for_a_link_in_view() {
    let with = |name: &str, config: &str| {
        let pages = [("pages/index.html", HOME), ("pages/about.html", "<p>About</p>")];
        site(name, &[("cinders.toml", config), pages[0], pages[1]])
    };
    let shipped = |root: &Path| {
        let built = build(root);
        text(&built, script(&built, "index.html"))
    };
    let served = |root: &Path| text(&build_fast(root), "assets/navigation.js");
    let hover = with("prefetch-hover", "[navigation]\nprefetch = \"hover\"\n");
    let code = shipped(&hover);
    assert_eq!(code, shipped(&with("prefetch-default", "")), "hover is the default");
    assert!(
        !code.contains("IntersectionObserver"),
        "hover mode does not ship viewport-prefetch code: {code}"
    );
    let code = served(&hover);
    assert!(code.contains("\nconst VISIBLE = false;\n"), "{code}");
    let visible = with("prefetch-visible", "[navigation]\nprefetch = \"visible\"\n");
    let code = shipped(&visible);
    assert!(code.contains("IntersectionObserver"), "it watches each link: {code}");
    let code = served(&visible);
    assert!(code.starts_with("const DEV = true;\nconst VISIBLE = true;\n"), "in dev too: {code}");

    let full =
        build(&with("prefetch-full", "[navigation]\nclient = false\nprefetch = \"visible\"\n"));
    assert_eq!(
        assets(&full, "js").len(),
        0,
        "client = false does not ship a script even with prefetch = visible"
    );
}

#[test]
fn the_scripts_name_follows_the_scripts_of_the_site() {
    // Builds a variant of the site and returns its navigation script path.
    let name = |form: &str, head: &str, module: &str, classic: &str, styles: &str| {
        let home = format!(
            "<!doctype html><html><head>{head}<link rel=\"stylesheet\" href=\"../site.css\">\
             <script type=\"module\" src=\"../app.ts\" data-client-nav></script>\
             <script src=\"../classic.js\" data-client-nav></script></head><body>home</body></html>"
        );
        let files = [
            ("pages/index.html", home.as_str()),
            ("pages/about.html", "<p>about</p>"),
            ("app.ts", module),
            ("classic.js", classic),
            ("site.css", styles),
        ];
        let built = build(&site(&format!("navigation-hash-{form}"), &files));
        script(&built, "index.html").to_string()
    };
    let inline = "<script data-client-nav>console.log(1)</script>";
    let elsewhere = "<script src=\"https://example.com/a.js\" data-client-nav></script>";
    let data = "<script type=\"application/ld+json\">{\"a\": 1}</script>";
    let first = name("first", inline, "console.log('a');", "var a = 1;", "p { margin: 0 }");
    for (form, head, module, classic, styles) in [
        ("styles", inline, "console.log('a');", "var a = 1;", "p { margin: 1px }"),
        ("comment", inline, "console.log('a'); // a", "var a = 1;", "p { margin: 0 }"),
        ("data", &format!("{inline}{data}")[..], "console.log('a');", "var a = 1;", "p {}"),
    ] {
        let same = name(form, head, module, classic, styles);
        assert_eq!(same, first, "{form}: the scripts ship the same, so the file is the same");
    }
    let mut names = vec![first];
    for (form, head, module, classic) in [
        ("module", inline, "console.log('b');", "var a = 1;"),
        ("classic", inline, "console.log('a');", "var a = 2;"),
        (
            "inline",
            "<script data-client-nav>console.log(2)</script>",
            "console.log('a');",
            "var a = 1;",
        ),
        ("none", "", "console.log('a');", "var a = 1;"),
        ("elsewhere", elsewhere, "console.log('a');", "var a = 1;"),
    ] {
        let changed = name(form, head, module, classic, "p { margin: 0 }");
        assert!(!names.contains(&changed), "{form}: a changed script changes the file's name");
        names.push(changed);
    }

    // Scripts in `public/` keep a fixed URL, so content changes must still change the hash.
    let public = |form: &str, code: &str| {
        let files = [
            ("pages/index.html", "<script src=\" /lib/widget.js?v=1\" data-client-nav></script>"),
            ("pages/about.html", "<p>about</p>"),
            ("public/lib/widget.js", code),
        ];
        let built = build(&site(&format!("navigation-public-{form}"), &files));
        script(&built, "index.html").to_string()
    };
    let widget = public("first", "console.log('one');");
    assert_eq!(public("same", "console.log('one');"), widget, "the same script in public/");
    assert_ne!(
        public("changed", "console.log('two');"),
        widget,
        "changing a public/ script's content changes the hash"
    );
    let pair = |form: &str, first: &str, second: &str| {
        let home = "<script src=\"/a.js\" data-client-nav></script>\
                    <script src=\"/b.js\" data-client-nav></script>";
        let files = [
            ("pages/index.html", home),
            ("pages/about.html", "<p>about</p>"),
            ("public/a.js", first),
            ("public/b.js", second),
        ];
        let built = build(&site(&format!("navigation-pair-{form}"), &files));
        script(&built, "index.html").to_string()
    };
    let swapped = pair("swapped", "var b;", "var a;");
    assert_ne!(
        pair("first", "var a;", "var b;"),
        swapped,
        "swapping contents between two public/ scripts changes the hash"
    );

    // A script referenced only on another page still affects every page's navigation script hash.
    let elsewhere = |form: &str, code: &str| {
        let about = "<script type=\"module\" src=\"../about.ts\" data-client-nav></script>";
        let files =
            [("pages/index.html", "<p>home</p>"), ("pages/about.html", about), ("about.ts", code)];
        let built = build(&site(&format!("navigation-elsewhere-{form}"), &files));
        script(&built, "index.html").to_string()
    };
    let changed = elsewhere("changed", "console.log('two');");
    assert_ne!(
        elsewhere("first", "console.log('one');"),
        changed,
        "changing another page's script changes the hash on index.html"
    );

    let plain = |name: &str, text: &str| {
        let files = [("pages/index.html", text), ("pages/about.html", "<p>about</p>")];
        let built = build(&site(name, &files));
        script(&built, "index.html").to_string()
    };
    let none = plain("navigation-hash-plain", "<p>one</p>");
    assert_eq!(
        plain("navigation-hash-other", "<p>two</p>"),
        none,
        "sites without scripts share a hash"
    );
    assert!(!names.contains(&none), "{none}");
}

#[test]
fn the_script_takes_the_prefix_and_does_not_take_a_site_address() {
    let files = [
        ("cinders.toml", "[site]\nurl = \"https://example.com/base\"\n"),
        (
            "pages/index.html",
            "<script type=\"module\" src=\"../navigation.js\" data-client-nav></script>",
        ),
        ("pages/about.html", "<p>about</p>"),
        ("navigation.js", "console.log('the site has a script of this name');"),
    ];
    let root = site("navigation-prefix", &files);
    let built = build(&root);
    let name = script(&built, "about/index.html");
    let tag = format!("<script type=module src=/base/{name}></script>");
    assert_eq!(text(&built, "about/index.html"), format!("{tag}<p>about</p>"));
    let scripts = assets(&built, "js");
    assert_eq!(scripts.len(), 2, "the site's script keeps its own file: {scripts:?}");
    assert!(text(&built, "index.html").starts_with(&tag), "the tag comes before the page's own");

    let fast = build_fast(&root);
    let tag = r#"<script type="module" src="/base/assets/navigation.js"></script>"#;
    assert_eq!(text(&fast, "about/index.html"), format!("{tag}<p>about</p>"));
    let own = text(&fast, "_cinders/navigation.js");
    assert!(own.contains("the site has a script of this name"), "{own}");
    assert!(text(&fast, "assets/navigation.js").starts_with("const DEV = true;"));
}

#[test]
fn build_notes_unmarked_executable_scripts_only_when_client_navigation_is_active() {
    let unmarked = [
        (
            "pages/index.html",
            r#"<script type="module" src="../scripts/app.ts"></script><script>console.log(1)</script><script type="application/ld+json">{}</script>"#,
        ),
        ("pages/about.html", r#"<script type="module" src="../scripts/app.ts"></script>"#),
        ("scripts/app.ts", "console.log('a');"),
    ];
    let root = site("navigation-note-unmarked", &unmarked);
    for built in [build(&root), build_fast(&root)] {
        assert_eq!(built.notes.len(), 1, "{:?}", built.notes);
        let note = &built.notes[0];
        assert!(note.contains("scripts/app.ts"), "{note}");
        assert!(note.contains("inline script in pages/index.html"), "{note}");
        assert!(note.contains("data-client-nav"), "{note}");
    }

    let marked = [
        (
            "pages/index.html",
            r#"<script type="module" src="../scripts/app.ts" data-client-nav></script><script nomodule src="../scripts/legacy.js"></script><script src=""></script><script type="application/ld+json">{}</script>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("scripts/app.ts", "console.log('a');"),
        ("scripts/legacy.js", "var legacy = 1;"),
    ];
    let built = build_fast(&site("navigation-note-marked", &marked));
    assert!(
        built.notes.is_empty(),
        "marked, nomodule, empty-src, and JSON scripts do not emit notes: {:?}",
        built.notes
    );
    assert!(
        text(&built, "index.html").contains("scripts/legacy.js"),
        "nomodule scripts still ship for older browsers"
    );

    let mime = [
        (
            "pages/index.html",
            r#"<script type="application/javascript" src="../scripts/app.js"></script><script type=" module " src="../scripts/mod.ts"></script>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("scripts/app.js", "var a = 1;"),
        ("scripts/mod.ts", "const x: number = 1; console.log(x);"),
    ];
    let mime_built = build(&site("navigation-note-mime", &mime));
    assert_eq!(
        mime_built.notes.len(),
        1,
        "MIME and padded-type scripts are noticed: {:?}",
        mime_built.notes
    );
    assert!(
        mime_built.notes[0].contains("scripts/app.js")
            && mime_built.notes[0].contains("scripts/mod.ts"),
        "{}",
        mime_built.notes[0]
    );
    assert_eq!(
        assets(&mime_built, "js").len(),
        3,
        "both scripts are processed alongside the navigation script"
    );

    let many = [
        (
            "pages/index.html",
            r#"<script src="https://example.com/1.js"></script><script src="/lib.js"></script><script src="../a.js"></script><script src="../b.js"></script>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("public/lib.js", "var l = 1;"),
        ("a.js", "var a = 1;"),
        ("b.js", "var b = 1;"),
    ];
    let many_built = build_fast(&site("navigation-note-many", &many));
    assert_eq!(many_built.notes.len(), 1, "{:?}", many_built.notes);
    assert!(
        many_built.notes[0].contains("and 1 more"),
        "more than three unmarked scripts are summarized: {}",
        many_built.notes[0]
    );

    let off =
        [("cinders.toml", "[navigation]\nclient = false\n"), unmarked[0], unmarked[1], unmarked[2]];
    let off_built = build_fast(&site("navigation-note-off", &off));
    assert!(
        off_built.notes.is_empty(),
        "client = false does not emit a note: {:?}",
        off_built.notes
    );

    let one = [unmarked[0], unmarked[2]];
    let one_built = build_fast(&site("navigation-note-one", &one));
    assert!(one_built.notes.is_empty(), "one route does not emit a note: {:?}", one_built.notes);
}
