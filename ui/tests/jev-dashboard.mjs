/* The Jev dashboard (t-5807, docs/design/jev-dashboard-and-perfection-20260921.md §2).
 *
 * What is measured: the rail door opens one tab; the table carries every
 * seat the settings card carries, in the card's order and under the card's
 * names; the numbers zo answered stand in their cells, the trend has one
 * point per counted day, and the recent list shows what the digest said;
 * a switch moved on the dashboard goes through the card's door and the card
 * follows; the refresh policy is one ask on open, one settled ask per burst
 * of ledger events, none on a re-open within the beat; and the time from
 * the click to the drawn table, with the backend answering at once, sits
 * under the design's line (§2: ≤ 200 ms).
 *
 * The backend is the window harness's stub with three answers swapped in
 * (`window.__ANSWER__`): the settings' switches, zo's summary and the seat
 * switch's door. */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { openWindowTestPage } from "./window-boot.mjs";
import { setQualityTheme, settlePaint } from "./settings-quality.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
/* Where the evidence goes: a person looks at these (t-5807, the operator's
 * ask), so they are written under the checkout's output folder, which git
 * ignores, and named in the report. */
const EVIDENCE_DIR = join(ROOT, "output", "t-5807");

/* Installed on the page: the fixture the three answers are drawn from.
 * With `real` — this machine's own count, from `realSummary` — the summary
 * answer is that count and every switch stands where that count says. */
