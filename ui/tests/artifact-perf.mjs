/* The Artifacts tab's weight on a catalog of 1,500 rows and 400 tasks (t-36910).
 *
 * The ruler for 「숫자 없는 개선 금지」. Before is the base commit's `ui/` unpacked as a snapshot,
 * after is this tree, and both run this one file (`git archive <ref> ui crates/… | tar -x`, copy this
 * file in, run — the way `writing-badge-perf.mjs` is run). The fake runtime answers as the real one
 * does on either side: asked for present rows it leaves the vanished ones out and counts them by
 * kind; asked for everything it sends them all and names the vanished. So what the base window pays
 * for the rows it only hides is in its numbers, and what this window saves by not carrying them is
 * in its own.
 *
 * The catalog is shaped like the measured one (2026-10-04: 1,346 rows — 50% reports, 37% rows whose
 * files are gone, every one of them a Computer Use screenshot or step log): 1,500 rows, 400 tasks,
 * 555 vanished. Every word in it is synthetic.
 *
 * Numbers:
 *   1. first paint — from the sidebar's press to the first cards on screen, ms; rows the window
 *      holds; the listing answer's rows and bytes;
 *   2. a task filter — from the filter to the next frame, ms (the road a card's task chip takes);
 *   3. opening the drawer on a report — from the selection to the painted body, ms;
 *   4. memory — the JS heap after a collection and the document's element count, before and after
 *      N rounds of opening and closing the drawer over five reports; and the ms of each opening;
 *   5. where the time goes, when a number above moved: the N rounds are read in ten parts, each with
 *      the median of its openings and the renderer's own clocks for one round (script, style, layout,
 *      and how many times each ran), and with the listeners and nodes the page held before and after
 *      — so a cost that grows with the rounds shows as a slope and says in which clock it grew.
 *      `--attribute N` reads the same clocks over N rounds of each single operation (the drawer, the
 *      tab, the filter), `--profile` adds the heaviest functions of the first and the last part,
 *      `--count` counts the observers, listeners, timers and frames the page asked for, and
 *      `--without a,b` measures with the window's functions of those names doing nothing — what a
 *      number owes to one step of a painting is the difference.
 *
 * Run (`WINDOW_CPU_THROTTLE=4` slows the page's CPU four times — the low-spec level cpu4x):
 *   node ui/tests/artifact-perf.mjs                               Chromium, median of 5 rounds
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/artifact-perf.mjs
 *   node ui/tests/artifact-perf.mjs --rounds 3 --soak 500 --json out.json
 *   node ui/tests/artifact-perf.mjs --rounds 1 --soak 500 --attribute 30 --profile --count --json out.json */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { median, quantile } from "./coordinator-desk-perf.mjs";
import { loadNote } from "./machine-load.mjs";

/* The window a person has open, where the tab measures 809px. */
const REAL_WINDOW = Object.freeze({ width: 1440, height: 900 });
const CATALOG = Object.freeze({ rows: 1500, tasks: 400, vanished: 555 });
const SOAK_ROUNDS = 500;
/* The reports the soak walks, and how many frames one painted drawer may take before the round is
 * called stuck — a bound, so a window that never paints fails the run instead of hanging it. */
const SOAK_REPORTS = 5;
const PAINT_FRAMES_MAX = 240;
/* The parts a soak is read in: ten, so the first and the last are the tenths the summary names. */
const SOAK_PARTS = 10;
/* The renderer's own clocks (CDP `Performance.getMetrics`: seconds and counts since the page began),
 * under the names one round's share is reported by. */
const PAGE_CLOCKS = Object.freeze({
  ScriptDuration: "scriptMs",
  RecalcStyleDuration: "styleMs",
  LayoutDuration: "layoutMs",
  TaskDuration: "taskMs",
  RecalcStyleCount: "styleRecalcs",
  LayoutCount: "layouts",
});
/* A profile samples every 200 µs — fine enough to rank the functions of a 20 ms opening — and names
 * this many of the heaviest. */
const PROFILE_INTERVAL_US = 200;
const PROFILE_TOP = 16;

