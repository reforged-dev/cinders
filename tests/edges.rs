//! Edge cases of the optimizer, each on a small site in a temporary directory.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;

use cinders::{Content, Mode, Site};
use common::{asset, assets, build, build_fast, error, fixture, site, text};

const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg'/>";

#[test]
fn references_in_styles_scripts_and_public() {
    let page = concat!(
        r#"<link rel="stylesheet" href="../site.css">"#,
        r#"<div style="background: url('bg.svg')"></div>"#,
        r#"<a href="/docs/">docs</a><a href="/docs">docs</a><a href="?page=2">next</a>"#,
        r#"<a href="https://example.com/x.png">out</a><a href="mailto:a@b.c">mail</a>"#,
        r#"<a href="//cdn.example/x.js">host</a>"#,
        r#"<script type="module">const r = await fetch("/"); console.log(r)</script>"#,
        "<a href=#x>a</a> <a href=#y>b</a>",
    );
    let files = [
        ("pages/index.html", page),
        ("pages/bg.svg", SVG),
        ("site.css", "@import url(\"https://fonts.example/a.css\");\nbody { margin: 0 }\n"),
        ("public/docs/index.html", "docs"),
    ];
    let site = build(&site("refs", &files));
    let html = text(&site, "index.html");
    assert!(html.contains("url(/assets/bg-"), "{html}");
    assert!(html.contains("href=/docs/>") && html.contains("href=/docs>"), "{html}");
    assert!(html.contains(r#"href="?page=2""#) && html.contains("href=#x"), "{html}");
    assert!(html.contains("href=https://example.com/x.png") && html.contains("mailto:a@b.c"));
    assert!(html.contains("href=//cdn.example/x.js>"), "{html}");
    assert!(html.contains("await fetch"), "an inline module may use top-level await: {html}");
    assert!(html.contains("</a> <a"), "the space between inline elements stays: {html}");
    let css = asset(&site, "css");
    assert!(css.contains("@import \"https://fonts.example/a.css\""), "{css}");
}

#[test]
fn encoded_and_unusual_file_names() {
    let page =
        r#"<img src="my%20shot.svg"><img src="caf&eacute;.svg"><a href="caf%C3%A9.svg">x</a>"#;
    let files = [("pages/index.html", page), ("pages/my shot.svg", SVG), ("pages/café.svg", SVG)];
    let root = site("names", &files);
    let html = text(&build(&root), "index.html");
    assert!(html.contains("src=/assets/my-shot-") && html.contains("src=/assets/caf--"), "{html}");
    assert!(html.contains("href=/assets/caf--"), "{html}");
    let fast = text(&build_fast(&root), "index.html");
    assert!(fast.contains(r#"src="/_cinders/pages/my%20shot.svg""#), "{fast}");
    assert!(fast.contains(r#"src="/_cinders/pages/caf%C3%A9.svg""#), "{fast}");
}

#[test]
fn route_urls_are_percent_encoded() {
    let files = [
        ("cinders.toml", "[navigation]\nclient = false\n[site]\nurl = \"https://example.com\"\n"),
        ("pages/index.html", r#"<a href="my%20post.html">a</a><a href="c%23.html">b</a>"#),
        ("pages/my post.html", r#"<a href="/c%23/">c</a><a href="/my post/">self</a>"#),
        ("pages/c#.html", r#"{page.url}<a href="/sitemap.xml">map</a>"#),
    ];
    let site = build(&site("route-names", &files));
    let html = text(&site, "index.html");
    assert!(html.contains("href=/my%20post/") && html.contains("href=/c%23/"), "{html}");
    let post = text(&site, "my post/index.html");
    assert!(post.contains("href=/c%23/>") && post.contains(r#"href="/my post/""#), "{post}");
    let sharp = text(&site, "c#/index.html");
    assert!(sharp.starts_with("/c%23/<a href=/sitemap.xml>"), "{sharp}");
    let sitemap = text(&site, "sitemap.xml");
    assert!(sitemap.contains("<loc>https://example.com/my%20post/</loc>"), "{sitemap}");
}

#[test]
fn srcset_urls_may_hold_commas() {
    let page = concat!(
        r#"<img src="a.svg" srcset="https://img.example/w_1,h_1/x.jpg 1x, a.svg 2x">"#,
        r#"<img src="a.svg" srcset="a.svg, data:image/gif;base64,R0lGOD 2x,a.svg 3x">"#,
    );
    let hero =
        r#"<img src="a.svg" srcset="a.svg 2x" style="mask: url(a.svg)"><Icon src="a.svg#b" />"#;
    let files = [
        ("pages/index.html", &format!("{page}<Hero />")[..]),
        ("pages/a.svg", SVG),
        ("components/my hero/Hero.html", hero),
        ("components/my hero/a.svg", SVG),
        ("components/Icon.html", "<use href={src} />"),
    ];
    let root = site("srcset", &files);
    let html = text(&build(&root), "index.html");
    assert!(
        html.contains(r#"srcset="/assets/a-"#) && html.contains("mask: url(/assets/a-"),
        "a component's directory may hold a space: {html}"
    );
    assert!(html.contains(".svg#b>"), "a prop that names a file may hold a fragment: {html}");
    assert!(html.contains("https://img.example/w_1,h_1/x.jpg 1x, /assets/a-"), "{html}");
    assert!(html.contains(".svg, data:image/gif;base64,R0lGOD 2x, /assets/a-"), "{html}");
}

#[test]
fn character_references_in_paths() {
    let page = r#"<img src="it&#39;s.svg"><a href="it&#x27;s.svg#top">x</a>"#;
    let root = site("apostrophe", &[("pages/index.html", page), ("pages/it's.svg", SVG)]);
    let html = text(&build(&root), "index.html");
    assert!(html.contains("src=/assets/it-s-"), "{html}");
    assert!(
        html.contains(".svg#top>x</a>") && !html.contains("&#"),
        "the `#` of a character reference starts no fragment: {html}"
    );
}

#[test]
fn css_strings_and_rooted_imports() {
    let data = r#"url('data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg"/>')"#;
    let css = format!("@import \"/theme.css\";\n.x {{ background: {data} }}\n");
    let page =
        format!(r#"<link rel="stylesheet" href="../s.css"><style>.y{{mask:{data}}}</style>"#);
    let files = [("pages/index.html", page.as_str()), ("s.css", &css), ("public/theme.css", "")];
    let root = site("css-strings", &files);
    let site = build(&root);
    let escaped = r#"url("data:image/svg+xml,<svg xmlns=\"http://www.w3.org/2000/svg\"/>")"#;
    let css = asset(&site, "css");
    assert!(css.contains(escaped) && css.contains(r#"@import "/theme.css""#), "{css}");
    assert!(text(&site, "index.html").contains(escaped));
    fs::remove_file(root.join("public/theme.css")).unwrap();
    let error = error(&root);
    assert!(error.contains("`/theme.css` is not a route or a file in public/"), "{error}");

    let files = [("pages/index.html", r#"<style>.x{mask:url("data:,a\a b")}.z{top:0}</style>"#)];
    let html = text(&build(&common::site("css-break", &files)), "index.html");
    assert!(
        html.contains(r#"url("data:,a\a b")}.z{top:0}"#),
        "a line break, written as an escape, is written back as one: {html}"
    );
    let files =
        [("pages/index.html", r#"<style>.x::before{content:"</style\3e "}.z{top:0}</style>x"#)];
    let html = text(&build(&common::site("css-end", &files)), "index.html");
    assert!(html.ends_with(".z{top:0}</style>x"), "the rule after the string is kept: {html}");
    assert_eq!(
        html.matches("</style").count(),
        1,
        "`</style` in the string is not an end tag: {html}"
    );
}

#[test]
fn plain_links_leave_bundles_and_fonts_alone() {
    let page = concat!(
        r#"<link rel="stylesheet" href="../a.css"><a href="../a.css">source</a>"#,
        r#"<a href="../m.js">source</a><script type="module" src="../m.js"></script>"#,
        r#"<a href="../f.ttf" download>font</a>"#,
    );
    let files = [
        ("pages/index.html", page),
        ("a.css", "@import \"b.css?v=1\";"),
        ("b.css", "p{top:0}"),
        ("m.js", "import { n } from './n.js';\nconsole.log(n);\n"),
        ("n.js", "export const n = 1;\n"),
    ];
    let root = site("plain-links", &files);
    fs::copy(fixture().join("fonts/GeistMono.ttf"), root.join("f.ttf")).unwrap();
    let fast = build_fast(&root);
    assert!(text(&fast, "_cinders/a.css").contains("top: 0"), "the bundle is served");
    let script = text(&fast, "_cinders/m.js");
    assert!(script.contains("const n = 1;"), "and so is the joined script: {script}");
    let built = build(&root);
    assert_eq!(assets(&built, "ttf").len(), 1, "a linked font ships whole");
    let sheets: Vec<String> = assets(&built, "css").iter().map(|path| text(&built, path)).collect();
    assert_eq!(sheets.len(), 2, "a build ships the bundle and the source: {sheets:?}");
    assert!(sheets.contains(&"p{top:0}".to_string()), "{sheets:?}");
    assert!(sheets.contains(&"@import \"b.css?v=1\";".to_string()), "{sheets:?}");
    let scripts: Vec<String> = assets(&built, "js").iter().map(|path| text(&built, path)).collect();
    assert_eq!(scripts.len(), 2, "and the joined script and its source: {scripts:?}");
    assert!(scripts.contains(&"console.log(1);".to_string()), "{scripts:?}");
}

#[test]
fn image_set_strings_stay_quoted() {
    let rule = r#".x{background-image:image-set("../img/b.svg" 1x)}"#;
    let root = site(
        "image-set",
        &[
            ("cinders.toml", r#"browsers = ["chrome 120", "safari 17", "firefox 120"]"#),
            (
                "pages/index.html",
                &format!(r#"<link rel="stylesheet" href="../styles/s.css"><style>{rule}</style>"#),
            ),
            ("styles/s.css", &format!("{rule}.y{{background:url(\"../img/b.svg\")}}")),
            ("img/b.svg", SVG),
        ],
    );
    let site = build(&root);
    let html = text(&site, "index.html");
    assert!(html.contains(r#"image-set("/assets/b-"#), "{html}");
    let css = asset(&site, "css");
    assert!(css.contains(r#"image-set("/assets/b-"#) && css.contains("url(/assets/b-"), "{css}");
}

#[test]
fn stylesheets_are_lowered_for_the_browsers() {
    let page = r#"<link rel="stylesheet" href="s.css">"#;
    let css = ".a { color: #ff0000; & b { inset: 0 } }\n";
    let files = [("pages/index.html", page), ("pages/s.css", css)];
    let root = site("browsers", &files);
    let default = asset(&build(&root), "css");
    assert_eq!(default, ".a{color:red}.a b{inset:0}", "the default browsers lack nesting");
    fs::write(root.join("cinders.toml"), "browsers = [\"chrome 120\", \"safari 17.2.1\"]\n")
        .unwrap();
    assert_eq!(asset(&build(&root), "css"), ".a{color:red;& b{inset:0}}", "newer ones nest");
    fs::write(root.join("cinders.toml"), "browsers = [\"chrome 80\"]\n").unwrap();
    let old = asset(&build(&root), "css");
    assert_eq!(old, ".a{color:red}.a b{top:0;bottom:0;left:0;right:0}", "an old one has no inset");
    let fast = text(&build_fast(&root), "_cinders/pages/s.css");
    assert_eq!(
        fast, ".a {\n  color: red;\n}\n\n.a b {\n  inset: 0;\n}\n",
        "only nesting is lowered"
    );

    let browsers = |value: &str| {
        fs::write(root.join("cinders.toml"), format!("browsers = {value}\n")).unwrap();
        error(&root)
    };
    let unknown = browsers(r#"["chrome 120", "netscape 4"]"#);
    let expected = "cinders.toml: browsers: `netscape 4` is not a browser cinders knows: chrome, edge, firefox";
    assert!(unknown.contains(expected), "{unknown}");
    for value in ["chrome", "chrome latest", "safari 1.2.3.4", "safari 16.300"] {
        let malformed = browsers(&format!("[\"{value}\"]"));
        let expected = format!("`{value}` needs a version, as in `safari 16.4`");
        assert!(malformed.contains(&expected), "{malformed}");
    }
}

#[test]
fn svg_links_and_entity_quoted_styles() {
    let page = concat!(
        r##"<svg><use xlink:href="../img/icons.svg#x"></use><use href="#local"></use></svg>"##,
        r#"<div style="background:url(&quot;../img/icons.svg&quot;)">x</div>"#,
        r#"<img src="../img/icons.svg" srcset="../img/icons.svg 1x, ../img/icons.svg?v=2 2x">"#,
    );
    let root = site("svg-links", &[("pages/index.html", page), ("img/icons.svg", SVG)]);
    let html = text(&build(&root), "index.html");
    assert!(html.contains("xlink:href=\"/assets/icons-") && html.contains(".svg#x\""), "{html}");
    assert!(html.contains("background:url(/assets/icons-"), "{html}");
    assert!(html.contains(".svg 1x, /assets/icons-") && html.contains(".svg?v=2 2x"), "{html}");

    fs::remove_file(root.join("img/icons.svg")).unwrap();
    let error = error(&root);
    assert!(error.contains("pages/index.html: <use xlink:href>"), "{error}");
    assert!(error.contains("`../img/icons.svg#x` does not exist"), "{error}");
}

#[test]
fn scripts_that_cannot_import() {
    let page = concat!(
        r#"<script src="../legacy.js"></script><button onclick="hello()">x</button>"#,
        "<script>/*! inline 1.0, MIT */\n// a note\nvar inline = 1;</script>",
        r#"<script type="module">console.log(import.meta.url)</script>"#,
    );
    let legacy = "/*! legacy 1.0, MIT */\n// a note\nfunction hello() { alert('hi') }\n\
                  var counter = 0;\n";
    let root = site("classic", &[("pages/index.html", page), ("legacy.js", legacy)]);
    let built = build(&root);
    let js = asset(&built, "js");
    assert!(js.contains("function hello") && js.contains("counter"), "{js}");
    assert!(js.starts_with("/*! legacy 1.0, MIT */") && !js.contains("a note"), "{js}");
    let html = text(&built, "index.html");
    assert!(html.contains("<script>/*! inline 1.0, MIT */"), "license comments stay: {html}");
    assert!(!html.contains("a note"), "other comments are removed: {html}");
    assert!(html.contains("console.log(import.meta.url)"), "import.meta is allowed: {html}");

    let page = r#"<script>import "./x.js"</script>"#;
    let root = site("inline-import", &[("pages/index.html", page), ("pages/x.js", "")]);
    let classic = error(&root);
    let expected = "pages/index.html: <script>: Cannot use import statement outside a module";
    assert!(classic.contains(expected), "{classic}");
    let page = r#"<script type="module">import "./x.js"</script>"#;
    fs::write(root.join("pages/index.html"), page).unwrap();
    let module = error(&root);
    let expected = "pages/index.html: <script>: this script cannot import or export; give it \
                    type=\"module\" and a src";
    assert_eq!(module, expected);

    let page = r#"<script type="module" src="app.js"></script>"#;
    let files = [("pages/index.html", page), ("pages/app.js", "import 'nope'")];
    let package = error(&site("package-missing", &files));
    assert!(package.contains("pages/app.js:1: `nope` is not a relative path"), "{package}");
}

#[test]
fn minifier_keeps_what_renders() {
    let page = concat!(
        "<!doctype html>\n<html>\n<head>\n<title>t</title>\n</head>\n<body class=\"a b\">\n",
        "<svg><text>Click <a href=\"/\">here</a> now</text></svg>\n",
        "<p>\n  one\n  <b>two</b> three\n</p>\n<p title='say \"hi\"' hidden=\"\">x</p>\n",
        "<textarea>  keep  </textarea>\n</body>\n</html>\n",
    );
    let root = site("minify", &[("pages/index.html", page)]);
    let html = text(&build(&root), "index.html");
    assert!(
        html.starts_with("<!doctype html><html><title>t</title><body class=\"a b\">"),
        "{html}"
    );
    assert!(html.contains(r#"Click <a href="/">here</a> now"#), "{html}");
    assert!(html.contains("<p>one\n<b>two</b> three</p>"), "{html}");
    assert!(html.contains("<p title='say \"hi\"' hidden>x</p>"), "{html}");
    assert!(html.ends_with("<textarea>  keep  </textarea>"), "{html}");
    let fast = text(&build_fast(&root), "index.html");
    assert!(fast.contains("<p>\n  one\n  <b>two</b> three\n</p>\n"), "nothing is minified: {fast}");
}

#[test]
fn each_file_ships_once_under_its_fingerprint() {
    let files = [
        ("cinders.toml", "[navigation]\nclient = false\n"),
        ("pages/index.html", r#"<img src="a.svg"><img src="a.svg?v=1"><img src="b/a.svg">"#),
        ("pages/about.html", r#"<img src="a.svg"><img src="c/a.svg"><a href="LICENSE">x</a>"#),
        ("pages/a.svg", SVG),
        ("pages/b/a.svg", SVG),
        ("pages/c/a.svg", "<svg/>"),
        ("pages/LICENSE", "MIT"),
    ];
    let root = site("once", &files);
    let site = build(&root);
    let home = text(&site, "index.html");
    let same = &home["<img src=".len()..home.find('>').unwrap()];
    let shared = format!("<img src={same}><img src=\"{same}?v=1\"><img src={same}>");
    assert_eq!(home, shared, "files of one name and one text share an output");
    assert_eq!(assets(&site, "svg").len(), 2, "another text is another output");
    assert!(matches!(site.files[&same[1..]], Content::File(_)), "a file is read when written");

    let about = text(&site, "about/index.html");
    let license = about.split("<a href=/assets/LICENSE-").nth(1).unwrap();
    assert_eq!(&license[8..], ">x</a>", "a name with no extension ends at its fingerprint");

    let fast = build_fast(&root);
    let paths: Vec<&str> = fast.files.keys().map(String::as_str).collect();
    let sources = [
        "_cinders/pages/LICENSE",
        "_cinders/pages/a.svg",
        "_cinders/pages/b/a.svg",
        "_cinders/pages/c/a.svg",
        "about/index.html",
        "index.html",
    ];
    assert_eq!(paths, sources, "a fast build serves each file at its source path");
    let home = text(&fast, "index.html");
    assert!(home.ends_with(r#"<img src="/_cinders/pages/b/a.svg">"#), "{home}");
}

#[test]
fn fonts_subset_to_the_sites_characters() {
    let page = r#"<link rel="stylesheet" href="s.css"><p>abc</p>"#;
    let css = r#"@font-face { font-family: f; src: url(f.ttf) format("truetype") }"#;
    let files = [("pages/index.html", page), ("pages/s.css", css), ("pages/extra.txt", "XYZ")];
    let root = site("fonts", &files);
    fs::copy(fixture().join("fonts/GeistMono.ttf"), root.join("pages/f.ttf")).unwrap();
    let small = build(&root);
    fs::write(root.join("cinders.toml"), "[fonts]\nscan = [\"pages/extra.txt\"]\n").unwrap();
    let large = build(&root);
    let size = |site| len(site, assets(site, "woff2")[0]);
    assert!(size(&small) < size(&large), "scanned characters join the subset");
    assert!(size(&large) < 4000, "the subset holds a handful of glyphs");
    assert_eq!(fs::read_dir(root.join(".cinders/fonts")).unwrap().count(), 2);
    let css = asset(&large, "css");
    assert!(css.contains(r#".woff2)format("woff2")"#), "the format hint follows: {css}");

    let fast = build_fast(&root);
    assert!(matches!(fast.files["_cinders/pages/f.ttf"], Content::File(_)), "the font is whole");

    fs::write(root.join("cinders.toml"), "[fonts]\nscan = [\"missing.js\"]\n").unwrap();
    assert!(error(&root).contains("cinders.toml: [fonts] scan: missing.js"));
    fs::remove_file(root.join("cinders.toml")).unwrap();
    fs::write(root.join("pages/f.ttf"), "no font").unwrap();
    let broken = error(&root);
    let expected = "pages/index.html: <link href>: pages/s.css: pages/f.ttf: not a TrueType or \
                    OpenType font";
    assert!(broken.contains(expected), "{broken}");
}

#[test]
fn fonts_hold_the_text_that_css_creates() {
    // The path of the subset of a site with this page body and these rules. Two sites that
    // draw the same characters ship the same subset.
    let subset = |name: &str, body: &str, rules: &str, imported: &str| {
        let page = format!(r#"<link rel="stylesheet" href="s.css">{body}"#);
        let css = format!(
            "@import 'more.css';\n@font-face {{ font-family: f; src: url(f.ttf) }}\n{rules}"
        );
        let files = [
            ("pages/index.html", page.as_str()),
            ("pages/s.css", &css),
            ("pages/more.css", imported),
        ];
        let root = site(name, &files);
        fs::copy(fixture().join("fonts/GeistMono.ttf"), root.join("pages/f.ttf")).unwrap();
        assets(&build(&root), "woff2")[0].to_string()
    };
    let cased = subset("font-cased", "<p>Abc</p>", "", "");
    let both = subset("font-both", "<p>ABCabc</p>", "", "");
    assert_eq!(cased, both, "`text-transform` draws the other case of a letter");
    assert_ne!(cased, subset("font-other", "<p>Abcd</p>", "", ""));
    let drawn = "<input placeholder=abc value=AB alt=c>";
    assert_eq!(cased, subset("font-attributes", drawn, "", ""), "attributes draw their text");
    let unseen = "<p>abc</p><script>var d</script><p title=e></p>";
    assert_eq!(cased, subset("font-unseen", unseen, "", ""), "a script and a title draw none");

    let drawn = subset("font-drawn", "<p>abc\u{2192}x!?</p>", "", "");
    let rules = r#"p::before { content: "\2192 x" } /* don't */ li::marker { content: '!\?' }"#;
    let linked = subset("font-linked", "<p>abc</p>", rules, "");
    assert_eq!(drawn, linked, "`content` draws its strings in a linked stylesheet");
    let imported = subset("font-imported", "<p>abc</p>", "", rules);
    assert_eq!(drawn, imported, "and in one that it imports");
    let style = format!("<style>{rules}</style><p>abc</p>");
    assert_eq!(drawn, subset("font-style", &style, "", ""), "and in a `<style>` block");
}

/// The size in bytes of the built file at `path`.
fn len(site: &Site, path: &str) -> usize {
    site.files[path].bytes().unwrap().len()
}

#[test]
fn images_that_are_left_alone() {
    let page = concat!(
        r#"<img src="a.png" srcset="a.png 2x"><picture><img src="a.png"></picture>"#,
        r#"<video poster="a.png"></video><div style="background: url(a.png)"></div>"#,
        r#"<img src="a.gif"><link rel="icon" href="a.png">"#,
    );
    // Not real images, so the build fails if anything tries to decode one.
    let files = [("pages/index.html", page), ("pages/a.png", "fake"), ("pages/a.gif", "fake")];
    let root = site("images-alone", &files);
    let site = build(&root);
    assert!(assets(&site, "avif").is_empty() && assets(&site, "png").len() == 1);
    assert!(!root.join(".cinders").exists(), "a build that encodes nothing writes no store");

    fs::write(root.join("pages/index.html"), r#"<img src="a.png">"#).unwrap();
    let error = error(&root);
    assert!(error.contains("pages/index.html: <img src>: pages/a.png: decoding"), "{error}");
    let fast = build_fast(&root);
    assert!(matches!(fast.files["_cinders/pages/a.png"], Content::File(_)), "nothing decodes it");

    let animated = fixture().join("../animated.png");
    fs::copy(&animated, root.join("pages/a.png")).unwrap();
    let site = build(&root);
    let html = text(&site, "index.html");
    assert!(html.starts_with("<img src=/assets/a-") && html.ends_with(".png>"), "{html}");
    let shipped = site.files[assets(&site, "png")[0]].bytes().unwrap().into_owned();
    assert_eq!(shipped, fs::read(animated).unwrap(), "an animated PNG ships unchanged");
    assert!(!root.join(".cinders").exists(), "and nothing is encoded");
}

#[test]
fn images_wider_than_the_limit_are_scaled_down() {
    let page = r#"<img src="shot.png"><img src="badge.png"><img src="shot.png" width="10">"#;
    let root = site("image-width", &[("pages/index.html", page)]);
    for image in ["shot.png", "badge.png"] {
        fs::copy(fixture().join("images").join(image), root.join("pages").join(image)).unwrap();
    }
    let store = || fs::read_dir(root.join(".cinders/images")).unwrap().count();
    let limit = |max: u32| {
        fs::write(root.join("cinders.toml"), format!("[images]\nmax_width = {max}\n")).unwrap();
        text(&build(&root), "index.html")
    };
    // The screenshot is 64 by 48 and the badge 24 by 24.
    let html = limit(40);
    assert!(html.contains(".avif width=40 height=30><img src=/assets/badge-"), "{html}");
    assert!(html.contains(".avif width=24 height=24><img src=/assets/shot-"), "{html}");
    assert!(html.ends_with(".avif width=10>"), "the size an author wrote stays: {html}");
    assert_eq!(store(), 4, "each image is an encode and its size");
    assert!(limit(48).contains(".avif width=48 height=36><img src=/assets/badge-"));
    assert_eq!(store(), 6, "a new limit encodes again only the image that it shrinks");
    assert!(limit(1).contains(".avif width=1 height=1><img"), "a side keeps one pixel");
}

#[test]
fn public_ships_every_file_but_the_systems() {
    let files = [
        ("pages/index.html", r#"<a href="/.well-known/security.txt">x</a>"#),
        ("public/.DS_Store", "the system's"),
        ("public/.nojekyll", ""),
        ("public/.well-known/security.txt", "Contact: mailto:a@b.c"),
        ("public/.well-known/.DS_Store", "the system's"),
        ("public/deep/index.html", "deep"),
    ];
    let root = site("public", &files);
    let site = build(&root);
    let paths: Vec<&str> = site.files.keys().map(String::as_str).collect();
    let shipped = [".nojekyll", ".well-known/security.txt", "deep/index.html", "index.html"];
    assert_eq!(paths, shipped);
    assert_eq!(text(&site, "deep/index.html"), "deep", "a public file ships as written");

    fs::write(root.join("pages/index.html"), r#"<a href="/.DS_Store">x</a>"#).unwrap();
    let hidden = error(&root);
    assert!(hidden.contains("`/.DS_Store` is not a route or a file in public/"), "{hidden}");

    fs::write(root.join("pages/index.html"), r#"<a href="/deep">x</a><a href="/re">y</a>"#)
        .unwrap();
    fs::write(root.join("public/reindex.html"), "no index").unwrap();
    let named = error(&root);
    assert!(named.contains("`/re` is not a route"), "only index.html names its directory: {named}");

    fs::write(root.join("pages/index.html"), "home").unwrap();
    fs::create_dir_all(root.join("public/assets")).unwrap();
    fs::write(root.join("public/assets/.DS_Store"), "the system's").unwrap();
    assert_eq!(text(&build(&root), "index.html"), "home", "an empty public/assets/ is allowed");
    fs::write(root.join("public/assets/logo.svg"), SVG).unwrap();
    let expected = "public/assets/logo.svg: /assets/ is for fingerprinted files, which a host \
                    caches for a year. Move it out of public/assets/, or reference it by a \
                    relative path";
    assert_eq!(error(&root), expected, "a file in public/assets/ is an error");
    let fast = cinders::build(&root, Mode::Fast).map(drop).unwrap_err();
    assert_eq!(format!("{fast:#}"), expected, "in a fast build too");
}

#[test]
fn link_preview_images_are_checked_and_written_in_full() {
    let head = concat!(
        r#"<meta property="og:image" content="card.png">"#,
        r#"<meta name="twitter:image" content="/social.png?v=2">"#,
        r#"<meta property="og:image" content="https://cdn.example/x.png">"#,
        r#"<meta name="description" content="card.png">"#,
    );
    let files = [
        ("cinders.toml", "[site]\nurl = \"https://x.example/\"\n"),
        ("components/Head.html", head),
        ("pages/blog/index.html", r#"<Head /><img src="../../components/card.png">"#),
        ("public/social.png", "not decoded"),
    ];
    let root = site("previews", &files);
    fs::copy(fixture().join("images/shot.png"), root.join("components/card.png")).unwrap();
    let site = build(&root);
    let html = text(&site, "blog/index.html");
    let card = format!("https://x.example/{}", assets(&site, "png")[0]);
    assert!(html.contains(&format!("<meta property=og:image content={card}>")), "{html}");
    assert!(html.contains("content=\"https://x.example/social.png?v=2\">"), "{html}");
    assert!(html.contains("content=https://cdn.example/x.png>"), "{html}");
    assert!(html.contains("<meta name=description content=card.png>"), "{html}");
    assert!(html.contains("<img src=/assets/card-"), "{html}");
    assert_eq!(assets(&site, "avif").len(), 1, "the page's image is encoded, the preview's not");
    let fast = text(&build_fast(&root), "blog/index.html");
    assert!(fast.contains(r#"content="https://x.example/_cinders/components/card.png""#), "{fast}");

    fs::remove_file(root.join("public/social.png")).unwrap();
    let missing = error(&root);
    let expected = "pages/blog/index.html: <meta twitter:image>: `/social.png?v=2` is not a route \
                    or a file in public/";
    assert!(missing.contains(expected), "{missing}");
    fs::write(root.join("components/Head.html"), r#"<meta property="og:image" content="x.png">"#)
        .unwrap();
    let missing = error(&root);
    assert!(
        missing.contains("<meta og:image>: `../../components/x.png` does not exist"),
        "{missing}"
    );
    fs::write(root.join("components/x.png"), "").unwrap();
    fs::remove_file(root.join("cinders.toml")).unwrap();
    let relative = error(&root);
    let expected = "<meta og:image> needs [site] url in cinders.toml, since a link preview \
                    takes a full URL";
    assert!(relative.contains(expected), "{relative}");
}

#[test]
fn a_site_under_a_path_prefixes_every_rooted_reference() {
    let home = concat!(
        "<!doctype html><title>t</title><link rel=\"stylesheet\" href=\"../styles/site.css\">",
        "<meta property=\"og:image\" content=\"../art/mark.svg\">",
        "<script type=\"module\" src=\"../scripts/app.js\"></script>",
        "<style>p { background: url(/file.txt) }</style>",
        "<a href=\"/\">home</a><a href=\"/docs/a/#part\">rooted</a><a href=\"docs/a.html\">source</a>",
        "<a href=\"/file.txt\">public</a><a href=\"/api/x\">unchecked</a><a href=\"#top\">fragment</a>",
        "<a href=\"https://example.com/elsewhere\">out</a>",
        "<img src=\"../art/mark.svg\" srcset=\"/file.txt 1x, ../art/mark.svg 2x\" alt=\"\">",
        "<i style=\"background: url('/a b.txt'), url(../art/mark.svg)\"></i>",
        "{#if page.url == '/'}<a href={page.url}>{page.url}</a>{/if}",
    );
    let files = [
        ("cinders.toml", "unchecked = [\"/api/\"]\n[site]\nurl = \"https://example.com/base/\"\n"),
        ("pages/index.html", home),
        (
            "pages/docs/a.html",
            "<a href=\"/\">home</a><img src=\"../../art/mark.svg\" alt=\"mark\">",
        ),
        (
            "styles/site.css",
            "@import \"/extra.css\";\np { background: url(../art/mark.svg), url(/file.txt) }\n",
        ),
        ("scripts/app.js", "console.log(1);\n"),
        ("art/mark.svg", SVG),
        ("public/file.txt", "a file"),
        ("public/a b.txt", "a file"),
        ("public/extra.css", "p { margin: 0 }"),
    ];
    let root = site("e-prefix", &files);
    let built = build(&root);
    assert!(
        built.files.contains_key("docs/a/index.html") && built.prefix == "/base",
        "the output stays flat, and only the references carry the path"
    );
    let home = text(&built, "index.html");
    for written in [
        "<link rel=stylesheet href=/base/assets/site-",
        "content=https://example.com/base/assets/mark-",
        "<script type=module src=/base/assets/app-",
        "<style>p{background:url(/base/file.txt)}</style>",
        "<a href=/base/>home</a><a href=/base/docs/a/#part>rooted</a><a href=/base/docs/a/>source</a>",
        "<a href=/base/file.txt>public</a><a href=/base/api/x>unchecked</a><a href=#top>fragment</a>",
        "<a href=https://example.com/elsewhere>out</a>",
        "<img src=/base/assets/mark-",
        "srcset=\"/base/file.txt 1x, /base/assets/mark-",
        "style='background: url(\"/base/a b.txt\"), url(/base/assets/mark-",
        // `page.url` stays the route, so a comparison holds at any address.
        "<a href=/base/>/</a>",
    ] {
        assert!(home.contains(written), "{written} is not in {home}");
    }
    let css = asset(&built, "css");
    assert!(css.starts_with("@import \"/base/extra.css\";"), "{css}");
    assert!(css.contains("url(/base/assets/mark-") && css.contains("url(/base/file.txt)"), "{css}");
    let docs = text(&built, "docs/a/index.html");
    assert!(docs.contains("</script><a href=/base/>home</a><img src=/base/assets/mark-"), "{docs}");
    let sitemap = text(&built, "sitemap.xml");
    assert!(sitemap.contains("<loc>https://example.com/base/docs/a/</loc>"), "{sitemap}");
    assert!(!built.files.contains_key("robots.txt"), "crawlers read it at the root of a host only");

    let fast = build_fast(&root);
    let home = text(&fast, "index.html");
    assert!(
        home.contains(r#"<script type="module" src="/base/_cinders/scripts/app.js">"#),
        "{home}"
    );
    assert!(home.contains(r#"<a href="/base/docs/a/">source</a>"#), "{home}");
    assert!(fast.files.contains_key("_cinders/scripts/app.js"));
    let css = text(&fast, "_cinders/styles/site.css");
    assert!(css.contains("url(/base/_cinders/art/mark.svg), url(/base/file.txt)"), "{css}");

    let repeated = [
        ("cinders.toml", "[site]\nurl = \"https://example.com/base\"\n"),
        ("pages/index.html", "<a href=\"/base/about/\">about</a>"),
        ("pages/about.html", "about"),
    ];
    let repeated = error(&site("e-prefix-twice", &repeated));
    let expected = "pages/index.html: <a href>: `/base/about/` is not a route or a file in \
                    public/: write it without `/base`, which cinders adds";
    assert!(
        repeated.contains(expected),
        "a path that repeats the prefix names no place: {repeated}"
    );
}

#[test]
fn errors_name_the_problem() {
    let style = "<style>@import \"other.css\"; p { color: red }</style>";
    let root = site("e-import", &[("pages/index.html", style), ("pages/other.css", "")]);
    let import = error(&root);
    let expected = "pages/index.html: <style>: `@import \"other.css\"` in a <style> block: link \
                    the stylesheet instead";
    assert!(import.contains(expected), "{import}");

    let files = [
        ("pages/index.html", r#"<link rel="stylesheet" href="s.css">"#),
        ("pages/s.css", "@import \"more.css\";\np { mask: url(m.svg) }\n"),
        ("pages/more.css", "p { color: red"),
    ];
    let root = site("e-css", &files);
    let missing = error(&root);
    let expected = "pages/index.html: <link href>: pages/s.css: `m.svg` does not exist";
    assert!(missing.contains(expected), "{missing}");
    fs::remove_file(root.join("pages/more.css")).unwrap();
    let import = error(&root);
    let expected = "pages/index.html: <link href>: pages/s.css: `more.css` does not exist";
    assert!(import.ends_with(expected), "a missing import is named: {import}");
    fs::write(root.join("pages/s.css"), "@import \"../../e-css-outside.css\";\n").unwrap();
    fs::write(root.join("../e-css-outside.css"), "p { color: red }").unwrap();
    let outside = error(&root);
    let expected = "pages/s.css: `../../e-css-outside.css` is outside the site directory";
    assert!(outside.ends_with(expected), "an import outside the site is an error: {outside}");

    let root = site("e-twice", &[("pages/about.html", "a"), ("pages/about/index.html", "b")]);
    let twice = error(&root);
    assert!(twice.contains("both write about/index.html"), "{twice}");

    let root = site("e-public", &[("pages/index.html", "a"), ("public/index.html", "b")]);
    assert!(error(&root).contains("public/index.html and pages/index.html both write index.html"));

    let root = site("e-route", &[("pages/index.html", r#"<a href="/nope/">x</a>"#)]);
    assert!(error(&root).contains("<a href>: `/nope/` is not a route or a file in public/"));

    let files = [("pages/index.html", r#"<a href="_part.html">x</a>"#), ("pages/_part.html", "")];
    assert!(error(&site("e-part", &files)).contains("`_part.html` is a page that is not built"));

    let files = [
        ("pages/index.html", r#"<a href="notes.md#top">x</a>"#),
        ("pages/notes.md", "# Notes"),
        ("pages/README.md", "Linked by no page, so it is left alone."),
    ];
    let root = site("e-markdown", &files);
    let markdown = error(&root);
    let expected = "<a href>: `notes.md#top` is a Markdown page, which is not yet supported";
    assert!(markdown.contains(expected), "{markdown}");
    fs::write(root.join("pages/index.html"), r#"<a href="../notes.md">x</a>"#).unwrap();
    fs::write(root.join("notes.md"), "# Notes").unwrap();
    let html = text(&build(&root), "index.html");
    assert!(html.starts_with("<a href=/assets/notes-"), "outside pages/ it is a file: {html}");

    let files = [("pages/index.html", r#"<a href="blog">x</a>"#), ("pages/blog/a.html", "")];
    assert!(error(&site("e-dir", &files)).contains("is a directory, not a file"));

    let root = site(
        "e-outside",
        &[("pages/index.html", r#"<a href="../../e-dir/pages/blog/a.html">x</a>"#)],
    );
    assert!(error(&root).contains("is outside the site directory"));

    let files =
        [("pages/index.html", r#"<img srcset="a.svg 1x, b.svg 2x">"#), ("pages/a.svg", SVG)];
    let srcset = error(&site("e-srcset", &files));
    assert!(srcset.contains("pages/index.html: <img srcset>: `b.svg` does not exist"), "{srcset}");

    let files = [("pages/index.html", r#"<p style="mask: url(m.svg)">x</p>"#)];
    let style = error(&site("e-style", &files));
    let expected = "pages/index.html: style attribute: `m.svg` does not exist";
    assert!(style.contains(expected), "{style}");

    let page =
        r#"<script type="module" src="a.ts"></script><script type="module" src="a.js"></script>"#;
    let files = [("pages/index.html", page), ("pages/a.ts", ""), ("pages/a.js", "")];
    let root = site("e-fast", &files);
    assert_eq!(assets(&build(&root), "js").len(), 1, "two empty scripts ship as one file");
    let Err(fast) = cinders::build(&root, Mode::Fast) else { panic!("the build should fail") };
    let fast = format!("{fast:#}");
    let expected = "both write _cinders/pages/a.js";
    assert!(fast.contains(expected), "either script may be named first: {fast}");
}

#[test]
fn hoisted_blocks_follow_a_late_doctype_and_inline_blocks_escape_close_tags() {
    let style = r#"<style>p { content: "<\/STYLE>" } b { content: "</style-sheet>" }</style>"#;
    let script = r#"<script>console.log("<\/script>", "<\/script >", "<\/script/>");</script>"#;
    let card = format!("{style}<p>hi</p>");
    let page = format!("\n<!-- Note. -->\n<!doctype html><Card />{script}");
    let files = [("components/Card.html", card.as_str()), ("pages/index.html", &page)];
    let built = build(&site("e-doctype-escape", &files));
    let html = text(&built, "index.html");
    assert!(
        html.starts_with("<!doctype html><style>"),
        "hoisted blocks follow a doctype preceded by whitespace and comments: {html}"
    );
    assert!(
        html.contains(r#"<style>p{content:"<\/STYLE>"}b{content:"</style-sheet>"}</style>"#),
        "real close tags in inline <style> are escaped and longer tag names stay: {html}"
    );
    assert_eq!(
        html.matches("</script>").count(),
        1,
        "</script inside minified inline <script> stays escaped: {html}"
    );
    assert!(html.contains(r"<\/script>"), "escaped close tag stays in script body: {html}");
    assert!(html.contains(r"<\/script >"), "close tag with space is escaped: {html}");
    assert!(html.contains(r"<\/script/>"), "close tag with slash is escaped: {html}");

    // A fast build ships a script as written, so only the escape can change it.
    let raw = "<script>console.log(String.raw`</script-editor>`); // </script</script>";
    let fast =
        text(&build_fast(&site("e-escape-fast", &[("pages/index.html", raw)])), "index.html");
    let expected = r"<script>console.log(String.raw`</script-editor>`); // <\/script</script>";
    assert_eq!(fast, expected, "a longer tag name stays, and a trailing </script is escaped");
}
