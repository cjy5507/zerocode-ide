/* What opening a page artifact costs (t-55370) — the window's side and the engine's side, apart.
 *
 * The window harness has no native browser pane: the backend makes one (a CEF pane on macOS, a wry
 * pane elsewhere) when the window asks `open_browser_pane`. So the window's side is counted on the
 * stub backend — the window's own work from the press to the tab standing with its strip, and how
 * many native panes and requests that press asked for — and the engine's side is measured on the
 * engines themselves: a synthetic report opened in a page made for it (a new pane) and in a page
 * that already stands (a pane kept), to its first paint, and the renderers' footprint after N
 * openings each way. Chromium is the engine of the macOS browser pane (CEF); WebKit is the window's
 * own and the hidden thumbnail and export panes'.
 *
 * The window's side (stub backend, the gallery fixture's two pages):
 *   open     the drawer's 「열기」 on a page → its browser tab in front with the page's title on the strip
 *   reopen   back to the gallery, 「열기」 on the same page again
 *   other    back to the gallery, 「열기」 on the other page
 *   back     from the page's tab to the gallery's tab: to the frame after, cards made, nodes added
 *   toPane   from the gallery to the terminal pane's tab, and fromPane: back to the gallery
 * Each says its ms (from the press to the frame after the end), the native panes asked for, the
 * requests asked and the tabs standing after it.
 *
 * The engine's side (two synthetic reports, every word synthetic: 50 KB of text and tables, and
 * 500 KB with pictures — a published report's own shape, whose bytes are mostly its figures):
 *   fresh    a new page, then the report: from the ask to its first paint and to its load
 *   kept     a page showing the other report, then this one: the same two numbers
 *   memory   after N openings, the footprint of the renderers that stand: one page each time (fresh)
 *            against one page walked N times (kept)
 *
 *   node ui/tests/artifact-page-perf.mjs [--rounds 5] [--engine chromium|webkit|both] [--side window|engine|both]
 *                                        [--openings 4] [--json out.json]
 * `WINDOW_CPU_THROTTLE=4` slows the window's page (the window's side) and each report's page (the
 * engine's side, Chromium only) four times; `taskpolicy -b node …` runs all of it on the efficiency
 * cores. */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage, recordBootPhases, waitBootSettled } from "./window-boot.mjs";
import { median } from "./coordinator-desk-perf.mjs";
import { standingPids, webkitType } from "./conversation-perf.mjs";
import { footprintOf, pageProcesses, syntheticPng } from "./memory-census.mjs";
import { loadNote } from "./machine-load.mjs";

/* The window a person has open. */
const REAL_WINDOW = Object.freeze({ width: 1440, height: 900 });
/* The fixture's catalog: report rows and the three gallery rows twice over — page, document, web —
 * so two pages stand (`gal-0`, `gal-3`). */
const CATALOG = Object.freeze({ count: 30, gallery: 6 });
const PAGES = Object.freeze(["gal-0", "gal-3"]);
/* How many frames a gesture may take before the round is called stuck — a bound, so a window that
 * never stands the tab fails the run instead of hanging it. */
const STAND_FRAMES_MAX = 240;
/* The two reports: one of text and tables, one whose bytes are mostly pictures. */
export const REPORTS = Object.freeze([
  { name: "report-50k", bytes: 50 * 1024, pictures: false },
  { name: "report-500k", bytes: 500 * 1024, pictures: true },
]);
/* The pictures a report carries, cycled — sizes a report's figures have (screens and crops). */
const FIGURES = Object.freeze([
  { width: 1280, height: 720, noisyEvery: 24 },
  { width: 960, height: 600, noisyEvery: 18 },
  { width: 640, height: 480, noisyEvery: 12 },
]);
const VIEWPORT = Object.freeze({ width: 1280, height: 800 });
/* How long the engine waits for a page's own load, in ms. */
const LOAD_PATIENCE_MS = 30000;

/* ---- the synthetic reports --------------------------------------------------------------- */

const WORDS = "닫을 때 큐에 남은 일을 비우고 비운 수를 돌려준다. 호출한 쪽은 그 수를 기록에 남긴다. ";