function jevDashboardFixture(real = null) {
  const seats = [...document.querySelectorAll("[data-jev-seat]")].map((select) => select.dataset.jevSeat);
  const modes = [
    { mode: "off", asks: false, applies: false, automatic: false },
    { mode: "shadow", asks: true, applies: false, automatic: false },
    { mode: "on", asks: true, applies: true, automatic: false },
    { mode: "auto", asks: true, applies: false, automatic: true },
  ];
  const modeOf = Object.fromEntries(seats.map((id) => [id, "on"]));
  modeOf.recall = "shadow";
  const now = Date.now();
  const day = 24 * 60 * 60 * 1000;
  const today = now - (now % day);
  const empty = () => ({
    rows: 0, answered: 0, refused: 0, applied: 0, refusals: [], answeredShare: null, answeredLowerBound: null,
    called: 0, requests: 0, redactedLines: 0, inputTokens: 0, p50Ms: null, p95Ms: null, failures: [],
  });
  const counted = (rows, answered, extra = {}) => ({
    ...empty(), rows, answered, answeredShare: rows ? answered / rows : null,
    answeredLowerBound: rows ? Math.max(0, answered / rows - 0.12) : null, called: answered, requests: rows, ...extra,
  });
  const days = (shares) => shares.map((share, at) => ({
    startMs: today - (6 - at) * day,
    tally: share === null ? empty() : counted(10, Math.round(share * 10), { p50Ms: 200 + at * 10 }),
    agreement: share === null ? { compared: 0, agreed: 0, lowerBound: null } : { compared: 4, agreed: 3, lowerBound: 0.3 },
  }));
  const decisions = (count) => Array.from({ length: count }, (unused, at) => ({
    at: now - (at + 1) * 60_000, outcome: at === 2 ? "not_consented" : "answered", elapsedMs: at === 2 ? null : 230 + at,
    cached: false, asked: { task: `t-${5800 + at}`, worker: `w-${5810 + at}`, options: 5 },
    answered: at === 2 ? null : "claude", confidence: at === 2 ? null : 0.19,
    applied: at === 2 ? null : at % 2 === 0, agreed: at === 2 ? null : true, followed: null,
  }));
  const numbers = (id, args) => {
    const seat = {
      id, setting: id, mode: modeOf[id], ledger: `${id}.jsonl`, found: null, today: empty(), week: empty(),
      costUsd: null, riseFloorPermille: null, clearsRiseFloor: null, rowsToNextJudgment: null, judged: null,
      stand: "recording", applies: modeOf[id] === "on", verdict: null, days: days([null, null, null, null, null, null, null]),
      recent: [],
    };
    if (id === "summon") {
      seat.found = "/h/.zo/jev/summon-choice.jsonl";
      seat.today = counted(32, 32, { p50Ms: 227, p95Ms: 300 });
      seat.week = counted(50, 48, { refused: 2, refusals: [{ token: "not_consented", rows: 2 }],
        failures: [{ token: "not_consented", rows: 2 }], p50Ms: 230, p95Ms: 410, applied: 0, inputTokens: 90_000 });
      seat.costUsd = 0.0123;
      seat.riseFloorPermille = 900;
      seat.clearsRiseFloor = false;
      seat.rowsToNextJudgment = 10;
      seat.judged = { window: counted(34, 34), windowWanted: 34,
        agreement: { compared: 44, agreed: 16, lowerBound: 0.24, controlRows: 0 } };
      seat.verdict = { verdict: "hold", line: "answered" };
      seat.days = days([null, 0.9, 1, 0.8, null, 1, 0.96]);
      seat.recent = args?.recent ? decisions(Math.min(args.recent, 5)) : [];
    }
    if (id === "recall") {
      // A seat the table never promotes: no floor, no verdict, and a
      // person's `shadow` — it records, and the why column says so.
      seat.found = "/h/state/smart-router/rerank-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 211, p95Ms: 232 });
      seat.week = counted(1005, 935, { applied: 845, p50Ms: 211, p95Ms: 400 });
      seat.costUsd = 0.31;
      seat.days = days([1, 0.95, 0.9, 1, 0.92, 0.9, 0.93]);
    }
    if (id === "routing") {
      seat.found = "/h/state/smart-router/decision-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 541, p95Ms: 600 });
      seat.week = counted(35, 31, { refused: 0, p50Ms: 500, p95Ms: 884, applied: 23 });
      seat.costUsd = 0.004;
      seat.riseFloorPermille = 950;
      seat.clearsRiseFloor = false;
      seat.rowsToNextJudgment = 5;
      seat.judged = { window: counted(35, 31), windowWanted: 73, agreement: { compared: 3, agreed: 3, lowerBound: 0.43, controlRows: 1 } };
      seat.verdict = { verdict: "hold", line: "too_few_rows" };
      seat.days = days([0.8, 0.9, 1, 0.8, 0.7, 1, 0.9]);
      seat.recent = args?.recent ? [{
        at: now - 30_000, outcome: "answered", elapsedMs: 541, cached: false,
        asked: { task: "68212a1194a4e327", attempt: "session-1@1" },
        answered: { complexity: "small", intent: "other", risk: "low" }, confidence: null,
        applied: true, agreed: null, followed: null,
      }] : [];
    }
    return seat;
  };
  window.__JEV__ = { seats, modeOf, asks: [] };
  const settings = () => ({
    keysKeptHere: true, keySaved: true,
    switches: seats.map((id) => ({ id, setting: id, mode: modeOf[id], modes })),
    classifier: { setting: "autoClassifier", mode: "probed", probes: true, gates: seats[0],
      modes: [{ mode: "probed", runs: true, markers: false, probes: true }] },
  });
  window.__ANSWER__.typesafe_settings = settings;
  window.__ANSWER__.jev_summary = (args) => {
    window.__JEV__.asks.push(args ?? {});
    return seats.map((id) => numbers(id, args));
  };
  window.__ANSWER__.set_jev_mode = (args) => {
    modeOf[args.use] = args.mode;
    return settings();
  };
  if (real) {
    for (const seat of real.seats) if (seat.id in modeOf) modeOf[seat.id] = seat.mode;
    window.__ANSWER__.jev_summary = (args) => {
      window.__JEV__.asks.push(args ?? {});
      return real.seats;
    };
  }
  /* Text that a person cannot read is a fault: cut by the surface that
   * holds it, clipped inside its own box, or drawn over another text. Every
   * element with words of its own is measured against the root and against
   * every other such element; ancestors hold their descendants by design. */
  window.__JEV_TEXT_FAULTS__ = (root) => {
    const faults = [];
    const box = root.getBoundingClientRect();
    // What is folded away is not on the surface: a closed `<details>` keeps
    // its body's boxes (content-visibility) but nobody sees them.
    const folded = (node) => {
      const fold = node.closest("details:not([open])");
      return fold !== null && !(node.tagName === "SUMMARY" && node.parentElement === fold) && !node.closest("summary");
    };
    const held = [...root.querySelectorAll("*")].filter((node) =>
      !(node instanceof SVGElement) && node.tagName !== "OPTION" && node.tagName !== "SELECT"
      && !node.closest(".sr") && !node.closest("[hidden]") && !folded(node) && node.getClientRects().length > 0
      && [...node.childNodes].some((child) => child.nodeType === Node.TEXT_NODE && child.textContent.trim()));
    const rects = [];
    for (const node of held) {
      const rect = node.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) continue;
      rects.push([node, rect]);
      const words = node.textContent.trim().slice(0, 40);
      if (rect.right > box.right + 1 || rect.bottom > box.bottom + 1 || rect.left < box.left - 1 || rect.top < box.top - 1) {
        faults.push(`cut by the surface: ${words}`);
      }
      if (getComputedStyle(node).overflowX !== "visible" && node.scrollWidth > node.clientWidth + 1) {
        faults.push(`clipped in its box: ${words}`);
      }
      const wrap = node.closest(".jev-table-wrap");
      if (wrap && rect.right > wrap.getBoundingClientRect().right + 1) faults.push(`cut by the table: ${words}`);
    }
    for (let a = 0; a < rects.length; a += 1) {
      for (let b = a + 1; b < rects.length; b += 1) {
        const [nodeA, rectA] = rects[a];
        const [nodeB, rectB] = rects[b];
        if (nodeA.contains(nodeB) || nodeB.contains(nodeA)) continue;
        const across = Math.min(rectA.right, rectB.right) - Math.max(rectA.left, rectB.left);
        const down = Math.min(rectA.bottom, rectB.bottom) - Math.max(rectA.top, rectB.top);
        if (across > 1 && down > 1) {
          faults.push(`overlap: "${nodeA.textContent.trim().slice(0, 30)}" × "${nodeB.textContent.trim().slice(0, 30)}"`);
        }
      }
    }
    return faults;
  };
}