/* The synthetic catalog and the fake runtime, stood in the page. */
function standCatalog(page) {
  return page.evaluate(({ rows: total, tasks, vanished }) => {
    const now = Date.now();
    const minute = 60 * 1000;
    const words = "닫을 때 큐에 남은 일을 비우고 비운 수를 돌려준다 호출한 쪽은 그 수를 기록에 남긴다 ";
    const preview = words.repeat(13).slice(0, 600);
    const rows = [];
    const gone = new Set();
    for (let at = 0; at < total; at += 1) {
      // 760 reports, 360 screenshots, 200 evidence files, 110 pages, 60 documents, 10 web rows.
      const kind = at % 150 < 76 ? "report" : at % 150 < 112 ? "screenshot" : at % 150 < 132 ? "evidence"
        : at % 150 < 143 ? "page" : at % 150 < 149 ? "document" : "web";
      const id = `row-${String(at).padStart(4, "0")}`;
      const linked = kind === "report" ? at % 4 !== 0 : kind === "page" && at % 10 === 0;
      const task = linked ? `t-${at % tasks}` : null;
      const origin = kind === "report"
        ? (task
          ? {
            work_summary: `합성 작업 ${at % tasks}`, run: "run-9", task, worker: `w-${at % 900}`, pane: `team-1/%${at % 7}`,
            agent: at % 3 === 0 ? "codex" : "claude", model: at % 3 === 0 ? "gpt-6-astra" : "claude-sonnet-5-5",
            worktree: `/tmp/zerocode-window-test/wt-${at % 40}`,
          }
          : {})
        : kind === "screenshot" || kind === "evidence"
          ? { automation: "computer-use" }
          : { agent: "claude", session: `s-${at}`, project: "/tmp/zerocode-window-test/project", ...(task ? { task } : {}) };
      const path = kind === "web" ? ""
        : kind === "report" ? `/data/artifacts/run/${id}/REPORT.md`
          : kind === "screenshot" || kind === "evidence"
            ? `/tmp/zerocode-window-test/computer-use/sessions/s-${at % 60}/${kind === "evidence" ? "steps.jsonl" : `${at}.png`}`
            : `/tmp/zerocode-window-test/project/${id}.${kind === "page" ? "html" : "md"}`;
      rows.push({
        id, kind, title: `합성 ${kind} ${at} — 닫을 때 남은 일을 비운다`, path, bytes: 4000 + at,
        created_ms: now - at * minute, modified_ms: now - at * minute, origin, tags: [],
        preview: kind === "report" || kind === "document" ? { kind: "markdown", text: preview }
          : kind === "evidence" ? { kind: "text", text: preview }
            : kind === "screenshot" ? { kind: "image", w: 1280, h: 800 } : { kind: "none" },
        source: kind === "report" ? "worker_report" : kind === "web" ? "remote" : kind === "page" || kind === "document" ? "agent_page" : "evidence",
        ...(kind === "report" ? { subtype: ["report", "review", "brief", "handover", "proposal"][at % 5] } : {}),
        ...(kind === "web" ? { url: `https://example.com/artifact/${at}` } : {}),
      });
      // Every screenshot and enough of the evidence to reach the measured share.
      if (kind === "screenshot" || (kind === "evidence" && gone.size < vanished)) gone.add(id);
    }
    const count = (held, pick) => {
      const counted = {};
      for (const one of held) if (pick(one)) counted[one.kind] = (counted[one.kind] ?? 0) + 1;
      return counted;
    };
    const asks = (window.__PERF_ASKS__ = []);
    // Two frames after whatever was asked for has been painted.
    window.__PERF_SETTLE__ = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    window.__ANSWER__.artifacts_list = (args) => {
      const filter = args?.filter ?? {};
      const held = rows.filter((one) => filter.present !== true || !gone.has(one.id));
      const answer = {
        rows: held, total: held.length, truncated: false,
        missing: held.filter((one) => gone.has(one.id)).map((one) => one.id), missing_total: gone.size,
        missing_by_kind: count(rows, (one) => gone.has(one.id)),
        unlinked_by_kind: count(rows, (one) => !gone.has(one.id) && !one.origin.task),
        by_kind: count(held, () => true),
        retention_days: 30, thumb: { width: 320, height: 240, queue_max: 2 },
      };
      asks.push({ present: filter.present === true, rows: held.length, bytes: JSON.stringify(answer).length });
      return answer;
    };
    window.__ANSWER__.artifact_counts = () => ({
      total: rows.length, by_worker: {}, by_task: {}, by_run: {}, by_worktree: {}, by_automation: {}, report_by_worker: {},
    });
    window.__ANSWER__.artifact_thumbnail = () => ({ data_url: null, cached: false });
    // A report of the median size (9 KB on the measured catalog): a title and twelve sections.
    const section = (at) => [`## ${at + 1}. 합성 절`, "", words.repeat(6), "", `- 항목 하나 ${at}`, `- 항목 둘 ${at}`, ""].join("\n");
    const report = (title) => [`# ${title}`, "", words.repeat(2), ""].concat(Array.from({ length: 12 }, (_, at) => section(at))).join("\n");
    window.__ANSWER__.artifact_preview = (args) => {
      const row = rows.find((one) => one.id === args.id);
      if (row?.kind !== "report") return { kind: "none", bytes: 0, truncated: false };
      const text = report(row.title);
      return { kind: "markdown", text, bytes: text.length, truncated: false };
    };
    const reports = rows.filter((one) => one.kind === "report" && one.origin.task);
    return { reports: reports.slice(0, 8).map((one) => one.id), task: reports[0].origin.task, vanished: gone.size };
  }, CATALOG);
}