function section(at) {
  const rows = Array.from({ length: 8 }, (_, row) =>
    `<tr><td>항목 ${at}-${row}</td><td>${(row * 7.3 + at).toFixed(1)} ms</td><td>${row * 3 + at} 개</td><td>합성</td></tr>`).join("");
  return [
    `<section id="s${at}"><h2>${at + 1}. 합성 절 ${at + 1}</h2>`,
    `<p>${WORDS.repeat(5)}</p>`,
    `<ul><li>${WORDS}</li><li>${WORDS}</li><li>${WORDS}</li></ul>`,
    `<table><thead><tr><th>무엇</th><th>시간</th><th>수</th><th>출처</th></tr></thead><tbody>${rows}</tbody></table>`,
    `<pre><code>${"fn close(queue: &mut Queue) -> usize { queue.drain(..).count() }\n".repeat(4)}</code></pre>`,
    `<p>${WORDS.repeat(3)}</p></section>`,
  ].join("\n");
}

/* One report of about `bytes`: a head, sections of text and tables until the text is a tenth of
 * the bytes (or all of them, without pictures), then figures until the bytes are reached. */
export function syntheticReport({ bytes, pictures }) {
  const head = [
    "<!doctype html><html lang=\"ko\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">",
    "<title>합성 보고서</title><style>",
    ":root{--bg:#fbfaf7;--fg:#1d1c1a;--muted:#6b6862;--line:#e3e0d8}",
    "body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.65 -apple-system,system-ui,sans-serif}",
    "main{max-width:880px;margin:0 auto;padding:32px 24px}h1{font-size:28px}h2{font-size:20px;margin-top:40px}",
    "table{border-collapse:collapse;width:100%;font-size:13px}td,th{border-bottom:1px solid var(--line);padding:6px 8px;text-align:left}",
    "pre{background:#f1efe9;padding:12px;overflow:auto;font-size:12px}figure{margin:24px 0}img{max-width:100%;height:auto;display:block}",
    "figcaption{color:var(--muted);font-size:13px}",
    "</style></head><body><main><h1>합성 보고서 — 닫을 때 남은 일을 비운다</h1>",
    `<p>${WORDS.repeat(2)}</p>`,
  ].join("\n");
  const tail = "</main></body></html>\n";
  const textBudget = pictures ? Math.round(bytes / 10) : bytes;
  const parts = [head];
  let size = Buffer.byteLength(head) + Buffer.byteLength(tail);
  for (let at = 0; size < textBudget; at += 1) {
    const one = section(at);
    parts.push(one);
    size += Buffer.byteLength(one);
  }
  for (let at = 0; pictures && size < bytes; at += 1) {
    const png = syntheticPng(FIGURES[at % FIGURES.length]).toString("base64");
    const one = `<figure><img alt="합성 그림 ${at + 1}" src="data:image/png;base64,${png}"><figcaption>그림 ${at + 1}. 합성</figcaption></figure>`;
    parts.push(one);
    size += Buffer.byteLength(one);
  }
  parts.push(tail);
  return parts.join("\n");
}

/* ---- the window's side ------------------------------------------------------------------- */

