//! Runs `cinders dev` and checks what it serves, how it reloads, and how it reports a failed
//! build.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use common::{fixture, site};

/// The header of a request that a browser on this machine sends.
const LOCAL: &str = "Host: localhost\r\n";

/// A running `cinders dev`, killed when the test ends, even by a failed assertion.
struct Server {
    child: Child,
    port: u16,
    lines: Receiver<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().unwrap_or_default();
        self.child.wait().map(drop).unwrap_or_default();
    }
}

struct Response {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Response {
    fn text(&self) -> &str {
        str::from_utf8(&self.body).unwrap()
    }
}

/// Starts `cinders dev` for `root` on a free port, which its first line of output names.
fn serve(root: &Path, args: &[&str]) -> Server {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cinders"));
    command.arg("dev").arg(root).args(["--port", "0"]).args(args).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut log = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    log.read_line(&mut line).unwrap();
    let port = line.trim().rsplit(':').next().unwrap().parse();
    let (tx, lines) = mpsc::channel();
    // Drain stderr so the server never waits on a full pipe, forwarding lines to tests that read them.
    thread::spawn(move || {
        for line in log.lines().map_while(Result::ok) {
            tx.send(line).map(drop).unwrap_or_default();
        }
    });
    Server { child, port: port.unwrap_or_else(|_| panic!("no port in `{line}`")), lines }
}

impl Server {
    /// Sends a request with `headers`, which holds whole header lines, and no other header.
    fn connect(&self, method: &str, path: &str, headers: &str) -> TcpStream {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\n{headers}\r\n").unwrap();
        stream
    }

    fn request(&self, method: &str, path: &str, headers: &str) -> Response {
        let mut bytes = Vec::new();
        self.connect(method, path, headers).read_to_end(&mut bytes).unwrap();
        let end = bytes.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        Response { status, head, body: bytes[end + 4..].to_vec() }
    }

    fn get(&self, path: &str) -> Response {
        self.request("GET", path, LOCAL)
    }

    /// Opens the event stream.
    fn events(&self) -> BufReader<TcpStream> {
        BufReader::new(self.connect("GET", "/_cinders/events", LOCAL))
    }
}

/// Waits for the next build number on an event stream.
fn next_build(events: &mut impl BufRead) -> u64 {
    let start = Instant::now();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(events.read_line(&mut line).unwrap() > 0, "the stream ended");
        // An idle stream still sends comments, so only the clock ends a wait for no build.
        assert!(start.elapsed() < Duration::from_secs(20), "no build arrived");
        if let Some(data) = line.strip_prefix("data: ") {
            return data.trim().parse().unwrap();
        }
    }
}

/// Reads build numbers until the server's response to `path` passes `shown`, and returns the
/// last number with that response.
fn wait_for(
    server: &Server,
    events: &mut impl BufRead,
    path: &str,
    shown: impl Fn(&Response) -> bool,
) -> (u64, Response) {
    loop {
        let build = next_build(events);
        let response = server.get(path);
        if shown(&response) {
            return (build, response);
        }
    }
}

/// The first `href` of a page, without its quotes.
fn first_href(page: &Response) -> &str {
    let href = page.text().split("href=").nth(1).unwrap().split('>').next().unwrap();
    href.trim_matches('"')
}