async function heapBytes(cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  return (await cdp.send("Runtime.getHeapUsage")).usedSize;
}

/* What the document holds — read right after a collection (`heapBytes`), so what is counted is kept
 * — and how many card nodes the window has made so far (its own count; a pool that does its work
 * makes none after the first paint). */
async function heldByPage(page, cdp) {
  const { nodes, jsEventListeners } = await cdp.send("Memory.getDOMCounters");
  const cardsMade = await page.evaluate(() => (typeof artifactCardCreations === "number" ? artifactCardCreations : null));
  return { nodes, listeners: jsEventListeners, cardsMade };
}

/* `--without`: the window's functions of these names do nothing from here on. A name the window
 * does not have is left alone, so one list serves a tree before a feature and after it. */
function standDown(page, names) {
  return page.evaluate((asked) => asked.filter((name) => {
    if (typeof window[name] !== "function") return false;
    window[name] = () => {};
    return true;
  }), names);
}

const elementCount = (page) => page.evaluate(() => document.getElementsByTagName("*").length);

async function pageClocks(cdp) {
  const { metrics } = await cdp.send("Performance.getMetrics");
  const read = new Map(metrics.map((one) => [one.name, one.value]));
  return Object.fromEntries(Object.keys(PAGE_CLOCKS).map((clock) => [clock, read.get(clock) ?? 0]));
}

/* One round's share of what the clocks moved: durations in ms, counts as they are. */
function clocksPerRound(from, to, rounds) {
  return Object.fromEntries(Object.entries(PAGE_CLOCKS).map(([clock, name]) => {
    const moved = (to[clock] - from[clock]) / rounds;
    return [name, name.endsWith("Ms") ? moved * 1000 : moved];
  }));
}

/* `--count`: every observer, listener, timer and frame the page asks for is counted from its first
 * script on — the counts only, nothing the page does is changed. A cost that grows with the rounds
 * has to come from something that is made each round and kept, or called more each round. */
function countPageWork(page) {
  return page.addInitScript(() => {
    const made = (window.__PERF_MADE__ = {});
    const count = (name) => {
      made[name] = (made[name] ?? 0) + 1;
    };
    const counted = (Base, word) => class extends Base {
      constructor(callback, ...rest) {
        super((...heard) => {
          count(`${word}Calls`);
          return callback(...heard);
        }, ...rest);
        count(`${word}Observers`);
      }

      observe(...asked) {
        count(`${word}Observed`);
        return super.observe(...asked);
      }
    };
    window.ResizeObserver = counted(window.ResizeObserver, "resize");
    window.MutationObserver = counted(window.MutationObserver, "mutation");
    window.IntersectionObserver = counted(window.IntersectionObserver, "intersection");
    const listen = EventTarget.prototype.addEventListener;
    const unlisten = EventTarget.prototype.removeEventListener;
    EventTarget.prototype.addEventListener = function addCounted(...asked) {
      count("listenersAdded");
      return listen.apply(this, asked);
    };
    EventTarget.prototype.removeEventListener = function removeCounted(...asked) {
      count("listenersRemoved");
      return unlisten.apply(this, asked);
    };
    for (const [name, word] of [["setTimeout", "timeouts"], ["setInterval", "intervals"], ["requestAnimationFrame", "frames"]]) {
      const ask = window[name].bind(window);
      window[name] = (...asked) => {
        count(word);
        return ask(...asked);
      };
    }
  });
}