/* The gallery fixture standing, the tab open, the pane door counting. */
async function standGallery(page) {
  await page.evaluate(async (catalog) => {
    dropTab("artifacts");
    window.__ARTIFACTS__ = catalog;
    let born = 0;
    window.__PANES_MADE__ = 0;
    window.__ANSWER__.open_browser_pane = () => {
      born += 1;
      window.__PANES_MADE__ += 1;
      return `browser-perf-${born}`;
    };
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.close_browser_pane = () => null;
    window.__ANSWER__.browser_navigate = () => null;
    el("nav-artifacts").click();
    await window.__PAINTED__();
    for (let beat = 0; beat < 600 && (artifactAsking || artifactRows.size === 0); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await window.__PAINTED__();
  }, CATALOG);
}

/* One gesture, read: `act` runs in the page, `stood` says when it is done; answers the ms from the
 * act to the frame after it stood, the native panes and the requests it asked for, and the tabs. */
function readGesture(page, { act, stood, arg }) {
  return page.evaluate(async ({ act, stood, arg, frames }) => {
    const run = new Function("arg", `return (async () => { ${act} })();`);
    const done = new Function("arg", `return (${stood});`);
    const countsBefore = { ...window.__COUNTS__ };
    const panesBefore = window.__PANES_MADE__;
    const cardsBefore = typeof artifactCardCreations === "number" ? artifactCardCreations : 0;
    let added = 0;
    const watch = new MutationObserver((records) => {
      for (const record of records) added += record.addedNodes.length;
    });
    watch.observe(document.body, { childList: true, subtree: true });
    const from = performance.now();
    await run(arg);
    let beat = 0;
    for (; beat < frames && !done(arg); beat += 1) await new Promise((next) => requestAnimationFrame(next));
    await window.__PAINTED__();
    const ms = performance.now() - from;
    watch.disconnect();
    const asked = {};
    for (const [command, count] of Object.entries(window.__COUNTS__)) {
      const more = count - (countsBefore[command] ?? 0);
      if (more > 0) asked[command] = more;
    }
    return {
      ms,
      stood: done(arg),
      panesMade: window.__PANES_MADE__ - panesBefore,
      asked,
      requests: Object.values(asked).reduce((sum, count) => sum + count, 0),
      cardsMade: (typeof artifactCardCreations === "number" ? artifactCardCreations : 0) - cardsBefore,
      nodesAdded: added,
      browserTabs: tabs.filter((one) => one.kind === "browser").length,
      pageTabs: tabs.filter((one) => one.kind === "browser" && one.artifact).length,
    };
  }, { act, stood, arg, frames: STAND_FRAMES_MAX });
}

/* The page's tab in front with its strip saying the page's title. */
const PAGE_STANDS = `(() => {
  const tab = tabs.find((one) => one.id === activeTabId);
  if (!tab || tab.kind !== "browser" || tab.artifact?.id !== arg) return false;
  const strip = docHost(tab.pane, "browser").querySelector(".artifact-strip");
  return strip && !strip.hidden && strip.querySelector(".artifact-strip-title").textContent === artifactRows.get(arg).title;
})()`;
/* The gallery in front with its cards painted. */
const GALLERY_STANDS = `activeTabId === ARTIFACTS_TAB.id && artifactsView() !== null && artifactsView().querySelectorAll(".artifact-card").length > 0`;
/* The press a person makes: the card selected, the drawer's 「열기」. */
const OPEN_FROM_DRAWER = `
  const view = artifactsView();
  selectArtifact(view, arg);
  view.querySelector('.artifacts-drawer [data-artifact-action="open"]').click();
  for (let beat = 0; beat < 240 && artifactOpening.has(arg); beat += 1) await new Promise((next) => setTimeout(next, 0));`;
const BACK_TO_GALLERY = `setActiveTab(ARTIFACTS_TAB.id);`;
/* The boot's terminal tab — the pane a person works in beside the gallery. */
const TO_PANE = `setActiveTab(tabs.find((one) => one.kind === "term").id);`;
const PANE_STANDS = `tabs.find((one) => one.id === activeTabId)?.kind === "term"`;

export async function measureWindowSide(page) {
  await standGallery(page);
  const open = await readGesture(page, { act: OPEN_FROM_DRAWER, stood: PAGE_STANDS, arg: PAGES[0] });
  const back = await readGesture(page, { act: BACK_TO_GALLERY, stood: GALLERY_STANDS, arg: null });
  const reopen = await readGesture(page, { act: OPEN_FROM_DRAWER, stood: PAGE_STANDS, arg: PAGES[0] });
  await readGesture(page, { act: BACK_TO_GALLERY, stood: GALLERY_STANDS, arg: null });
  const other = await readGesture(page, { act: OPEN_FROM_DRAWER, stood: PAGE_STANDS, arg: PAGES[1] });
  await readGesture(page, { act: BACK_TO_GALLERY, stood: GALLERY_STANDS, arg: null });
  const toPane = await readGesture(page, { act: TO_PANE, stood: PANE_STANDS, arg: null });
  const fromPane = await readGesture(page, { act: BACK_TO_GALLERY, stood: GALLERY_STANDS, arg: null });
  return { open, back, reopen, other, toPane, fromPane };
}

async function windowRounds({ rounds, engine }) {
  const { files, origin } = await createWindowServer();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  const taken = [];
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin, { before: recordBootPhases });
      try {
        await page.setViewportSize(REAL_WINDOW);
        await waitBootSettled(page);
        const one = await measureWindowSide(page);
        if (faults.length) one.faults = faults.slice(0, 3);
        taken.push(one);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const summary = {};
  for (const gesture of Object.keys(taken[0]).filter((key) => key !== "faults")) {
    const all = taken.map((one) => one[gesture]);
    summary[gesture] = {
      ms: median(all.map((one) => one.ms)),
      stood: all.every((one) => one.stood),
      panesMade: median(all.map((one) => one.panesMade)),
      requests: median(all.map((one) => one.requests)),
      asked: all.at(-1).asked,
      cardsMade: median(all.map((one) => one.cardsMade)),
      nodesAdded: median(all.map((one) => one.nodesAdded)),
      browserTabs: all.at(-1).browserTabs,
      pageTabs: all.at(-1).pageTabs,
    };
  }
  return { engine, rounds, summary, taken };
}

/* ---- the engine's side ------------------------------------------------------------------- */

/* How long a loaded page may take to report its first contentful paint, in ms — a small page loads
 * before its first frame, so the paint is waited for rather than read at the load. */
const PAINT_PATIENCE_MS = 3000;

/* The page's own clocks once it has loaded and painted: its time origin (epoch ms), the first
 * contentful paint and the load's end, both from the origin. */
const pageClocks = (page) => page.evaluate((patience) => new Promise((done) => {
  const read = () => ({
    origin: performance.timeOrigin,
    paint: performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? null,
    load: performance.getEntriesByType("navigation")[0]?.loadEventEnd ?? null,
  });
  if (read().paint !== null) {
    done(read());
    return;
  }
  const watch = new PerformanceObserver(() => {
    if (read().paint === null) return;
    watch.disconnect();
    done(read());
  });
  watch.observe({ type: "paint", buffered: true });
  setTimeout(() => {
    watch.disconnect();
    done(read());
  }, patience);
}), PAINT_PATIENCE_MS);

/* One report shown in `page`, read from `asked` (the epoch ms the ask was made at). */
async function showReport(page, url, asked) {
  await page.goto(url, { waitUntil: "load", timeout: LOAD_PATIENCE_MS });
  const clocks = await pageClocks(page);
  return {
    toPaintMs: clocks.paint === null ? null : clocks.origin + clocks.paint - asked,
    toLoadMs: clocks.load === null ? null : clocks.origin + clocks.load - asked,
  };
}

async function slowed(context, page, engine) {
  const rate = Number(process.env.WINDOW_CPU_THROTTLE ?? 0);
  if (engine !== "chromium" || !(rate > 1)) return;
  const cdp = await context.newCDPSession(page);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate });
}

