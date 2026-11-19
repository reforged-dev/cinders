//! The template language, each case on a small site in a temporary directory.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

const SITE: &str = r#"[site]
title = "Fish & <Chips>"
name = "Cinders"
empty = ""
zero = 0
count = 3
ratio = 1.5
draft = false
since = 2026-01-05
tags = ["a", "b", "c"]
nav = [{ title = "Blog", url = "/blog/" }, { title = "Home", url = "/" }]
"#;

const SUPPORTED: &str = "expressions are literals, property paths, comparisons, &&, ||, and !. To \
                         print a brace, as in a code sample, write {\"{\"}";

/// The rendered HTML of a page with the source `page`, on a site with `files` beside it.
fn page_with(name: &str, page: &str, files: &[(&str, &str)]) -> String {
    let files = [&[("cinders.toml", SITE), ("pages/index.html", page)], files].concat();
    common::rendered(&common::site(name, &files), "index.html")
}

/// The error that rendering a page with the source `page` fails with.
fn error_with(name: &str, page: &str, files: &[(&str, &str)]) -> String {
    let files = [&[("cinders.toml", SITE), ("pages/index.html", page)], files].concat();
    common::render_error(&common::site(name, &files))
}

#[test]
fn holes_print_escaped_values() {
    let page = |source| page_with("t-holes", source, &[]);
    assert_eq!(page("<h1>{site.title}</h1>"), "<h1>Fish &amp; &lt;Chips&gt;</h1>");
    assert_eq!(page(r#"{'say "hi"'} {"it's"} {"a\"b"}"#), "say &quot;hi&quot; it's a&quot;b");
    assert_eq!(
        page("{site.count} {site.ratio} {site.draft} {42} {site.since}"),
        "3 1.5 false 42 2026-01-05"
    );
    assert_eq!(page(r#"a {"{"} b {"}"} c } d {page.url}"#), "a { b } c } d /");
    assert_eq!(page("< a <3 <!-- {x} --> </p>"), "< a <3 <!-- {x} --> </p>");
    let error = |source| error_with("t-holes", source, &[]);
    assert_eq!(error("{site.tags}"), "pages/index.html:1: cannot print a list: site.tags");
    assert_eq!(error("\n\n{site}"), "pages/index.html:3: cannot print a map: site");
    assert_eq!(error("{page}"), "pages/index.html:1: cannot print a map: page");
    assert_eq!(error("<p>{site.name"), "pages/index.html:1: unclosed `{`");
}

#[test]
fn page_holds_only_its_url() {
    let page = |source| page_with("t-page", source, &[("components/Url.html", "{page.url}")]);
    assert_eq!(page("{page.url}|<Url />"), "/|/", "a component reads the page's URL too");
    assert_eq!(page("{#if page.title}titled{:else}untitled{/if}"), "untitled");
    let error = error_with("t-page", "{page.title}", &[]);
    assert_eq!(error, "pages/index.html:1: page.title is missing");
}

#[test]
fn attributes_take_text_holes_and_booleans() {
    let page = |source| page_with("t-attrs", source, &[]);
    assert_eq!(page("<a href={page.url}>x</a>"), r#"<a href="/">x</a>"#);
    assert_eq!(page(r#"<p class="a {site.name} b">"#), r#"<p class="a Cinders b">"#);
    assert_eq!(page("<p title={site.title}>"), r#"<p title="Fish &amp; &lt;Chips&gt;">"#);
    assert_eq!(page(r#"<p title='say "{site.name}"'>"#), r#"<p title="say &quot;Cinders&quot;">"#);
    assert_eq!(page("<input disabled value=x />"), r#"<input disabled value="x" />"#);
    assert_eq!(page("<p data-n={site.count} id = 'a'>"), r#"<p data-n="3" id="a">"#);
    assert_eq!(
        page("<input checked={site.draft} required={!site.draft}>"),
        "<input required>",
        "a boolean adds or omits the attribute"
    );
    assert_eq!(page("<input checked={site.count == 3}>"), "<input checked>");
    let error = |source| error_with("t-attrs", source, &[]);
    assert_eq!(error("<a href={nope.url}>"), "pages/index.html:1: nope.url is missing");
    assert_eq!(error("\n<a class=\"x {post.cls}\">"), "pages/index.html:2: post.cls is missing");
    assert_eq!(error("<a href=\"x>"), "pages/index.html:1: unclosed attribute value");
    assert_eq!(error("<a href"), "pages/index.html:1: unclosed tag");
    assert_eq!(error("<a =x>"), "pages/index.html:1: expected an attribute");
    assert!(error("<a class={#if site.name}a{/if}>").contains("a block needs a quoted attribute"));
}

#[test]
fn if_blocks_pick_the_first_true_arm() {
    let page = |source| page_with("t-if", source, &[]);
    let chain =
        "{#if site.zero}zero{:else if site.empty}empty{:else if site.name}name{:else}none{/if}";
    assert_eq!(page(chain), "name");
    assert_eq!(page("{#if site.draft}d{:else}live{/if}"), "live");
    assert_eq!(page("{#if !site.draft && site.count}yes{/if}"), "yes");
    assert_eq!(page("{#if site.draft || (site.name && !site.empty)}yes{/if}"), "yes");
    assert_eq!(
        page("{#if site.tags}tags{/if}{#if site.empty}no{/if}{#if site}map{/if}"),
        "tagsmap"
    );
    assert_eq!(page("{#if true}{#if false}a{:else}b{/if}{/if}"), "b");
    let nav =
        r#"{#each site.nav as link}<a class="nav{#if link.url == page.url} current{/if}">{/each}"#;
    let expected = r#"<a class="nav"><a class="nav current">"#;
    assert_eq!(page(nav), expected, "a block works inside a quoted attribute value");
    let classes =
        r#"<p class="{#each site.tags as tag}t-{tag} {/each}{#if site.draft}d{:else}live{/if}">"#;
    assert_eq!(page(classes), r#"<p class="t-a t-b t-c live">"#);
}

#[test]
fn each_blocks_loop_over_lists() {
    let page = |source| page_with("t-each", source, &[]);
    assert_eq!(
        page("{#each site.tags as tag, i}{#if i > 0}, {/if}{i}:{tag}{/each}"),
        "0:a, 1:b, 2:c"
    );
    let nested = "{#each site.tags as a}{#each site.tags as b}{#if a == b}{a}{/if}{/each}{/each}";
    assert_eq!(page(nested), "abc");
    assert_eq!(
        page("{#each site.nav as link}<a href={link.url}>{link.title}</a>{/each}"),
        r#"<a href="/blog/">Blog</a><a href="/">Home</a>"#
    );
    assert_eq!(page("[{#each site.none as x}{x}{/each}]"), "[]");
    assert_eq!(
        page("{#each site.tags as site}{site}{/each}{site.name}"),
        "abcCinders",
        "an inner name hides an outer one, and ends with its block"
    );
    let error = error_with("t-each", "{#each site.name as x}{/each}", &[]);
    assert_eq!(error, "pages/index.html:1: cannot loop over a string: site.name");
}

#[test]
fn block_errors_name_the_line() {
    let error = |source| error_with("t-blocks", source, &[("components/Card.html", "<slot />")]);
    assert_eq!(error("a\n{#if site.title}x"), "pages/index.html:2: unclosed {#if}");
    assert_eq!(error("{#each site.tags as t}x"), "pages/index.html:1: unclosed {#each}");
    assert_eq!(
        error("{#if site.title}x{/each}"),
        "pages/index.html:1: unexpected {/each}, expected {/if}"
    );
    assert_eq!(error("\n{/if}"), "pages/index.html:2: unexpected {/if}");
    assert_eq!(error("{:else}"), "pages/index.html:1: unexpected {:else}");
    let after_else = "{#if site.a}a{:else}b{:else if site.c}c{/if}";
    assert_eq!(error(after_else), "pages/index.html:1: unexpected {:else}, expected {/if}");
    assert!(error("{#each site.tags}{/each}").contains("expected `{#each list as item}` or"));
    assert!(error("{#each site.tags as 1x}{/each}").contains("expected `{#each list as item}` or"));
    assert_eq!(error("{#while x}"), "pages/index.html:1: unknown block `{#while x}`");
    assert_eq!(error("{:elseif x}"), "pages/index.html:1: unknown block `{:elseif x}`");
    assert_eq!(
        error("<Card>{#if x}</Card>{/if}"),
        "pages/index.html:1: unexpected </Card>, expected {/if}"
    );
    assert_eq!(error("<Card>"), "pages/index.html:1: unclosed <Card>");
    assert_eq!(error("</Card>"), "pages/index.html:1: unexpected </Card>");
    assert_eq!(
        error("<p class=\"{#if x}a\">"),
        "pages/index.html:1: unexpected closing quote, expected {/if}"
    );
    assert_eq!(error("<script>let a = {}"), "pages/index.html:1: unclosed <script>");
    assert!(error("<slot>x</slot>").contains("a slot takes no fallback content"));
}

#[test]
fn missing_values_are_false_and_never_print() {
    let page = |source| page_with("t-missing", source, &[]);
    assert_eq!(page("{#if site.titel}x{:else}missing{/if}"), "missing");
    assert_eq!(page("{#if site.titel.deeper}x{/if}{#if !nope.at.all}ok{/if}"), "ok");
    assert_eq!(page("{#if site.titel == site.other}same{/if}"), "same");
    assert_eq!(page("{#if site.titel != 'x'}ne{/if}{#if site.titel < 3}lt{/if}"), "ne");
    let error = error_with("t-missing", "<h1>\n{site.titel}</h1>", &[]);
    assert_eq!(error, "pages/index.html:2: site.titel is missing");
}

#[test]
fn unsupported_expressions_list_the_supported_forms() {
    for expr in [
        "x => y",
        "site.tags.map(t)",
        "site.title.trim()",
        "a = b",
        "a + b",
        "a - 1",
        "site.tags[0]",
        "a ? b : c",
        "foo(1)",
        "`tpl`",
        "",
        "site.",
        "(a",
        "1.2.3",
        "a b",
    ] {
        let error = error_with("t-unsupported", &format!("<p>\n{{{expr}}}</p>"), &[]);
        let expected = format!("pages/index.html:2: unsupported expression `{expr}`: {SUPPORTED}");
        assert_eq!(error, expected, "for `{expr}`");
    }
    let sample = "<pre>fn main() {\"{\"} println!(\"hi\"); }</pre>";
    let expected = "<pre>fn main() { println!(\"hi\"); }</pre>";
    assert_eq!(page_with("t-brace", sample, &[]), expected, "the error's own advice works");
    let error = error_with("t-brace-error", "<pre>fn main() { run(); }</pre>", &[]);
    assert!(error.ends_with("write {\"{\"}") && error.contains("`run();`"), "{error}");
}

#[test]
fn calls_are_unsupported_expressions() {
    for (source, expr) in [
        ("{pages('blog')}", "pages('blog')"),
        ("{#each pages(\"blog\") as post}{post.title}{/each}", "pages(\"blog\")"),
        ("{#each limit(site.tags, 2) as tag}{tag}{/each}", "limit(site.tags, 2)"),
        ("{date(site.since, '%B %-d, %Y')}", "date(site.since, '%B %-d, %Y')"),
        ("{#if site.name && date(site.since, '%Y')}x{/if}", "site.name && date(site.since, '%Y')"),
        ("{#if !(pages(''))}x{/if}", "!(pages(''))"),
        ("<a href={limit (site.tags, 1)}>x</a>", "limit (site.tags, 1)"),
    ] {
        let error = error_with("t-calls", source, &[]);
        let expected = format!("pages/index.html:1: unsupported expression `{expr}`: {SUPPORTED}");
        assert_eq!(error, expected, "for `{source}`");
    }
}

#[test]
fn braces_are_literal_in_scripts_styles_and_comments() {
    let page = |source| page_with("t-literal", source, &[]);
    let source = "<script>if (a) { b() }</script><style>p { color: red }</style><!-- {x} -->";
    assert_eq!(page(source), source);
    let source = r#"<script type="module" src={page.url}>let o = {};</script>"#;
    assert_eq!(page(source), r#"<script type="module" src="/">let o = {};</script>"#);
    let source =
        "<script>a = \"</scripture>\"; b = {}</SCRIPT ><style>p { --x: \"</styles>\" }</style>";
    assert_eq!(
        page(source),
        source.replace("</SCRIPT >", "</script>"),
        "`</scripture>` and `</styles>` are not end tags"
    );
}

#[test]
fn comparisons_never_convert_types() {
    let page = |source| page_with("t-compare", source, &[]);
    assert_eq!(page("{#if site.count == 3}eq{/if}{#if site.count != 4}ne{/if}"), "eqne");
    assert_eq!(page("{#if site.count < 10}lt{/if}{#if site.count >= 3}ge{/if}"), "ltge");
    assert_eq!(page("{#if site.count > 3}gt{/if}{#if site.count <= 2}le{/if}"), "");
    assert_eq!(
        page("{#if 'apple' < 'banana'}lt{/if}{#if site.name == \"Cinders\"}eq{/if}"),
        "lteq"
    );
    assert_eq!(
        page("{#if site.count == '3'}loose{/if}{#if site.draft == false}strict{/if}"),
        "strict"
    );
    assert_eq!(page("{site.count == 3} {1.5 == site.ratio}"), "true true");
    assert_eq!(
        page("{#if site.tags == site.tags}same{/if}{#if site.nav == site.tags}x{/if}"),
        "same"
    );
    let error = error_with("t-compare", "{#if site.count < 'x'}{/if}", &[]);
    assert_eq!(error, "pages/index.html:1: cannot compare a number with a string");
}

#[test]
fn components_take_props_and_children() {
    let files = [
        (
            "components/Card.html",
            "<div class=\"card {kind}\"><h2>{title}</h2>{count} {site.name}<slot /></div>",
        ),
        ("components/ui/Bare.html", "<hr>{#if open}open{/if}"),
        (
            "components/Wrap.html",
            "<Card title={title} count={0} kind=\"w\"><slot /><slot /></Card>",
        ),
        ("components/List.html", "{#each items as item}{item}{/each}"),
        ("components/Self.html", "<Self />"),
        ("components/notes.txt", "not a component"),
    ];
    let page = |source| page_with("t-components", source, &files);
    let cards = "{#each site.tags as t}<Card title=\"T {t}\" count={site.count} kind=x><b>{t}</b></Card>{/each}";
    assert!(page(cards).starts_with(
        r#"<div class="card x"><h2>T a</h2>3 Cinders<b>a</b></div><div class="card x"><h2>T b"#
    ));
    assert_eq!(
        page("<Bare>dropped</Bare><Bare open />"),
        "<hr><hr>open",
        "a component is found by its file name alone, and a bare prop is true"
    );
    assert_eq!(
        page("{#each site.tags as t}<Wrap title={t}>{t}!</Wrap>{/each}"),
        r#"<div class="card w"><h2>a</h2>0 Cindersa!a!</div><div class="card w"><h2>b</h2>0 Cindersb!b!</div><div class="card w"><h2>c</h2>0 Cindersc!c!</div>"#,
        "children pass through nested components, and render in the scope that wrote them"
    );
    assert_eq!(
        page("<Card title=\"Tom &amp; {site.name} &lt;3\" count='1' kind=k />"),
        r#"<div class="card k"><h2>Tom &amp; Cinders &lt;3</h2>1 Cinders</div>"#,
        "entities in a string prop decode, so the text is escaped once when it prints"
    );
    assert_eq!(page("<List items={site.tags} />"), "abc", "a prop takes a value of any type");
    let error = |source| error_with("t-components", source, &files);
    assert_eq!(error("\n<Nope a=\"b\" />"), "pages/index.html:2: unknown component <Nope>");
    assert_eq!(error("<DIV>x</DIV>"), "pages/index.html:1: unknown component <DIV>");
    assert_eq!(
        error("<Card title=\"t\" count={1} kind=k>\n{title}</Card>"),
        "pages/index.html:2: title is missing"
    );
    assert_eq!(
        error("<Card count={1} kind=k />"),
        "pages/index.html: components/Card.html:1: title is missing"
    );
    assert_eq!(
        error("<Self />"),
        "pages/index.html: components/Self.html:1: <Self>: components nest deeper than 32 levels"
    );

    let twice =
        [("components/Card.html", ""), ("components/ui/Card.html", ""), ("pages/index.html", "")];
    assert_eq!(
        common::render_error(&common::site("t-twice", &twice)),
        "components/Card.html and components/ui/Card.html are both the component <Card>"
    );
    let broken = [("components/Card.html", "\n{#if x}"), ("pages/index.html", "")];
    assert_eq!(
        common::render_error(&common::site("t-broken", &broken)),
        "components/Card.html:2: unclosed {#if}"
    );
}

#[test]
fn component_styles_and_scripts_hoist_once() {
    let files = [
        (
            "components/A.html",
            "<style>.a{}</style>\n<script type=\"module\" src=\"a.ts\"></script><B /><i>a</i>",
        ),
        (
            "components/B.html",
            "<style>.b{}</style><div><style>.nested{}</style></div>{#if site.name}<style>.block{}</style>{/if}<b>b</b>",
        ),
    ];
    let page = |source| page_with("t-hoist", source, &files);
    let b = "<div><style>.nested{}</style></div><style>.block{}</style><b>b</b>";
    let head = r#"<style>.a{}</style><script type="module" src="../components/a.ts"></script><style>.b{}</style>"#;
    assert_eq!(
        page("<style>.page{}</style><A /><A /><B />"),
        format!("{head}<style>.page{{}}</style>\n{b}<i>a</i>\n{b}<i>a</i>{b}"),
        "a page's own style stays in place, and without a <head> the blocks go first"
    );
    let after_doctype = format!("<!DOCTYPE html><style>.b{{}}</style><title>t</title>{b}");
    assert_eq!(page("<!DOCTYPE html><title>t</title><B />"), after_doctype);
    assert_eq!(
        page("\u{feff}<!DOCTYPE html><title>t</title><B />"),
        after_doctype,
        "a page's byte order mark is dropped, so the doctype stays first"
    );
    let marked = [("components/Mark.html", "\u{feff}<i>m</i>")];
    assert_eq!(
        page_with("t-mark", "<p><Mark /></p>\u{feff}", &marked),
        "<p><i>m</i></p>\u{feff}",
        "a component's byte order mark is dropped, and only a leading one is"
    );
    assert_eq!(
        page("<html><head><title>t</title></head><body><B /><B /></body></html>"),
        format!(
            "<html><head><title>t</title><style>.b{{}}</style></head><body>{b}{b}</body></html>"
        )
    );
}

#[test]
fn paths_in_components_resolve_from_the_page() {
    let hero = concat!(
        "<style>.h { background: url(\"img/bg.png\"); mask: url(/m.svg); cursor: url( 'c.cur' ) }</style>",
        "<img src=\"img/a.png\" srcset=\"img/a.png 1x, ../b.png 2x\" alt=\"img/a.png\">",
        "<a href=\"#top\"></a><a href=\"https://x.y/a\"></a><a href=\"mailto:a@b\"></a><a href=\"/about/\"></a>",
        "<a href=\"doc.pdf?v=1#p\"></a><video poster=\"./p.jpg\"></video><object data='d.svg'></object>",
        "<a href={page.url}></a><a href=\"{page.url}x.html\"></a><use xlink:href=\"icons.svg#x\"/>",
        "<div style=\"background: url(../img/a.png); mask: url(&quot;m.svg&quot;)\"></div><p style=\"color: red\">",
    );
    let edges = concat!(
        "<meta property=\"og:image\" content=\"card.png\"><meta name=\"description\" content=\"card.png\">",
        "<img srcset=\"a.png, b.png 480w\"><img src=\"my dir/a b.png\">",
        "<a href=\"../../pages/blog/\"></a><a href=\"../../pages/blog/pic.png#x\"></a>",
        "<a href=\"../../pages/blog/deep/#top\"></a>",
    );
    let files = [
        ("components/Hero.html", hero),
        ("components/ui/Edges.html", edges),
        ("pages/blog/index.html", "<Hero /><img src=\"a.png\">"),
        ("pages/blog/deep/post.html", "<Edges />"),
        ("pages/index.html", "<Edges />"),
    ];
    let root = common::site("t-rebase", &files);
    let html = common::rendered(&root, "blog/index.html");
    let expected = concat!(
        "<style>.h { background: url(\"../../components/img/bg.png\"); mask: url(/m.svg); cursor: url( '../../components/c.cur' ) }</style>",
        "<img src=\"../../components/img/a.png\" srcset=\"../../components/img/a.png 1x, ../../b.png 2x\" alt=\"img/a.png\">",
        "<a href=\"#top\"></a><a href=\"https://x.y/a\"></a><a href=\"mailto:a@b\"></a><a href=\"/about/\"></a>",
        "<a href=\"../../components/doc.pdf?v=1#p\"></a><video poster=\"../../components/p.jpg\"></video><object data=\"../../components/d.svg\"></object>",
        "<a href=\"/blog/\"></a><a href=\"/blog/x.html\"></a><use xlink:href=\"../../components/icons.svg#x\" />",
        "<div style=\"background: url(../../img/a.png); mask: url(&quot;../../components/m.svg&quot;)\"></div><p style=\"color: red\">",
        "<img src=\"a.png\">",
    );
    assert_eq!(html, expected, "a path the page wrote itself stays as written");

    let deep = common::rendered(&root, "blog/deep/post/index.html");
    let expected = concat!(
        "<meta property=\"og:image\" content=\"../../../components/ui/card.png\"><meta name=\"description\" content=\"card.png\">",
        "<img srcset=\"../../../components/ui/a.png, ../../../components/ui/b.png 480w\"><img src=\"../../../components/ui/my%20dir/a%20b.png\">",
        "<a href=\"..\"></a><a href=\"../pic.png#x\"></a>",
        "<a href=\".#top\"></a>",
    );
    assert_eq!(deep, expected, "a path moves by the page's depth, and its own directory is a dot");
    let home = common::rendered(&root, "index.html");
    assert!(home.contains("<a href=\"blog\"></a><a href=\"blog/pic.png#x\"></a>"), "{home}");
    assert!(home.contains("content=\"../components/ui/card.png\">"), "{home}");
}

#[test]
fn string_props_that_name_a_file_resolve_from_the_page() {
    let files = [
        ("components/Icon.html", "<img src={src} alt={alt}>"),
        ("components/Star.html", "<Icon src=\"icons/star.svg\" alt=\"icons/nope.svg\" />"),
        ("components/icons/star.svg", "<svg/>"),
        ("pages/blog/index.html", "<Star /><Icon src=\"pic.png\" alt=\"x\" />"),
        ("pages/blog/pic.png", ""),
    ];
    let html = common::rendered(&common::site("t-file-props", &files), "blog/index.html");
    assert_eq!(
        html,
        r#"<img src="../../components/icons/star.svg" alt="icons/nope.svg"><img src="pic.png" alt="x">"#
    );
}
