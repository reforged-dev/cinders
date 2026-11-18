//! Edge cases of the optimizer, each on a small site in a temporary directory.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;

use cinders::Content;
use common::{assets, build, build_fast, error, fixture, site, text};

const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg'/>";

#[test]
fn references_in_style_attributes_and_public() {
    let page = concat!(
        r#"<div style="background: url('bg.svg')"></div>"#,
        r#"<a href="/docs/">docs</a><a href="/docs">docs</a><a href="?page=2">next</a>"#,
        r#"<a href="https://example.com/x.png">out</a><a href="mailto:a@b.c">mail</a>"#,
        r#"<a href="//cdn.example/x.js">host</a>"#,
        "<a href=#x>a</a> <a href=#y>b</a>",
    );
    let files =
        [("pages/index.html", page), ("pages/bg.svg", SVG), ("public/docs/index.html", "docs")];
    let site = build(&site("refs", &files));
    let html = text(&site, "index.html");
    assert!(html.contains("url(/assets/bg-"), "{html}");
    assert!(html.contains("href=/docs/>") && html.contains("href=/docs>"), "{html}");
    assert!(html.contains(r#"href="?page=2""#) && html.contains("href=#x"), "{html}");
    assert!(html.contains("href=https://example.com/x.png") && html.contains("mailto:a@b.c"));
    assert!(html.contains("href=//cdn.example/x.js>"), "{html}");
    assert!(html.contains("</a> <a"), "the space between inline elements stays: {html}");
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
        ("cinders.toml", "[site]\nurl = \"https://example.com\"\n"),
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
    assert!(error.contains("`../img/icons.svg#x` names no file"), "{error}");
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
    assert!(hidden.contains("`/.DS_Store` matches no route and no file in public/"), "{hidden}");

    fs::write(root.join("pages/index.html"), r#"<a href="/deep">x</a><a href="/re">y</a>"#)
        .unwrap();
    fs::write(root.join("public/reindex.html"), "no index").unwrap();
    let named = error(&root);
    assert!(
        named.contains("`/re` matches no route"),
        "only index.html names its directory: {named}"
    );
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
    let fast = text(&build_fast(&root), "blog/index.html");
    assert!(fast.contains(r#"content="https://x.example/_cinders/components/card.png""#), "{fast}");

    fs::remove_file(root.join("public/social.png")).unwrap();
    let missing = error(&root);
    let expected = "pages/blog/index.html: <meta twitter:image>: `/social.png?v=2` matches no \
                    route and no file in public/";
    assert!(missing.contains(expected), "{missing}");
    fs::write(root.join("components/Head.html"), r#"<meta property="og:image" content="x.png">"#)
        .unwrap();
    let missing = error(&root);
    assert!(
        missing.contains("<meta og:image>: `../../components/x.png` names no file"),
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
        ("styles/site.css", "p { margin: 0 }\n"),
        ("scripts/app.js", "console.log(1);\n"),
        ("art/mark.svg", SVG),
        ("public/file.txt", "a file"),
        ("public/a b.txt", "a file"),
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
    let docs = text(&built, "docs/a/index.html");
    assert!(docs.starts_with("<a href=/base/>home</a><img src=/base/assets/mark-"), "{docs}");
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

    let repeated = [
        ("cinders.toml", "[site]\nurl = \"https://example.com/base\"\n"),
        ("pages/index.html", "<a href=\"/base/about/\">about</a>"),
        ("pages/about.html", "about"),
    ];
    let repeated = error(&site("e-prefix-twice", &repeated));
    let expected = "pages/index.html: <a href>: `/base/about/` matches no route and no file in \
                    public/: write it without `/base`, which cinders adds";
    assert!(
        repeated.contains(expected),
        "a path that repeats the prefix names no place: {repeated}"
    );
}

#[test]
fn errors_name_the_problem() {
    let root = site("e-twice", &[("pages/about.html", "a"), ("pages/about/index.html", "b")]);
    let twice = error(&root);
    assert!(twice.contains("both write about/index.html"), "{twice}");

    let root = site("e-public", &[("pages/index.html", "a"), ("public/index.html", "b")]);
    assert!(error(&root).contains("public/index.html and pages/index.html both write index.html"));

    let root = site("e-route", &[("pages/index.html", r#"<a href="/nope/">x</a>"#)]);
    assert!(error(&root).contains("<a href>: `/nope/` matches no route and no file in public/"));

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
    assert!(error(&site("e-dir", &files)).contains("names a directory, not a file"));

    let root = site(
        "e-outside",
        &[("pages/index.html", r#"<a href="../../e-dir/pages/blog/a.html">x</a>"#)],
    );
    assert!(error(&root).contains("is outside the site directory"));

    let files =
        [("pages/index.html", r#"<img srcset="a.svg 1x, b.svg 2x">"#), ("pages/a.svg", SVG)];
    let srcset = error(&site("e-srcset", &files));
    assert!(srcset.contains("pages/index.html: <img srcset>: `b.svg` names no file"), "{srcset}");

    let files = [("pages/index.html", r#"<p style="mask: url(m.svg)">x</p>"#)];
    let style = error(&site("e-style", &files));
    let expected = "pages/index.html: style attribute: `m.svg` names no file";
    assert!(style.contains(expected), "{style}");
}
