/* The Jev dashboard (t-5807, docs/design/jev-dashboard-and-perfection-20260921.md §2).
 *
 * What is measured: the rail door opens one tab; the table carries every
 * feature the settings markup names (`#jev-features`), in its order and
 * under its names; the numbers zo answered stand in their cells, the trend
 * has one point per counted day, and the recent list shows what the digest
 * said; the one switch over the table is the card's, pressed through the
 * card's door (docs/design/jev-settings-20260917.md §6.1), and no feature
 * offers a mode to choose; the refresh policy is one ask on open, one
 * settled ask per burst of ledger events, none on a re-open within the
 * beat; and the time from the click to the drawn table, with the backend
 * answering at once, sits under the design's line (§2: ≤ 200 ms).
 *
 * The backend is the window harness's stub with its answers swapped in
 * (`window.__ANSWER__`): the settings' state, zo's summary, the day's
 * count and the switch's door. */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { openWindowTestPage } from "./window-boot.mjs";
import { createRequire } from "node:module";
import { contrastTable, setQualityTheme, settlePaint } from "./settings-quality.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
/* Where the evidence goes: a person looks at these (t-5807, the operator's
 * ask), so they are written under the checkout's output folder, which git
 * ignores, and named in the report. */
const EVIDENCE_DIR = join(ROOT, "output", "t-5807");
/* How many features the settings markup names — counted off the markup
 * rather than typed here, so a feature added to the use table (which
 * `typesafe_settings.rs` holds the markup to) is not a second number to
 * move. */
const { readFileSync } = await import("node:fs");
const SEATS = (readFileSync(join(ROOT, "ui", "index.html"), "utf8").match(/data-jev-feature="/g) ?? []).length;
/* The fewest days with a value a trend is drawn from (t-6243 D4). */
const TREND_DAYS = 3;
/* The rows a 1080p stage holds under the charts before any scroll, both
 * side panels open (t-9633): the charts took the room of two of the nine
 * rows t-6243 D1 kept, and the fixture's six features in use still stand
 * with one row to spare. */
const FIRST_SCREEN_ROWS = 7;
/* The narrow window the dashboard has to stay whole in (t-9633): a laptop
 * split in two, both side panels open. */
const NARROW = { width: 900, height: 1000 };

/* The narrow window, grown as tall as the dashboard it holds so every part
 * of it stands on the screen at once — again after each growth, since a
 * window that grows can lay the page out again. */
async function standNarrow(page) {
  await page.setViewportSize(NARROW);
  await settlePaint(page);
  for (let round = 0; round < 4; round += 1) {
    const wanted = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      view.scrollTop = 0;
      return view.scrollHeight - view.clientHeight;
    });
    if (wanted <= 0) return;
    const now = page.viewportSize();
    await page.setViewportSize({ width: now.width, height: Math.min(8000, now.height + wanted + 16) });
    await settlePaint(page);
  }
}
/* The contrast every text on the dashboard and the card clears, in both
 * treatments, large or not (t-6277 D10): WCAG AA's line for body text. */
const CONTRAST_FLOOR = 4.5;

/* axe, for the contrast table (`contrastTable`) — resolved from the
 * checkout's own node_modules, as the settings gate resolves it; `null` when
 * it is not installed, which the check then says rather than skipping. */
function axeBuilder() {
  try {
    const require = createRequire(import.meta.url);
    const found = require(require.resolve("@axe-core/playwright"));
    return found.default ?? found;
  } catch {
    return null;
  }
}

/* Installed on the page: the fixture the answers are drawn from — exported
 * so a measurement draws the same dashboard the gate does.
 * With `real` — this machine's own count, from `realSummary` — the summary
 * answer is that count and every switch stands where that count says. */
