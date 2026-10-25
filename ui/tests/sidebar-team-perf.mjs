/* What the sidebar costs over one synthetic team (t-56141) — the numbers the team view is held to.
 *
 * The scene is the `sidebar-team` suite's: three coordinators, five workers, thirteen checkouts, built from the
 * window's own roads over a stub backend (`standTeamScene`). The before is the base commit's `ui/` unpacked with
 * this file and the scene copied in (`git archive <base> ui | tar -x`, then copy `sidebar-team.mjs` and this
 * file) — that window has no team view, so the same scene draws its branch list; the after is this tree, whose
 * window draws the team. The same file measures both, so the difference is the change.
 *
 * Numbers (medians over rounds, each round a fresh page):
 *   rows        what a person reads: the team's cards and rows, or the branch list's rows and agent rows
 *   elements    elements under the sidebar's scroller
 *   drawMs      the whole list drawn again from what the window holds — the branch list's rebuild, or the
 *               team's — with the layout it forces, in JS ms
 *   per beat    for four beats of the ledger — nothing moved, a worker handed in, a worker sent a status letter,
 *               an account switch moved a worker's pane — the rows written (distinct rows a mutation touched)
 *               and the paint JS (the agent paint's flush and the team's own paint, summed)
 *   heap        the page's JS heap after two collections, and the DOM nodes and listeners (Chromium)
 *
 *   node ui/tests/sidebar-team-perf.mjs [--rounds 5] [--json out.json]
 *   node ui/tests/sidebar-team-perf.mjs --beats        eight beats of the family scene (t-60498), team and branch list
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/sidebar-team-perf.mjs          low spec (cpu ×4, Chromium)
 *   taskpolicy -b node ui/tests/sidebar-team-perf.mjs                  efficiency cores */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { median } from "./coordinator-desk-perf.mjs";
import { loadNote } from "./machine-load.mjs";
import { FAMILY_RUN, FAMILY_WORKERS, playTeamBeats, standTeamScene } from "./sidebar-team.mjs";

/* One beat of the ledger, after `change` rewrote the rows the stub answers: the rows a mutation touched and the
 * paint JS the beat cost. */
async function measureBeat(page, change) {
  return page.evaluate(async (change) => {
    if (change === "handIn") {
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-a1"
        ? { ...row, reported: true, handed_in: "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", progress: { stage: "report_only" } } : row);
    } else if (change === "status") {
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-b2"
        ? { ...row, last_status: { message_id: `m-${performance.now()}`, created_ms: Date.now(), line: "착지 묶음 들어감" } } : row);
    } else if (change === "account") {
      window.__LEDGER__ = window.__LEDGER__.map((row) => row.worker === "w-a2" ? { ...row, pane: row.pane === "%3" ? "%7" : "%3" } : row);
    }
    const host = document.getElementById("sidebar-scroll");
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(host, { subtree: true, childList: true, attributes: true, characterData: true });
    window.__PERF_JS__ = 0;
    await window.__LEDGER_MOVED__();
    records.push(...watch.takeRecords());
    watch.disconnect();
    const rows = new Set();
    for (const record of records) {
      const at = record.target.nodeType === Node.ELEMENT_NODE ? record.target : record.target.parentElement;
      const row = at?.closest?.(".team-row, .team-mine, .team-foot, .wt-agent, .wt-row, .wt-node, .proj") ?? null;
      if (row) rows.add(row);
    }
    return { rows: rows.size, mutations: records.length, js: window.__PERF_JS__ };
  }, change);
}

async function measureSidebar(page, { now }) {
  await standTeamScene(page, { now });
  // The paint JS of a beat: the agent paint's flush (the branch list's rows) and the team's own paint, timed where
  // the window calls them — both are global functions the beat calls by name.
  // A paint inside another (the flush calls the team's) is counted once, by the outermost.
  await page.evaluate(() => {
    window.__PERF_JS__ = 0;
    let depth = 0;
    for (const name of ["flushAgentPaint", "paintSidebarTeam"]) {
      const held = window[name];
      if (typeof held !== "function") continue;
      window[name] = function timed(...args) {
        const started = performance.now();
        depth += 1;
        try {
          return held.apply(this, args);
        } finally {
          depth -= 1;
          if (depth === 0) window.__PERF_JS__ += performance.now() - started;
        }
      };
    }
  });
  const rows = await page.evaluate(() => window.__ROWS__());
  // The whole list again, from what the window holds.
  const drawMs = await page.evaluate(async () => {
    const team = typeof sidebarTeamStood !== "undefined" && sidebarTeamStood;
    worktreeListDrawn = null;
    if (team) {
      sidebarTeamNodes.clear();
      document.getElementById("sidebar-team").replaceChildren();
    }
    const started = performance.now();
    await refreshWorktrees();
    if (team) paintSidebarTeam();
    void document.getElementById("sidebar-scroll").offsetHeight;
    return performance.now() - started;
  });
  const beats = {};
  for (const change of ["quiet", "handIn", "status", "account"]) beats[change] = await measureBeat(page, change);
  return { rows, drawMs, beats };
}