/* The counts so far, or null on a page that is not counting. */
const pageWork = (page) => page.evaluate(() => (window.__PERF_MADE__ ? { ...window.__PERF_MADE__ } : null));

function workPerRound(from, to, rounds) {
  if (!from || !to) return {};
  return { made: Object.fromEntries(Object.keys(to).map((name) => [name, (to[name] - (from[name] ?? 0)) / rounds])) };
}

/* The heaviest functions of a sampled profile: by their own time, and by their time with everything
 * they called (counted once for the outermost call of a function that calls itself). Work the
 * renderer does outside any script — style and layout between frames — is `(program)`. */
function heaviest(profile) {
  const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
  const sampled = new Map();
  profile.samples.forEach((id, at) => sampled.set(id, (sampled.get(id) ?? 0) + (profile.timeDeltas[at] ?? 0)));
  const nameOf = ({ callFrame }) => {
    const file = callFrame.url ? callFrame.url.split("/").pop().split("?")[0] : "";
    const name = callFrame.functionName || "(anonymous)";
    return file ? `${name} ${file}:${callFrame.lineNumber + 1}` : name;
  };
  const own = new Map();
  const whole = new Map();
  const walk = (node, open) => {
    const name = nameOf(node);
    let micros = sampled.get(node.id) ?? 0;
    own.set(name, (own.get(name) ?? 0) + micros);
    const outermost = !open.has(name);
    if (outermost) open.add(name);
    for (const child of node.children ?? []) micros += walk(nodes.get(child), open);
    if (outermost) {
      open.delete(name);
      whole.set(name, (whole.get(name) ?? 0) + micros);
    }
    return micros;
  };
  const total = walk(profile.nodes[0], new Set());
  const top = (held) => [...held]
    .filter(([name]) => name !== "(root)")
    .sort((a, b) => b[1] - a[1])
    .slice(0, PROFILE_TOP)
    .map(([name, micros]) => ({ name, ms: micros / 1000 }));
  return { sampledMs: total / 1000, own: top(own), whole: top(whole) };
}

/* Runs `run`, under the sampling profiler when one is wanted; answers the heaviest functions or null. */
async function profiled(cdp, wanted, run) {
  if (!wanted) {
    await run();
    return null;
  }
  await cdp.send("Profiler.enable");
  await cdp.send("Profiler.setSamplingInterval", { interval: PROFILE_INTERVAL_US });
  await cdp.send("Profiler.start");
  await run();
  const { profile } = await cdp.send("Profiler.stop");
  return heaviest(profile);
}

/* `rounds` rounds of `once` (one round, answering its ms), read as one reading: the median, one
 * round's share of the renderer's clocks and of what the page asked for, and — when a profile is
 * wanted — the heaviest functions. */
async function readRounds(page, cdp, { rounds, once, profile = false }) {
  const clocks = await pageClocks(cdp);
  const made = await pageWork(page);
  const taken = [];
  const heavy = await profiled(cdp, profile, async () => {
    for (let at = 0; at < rounds; at += 1) taken.push(await once(at));
  });
  return {
    taken,
    reading: {
      p50Ms: median(taken),
      ...clocksPerRound(clocks, await pageClocks(cdp), rounds),
      ...workPerRound(made, await pageWork(page), rounds),
      ...(heavy ? { heavy } : {}),
    },
  };
}

/* N rounds of one opening and closing, read in ten parts. `warm` walks the same things once first,
 * so the start already holds whatever the caches keep; `once(at)` is round `at`, answering the ms of
 * its opening. The heap and the element count say whether anything is kept; the parts say whether
 * the same round costs more the later it runs, and the first and the last part are profiled when a
 * profile is wanted. */
async function soakOf(page, cdp, { rounds, warm, once, profile }) {
  await warm();
  const heapStart = await heapBytes(cdp);
  const heldStart = await heldByPage(page, cdp);
  const elementsStart = await elementCount(page);
  const size = Math.ceil(rounds / SOAK_PARTS);
  const opens = [];
  const parts = [];
  for (let from = 0; from < rounds; from += size) {
    const to = Math.min(rounds, from + size);
    const edge = from === 0 || to === rounds;
    const { taken, reading } = await readRounds(page, cdp, { rounds: to - from, once: (at) => once(from + at), profile: profile && edge });
    opens.push(...taken);
    parts.push(reading);
  }
  const heapEnd = await heapBytes(cdp);
  const heldEnd = await heldByPage(page, cdp);
  return {
    soakRounds: rounds,
    soakOpenP50Ms: median(opens),
    soakOpenP95Ms: quantile(opens, 0.95),
    soakFirstTenthP50Ms: median(opens.slice(0, size)),
    soakLastTenthP50Ms: median(opens.slice(-size)),
    soakParts: parts,
    heapStartKb: heapStart / 1024,
    heapEndKb: heapEnd / 1024,
    heapGrowthKb: (heapEnd - heapStart) / 1024,
    elementsStart,
    elementsEnd: await elementCount(page),
    listenersStart: heldStart.listeners,
    listenersEnd: heldEnd.listeners,
    nodesStart: heldStart.nodes,
    nodesEnd: heldEnd.nodes,
    cardsMadeStart: heldStart.cardsMade,
    cardsMadeEnd: heldEnd.cardsMade,
  };
}

