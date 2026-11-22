// Client-side navigation: clicks between pages of the site update the current document in place,
// changing only what differs between the served HTML of the two pages. Shared nodes are never
// touched, so script state, event listeners, and loaded media stay intact. Anything unexpected
// falls back to a full page load.
//
// The build prepends `const DEV = ...` and `const VISIBLE = ...` before this file. DEV is true
// in `cinders dev` and logs why a navigation fell back to a full load. VISIBLE is true when
// `prefetch = "visible"`. An optimized build minifies the file, removing log strings and any
// disabled branch.

const REST_MS = 80;
const KEEP_MS = 30_000;
const MATCH_LIMIT = 1_000_000;
const ELEMENT = 1;
const HIDDEN = "position:fixed;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap";
const EXECUTES = /^(module|(text|application)\/(x-)?(java|ecma)script)?$/i;
const STYLES = "link[rel~=stylesheet][href]";
const SHADOW = "template[shadowrootmode]";
const STAYS = /^(HTML|HEAD|BODY)$/;
// Elements whose content is not just child nodes, so they only match if their innerHTML matches too.
const WHOLE = /^(SCRIPT|TEMPLATE)$/;

const me = import.meta.url;
// Root URL of this site (this script lives under `<site>/assets/`).
const site = new URL("..", me).href;
const isRoute = (url) => url.href.startsWith(site) && url.pathname.endsWith("/");
const address = (script) => new URL(script.getAttribute("src"), location.href).href;
const executes = (script) => !script.noModule && EXECUTES.test(script.type.trim());
// Modules and deferred external classic scripts run after non-deferred classic scripts.
const isDeferred = (script) => /^module$/i.test(script.type.trim())
  || (script.defer && !script.hasAttribute("async") && script.hasAttribute("src"));
const key = (script) => (script.hasAttribute("src") ? address(script) : `inline:${script.textContent}`);
const isMine = (script) => script.hasAttribute("src") && address(script) === me;
const scripts = (node) => [...node.querySelectorAll("script")].filter(executes);
// Every page from this build loads this exact script URL.
const mine = (node) => scripts(node).some(isMine);
const pageOf = (href) => href.split("#")[0];
// First executable script on a page that is neither marked `data-client-nav` nor this script.
const unmarked = (node) => scripts(node).find((script) => !script.hasAttribute("data-client-nav") && !isMine(script));
const name = (script) => script.getAttribute("src") ?? "an inline script";
const settled = (element) => new Promise((done) => {
  for (const type of ["load", "error"]) element.addEventListener(type, done);
});

// Parsed root `<html>` of the current page as served, its URL without fragment, and a map from
// each served node to its corresponding live DOM node.
const root = document.documentElement;
let source = null;
let shown = pageOf(location.href);
const live = new WeakMap();
// Keys of scripts that have already run in this document (marked scripts run once).
const ran = new Set();
// In-flight or recently prefetched pages, keyed by URL without fragment.
const fetched = {};
// Maps each same-document history entry id to the served page URL for that entry.
const pageAt = {};
let loading = 0;
let watch = null;

function attributesMatch(a, b) {
  const attrs = a.attributes;
  return attrs.length === b.attributes.length && [...attrs].every(({ name: n, value }) => b.getAttribute(n) === value);
}

// Whether two nodes represent the same node across pages (before comparing their children).
function same(a, b) {
  if (a.nodeType !== b.nodeType) return false;
  if (a.nodeType !== ELEMENT) return a.nodeValue === b.nodeValue;
  const tag = a.tagName;
  if (tag !== b.tagName) return false;
  return STAYS.test(tag) || (attributesMatch(a, b) && (!WHOLE.test(tag) || a.innerHTML === b.innerHTML));
}

