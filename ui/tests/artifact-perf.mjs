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
 *      N rounds of opening and closing the drawer over five reports; and the ms of each opening.
 *
 * Run (`WINDOW_CPU_THROTTLE=4` slows the page's CPU four times — the low-spec level cpu4x):
 *   node ui/tests/artifact-perf.mjs                               Chromium, median of 5 rounds
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/artifact-perf.mjs
 *   node ui/tests/artifact-perf.mjs --rounds 3 --soak 500 --json out.json */
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

export async function measureArtifacts(page, { soak = 0 } = {}) {
  const cdp = await page.context().newCDPSession(page);
  const fixture = await standCatalog(page);
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
  if (soak > 0) {
    const elements = () => page.evaluate(() => document.getElementsByTagName("*").length);
    // Warm: one walk over the reports first, so the soak's start already holds whatever the caches keep.
    for (const id of fixture.reports.slice(0, SOAK_REPORTS)) {
      await open(id);
      await close();
    }
    const heapStart = await heapBytes(cdp);
    const elementsStart = await elements();
    const opens = [];
    for (let round = 0; round < soak; round += 1) {
      opens.push(await open(fixture.reports[round % SOAK_REPORTS]));
      await close();
    }
    const heapEnd = await heapBytes(cdp);
    Object.assign(result, {
      soakRounds: soak,
      soakOpenP50Ms: median(opens),
      soakOpenP95Ms: quantile(opens, 0.95),
      soakFirstTenthP50Ms: median(opens.slice(0, Math.ceil(soak / 10))),
      soakLastTenthP50Ms: median(opens.slice(-Math.ceil(soak / 10))),
      heapStartKb: heapStart / 1024,
      heapEndKb: heapEnd / 1024,
      heapGrowthKb: (heapEnd - heapStart) / 1024,
      elementsStart,
      elementsEnd: await elements(),
    });
  }
  await cdp.detach();
  return result;
}

export async function measureArtifactRounds({ rounds = 5, soak = 0 } = {}) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const taken = [];
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize(REAL_WINDOW);
        // The soak runs once, on the last round: it is a question about growth, not about a median.
        taken.push(await measureArtifacts(page, { soak: round === rounds - 1 ? soak : 0 }));
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
  for (const key of Object.keys(last)) if (!(key in summary) || key.startsWith("soak") || key.startsWith("heap") || key.startsWith("elements")) summary[key] = last[key];
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
  });
  const json = option("--json", "");
  if (json) writeFileSync(json, `${JSON.stringify(measured, null, 1)}\n`);
  console.log(JSON.stringify({ ...measured, taken: undefined }, null, 1));
}
