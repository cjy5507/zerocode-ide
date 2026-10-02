/* The window's IPC census (t-20972): which requests the page sends over the
 * custom-scheme road, how many, and how heavy.
 *
 * Every `invoke` of the window is a `fetch("ipc://localhost/<command>")`: the
 * UI process's MAIN thread receives it (`wry … url_scheme_handler::start_task`),
 * has WebKit rebuild it as an NSURLRequest, copies its body into a `Vec`,
 * rebuilds its headers into an `http::HeaderMap`, parses a JSON body into a
 * `serde_json::Value` and only then hands the command to its own thread. A
 * request costs the main thread a fixed part plus a part per body byte, so
 * what matters about a sender is how MANY requests it sends in a burst and how
 * BIG each body is — and the harness's backend stub sees both, because the
 * page's own code builds every body.
 *
 *   WINDOW_IPC_CENSUS=<file.json> WINDOW_SUITES=<suite> node ui/tests/window.mjs
 *
 * turns the census on for that run: every page the harness stands records one
 * row per request — command, the bytes its body would weigh on the wire, and
 * when — and at exit the run writes the rows to <file.json> and prints the
 * table (top senders by count and by bytes) to stderr. Off by default: a page
 * with the census on pays a JSON encode per request, which is the cost of
 * asking, and no other test should pay it.
 *
 * `ipcBodyOf` below is the page's mirror of what Tauri's own script
 * (`scripts/process-ipc-message-fn.js`, tauri 2.11) puts on the wire for the
 * arguments of an `invoke`: a top-level `ArrayBuffer`, typed array or array
 * rides raw (`application/octet-stream`); anything else is `JSON.stringify`ed
 * with the replacer that turns a `Map` into an object and a `Uint8Array` or
 * `ArrayBuffer` INSIDE the arguments into a JSON array of numbers — four
 * bytes or more for every byte, which is why a picture belongs at the top
 * level of an `invoke`, never inside an object. */

import { writeFileSync } from "node:fs";

/* How often a page hands the rows it holds to the run. A page that dies
 * between two beats takes its last rows with it; `pagehide` covers the
 * ordinary close and the beat covers a page the harness drops hard. */
const FLUSH_EVERY_MS = 200;
const SINK_BINDING = "__ipcCensusSink";

/* Installed as an init script, before the backend stub, so `window.__IPC_LOG__`
 * is there when the stub's `invoke` first asks for it. Self-contained: it is
 * serialised to the page by its source. */
export const installIpcLog = ({ flushEveryMs, sink }) => {
  const rows = [];
  const pageId = Math.random().toString(36).slice(2, 8);
  const replacer = (_key, value) => {
    if (value instanceof Map) return Object.fromEntries(value.entries());
    if (value instanceof Uint8Array) return Array.from(value);
    if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
    if (typeof value === "object" && value !== null && "__TAURI_TO_IPC_KEY__" in value) {
      return value.__TAURI_TO_IPC_KEY__();
    }
    return value;
  };
  const encoder = new TextEncoder();
  // What the receiver is told, in 32 bits: FNV-1a over the text of the body, so
  // the census can say how often a sender repeated itself word for word.
  const digestOf = (text) => {
    let hash = 0x811c9dc5;
    for (let at = 0; at < text.length; at += 1) {
      hash = Math.imul(hash ^ text.charCodeAt(at), 0x01000193) >>> 0;
    }
    return hash.toString(16);
  };
  const bodyOf = (args) => {
    if (args instanceof ArrayBuffer || ArrayBuffer.isView(args)) {
      const view = new Uint8Array(args.buffer ?? args, args.byteOffset ?? 0, args.byteLength);
      // A raw body is hashed by its first 4 KiB: a picture's repeat shows there.
      return { kind: "raw", bytes: args.byteLength, digest: digestOf(String.fromCharCode(...view.subarray(0, 4096))) };
    }
    if (Array.isArray(args)) {
      const text = String(args);
      return { kind: "raw", bytes: encoder.encode(text).length, digest: digestOf(text) };
    }
    // `invoke(cmd)` sends `{}`: two bytes, and the floor of every request.
    const json = JSON.stringify(args ?? {}, replacer) ?? "{}";
    return { kind: "json", bytes: encoder.encode(json).length, digest: digestOf(json) };
  };
  window.__IPC_BODY_OF__ = bodyOf;
  window.__IPC_LOG__ = (command, args, options) => {
    let body;
    try {
      body = bodyOf(args);
    } catch {
      body = { kind: "unreadable", bytes: -1, digest: "" };
    }
    const headers = options && options.headers ? Object.keys(options.headers).length : 0;
    rows.push({
      at: Math.round(performance.now() * 100) / 100,
      command,
      kind: body.kind,
      bytes: body.bytes,
      digest: body.digest ?? "",
      headers,
      page: pageId,
    });
  };
  const flush = () => {
    if (rows.length === 0 || typeof window[sink] !== "function") return;
    window[sink](JSON.stringify(rows.splice(0)));
  };
  setInterval(flush, flushEveryMs);
  window.addEventListener("pagehide", flush);
  document.addEventListener("visibilitychange", flush);
};

/* Everything the run's pages have sent, in arrival order. */
export const censusRows = [];