export async function measureSidebarTeam({ rounds = 5 } = {}) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const taken = [];
  const now = Date.now();
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width: 1280, height: 1000 });
        const row = await measureSidebar(page, { now });
        const cdp = await page.context().newCDPSession(page);
        await cdp.send("HeapProfiler.collectGarbage");
        await cdp.send("HeapProfiler.collectGarbage");
        const usage = await cdp.send("Runtime.getHeapUsage");
        const counters = await cdp.send("Memory.getDOMCounters");
        row.heap = { js: usage.usedSize, domNodes: counters.nodes, listeners: counters.jsEventListeners };
        if (faults.length) row.faults = faults.slice(0, 3);
        taken.push(row);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const pick = (read) => median(taken.map(read));
  const summary = {
    engine: "chromium",
    cpuThrottle: Number(process.env.WINDOW_CPU_THROTTLE ?? "1") || 1,
    rounds,
    load: loadNote(),
    view: taken[0].rows.team > 0 ? "team" : "branches",
    rows: taken[0].rows.team > 0 ? taken[0].rows.coordinators + taken[0].rows.workers : taken[0].rows.branches,
    agentRows: taken[0].rows.agents,
    elements: pick((one) => one.rows.elements),
    drawMs: pick((one) => one.drawMs),
    beats: Object.fromEntries(["quiet", "handIn", "status", "account"].map((change) => [change, {
      rows: pick((one) => one.beats[change].rows),
      mutations: pick((one) => one.beats[change].mutations),
      jsMs: pick((one) => one.beats[change].js),
    }])),
    heapMiB: pick((one) => one.heap.js) / 1048576,
    domNodes: pick((one) => one.heap.domNodes),
    listeners: pick((one) => one.heap.listeners),
    faults: taken.flatMap((one) => one.faults ?? []).slice(0, 3),
  };
  return { summary, taken };
}

/* Eight beats of the family scene, a minute apart (`playTeamBeats`): the words that changed, the rows that moved and
 * grew, the rows written and built — the team closed and opened — and the branch list's rows, with the task id on the
 * catalog's rows and without. The base commit's window answers the same script (it has no family row, so the four
 * workers of one root stand as four rows). */
export async function measureFamilyBeats() {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const out = {};
  try {
    for (const open of [false, true]) {
      const { page } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width: 1280, height: 1600 });
        await standTeamScene(page, { now: Date.now(), runs: [FAMILY_RUN], workers: FAMILY_WORKERS });
        out[open ? "open" : "closed"] = await playTeamBeats(page, { open });
      } finally {
        await page.close();
      }
    }
    for (const taskIds of [false, true]) {
      const { page } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width: 1280, height: 1700 });
        await standTeamScene(page, { now: Date.now(), runs: [FAMILY_RUN], workers: FAMILY_WORKERS, taskIds,
          extras: ["/r/wt-f1-verify", "/r/wt-f1-windows", "/r/wt-g1-red", "/r/old-1"] });
        out[taskIds ? "branchesWithTaskIds" : "branchesNoTaskIds"] = await page.evaluate(async () => {
          setSidebarView({ layout: "branches" });
          await window.__TEAM_SETTLED__();
          await new Promise((done) => setTimeout(done, 300));
          return { rows: window.__ROWS__().branches, chips: [...document.querySelectorAll("#worktrees .wt-fold")]
            .filter((node) => !node.hidden).map((node) => node.textContent) };
        });
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  return out;
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href && process.argv.includes("--beats")) {
  const found = await measureFamilyBeats();
  console.log(`SIDEBAR_TEAM_BEATS ${JSON.stringify(found)}`);
  process.exit(0);
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const { summary, taken } = await measureSidebarTeam({ rounds: Number(option("--rounds", "5")) });
  const ms = (value) => `${value.toFixed(2)} ms`;
  console.log(`chromium · cpu ×${summary.cpuThrottle} · ${summary.rounds} rounds (medians) · ${summary.load}`);
  console.log(`  ${summary.view}: rows ${summary.rows} (agent rows ${summary.agentRows}) · elements ${summary.elements} · whole list drawn ${ms(summary.drawMs)}`);
  for (const [change, beat] of Object.entries(summary.beats)) {
    console.log(`  beat ${change.padEnd(7)} rows written ${beat.rows} · mutations ${beat.mutations} · paint JS ${ms(beat.jsMs)}`);
  }
  console.log(`  JS heap after GC ${summary.heapMiB.toFixed(2)} MiB · DOM nodes ${summary.domNodes} · listeners ${summary.listeners}`);
  if (summary.faults.length) console.log(`  FAULTS ${summary.faults.join(" | ")}`);
  console.log(`SIDEBAR_TEAM_NUMBERS ${JSON.stringify(summary)}`);
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify({ summary, taken }, null, 2)}\n`);
  if (summary.faults.length) process.exitCode = 1;
}
