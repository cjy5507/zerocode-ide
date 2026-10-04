import { writeFileSync } from "node:fs";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* What the explain action costs the page, before and after (t-32787).
 *
 * The same script runs on the tree without the action (the "before": only the
 * generic rows are measured) and on the tree with it, so that every number in
 * the report is a pair. `WINDOW_CPU_THROTTLE=4` is the page's CPU slowed 4x
 * (the low-spec `cpu4x` level); under `taskpolicy -b` the whole process runs on
 * the efficiency cores. Medians over repeated runs inside the page, never one
 * run; the page is a fresh document so nothing here inherits a neighbour.
 *
 *   node ui/tests/explain-perf.mjs [--label after] [--json out.json]
 *
 * Rows:
 *   boot      DOM nodes, documents and event listeners right after boot (CDP counters)
 *   diff      what a diff view costs: counters after opening it, and the median
 *             time of one repaint (`paintDiffView`, which now also keeps the button)
 *   pointer   the conversation list's pointerover handler's cost per event (the
 *             document keeps its one tooltip listener per gesture), and the
 *             nodes a conversation of answers gains when the pointer touches them
 *   card      (after only) the card's open latency, its nodes and listeners, and
 *             the long tasks seen while it opened
 *   growth    (after only) heap after hundreds of card open/close and request
 *             cycles, after a collection, against the heap before them */

const RUNS = 5;
const REPAINTS = 200;
const POINTER_EVENTS = 4000;
const TURNS = 120;
const CARD_OPENS = 30;
const CYCLES = 300;

const args = process.argv.slice(2);
const flag = (name, fallback = null) => {
  const at = args.indexOf(name);
  return at >= 0 && at + 1 < args.length ? args[at + 1] : fallback;
};

const median = (list) => {
  const sorted = [...list].sort((a, b) => a - b);
  return sorted.length ? sorted[Math.floor(sorted.length / 2)] : null;
};
const p95 = (list) => {
  const sorted = [...list].sort((a, b) => a - b);
  return sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] : null;
};

async function counters(cdp) {
  const { documents, nodes, jsEventListeners } = await cdp.send("Memory.getDOMCounters");
  return { documents, nodes, listeners: jsEventListeners };
}

async function heap(cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  await cdp.send("HeapProfiler.collectGarbage");
  const { usedSize } = await cdp.send("Runtime.getHeapUsage");
  return usedSize;
}

