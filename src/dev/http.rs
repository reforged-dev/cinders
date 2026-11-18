//! Answers one request from the current build, and streams build numbers to the pages that
//! wait to reload.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use super::Dev;
use crate::paths;
use crate::site::{Content, Site};

const EVENTS: &str = "/_cinders/events";
const HTML: &str = "text/html; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";
const FAILED: &str = "500 Internal Server Error";
/// How often an idle event stream gets a comment, which is how a closed one is found.
const KEEP_ALIVE: Duration = Duration::from_secs(5);

/// A request, and the connection to answer it on.
struct Request<'a> {
    stream: &'a TcpStream,
    /// Whether the method is HEAD, which gets the headers alone.
    head: bool,
    range: Option<String>,
}

/// Reads one request from `stream` and answers it. Every response closes its connection.
pub(super) fn respond(dev: &Dev, stream: &TcpStream) -> io::Result<()> {
    // Without this, the last short packet of a response waits for the browser's delayed ACK.
    stream.set_nodelay(true)?;
    let mut lines = BufReader::new(stream.take(1 << 16)).lines();
    let line = lines.next().transpose()?.unwrap_or_default();
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let mut request = Request { stream, head: method == "HEAD", range: None };
    // A request whose headers end before a Host, as at a line that is not UTF-8, is refused.
    let mut local = None;
    for header in lines.map_while(Result::ok).take_while(|header| !header.is_empty()) {
        let (name, value) = header.split_once(':').unwrap_or_default();
        if name.eq_ignore_ascii_case("range") {
            request.range = Some(value.trim().to_string());
        } else if name.eq_ignore_ascii_case("host") {
            // One Host header that names another machine refuses the request.
            local = Some(local != Some(false) && is_local(value.trim()));
        }
    }
    // A page on another site could otherwise read this one through a name that it points
    // here. It cannot do so by address, since the address is this server's own origin.
    if local != Some(true) {
        let refused = "open this site as localhost or by its IP address\n";
        return request.send("403 Forbidden", TEXT, "", Cursor::new(refused));
    }
    if method != "GET" && !request.head {
        return request.send("405 Method Not Allowed", TEXT, "Allow: GET, HEAD\r\n", io::empty());
    }
    let (path, query) = target.split_at(target.find('?').unwrap_or(target.len()));
    if path == EVENTS {
        return events(dev, stream);
    }
    let build = dev.current();
    let reload = reload(build.number);
    match &build.site {
        Ok(site) => request.file(site, path, query, &reload),
        Err(error) => {
            let error = format!("{error:#}");
            let page = format!(
                "<!doctype html><meta name=color-scheme content=\"light dark\">\
                 <title>Build error</title><pre style=white-space:pre-wrap>{}</pre>{reload}",
                html_escape::encode_text(&error)
            );
            request.send(FAILED, HTML, "", Cursor::new(page))
        }
    }
}

/// Whether a `Host` header names this machine as `localhost` or by an IP address.
fn is_local(host: &str) -> bool {
    // An IPv6 address is in brackets, and a port may follow either form.
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(host, _port)| host),
    };
    host == "localhost" || host.parse::<IpAddr>().is_ok()
}