export async function measureArtifacts(page, { soak = 0, attribute = 0, profile = false, without = [] } = {}) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const fixture = await standCatalog(page);
  const stoodDown = without.length > 0 ? await standDown(page, without) : [];
  const first = await page.evaluate(async () => {
    dropTab("artifacts");
    artifactFilter.tab = "pages";
    artifactFilter.origin = null;
    artifactFilter.showMissing = false;
    artifactTabPicked = false;
    artifactSelectedId = null;
    const from = performance.now();
    el("nav-artifacts").click();
    for (let beat = 0; beat < 600 && (artifactAsking || artifactRows.size === 0); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await window.__PERF_SETTLE__();
    const ms = performance.now() - from;
    const view = artifactsView();
    return {
      ms, held: artifactRows.size, asks: window.__PERF_ASKS__.slice(),
      cards: view.querySelectorAll(".artifact-card").length,
      elements: view.getElementsByTagName("*").length,
      width: Math.round(view.getBoundingClientRect().width),
    };
  });
  const reportsTab = await page.evaluate(async () => {
    const view = artifactsView();
    const from = performance.now();
    view.querySelector('[data-artifact-tab="reports"]').click();
    const js = performance.now() - from;
    await window.__PERF_SETTLE__();
    return { ms: performance.now() - from, js, shown: artifactOrder.length };
  });
  const filtered = await page.evaluate(async (task) => {
    const view = artifactsView();
    const asksBefore = window.__PERF_ASKS__.length;
    const from = performance.now();
    // The road a card's task chip takes: the origin filter, then the repaint.
    artifactFilter.origin = { field: "task", value: task, label: task };
    changeArtifactFilter(view);
    const js = performance.now() - from;
    await window.__PERF_SETTLE__();
    const ms = performance.now() - from;
    const shown = artifactOrder.length;
    artifactFilter.origin = null;
    changeArtifactFilter(view);
    await window.__PERF_SETTLE__();
    return { ms, js, shown, asks: window.__PERF_ASKS__.length - asksBefore };
  }, fixture.task);
  /* One opening of the drawer on one report: from the selection to the frame after its body stands. */
  const open = (id) => page.evaluate(async ({ id, frames }) => {
    const view = artifactsView();
    const md = view.querySelector(".artifact-preview-md");
    const from = performance.now();
    selectArtifact(view, id);
    for (let beat = 0; beat < frames; beat += 1) {
      const detail = view.querySelector(".artifacts-detail");
      if (detail.dataset.artifactId === id && !md.hidden && md.childElementCount > 0 && md._painted?.includes(artifactRows.get(id).title)) break;
      await new Promise((done) => requestAnimationFrame(done));
    }
    await window.__PERF_SETTLE__();
    return performance.now() - from;
  }, { id, frames: PAINT_FRAMES_MAX });
  const close = () => page.evaluate(async () => {
    const view = artifactsView();
    artifactSelectedId = null;
    paintArtifactCards(view);
    paintArtifactDrawer(view);
    await new Promise((done) => requestAnimationFrame(done));
  });
  const drawerMs = [];
  for (const id of fixture.reports.slice(0, SOAK_REPORTS)) {
    drawerMs.push(await open(id));
    await close();
  }
  const drawer = await page.evaluate(() => {
    const pane = artifactsView().querySelector(".artifacts-drawer");
    return { width: Math.round(pane.getBoundingClientRect().width) };
  });
  const result = {
    ...(without.length > 0 ? { without: stoodDown } : {}),
    tabWidth: first.width,
    firstPaintMs: first.ms,
    heldRows: first.held,
    listingRows: first.asks[0]?.rows ?? null,
    listingBytes: first.asks[0]?.bytes ?? null,
    listingAskedPresent: first.asks[0]?.present ?? null,
    listingAsks: first.asks.length,
    firstCards: first.cards,
    firstElements: first.elements,
    reportsTabMs: reportsTab.ms,
    reportsTabJsMs: reportsTab.js,
    reportsShown: reportsTab.shown,
    filterMs: filtered.ms,
    filterJsMs: filtered.js,
    filterShown: filtered.shown,
    filterAsks: filtered.asks,
    drawerOpenMs: median(drawerMs),
    drawerOpenMaxMs: Math.max(...drawerMs),
    drawerWidth: drawer.width,
  };
  /* One round of the drawer: open report `at` of the five, read, close. */
  const drawerRound = async (at) => {
    const ms = await open(fixture.reports[at % SOAK_REPORTS]);
    await close();
    return ms;
  };
  if (attribute > 0) {
    const read = async (once) => (await readRounds(page, cdp, { rounds: attribute, once, profile })).reading;
    result.attributed = {
      rounds: attribute,
      drawerOpen: await read(drawerRound),
      // To the reports from the pages, and back before the next round.
      reportsTab: await read(() => page.evaluate(async () => {
        const view = artifactsView();
        view.querySelector('[data-artifact-tab="pages"]').click();
        await window.__PERF_SETTLE__();
        const from = performance.now();
        view.querySelector('[data-artifact-tab="reports"]').click();
        await window.__PERF_SETTLE__();
        return performance.now() - from;
      })),
      taskFilter: await read(() => page.evaluate(async (task) => {
        const view = artifactsView();
        const from = performance.now();
        artifactFilter.origin = { field: "task", value: task, label: task };
        changeArtifactFilter(view);
        await window.__PERF_SETTLE__();
        const ms = performance.now() - from;
        artifactFilter.origin = null;
        changeArtifactFilter(view);
        await window.__PERF_SETTLE__();
        return ms;
      }, fixture.task)),
    };
  }
  if (soak > 0) {
    Object.assign(result, await soakOf(page, cdp, {
      rounds: soak,
      warm: async () => {
        for (let at = 0; at < SOAK_REPORTS; at += 1) await drawerRound(at);
      },
      once: drawerRound,
      profile,
    }));
  }
  await cdp.detach();
  return result;
}