export async function measure(browser, origin) {
  const { page } = await openWindowTestPage(browser, origin);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.enable");
  const out = { throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1) };
  try {
    await page.evaluate(() => new Promise((done) => setTimeout(done, 600)));
    out.has = await page.evaluate(() => typeof ensureExplainButton === "function");

    /* ---- boot ---- */
    out.boot = await counters(cdp);

    /* ---- diff ---- */
    const diffTimes = [];
    await page.evaluate(async () => {
      const term = await openTermTab();
      window.__AGENT_TERMS__ = [[term, "claude"]];
      paneAgents.set(term, "claude");
      setDiffSideBySide(false);
      await openDiff("src/a.rs");
      await new Promise((done) => setTimeout(done, 300));
    });
    out.diffOpen = await counters(cdp);
    for (let run = 0; run < RUNS; run += 1) {
      diffTimes.push(await page.evaluate((repaints) => {
        const view = [...document.querySelectorAll(".file-view")].find((one) => one.querySelector(".diff-merge") && !one.hidden);
        const tab = tabs.find((one) => one.id === view.dataset.tab);
        const t0 = performance.now();
        for (let n = 0; n < repaints; n += 1) paintDiffView(tab);
        return (performance.now() - t0) / repaints;
      }, REPAINTS));
    }
    out.diff = { repaintMs: median(diffTimes), repaintRuns: diffTimes.map((one) => Number(one.toFixed(4))) };
    out.diff.buttons = await page.evaluate(() => document.querySelectorAll(".explain-open").length);

    /* ---- pointer: the list's handler, and the nodes a touched conversation gains ---- */
    await page.evaluate(async (turns) => {
      const term = await openTermTab({ placement: "tab" });
      window.__AGENT_TERMS__ = [[term, "zo"]];
      paneAgents.set(term, "zo");
      hookStates.set(term, "working");
      const list = [];
      for (let n = 0; n < turns / 2; n += 1) {
        list.push({ role: "user", text: `질문 ${n}` });
        list.push({ role: "assistant", text: `답 ${n}\n\n둘째 문단 ${n}` });
      }
      window.__ANSWER__.pane_log = () => ({ found: true, next: turns + 1, skipped: false, more: false, folded: false, model: "claude-opus-5", turns: list });
      await setPaneChat(term, true);
      await new Promise((done) => setTimeout(done, 800));
    }, TURNS);
    out.chatOpen = await counters(cdp);
    out.answers = await page.evaluate(() => document.querySelectorAll(".helper-turn.is-assistant").length);
    const eventMs = [];
    for (let run = 0; run < RUNS; run += 1) {
      eventMs.push(await page.evaluate((events) => {
        const target = document.querySelector(".helper-turns") ?? document.body;
        const t0 = performance.now();
        for (let n = 0; n < events; n += 1) target.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        return ((performance.now() - t0) / events) * 1000;
      }, POINTER_EVENTS));
    }
    out.pointer = { microsPerEvent: median(eventMs), runs: eventMs.map((one) => Number(one.toFixed(3))) };
    const touched = await page.evaluate(() => {
      const rows = [...document.querySelectorAll(".helper-turn.is-assistant")];
      const t0 = performance.now();
      for (const row of rows) row.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      return { rows: rows.length, ms: performance.now() - t0, buttons: document.querySelectorAll(".helper-actions .explain-open").length };
    });
    out.touched = { ...touched, microsPerRow: touched.rows ? (touched.ms / touched.rows) * 1000 : null };
    out.chatTouched = await counters(cdp);

    if (!out.has) return out;

    /* ---- card ---- */
    await page.evaluate(() => {
      window.__ANSWER__.explain_roads = () => [{ agent: "claude" }, { agent: "codex" }];
      window.__ANSWER__.explain_preview = () => ({ lines: 3, masked: 2, clipped: false });
      window.__ANSWER__.explain_start = () => null;
      window.__ANSWER__.explain_cancel = () => true;
      window.__LONG__ = 0;
      new PerformanceObserver((list) => { window.__LONG__ += list.getEntries().length; }).observe({ entryTypes: ["longtask"] });
    });
    const open = [];
    for (let n = 0; n < CARD_OPENS; n += 1) {
      open.push(await page.evaluate(async () => {
        const answer = document.querySelector(".helper-turn.is-assistant");
        const button = answer.querySelector(".explain-open");
        const t0 = performance.now();
        await openExplainCard(button);
        const ms = performance.now() - t0;
        closeNoteSend();
        await new Promise((done) => setTimeout(done, 200));
        return ms;
      }));
    }
    out.card = { openMs: median(open), openP95Ms: p95(open), longTasks: await page.evaluate(() => window.__LONG__) };
    await page.evaluate(async () => {
      const answer = document.querySelector(".helper-turn.is-assistant");
      await openExplainCard(answer.querySelector(".explain-open"));
    });
    out.cardOpen = await counters(cdp);
    await page.evaluate(() => closeNoteSend());
    await page.evaluate(() => new Promise((done) => setTimeout(done, 300)));
    out.cardClosed = await counters(cdp);

    /* ---- growth: many cards and many requests, then the heap after a collection ---- */
    const before = await heap(cdp);
    await page.evaluate(async (cycles) => {
      const answer = document.querySelector(".helper-turn.is-assistant");
      const button = answer.querySelector(".explain-open");
      for (let n = 0; n < cycles; n += 1) {
        await openExplainCard(button);
        closeNoteSend();
        const id = `explain-perf-${n}`;
        explainRequests.set(id, { id, agent: "Claude", state: "sent", route: { via: "conversation", term: 1 } });
        paintExplainStatus();
        noteExplainState({ id, state: "failed", why: "no_page" });
      }
      for (const note of document.querySelectorAll(".toast")) note.remove();
    }, CYCLES);
    await page.evaluate(() => new Promise((done) => setTimeout(done, 400)));
    const after = await heap(cdp);
    out.growth = {
      cycles: CYCLES,
      heapBeforeKiB: Math.round(before / 1024),
      heapAfterKiB: Math.round(after / 1024),
      deltaKiB: Math.round((after - before) / 1024),
      requestsLeft: await page.evaluate(() => explainRequests.size),
      toastsLeft: await page.evaluate(() => document.querySelectorAll(".toast").length),
    };
    out.final = await counters(cdp);
    return out;
  } finally {
    await page.close();
  }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true, args: ["--disable-3d-apis", "--enable-precise-memory-info", "--js-flags=--expose-gc"] });
  try {
    const result = await measure(browser, origin);
    result.label = flag("--label", "run");
    const text = JSON.stringify(result, null, 2);
    const to = flag("--json");
    if (to) writeFileSync(to, `${text}\n`);
    console.log(text);
  } finally {
    await browser.close();
    files.close();
  }
}
