//! Routing, the sitemap, and `robots.txt`, each on a small site in a temporary directory.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use cinders::Rendered;

const URL: (&str, &str) = ("cinders.toml", "[site]\nurl = \"https://example.com/\"\n");

/// The output paths of the rendered pages, in their order.
fn paths(rendered: &Rendered) -> Vec<&str> {
    rendered.pages.iter().map(|page| page.path.as_str()).collect()
}

/// The sitemap that lists `urls`, each without the site's address.
fn sitemap(urls: &[&str]) -> String {
    let urls = urls.iter().map(|url| format!("<url><loc>https://example.com{url}</loc></url>"));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{}</urlset>\n",
        urls.collect::<String>()
    )
}

#[test]
fn routes_follow_the_files_under_pages() {
    let files = [
        URL,
        ("pages/index.html", "home"),
        ("pages/about.html", "about"),
        ("pages/404.html", "lost"),
        ("pages/blog/index.html", "blog"),
        ("pages/blog/post.html", "{page.url}"),
        ("pages/blog/404.html", "a post about 404s"),
        ("pages/blog/_partial.html", "never a route"),
        ("pages/blog/.hidden.html", "never a route"),
        ("pages/blog/photo.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>"),
        ("pages/blog/notes.txt", "an asset"),
    ];
    let root = common::site("r-routes", &files);
    let rendered = common::render(&root).unwrap();
    assert_eq!(
        paths(&rendered),
        [
            "404.html",
            "about/index.html",
            "blog/404/index.html",
            "blog/index.html",
            "blog/post/index.html",
            "index.html"
        ],
        "pages come in the order of their output paths"
    );
    let post = &rendered.pages[4];
    assert_eq!(post.html, "/blog/post/");
    assert_eq!(post.url(), "/blog/post/");
    assert_eq!(post.source, root.canonicalize().unwrap().join("pages/blog/post.html"));
    assert_eq!(rendered.pages[0].url(), "/404.html");

    assert_eq!(
        rendered.files["sitemap.xml"],
        sitemap(&["/", "/about/", "/blog/", "/blog/404/", "/blog/post/"]),
        "the sitemap lists every route but the 404 page"
    );
    assert_eq!(
        rendered.files["robots.txt"],
        "User-agent: *\nAllow: /\nSitemap: https://example.com/sitemap.xml\n"
    );
    assert_eq!(rendered.files.len(), 2);
}

#[test]
fn markdown_files_and_layouts_build_no_route() {
    let files = [
        URL,
        ("pages/index.html", "home"),
        ("pages/post.md", "# Post"),
        ("pages/_layout.html", "<main><slot /></main>"),
        ("pages/blog/index.md", "---\ntitle: Blog\n---\nPosts."),
        ("pages/blog/_layout.html", "<slot />"),
    ];
    let rendered = common::render(&common::site("r-markdown", &files)).unwrap();
    assert_eq!(paths(&rendered), ["index.html"]);
    assert_eq!(rendered.pages[0].html, "home", "no layout wraps a page");
    assert_eq!(rendered.files["sitemap.xml"], sitemap(&["/"]));
}

#[test]
fn frontmatter_is_not_supported_yet() {
    for (name, page) in [
        ("r-frontmatter", "---\ntitle: Home\n---\n<h1>Home</h1>"),
        ("r-frontmatter-crlf", "---\r\ntitle: Home\r\n---\r\n<h1>Home</h1>"),
        ("r-frontmatter-mark", "\u{feff}---\n---\n"),
    ] {
        let error = common::render_error(&common::site(name, &[("pages/about.html", page)]));
        assert_eq!(error, "pages/about.html:1: frontmatter is not yet supported for HTML pages");
    }
    let files = [
        ("pages/index.html", "--- a rule ---\n<Rule />"),
        ("components/Rule.html", "---\ntitle: Rule\n---\n<hr>"),
    ];
    let html = common::rendered(&common::site("r-dashes", &files), "index.html");
    let expected = "--- a rule ---\n---\ntitle: Rule\n---\n<hr>";
    assert_eq!(html, expected, "dashes are text in a component and within a page's first line");
}

#[test]
fn route_urls_are_percent_encoded() {
    let files = [
        ("cinders.toml", "[site]\nurl = \"https://example.com/a&b\"\n"),
        ("pages/my post.html", "{page.url}"),
        ("pages/c#/index.html", "{page.url}"),
    ];
    let rendered = common::render(&common::site("r-encoded", &files)).unwrap();
    assert_eq!(paths(&rendered), ["c#/index.html", "my post/index.html"]);
    assert_eq!(rendered.pages[0].html, "/c%23/");
    assert_eq!(rendered.pages[1].html, "/my%20post/", "page.url leaves out the prefix");
    let sitemap = &rendered.files["sitemap.xml"];
    let expected = "<loc>https://example.com/a&amp;b/c%23/</loc></url><url><loc>https://example.com/a&amp;b/my%20post/</loc>";
    assert!(sitemap.contains(expected), "{sitemap}");
    assert_eq!(rendered.files.len(), 1, "a site with a prefix gets no robots.txt");
}

#[test]
fn sitemap_and_robots_yield_to_public_files() {
    let page = ("pages/index.html", "home");
    let both = [URL, page, ("public/sitemap.xml", "mine"), ("public/robots.txt", "mine")];
    let rendered = common::render(&common::site("r-public", &both)).unwrap();
    assert!(rendered.files.is_empty(), "each public file takes the generated one's place");
    for (name, public, generated) in [
        ("r-public-sitemap", "public/sitemap.xml", "robots.txt"),
        ("r-public-robots", "public/robots.txt", "sitemap.xml"),
    ] {
        let rendered = common::render(&common::site(name, &[URL, page, (public, "mine")])).unwrap();
        let files: Vec<&String> = rendered.files.keys().collect();
        assert_eq!(files, [generated], "{public} leaves the other file generated");
    }
    let none = common::render(&common::site("r-no-url", &[page])).unwrap();
    assert!(none.files.is_empty(), "no sitemap without [site] url");
}

#[test]
fn a_site_with_no_pages_is_an_error() {
    let files = [URL, ("pages/_draft.html", "draft"), ("pages/notes.md", "notes")];
    let error = common::render_error(&common::site("r-empty", &files));
    let expected = "r-empty/pages has no .html file, so there is no site to build";
    assert!(error.ends_with(expected), "{error}");
    let error = common::render_error(&common::site("r-bare", &[("dist/kept.txt", "kept")]));
    assert!(
        error.contains("r-bare/pages has no .html file"),
        "a directory with no pages/ fails too: {error}"
    );
}

#[test]
fn build_reads_the_site_in_a_directory() {
    let files = [("pages/index.html", r#"<img src="a.svg">"#), ("pages/a.svg", "<svg/>")];
    let root = common::site("r-build", &files);
    let site = common::build(&root.join("pages/.."));
    let html = common::text(&site, "index.html");
    assert!(html.starts_with("<img src=/assets/a-"), "a reference resolves in the site: {html}");

    let error = common::error(&root.join("nope"));
    assert!(error.contains("r-build/nope"), "the error names the directory: {error}");
    let broken = common::site("r-broken", &[("cinders.toml", "feeds = [\"blog\"]\n")]);
    let error = common::error(&broken);
    assert!(error.contains("unknown field `feeds`"), "{error}");
}