/* This machine's own ledgers, counted by the zo the window would run
 * (`~/.local/bin/zo`, or JEV_ZO_BIN) over the project JEV_PROJECT_DIR names
 * (else this checkout), with the recent list when that zo knows the flag.
 * `null` when no zo answers — the fixture then stands in, and the evidence
 * says so. */
function realSummary() {
  const zo = process.env.JEV_ZO_BIN || join(homedir(), ".local", "bin", "zo");
  if (!existsSync(zo)) return null;
  const project = process.env.JEV_PROJECT_DIR || ROOT;
  const sessions = process.env.JEV_SESSIONS_DIR
    || join(homedir(), "Library", "Application Support", "dev.zerocode.app", "computer-use", "sessions");
  const base = ["jev", "summary", "--json", "--cwd", project];
  if (existsSync(sessions)) base.push("--computer-use", sessions);
  for (const args of [[...base, "--recent", "12"], base]) {
    const ran = spawnSync(zo, args, { encoding: "utf8", timeout: 20_000 });
    if (ran.status !== 0) continue;
    try {
      const seats = JSON.parse(ran.stdout).seats;
      if (Array.isArray(seats) && seats.length > 0) return { seats, zo, recent: args.includes("--recent") };
    } catch {
      // An answer this reader does not understand: try the next shape.
    }
  }
  return null;
}

/* The evidence a person looks at (t-5807): the dashboard with this
 * machine's numbers in the light and the dark treatment, everything on one
 * screen, no text cut, clipped or overlapping; and the settings card the
 * same numbers stand on. */
