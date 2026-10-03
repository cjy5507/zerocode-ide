import assert from "node:assert/strict";
import test from "node:test";

import { installIpcLog, summarize, table, widestBurst } from "./ipc-census.mjs";

/* The census's own arithmetic (t-20972), in node alone: what a body weighs on
 * the wire, how a sender's repeats and a page's bursts are counted. The page
 * half is run here against a stand-in `window`, because it is the half that has
 * to say what Tauri's own script would put on the wire. */

/* Runs the page half once, in this process, and hands back what a page would
 * hold: the function the stub calls, and the rows it has taken. */
function pageWithCensus() {
  const held = { listeners: [] };
  const flushed = [];
  const realInterval = globalThis.setInterval;
  const realWindow = globalThis.window;
  const realDocument = globalThis.document;
  globalThis.setInterval = () => 0;
  globalThis.window = {
    addEventListener: (name, listener) => held.listeners.push([name, listener]),
    __sink: (batch) => flushed.push(...JSON.parse(batch)),
  };
  globalThis.document = { addEventListener: (name, listener) => held.listeners.push([name, listener]) };
  try {
    installIpcLog({ flushEveryMs: 1, sink: "__sink" });
  } finally {
    globalThis.setInterval = realInterval;
  }
  const page = globalThis.window;
  const flush = () => held.listeners.find(([name]) => name === "pagehide")[1]();
  const restore = () => {
    globalThis.window = realWindow;
    globalThis.document = realDocument;
  };
  return { page, flush, flushed, restore };
}

test("a body weighs what Tauri's own script puts on the wire", () => {
  const { page, restore } = pageWithCensus();
  try {
    const bodyOf = page.__IPC_BODY_OF__;
    // `invoke(cmd)` sends `{}`: two bytes, the floor of every request.
    assert.deepEqual(bodyOf(undefined), { kind: "json", bytes: 2, digest: bodyOf({}).digest });
    // JSON is measured in UTF-8 bytes, not characters: a Korean word is three each.
    assert.equal(bodyOf({ text: "한" }).bytes, new TextEncoder().encode('{"text":"한"}').length);
    // A raw buffer at the top level rides as it is.
    assert.deepEqual(
      { kind: bodyOf(new Uint8Array(1000)).kind, bytes: bodyOf(new Uint8Array(1000)).bytes },
      { kind: "raw", bytes: 1000 },
    );
    assert.equal(bodyOf(new ArrayBuffer(64)).bytes, 64);
    // The same bytes INSIDE an object become a JSON array of numbers — "0,0,0"
    // is two bytes a byte, and noise is up to four.
    const inside = bodyOf({ bytes: new Uint8Array(1000) });
    assert.equal(inside.kind, "json");
    assert.ok(inside.bytes >= 2 * 1000, `a typed array in an object cost ${inside.bytes}`);
    assert.equal(bodyOf({ map: new Map([["a", 1]]) }).bytes, new TextEncoder().encode('{"map":{"a":1}}').length);
  } finally {
    restore();
  }
});

test("a repeated request has the digest of the one before it, and a different one does not", () => {
  const { page, restore } = pageWithCensus();
  try {
    const bodyOf = page.__IPC_BODY_OF__;
    assert.equal(bodyOf({ term: 3, rows: 24 }).digest, bodyOf({ term: 3, rows: 24 }).digest);
    assert.notEqual(bodyOf({ term: 3, rows: 24 }).digest, bodyOf({ term: 3, rows: 25 }).digest);
  } finally {
    restore();
  }
});

test("rows reach the run in the order the page sent them, with the page that sent them", () => {
  const { page, flush, flushed, restore } = pageWithCensus();
  try {
    page.__IPC_LOG__("term_pull", { resync: false }, undefined);
    page.__IPC_LOG__("set_keybinding", { id: "x" }, { headers: { a: "1", b: "2" } });
    flush();
    assert.deepEqual(flushed.map((row) => row.command), ["term_pull", "set_keybinding"]);
    assert.equal(flushed[1].headers, 2);
    assert.equal(new Set(flushed.map((row) => row.page)).size, 1);
  } finally {
    restore();
  }
});

const row = (page, command, bytes, digest, at = 0) => ({ page, command, bytes, digest, kind: "json", at });

test("a sender's repeats are counted within a page, never across pages", () => {
  const rows = [
    row("a", "term_resize", 30, "1"),
    row("a", "term_resize", 30, "1"),
    row("a", "term_resize", 30, "1"),
    row("a", "term_resize", 31, "2"),
    row("b", "term_resize", 30, "1"),
  ];
  const [sender] = summarize(rows);
  assert.equal(sender.count, 5);
  assert.equal(sender.unique, 2);
  // Two repeats on page a; page b's first request is nobody's repeat.
  assert.equal(sender.again, 2);
  assert.equal(sender.max, 31);
});

test("a row with no digest is nobody's repeat", () => {
  // The rows a census wrote before it hashed bodies carry neither digest nor page.
  const rows = [1, 2, 3].map((at) => ({ command: "term_pull", bytes: 16, kind: "json", at }));
  const [sender] = summarize(rows);
  assert.equal(sender.count, 3);
  assert.equal(sender.again, 0);
});

test("the widest burst is the most requests one page sent inside one window", () => {
  const rows = [
    ...[0, 10, 20, 30, 200].map((at) => row("a", "x", 2, "0", at)),
    // Another page's clock is its own: its 300 ms does not sit beside a's.
    ...[0, 5].map((at) => row("b", "x", 2, "0", at)),
  ];
  assert.equal(widestBurst(rows, 50), 4);
  assert.equal(widestBurst(rows, 1000), 5);
});

test("the table names the senders by count, bytes and repeats, and the largest body", () => {
  const rows = [
    row("a", "term_pull", 16, "p", 0),
    row("a", "term_pull", 16, "p", 1),
    row("a", "write_text_file", 5000, "w", 2),
  ];
  const said = table(rows, { top: 3 });
  assert.match(said, /top senders by count/);
  assert.match(said, /top senders by bytes/);
  assert.match(said, /top senders by repeats/);
  assert.match(said, /largest body: write_text_file 5000 bytes/);
});
