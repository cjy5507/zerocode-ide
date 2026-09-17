/* The observation ring, planted at document start and driven for real.
 *
 * `ui/browser-ring.js` shares a private state slot with the menu (docs/design/
 * browser-door-for-agents.md §2.1). Rust reads it by polling `take`; this
 * file is what proves the ring observes without changing the page: the five
 * console levels, fetch ok/4xx/network error, XHR, PerformanceObserver
 * resources, window errors and unhandled rejections, cap overflow, the
 * `since` cursor, page-side scrubbing, main frame only, idempotence.
 *
 *   node ui/tests/browser-ring.mjs
 *
 * Playwright's `addInitScript` is the same moment as wry's
 * `initialization_script` — after the global object exists, before the
 * document parses — so the ring meets the page here the way it does in the
 * pane.
 */

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./playwright-chromium.mjs";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CLOAK_SOURCE = await readFile(resolve(UI, "browser-cloak.js"), "utf8").catch(() => "");
const RUNTIME_SOURCE = await readFile(resolve(UI, "../crates/zerocode-shell/src/browser_runtime.rs"), "utf8");
const MENU_SOURCE = RUNTIME_SOURCE.match(/const BROWSER_MENU_JS: &str = r##"([\s\S]*?)"##;/)[1];
const GUEST_KEY = (await readFile(resolve(UI, "browser-guest-key.txt"), "utf8")).trim();
const RING_SOURCE = (await readFile(resolve(UI, "browser-ring.js"), "utf8")).replaceAll("__GUEST_STATE__", GUEST_KEY);


/* The caps the ring keeps, read from the ring's own table rather than typed
 * here a second time: the number that matters is the one the page enforces. */
const CAPS = Object.fromEntries(
  [...RING_SOURCE.matchAll(/^\s+(console|network|errors|text|url|budget):\s*([\d_]+),/gm)]
    .map(([, name, value]) => [name, Number(value.replace(/_/g, ""))]),
);

const PNG_1X1 = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

const site = createServer((request, response) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  switch (url.pathname) {
    case "/":
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end("<!doctype html><title>ring fixture</title><main id=main>fixture</main>");
      return;
    case "/frame":
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      response.end("<!doctype html><p>frame</p>");
      return;
    case "/ok":
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ answered: true }));
      return;
    case "/img-ok.png":
      response.writeHead(200, { "content-type": "image/png" });
      response.end(PNG_1X1);
      return;
    case "/img-missing.png":
    case "/missing":
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("nothing here");
      return;
    default:
      response.writeHead(500, { "content-type": "text/plain" });
      response.end("boom");
  }
});
await new Promise((done) => site.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${site.address().port}`;

const results = [];
const pass = (name, detail = "") => results.push({ name, pass: true, detail });
const fail = (name, detail) => results.push({ name, pass: false, detail: String(detail) });

async function test(name, run) {
  try {
    const detail = await run();
    pass(name, detail ?? "");
  } catch (error) {
    fail(name, error?.stack ?? error);
  }
}

function assert(condition, message, detail = undefined) {
  if (!condition) {
    throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
  }
}

const browser = await chromium.launch();
const context = await browser.newContext();
// Every frame gets the script, as wry hands it to every frame on Windows;
// the ring's own guard is what keeps it to the main frame.
await context.addInitScript(RING_SOURCE);
const page = await context.newPage();
const consoleSeen = [];
page.on("console", (message) => consoleSeen.push({ type: message.type(), text: message.text() }));
const faults = [];
page.on("pageerror", (error) => faults.push(String(error)));
await page.goto(`${origin}/`);

await test("the cloak restores native Notification and removes configurable Tauri globals at document start", async () => {
  const isolated = await browser.newContext();
  try {
    // Tauri runs before builder scripts. Only the main frame is injected on macOS.
    await isolated.addInitScript(`(() => {
      if (window.top !== window) return;
      window.__TAURI__ = {};
      window.__TAURI_PLUGIN_TEST__ = {};
      window.Notification = function Notification() { throw new Error("plugin IPC"); };
    })();\n${CLOAK_SOURCE}`);
    const guest = await isolated.newPage();
    await guest.goto(`${origin}/`);
    const seen = await guest.evaluate(() => ({
      tauri: Object.getOwnPropertyNames(window).filter(name => /^__TAURI/.test(name)),
      notification: Function.prototype.toString.call(Notification),
      permission: typeof Notification.requestPermission,
      frames: document.querySelectorAll("iframe").length,
    }));
    assert(seen.tauri.length === 0, "Tauri globals remain", seen);
    assert(seen.notification.includes("[native code]") && seen.permission === "function", "Notification is still a plugin shim", seen);
    assert(seen.frames === 0, "the cloak left its temporary frame", seen);
  } finally { await isolated.close(); }
});

const take = (kind, since = 0, options = {}) =>
  page.evaluate(
    ([key, kind, since, options]) => window[key].ring.take(kind, since, options),
    [GUEST_KEY, kind, since, options],
  );
const settle = (ms = 80) => page.waitForTimeout(ms);

await test("the ring is one frozen global with a caps table and the document's own status", async () => {
  const seen = await page.evaluate((key) => ({
    present: typeof window[key].ring === "object",
    frozen: Object.isFrozen(window[key].ring),
    globals: Object.keys(window).filter((name) => name.startsWith("__zerocode")),
    caps: window[key].ring.caps,
    document: window[key].ring.document,
    enumerable: Object.getOwnPropertyDescriptor(window, key).enumerable,
  }), GUEST_KEY);
  assert(seen.present && seen.frozen, "no frozen ring", seen);
  assert(seen.globals.length === 0 && !seen.enumerable, "observation globals are enumerable", seen);
  assert(seen.caps.console === CAPS.console && seen.caps.network === CAPS.network && seen.caps.errors === CAPS.errors, "caps table drifted", seen.caps);
  assert(seen.document.status === 200 && seen.document.type === "navigate", "document facts missing", seen.document);
  assert(seen.document.url === `${origin}/`, "document url missing", seen.document);
  return JSON.stringify(seen.caps);
});

await test("observation preserves native function surfaces and forwards receiver, arguments and failures", async () => {
  const seen = await page.evaluate(() => {
    const frame = document.createElement("iframe");
    document.body.appendChild(frame);
    const native = frame.contentWindow;
    const pairs = [[fetch, native.fetch], [XMLHttpRequest.prototype.open, native.XMLHttpRequest.prototype.open],
      [XMLHttpRequest.prototype.send, native.XMLHttpRequest.prototype.send],
      ...["log", "info", "warn", "error", "debug"].map(level => [console[level], native.console[level]])];
    const surfaces = pairs.map(([wrapped, original]) => ({
      native: Function.prototype.toString.call(wrapped).includes("[native code]"),
      name: wrapped.name === original.name, length: wrapped.length === original.length,
      own: JSON.stringify(Reflect.ownKeys(wrapped)) === JSON.stringify(Reflect.ownKeys(original)),
    }));
    let illegalReceiver = false;
    try { Reflect.apply(XMLHttpRequest.prototype.open, {}, ["GET", "/ok"]); }
    catch (error) { illegalReceiver = error instanceof TypeError; }
    frame.remove();
    return { surfaces, illegalReceiver };
  });
  assert(seen.surfaces.every(row => row.native && row.name && row.length && row.own), "wrapper changed a native surface", seen);
  assert(seen.illegalReceiver, "XHR no longer rejects an illegal receiver", seen);
});

await test("the guest exposes no enumerable observation or Tauri globals", async () => {
  const seen = await page.evaluate(() => Object.keys(window).filter(name => /__zerocode|__TAURI/.test(name)));
  assert(seen.length === 0, "guest globals are enumerable", seen);
});

await test("the context menu shares the hidden ring slot and installs only one listener", async () => {
  const seen = await page.evaluate(([key, source]) => {
    const before = Object.getOwnPropertyNames(window);
    const state = window[key];
    let listeners = 0;
    const add = document.addEventListener;
    document.addEventListener = function (...args) {
      if (args[0] === "contextmenu") listeners += 1;
      return Reflect.apply(add, this, args);
    };
    try {
      (0, eval)(source);
      (0, eval)(source);
    } finally { document.addEventListener = add; }
    const link = document.createElement("a");
    link.href = "/ok";
    document.body.appendChild(link);
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 12, clientY: 34 });
    link.dispatchEvent(event);
    const menu = state.menu;
    state.menu = null;
    link.remove();
    return { menu, cleared: state.menu === null, sameRing: state.ring === window[key].ring,
      added: Object.getOwnPropertyNames(window).filter(name => !before.includes(name)),
      prevented: event.defaultPrevented, listeners };
  }, [GUEST_KEY, MENU_SOURCE.replaceAll("__GUEST_STATE__", GUEST_KEY)]);
  assert(seen.listeners === 1, "the menu installed duplicate listeners", seen);
  assert(seen.added.length === 0 && seen.sameRing && seen.cleared, "menu leaked a global or lost the ring", seen);
  assert(seen.menu.x === 12 && seen.menu.y === 34 && seen.menu.linkUrl === `${origin}/ok` && seen.prevented, "menu facts changed", seen);
});

await test("the five console levels are observed and the originals still speak", async () => {
  consoleSeen.length = 0;
  await page.evaluate(() => {
    console.log("ring-log");
    console.info("ring-info");
    console.warn("ring-warn");
    console.error("ring-error");
    console.debug("ring-debug");
  });
  const held = await take("console");
  const levels = held.entries.map((entry) => [entry.level, entry.text]);
  assert(
    JSON.stringify(levels) === JSON.stringify([
      ["log", "ring-log"], ["info", "ring-info"], ["warn", "ring-warn"], ["error", "ring-error"], ["debug", "ring-debug"],
    ]),
    "levels or texts drifted",
    levels,
  );
  assert(held.entries.every((entry, at) => at === 0 || entry.seq > held.entries[at - 1].seq), "seq is not monotonic", held.entries);
  assert(held.entries.every((entry) => typeof entry.at === "number" && entry.at > 0), "no clock on the entries");
  // The originals still ran — Playwright hears the real console.
  const spoken = consoleSeen.map((one) => one.text);
  assert(["ring-log", "ring-info", "ring-warn", "ring-error", "ring-debug"].every((word) => spoken.includes(word)), "a wrapped level swallowed the original", spoken);
  // Objects are formatted; a sensitive key is redacted by the same table.
  await page.evaluate(() => console.log("shape", { ok: 1, password: "hunter2", nested: { apiKey: "k" } }, new Error("named")));
  const [last] = (await take("console", held.seq)).entries;
  assert(last.text.includes("shape") && last.text.includes('"ok":1') && last.text.includes("Error: named"), "objects are not formatted", last.text);
  assert(!last.text.includes("hunter2") && last.text.includes("[redacted]") && !last.text.includes('"k"'), "a sensitive key crossed", last.text);
  return `${held.entries.length} entries`;
});

await test("level filters: error alone, warn with error, all", async () => {
  const all = await take("console", 0, { level: "all" });
  const warn = await take("console", 0, { level: "warn" });
  const error = await take("console", 0, { level: "error" });
  assert(all.entries.length >= 6, "all lost entries", all.entries.length);
  assert(warn.entries.every((entry) => entry.level === "warn" || entry.level === "error") && warn.entries.length === 2, "warn filter drifted", warn.entries);
  assert(error.entries.length === 1 && error.entries[0].level === "error", "error filter drifted", error.entries);
});

await test("fetch: ok, 4xx and a network error are observed, and the page still gets the real answer", async () => {
  const before = (await take("network")).seq;
  const seen = await page.evaluate(async (origin) => {
    const out = {};
    const ok = await fetch(`${origin}/ok`);
    out.okJson = await ok.json();
    out.okIsResponse = ok instanceof Response;
    const missing = await fetch(`${origin}/missing`, { method: "POST", headers: { authorization: "Bearer never-recorded" } });
    out.missingStatus = missing.status;
    try {
      await fetch("http://127.0.0.1:1/refused");
      out.refusedThrew = false;
    } catch (error) {
      out.refusedThrew = true;
      out.refusedName = error.name;
    }
    return out;
  }, origin);
  assert(seen.okJson?.answered === true && seen.okIsResponse, "fetch no longer returns the real Response", seen);
  assert(seen.missingStatus === 404 && seen.refusedThrew, "fetch behaviour changed", seen);
  await settle();
  const held = await take("network", before);
  const fetched = held.entries.filter((entry) => entry.kind === "fetch");
  assert(fetched.length === 3, "three fetches expected", held.entries);
  const [ok, missing, refused] = fetched;
  assert(ok.method === "GET" && ok.status === 200 && ok.ok === true && ok.url === `${origin}/ok` && typeof ok.ms === "number", "ok fetch row", ok);
  assert(missing.method === "POST" && missing.status === 404 && missing.ok === false, "404 fetch row", missing);
  assert(refused.status === 0 && refused.ok === false && typeof refused.error === "string" && refused.error.length > 0, "network error row", refused);
  const dumped = JSON.stringify(held);
  assert(!dumped.includes("never-recorded") && !dumped.includes("authorization"), "a header crossed into the ring", dumped);
  assert(!("headers" in ok) && !("body" in ok), "the row has a headers or body slot", Object.keys(ok));
  const failed = await take("network", before, { failed: true });
  assert(failed.entries.every((entry) => entry.ok === false) && failed.entries.length === 2, "failed filter drifted", failed.entries);
});

await test("XHR is observed through its own loadend, and the page's handlers still run", async () => {
  const before = (await take("network")).seq;
  const seen = await page.evaluate((origin) => new Promise((done) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", `${origin}/missing?sessionId=abc&keep=1`);
    xhr.onload = () => done({ status: xhr.status, text: xhr.responseText });
    xhr.send("payload-never-recorded");
  }), origin);
  assert(seen.status === 404 && seen.text === "nothing here", "XHR behaviour changed", seen);
  await settle();
  const held = await take("network", before);
  const row = held.entries.find((entry) => entry.kind === "xhr");
  assert(row && row.method === "PUT" && row.status === 404 && row.ok === false, "XHR row", held.entries);
  assert(row.url === `${origin}/missing?sessionId=%5Bredacted%5D&keep=1`, "XHR url not scrubbed", row.url);
  assert(!JSON.stringify(held).includes("payload-never-recorded"), "a body crossed into the ring");
});

await test("images and other resources arrive by PerformanceObserver with their status", async () => {
  const before = (await take("network")).seq;
  await page.evaluate((origin) => new Promise((done) => {
    let left = 2;
    for (const name of ["img-ok.png", "img-missing.png"]) {
      const img = new Image();
      img.onload = img.onerror = () => { if (--left === 0) done(); };
      img.src = `${origin}/${name}?token=t0ken&cache=${Date.now()}`;
      document.body.append(img);
    }
  }), origin);
  await settle(250);
  const held = await take("network", before);
  const imgs = held.entries.filter((entry) => entry.kind === "img");
  assert(imgs.length === 2, "two image rows expected", held.entries);
  const ok = imgs.find((entry) => entry.url.includes("img-ok"));
  const missing = imgs.find((entry) => entry.url.includes("img-missing"));
  assert(ok && ok.status === 200 && ok.ok === true && ok.method === "GET", "ok image row", ok);
  assert(missing && missing.status === 404 && missing.ok === false, "missing image row", missing);
  assert(imgs.every((entry) => entry.url.includes("token=%5Bredacted%5D")), "resource url not scrubbed", imgs);
});

await test("uncaught errors and unhandled rejections land in the errors ring", async () => {
  const before = (await take("errors")).seq;
  await page.evaluate(() => {
    setTimeout(() => { throw new Error("boom sk-abcdefghijklmnop"); }, 0);
    Promise.reject(new Error("later"));
    Promise.reject("bare-reason");
  });
  await settle(150);
  const held = await take("errors", before);
  const texts = held.entries.map((entry) => entry.text);
  assert(texts.some((text) => text.includes("boom")), "the thrown error is missing", texts);
  assert(texts.some((text) => text.includes("later")) && texts.some((text) => text.includes("bare-reason")), "a rejection is missing", texts);
  assert(!texts.some((text) => text.includes("sk-abcdefghijklmnop")) && texts.some((text) => text.includes("[redacted]")), "a token crossed in an error", texts);
  const thrown = held.entries.find((entry) => entry.text.includes("boom"));
  assert(typeof thrown.line === "number" && typeof thrown.source === "string", "the error lost its place", thrown);
  faults.length = 0;
});

await test("the cap holds: the newest stay, the oldest go, seq keeps counting", async () => {
  const cap = CAPS.console;
  const seen = await page.evaluate(([key, cap]) => {
    const first = window[key].ring.take("console", 0, { budget: 1e9 }).seq;
    for (let n = 0; n < cap + 100; n += 1) console.log(`flood-${n}`);
    const held = window[key].ring.take("console", 0, { budget: 1e9 });
    return {
      first,
      count: held.entries.length,
      firstText: held.entries[0].text,
      lastText: held.entries[held.entries.length - 1].text,
      seq: held.seq,
    };
  }, [GUEST_KEY, cap]);
  assert(seen.count === cap, "the ring holds more than its cap", seen);
  assert(seen.firstText === "flood-100" && seen.lastText === `flood-${cap + 99}`, "the wrong end was dropped", seen);
  assert(seen.seq === seen.first + cap + 100, "seq was reset by the overflow", seen);
});

await test("the since cursor reads forward without emptying, and a budget says `more`", async () => {
  const all = await take("console", 0, { budget: 1e9 });
  const last = all.entries[all.entries.length - 1].seq;
  const nothing = await take("console", last);
  assert(nothing.entries.length === 0 && nothing.seq === last && nothing.last === last, "since=last still answers rows", nothing);
  await page.evaluate(() => console.log("after-cursor"));
  const next = await take("console", last);
  assert(next.entries.length === 1 && next.entries[0].text === "after-cursor" && next.entries[0].seq === last + 1, "the cursor lost the new row", next);
  const again = await take("console", 0, { budget: 1e9 });
  assert(again.entries.length === CAPS.console, "take emptied the ring", again.entries.length);
  const slice = await take("console", 0, { budget: 600 });
  assert(slice.more === true && slice.entries.length > 0 && slice.entries.length < CAPS.console, "the budget did not cut", { more: slice.more, n: slice.entries.length });
  assert(slice.last === slice.entries[slice.entries.length - 1].seq, "`last` is not the last returned seq", slice);
  const rest = await take("console", slice.last, { budget: 1e9 });
  assert(rest.entries.length + slice.entries.length === CAPS.console, "the cursor after a cut skips rows", { rest: rest.entries.length, slice: slice.entries.length });
});

await test("scrubbing: token shapes, userinfo and sensitive query keys never cross", async () => {
  const before = (await take("console")).seq;
  await page.evaluate((origin) => {
    console.log("Authorization: Bearer abcdefghijklmnopqrstuvwxyz");
    console.log("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c end");
    console.log("gh ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 and password=hunter2 and api_key: k-123 stays? no");
    console.log(`${origin}/ok?token=xyz&q=docs`);
    console.log("http://user:pw@host.example/path#access_token=abc&state=kept");
  }, origin);
  const held = await take("console", before);
  const texts = held.entries.map((entry) => entry.text);
  const dumped = texts.join("\n");
  for (const secret of ["abcdefghijklmnopqrstuvwxyz", "eyJhbGciOiJIUzI1NiJ9", "ghp_ABCDEF", "hunter2", "k-123", "token=xyz", "user:pw", "access_token=abc"]) {
    assert(!dumped.includes(secret), `a secret crossed: ${secret}`, texts);
  }
  assert(texts[0] === "Authorization: [redacted]" || texts[0].startsWith("Authorization: [redacted]"), "bearer word", texts[0]);
  assert(texts[3] === `${origin}/ok?token=%5Bredacted%5D&q=docs`, "url query", texts[3]);
  assert(texts[4] === "http://host.example/path#access_token=%5Bredacted%5D&state=kept", "userinfo and hash", texts[4]);
  assert(texts[2].includes("password=[redacted]") && texts[2].includes("api_key: [redacted]"), "key=value pairs", texts[2]);
  // Long lines are cut at the text cap.
  await page.evaluate((cap) => console.log("x".repeat(cap * 3)), CAPS.text);
  const [long] = (await take("console", held.seq)).entries;
  assert(long.text.length === CAPS.text, "the text cap is not applied", long.text.length);
});

await test("a child frame gets no ring — the main frame only", async () => {
  await page.evaluate((origin) => new Promise((done) => {
    const frame = document.createElement("iframe");
    frame.id = "child";
    frame.onload = () => done();
    frame.src = `${origin}/frame`;
    document.body.append(frame);
  }), origin);
  const child = page.frames().find((frame) => frame.url().endsWith("/frame"));
  assert(child, "the child frame did not load");
  const seen = await child.evaluate((key) => ({
    ring: typeof window[key]?.ring,
    top: window.top === window,
  }), GUEST_KEY);
  assert(seen.ring === "undefined" && seen.top === false, "the ring ran in a child frame", seen);
});

await test("planting the ring twice changes nothing — idempotent, no double wrapping", async () => {
  const seen = await page.evaluate(async ([key, source]) => {
    const before = window[key].ring;
    const fetchBefore = window.fetch;
    const logBefore = console.log;
    (0, eval)(source);
    const seqBefore = window[key].ring.take("console", 0).seq;
    console.log("once");
    const after = window[key].ring.take("console", seqBefore);
    return {
      same: window[key].ring === before,
      sameFetch: window.fetch === fetchBefore,
      sameLog: console.log === logBefore,
      entries: after.entries.length,
    };
  }, [GUEST_KEY, RING_SOURCE]);
  assert(seen.same && seen.sameFetch && seen.sameLog, "the second planting replaced the wrappers", seen);
  assert(seen.entries === 1, "one log became more than one entry", seen);
});

await test("an unknown kind answers empty rather than throwing", async () => {
  const seen = await page.evaluate((key) => {
    try {
      const held = window[key].ring.take("cookies", 0);
      return { threw: false, entries: held.entries.length };
    } catch (error) {
      return { threw: true, error: String(error) };
    }
  }, GUEST_KEY);
  assert(seen.threw === false && seen.entries === 0, "an unknown kind throws or answers rows", seen);
});

await test("the page raised no error of the ring's own making", async () => {
  assert(faults.length === 0, "page errors", faults);
});

await test("a fresh Chromium protocol session enables Page before document-start observation", async () => {
  for (const enable of [false, true]) {
    const context = await browser.newContext();
    try {
      const guest = await context.newPage();
      const client = await context.newCDPSession(guest);
      if (enable) await client.send("Page.enable");
      await client.send("Page.addScriptToEvaluateOnNewDocument", { source: RING_SOURCE });
      await client.send("Page.navigate", { url: `${origin}/?init=${enable}` });
      await guest.waitForFunction(() => location.search.startsWith("?init=") && document.readyState === "complete");
      const ready = await guest.evaluate((key) => typeof window[key]?.ring?.take === "function", GUEST_KEY);
      assert(ready === enable, "document-start observation requires Page enablement", { enable, ready });
      if (enable) {
        await guest.evaluate(() => console.error("owned document-start probe"));
        const entries = await guest.evaluate((key) => window[key].ring.take("console", 0).entries, GUEST_KEY);
        assert(entries.some((entry) => entry.text === "owned document-start probe"), "first-document errors are observable", entries);
      }
    } finally { await context.close(); }
  }
});

await browser.close();
site.close();

let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  const detail = result.detail ? `  — ${result.detail}` : "";
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${detail}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