/* The footprint of the renderers this browser stands now, summed. */
function renderersFootprint(engine, before, folder) {
  const rows = pageProcesses(engine, before);
  let total = 0;
  for (const row of rows) {
    try {
      total += footprintOf(row.pid, folder).footprint;
    } catch {
      // A spare renderer that left between the listing and the reading holds nothing.
    }
  }
  return { renderers: rows.length, footprint: total };
}

async function engineRound({ engine, urls, openings, folder }) {
  const before = standingPids();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  const out = { reports: {} };
  try {
    for (const [name, url] of Object.entries(urls)) {
      const other = Object.values(urls).find((one) => one !== url);
      const context = await browser.newContext({ viewport: VIEWPORT });
      try {
        // fresh: the pane is made for this opening.
        const askedFresh = Date.now();
        const freshPage = await context.newPage();
        await slowed(context, freshPage, engine);
        const madeMs = Date.now() - askedFresh;
        const fresh = { madeMs, ...(await showReport(freshPage, url, askedFresh)) };
        await freshPage.close();
        // kept: the pane stands on the other report; the opening walks it here.
        const keptPage = await context.newPage();
        await slowed(context, keptPage, engine);
        await keptPage.goto(other, { waitUntil: "load", timeout: LOAD_PATIENCE_MS });
        const kept = await showReport(keptPage, url, Date.now());
        await keptPage.close();
        out.reports[name] = { fresh, kept };
      } finally {
        await context.close();
      }
    }
    // memory: N openings, one page each against one page walked N times — the two reports in turn,
    // the bigger one last.
    const walk = Array.from({ length: openings }, (_, at) => REPORTS[(openings - 1 - at) % REPORTS.length].name)
      .map((name) => urls[name]).reverse();
    const memory = {};
    for (const way of ["fresh", "kept"]) {
      const context = await browser.newContext({ viewport: VIEWPORT });
      try {
        let held = null;
        for (const url of walk) {
          if (way === "fresh" || held === null) held = await context.newPage();
          await held.goto(url, { waitUntil: "load", timeout: LOAD_PATIENCE_MS });
        }
        await new Promise((done) => setTimeout(done, 500));
        memory[way] = renderersFootprint(engine, before, folder);
      } finally {
        await context.close();
      }
    }
    out.memory = { openings, ...memory };
  } finally {
    await browser.close();
  }
  return out;
}