export function jevDashboardFixture(real = null) {
  /* What `jev_summary` answers for `seats` (t-9091): the reading names the
   * scope it counted — this checkout, with records of its own — and each
   * feature where its rows are kept, read off the file zo found them in (the
   * machine's one place, `~/.zo/jev`, or the project's own state). */
  function jevReading(args, seats) {
    const reachOf = (found) => (!found ? null : /\/\.zo\/jev\//.test(found) ? "machine" : "project");
    return {
      scope: { scope: args?.scope ?? "workspace", workspace: "/Users/p/project", workspaceName: "project", recorded: true, projects: [], windowDays: 7 },
      seats: seats.map((seat) => ({ ...seat, reach: reachOf(seat.found) })),
    };
  }
  const seats = [...document.getElementById("jev-features").content.querySelectorAll("[data-jev-feature]")]
    .map((feature) => feature.dataset.jevFeature);
  const modes = [
    { mode: "off", asks: false, applies: false, automatic: false },
    { mode: "shadow", asks: true, applies: false, automatic: false },
    { mode: "on", asks: true, applies: true, automatic: false },
    { mode: "auto", asks: true, applies: false, automatic: true },
  ];
  const modeOf = Object.fromEntries(seats.map((id) => [id, "on"]));
  modeOf.recall = "shadow";
  modeOf.placement = "auto";
  // A feature a person switched off by hand in the settings file (t-6277 D8).
  modeOf.skills = "off";
  const now = Date.now();
  const day = 24 * 60 * 60 * 1000;
  const today = now - (now % day);
  const empty = () => ({
    rows: 0, answered: 0, refused: 0, applied: 0, refusals: [], answeredShare: null, answeredLowerBound: null,
    called: 0, requests: 0, redactedLines: 0, inputTokens: 0, p50Ms: null, p95Ms: null, failures: [],
    guards: { instructed: 0, walled: 0 }, controls: { named: 0, destructiveHeld: 0 },
  });
  const counted = (rows, answered, extra = {}) => ({
    ...empty(), rows, answered, answeredShare: rows ? answered / rows : null,
    answeredLowerBound: rows ? Math.max(0, answered / rows - 0.12) : null, called: answered, requests: rows, ...extra,
  });
  /* A feature's week, day by day (t-9633): each day's requests and answers,
   * how long they took, the input tokens they billed, and the marks that
   * graded them — the judgment's own and, where one is given (`base`), the
   * simplest method's over the same marks — and, where one is given
   * (`notComparedBy`), why the day's rows that compared nothing say so, word
   * by word (t-9556). `null` is a day nothing asked. */
  const noMarks = () => ({ compared: 0, agreed: 0, lowerBound: null, controlRows: 0,
    baselineCompared: 0, baselineAgreed: 0, baselineShare: null, notCompared: 0, notComparedBy: {} });
  /* Rows that compared nothing, by the words their writers gave, and how
   * many they are — the two numbers zo sends beside every agreement. */
  const withheld = (by) => ({ notCompared: Object.values(by).reduce((sum, count) => sum + count, 0), notComparedBy: by });
  const days = (list) => list.map((one, at) => ({
    startMs: today - (6 - at) * day,
    tally: one === null ? empty() : counted(one.rows ?? 10, one.answered ?? one.rows ?? 10, {
      p50Ms: one.p50 ?? 200 + at * 10, p95Ms: one.p95 ?? 400 + at * 20, inputTokens: one.tokens ?? 0,
    }),
    agreement: {
      ...noMarks(),
      ...(!one?.compared ? {} : { compared: one.compared, agreed: one.agreed, lowerBound: one.bound ?? null }),
      ...(one?.base === undefined ? {} : { baselineCompared: one.compared, baselineAgreed: one.base, baselineShare: one.base / one.compared }),
      ...(one?.notComparedBy ? withheld(one.notComparedBy) : {}),
    },
  }));
  const quietWeek = () => days([null, null, null, null, null, null, null]);
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
      stand: "recording", applies: modeOf[id] === "on", verdict: null, days: quietWeek(),
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
      // The id asked and the version that answered, and the version a change
      // of version cut away (t-6187) — what the why column and the card say.
      seat.askedModel = "jev-latest";
      seat.model = "jev-1.13.0";
      seat.verdict = { verdict: "hold", line: "answered", cutModel: "jev-1.12.0" };
      seat.agreementWeek = { compared: 50, agreed: 20, lowerBound: 0.28 };
      // Its most confident answers were its worst, so its record draws no
      // confidence bar (t-9468's `non_monotone`).
      seat.calibration = { readsActLine: true, actFromPermille: null, reason: "non_monotone", tableLine: null,
        fixed: { fromPermille: 850, marks: 12, applyShare: 0.3, errorPermille: 500, baselineErrorPermille: 250, underErrorPermille: 300 },
        drawn: null };
      // Five days that asked, each graded on eight marks; the simplest method
      // was compared on the last two, and did as well on the last one. The
      // days' input tokens are the week's 90,000.
      seat.days = days([null,
        { answered: 9, tokens: 10_000, compared: 8, agreed: 6, bound: 0.41 },
        { tokens: 20_000, compared: 8, agreed: 7, bound: 0.53 },
        { answered: 8, tokens: 15_000, compared: 8, agreed: 5, bound: 0.31 },
        null,
        { tokens: 25_000, compared: 8, agreed: 8, bound: 0.68, base: 6 },
        { tokens: 20_000, compared: 8, agreed: 7, bound: 0.53, base: 7 },
      ]);
      seat.recent = args?.recent ? decisions(Math.min(args.recent, 5)) : [];
    }
    if (id === "recall") {
      // A seat the table never promotes: no floor, no verdict, and a
      // person's `shadow` — it records, and the why column says so.
      seat.found = "/h/state/smart-router/rerank-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 211, p95Ms: 232 });
      seat.week = counted(1005, 935, { applied: 845, p50Ms: 211, p95Ms: 400, inputTokens: 70_000,
        failures: [{ token: "schema", rows: 65 }, { token: "timeout", rows: 5 }] });
      seat.costUsd = 0.31;
      // Asked every day and graded on none of them: nothing to compare —
      // its turns touched no note (t-9556), five of them today.
      seat.days = days(Array.from({ length: 7 }, (unused, at) => ({ rows: 143, answered: 133, tokens: 10_000,
        ...(at === 6 ? { notComparedBy: { no_note_touched: 5 } } : {}) })));
      seat.agreementWeek = { ...noMarks(), ...withheld({ no_note_touched: 34 }) };
    }
    if (id === "placement") {
      // A seat under its bar: the window is full and the judge holds it on
      // agreement (t-6243 D1's 기준 미달).
      seat.today = counted(4, 4, { p50Ms: 606, p95Ms: 768 });
      seat.week = counted(60, 59, { p50Ms: 606, p95Ms: 768, inputTokens: 1_400 });
      seat.costUsd = 0.002;
      seat.riseFloorPermille = 800;
      seat.clearsRiseFloor = true;
      seat.rowsToNextJudgment = 5;
      seat.judged = { window: counted(25, 25), windowWanted: 25, agreement: { compared: 21, agreed: 12, lowerBound: 0.37, controlRows: 0 } };
      seat.verdict = { verdict: "hold", line: "agreement" };
      // Three graded days: the simplest method did as well on the first two
      // and worse on the last — and most of its week compared nothing: a pane
      // nobody looked at, or one nobody moved out of a room the answer did
      // not name (t-9556), four of the first today.
      seat.days = days([null, null, null, null,
        { rows: 12, tokens: 400, compared: 7, agreed: 4, bound: 0.18, base: 5 },
        { rows: 12, tokens: 500, compared: 7, agreed: 4, bound: 0.18, base: 4 },
        { rows: 12, tokens: 500, compared: 7, agreed: 6, bound: 0.49, base: 4, notComparedBy: { unseen: 4 } },
      ]);
      seat.agreementWeek = { ...noMarks(), compared: 21, agreed: 12, lowerBound: 0.37, ...withheld({ unseen: 95, not_carried: 3 }) };
      // Its record draws a confidence bar and the table holds it (t-9468):
      // at 0.7 and above it acts on 45% of its answers, wrong 66 times in a
      // thousand where the simplest method was wrong 333.
      const line = { fromPermille: 700, marks: 30, applyShare: 0.45, errorPermille: 66, baselineErrorPermille: 333, underErrorPermille: 250 };
      seat.calibration = { readsActLine: true, actFromPermille: 700, reason: null, tableLine: 700,
        fixed: { ...line, fromPermille: 850, marks: 9, applyShare: 0.2 }, drawn: line };
      seat.applyShare = line.applyShare;
      seat.appliedErrorPermille = line.errorPermille;
      seat.baselineErrorPermille = line.baselineErrorPermille;
    }
    if (id === "browser") {
      // A small sample: four answers in the week, too few for a lower bound
      // to be a measurement (t-6243 D3) — and a screen feature: its rows name
      // the controls it judged, two presses its guard stopped for the
      // screen's own orders, one at a wall, and one control a press cannot
      // take back that it handed to the person (t-6277 D6).
      seat.week = counted(4, 4, { p50Ms: 243, p95Ms: 435,
        guards: { instructed: 2, walled: 1 }, controls: { named: 4, destructiveHeld: 1 } });
      seat.days = days([null, null, null, null, null, null, { rows: 4, p50: 243, p95: 435 }]);
    }
    if (id === "notify") {
      // A seat every request of which the door refused for consent: nothing
      // it asked was answered (t-6243 D1's 키·동의 필요).
      seat.today = counted(2, 0, { refused: 2, refusals: [{ token: "not_consented", rows: 2 }], failures: [{ token: "not_consented", rows: 2 }] });
      seat.week = counted(2, 0, { refused: 2, refusals: [{ token: "not_consented", rows: 2 }], failures: [{ token: "not_consented", rows: 2 }] });
      seat.costUsd = 0;
      seat.days = days([null, null, null, null, null, null, { rows: 2, answered: 0 }]);
    }
    if (id === "routing") {
      seat.found = "/h/state/smart-router/decision-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 541, p95Ms: 600 });
      // Three requests the door refused for want of a key, one the wire timed
      // out on (t-6243 D5).
      seat.week = counted(35, 31, { refused: 3, refusals: [{ token: "no_key", rows: 3 }],
        failures: [{ token: "no_key", rows: 3 }, { token: "timeout", rows: 1 }], p50Ms: 500, p95Ms: 884, applied: 23,
        inputTokens: 7_000 });
      seat.costUsd = 0.004;
      seat.riseFloorPermille = 950;
      seat.clearsRiseFloor = false;
      // The core's countdown while the window fills is what the window
      // still wants (`promote::rows_to_next_judgment`): 73 − 35.
      seat.rowsToNextJudgment = 38;
      seat.judged = { window: counted(35, 31), windowWanted: 73, agreement: { compared: 3, agreed: 3, lowerBound: 0.43, controlRows: 1 } };
      seat.verdict = { verdict: "hold", line: "too_few_rows" };
      // Three marks are too few to read a confidence bar off: the reason is
      // the judge's own line, by its word (t-9468's `NoLine::Line`).
      seat.calibration = { readsActLine: true, actFromPermille: null, reason: "too_few_compared", tableLine: null, fixed: null, drawn: null };
      // Asked every day, graded on one day only, under the sample floor.
      seat.days = days([0, 1, 2, 3, 4, 5, 6].map((at) => ({ rows: 5, tokens: 1_000,
        ...(at === 2 ? { compared: 3, agreed: 3, bound: 0.43 } : {}) })));
      seat.recent = args?.recent ? [{
        at: now - 30_000, outcome: "answered", elapsedMs: 541, cached: false,
        asked: { task: "68212a1194a4e327", attempt: "session-1@1" },
        answered: { complexity: "small", intent: "other", risk: "low" }, confidence: null,
        applied: true, agreed: null, followed: null,
      }] : [];
    }
    return seat;
  };
  // The one switch (§6.1): in use, from four folders — a machine set up
  // before the switch — until a press says otherwise.
  const jev = { on: true, everywhere: false, folders: 4 };
  window.__JEV__ = { seats, modeOf, asks: [], jev, pressed: [] };
  const settings = () => ({
    keysKeptHere: true, keySaved: true, jev: { ...jev },
    // Every seat's comparison sample floor, the use table's
    // (`JevUse::agreement_rows_wanted`, a judgment window's worth).
    switches: seats.map((id) => ({ id, setting: id, mode: modeOf[id], modes, written: true, agreementRowsWanted: 20 })),
    classifier: { setting: "autoClassifier", mode: "probed", probes: true, gates: seats[0],
      modes: [{ mode: "probed", runs: true, markers: false, probes: true }] },
  });
  window.__ANSWER__.typesafe_settings = settings;
  // The day's requests from this machine against the person's limit
  // (`jev_day`, t-6243 D1).
  window.__ANSWER__.jev_day = () => ({ sent: 247, most: 500 });
  window.__ANSWER__.jev_summary = (args) => {
    window.__JEV__.asks.push(args ?? {});
    return jevReading(args, seats.map((id) => numbers(id, args)));
  };
  // The switch's door, as the core presses it: on is every folder and every
  // seat at its recommendation, off is nothing in use.
  window.__ANSWER__.set_jev_enabled = (args) => {
    window.__JEV__.pressed.push(args.on);
    Object.assign(jev, args.on ? { on: true, everywhere: true, folders: 0 } : { on: false });
    return settings();
  };
  if (real) {
    for (const seat of real.seats) if (seat.id in modeOf) modeOf[seat.id] = seat.mode;
    window.__ANSWER__.jev_summary = (args) => {
      window.__JEV__.asks.push(args ?? {});
      return jevReading(args, real.seats);
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
    // its body's boxes (content-visibility) but nobody sees them — and a fold
    // inside a closed fold is out of sight, its own summary with it.
    const folded = (node) => {
      for (let fold = node.closest("details:not([open])"); fold; fold = fold.parentElement?.closest("details:not([open])") ?? null) {
        const summary = node.closest("summary");
        if (!(summary && summary.parentElement === fold)) return true;
      }
      return false;
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
  // The table carries every seat of the card; the numbers, trends and card
  // lines are drawn for the seats the answering zo could count. An installed
  // zo can predate a seat the table just gained (it reads the same table,
  // once rebuilt), and a row it never answered stands with no trend.
  const counted = real ? real.seats.length : SEATS;
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
    const oneScreen = { width: Math.min(2600, Math.ceil(wanted.width) + 16), height: Math.min(4000, Math.ceil(wanted.height) + 16) };
    await page.setViewportSize(oneScreen);
    await settlePaint(page);
    const shots = {};
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await settlePaint(page);
      const seen = await page.evaluate((trendDays) => {
        const view = document.querySelector("#jev-view");
        return {
          faults: window.__JEV_TEXT_FAULTS__(view),
          oneScreen: view.scrollHeight <= view.clientHeight + 1,
          // The rows in use stay short: a status line, a bar and a fact at
          // most, so a 1080p screen holds them (t-6243 D1/D2).
          rowHeights: [...view.querySelectorAll("[data-jev-dash-row]")].map((row) => Math.round(row.getBoundingClientRect().height)),
          tableFits: (() => { const wrap = view.querySelector(".jev-table-wrap"); return wrap.scrollWidth <= wrap.clientWidth + 1; })(),
          rows: view.querySelectorAll("[data-jev-dash-row]").length,
          // The features nothing asked all week fold into one row (t-6243
          // D1); the rest stand, whether or not zo could count them.
          unused: (jevNumbers ?? []).filter((one) => one.week.rows === 0).length,
          fold: view.querySelector("[data-jev-fold]") !== null,
          decisions: view.querySelectorAll(".jev-decision").length,
          // One accuracy picture per feature with three graded days (t-6243
          // D4, t-9633 (b)), one day strip per feature that asked on any
          // day (a), and the two charts over the table (c, e).
          pictures: view.querySelectorAll('[data-jev-picture="trend"]').length,
          drawable: [...view.querySelectorAll("[data-jev-dash-row]")].filter((row) =>
            ((jevNumbers ?? []).find((one) => one.id === row.dataset.jevDashRow)?.days ?? [])
              .filter((day) => day.agreement?.compared > 0).length >= trendDays).length,
          strips: view.querySelectorAll('[data-jev-picture="timeline"]').length,
          asked: [...view.querySelectorAll("[data-jev-dash-row]")].filter((row) =>
            ((jevNumbers ?? []).find((one) => one.id === row.dataset.jevDashRow)?.days ?? [])
              .some((day) => day.tally.rows > 0)).length,
          charts: [...view.querySelectorAll("[data-jev-chart]")].filter((card) =>
            card.querySelector("svg[data-jev-picture]") !== null || !card.querySelector(".jev-chart-empty").hidden).length,
          theme: document.documentElement.dataset.theme ?? "dark",
          viewport: [window.innerWidth, window.innerHeight],
        };
      }, TREND_DAYS);
      const path = join(EVIDENCE_DIR, `jev-dashboard-${theme}.png`);
      await page.screenshot({ path });
      shots[theme] = { ...seen, path };
      ok(`the ${theme} dashboard is one readable screen: no text cut, clipped or overlapping`,
        seen.faults.length === 0 && seen.oneScreen && seen.tableFits && seen.rows === SEATS - seen.unused
          && seen.fold === (seen.unused > 0) && seen.pictures === seen.drawable && seen.strips === seen.asked
          && seen.charts === 2 && seen.theme === theme,
        JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
    }
    // A narrow window (t-9633): the rows stand as cards and the charts one
    // under the other, every word whole, nothing wider than the page — the
    // window as tall as the page, so the picture is the whole of it.
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await standNarrow(page);
      const seen = await page.evaluate(() => {
        const view = document.querySelector("#jev-view");
        view.scrollTop = 0;
        const wide = [view, ...view.querySelectorAll(".jev-table-wrap, [data-jev-chart]")]
          .filter((one) => one.scrollWidth > one.clientWidth + 1).map((one) => one.className);
        return {
          faults: window.__JEV_TEXT_FAULTS__(view), wide, width: Math.round(view.getBoundingClientRect().width),
          theme: document.documentElement.dataset.theme ?? "dark",
        };
      });
      const path = join(EVIDENCE_DIR, `jev-dashboard-narrow-${theme}.png`);
      await page.screenshot({ path });
      ok(`the ${theme} dashboard in a narrow window keeps every word whole and nothing wider than the page`,
        seen.faults.length === 0 && seen.wide.length === 0 && seen.theme === theme,
        JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
    }
    ok("the evidence carries this machine's own count when zo answers, and says which",
      true,
      real ? `real: ${real.zo}${real.recent ? " --recent 12" : " (no recent list: that zo predates the flag)"}` : "fixture: no zo answered");

    await page.setViewportSize(oneScreen);

    // The settings card: one switch, the key, and a line to the dashboard
    // (§6.1) — no choice of mode in sight.
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await page.evaluate(() => setSettingsOpen(true, "jev-enabled"));
      await page.waitForFunction(() => document.getElementById("jev-enabled")?.checked !== undefined);
      await settlePaint(page);
      const card = await page.evaluate(() => {
        const card = document.querySelector("#typesafe-card");
        card.scrollIntoView({ block: "start" });
        return {
          faults: window.__JEV_TEXT_FAULTS__(card),
          visibleSelects: [...card.querySelectorAll("select")]
            .filter((one) => !one.closest("details:not([open])") && one.getClientRects().length > 0).length,
          switches: card.querySelectorAll("[data-jev-switch]").length,
        };
      });
      const path = join(EVIDENCE_DIR, `jev-settings-card-${theme}.png`);
      await page.locator("#typesafe-card").screenshot({ path });
      await page.evaluate(() => setSettingsOpen(false));
      ok(`the ${theme} settings card is one switch and the key, with no text cut, clipped or overlapping`,
        card.faults.length === 0 && card.visibleSelects === 0 && card.switches === 1,
        JSON.stringify({ ...card, faults: card.faults.slice(0, 6), path, counted }));
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
      const cardNames = window.__JEV__.seats.map((id) => document.getElementById("jev-features").content
        .querySelector(`[data-jev-feature="${id}"] [data-jev-name]`).textContent.trim());
      const cell = (id, name) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="${name}"]`);
      const fact = (id, name) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-fact="${name}"]`)?.textContent ?? null;
      const summon = {
        today: fact("summon", "today"), week: fact("summon", "week"),
        answered: cell("summon", "answered").textContent, latency: [fact("summon", "p50"), fact("summon", "p95")],
        applied: fact("summon", "applied"),
        agreement: fact("summon", "agreement"), cost: cell("summon", "cost").textContent,
        boundTip: view().querySelector('[data-jev-dash-row="summon"] [data-jev-fact="bound"]')?.dataset.tip ?? null,
      };
      const recallWeek = fact("recall", "week");
      // Each feature's state (t-6243 D2): its chip and tone, one reason, the
      // samples still owed while it wants some — in words; the bar that fills
      // toward the judgment is the chart's over the table (t-9633) — and any
      // fact beside them.
      const statusOf = (id) => {
        const holder = cell(id, "status");
        const chip = holder.querySelector(".jev-chip");
        const words = holder.textContent.trim().split(/\s+/).filter((word) => word && !/^[·—–\-/|:]+$/.test(word));
        return {
          chip: chip?.textContent ?? null, tone: chip?.dataset.status ?? null,
          reason: holder.querySelector(".jev-status-reason")?.textContent ?? "",
          under: holder.querySelector(".jev-status-reason")?.classList.contains("is-under") ?? false,
          chips: holder.querySelectorAll(".jev-chip").length,
          owed: holder.querySelector(".jev-owed")?.textContent ?? null,
          bars: holder.querySelectorAll(".jev-progress, [role=\"progressbar\"]").length,
          facts: [...holder.querySelectorAll(".jev-status-fact")].map((one) => one.textContent),
          // A separator between facts is spaced (" · "); the dot inside a
          // word pair (키·동의) is Korean punctuation, not a separator.
          words: words.length, dots: holder.textContent.includes(" · "),
        };
      };
      const statuses = Object.fromEntries(rows.map((row) => [row.dataset.jevDashRow, statusOf(row.dataset.jevDashRow)]));
      const caption = view().querySelector(".jev-table caption")?.textContent ?? null;
      // The strip over the table (t-6243 D1): each figure by its own name.
      const strip = Object.fromEntries([...view().querySelectorAll("[data-jev-stat]")]
        .map((one) => [one.dataset.jevStat, one.querySelector("dd").textContent]));
      const foldRow = view().querySelector("[data-jev-fold]");
      const bodyRows = [...view().querySelectorAll(".jev-table tbody tr")];
      const fold = foldRow ? {
        text: foldRow.textContent, expanded: foldRow.querySelector("button").getAttribute("aria-expanded"),
        at: bodyRows.indexOf(foldRow),
      } : null;
      // The week's accuracy picture (t-6243 D4, t-9633 (b)): how many, which
      // lines with how many points, and what the cell says when there is none.
      const trendOf = (id) => {
        const holder = cell(id, "trend");
        const picture = holder.querySelectorAll('svg[data-jev-picture="trend"]');
        return {
          pictures: picture.length,
          lines: [...holder.querySelectorAll('svg[data-jev-picture="trend"] [data-line]')].map((line) => `${line.dataset.line}:${line.dataset.points}`),
          text: holder.querySelector(".jev-trend-short")?.textContent.trim() ?? "",
          legend: [...holder.querySelectorAll(".jev-trend-legend .jev-trend-last")].map((one) => `${one.dataset.tip} ${one.textContent}`).join("|"),
        };
      };
      const trends = Object.fromEntries(["summon", "placement", "routing", "recall", "notify", "browser"].map((id) => [id, trendOf(id)]));
      // What the rate and accuracy cells say (t-6243 D3).
      const said = (id) => ({ answered: cell(id, "answered").textContent, agreement: fact(id, "agreement") });
      const small = Object.fromEntries(["summon", "placement", "routing", "notify", "browser"].map((id) => [id, said(id)]));
      // The switch over the table (§6.1), and whether any feature still
      // offers a mode to choose.
      const mirror = view().querySelector(".jev-switch");
      const switchOver = mirror ? {
        checked: mirror.querySelector("[data-jev-switch]")?.checked ?? null,
        id: mirror.querySelector("[id]")?.id ?? null,
        partial: !mirror.querySelector("[data-jev-partial]")?.hidden,
        said: mirror.querySelector("[data-jev-partial-said]")?.textContent ?? "",
        beforeTable: Boolean(mirror.compareDocumentPosition(view().querySelector(".jev-table")) & Node.DOCUMENT_POSITION_FOLLOWING),
      } : null;
      const tableSelects = view().querySelectorAll(".jev-table select").length;
      return {
        ms, active: activeTabId, visible: !view().hidden, hiddenAttr: view().hidden,
        rowIds: rows.map((row) => row.dataset.jevDashRow),
        rowNames: rows.map((row) => row.querySelector('[data-jev-cell="seat"] .jev-row-open').textContent),
        cardNames, cardSeats: window.__JEV__.seats, summon, recallWeek, statuses, caption, small, trends, switchOver, tableSelects,
        strip, fold, bodyRows: bodyRows.length,
        asks: window.__JEV__.asks.slice(), settingsAsks: window.__COUNTS__.typesafe_settings ?? 0,
        summaryAsks: window.__COUNTS__.jev_summary ?? 0, dayAsks: window.__COUNTS__.jev_day ?? 0,
        title: tabLabel(tabs.find((tab) => tab.id === "jev")),
        pressed: el("nav-jev").getAttribute("aria-pressed"),
        heads: [...view().querySelectorAll("thead th")].map((th) => th.scope),
        selects: view().querySelectorAll("select").length,
        drawerHidden: view().querySelector(".jev-drawer").hidden,
        pollMs: JEV_POLL_MS, recentRows: JEV_RECENT_ROWS,
        fresh: view().querySelector(".jev-freshness").textContent,
      };
    });
    ok("the rail door opens the Jev tab and wears its pressed state",
      opened.active === "jev" && opened.visible && opened.pressed === "true", JSON.stringify({ active: opened.active, pressed: opened.pressed }));
    // The features in use lead, busiest first — today's requests, then the
    // week's, then the card's order — and the ones nothing asked all week
    // fold into one row after them (t-6243 D1).
    const cardName = (id) => opened.cardNames[opened.cardSeats.indexOf(id)];
    ok("the table leads with the features in use, busiest first, and folds the unused ones into one row",
      opened.rowIds.join(",") === "summon,placement,recall,routing,notify,browser"
        && opened.rowNames.join("|") === opened.rowIds.map(cardName).join("|")
        && opened.fold !== null && opened.fold.at === 6 && opened.bodyRows === 7
        && opened.fold.text.includes(String(SEATS - 6)) && opened.fold.expanded === "false",
      JSON.stringify({ rows: opened.rowIds, names: opened.rowNames, fold: opened.fold, bodyRows: opened.bodyRows }));
    ok("the strip over the table counts today, the week, its cost, where the features stand and the day's limit",
      opened.strip.today === "44" && opened.strip.week === "1,156" && opened.strip.cost === "$0.328"
        && opened.strip.applying === "3" && opened.strip.recording === "2" && opened.strip.under === "1"
        && opened.strip.blocked === "1" && opened.strip.day === "247 / 500",
      JSON.stringify(opened.strip));
    ok("opening costs one settings ask, one summary ask with the recent list, and one read of the day's count",
      opened.settingsAsks === 1 && opened.summaryAsks === 1 && opened.dayAsks === 1
        && opened.asks.length === 1 && opened.asks[0].recent === opened.recentRows,
      JSON.stringify({ settings: opened.settingsAsks, summary: opened.summaryAsks, day: opened.dayAsks, asks: opened.asks }));

    // A 1080p screen holds the charts and every feature in use, one after
    // another, before any scroll (the plan's D1 measure; the charts, t-9633).
    await page.setViewportSize({ width: 1920, height: 1080 });
    const firstScreen = await page.evaluate(async () => {
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      view.scrollTop = 0;
      const box = view.getBoundingClientRect();
      const bottom = Math.min(box.bottom, window.innerHeight);
      const rows = [...view.querySelectorAll("[data-jev-dash-row]")];
      const charts = view.querySelector("[data-jev-charts]")?.getBoundingClientRect() ?? null;
      return {
        rows: rows.length,
        inside: rows.filter((row) => { const rect = row.getBoundingClientRect(); return rect.top >= box.top - 1 && rect.bottom <= bottom + 1; }).length,
        charts: charts !== null && charts.height > 0 && charts.top >= box.top - 1 && charts.bottom <= bottom + 1,
        // Room under the first row, and the tallest row: FIRST_SCREEN_ROWS
        // rows of that height must fit under the charts.
        room: Math.round(bottom - rows[0].getBoundingClientRect().top),
        tallest: Math.round(Math.max(...rows.map((row) => row.getBoundingClientRect().height))),
        // Which cell holds the tallest row up, for the reader of a failure.
        tallestCells: (() => {
          const row = rows.reduce((a, b) => (b.getBoundingClientRect().height > a.getBoundingClientRect().height ? b : a));
          return [row.dataset.jevDashRow, ...[...row.querySelectorAll("td")]
            .map((td) => `${td.dataset.jevCell}:${Math.round(td.firstElementChild?.getBoundingClientRect().height ?? td.getBoundingClientRect().height)}`)];
        })(),
      };
    });
    ok(`a 1080p screen holds the charts and every feature in use before any scroll, and room for ${FIRST_SCREEN_ROWS}`,
      firstScreen.charts && firstScreen.rows === 6 && firstScreen.inside === 6 && firstScreen.tallest * FIRST_SCREEN_ROWS <= firstScreen.room,
      JSON.stringify(firstScreen));

    // Why a feature waiting on comparisons compares nothing (t-9935): held
    // on a line of the comparing kind — too few compared, no outcome yet, an
    // outcome that never says wrong — its state names, beside what it still
    // owes, the reasons its week's rows compared nothing, the most frequent
    // first, in words and with how many; a reason no table words stands as
    // itself. The cell still says at most fourteen words with no separator
    // dots, and the row stands no taller than a 1080p screen has room for.
    // A feature held on a line of another kind names no reason.
    const whyRows = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const saved = jevNumbers;
      const read = (id) => {
        const holder = view.querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="status"]`);
        const why = holder.querySelector("[data-jev-why]");
        const words = holder.textContent.trim().split(/\s+/).filter((word) => word && !/^[·—–\-/|:]+$/.test(word));
        return {
          why: why?.textContent ?? null, tip: why?.dataset.tip ?? null,
          reason: holder.querySelector(".jev-status-reason")?.textContent ?? "",
          owed: holder.querySelector(".jev-owed")?.textContent ?? null,
          words: words.length, dots: holder.textContent.includes(" · "),
        };
      };
      const room = () => {
        view.scrollTop = 0;
        const rows = [...view.querySelectorAll("[data-jev-dash-row]")];
        const tallest = rows.reduce((a, b) => (b.getBoundingClientRect().height > a.getBoundingClientRect().height ? b : a));
        return {
          height: Math.round(view.querySelector('[data-jev-dash-row="placement"]').getBoundingClientRect().height),
          tallest: Math.round(tallest.getBoundingClientRect().height),
          // Which cell holds the tallest row up, for the reader of a failure.
          tallestCells: [tallest.dataset.jevDashRow, ...[...tallest.querySelectorAll("td")].map((td) =>
            `${td.dataset.jevCell}:${Math.round(td.firstElementChild?.getBoundingClientRect().height ?? td.getBoundingClientRect().height)}/${Math.round(td.getBoundingClientRect().width)}`)],
        };
      };
      const held = async (line, notComparedBy) => {
        jevNumbers = saved.map((seat) => (seat.id !== "placement" ? seat : {
          ...seat, verdict: { verdict: "hold", line },
          judged: { ...seat.judged, agreement: { ...seat.judged.agreement, compared: 5 } },
          agreementWeek: { ...seat.agreementWeek, notComparedBy },
        }));
        paintJevViews();
        await window.__PAINTED__();
        return { ...read("placement"), ...room() };
      };
      const agreement = read("placement");
      const compared = await held("too_few_compared", { away: 1, unseen: 95, not_carried: 3 });
      const unlabeled = await held("unlabeled", { unseen: 7 });
      const oneSided = await held("one_sided", { not_carried: 2 });
      const stranger = await held("too_few_compared", { mystery_word: 5 });
      jevNumbers = saved;
      paintJevViews();
      await window.__PAINTED__();
      return { agreement, compared, unlabeled, oneSided, stranger, after: read("placement") };
    });
    ok("a feature held for want of comparisons names, beside its state, the two reasons its week compared nothing most often",
      whyRows.agreement.why === null && whyRows.after.why === null
        && whyRows.compared.why === "아무도 안 봄 95건, 실행 안 됨 3건"
        && Boolean(whyRows.compared.tip?.includes("비교하지 못한 판단"))
        && whyRows.compared.owed === "정확도 판정까지 15건"
        && whyRows.unlabeled.why === "아무도 안 봄 7건"
        && whyRows.oneSided.why === "실행 안 됨 2건"
        && whyRows.oneSided.reason === "틀렸다는 결과가 거의 없어 정확도를 믿을 수 없습니다"
        && whyRows.stranger.why === "mystery_word 5건",
      JSON.stringify(whyRows));
    // Measured against the first screen's own room: the row naming its
    // reasons stands no taller than the table's tallest did, so the seven
    // rows' room it was measured with still holds.
    ok(`a feature naming its reasons says at most fourteen words, no separator dots, and still leaves ${FIRST_SCREEN_ROWS} rows' room at 1080p`,
      whyRows.compared.words <= 14 && !whyRows.compared.dots && !whyRows.unlabeled.dots && !whyRows.oneSided.dots
        && whyRows.compared.height <= firstScreen.tallest && whyRows.compared.tallest <= firstScreen.tallest
        && whyRows.compared.tallest * FIRST_SCREEN_ROWS <= firstScreen.room,
      JSON.stringify({ compared: whyRows.compared, before: { tallest: firstScreen.tallest, room: firstScreen.room } }));

    // A feature whose confidence bar would apply too small a share of its
    // answers (t-9468's `apply_share`) is held under a line of its own and
    // says which — not that it cleared the bar.
    const applyShare = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const saved = jevNumbers;
      jevNumbers = saved.map((seat) => (seat.id === "placement" ? { ...seat, verdict: { verdict: "hold", line: "apply_share" } } : seat));
      paintJevViews();
      await window.__PAINTED__();
      const reason = view.querySelector('[data-jev-dash-row="placement"] .jev-status-reason');
      const seen = { reason: reason?.textContent ?? "", under: reason?.classList.contains("is-under") ?? false,
        strip: view.querySelector('[data-jev-stat="under"] dd').textContent };
      jevNumbers = saved;
      paintJevViews();
      await window.__PAINTED__();
      return seen;
    });
    ok("a feature held because too few answers clear its confidence bar says so, as a feature under its bar",
      applyShare.reason === "확신도 기준을 넘는 답이 너무 적습니다" && applyShare.under && applyShare.strip === "1",
      JSON.stringify(applyShare));
    ok("from the click to the drawn table is under the design's 200 ms with the backend answering at once",
      opened.ms < 200, `${opened.ms.toFixed(1)} ms`);
    ok("a counted seat's numbers stand in its cells",
      opened.summon.today === "32" && opened.summon.week === "50" && opened.summon.answered.startsWith("96%")
        && opened.summon.answered.includes("84%")
        && opened.summon.applied === "0"
        && opened.summon.agreement === "16/44 (24%)" && opened.summon.cost === "$0.0123"
        && opened.summon.latency.join("|") === "230 ms|느릴 때 410" && opened.recallWeek === "1,005",
      JSON.stringify(opened.summon));
    // One chip, one reason, and — while the feature still wants samples —
    // a bar with how many remain until the check, which then is the reason:
    // nothing said twice (t-6243 D2).
    const st = opened.statuses;
    ok("each feature's state is one chip, one reason and, while it wants samples, how many remain until the check",
      Object.values(st).every((one) => one.chips === 1 && one.bars === 0 && ["꺼짐", "기록 중", "적용 중", "키 필요", "동의 필요"].includes(one.chip))
        && st.summon.chip === "적용 중" && st.summon.tone === "applying" && st.summon.reason === "직접 켰습니다 — 응답률이 기준에 못 미칩니다"
        && st.summon.owed === null && st.summon.facts.join("|") === "이전 버전 jev-1.12.0의 기록은 제외"
        && st.routing.chip === "적용 중" && st.routing.reason === "직접 켰습니다"
        && st.routing.owed === "판정까지 38건"
        && st.recall.chip === "기록 중" && st.recall.tone === "recording" && st.recall.reason === "자동 적용 대상이 아니라 기록만 합니다"
        && st.placement.chip === "기록 중" && st.placement.tone === "recording" && st.placement.reason === "정확도가 기준에 못 미칩니다"
        && st.placement.under && !st.recall.under
        && st.notify.chip === "동의 필요" && st.notify.tone === "blocked"
        && st.notify.reason === "동의하지 않은 폴더라 판단을 보내지 않았습니다",
      JSON.stringify(st));
    // A small sample says its size, not a share: a lower bound over a few
    // rows is the width of its interval, and 0/3 reads as a failing feature
    // when it is one not yet judged (t-6243 D3).
    const small = opened.small;
    ok("a small sample says its size rather than a share, and a feature refused throughout says so",
      small.browser.answered === "표본 4건" && small.notify.answered === "거절 2건"
        && small.summon.answered === "96% 신뢰 하한 84%"
        && small.routing.agreement === "표본 3건" && small.summon.agreement === "16/44 (24%)"
        && small.placement.agreement === "12/21 (37%)",
      JSON.stringify(small));
    // Every failure a feature's week carries is a chip in words; the ones a
    // person fixes are buttons that go where the fix is (t-6243 D5).
    const chips = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const chipsOf = (id) => [...view.querySelectorAll(`[data-jev-dash-row="${id}"] [data-jev-cell="rows"] .jev-token`)]
        .map((chip) => `${chip.dataset.token}|${chip.textContent}|${chip.tagName === "BUTTON" ? "button" : "word"}`);
      return { routing: chipsOf("routing"), recall: chipsOf("recall"), notify: chipsOf("notify"), summon: chipsOf("summon"),
        placement: chipsOf("placement") };
    });
    ok("every failure a feature's week carries is a chip in words, and the ones a person fixes are buttons",
      chips.routing.join(",") === "no_key|API 키 없음 3|button,timeout|시간 초과 1|word"
        && chips.recall.join(",") === "schema|형식 오류 65|word,timeout|시간 초과 5|word"
        && chips.notify.join(",") === "not_consented|동의 안 된 폴더 2|button"
        && chips.summon.join(",") === "not_consented|동의 안 된 폴더 2|button" && chips.placement.length === 0,
      JSON.stringify(chips));
    const fixed = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      view.querySelector('[data-jev-dash-row="routing"] .jev-token[data-token="no_key"]').click();
      await window.__PAINTED__();
      const key = { open: !settingsView.hidden, focus: document.activeElement?.id ?? null };
      setSettingsOpen(false);
      await window.__PAINTED__();
      view.querySelector('[data-jev-dash-row="notify"] .jev-token[data-token="not_consented"]').click();
      await window.__PAINTED__();
      const consent = { open: !settingsView.hidden, said: document.querySelector("#typesafe-status")?.textContent ?? "",
        focus: document.activeElement?.id ?? null };
      setSettingsOpen(false);
      await window.__PAINTED__();
      return { key, consent, back: activeTabId };
    });
    ok("a key chip opens settings on the key field, and a consent chip lands on the switch that consents every folder",
      fixed.key.open && fixed.key.focus === "typesafe-key-input" && fixed.consent.open
        && fixed.consent.focus === "jev-enabled" && fixed.consent.said.includes("「모든 폴더에서 사용」")
        && fixed.back === "jev",
      JSON.stringify(fixed));
    ok("the model is named once, over the table, and a row names a version only when it is another",
      opened.caption === "모델 jev-1.13.0" && Object.values(st).every((one) => !one.facts.some((fact) => fact.startsWith("모델 "))),
      JSON.stringify({ caption: opened.caption }));
    ok("the switch stands over the table as the card's, in use from some folders, and no feature offers a mode",
      opened.switchOver !== null && opened.switchOver.checked === true && opened.switchOver.id === null
        && opened.switchOver.partial && opened.switchOver.said === "일부 폴더(4개)에서만 사용 중입니다."
        && opened.switchOver.beforeTable && opened.tableSelects === 0,
      JSON.stringify({ switchOver: opened.switchOver, tableSelects: opened.tableSelects }));
    ok("a state cell says at most fourteen words and no separator dots",
      Object.values(st).every((one) => one.words <= 14 && !one.dots),
      JSON.stringify(Object.fromEntries(Object.entries(st).map(([id, one]) => [id, one.words]))));
    ok("the response rate's lower bound says, as its tip, where it stands against the bar for automatic use",
      (opened.summon.boundTip ?? "").includes("90%") && opened.summon.boundTip.includes("미만"),
      opened.summon.boundTip ?? "no tip");
    // One press unfolds the unused features after the fold, in the card's
    // order and under its names; the choice is remembered, so the next open
    // stands the same way (t-6243 D1).
    const unfolded = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      view.querySelector("[data-jev-fold] button").click();
      await window.__PAINTED__();
      const rows = () => [...view.querySelectorAll("[data-jev-dash-row]")].map((row) => row.dataset.jevDashRow);
      const opened = { rows: rows(), expanded: view.querySelector("[data-jev-fold] button").getAttribute("aria-expanded"),
        stored: localStorage.getItem("zerocode.jev-unused-open.v1") };
      dropTab("jev");
      await window.__PAINTED__();
      el("nav-jev").click();
      await window.__PAINTED__();
      opened.reopened = rows();
      const again = document.querySelector("#jev-view");
      const cell = (id, name) => again.querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="${name}"]`);
      const fact = (id, name) => again.querySelector(`[data-jev-dash-row="${id}"] [data-jev-fact="${name}"]`)?.textContent ?? null;
      opened.quiet = { week: fact("skills", "week"), chips: cell("skills", "rows").querySelectorAll(".jev-token").length,
        trend: cell("skills", "trend").textContent.trim(), pictures: cell("skills", "trend").querySelectorAll("svg").length,
        chip: cell("skills", "status").querySelector(".jev-chip")?.textContent ?? null,
        tone: cell("skills", "status").querySelector(".jev-chip")?.dataset.status ?? null,
        reason: cell("skills", "status").querySelector(".jev-status-reason")?.textContent ?? "" };
      // A feature switched on that nothing asked all week stands where its
      // switch puts it, and says why its row is empty.
      opened.idle = { chip: cell("desktop", "status").querySelector(".jev-chip")?.textContent ?? null,
        tone: cell("desktop", "status").querySelector(".jev-chip")?.dataset.status ?? null,
        reason: cell("desktop", "status").querySelector(".jev-status-reason")?.textContent ?? "" };
      return opened;
    });
    const inUse = ["summon", "placement", "recall", "routing", "notify", "browser"];
    const unusedInOrder = opened.cardSeats.filter((id) => !inUse.includes(id));
    ok("one press unfolds the unused features in the card's order, and the next open remembers it",
      unfolded.rows.join(",") === [...inUse, ...unusedInOrder].join(",")
        && unfolded.expanded === "true" && unfolded.stored === "1" && unfolded.reopened.join(",") === unfolded.rows.join(","),
      JSON.stringify({ rows: unfolded.rows, expanded: unfolded.expanded, stored: unfolded.stored, reopened: unfolded.reopened.length }));
    const tr = opened.trends;
    ok("the week's accuracy is one picture — the judgment's share, the band down to its lower bound and the simplest method — drawn only from three graded days",
      tr.summon.pictures === 1 && tr.summon.lines.join(",") === "band:5,baseline:2,agreement:5"
        && tr.summon.legend === "정확도 88%|단순 방식 88%"
        && tr.placement.pictures === 1 && tr.placement.lines.join(",") === "band:3,baseline:3,agreement:3"
        && tr.routing.pictures === 0 && tr.routing.text === "1일치만 있음"
        && tr.recall.pictures === 0 && tr.recall.text === "비교한 판단 없음"
        && tr.notify.pictures === 0 && tr.notify.text === "비교한 판단 없음"
        && tr.browser.pictures === 0 && tr.browser.text === "비교한 판단 없음"
        && unfolded.quiet.pictures === 0 && unfolded.quiet.trend === "—",
      JSON.stringify({ ...tr, quiet: { pictures: unfolded.quiet.pictures, text: unfolded.quiet.trend } }));
    ok("a feature switched off says so in its one chip; one nothing asked all week says its row is empty, not zero",
      unfolded.quiet.week === "0" && unfolded.quiet.chips === 0 && unfolded.quiet.chip === "꺼짐"
        && unfolded.quiet.tone === "dormant" && unfolded.quiet.reason === ""
        && unfolded.idle.chip === "적용 중" && unfolded.idle.tone === "applying"
        && unfolded.idle.reason === "지난 7일 판단 요청이 없었습니다",
      JSON.stringify({ quiet: unfolded.quiet, idle: unfolded.idle }));
    ok("the dashboard holds no select at all, and its drawer starts closed (t-6277 D6/D9)",
      opened.selects === 0 && opened.drawerHidden === true,
      JSON.stringify({ selects: opened.selects, drawerHidden: opened.drawerHidden }));
    ok("the tab is titled and the head cells are column heads",
      opened.title !== null && opened.title.length > 0 && opened.heads.every((scope) => scope === "col"), JSON.stringify({ title: opened.title, heads: opened.heads.length }));
    ok("the freshness line names the ask's own cost", /\d+ ms/.test(opened.fresh), opened.fresh);

    // A row opens its drawer (t-6277 D6): the feature's last judgments, its
    // comparisons, what a screen feature's guards stopped, the model and what
    // it sends — all from what is in hand, so opening asks zo nothing.
    const drawer = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const before = window.__COUNTS__.jev_summary;
      const read = () => {
        const drawer = view.querySelector(".jev-drawer");
        const part = (name) => drawer.querySelector(`[data-jev-drawer-part="${name}"]`);
        const facts = (name) => [...part(name).querySelectorAll("dt")].map((term) =>
          `${term.textContent}=${term.nextElementSibling?.textContent ?? ""}`);
        return {
          hidden: drawer.hidden,
          title: drawer.querySelector(".jev-drawer-title").textContent,
          id: drawer.querySelector(".jev-drawer-id").textContent,
          summary: drawer.querySelector(".jev-drawer-summary").textContent,
          written: !drawer.querySelector(".jev-drawer-written").hidden,
          decisions: drawer.querySelectorAll(".jev-decision").length,
          first: drawer.querySelector(".jev-decision")?.textContent ?? "",
          refused: drawer.querySelector('.jev-decision[data-outcome="not_consented"] .jev-decision-outcome')?.textContent ?? null,
          marks: [...drawer.querySelectorAll(".jev-decision-marks")].map((one) => one.textContent),
          agreement: facts("agreement"),
          guardsHidden: part("guards").hidden,
          guards: facts("guards"),
          version: facts("version"),
          sends: part("sends").querySelector(".jev-drawer-text").textContent,
          open: [...view.querySelectorAll("[data-jev-dash-row].is-open")].map((row) => row.dataset.jevDashRow),
          expanded: view.querySelector(`[data-jev-dash-row="${drawer.querySelector(".jev-drawer-id").textContent}"] .jev-row-open`)
            ?.getAttribute("aria-expanded") ?? null,
          focus: document.activeElement?.className ?? "",
        };
      };
      view.querySelector('[data-jev-dash-row="routing"] [data-jev-cell="latency"]').click();
      await window.__PAINTED__();
      const routing = read();
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      const summon = read();
      view.querySelector('[data-jev-dash-row="browser"] .jev-row-open').click();
      await window.__PAINTED__();
      const browser = read();
      // The page scrolls; the drawer stays over its window onto the table.
      view.scrollTop = view.scrollHeight;
      await window.__PAINTED__();
      const box = view.getBoundingClientRect();
      const rect = view.querySelector(".jev-drawer").getBoundingClientRect();
      const pinned = { top: Math.round(rect.top - box.top), bottom: Math.round(box.bottom - rect.bottom),
        right: Math.round(box.right - rect.right), scrolled: view.scrollTop };
      view.scrollTop = 0;
      // Escape closes it and hands the focus back to the row that opened it;
      // the open row's own press closes it too.
      view.querySelector(".jev-drawer-close").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await window.__PAINTED__();
      const escaped = { hidden: view.querySelector(".jev-drawer").hidden, focus: document.activeElement?.closest("[data-jev-dash-row]")?.dataset.jevDashRow ?? null };
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      const toggled = view.querySelector(".jev-drawer").hidden;
      return { routing, summon, browser, pinned, escaped, toggled, asks: window.__COUNTS__.jev_summary - before };
    });
    const dr = drawer;
    ok("a row opens its drawer on the feature's last judgments, and opening asks zo nothing",
      !dr.routing.hidden && dr.routing.id === "routing" && dr.routing.title === opened.cardNames[opened.cardSeats.indexOf("routing")]
        && dr.routing.decisions === 1 && dr.routing.first.includes("task: 68212a1194a4e327")
        && dr.routing.first.includes("complexity=small") && dr.routing.first.includes("적용")
        && dr.summon.decisions === 5 && dr.summon.refused === "동의 안 된 폴더"
        && dr.summon.marks[0].includes("확신도 19%") && dr.summon.marks[0].includes("적용됨") && dr.summon.marks[0].includes("일치")
        && dr.summon.marks[1].includes("기록만")
        && dr.summon.open.join(",") === "summon" && dr.summon.expanded === "true" && dr.summon.focus.includes("jev-drawer-close")
        && dr.asks === 0,
      JSON.stringify({ routing: dr.routing, summonDecisions: dr.summon.decisions, marks: dr.summon.marks, asks: dr.asks }));
    ok("the drawer says how often the judgment matched, the model and the version cut away, and what the feature sends",
      dr.summon.agreement.join("|") === "판정 표본=16/44건 일치 · 신뢰 하한 24%|지난 7일=20/50건 일치"
        && dr.summon.version.join("|") === "응답한 모델=모델 jev-1.13.0|버전 변경=이전 버전 jev-1.12.0의 기록은 제외"
        && dr.summon.sends.length > 80 && dr.summon.summary.length > 10 && dr.summon.written
        && dr.routing.agreement.join("|") === "판정 표본=3/3건 일치 · 신뢰 하한 43% · 대조 표본 1건 포함",
      JSON.stringify({ agreement: dr.summon.agreement, version: dr.summon.version, routing: dr.routing.agreement }));
    ok("a screen feature's drawer says what its guards stopped and what it handed to the person; others say nothing of it",
      dr.summon.guardsHidden && dr.routing.guardsHidden && !dr.browser.guardsHidden
        && dr.browser.guards.join("|") === "화면의 글이 지시해 멈춤=2|로그인·오류 화면이라 멈춤=1|되돌릴 수 없는 조작을 사람에게=1건 (조작 판단 4건 중)",
      JSON.stringify({ browser: dr.browser.guards }));
    ok("the drawer stays over the page's window while the table scrolls, and Escape or a second press closes it",
      dr.pinned.scrolled > 0 && Math.abs(dr.pinned.top) <= 1 && Math.abs(dr.pinned.bottom) <= 1 && Math.abs(dr.pinned.right) <= 1
        && dr.escaped.hidden && dr.escaped.focus === "browser" && dr.toggled,
      JSON.stringify({ pinned: dr.pinned, escaped: dr.escaped, toggled: dr.toggled }));

    // What stands between a feature and automatic use, in its drawer
    // (t-9935): the check's result — the line it is held on, every bar
    // cleared, or none to clear — the confidence bar it acts from with what
    // that bar applies and how often that is wrong beside the simplest
    // method, or why its record draws none (a judge's own line in that
    // line's words), and why its week's rows compared nothing, reason by
    // reason with the week's count, its share and today's.
    const whyDrawer = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const saved = jevNumbers;
      const read = async (id) => {
        if (jevDrawerSeat !== id) view.querySelector(`[data-jev-dash-row="${id}"] .jev-row-open`).click();
        await window.__PAINTED__();
        const part = view.querySelector('.jev-drawer [data-jev-drawer-part="why"]');
        if (!part) return null;
        return {
          hidden: part.hidden,
          facts: [...part.querySelectorAll("dt")].map((term) => `${term.textContent}=${term.nextElementSibling?.textContent ?? ""}`),
          head: part.querySelector("[data-jev-why-head]")?.hidden === false ? part.querySelector("[data-jev-why-head]").textContent : null,
          reasons: [...part.querySelectorAll("[data-jev-reason]")].map((item) => [item.dataset.jevReason,
            item.querySelector(".jev-judgment-name")?.textContent ?? "", item.querySelector(".jev-judgment-owed")?.textContent ?? "",
            item.querySelector('svg[data-jev-picture] [data-values]')?.dataset.values ?? ""].join("|")),
          text: part.textContent,
        };
      };
      const again = async (id, edit) => {
        jevNumbers = saved.map((seat) => (seat.id === id ? edit(seat) : seat));
        paintJevViews();
        return read(id);
      };
      const placement = await read("placement");
      const summon = await read("summon");
      const routing = await read("routing");
      const recall = await read("recall");
      const unread = await again("summon", (seat) => ({ ...seat, calibration: { ...seat.calibration, readsActLine: false } }));
      const drawn = await again("summon", (seat) => ({ ...seat, calibration: { ...seat.calibration, reason: null, actFromPermille: 800,
        drawn: { fromPermille: 800, marks: 25, applyShare: 0.36, errorPermille: 40, baselineErrorPermille: 280, underErrorPermille: 350 } } }));
      jevNumbers = saved;
      paintJevViews();
      setLocale("en");
      const english = await read("placement");
      setLocale("ko");
      view.querySelector(".jev-drawer-close").click();
      await window.__PAINTED__();
      return { placement, summon, routing, recall, unread, drawn, english: english?.text ?? null };
    });
    const wd = whyDrawer;
    ok("a drawer says the check's result, the confidence bar the feature acts from with what it applies and how often that is wrong",
      wd.placement !== null && !wd.placement.hidden
        && wd.placement.facts.join("|") === "판정 결과=정확도가 기준에 못 미칩니다|확신도 기준=70% 이상|적용 몫=45%|적용한 답의 오류=66‰|단순 방식의 오류=333‰"
        && wd.summon?.facts.join("|") === "판정 결과=응답률이 기준에 못 미칩니다|확신도 기준=없음 — 확신도가 높은 답이 더 자주 틀림"
        && wd.routing?.facts.join("|") === "판정 결과=판단 기록이 더 쌓여야 합니다|확신도 기준=없음 — 정확도를 비교할 표본이 더 필요합니다"
        && wd.recall?.facts.join("|") === "판정 결과=자동 적용 대상이 아니라 기록만 합니다"
        && wd.unread?.facts.join("|") === "판정 결과=응답률이 기준에 못 미칩니다|확신도 기준=쓰지 않음 — 확신도와 상관없이 답을 모두 적용합니다"
        && wd.drawn?.facts.join("|") === "판정 결과=응답률이 기준에 못 미칩니다|확신도 기준=80% 이상 — 기록이 가리키지만 아직 쓰지 않음|적용 몫=36%|적용한 답의 오류=40‰|단순 방식의 오류=280‰",
      JSON.stringify({ placement: wd.placement?.facts, summon: wd.summon?.facts, routing: wd.routing?.facts, recall: wd.recall?.facts,
        unread: wd.unread?.facts, drawn: wd.drawn?.facts }));
    ok("a drawer says why its week's rows compared nothing, reason by reason — the week's count, its share and today's — each with its bar",
      wd.placement?.head === "비교하지 못한 판단 98건 — 지난 7일, 까닭별"
        && wd.placement.reasons.join(",") === "unseen|아무도 안 봄|95건 (97%) · 오늘 4건|95/98,not_carried|실행 안 됨|3건 (3%) · 오늘 0건|3/98"
        && wd.recall?.reasons.join(",") === "no_note_touched|노트 안 씀|34건 (100%) · 오늘 5건|34/34"
        && wd.summon?.head === null && wd.summon.reasons.length === 0,
      JSON.stringify({ placement: [wd.placement?.head, wd.placement?.reasons], recall: wd.recall?.reasons, summon: [wd.summon?.head, wd.summon?.reasons] }));
    ok("the drawer's check, bar and reasons speak the language in force",
      wd.english !== null && !/[가-힣]/.test(wd.english) && wd.english.includes("nobody looked"),
      JSON.stringify(wd.english));

    // Each feature names its id under its name, small, and says what it
    // judges in one sentence as its tip — the first sentence of the paragraph
    // the drawer shows whole, read from the same key (t-6277 D7). The tip is
    // the window's own node, never the operating system's `title`.
    const named = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const folded = !jevUnusedOpen;
      if (folded) {
        view.querySelector("[data-jev-fold] button").click();
        await window.__PAINTED__();
      }
      const template = document.getElementById("jev-features").content;
      const firstSentence = (text) => {
        const end = text.search(/[.!?](\s|$)|[。！？]/u);
        return end < 0 ? text : text.slice(0, end + 1).trim();
      };
      const rows = [...view.querySelectorAll("[data-jev-dash-row]")].map((row) => {
        const id = row.dataset.jevDashRow;
        const hint = template.querySelector(`[data-jev-feature="${id}"] [data-jev-hint]`);
        return {
          id,
          shownId: row.querySelector(".jev-row-id")?.textContent ?? null,
          tip: row.querySelector(".jev-row-open")?.dataset.tip ?? null,
          expected: firstSentence(t(hint.dataset.i18n, hint.textContent.replace(/\s+/g, " ").trim())),
        };
      });
      const titled = view.querySelectorAll("[title]").length;
      const button = view.querySelector('[data-jev-dash-row="placement"] .jev-row-open');
      button.focus();
      await window.__PAINTED__();
      const tooltip = document.querySelector(".tooltip");
      const shown = { text: tooltip?.textContent ?? null, hidden: tooltip?.hidden ?? true };
      button.blur();
      if (folded) {
        view.querySelector("[data-jev-fold] button").click();
        await window.__PAINTED__();
      }
      return { rows, titled, shown };
    });
    const unnamed = named.rows.filter((row) => row.shownId !== row.id || !row.tip || row.tip !== row.expected);
    ok("every feature names its id under its name and tips what it judges in one sentence, from its own key",
      named.rows.length === SEATS && unnamed.length === 0 && named.rows.every((row) => row.tip.length < row.expected.length + 1)
        && named.titled === 0 && !named.shown.hidden
        && named.shown.text === named.rows.find((row) => row.id === "placement").tip,
      JSON.stringify({ rows: named.rows.length, unnamed: unnamed.slice(0, 3), titled: named.titled, shown: named.shown }));

    // The switch over the table goes through the card's door, and the card's
    // own switch follows — one state, two surfaces (§6.1): the line's press
    // consents every folder, and a press off turns both switches off.
    const pressed = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const settle = async () => {
        await new Promise((done) => setTimeout(done, 50));
        await window.__PAINTED__();
      };
      view.querySelector(".jev-switch [data-jev-everywhere]").click();
      await settle();
      const everywhere = {
        partial: !view.querySelector(".jev-switch [data-jev-partial]").hidden,
        card: document.getElementById("jev-enabled").checked,
        cardPartial: !document.querySelector("#typesafe-card [data-jev-partial]").hidden,
      };
      const toggle = view.querySelector(".jev-switch [data-jev-switch]");
      toggle.click();
      await settle();
      return {
        calls: window.__COUNTS__.set_jev_enabled ?? 0,
        pressed: window.__JEV__.pressed.slice(),
        everywhere,
        dashboard: toggle.checked,
        card: document.getElementById("jev-enabled").checked,
        status: view.querySelector(".jev-status").textContent,
        statusHidden: view.querySelector(".jev-status").hidden,
      };
    });
    ok("the switch over the table goes through set_jev_enabled and the card follows",
      pressed.calls === 2 && pressed.pressed.join(",") === "true,false"
        && !pressed.everywhere.partial && pressed.everywhere.card && !pressed.everywhere.cardPartial
        && pressed.dashboard === false && pressed.card === false
        && !pressed.statusHidden && pressed.status === "껐습니다. 아무것도 보내지 않습니다.",
      JSON.stringify(pressed));

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
      refreshed.reopened === refreshed.after && refreshed.rows === SEATS && refreshed.closedPressed === "false", JSON.stringify(refreshed));
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
      english.head === "Feature" && english.title === "Jev dashboard" && english.refresh === "Refresh" && english.back === "기능",
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
      !refused.hidden && refused.error.includes("refused: jev_summary") && refused.rows === SEATS && refused.week === "50",
      JSON.stringify(refused));

    // One frame and one edge (t-6277 D10): the switch over the table is the
    // card's framed row and the line under it, standing at the page's edges
    // as the strip and the table do — not a box around a box; and a word
    // under a count starts where the count starts, while a button's tint is
    // what lines up with it.
    const edges = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const holder = view.querySelector(".jev-switch");
      // On from some folders only, so the line under the switch stands.
      const held = typesafeState.jev;
      typesafeState = { ...typesafeState, jev: { ...held, on: true, everywhere: false, folders: 4 } };
      paintJevSwitches();
      await window.__PAINTED__();
      const box = (node) => node.getBoundingClientRect();
      const framed = (node) => parseFloat(getComputedStyle(node).borderTopWidth) > 0;
      const text = (node) => {
        const range = document.createRange();
        range.selectNodeContents(node);
        return range.getBoundingClientRect();
      };
      const table = box(view.querySelector(".jev-table-wrap"));
      const row = holder.querySelector("[data-jev-switch-row]");
      const partial = holder.querySelector("[data-jev-partial]");
      const cell = (id) => view.querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="rows"]`);
      const count = (id) => box(cell(id).querySelector('[data-jev-fact="today"]')).left;
      const word = cell("recall").querySelector("span.jev-token");
      const button = cell("routing").querySelector("button.jev-token");
      const seen = {
        holderFramed: framed(holder),
        holderGround: getComputedStyle(holder).backgroundColor,
        rowFramed: framed(row),
        row: [box(row).left - table.left, box(row).right - table.right].map(Math.round),
        partialShown: !partial.hidden,
        partial: [box(partial).left - box(row).left, box(partial).right - box(row).right].map(Math.round),
        word: Math.round(text(word).left - count("recall")),
        button: Math.round(box(button).left - count("routing")),
      };
      typesafeState = { ...typesafeState, jev: held };
      paintJevSwitches();
      return seen;
    });
    ok("the switch over the table wears one frame at the table's edges, and a word under a count starts where the count does",
      !edges.holderFramed && edges.holderGround === "rgba(0, 0, 0, 0)" && edges.rowFramed && edges.partialShown
        && [...edges.row, ...edges.partial].every((gap) => Math.abs(gap) <= 1)
        && Math.abs(edges.word) <= 1 && Math.abs(edges.button) <= 1,
      JSON.stringify(edges));

    // The strip is read for its figures (t-6277 D10): each figure larger and
    // heavier than its label, and where the features stand set apart from
    // the sums before it.
    const strip = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const stats = [...view.querySelectorAll(".jev-stat:not([hidden])")];
      const size = (node) => parseFloat(getComputedStyle(node).fontSize);
      const lastTotal = stats.filter((one) => one.dataset.group === "totals").at(-1);
      const firstTotal = stats.find((one) => one.dataset.group === "totals");
      const firstState = stats.find((one) => one.dataset.group === "states");
      const between = (a, b) => (a && b ? Math.round(b.getBoundingClientRect().left - a.getBoundingClientRect().right) : null);
      return {
        scale: stats.map((one) => size(one.querySelector("dd")) / size(one.querySelector("dt"))),
        weights: stats.map((one) => Number(getComputedStyle(one.querySelector("dd")).fontWeight)),
        labels: stats.map((one) => Number(getComputedStyle(one.querySelector("dt")).fontWeight)),
        withinSums: between(firstTotal, firstTotal?.nextElementSibling),
        beforeStates: between(lastTotal, firstState),
      };
    });
    ok("the strip's figures are larger and heavier than their labels, and the states stand apart from the sums",
      strip.scale.every((scale) => scale >= 1.5) && strip.weights.every((weight, at) => weight > strip.labels[at])
        && strip.withinSums !== null && strip.beforeStates !== null && strip.beforeStates > strip.withinSums,
      JSON.stringify(strip));

    // The head stays at the page's top edge while the rows scroll under it
    // (t-6277 D10).
    const sticky = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      if (!jevUnusedOpen) {
        view.querySelector("[data-jev-fold] button").click();
        await window.__PAINTED__();
      }
      view.scrollTop = view.scrollHeight;
      await window.__PAINTED__();
      const box = view.getBoundingClientRect();
      const head = view.querySelector(".jev-table thead th").getBoundingClientRect();
      const seen = { scrolled: view.scrollTop, headTop: Math.round(head.top - box.top), headHeight: Math.round(head.height) };
      view.scrollTop = 0;
      view.querySelector("[data-jev-fold] button").click();
      await window.__PAINTED__();
      return seen;
    });
    ok("the table's head stays at the page's top edge while the rows scroll",
      sticky.scrolled > 0 && Math.abs(sticky.headTop) <= 1 && sticky.headHeight > 0, JSON.stringify(sticky));

    // No feature asked all week: the table says why — Jev is off, or it is
    // on and quiet (t-6277 D10).
    const empty = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const quietDay = (one) => ({ ...one.week, rows: 0, answered: 0, refused: 0, failures: [], refusals: [] });
      const saved = jevNumbers;
      jevNumbers = saved.map((one) => ({ ...one, today: quietDay(one), week: quietDay(one), days: [] }));
      paintJevViews();
      const said = () => view.querySelector("[data-jev-empty] .jev-empty")?.textContent ?? null;
      const held = typesafeState.jev;
      typesafeState = { ...typesafeState, jev: { ...held, on: true } };
      paintJevViews();
      const quiet = said();
      typesafeState = { ...typesafeState, jev: { ...held, on: false } };
      paintJevViews();
      const off = said();
      typesafeState = { ...typesafeState, jev: held };
      jevNumbers = saved;
      paintJevViews();
      return { quiet, off, after: said() };
    });
    ok("a week nobody asked anything says so, and says when it is because Jev is off",
      empty.quiet === "지난 7일 판단 요청이 없었습니다. 기능이 판단을 요청하면 여기에 쌓입니다."
        && empty.off === "Jev가 꺼져 있어 판단을 요청하지 않습니다. 위의 스위치로 켤 수 있습니다." && empty.after === null,
      JSON.stringify(empty));

    // Every text on the dashboard — the switch, the strip, the table, a
    // screen feature's drawer — and on the settings card with 고급 open
    // clears 4.5:1 in both treatments (t-6277 D10).
    const AxeBuilder = axeBuilder();
    const contrast = {};
    if (AxeBuilder) {
      for (const theme of ["dark", "light"]) {
        await setQualityTheme(page, theme);
        // The page with the drawer shut — an open drawer lies over the
        // table's right side, and text under it has no ground to measure —
        // then the drawer of a screen feature on its own.
        const dashboard = await contrastTable(page, AxeBuilder, "#jev-view");
        await page.evaluate(async () => {
          document.querySelector('#jev-view [data-jev-dash-row="browser"] .jev-row-open').click();
          await window.__PAINTED__();
        });
        const drawer = await contrastTable(page, AxeBuilder, "#jev-view .jev-drawer");
        await page.evaluate(async () => {
          document.querySelector("#jev-view .jev-drawer-close").click();
          setSettingsOpen(true, "jev-enabled");
          document.getElementById("typesafe-advanced").open = true;
          await window.__PAINTED__();
        });
        const card = await contrastTable(page, AxeBuilder, "#typesafe-card");
        await page.evaluate(() => {
          document.getElementById("typesafe-advanced").open = false;
          setSettingsOpen(false);
        });
        contrast[theme] = [
          ...dashboard.map((row) => ({ surface: "dashboard", ...row })),
          ...drawer.map((row) => ({ surface: "drawer", ...row })),
          ...card.map((row) => ({ surface: "card", ...row })),
        ];
      }
      await setQualityTheme(page, "dark");
    }
    const rows = Object.values(contrast).flat();
    // The table is evidence a person reads, beside the pictures (t-5807's
    // folder): every text's ink, ground and ratio, the least first.
    if (rows.length > 0) {
      await mkdir(EVIDENCE_DIR, { recursive: true });
      const least = (list) => [...list].sort((a, b) => (a.ratio ?? 0) - (b.ratio ?? 0));
      await writeFile(join(EVIDENCE_DIR, "jev-contrast.json"),
        `${JSON.stringify(Object.fromEntries(Object.entries(contrast).map(([theme, list]) => [theme, least(list)])), null, 2)}\n`);
    }
    const under = rows.filter((row) => row.verdict === "fail" || (row.ratio !== null && row.ratio < CONTRAST_FLOOR));
    const unknown = rows.filter((row) => row.verdict === "unknown" || row.ratio === null);
    ok(`every text on the dashboard and the card clears ${CONTRAST_FLOOR}:1 in both treatments`,
      AxeBuilder !== null && rows.length > 0 && under.length === 0 && unknown.length === 0,
      AxeBuilder === null ? "@axe-core/playwright is not installed" : JSON.stringify({
        measured: rows.length, least: Math.min(...rows.filter((row) => row.ratio !== null).map((row) => row.ratio)),
        under: under.slice(0, 6), unknown: unknown.slice(0, 6) }));

    ok("the dashboard raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* t-9633: the dashboard draws its numbers. Five pictures, each read off what
 * zo answered and nothing else: what each feature's week did day by day, and
 * where it stands now (a); its accuracy against the simplest method, with the
 * band down to the lower bound (b); how far each feature is from its next
 * judgment (c); how long its answers took day by day (d); and the input
 * tokens the week billed, with what they cost (e). Every picture says its
 * numbers to a screen reader, wears the stylesheet's colours in both
 * treatments, is drawn again only when its numbers moved, stays whole in a
 * narrow window and speaks every language. */
export async function testJevDashboardPictures(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1600, height: 1200 });
    await page.evaluate(jevDashboardFixture);
    // Opened before zo answers: the charts stand with the loading line, in
    // their own frames, rather than as nothing.
    const loading = await page.evaluate(async () => {
      const answer = window.__ANSWER__.jev_summary;
      let release = null;
      window.__ANSWER__.jev_summary = (args) => new Promise((done) => { release = () => done(answer(args)); });
      el("nav-jev").click();
      await new Promise((done) => {
        const look = () => (release ? done() : requestAnimationFrame(look));
        look();
      });
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      const seen = [...view.querySelectorAll("[data-jev-chart]")].map((chart) => ({
        chart: chart.dataset.jevChart,
        said: chart.querySelector(".jev-chart-empty:not([hidden])")?.textContent.trim() ?? null,
        pictures: chart.querySelectorAll("svg[data-jev-picture]").length,
      }));
      window.__ANSWER__.jev_summary = answer;
      release();
      return seen;
    });
    ok("before zo answers, each chart stands in its frame and says it is counting",
      loading.length === 2 && loading.every((one) => one.said === "기록을 집계하는 중…" && one.pictures === 0),
      JSON.stringify(loading));
    await page.waitForFunction(() => document.querySelector("#jev-view [data-jev-dash-row] [data-jev-fact=\"week\"]")?.textContent === "50");
    await settlePaint(page);

    // (e) and (c): the charts over the table, in that order.
    const charts = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const chart = (name) => view.querySelector(`[data-jev-chart="${name}"]`);
      const picture = (name) => chart(name)?.querySelector("svg[data-jev-picture]") ?? null;
      const table = (name) => [...(chart(name)?.querySelectorAll("table.sr tbody tr") ?? [])]
        .map((row) => [...row.children].map((cell) => cell.textContent.trim()));
      const band = view.querySelector("[data-jev-charts]");
      return {
        order: [...view.querySelectorAll("[data-jev-chart]")].map((one) => one.dataset.jevChart),
        beforeTable: Boolean(band && band.compareDocumentPosition(view.querySelector(".jev-table")) & Node.DOCUMENT_POSITION_FOLLOWING),
        tokens: {
          kind: picture("tokens")?.dataset.jevPicture ?? null,
          values: picture("tokens")?.querySelector("[data-values]")?.dataset.values ?? null,
          figure: chart("tokens")?.querySelector(".jev-chart-figure")?.textContent ?? null,
          cost: chart("tokens")?.querySelector(".jev-chart-sub")?.textContent ?? null,
          table: table("tokens"),
          role: picture("tokens")?.getAttribute("role") ?? null,
          label: picture("tokens")?.getAttribute("aria-label") ?? "",
        },
        judgment: {
          kind: picture("judgment")?.dataset.jevPicture ?? null,
          rows: [...(chart("judgment")?.querySelectorAll("[data-jev-judgment]") ?? [])].map((one) => one.dataset.jevJudgment),
          names: [...(chart("judgment")?.querySelectorAll("[data-jev-judgment] .jev-judgment-name") ?? [])].map((one) => one.textContent.trim()),
          owed: [...(chart("judgment")?.querySelectorAll(".jev-judgment-owed") ?? [])].map((one) => one.textContent.trim()),
          bars: [...(chart("judgment")?.querySelectorAll('svg[data-jev-picture="judgment"] [data-values]') ?? [])]
            .map((one) => one.dataset.values).join(","),
          figure: chart("judgment")?.querySelector(".jev-chart-figure")?.textContent ?? null,
          table: table("judgment"),
          role: picture("judgment")?.getAttribute("role") ?? null,
          label: picture("judgment")?.getAttribute("aria-label") ?? "",
        },
        names: Object.fromEntries(["summon", "placement", "routing"].map((id) => [id, jevSeatName(id)])),
      };
    });
    const tk = charts.tokens;
    ok("(e) the week's input tokens stand day by day over the table, with the strip's bill spread over them as an estimate",
      charts.order.join(",") === "tokens,judgment" && charts.beforeTable && tk.kind === "tokens"
        && tk.values === "11000,21000,31000,26000,11400,36500,31500" && tk.figure === "168,400"
        && tk.cost === "추정 비용 $0.328" && tk.table.length === 7
        && tk.table[0][1] === "11,000" && tk.table[0][2] === "$0.0449" && tk.table[6][1] === "31,500" && tk.table[6][2] === "$0.0483",
      JSON.stringify(tk));
    const jd = charts.judgment;
    ok("(c) the features waiting on a judgment stand nearest first, each with the samples it has and how many requests remain",
      jd.kind === "judgment" && jd.rows.join(",") === "placement,summon,routing"
        && jd.names.join("|") === ["placement", "summon", "routing"].map((id) => charts.names[id]).join("|")
        && jd.owed.join("|") === "다음 판정까지 5건|다음 판정까지 10건|판정까지 38건"
        && jd.bars === "25/25,34/34,35/73" && jd.figure === "3"
        && jd.table.map((row) => row.slice(1).join(" ")).join("|") === "25/25 5|34/34 10|35/73 38",
      JSON.stringify(jd));

    // (a), (b) and (d): the pictures in each row.
    const rows = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const numbers = (values) => (values ?? "").split(",").map((one) => (one === "" ? null : Number(Number(one).toFixed(3))));
      const of = (id) => {
        const row = view.querySelector(`[data-jev-dash-row="${id}"]`);
        const strip = row.querySelector('svg[data-jev-picture="timeline"]');
        const trend = row.querySelector('svg[data-jev-picture="trend"]');
        const latency = row.querySelector('svg[data-jev-picture="latency"]');
        return {
          states: strip?.dataset.states ?? null,
          now: strip?.querySelector("[data-status]")?.dataset.status ?? null,
          stripLabel: strip?.getAttribute("aria-label") ?? "",
          trend: trend ? Object.fromEntries([...trend.querySelectorAll("[data-line]")].map((line) => [line.dataset.line, numbers(line.dataset.values)])) : null,
          trendLabel: trend?.getAttribute("aria-label") ?? "",
          latency: latency ? { p50: numbers(latency.dataset.p50), p95: numbers(latency.dataset.p95) } : null,
          latencyLabel: latency?.getAttribute("aria-label") ?? "",
        };
      };
      return Object.fromEntries(["summon", "placement", "routing", "recall", "notify", "browser"].map((id) => [id, of(id)]));
    });
    ok("(a) each feature's week is a strip of days — asked or not, graded or not, beaten by the simplest method or not — ending where it stands now",
      rows.summon.states === "idle,measured,measured,measured,idle,measured,behind" && rows.summon.now === "applying"
        && rows.placement.states === "idle,idle,idle,idle,behind,behind,measured" && rows.placement.now === "recording"
        && rows.routing.states === "recorded,recorded,thin,recorded,recorded,recorded,recorded" && rows.routing.now === "applying"
        && rows.recall.states === Array(7).fill("recorded").join(",")
        && rows.notify.states === "idle,idle,idle,idle,idle,idle,recorded" && rows.notify.now === "blocked"
        && rows.summon.stripLabel.includes("단순 방식이 같거나 나음") && rows.summon.stripLabel.includes("적용 중"),
      JSON.stringify(Object.fromEntries(Object.entries(rows).map(([id, one]) => [id, { states: one.states, now: one.now }]))));
    ok("(b) the accuracy picture carries each graded day's share, its lower bound and the simplest method's share",
      JSON.stringify(rows.summon.trend) === JSON.stringify({
        band: [null, 0.41, 0.53, 0.31, null, 0.68, 0.53],
        baseline: [null, null, null, null, null, 0.75, 0.875],
        agreement: [null, 0.75, 0.875, 0.625, null, 1, 0.875],
      }) && JSON.stringify(rows.placement.trend?.baseline) === JSON.stringify([null, null, null, null, 0.714, 0.571, 0.571])
        && rows.routing.trend === null && rows.recall.trend === null
        && rows.summon.trendLabel.includes("75%") && rows.summon.trendLabel.includes("88%"),
      JSON.stringify({ summon: rows.summon.trend, placement: rows.placement.trend, label: rows.summon.trendLabel }));
    ok("(d) the latency picture carries each day's typical and slow answer, drawn from three days that answered",
      JSON.stringify(rows.summon.latency) === JSON.stringify({ p50: [null, 210, 220, 230, null, 250, 260], p95: [null, 420, 440, 460, null, 500, 520] })
        && JSON.stringify(rows.routing.latency?.p50) === JSON.stringify([200, 210, 220, 230, 240, 250, 260])
        && rows.browser.latency === null && rows.notify.latency === null
        && rows.summon.latencyLabel.includes("230 · 460 ms"),
      JSON.stringify({ summon: rows.summon.latency, routing: rows.routing.latency, label: rows.summon.latencyLabel }));

    // Every picture is named for a screen reader, with its numbers; a chart
    // over the table has a table of the same numbers behind it.
    const named = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const pictures = [...view.querySelectorAll("svg[data-jev-picture]")];
      const unnamed = pictures.filter((one) => one.getAttribute("role") !== "img" || !/\d/.test(one.getAttribute("aria-label") ?? ""))
        .map((one) => `${one.dataset.jevPicture}@${one.closest("[data-jev-dash-row]")?.dataset.jevDashRow ?? one.closest("[data-jev-chart]")?.dataset.jevChart}`);
      const tables = [...view.querySelectorAll("[data-jev-chart]")].map((chart) => ({
        chart: chart.dataset.jevChart,
        table: chart.querySelector("table.sr") !== null,
        caption: chart.querySelector("table.sr caption")?.textContent.trim() ?? "",
        heads: [...chart.querySelectorAll("table.sr thead th")].every((one) => one.scope === "col"),
      }));
      const kinds = [...new Set(pictures.map((one) => one.dataset.jevPicture))].sort();
      const hidden = pictures.filter((one) => one.getAttribute("aria-hidden") === "true").length;
      return { count: pictures.length, unnamed, tables, kinds, hidden };
    });
    ok("every picture is an image named with its numbers, and each chart over the table keeps a table of them",
      named.count > 10 && named.unnamed.length === 0 && named.hidden === 0
        && named.kinds.join(",") === "judgment,latency,timeline,tokens,trend"
        && named.tables.length === 2 && named.tables.every((one) => one.table && one.caption.length > 0 && one.heads),
      JSON.stringify(named));

    // The colours are the stylesheet's tokens, in both treatments: no mark
    // carries a colour of its own, and each mark wears its role's token.
    const colours = {};
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      colours[theme] = await page.evaluate(() => {
        const view = document.querySelector("#jev-view");
        const probe = document.createElement("span");
        view.append(probe);
        const token = (name) => {
          probe.style.color = `var(${name})`;
          return getComputedStyle(probe).color;
        };
        const worn = (selector, property) => {
          const mark = view.querySelector(selector);
          return mark ? getComputedStyle(mark)[property] : null;
        };
        const marks = [
          ['[data-jev-picture="trend"] [data-line="agreement"]', "stroke", "--jev-chart-accent"],
          ['[data-jev-picture="trend"] [data-line="band"]', "fill", "--jev-chart-wash"],
          ['[data-jev-picture="trend"] [data-line="baseline"]', "stroke", "--jev-chart-context"],
          ['[data-jev-picture="timeline"] [data-state="measured"]', "fill", "--jev-chart-accent"],
          ['[data-jev-picture="timeline"] [data-state="behind"]', "fill", "--jev-chart-behind"],
          ['[data-jev-picture="timeline"] [data-state="thin"]', "fill", "--jev-chart-thin"],
          ['[data-jev-picture="timeline"] [data-state="recorded"]', "fill", "--jev-chart-quiet"],
          ['[data-jev-picture="latency"] [data-line="p50"]', "stroke", "--jev-chart-accent"],
          ['[data-jev-picture="latency"] [data-line="p95"]', "stroke", "--jev-chart-range"],
          ['[data-jev-picture="tokens"] [data-values]', "stroke", "--jev-chart-accent"],
          ['[data-jev-picture="judgment"] [data-values]', "fill", "--jev-chart-accent"],
          ['[data-jev-picture="judgment"]', "backgroundColor", "--jev-chart-track"],
        ].map(([selector, property, name]) => ({ selector, worn: worn(selector, property), token: token(name) }));
        const accent = token("--jev-chart-accent");
        const behind = token("--jev-chart-behind");
        probe.remove();
        const inked = [...view.querySelectorAll("svg[data-jev-picture], svg[data-jev-picture] *")]
          .filter((one) => ["fill", "stroke", "style", "color", "stop-color"].some((name) => one.hasAttribute(name)))
          .map((one) => one.outerHTML.slice(0, 80));
        return { marks, inked, accent, behind };
      });
    }
    await setQualityTheme(page, "dark");
    const astray = Object.entries(colours).flatMap(([theme, seen]) =>
      seen.marks.filter((one) => one.worn === null || one.worn !== one.token).map((one) => ({ theme, ...one })));
    ok("every mark wears its role's token in both treatments, and no picture carries a colour of its own",
      astray.length === 0 && colours.dark.inked.length === 0 && colours.light.inked.length === 0
        && colours.dark.accent !== colours.light.accent && colours.dark.behind !== colours.light.behind,
      JSON.stringify({ astray: astray.slice(0, 6), inked: [...colours.dark.inked, ...colours.light.inked].slice(0, 4),
        accent: [colours.dark.accent, colours.light.accent], behind: [colours.dark.behind, colours.light.behind] }));

    // Every chart colour is declared twice: in the dark treatment the window
    // starts in, and in the light one's block (`:root[data-theme="light"]`).
    const declared = await page.evaluate(async () => {
      const css = await (await fetch("/tokens.css")).text();
      const plain = css.replace(/\/\*[\s\S]*?\*\//g, "");
      const blocks = [];
      const opener = /(:root(?:\[data-theme="light"\])?)\s*\{/g;
      for (let found = opener.exec(plain); found; found = opener.exec(plain)) {
        let depth = 1;
        let at = opener.lastIndex;
        while (depth > 0 && at < plain.length) {
          if (plain[at] === "{") depth += 1;
          else if (plain[at] === "}") depth -= 1;
          at += 1;
        }
        blocks.push({ light: found[1].includes("light"), body: plain.slice(opener.lastIndex, at - 1) });
      }
      const names = (light) => [...new Set(blocks.filter((one) => one.light === light)
        .flatMap((one) => [...one.body.matchAll(/(--jev-chart-[a-z-]+)\s*:/g)].map((match) => match[1])))].sort();
      return { dark: names(false), light: names(true) };
    });
    ok("every chart colour token is declared in the dark treatment and again in the light one",
      declared.dark.length >= 8 && declared.dark.join(",") === declared.light.join(","),
      JSON.stringify(declared));

    // Drawn again only where the numbers moved (t-6323's lesson): the same
    // numbers painted again touch nothing; a fresh answer in which one
    // feature's week moved draws that one row again and no chart; one in
    // which a day's input tokens moved draws that row and the tokens chart.
    const redrawn = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const watch = async (change) => {
        const rows = new Set();
        const charts = new Set();
        let drawer = false;
        const observer = new MutationObserver((records) => {
          for (const record of records) {
            const target = record.target.nodeType === 1 ? record.target : record.target.parentElement;
            const row = target?.closest?.("[data-jev-dash-row]");
            if (row) rows.add(row.dataset.jevDashRow);
            const chart = target?.closest?.("[data-jev-chart]");
            if (chart) charts.add(chart.dataset.jevChart);
            if (target?.closest?.(".jev-drawer")) drawer = true;
          }
        });
        observer.observe(view, { subtree: true, childList: true, characterData: true, attributes: true });
        change?.();
        paintJevViews();
        await Promise.resolve();
        observer.disconnect();
        return { rows: [...rows].sort(), charts: [...charts].sort(), drawer };
      };
      const fresh = (edit) => {
        jevNumbers = JSON.parse(JSON.stringify(jevNumbers)).map((seat) => (edit(seat) ?? seat));
      };
      const same = await watch(null);
      const answer = await watch(() => fresh(() => null));
      const one = await watch(() => fresh((seat) => (seat.id === "recall" ? { ...seat, week: { ...seat.week, rows: seat.week.rows + 1 } } : null)));
      const tokens = await watch(() => fresh((seat) => (seat.id === "routing"
        ? { ...seat, days: seat.days.map((day, at) => (at === 6 ? { ...day, tally: { ...day.tally, inputTokens: day.tally.inputTokens + 500 } } : day)) }
        : null)));
      // The reasons a feature's week compared nothing (t-9935): one more
      // row that nobody looked at draws that feature's row again — which
      // says so — and nothing else; the same reasons again draw nothing.
      const saved = jevNumbers;
      const why = view.querySelector.bind(view, '[data-jev-dash-row="placement"] [data-jev-why]');
      await watch(() => fresh((seat) => (seat.id === "placement" ? { ...seat, verdict: { verdict: "hold", line: "too_few_compared" } } : null)));
      const unseen = (seat) => ({ ...seat, agreementWeek: { ...seat.agreementWeek,
        notComparedBy: { ...seat.agreementWeek.notComparedBy, unseen: seat.agreementWeek.notComparedBy.unseen + 1 } } });
      const reason = { ...(await watch(() => fresh((seat) => (seat.id === "placement" ? unseen(seat) : null)))), said: why()?.textContent ?? null };
      const reasonAgain = await watch(() => fresh(() => null));
      // Today's reasons are the drawer's alone: one more today draws the open
      // drawer again, which says so, and no row.
      jevNumbers = saved;
      paintJevViews();
      view.querySelector('[data-jev-dash-row="placement"] .jev-row-open').click();
      await window.__PAINTED__();
      const today = await watch(() => fresh((seat) => (seat.id === "placement" ? { ...seat, days: seat.days.map((day, at) => (at !== 6 ? day
        : { ...day, agreement: { ...day.agreement, notComparedBy: { ...day.agreement.notComparedBy, unseen: day.agreement.notComparedBy.unseen + 1 } } })) } : null)));
      today.said = view.querySelector('.jev-drawer [data-jev-reason="unseen"] .jev-judgment-owed')?.textContent ?? null;
      view.querySelector(".jev-drawer-close").click();
      jevNumbers = saved;
      paintJevViews();
      await window.__PAINTED__();
      return { same, answer, one, tokens, reason, reasonAgain, today };
    });
    ok("a paint draws again only the rows and charts whose numbers moved",
      redrawn.same.rows.length === 0 && redrawn.same.charts.length === 0
        && redrawn.answer.rows.length === 0 && redrawn.answer.charts.length === 0
        && redrawn.one.rows.join(",") === "recall" && redrawn.one.charts.length === 0
        && redrawn.tokens.rows.join(",") === "routing" && redrawn.tokens.charts.join(",") === "tokens",
      JSON.stringify(redrawn));
    ok("a reason that moved draws its feature's row again, which says so, and the same reasons draw nothing; today's move only the open drawer",
      redrawn.reason.rows.join(",") === "placement" && redrawn.reason.charts.length === 0
        && redrawn.reason.said === "아무도 안 봄 96건, 실행 안 됨 3건"
        && redrawn.reasonAgain.rows.length === 0 && redrawn.reasonAgain.charts.length === 0 && !redrawn.reasonAgain.drawer
        && redrawn.today.rows.length === 0 && redrawn.today.charts.length === 0 && redrawn.today.drawer
        && redrawn.today.said === "95건 (97%) · 오늘 5건",
      JSON.stringify({ reason: redrawn.reason, again: redrawn.reasonAgain, today: redrawn.today }));

    // A feature whose record has too few misses to trust (one_sided) waits
    // for them: the chart says how many it needs as well as when it is next
    // judged.
    const oneSided = await page.evaluate(async () => {
      const saved = jevNumbers;
      jevNumbers = saved.map((seat) => (seat.id === "placement"
        ? { ...seat, verdict: { verdict: "hold", line: "one_sided" }, negativesWanted: 3 } : seat));
      paintJevViews();
      await window.__PAINTED__();
      const owed = document.querySelector('#jev-view [data-jev-chart="judgment"] .jev-judgment-owed')?.textContent.trim() ?? null;
      jevNumbers = saved;
      paintJevViews();
      return owed;
    });
    ok("a feature waiting for misses to trust its accuracy says how many it needs and when it is next judged",
      oneSided === "다음 판정까지 5건 · 틀린 결과 3건 필요", JSON.stringify(oneSided));

    // Nothing asked all week: each chart says so in its frame.
    const quiet = await page.evaluate(async () => {
      const saved = jevNumbers;
      jevNumbers = saved.map((seat) => ({ ...seat, today: { ...seat.today, rows: 0 }, week: { ...seat.week, rows: 0 },
        days: seat.days.map((day) => ({ ...day, tally: { ...day.tally, rows: 0, inputTokens: 0 } })) }));
      paintJevViews();
      await window.__PAINTED__();
      const said = [...document.querySelectorAll("#jev-view [data-jev-chart]")].map((chart) => ({
        chart: chart.dataset.jevChart, said: chart.querySelector(".jev-chart-empty:not([hidden])")?.textContent.trim() ?? null,
        pictures: chart.querySelectorAll("svg[data-jev-picture]").length,
      }));
      jevNumbers = saved;
      paintJevViews();
      return said;
    });
    ok("a week nothing asked leaves each chart's frame standing with a line saying why it is empty",
      quiet.length === 2 && quiet.every((one) => one.pictures === 0 && one.said)
        && quiet.find((one) => one.chart === "tokens")?.said === "입력 토큰을 쓴 판단이 없습니다"
        && quiet.find((one) => one.chart === "judgment")?.said === "판정을 기다리는 기능이 없습니다",
      JSON.stringify(quiet));

    // A feature's drawer carries its week large: the same picture with the
    // days' comparisons and the sample floor under it, and a table of the
    // days.
    const drawer = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      const part = view.querySelector('.jev-drawer [data-jev-drawer-part="days"]');
      const seen = {
        picture: part?.querySelector('svg[data-jev-picture="trend"]') !== null,
        samples: part?.querySelector('[data-line="samples"]')?.dataset.values ?? null,
        floor: part?.querySelector('[data-line="floor"]')?.dataset.values ?? null,
        rows: [...(part?.querySelectorAll("tbody tr") ?? [])].map((row) => [...row.children].map((cell) => cell.textContent.trim())),
        heads: [...(part?.querySelectorAll("thead th") ?? [])].map((one) => one.textContent.trim()),
      };
      view.querySelector(".jev-drawer-close").click();
      await window.__PAINTED__();
      return seen;
    });
    ok("a feature's drawer draws its week large, with each day's comparisons over the sample floor, and a table of the days",
      drawer.picture && drawer.samples === ",8,8,8,,8,8" && drawer.floor === "5"
        && drawer.rows.length === 7 && drawer.rows[1].join("|").includes("6/8") && drawer.rows[6].join("|").includes("7/8")
        && drawer.heads.length >= 5,
      JSON.stringify(drawer));

    // Every language names the pictures in its own words.
    const spoken = await page.evaluate(async () => {
      const said = {};
      for (const language of ["en", "ja", "zh", "es"]) {
        setLocale(language);
        await window.__PAINTED__();
        const view = document.querySelector("#jev-view");
        // What stands on the page: a closed drawer is said again when it opens.
        const shown = (selector) => [...view.querySelectorAll(selector)].filter((one) => !one.closest("[hidden]"));
        const words = [
          ...shown("[data-jev-charts], .jev-trends, [data-jev-cell=\"latency\"], thead").map((one) => one.textContent),
          ...shown("svg[data-jev-picture], [data-jev-charts] [data-tip]").map((one) => `${one.getAttribute("aria-label") ?? ""} ${one.dataset.tip ?? ""}`),
        ].join(" ");
        said[language] = { korean: (words.match(/[가-힣]+/g) ?? []).slice(0, 6), pictures: view.querySelectorAll("svg[data-jev-picture]").length };
      }
      setLocale("ko");
      await window.__PAINTED__();
      return said;
    });
    ok("the pictures, the charts and their tips speak every language, with no Korean left behind",
      Object.values(spoken).every((one) => one.korean.length === 0 && one.pictures > 10), JSON.stringify(spoken));

    // A 1080p stage with both side panels open holds the whole table — rows,
    // not cards, with every row's latency picture — in every language: what
    // the table needs is a length of words, and the widths the stylesheet
    // switches at were measured on the longest of them (t-9633). The pictures
    // come with the width alone, before any language moves a row.
    const latencies = () => page.evaluate(() => ({
      pictures: document.querySelectorAll('#jev-view [data-jev-dash-row] svg[data-jev-picture="latency"]').length,
      rows: getComputedStyle(document.querySelector("#jev-view .jev-table thead")).display !== "none",
    }));
    await page.setViewportSize({ width: 1800, height: 1080 });
    await settlePaint(page);
    const tight = await latencies();
    await page.setViewportSize({ width: 1920, height: 1080 });
    await settlePaint(page);
    const widened = await latencies();
    ok("a table with no room for the latency pictures draws none, and widening it draws them without another answer",
      tight.rows && tight.pictures === 0 && widened.rows && widened.pictures === 4, JSON.stringify({ tight, widened }));
    const whole = await page.evaluate(async () => {
      const said = {};
      for (const language of ["ko", "en", "ja", "zh", "es"]) {
        setLocale(language);
        await window.__PAINTED__();
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const view = document.querySelector("#jev-view");
        const wrap = view.querySelector(".jev-table-wrap");
        said[language] = {
          fits: wrap.scrollWidth <= wrap.clientWidth + 1,
          rows: getComputedStyle(view.querySelector(".jev-table thead")).display !== "none",
          latency: view.querySelectorAll('[data-jev-dash-row] svg[data-jev-picture="latency"]').length,
          faults: window.__JEV_TEXT_FAULTS__(view.querySelector(".jev-table-wrap")).length,
        };
      }
      setLocale("ko");
      await window.__PAINTED__();
      return said;
    });
    ok("a 1080p stage with both side panels holds the whole table, latency pictures and all, in every language",
      Object.values(whole).every((one) => one.fits && one.rows && one.latency === 4 && one.faults === 0),
      JSON.stringify(whole));

    // A narrow window: the charts one under the other and every row a card,
    // with every word whole and nothing wider than the page — the window as
    // tall as the page, so what is measured is width and not the fold.
    await standNarrow(page);
    const narrow = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      const wide = [view, ...view.querySelectorAll(".jev-table-wrap, [data-jev-chart], [data-jev-dash-row]")]
        .filter((one) => one.scrollWidth > one.clientWidth + 1).map((one) => one.className || one.tagName);
      const charts = [...view.querySelectorAll("[data-jev-chart]")].map((one) => one.getBoundingClientRect());
      const row = view.querySelector('[data-jev-dash-row="summon"]');
      const cells = [...row.querySelectorAll("td")].map((cell) => ({ cell: cell.dataset.jevCell, label: getComputedStyle(cell, "::before").content,
        width: Math.round(cell.getBoundingClientRect().width) }));
      return {
        width: Math.round(view.getBoundingClientRect().width),
        faults: window.__JEV_TEXT_FAULTS__(view),
        wide,
        stacked: charts.length === 2 && charts[1].top >= charts[0].bottom - 1,
        headHidden: view.querySelector(".jev-table thead").getBoundingClientRect().height <= 1,
        labelled: cells.filter((one) => one.cell !== "seat").every((one) => one.label && one.label !== "none" && one.label !== "normal"),
        cells,
      };
    });
    ok("in a narrow window the charts stand one under the other and every row is a labelled card, every word whole",
      narrow.width < 600 && narrow.faults.length === 0 && narrow.wide.length === 0 && narrow.stacked && narrow.headHidden && narrow.labelled,
      JSON.stringify({ ...narrow, faults: narrow.faults.slice(0, 6) }));

    ok("the pictures raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

/* t-9091: the dashboard says where it counts. On 2026-09-25 it was opened
 * while a worker's checkout was the one being looked at, and every zo
 * feature read zero — zo counts the project it is asked about, and that
 * checkout had no zo records of its own — while another project had
 * thirteen features' records that day; nothing on the page said which
 * project it had counted. The reading now names its scope: the checkout by
 * name, and — when it has no zo records — the projects that do, with a
 * switch to every project summed, where a feature says in how many projects
 * it acts. A feature whose records this computer keeps in one place is
 * marked, and reads the same numbers in both scopes. */
export async function testJevDashboardScope(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(jevDashboardFixture);
    const seen = await page.evaluate(async () => {
      const base = window.__ANSWER__.jev_summary;
      const workspace = "/Users/p/zerocode/workspaces/zerocode/t-1";
      const projects = [
        { path: "/Users/p/atlas", name: "atlas", newestMs: Date.now() - 3_600_000 },
        { path: "/Users/p/notes", name: "notes", newestMs: Date.now() - 86_400_000 },
      ];
      const nothing = (counted) => ({ ...counted, rows: 0, answered: 0, refused: 0, applied: 0, called: 0, requests: 0,
        answeredShare: null, answeredLowerBound: null, p50Ms: null, p95Ms: null, failures: [], refusals: [] });
      /* Two projects' rows added the way the window's sum adds them
       * (`jev_scope::SEAT`): the counts and the bill twice — the reasons
       * rows compared nothing word by word too — and what each project
       * judged of its own rows — its verdict, its judged window, its latency
       * percentiles, its confidence bar and what that bar applies — not
       * carried. */
      const twice = (window) => ({ ...window, rows: window.rows * 2, answered: window.answered * 2, refused: window.refused * 2,
        applied: window.applied * 2, called: window.called * 2, requests: window.requests * 2, inputTokens: window.inputTokens * 2,
        p50Ms: null, p95Ms: null, failures: window.failures.map((one) => ({ ...one, rows: one.rows * 2 })),
        refusals: window.refusals.map((one) => ({ ...one, rows: one.rows * 2 })) });
      const twiceMarks = (marks) => (!marks ? marks : { ...marks, compared: marks.compared * 2, agreed: marks.agreed * 2,
        notCompared: (marks.notCompared ?? 0) * 2,
        notComparedBy: Object.fromEntries(Object.entries(marks.notComparedBy ?? {}).map(([word, count]) => [word, count * 2])) });
      const summed = (seat) => ({
        ...seat, today: twice(seat.today), week: twice(seat.week), costUsd: seat.costUsd === null ? null : seat.costUsd * 2,
        verdict: null, judged: null, rowsToNextJudgment: null, clearsRiseFloor: null, negativesWanted: null,
        calibration: null, applyShare: null, appliedErrorPermille: null, baselineErrorPermille: null,
        agreementWeek: twiceMarks(seat.agreementWeek),
        days: seat.days.map((one) => ({ ...one, tally: twice(one.tally), agreement: twiceMarks(one.agreement) })),
      });
      window.__ANSWER__.jev_summary = (args) => {
        const answered = base(args);
        const scope = { ...answered.scope, workspace, workspaceName: "t-1", recorded: false, projects };
        const seats = answered.seats.map((seat) => {
          if (args.scope === "projects") {
            const across = { projects: projects.length, applying: seat.applies ? 1 : 0, summed: seat.reach === "project" };
            return across.summed ? { ...summed(seat), across } : { ...seat, across };
          }
          // This checkout has no zo records of its own: a project's rows are not here.
          return seat.reach !== "project" ? seat : {
            ...seat, reach: null, today: nothing(seat.today), week: nothing(seat.week), costUsd: null,
            verdict: null, judged: null, rowsToNextJudgment: null, clearsRiseFloor: null, days: [], recent: [],
          };
        });
        return { scope, seats };
      };
      const view = () => document.querySelector("#jev-view");
      const until = (drawn) => new Promise((done, fail) => {
        const end = performance.now() + 3_000;
        const look = () => {
          if (drawn()) done();
          else if (performance.now() > end) fail(new Error("the dashboard never drew it"));
          else requestAnimationFrame(look);
        };
        look();
      });
      const fact = (id, name) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-fact="${name}"]`)?.textContent ?? null;
      const reachOf = (id) => view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-reach]`)?.dataset.jevReach ?? null;
      const read = () => {
        const line = view().querySelector("[data-jev-scope-line]");
        const note = view().querySelector("[data-jev-scope-note]");
        return {
          line: line?.textContent.trim() ?? null,
          lineTip: line?.dataset.tip ?? null,
          note: note && !note.hidden ? note.textContent.trim() : null,
          choices: [...view().querySelectorAll("[data-jev-scope-choice]")]
            .map((one) => `${one.dataset.jevScopeChoice}:${one.getAttribute("aria-pressed")}`),
          summon: { week: fact("summon", "week"), reach: reachOf("summon") },
          routing: {
            week: fact("routing", "week"), reach: reachOf("routing"),
            across: view().querySelector('[data-jev-dash-row="routing"] [data-jev-across]')?.textContent.trim() ?? null,
          },
          // The charts say what the reading in hand says (t-9633): the days'
          // input tokens all counted, the estimate the strip's bill spread
          // over its days, no judgment a sum cannot have, and no latency
          // picture where a sum carries no percentile.
          charts: (() => {
            const tokens = view().querySelector('[data-jev-chart="tokens"]');
            const values = tokens?.querySelector("svg [data-values]")?.dataset.values ?? "";
            return {
              tokens: values.split(",").filter(Boolean).reduce((total, one) => total + Number(one), 0),
              expected: (jevNumbers ?? []).reduce((total, seat) =>
                total + (seat.days ?? []).reduce((all, day) => all + (day.tally.inputTokens ?? 0), 0), 0),
              cost: tokens?.querySelector(".jev-chart-sub")?.textContent ?? null,
              strip: view().querySelector('[data-jev-stat="cost"] dd')?.textContent ?? null,
              judged: [...view().querySelectorAll('[data-jev-chart="judgment"] [data-jev-judgment]')].map((one) => one.dataset.jevJudgment),
              routingLatency: view().querySelector('[data-jev-dash-row="routing"] svg[data-jev-picture="latency"]') !== null,
              routingP50: fact("routing", "p50"),
            };
          })(),
          ask: window.__JEV__.asks.at(-1) ?? null,
        };
      };
      el("nav-jev").click();
      await until(() => fact("summon", "week") === "50");
      const here = read();
      const asked = window.__JEV__.asks.length;
      const pressed = Boolean(view().querySelector('[data-jev-scope-choice="projects"]'));
      view().querySelector('[data-jev-scope-choice="projects"]')?.click();
      // Whether the other scope was asked for and drawn is itself a finding:
      // a dashboard with no switch never asks, and each claim below says so.
      const drawnAgain = await until(() => window.__JEV__.asks.length > asked && fact("routing", "week") === "70")
        .then(() => true, () => false);
      const everywhere = { ...read(), pressed, drawnAgain };
      // A summed feature's drawer (t-9935): its check and its confidence bar
      // are each project's own, so each is a dash that says so; why its rows
      // compared nothing is every project's, added.
      view().querySelector('[data-jev-dash-row="recall"] .jev-row-open')?.click();
      await window.__PAINTED__();
      const part = view().querySelector('.jev-drawer [data-jev-drawer-part="why"]');
      everywhere.drawer = !part ? null : {
        facts: [...part.querySelectorAll("dt")].map((term) => `${term.textContent}=${term.nextElementSibling?.textContent ?? ""}`),
        tips: [...part.querySelectorAll("dd")].map((one) => one.dataset.tip ?? ""),
        reasons: [...part.querySelectorAll("[data-jev-reason]")].map((item) => `${item.dataset.jevReason}|${item.querySelector(".jev-judgment-owed")?.textContent ?? ""}`),
      };
      view().querySelector(".jev-drawer-close")?.click();
      await window.__PAINTED__();
      let kept = null;
      try {
        kept = localStorage.getItem(JEV_SCOPE_KEY);
      } catch {
        kept = null;
      }
      return { here, everywhere, kept, workspace };
    }).catch((error) => ({ error: String(error) }));
    const { here, everywhere } = seen;
    ok("the dashboard names the checkout it counts, with its whole path as the tip",
      !seen.error && here.line?.includes("t-1") && here.lineTip?.includes(seen.workspace) && here.ask?.scope === "workspace",
      JSON.stringify(seen.error ?? { line: here.line, tip: here.lineTip, ask: here.ask }));
    ok("a checkout with no zo records says so and names the projects that have them",
      !seen.error && here.note !== null && here.note.includes("atlas") && here.note.includes("notes") && here.note.includes("2"),
      JSON.stringify(seen.error ?? here.note));
    ok("the scope is one choice of two, this checkout first and chosen",
      !seen.error && here.choices.join(",") === "workspace:true,projects:false",
      JSON.stringify(seen.error ?? here.choices));
    ok("every project, asked, is summed: the table says in how many projects a feature acts",
      !seen.error && everywhere.ask?.scope === "projects" && everywhere.line?.includes("2") && everywhere.note === null
        && everywhere.choices.join(",") === "workspace:false,projects:true"
        && everywhere.routing.week === "70" && Boolean(everywhere.routing.across?.includes("2") && everywhere.routing.across?.includes("1")),
      JSON.stringify(seen.error ?? everywhere));
    ok("the charts say what each scope counted: every day's input tokens, the strip's bill, no judgment or latency picture a sum cannot have",
      !seen.error && here.charts.tokens === here.charts.expected && here.charts.tokens > 0
        && everywhere.charts.tokens === everywhere.charts.expected && everywhere.charts.tokens > here.charts.tokens
        && Boolean(here.charts.cost?.includes(here.charts.strip)) && Boolean(everywhere.charts.cost?.includes(everywhere.charts.strip))
        && here.charts.strip !== everywhere.charts.strip
        && everywhere.charts.judged.join(",") === "placement,summon" && !everywhere.charts.routingLatency
        && everywhere.charts.routingP50 === "합산 안 함",
      JSON.stringify(seen.error ?? { here: here.charts, everywhere: everywhere.charts }));
    ok("a feature this computer keeps in one place is marked, reads the same in both scopes, and is the only one marked",
      !seen.error && here.summon.reach === "machine" && everywhere.summon.reach === "machine"
        && here.summon.week === "50" && everywhere.summon.week === "50" && everywhere.routing.reach === null,
      JSON.stringify(seen.error ?? { here: here.summon, everywhere: everywhere.summon, routing: everywhere.routing.reach }));
    ok("a summed feature's drawer says its check and its bar are each project's own, and adds up why its rows compared nothing",
      !seen.error && everywhere.drawer !== null && everywhere.drawer.facts.join("|") === "판정 결과=—|확신도 기준=—"
        && everywhere.drawer.tips.length === 2 && everywhere.drawer.tips.every((tip) => tip.includes("프로젝트"))
        && everywhere.drawer.reasons.join(",") === "no_note_touched|68건 (100%) · 오늘 10건",
      JSON.stringify(seen.error ?? everywhere.drawer));
    ok("the scope chosen is kept for the next open", !seen.error && seen.kept === "projects", JSON.stringify(seen.error ?? seen.kept));
    ok("the scope's dashboard raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
