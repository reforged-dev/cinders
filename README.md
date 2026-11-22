# Cinders

Cinders is a no-frills static site generator for lightweight, fast websites. Use it when
you want a highly optimized site without a JavaScript toolchain and packages to maintain.

You write plain HTML, CSS, and TypeScript or JavaScript. Cinders thoroughly optimizes your
site by minifying the pages, stylesheets, and scripts, compressing the images, and
subsetting the fonts. It also checks that every link between pages and every reference to
a file works.

`cinders dev` serves the site while you edit. It has live reload, so your browser shows
each change as soon as you save. Builds take milliseconds, even for a site of a thousand
pages.

Cinders currently works best for landing pages, such as a product page, an event page, or
a portfolio. Support for blogs, documentation sites, and other content-driven websites is
planned.

## What it supports

These features are built in:

- **File-based routing.** Each HTML file in `pages/` becomes a page at the matching path.
  The build includes the stylesheets, scripts, images, and fonts that your pages use.
- **Components and templates.** Write shared markup once as a component, and reuse it on
  any page. Templates can also print values, show markup conditionally, and loop over
  lists.
- **Image optimization.** Cinders re-encodes images in a smaller, modern format, at the
  quality you choose, and scales down any image wider than your limit. It adds a missing
  `width` and `height`, so the layout does not shift as images load.
- **Font subsetting.** Each font keeps only the characters your pages use, so it downloads
  as a smaller file.
- **Stylesheet bundling and minifying.** Cinders joins each stylesheet with the files it
  imports, lowers syntax for the browsers you support, and minifies the result.
- **TypeScript and JavaScript bundling and minifying.** Cinders joins each module with the
  modules it imports into one file, strips the types, and minifies the result.
- **Client-side navigation.** Clicking a link to another page of your site updates the
  page in place instead of reloading it. Shared layouts, images, and UI state stay
  intact.
- **Content hashing.** Each stylesheet, script, image, and font is named by its content,
  so browsers can cache it for a year. Repeat visits load faster and use less bandwidth.
- **`sitemap.xml` and `robots.txt`.** Set your site's address in `cinders.toml`, and
  cinders lists every page in `sitemap.xml` and points `robots.txt` to it.
- **Link and file checks.** Cinders checks every link between your pages and every
  reference to a file, so you find broken links and missing images before you publish.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/reforged-dev/cinders/main/install.sh | sh
```

The script downloads the latest release for your platform, checks it against the
release's checksums, and installs `cinders` to `~/.local/bin`. Releases cover macOS arm64,
Linux x86_64, and Linux aarch64.

To install a specific version, set `CINDERS_VERSION` for the shell that runs the script:

```sh
curl -fsSL https://raw.githubusercontent.com/reforged-dev/cinders/main/install.sh | CINDERS_VERSION=0.1.0 sh
```

## A first site

A site is a directory, and one page is enough.

```text
my-site/
  pages/
    index.html
```

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>Hello</title>
  </head>
  <body>
    <h1>Hello</h1>
  </body>
</html>
```

```sh
cinders dev my-site      # serves http://localhost:3000 and reloads on each save
cinders build my-site    # writes my-site/dist/
```

`dist/` holds the finished site. Upload it to a static host to publish it.

For a larger site, clone this repo and run its example:

```sh
cinders dev examples/landing
```

Each page of the example uses `<Layout>`, a component in `components/Layout.html` that
holds the markup the pages share.

### The config file

Cinders reads `cinders.toml` from the site's root directory. The file and every key in it
are optional.

```toml
browsers = ["chrome 111", "edge 121", "firefox 114", "safari 16.4"]
unchecked = ["/api/"]

[site]
url = "https://example.com"

[navigation]
client = true
prefetch = "hover"

[fonts]
scan = ["data/quotes.json"]

[images]
avif = 59
max_width = 2560
```

| Key                     | Meaning                                                                                                                                                                                                                                                                                                 |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `[site]`                | Values that templates read as `site`. Add any key you need.                                                                                                                                                                                                                                             |
| `[site] url`            | The site's address. Cinders needs it to write `sitemap.xml`, `robots.txt`, and the full URLs of link preview images. If the address has a path, such as `/docs`, cinders writes that path before every path in the site that starts with `/`.                                                           |
| `browsers`              | The oldest browsers you support. Cinders lowers stylesheet syntax for them.                                                                                                                                                                                                                             |
| `unchecked`             | Path prefixes, such as `/api/`. Cinders does not check a reference under one of them.                                                                                                                                                                                                                   |
| `[navigation] client`   | Whether links between pages update the current document in place instead of doing a full page reload. With `true`, cinders adds a small script that fetches the next page and patches only the changed HTML. With `false`, cinders does not add the script and every click triggers a normal page load. |
| `[navigation] prefetch` | When client-side navigation fetches the next page ahead of a click. With `"hover"`, it prefetches when a visitor hovers a link. With `"visible"`, it prefetches when a link scrolls into view.                                                                                                          |
| `[fonts] scan`          | Files with text that a script adds to a page. Cinders keeps their characters in each font.                                                                                                                                                                                                              |
| `[images] avif`         | The AVIF quality, a whole number from 0 to 100.                                                                                                                                                                                                                                                         |
| `[images] max_width`    | The widest an encoded image can be, in pixels. Cinders scales a wider image down.                                                                                                                                                                                                                       |

### Client-side navigation

`data-client-nav` marks a script as written for client-side navigation. A marked script
runs once per document, not once per page view, so its state and listeners carry across
navigations.

```html
<script type="module" src="../scripts/menu.ts" data-client-nav></script>
```

## Development

Cinders builds in a Nix dev shell, which pins the Rust toolchain and provides the tools
that compile the C libraries cinders links. Run every cargo command inside it:

```sh
nix develop -c cargo test
nix develop -c scripts/check.sh
```

`scripts/check.sh` runs every check a change must pass:

- the formatter and the lints
- the tests
- a build of the release binary
- a check that the binary is static on Linux, and on macOS links only system libraries
- a check that `NOTICES.md` is up to date

CI runs the same script on each pull request.

To build the release binary for your machine, run `nix build`. It writes the binary to
`result/bin/cinders`.

## Finding your way around

| Path               | What's in it                                                                                                                                        |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`      | The command line.                                                                                                                                   |
| `src/render/`      | The first stage of a build. It turns the pages and components into plain HTML.                                                                      |
| `src/optimize/`    | The second stage. It processes the stylesheets, scripts, images, and fonts that the HTML references, checks the references, and minifies the pages. |
| `src/dev/`         | `cinders dev`: the file watcher and the HTTP server.                                                                                                |
| `src/store.rs`     | Saves encoded images and font subsets in a site's `.cinders/` directory, so the next build does not encode them again.                              |
| `tests/`           | Most tests build a small site, or the one in `tests/fixtures/site/`, and verify the build output.                                                   |
| `examples/deploy/` | Config files for hosting a site.                                                                                                                    |
| `scripts/`         | The check suite, and the scripts that package a release.                                                                                            |

## License

MIT, see [`LICENSE`](LICENSE). [`NOTICES.md`](NOTICES.md) holds the licenses of the
libraries in the binary.