async function engineRounds({ rounds, engine, openings }) {
  const folder = mkdtempSync(join(tmpdir(), "zerocode-artifact-page-perf-"));
  const sizes = {};
  try {
    const urls = {};
    for (const report of REPORTS) {
      const html = syntheticReport(report);
      const path = join(folder, `${report.name}.html`);
      writeFileSync(path, html);
      sizes[report.name] = Buffer.byteLength(html);
      urls[report.name] = pathToFileURL(path).href;
    }
    const taken = [];
    for (let round = 0; round < rounds; round += 1) taken.push(await engineRound({ engine, urls, openings, folder }));
    const pick = (read) => median(taken.map(read).filter((one) => typeof one === "number"));
    const summary = { reports: {} };
    for (const { name } of REPORTS) {
      summary.reports[name] = { bytes: sizes[name] };
      for (const way of ["fresh", "kept"]) {
        summary.reports[name][way] = {
          toPaintMs: pick((one) => one.reports[name][way].toPaintMs),
          toLoadMs: pick((one) => one.reports[name][way].toLoadMs),
          ...(way === "fresh" ? { madeMs: pick((one) => one.reports[name].fresh.madeMs) } : {}),
        };
      }
    }
    summary.memory = {
      openings,
      freshMiB: pick((one) => one.memory.fresh.footprint) / 1048576,
      freshRenderers: pick((one) => one.memory.fresh.renderers),
      keptMiB: pick((one) => one.memory.kept.footprint) / 1048576,
      keptRenderers: pick((one) => one.memory.kept.renderers),
    };
    return { engine, rounds, summary, taken };
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
}

/* ---- the run ----------------------------------------------------------------------------- */

export async function measureArtifactPage({ rounds = 5, engines = ["chromium", "webkit"], sides = ["window", "engine"], openings = 4 } = {}) {
  const result = { throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1), load: loadNote(), window: {}, engine: {} };
  for (const engine of engines) {
    if (sides.includes("window")) result.window[engine] = await windowRounds({ rounds, engine });
    if (sides.includes("engine")) result.engine[engine] = await engineRounds({ rounds, engine, openings });
  }
  return result;
}

const direct = process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (direct) {
  const option = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at > 0 && process.argv[at + 1] ? process.argv[at + 1] : fallback;
  };
  const both = (value, pair) => (value === "both" ? pair : [value]);
  const measured = await measureArtifactPage({
    rounds: Number(option("--rounds", 5)),
    engines: both(option("--engine", "both"), ["chromium", "webkit"]),
    sides: both(option("--side", "both"), ["window", "engine"]),
    openings: Number(option("--openings", 4)),
  });
  const json = option("--json", "");
  if (json) writeFileSync(json, `${JSON.stringify(measured, null, 1)}\n`);
  const shown = JSON.parse(JSON.stringify(measured));
  for (const side of ["window", "engine"]) for (const engine of Object.keys(shown[side])) delete shown[side][engine].taken;
  console.log(JSON.stringify(shown, null, 1));
  const stuck = Object.values(measured.window).some((one) => Object.values(one.summary).some((gesture) => !gesture.stood));
  if (stuck) process.exitCode = 1;
}