export async function measureArtifactRounds({ rounds = 5, soak = 0, attribute = 0, profile = false, count = false, without = [] } = {}) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const taken = [];
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin, { before: count ? countPageWork : null });
      try {
        await page.setViewportSize(REAL_WINDOW);
        // The soak and the attribution run once, on the last round: they are questions about growth
        // and about where the time goes, not about a median.
        const last = round === rounds - 1;
        taken.push(await measureArtifacts(page, { soak: last ? soak : 0, attribute: last ? attribute : 0, profile, without }));
        if (faults.length) taken.at(-1).faults = faults.slice(0, 3);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const numeric = Object.keys(taken[0]).filter((key) => typeof taken[0][key] === "number");
  const summary = Object.fromEntries(numeric.map((key) => [key, median(taken.map((one) => one[key]).filter((held) => typeof held === "number"))]));
  const last = taken.at(-1);
  // What the soak read is the last round's, as it stands — never a median with rounds that ran no soak.
  const ofTheSoak = (key) => /^(soak|heap|elements|listeners|nodes|cardsMade)/.test(key);
  for (const key of Object.keys(last)) if (!(key in summary) || ofTheSoak(key)) summary[key] = last[key];
  return { catalog: CATALOG, throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1), rounds, load: loadNote(), summary, taken };
}

const direct = process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (direct) {
  const option = (name, fallback) => {
    const at = process.argv.indexOf(name);
    return at > 0 && process.argv[at + 1] ? process.argv[at + 1] : fallback;
  };
  const measured = await measureArtifactRounds({
    rounds: Number(option("--rounds", 5)),
    soak: Number(option("--soak", SOAK_ROUNDS)),
    attribute: Number(option("--attribute", 0)),
    profile: process.argv.includes("--profile"),
    count: process.argv.includes("--count"),
    without: option("--without", "").split(",").filter(Boolean),
  });
  const json = option("--json", "");
  if (json) writeFileSync(json, `${JSON.stringify(measured, null, 1)}\n`);
  console.log(JSON.stringify({ ...measured, taken: undefined }, null, 1));
}