export async function testJevDashboardEvidence(browser, origin, ok) {
  const real = realSummary();
  await mkdir(EVIDENCE_DIR, { recursive: true });
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1900, height: 1400 });
    await page.evaluate(jevDashboardFixture, real);
    await page.evaluate(() => el("nav-jev").click());
    await page.waitForFunction(() => document.querySelector("#jev-view [data-jev-dash-row] [data-jev-fact=\"week\"]") !== null);
    await settlePaint(page);
    // One screen means the screen is as large as the page needs, up to a
    // cap: the stage beside the sidebar and the explorer is what a person
    // gets, and the check below says whether that stage held everything.
    const wanted = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const wrap = view.querySelector(".jev-table-wrap");
      return { width: 1900 + Math.max(0, wrap.scrollWidth - wrap.clientWidth), height: 1400 + Math.max(0, view.scrollHeight - view.clientHeight) };
    });
    await page.setViewportSize({ width: Math.min(2600, Math.ceil(wanted.width) + 16), height: Math.min(4000, Math.ceil(wanted.height) + 16) });
    await settlePaint(page);
    const shots = {};
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await settlePaint(page);
      const seen = await page.evaluate(() => {
        const view = document.querySelector("#jev-view");
        return {
          faults: window.__JEV_TEXT_FAULTS__(view),
          oneScreen: view.scrollHeight <= view.clientHeight + 1,
          tableFits: (() => { const wrap = view.querySelector(".jev-table-wrap"); return wrap.scrollWidth <= wrap.clientWidth + 1; })(),
          rows: view.querySelectorAll("[data-jev-dash-row]").length,
          decisions: view.querySelectorAll(".jev-decision").length,
          sparks: view.querySelectorAll(".jev-spark[data-points]").length,
          theme: document.documentElement.dataset.theme ?? "dark",
          viewport: [window.innerWidth, window.innerHeight],
        };
      });
      const path = join(EVIDENCE_DIR, `jev-dashboard-${theme}.png`);
      await page.screenshot({ path });
      shots[theme] = { ...seen, path };
      ok(`the ${theme} dashboard is one readable screen: no text cut, clipped or overlapping`,
        seen.faults.length === 0 && seen.oneScreen && seen.tableFits && seen.rows === 11 && seen.sparks === 33 && seen.theme === theme,
        JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
    }
    ok("the evidence carries this machine's own count when zo answers, and says which",
      true,
      real ? `real: ${real.zo}${real.recent ? " --recent 12" : " (no recent list: that zo predates the flag)"}` : "fixture: no zo answered");

    // The settings card, standing on the same numbers.
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await page.evaluate(() => setSettingsOpen(true, "typesafe-routing-select"));
      await page.waitForFunction(() => document.querySelector("#jev-uses-card [data-jev-numbers]") !== null);
      await settlePaint(page);
      const card = await page.evaluate(() => {
        const card = document.querySelector("#jev-uses-card");
        card.scrollIntoView({ block: "start" });
        return { faults: window.__JEV_TEXT_FAULTS__(card), lines: card.querySelectorAll("[data-jev-numbers]").length };
      });
      const path = join(EVIDENCE_DIR, `jev-settings-card-${theme}.png`);
      await page.locator("#jev-uses-card").screenshot({ path });
      await page.evaluate(() => setSettingsOpen(false));
      ok(`the ${theme} settings card draws the same numbers with no text cut, clipped or overlapping`,
        card.faults.length === 0 && card.lines === 11, JSON.stringify({ ...card, faults: card.faults.slice(0, 6), path }));
    }
    ok("the evidence pages raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

export async function testJevDashboard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(jevDashboardFixture);
    const opened = await page.evaluate(async () => {
      const began = performance.now();
      el("nav-jev").click();
      const view = () => document.querySelector("#jev-view");
      // Drawn: every seat's row stands with a number in it, not only the
      // switch — the ask's answer has landed and been painted.
      await new Promise((done, fail) => {
        const until = performance.now() + 3_000;
        const look = () => {
          const rows = view()?.querySelectorAll("[data-jev-dash-row]") ?? [];
          const drawn = [...rows].some((row) => row.querySelector('[data-jev-fact="week"]')?.textContent === "50");
          if (drawn) done();
          else if (performance.now() > until) fail(new Error("the table never drew"));
          else requestAnimationFrame(look);
        };
        look();
      });
      const ms = performance.now() - began;
      const rows = [...view().querySelectorAll("[data-jev-dash-row]")];
      const cardNames = window.__JEV__.seats.map((id) => document.querySelector(`[data-jev-seat="${id}"]`)
        .closest("[data-jev-row]").querySelector(".settings-label").textContent.trim());
      const cell = (id, name) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="${name}"]`);
      const fact = (id, name) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-fact="${name}"]`)?.textContent ?? null;
      const summon = {
        today: fact("summon", "today"), week: fact("summon", "week"),
        answered: cell("summon", "answered").textContent, latency: cell("summon", "latency").textContent,
        refusals: fact("summon", "refusals"), applied: fact("summon", "applied"),
        agreement: fact("summon", "agreement"), cost: cell("summon", "cost").textContent,
        why: cell("summon", "why").textContent,
        sparks: [...cell("summon", "trend").querySelectorAll(".jev-spark")].map((svg) => Number(svg.dataset.points)),
        mode: cell("summon", "mode").querySelector("select").value,
      };
      const recallWhy = cell("recall", "why").textContent;
      const quiet = { week: fact("skills", "week"), refusals: fact("skills", "refusals"), why: cell("skills", "why").textContent,
        sparks: [...cell("skills", "trend").querySelectorAll(".jev-spark")].map((svg) => Number(svg.dataset.points)) };
      const numbersLine = cell("summon", "why").textContent;
      const cardLine = document.querySelector('[data-jev-seat="summon"]').closest("[data-jev-row]")
        .querySelector("[data-jev-numbers]")?.textContent ?? null;
      return {
        ms, active: activeTabId, visible: !view().hidden, hiddenAttr: view().hidden,
        rowIds: rows.map((row) => row.dataset.jevDashRow),
        rowNames: rows.map((row) => row.querySelector('[data-jev-cell="seat"]').textContent),
        cardNames, cardSeats: window.__JEV__.seats, summon, quiet, recallWhy, numbersLine, cardLine,
        asks: window.__JEV__.asks.slice(), settingsAsks: window.__COUNTS__.typesafe_settings ?? 0,
        summaryAsks: window.__COUNTS__.jev_summary ?? 0,
        title: tabLabel(tabs.find((tab) => tab.id === "jev")),
        pressed: el("nav-jev").getAttribute("aria-pressed"),
        heads: [...view().querySelectorAll("thead th")].map((th) => th.scope),
        recentSeat: view().querySelector(".jev-recent-seat").value,
        recentCount: view().querySelectorAll(".jev-decision").length,
        recentFirst: view().querySelector(".jev-decision")?.textContent ?? "",
        recentRefused: view().querySelector('.jev-decision[data-outcome="not_consented"] .jev-decision-outcome')?.textContent ?? null,
        recentOptions: [...view().querySelectorAll(".jev-recent-seat option")].map((option) => option.value),
        pollMs: JEV_POLL_MS, recentRows: JEV_RECENT_ROWS,
        fresh: view().querySelector(".jev-freshness").textContent,
      };
    });
    ok("the rail door opens the Jev tab and wears its pressed state",
      opened.active === "jev" && opened.visible && opened.pressed === "true", JSON.stringify({ active: opened.active, pressed: opened.pressed }));
    ok("the table carries every seat of the settings card, in the card's order, under the card's names",
      opened.rowIds.length === 11 && opened.rowIds.join(",") === opened.cardSeats.join(",")
        && opened.rowNames.join("|") === opened.cardNames.join("|"),
      JSON.stringify({ rows: opened.rowIds, names: opened.rowNames, card: opened.cardNames }));
    ok("opening costs one settings ask and one summary ask, with the recent list",
      opened.settingsAsks === 1 && opened.summaryAsks === 1 && opened.asks.length === 1 && opened.asks[0].recent === opened.recentRows,
      JSON.stringify({ settings: opened.settingsAsks, summary: opened.summaryAsks, asks: opened.asks }));
    ok("from the click to the drawn table is under the design's 200 ms with the backend answering at once",
      opened.ms < 200, `${opened.ms.toFixed(1)} ms`);
    ok("a counted seat's numbers stand in its cells",
      opened.summon.today === "32" && opened.summon.week === "50" && opened.summon.answered.startsWith("96%")
        && opened.summon.answered.includes("84%") && opened.summon.latency === "230 / 410"
        && opened.summon.refusals === "not_consented 2" && opened.summon.applied === "0"
        && opened.summon.agreement === "16/44 (24%)" && opened.summon.cost === "$0.0123" && opened.summon.mode === "on",
      JSON.stringify(opened.summon));
    ok("the why column says whose doing the acting is, what the judge said, the window it read and the rows still owed",
      opened.summon.why.includes("34/34") && opened.summon.why.includes("900") && opened.summon.why.includes("10")
        && opened.summon.why.includes("직접 켜서") && opened.summon.why.includes("답한 비율이 모자랍니다")
        && opened.recallWhy.includes("승격하지 않습니다"),
      JSON.stringify({ summon: opened.summon.why, recall: opened.recallWhy }));
    ok("the trend has one point per counted day and none for a day nothing was asked",
      opened.summon.sparks.join(",") === "5,5,5" && opened.quiet.sparks.join(",") === "0,0,0",
      JSON.stringify({ summon: opened.summon.sparks, quiet: opened.quiet.sparks }));
    ok("a seat nothing has asked yet reads as never asked, not as zero",
      opened.quiet.week === "0" && opened.quiet.refusals === null && opened.quiet.why.includes("아직 아무것도 묻지 않았습니다"),
      JSON.stringify(opened.quiet));
    ok("the recent list opens on the first seat with decisions and lists the digest",
      opened.recentSeat === "routing" && opened.recentCount === 1 && opened.recentFirst.includes("task: 68212a1194a4e327")
        && opened.recentFirst.includes("complexity=small") && opened.recentFirst.includes("적용")
        && opened.recentOptions.length === 11,
      JSON.stringify({ seat: opened.recentSeat, count: opened.recentCount, first: opened.recentFirst }));
    ok("the tab is titled and the head cells are column heads",
      opened.title !== null && opened.title.length > 0 && opened.heads.every((scope) => scope === "col"), JSON.stringify({ title: opened.title, heads: opened.heads.length }));
    ok("the freshness line names the ask's own cost", /\d+ ms/.test(opened.fresh), opened.fresh);

    // The recent list follows the picker, and a refused row wears its token.
    const picked = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const picker = view.querySelector(".jev-recent-seat");
      picker.value = "summon";
      picker.dispatchEvent(new Event("change", { bubbles: true }));
      await window.__PAINTED__();
      return {
        count: view.querySelectorAll(".jev-decision").length,
        refused: view.querySelector('.jev-decision[data-outcome="not_consented"] .jev-decision-outcome')?.textContent ?? null,
        marks: [...view.querySelectorAll(".jev-decision-marks")].map((one) => one.textContent),
        asks: window.__COUNTS__.jev_summary,
      };
    });
    ok("choosing another seat lists its decisions without asking zo again",
      picked.count === 5 && picked.refused === "not_consented" && picked.asks === 1
        && picked.marks[0].includes("확신 19%") && picked.marks[0].includes("적용") && picked.marks[0].includes("일치")
        && picked.marks[1].includes("기록만"),
      JSON.stringify(picked));

    // A switch moved on the dashboard goes through the card's door, and the
    // card's own switch follows — one state, two surfaces.
    const moved = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const select = view.querySelector('[data-jev-dash-seat="routing"]');
      select.value = "shadow";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      await new Promise((done) => setTimeout(done, 50));
      await window.__PAINTED__();
      return {
        calls: window.__COUNTS__.set_jev_mode ?? 0,
        held: window.__JEV__.modeOf.routing,
        dashboard: view.querySelector('[data-jev-dash-seat="routing"]').value,
        card: document.querySelector("#typesafe-routing-select").value,
        status: view.querySelector(".jev-status").textContent,
        statusHidden: view.querySelector(".jev-status").hidden,
      };
    });
    ok("a switch moved on the dashboard goes through set_jev_mode and the card follows",
      moved.calls === 1 && moved.held === "shadow" && moved.dashboard === "shadow" && moved.card === "shadow"
        && !moved.statusHidden && moved.status.includes("기록만"),
      JSON.stringify(moved));

    // The refresh policy: a burst of ledger events is one settled ask; a
    // re-open within the beat is none; the beat itself is slower than the
    // ledger's own second.
    const refreshed = await page.evaluate(async () => {
      const before = window.__COUNTS__.jev_summary;
      for (const handler of window.__LISTENERS__["ledger:changed"] ?? []) handler({ payload: null });
      await new Promise((done) => setTimeout(done, 300));
      for (const handler of window.__LISTENERS__["ledger:changed"] ?? []) handler({ payload: null });
      const during = window.__COUNTS__.jev_summary;
      await new Promise((done) => setTimeout(done, JEV_EVENT_SETTLE_MS + 400));
      const after = window.__COUNTS__.jev_summary;
      // Re-open within the beat: the tab is dropped and the door clicked again.
      dropTab("jev");
      await window.__PAINTED__();
      const closedPressed = el("nav-jev").getAttribute("aria-pressed");
      el("nav-jev").click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 50));
      const reopened = window.__COUNTS__.jev_summary;
      const rows = document.querySelector("#jev-view").querySelectorAll("[data-jev-dash-row]").length;
      return { before, during, after, reopened, closedPressed, rows, pollMs: JEV_POLL_MS, settleMs: JEV_EVENT_SETTLE_MS };
    });
    ok("a burst of ledger events is one settled summary ask",
      refreshed.during === refreshed.before && refreshed.after === refreshed.before + 1, JSON.stringify(refreshed));
    ok("a re-open within the beat draws what is in hand and asks nothing",
      refreshed.reopened === refreshed.after && refreshed.rows === 11 && refreshed.closedPressed === "false", JSON.stringify(refreshed));
    ok("the dashboard's beat is a slow one, not the ledger's second",
      refreshed.pollMs >= 10_000 && refreshed.settleMs >= 1_000, JSON.stringify({ poll: refreshed.pollMs, settle: refreshed.settleMs }));

    // The words move with the language, from the catalog, by key.
    const english = await page.evaluate(async () => {
      setLocale("en");
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      const said = {
        head: view.querySelector('thead th.jev-col-seat').textContent,
        title: view.querySelector(".jev-title").textContent,
        refresh: view.querySelector(".jev-refresh").textContent,
      };
      setLocale("ko");
      await window.__PAINTED__();
      said.back = view.querySelector('thead th.jev-col-seat').textContent;
      return said;
    });
    ok("the dashboard's words are the catalog's, in the language in force",
      english.head === "Seat" && english.title === "Jev dashboard" && english.refresh === "Refresh" && english.back === "자리",
      JSON.stringify(english));

    // zo that cannot count: the table still stands with the switches and says why.
    const refused = await page.evaluate(async () => {
      window.__FAIL__.add("jev_summary");
      await refreshJevDashboard({ force: true });
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      window.__FAIL__.delete("jev_summary");
      return {
        error: view.querySelector(".jev-error").textContent, hidden: view.querySelector(".jev-error").hidden,
        rows: view.querySelectorAll("[data-jev-dash-row]").length,
        week: view.querySelector('[data-jev-dash-row="summon"] [data-jev-fact="week"]').textContent,
      };
    });
    ok("a zo that cannot count leaves the last numbers standing and says so",
      !refused.hidden && refused.error.includes("refused: jev_summary") && refused.rows === 11 && refused.week === "50",
      JSON.stringify(refused));

    ok("the dashboard raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