/* The one place the harness's `standBackend` asks: a no-op unless the run
 * turned the census on. A surface that is only a stand-in for a page (the two
 * preview scripts pass an object with `addInitScript` alone) simply gets no
 * binding and so no rows. */
export async function standIpcCensus(surface) {
  if (!process.env.WINDOW_IPC_CENSUS) return;
  if (typeof surface.exposeBinding === "function") {
    try {
      await surface.exposeBinding(SINK_BINDING, (_source, batch) => {
        for (const row of JSON.parse(batch)) censusRows.push(row);
      });
    } catch {
      // A context that already has the binding keeps the one it has.
    }
  }
  await surface.addInitScript(installIpcLog, { flushEveryMs: FLUSH_EVERY_MS, sink: SINK_BINDING });
}

const percentile = (sorted, share) =>
  sorted.length === 0 ? 0 : sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * share))];

/* The senders, one row each. Pure: the same rows give the same table, in the
 * run or in a test.
 *
 * `again` counts the requests that repeated the one before them from the same
 * sender word for word — what the receiver already had. A page's rows arrive
 * in the order it sent them, and rows of different pages are told apart by
 * `page`, so a second window repeating the first is not a repeat. */
export function summarize(rows) {
  const byCommand = new Map();
  for (const row of rows) {
    if (!byCommand.has(row.command)) byCommand.set(row.command, []);
    byCommand.get(row.command).push(row);
  }
  return [...byCommand].map(([command, held]) => {
    const sizes = held.map((row) => row.bytes).sort((a, b) => a - b);
    const lastOfPage = new Map();
    let again = 0;
    for (const row of held) {
      const page = row.page ?? "";
      // A row that carries no digest (a file written before digests existed)
      // cannot be told from the one before it, so it is nobody's repeat.
      if (row.digest && lastOfPage.get(page) === row.digest) again += 1;
      lastOfPage.set(page, row.digest);
    }
    return {
      command,
      count: held.length,
      bytes: sizes.reduce((sum, size) => sum + Math.max(0, size), 0),
      max: sizes.at(-1) ?? 0,
      p50: percentile(sizes, 0.5),
      p95: percentile(sizes, 0.95),
      raw: held.filter((row) => row.kind === "raw").length,
      unique: new Set(held.map((row) => row.digest)).size,
      again,
    };
  });
}

/* The widest burst: the most requests that left one page inside any one window
 * of `windowMs`. A page serves its requests one at a time on its main thread,
 * so this is the length of the line that formed behind the first of them. Each
 * page keeps its own clock, so pages are counted apart. */
export function widestBurst(rows, windowMs) {
  const byPage = new Map();
  for (const row of rows) {
    const page = row.page ?? "";
    if (!byPage.has(page)) byPage.set(page, []);
    byPage.get(page).push(row.at);
  }
  let widest = 0;
  for (const held of byPage.values()) {
    const stamps = held.sort((a, b) => a - b);
    let from = 0;
    for (let at = 0; at < stamps.length; at += 1) {
      while (stamps[at] - stamps[from] > windowMs) from += 1;
      widest = Math.max(widest, at - from + 1);
    }
  }
  return widest;
}

const pad = (text, width) => String(text).padEnd(width);
const num = (value) => String(Math.round(value)).padStart(10);

/* The table the report carries: the top senders by how many requests they
 * sent, by how many bytes, and by how many of their requests repeated the one
 * before word for word; and the largest single body. */
export function table(rows, { top = 12, burstMs = 50 } = {}) {
  const senders = summarize(rows);
  const line = (sender) =>
    `${pad(sender.command, 34)}${num(sender.count)}${num(sender.bytes)}${num(sender.max)}${num(sender.p50)}${num(sender.p95)}${num(sender.again)}`;
  const head = `${pad("command", 34)}${"count".padStart(10)}${"bytes".padStart(10)}${"max".padStart(10)}${"p50".padStart(10)}${"p95".padStart(10)}${"again".padStart(10)}`;
  const out = [];
  out.push(`requests ${rows.length} · senders ${senders.length} · widest burst in ${burstMs} ms: ${widestBurst(rows, burstMs)}`);
  const ranked = (title, by) => {
    out.push(`— top senders by ${title} —`, head);
    for (const sender of [...senders].sort((a, b) => b[by] - a[by]).slice(0, top)) out.push(line(sender));
  };
  ranked("count", "count");
  ranked("bytes", "bytes");
  ranked("repeats (word for word, as the request before)", "again");
  const biggest = [...rows].sort((a, b) => b.bytes - a.bytes)[0];
  if (biggest) out.push(`largest body: ${biggest.command} ${biggest.bytes} bytes (${biggest.kind})`);
  return out.join("\n");
}

/* At exit: the rows to the file the run named, the table to stderr. A run
 * that sent nothing writes an empty list, so "the census saw nothing" is a
 * file and not an absence. */
if (process.env.WINDOW_IPC_CENSUS) {
  process.on("exit", () => {
    try {
      writeFileSync(process.env.WINDOW_IPC_CENSUS, JSON.stringify(censusRows));
      process.stderr.write(`${table(censusRows)}\n`);
    } catch (error) {
      process.stderr.write(`ipc census could not be written: ${error}\n`);
    }
  });
}