#[test]
fn serves_pages_and_assets_from_memory() {
    let server = serve(&fixture(), &[]);
    let home = server.get("/");
    assert_eq!(home.status, 200);
    assert!(home.head.contains("Content-Type: text/html; charset=utf-8"), "{}", home.head);
    assert!(home.head.contains("Cache-Control: no-cache"), "{}", home.head);
    let html = home.text();
    assert!(html.contains("<!-- A comment"), "a dev page is not minified: {html}");
    assert!(html.contains(r#"<script type="module" src="/_cinders/scripts/app.js">"#), "{html}");
    assert!(html.contains(r#"new EventSource("/_cinders/events")"#), "{html}");
    let reload = r#"<script type="module" data-client-nav>const build=""#;
    assert!(
        html.contains(reload),
        "the reload script is written for client-side navigation: {html}"
    );
    let navigation = r#"<head><script type="module" src="/assets/navigation.js"></script>"#;
    assert!(html.contains(navigation), "{html}");
    let navigation = server.get("/assets/navigation.js");
    assert!(navigation.head.contains("Content-Type: text/javascript"), "{}", navigation.head);
    assert!(navigation.text().starts_with("const DEV = true;"), "the script's dev form");
    assert_eq!(server.get("/?utm=1").body, home.body, "a query is ignored");
    assert_eq!(server.get("/index.html").body, home.body);

    let script = server.get("/_cinders/scripts/app.js");
    assert!(script.head.contains("Content-Type: text/javascript"), "{}", script.head);
    assert!(script.text().contains("function greet("), "scripts are bundled: {}", script.text());
    assert!(!script.text().contains("EventSource"), "only pages get the reload script");
    let styles = server.get("/_cinders/styles/site.css");
    assert!(styles.head.contains("Content-Type: text/css"), "{}", styles.head);
    let sitemap = server.get("/sitemap.xml");
    assert!(sitemap.head.contains("Content-Type: application/xml"), "{}", sitemap.head);
    assert!(sitemap.text().contains("https://fixture.example/about/"), "{}", sitemap.text());
    assert_eq!(server.get("/robots.txt").status, 200, "public files are served");

    let about = server.get("/about?x=1");
    assert_eq!(about.status, 307);
    assert!(about.head.contains("Location: /about/?x=1"), "{}", about.head);
    let missing = server.get("/nope/");
    assert_eq!(missing.status, 404);
    assert!(missing.text().contains("Nothing here."), "the site's 404 page: {}", missing.text());
    assert!(missing.text().contains("EventSource"), "{}", missing.text());

    let head = server.request("HEAD", "/", LOCAL);
    assert_eq!(head.status, 200);
    assert!(head.head.contains(&format!("Content-Length: {}", home.body.len())), "{}", head.head);
    assert!(head.body.is_empty(), "a HEAD request gets no body");
    let post = server.request("POST", "/", &format!("{LOCAL}Content-Length: 0\r\n"));
    assert_eq!(post.status, 405);
    assert!(post.head.contains("Allow: GET, HEAD"), "{}", post.head);
}

#[test]
fn answers_only_as_localhost_or_an_address() {
    let root = site("dev-host", &[("pages/index.html", "unpublished")]);
    // The rule is the same with `--host` and without it.
    for args in [&[][..], &["--host"]] {
        let server = serve(&root, args);
        let get = |headers: &str| server.request("GET", "/", headers);
        // A phone opens the site by this machine's address, which no other site can point a
        // name at.
        for host in ["192.168.1.20:3000", "10.0.0.5", "[fe80::1]:3000", "[::1]", "localhost:3000"] {
            let page = get(&format!("host: {host}\r\n"));
            assert!(page.status == 200 && page.text().contains("unpublished"), "{host}");
        }
        // A name that another site points at this machine, to read the site through the
        // author's browser.
        for host in ["rebound.example:3000", "phone.local", "localhost.example", "[rebound]", ""] {
            let refused = get(&format!("Host: {host}\r\n"));
            assert!(refused.status == 403 && !refused.text().contains("unpublished"), "{host}");
        }
        assert_eq!(get("").status, 403, "a request that names no host");
        let both = "Host: rebound.example\r\nHost: localhost\r\n";
        assert_eq!(get(both).status, 403, "a second Host header does not pass the first");
        let events = server.request("GET", "/_cinders/events", "Host: rebound.example\r\n");
        assert_eq!(events.status, 403, "the event stream is refused too");
    }
}

#[test]
fn answers_byte_ranges() {
    let server = serve(&fixture(), &[]);
    // A source file, read from disk as it is sent.
    let path = "/_cinders/images/shot.png";
    let whole = server.get(path);
    assert!(whole.head.contains("Content-Type: image/png"), "{}", whole.head);
    assert!(whole.head.contains("Accept-Ranges: bytes"), "{}", whole.head);
    assert_eq!(whole.body, fs::read(fixture().join("images/shot.png")).unwrap());
    let len = whole.body.len();
    let range =
        |range: &str| server.request("GET", path, &format!("{LOCAL}Range: bytes={range}\r\n"));

    let start = range("0-7");
    assert_eq!(start.status, 206);
    assert!(start.head.contains(&format!("Content-Range: bytes 0-7/{len}")), "{}", start.head);
    assert!(start.head.contains("Content-Length: 8"), "{}", start.head);
    assert_eq!(start.body, &whole.body[..8]);
    let rest = range("8-");
    let expected = format!("Content-Range: bytes 8-{}/{len}", len - 1);
    assert!(rest.head.contains(&expected), "{}", rest.head);
    assert_eq!(rest.body, &whole.body[8..]);
    let tail = range("-4");
    let expected = format!("Content-Range: bytes {}-{}/{len}", len - 4, len - 1);
    assert!(tail.head.contains(&expected), "{}", tail.head);
    assert_eq!(tail.body, &whole.body[len - 4..]);
    assert_eq!(range("4-999999").body, &whole.body[4..], "a long range ends with the file");

    let past = range(&format!("{len}-"));
    assert_eq!(past.status, 416);
    assert!(past.head.contains(&format!("Content-Range: bytes */{len}")), "{}", past.head);
    assert!(past.body.is_empty(), "a range past the end gets no bytes");
    assert_eq!(range("0-1,4-5").status, 200, "several ranges get the whole file");
    assert_eq!(range("0-1,4-5").body, whole.body);

    // Bytes the build produced.
    let headers = format!("{LOCAL}range: bytes=0-1\r\n");
    let script = server.request("GET", "/_cinders/scripts/app.js", &headers);
    assert_eq!((script.status, script.text()), (206, "fu"));
    // A range of a page counts the reload script, which is part of what the server sends.
    let page = server.request("GET", "/", &format!("{LOCAL}Range: bytes=-9\r\n"));
    assert_eq!((page.status, page.text()), (206, "</script>"));
    let missing = server.request("GET", "/nope/", &format!("{LOCAL}Range: bytes=0-1\r\n"));
    assert_eq!(missing.status, 404, "only a found file is sent in part");
}

#[test]
fn reloads_after_a_save_and_shows_a_failed_build() {
    let root = site("dev-reload", &[("pages/index.html", "<h1>One</h1>")]);
    let index = root.join("pages/index.html");
    let server = serve(&root, &[]);
    let mut events = server.events();
    let first = next_build(&mut events);
    assert!(server.get("/").text().contains("<h1>One</h1>"));
    let missing = server.get("/nope");
    assert_eq!((missing.status, missing.text()), (404, "not found\n"), "no 404 page to show");

    fs::write(&index, "<h1>Two</h1>").unwrap();
    let (second, page) = wait_for(&server, &mut events, "/", |page| page.text().contains("Two"));
    assert!(second > first, "{second} follows {first}");
    assert!(page.text().contains(&format!(r#"const build="{second}""#)), "{}", page.text());
    let mut late = server.events();
    assert_eq!(next_build(&mut late), second, "a stream opened later starts at the new build");

    fs::write(&index, "<h1>{#if}</h1>").unwrap();
    let (failed, page) = wait_for(&server, &mut events, "/", |page| page.status == 500);
    assert!(
        page.text().contains(">pages/index.html:1: unknown block `{#if}`</pre>"),
        "{}",
        page.text()
    );
    assert!(page.text().contains(&format!(r#"const build="{failed}""#)), "{}", page.text());
    assert_eq!(server.get("/any/path/").body, page.body, "the error replaces every page");

    fs::write(&index, "<h1>Three</h1>").unwrap();
    let (fixed, page) = wait_for(&server, &mut events, "/", |page| page.status == 200);
    assert!(fixed > failed && page.text().contains("Three"), "{}", page.text());
    assert_eq!(next_build(&mut late), failed);
    assert_eq!(next_build(&mut late), fixed, "every stream hears of every build");
}

#[test]
fn rebuilds_for_the_files_a_build_reads() {
    let files = [
        ("pages/index.html", r#"<link rel="stylesheet" href="../site.css"><Mark />"#),
        ("components/Mark.html", "<b>one</b>"),
        ("site.css", "b { margin: 1px }"),
        ("dist/old.html", "an earlier build"),
    ];
    let root = site("dev-watch", &files);
    let server = serve(&root, &[]);
    // macOS also reports the files written just before the watch began, which costs one
    // more build, so the number is read once those reports have passed.
    thread::sleep(Duration::from_millis(300));
    let first = next_build(&mut server.events());

    for path in ["dist/old.html", ".cinders/images/a.avif", ".git/index"] {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), "written by a build").unwrap();
    }
    thread::sleep(Duration::from_millis(300));
    let mut events = server.events();
    assert_eq!(next_build(&mut events), first, "what a build or git writes is no save");
    fs::write(root.join("components/Mark.html"), "<b>two</b>").unwrap();
    let (second, page) = wait_for(&server, &mut events, "/", |page| page.text().contains("two"));
    assert!(second > first, "{second} follows {first}");
    assert!(page.text().contains("<b>two</b>"), "{}", page.text());

    fs::write(root.join("site.css"), "b { margin: 2px }").unwrap();
    let saved = |styles: &Response| styles.text().contains("2px");
    wait_for(&server, &mut events, "/_cinders/site.css", saved);
    fs::write(root.join("cinders.toml"), "nope = 1").unwrap();
    let (_, page) = wait_for(&server, &mut events, "/", |page| page.status == 500);
    assert!(page.text().contains("unknown field `nope`"), "{}", page.text());
    let failed = next_build(&mut server.events());
    assert!(failed > second, "a stream that opens on a failed build hears its number");
}

#[test]
fn runs_the_slow_steps_only_when_optimized() {
    let stale = ".cinders/images/stale.avif";
    let files = [
        (
            "pages/index.html",
            r#"<link rel="stylesheet" href="../site.css"><img src="../badge.png" alt="">"#,
        ),
        ("site.css", "body {\n  margin: 0;\n}\n"),
        (stale, "an encode of an image the site no longer has"),
    ];
    let root = site("dev-fast", &files);
    fs::copy(fixture().join("images/badge.png"), root.join("badge.png")).unwrap();
    let server = serve(&root, &[]);
    let home = server.get("/");
    assert_eq!(first_href(&home), "/_cinders/site.css", "{}", home.text());
    assert!(home.text().contains(r#"<img src="/_cinders/badge.png" alt="">"#), "{}", home.text());
    assert!(server.get("/_cinders/site.css").text().contains("margin: 0"));
    let image = server.get("/_cinders/badge.png");
    assert_eq!(image.body, fs::read(root.join("badge.png")).unwrap(), "the source image");
    let stored = || fs::read_dir(root.join(".cinders/images")).unwrap().count();
    assert_eq!(stored(), 1, "a fast build writes nothing to the store");
    assert!(!root.join("dist").exists(), "the server writes no output");
    drop(server);

    let server = serve(&root, &["--optimized"]);
    let home = server.get("/");
    assert!(home.text().starts_with("<link rel=stylesheet href=/assets/site-"), "{}", home.text());
    let href = first_href(&home);
    assert!(href.ends_with(".css"), "{href}");
    assert_eq!(server.get(href).text(), "body{margin:0}");
    assert_eq!(server.get("/_cinders/site.css").status, 404);
    let src = home.text().split("<img src=").nth(1).unwrap().split(' ').next().unwrap();
    assert!(src.starts_with("/assets/badge-") && src.ends_with(".avif"), "{src}");
    let image = server.get(src);
    assert!(image.head.contains("Content-Type: image/avif"), "{}", image.head);
    assert_eq!(stored(), 3, "the encode and its size join the store, and nothing is pruned");
    assert!(root.join(stale).exists(), "the server never deletes an entry");
    assert!(!root.join("dist").exists(), "the server writes no output");
}

#[test]
fn serves_a_site_under_the_path_of_its_address() {
    let files = [
        (
            "cinders.toml",
            "[navigation]\nclient = false\n[site]\nurl = \"https://example.com/base\"\n",
        ),
        (
            "pages/index.html",
            r#"<link rel="stylesheet" href="../site.css"><a href="/about/">About</a>"#,
        ),
        ("pages/about.html", "about"),
        ("pages/404.html", "lost"),
        ("site.css", "body {\n  margin: 0;\n}\n"),
        ("public/file.txt", "a file"),
    ];
    for (name, args, styles) in [
        ("dev-prefix", &[][..], "/base/_cinders/site.css"),
        ("dev-prefix-optimized", &["--optimized"], "/base/assets/site-"),
    ] {
        let server = serve(&site(name, &files), args);
        // The root leads to the site, and nothing else outside the path is the site's.
        for (path, to) in [("/", "/base/"), ("/base", "/base/"), ("/?x=1", "/base/?x=1")] {
            let root = server.get(path);
            assert_eq!(root.status, 307, "{path}");
            assert!(root.head.ends_with(&format!("Location: {to}")), "{path}: {}", root.head);
        }
        for path in ["/about/", "/file.txt", "/_cinders/site.css", "/basement/"] {
            let outside = server.get(path);
            assert_eq!(outside.status, 404, "{path}");
            assert_eq!(outside.text(), "not found: this site is served under /base/\n", "{path}");
        }
        let home = server.get("/base/");
        assert_eq!(home.status, 200);
        let href = first_href(&home);
        assert!(href.starts_with(styles), "{href}");
        assert!(server.get(href).text().contains("margin:"), "{href}");
        assert!(home.text().contains("/base/about/"), "{}", home.text());
        assert!(home.text().contains(r#"new EventSource("/_cinders/events")"#), "{}", home.text());
        assert_eq!(server.get("/base/about/").text().split('<').next(), Some("about"));
        assert_eq!(server.get("/base/file.txt").text(), "a file");
        let about = server.get("/base/about?x=1");
        assert!(
            about.status == 307 && about.head.contains("Location: /base/about/?x=1"),
            "{}",
            about.head
        );
        let missing = server.get("/base/nope/");
        assert!(missing.status == 404 && missing.text().starts_with("lost"), "{}", missing.text());
        assert!(next_build(&mut server.events()) > 0, "the event stream stays at the root");
    }
}

#[test]
fn a_port_in_use_is_an_error() {
    let root = site("dev-port", &[("pages/index.html", "home")]);
    let server = serve(&root, &[]);
    let port = server.port.to_string();
    let second = Command::new(env!("CARGO_BIN_EXE_cinders"))
        .args(["dev", root.to_str().unwrap(), "--port", &port])
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1), "the second server stops");
    let error = String::from_utf8(second.stderr).unwrap();
    assert!(error.starts_with(&format!("error: cannot listen on port {port}: ")), "{error}");
}

#[test]
fn dev_notes_scripts_missing_data_client_nav() {
    let files = [
        (
            "pages/index.html",
            r#"<script type="module" src="../scripts/app.ts"></script><p>home</p>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("scripts/app.ts", "console.log('hi');"),
    ];
    let root = site("dev-unmarked-note", &files);
    let server = serve(&root, &[]);
    // Lines after `serving ...`: `built in ...`, then `note: ...`.
    let mut output = Vec::new();
    for _ in 0..2 {
        let line = server
            .lines
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("timed out waiting for dev server log: {output:?}"));
        output.push(line);
    }
    let output = output.join("\n");
    assert!(
        output.contains("note: pages with scripts/app.ts will use full page loads"),
        "{output}"
    );
    assert!(output.contains("data-client-nav"), "{output}");
}