// Finds the longest common subsequence of matching child nodes between `a` and `b`.
// Returns ascending `[i, j]` index pairs.
function match(a, b) {
  let start = 0;
  while (start < a.length && start < b.length && same(a[start], b[start])) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && same(a[endA - 1], b[endB - 1])) {
    endA--;
    endB--;
  }
  const pairs = [];
  for (let i = 0; i < start; i++) pairs.push([i, i]);
  const rows = endA - start;
  const columns = endB - start;
  // Skip the O(rows * columns) search for very large differing sections and replace them whole.
  if (rows && columns && rows * columns <= MATCH_LIMIT) {
    // longest[i][j] is the LCS length for a[start + i..endA] and b[start + j..endB].
    const longest = [...Array(rows + 1)].map(() => new Uint32Array(columns + 1));
    for (let i = rows - 1; i >= 0; i--) {
      for (let j = columns - 1; j >= 0; j--) {
        longest[i][j] = same(a[start + i], b[start + j]) ? longest[i + 1][j + 1] + 1 : Math.max(longest[i + 1][j], longest[i][j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < rows && j < columns) {
      if (same(a[start + i], b[start + j])) pairs.push([start + i++, start + j++]);
      else if (longest[i + 1][j] >= longest[i][j + 1]) i++;
      else j++;
    }
  }
  for (let i = endA, j = endB; i < a.length; i++, j++) pairs.push([i, j]);
  return pairs;
}

// Records a 1:1 mapping between corresponding nodes of two structurally identical trees.
function pair(from, to, into) {
  into.set(from, to);
  const a = from.childNodes;
  const b = to.childNodes;
  for (let i = 0; i < a.length; i++) pair(a[i], b[i], into);
}

// Maps each node of the served HTML (`a`) to its live DOM node by matching against `copy`,
// the snapshot taken when this script started. Nodes already mutated by an earlier script will
// not match and will trigger a full load if the next page also tries to change them.
function align(a, copy, copied) {
  live.set(a, copied.get(copy));
  if (a.nodeType !== ELEMENT) return;
  const from = [...a.childNodes];
  const to = [...copy.childNodes];
  // Live `<noscript>` holds raw HTML text, whereas DOMParser re-serializes it via innerHTML.
  const pairs = a.matches("noscript") && from[0] && to.length === 1 ? [[0, 0]] : match(from, to);
  for (const [i, j] of pairs) align(from[i], to[j], copied);
}

// Looks up the live DOM node for a served node, or throws to fall back to a full load.
// Error messages use `DEV && "..."` so minification drops them in optimized builds.
const liveOf = (node) => {
  const found = live.get(node);
  // Missing if an earlier script altered the node before startup, or removed it since.
  if (!found?.isConnected) throw new Error(DEV && "a script changed a node that the next page changes too");
  return found;
};

// Computes the DOM mutations needed to turn served tree `a` into `b` without mutating the
// document yet, so any failure aborts cleanly into a full load.
function plan(a, b, steps) {
  const from = [...a.childNodes];
  const to = [...b.childNodes];
  const pairs = match(from, to);
  const changed = pairs.length < from.length || pairs.length < to.length;
  // Only `STAYS` elements (html, head, body) can match while having different attributes.
  const attrs = STAYS.test(a.tagName) && !attributesMatch(a, b);
  if (changed || attrs) {
    const node = liveOf(a);
    if (attrs) {
      steps.push(() => {
        for (const { name: n } of a.attributes) if (!b.hasAttribute(n)) node.removeAttribute(n);
        for (const { name: n, value } of b.attributes) if (a.getAttribute(n) !== value) node.setAttribute(n, value);
      });
    }
    if (changed) {
      // Walk the gaps between matched pairs to collect removed nodes and runs of new nodes.
      const removed = [];
      const runs = [];
      let i = 0;
      let j = 0;
      for (const [nextI, nextJ] of [...pairs, [from.length, to.length]]) {
        for (; i < nextI; i++) removed.push(liveOf(from[i]));
        if (nextJ > j) runs.push([to.slice(j, nextJ).map(adopt), nextI < from.length ? liveOf(from[nextI]) : null]);
        i = nextI + 1;
        j = nextJ + 1;
      }
      steps.push(() => {
        for (const old of removed) old.remove();
        for (const [nodes, before] of runs) {
          // If a script reparented the anchor node, append at the end instead.
          const anchor = before?.parentNode === node ? before : null;
          for (const fresh of nodes) node.insertBefore(fresh, anchor);
        }
      });
    }
  }
  for (const [i, j] of pairs) {
    const kept = live.get(from[i]);
    if (kept) live.set(to[j], kept);
    if (from[i].nodeType === ELEMENT) plan(from[i], to[j], steps);
  }
}

// Clones a node from the next page into this document and maps every node in the subtree.
function adopt(node) {
  const copy = document.importNode(node, true);
  // Declarative shadow roots only attach during HTML parsing; importNode leaves them inert.
  if (copy.nodeType === ELEMENT && (copy.matches(SHADOW) || copy.querySelector(SHADOW))) {
    throw new Error(DEV && "the next page adds a declarative shadow root");
  }
  pair(node, copy, live);
  return copy;
}

// Executes newly added scripts in full-load order: non-deferred classic scripts in document order
// (awaiting each external non-async script before the next), followed by deferred classic scripts
// and modules in document order. Scripts cloned via importNode are inert, so each is replaced with
// a fresh `<script>` element. While `busy` is true, any new navigation falls back to a full load
// so pending scripts do not run on the wrong page.
let busy = false;
async function run(fresh) {
  busy = true;
  try {
    for (const original of fresh.sort((a, b) => isDeferred(a) - isDeferred(b))) {
      const inert = live.get(original);
      const id = key(original);
      // Skip duplicates on the same page or scripts removed by an earlier script.
      if (!inert?.isConnected || ran.has(id)) continue;
      ran.add(id);
      const script = document.createElement("script");
      for (const { name: n, value } of inert.attributes) script.setAttribute(n, value);
      script.textContent = inert.textContent;
      live.set(original, script);
      // Use addEventListener so any `onload` attribute on the tag still runs.
      const loaded = inert.getAttribute("src")?.trim() && !inert.hasAttribute("async") && settled(script);
      inert.replaceWith(script);
      if (loaded) await loaded;
    }
  } finally {
    busy = false;
  }
}

// Fetches and parses a page, caching the promise briefly to reuse prefetches. Returns null on network error.
function load(href, options) {
  const url = pageOf(href);
  if (fetched[url]) return fetched[url];
  // Do not follow redirects; let the full-load fallback handle them.
  const page = fetch(url, { redirect: "manual", ...options })
    .then(async (response) => {
      if (!response.headers.get("content-type")?.includes("text/html")) return {};
      const doc = new DOMParser().parseFromString(await response.text(), "text/html");
      // DOMParser parses `<noscript>` children as elements because scripting is disabled in its
      // document; convert them back to raw text so they do not load or apply when adopted.
      for (const off of doc.querySelectorAll("noscript")) off.textContent = off.innerHTML;
      return { doc, ok: response.ok };
    })
    .catch(() => {
      delete fetched[url];
      return null;
    });
  fetched[url] = page;
  // Expire unused prefetches so a later visit fetches a fresh copy.
  setTimeout(() => fetched[url] === page && delete fetched[url], KEEP_MS);
  return page;
}

// Lazily fetches the initial page's served HTML and aligns it with the live DOM (set by `start`).
let prepare = null;

// Preloads stylesheets new to `next` before updating the DOM to prevent a flash of unstyled content.
function stylesReady(next) {
  const applies = (link) => !link.media || matchMedia(link.media).matches;
  const have = new Set([...document.querySelectorAll(STYLES)].filter(applies).map((link) => link.href));
  const waits = [];
  for (const link of next.querySelectorAll(STYLES)) {
    if (!applies(link)) continue;
    const href = new URL(link.getAttribute("href"), location.href).href;
    if (have.has(href)) continue;
    have.add(href);
    const preload = document.createElement("link");
    preload.rel = "preload";
    preload.as = "style";
    preload.href = href;
    if (link.crossOrigin) preload.crossOrigin = link.crossOrigin;
    const done = settled(preload).then(() => preload.remove());
    document.head.append(preload);
    waits.push(done);
  }
  return Promise.all(waits);
}

// Announces the new page title to screen readers via a visually hidden live region.
let announcer = null;
let announceTimer = 0;
function announce() {
  if (!announcer?.isConnected) {
    announcer = document.createElement("div");
    announcer.setAttribute("aria-live", "assertive");
    announcer.setAttribute("aria-atomic", "true");
    announcer.setAttribute("style", HIDDEN);
    document.body.append(announcer);
  }
  clearTimeout(announceTimer);
  announcer.textContent = "";
  // Screen readers only announce changes to an existing live region, so set the title after a delay.
  announceTimer = setTimeout(() => (announcer.textContent = document.title), 60);
}

// Reloads the current URL (already updated by the Navigation API) and never settles,
// keeping the browser's loading indicator active until the full load takes over.
function fullLoad(why) {
  if (DEV) console.info(`[cinders] full load: ${why}`);
  location.reload();
  return new Promise(() => {});
}

// Transitions the document to `href`. Aborts without touching the DOM if `signal` fires first.
async function go(href, signal) {
  const [known, page] = await Promise.all([prepare(), load(href)]);
  if (signal.aborted) return;
  if (!known) return fullLoad(DEV && "this page could not be read again");
  if (!page) return fullLoad(DEV && "the request failed");
  if (!page.ok) return fullLoad(DEV && "the answer is an error, a redirect, or not a page");
  if (!mine(page.doc)) return fullLoad(DEV && "the page is not from this build of the site");
  const blocker = unmarked(page.doc);
  if (blocker) return fullLoad(DEV && `${name(blocker)} on the next page has no data-client-nav`);
  await stylesReady(page.doc);
  if (signal.aborted) return;
  if (busy) return fullLoad(DEV && "a script of this page is still loading");
  // Everything below is synchronous so another navigation cannot observe a half-updated DOM.
  delete fetched[pageOf(href)];
  const steps = [];
  const nextRoot = page.doc.documentElement;
  // `plan` maps matched children, so map the root `<html>` element explicitly.
  live.set(nextRoot, root);
  plan(source, nextRoot, steps);
  for (const step of steps) step();
  source = nextRoot;
  shown = pageOf(href);
  announce();
  if (VISIBLE) watch();
  run(scripts(page.doc));
}

function start() {
  const blocker = unmarked(document);
  if (blocker) {
    if (DEV) console.info(`[cinders] full loads from this page: ${name(blocker)} has no data-client-nav`);
    return;
  }
  for (const script of scripts(document)) ran.add(key(script));
  // Snapshot the DOM at startup so `prepare` can match the served HTML against it later,
  // even after other scripts mutate the page.
  let copy = root.cloneNode(true);
  let copied = new WeakMap();
  pair(copy, root, copied);
  // Capture the initial URL now, since a navigation updates `location` before `prepare` runs.
  const loaded = shown;
  let ready = null;
  prepare = () => {
    ready ??= (async () => {
      // Read the initial page from the browser cache to avoid a second network request.
      const page = await load(loaded, { cache: "force-cache" });
      delete fetched[loaded];
      // Verify the cached response is from this build and does not hold an unmarked script.
      const found = !!page?.doc && mine(page.doc) && !unmarked(page.doc);
      if (found) {
        source = page.doc.documentElement;
        align(source, copy, copied);
        // Record scripts from the served HTML too, in case one removed itself after running.
        for (const script of scripts(page.doc)) ran.add(key(script));
      }
      copy = copied = null;
      return found;
    })();
    return ready;
  };

  navigation.addEventListener("navigate", (event) => {
    if (!event.canIntercept || event.downloadRequest !== null) return;
    if (event.formData || event.navigationType === "reload") return;
    const dest = event.destination;
    let traverse = false;
    // Let the browser handle same-page hash jumps, script pushState/replaceState calls, and
    // traversals between entries that belong to the currently shown page.
    if (dest.sameDocument) {
      traverse = event.navigationType === "traverse";
      if (traverse ? (pageAt[dest.id] ?? shown) === shown : !loading) return;
    }
    const url = new URL(dest.url);
    const route = isRoute(url);
    if (!route && !traverse) return;
    // Tag every entry created while on `shown` before the browser adds the new entry.
    for (const entry of navigation.entries()) pageAt[entry.id] ??= shown;
    event.intercept({
      handler: () => {
        if (!route) return fullLoad(DEV && "the entry is not a route");
        pageAt[navigation.currentEntry.id] = pageOf(url.href);
        loading++;
        return go(url.href, event.signal)
          .catch((error) => fullLoad(DEV && error.message))
          .then(() => loading--);
      },
    });
  });

  // Extract a prefetchable same-site URL from an interaction event or IntersectionObserver entry.
  const intent = (event) => {
    const a = event.target.closest?.("a[href]");
    if (!a || a.hasAttribute("download") || (a.target && a.target !== "_self")) return null;
    return isRoute(a) && pageOf(a.href) !== pageOf(location.href) ? a.href : null;
  };
  const ahead = (href, options) => {
    if (!href || navigator.connection?.saveData) return;
    prepare();
    load(href, options);
  };
  let rest = 0;
  addEventListener("mouseover", (event) => {
    clearTimeout(rest);
    const href = intent(event);
    if (href) rest = setTimeout(ahead, REST_MS, href);
  });
  addEventListener("focusin", (event) => ahead(intent(event)));
  addEventListener("touchstart", (event) => ahead(intent(event)), { passive: true });

  if (VISIBLE) {
    // Prefetch links that enter the viewport at low priority so they do not compete with page resources.
    const inView = new IntersectionObserver((entries) => {
      for (const entry of entries) if (entry.isIntersecting) {
        inView.unobserve(entry.target);
        ahead(intent(entry), { priority: "low" });
      }
    });
    // Re-observe after each navigation so persistent links (e.g. nav links to the previous page) are re-evaluated.
    watch = () => {
      inView.disconnect();
      for (const a of document.links) inView.observe(a);
    };
    watch();
  }
}

if (window.navigation) start();