impl Request<'_> {
    /// Answers with the file of `site` at the request path `raw`, or with a redirect or the
    /// 404 page. A page gets `reload` after it.
    fn file(&self, site: &Site, raw: &str, query: &str, reload: &str) -> io::Result<()> {
        let redirect = |to: &str| {
            // Temporary, since a browser would keep a permanent one for every site on this port.
            let to = format!("Location: {to}/{query}\r\n");
            self.send("307 Temporary Redirect", TEXT, &to, io::empty())
        };
        // The site is served under the path of `[site] url`, as its host will serve it.
        let path = paths::percent_decode(raw);
        let prefix = paths::percent_decode(&site.prefix);
        let Some(path) = path.strip_prefix(&prefix).and_then(|path| path.strip_prefix('/')) else {
            if path == "/" || path == prefix {
                return redirect(&site.prefix);
            }
            let outside = format!("not found: this site is served under {}/\n", site.prefix);
            return self.send("404 Not Found", TEXT, "", Cursor::new(outside));
        };
        let mut file = path.to_string();
        if file.is_empty() || file.ends_with('/') {
            file.push_str("index.html");
        }
        let mut status = "200 OK";
        if !site.files.contains_key(&file) {
            if site.files.contains_key(&format!("{path}/index.html")) {
                return redirect(raw);
            }
            status = "404 Not Found";
            file = "404.html".to_string();
        }
        let Some(content) = site.files.get(&file) else {
            return self.send(status, TEXT, "", Cursor::new("not found\n"));
        };
        let kind = content_type(&file);
        let html = file.ends_with(".html");
        let sent = match content {
            // A source file is read as it is sent, so a large video is never held in memory.
            Content::File(path) if !html => {
                File::open(path).and_then(|source| self.send(status, kind, "", source))
            }
            _ => content.bytes().map_err(io::Error::other).and_then(|mut page| {
                if html {
                    page.to_mut().extend(reload.bytes());
                }
                self.send(status, kind, "", Cursor::new(&*page))
            }),
        };
        // A source file can go missing between the build and the request.
        sent.or_else(|e| self.send(FAILED, TEXT, "", Cursor::new(format!("{e}\n"))))
    }

    /// Writes a response with `body`, or with the part of it that a `Range` header asks for.
    /// `headers` holds whole header lines.
    fn send(
        &self,
        status: &str,
        kind: &str,
        headers: &str,
        mut body: impl Read + Seek,
    ) -> io::Result<()> {
        let len = body.seek(SeekFrom::End(0))?;
        let range = self.range.as_deref().filter(|_| status == "200 OK");
        let mut status = status;
        let mut start = 0;
        let mut count = len;
        let mut content_range = String::new();
        match range.and_then(|range| bounds(range, len)) {
            None => {}
            Some((first, last)) if first <= last && first < len => {
                status = "206 Partial Content";
                start = first;
                count = last - first + 1;
                content_range = format!("Content-Range: bytes {first}-{last}/{len}\r\n");
            }
            Some(_) => {
                status = "416 Range Not Satisfiable";
                count = 0;
                content_range = format!("Content-Range: bytes */{len}\r\n");
            }
        }
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {count}\r\n\
             Accept-Ranges: bytes\r\nCache-Control: no-cache\r\nConnection: close\r\n\
             {content_range}{headers}\r\n"
        );
        let mut stream = self.stream;
        stream.write_all(response.as_bytes())?;
        if !self.head {
            body.seek(SeekFrom::Start(start))?;
            io::copy(&mut body.take(count), &mut stream)?;
        }
        Ok(())
    }
}

/// The first and last byte that the `Range` header `range` asks for, of `len` bytes. It is
/// `None` for a header to ignore, such as one with several ranges, which gets the whole body.
fn bounds(range: &str, len: u64) -> Option<(u64, u64)> {
    let (first, last) = range.strip_prefix("bytes=")?.split_once('-')?;
    let end = len.saturating_sub(1);
    Some(match (first.trim(), last.trim()) {
        // The last `n` bytes.
        ("", n) => (len.saturating_sub(n.parse().ok()?), end),
        (first, "") => (first.parse().ok()?, end),
        (first, last) => (first.parse().ok()?, last.parse::<u64>().ok()?.min(end)),
    })
}

/// Streams the build number as server-sent events: the current one first, then each new one.
fn events(dev: &Dev, mut stream: &TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
          Connection: close\r\n\r\n",
    )?;
    let mut sent = 0;
    loop {
        let number = dev.wait(sent, KEEP_ALIVE);
        let message = match number == sent {
            true => ":\n\n".to_string(),
            false => format!("data: {number}\n\n"),
        };
        sent = number;
        stream.write_all(message.as_bytes())?;
    }
}

/// The script that reloads a page when the server's build number differs from `build`, the
/// one the page was served from. It holds a stream only while the tab is visible, since
/// browsers allow six connections to an origin.
fn reload(build: u64) -> String {
    format!(
        r#"<script type="module">const build="{build}";let events;
const stop=()=>{{events?.close();events=null}};
const start=()=>{{if(document.hidden)return stop();events??=new EventSource("{EVENTS}");events.onmessage=e=>e.data!=build&&location.reload()}};
document.addEventListener("visibilitychange",start);addEventListener("pagehide",stop);addEventListener("pageshow",start);start()</script>"#
    )
}

/// The content type of a file, by its extension.
fn content_type(file: &str) -> &'static str {
    match paths::extension(Path::new(file)).as_str() {
        "html" => HTML,
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" | "webmanifest" => "application/json",
        "xml" => "application/xml",
        "txt" => TEXT,
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}
