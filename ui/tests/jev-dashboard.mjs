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
/* How many seats the settings card carries — counted off the card's own
 * markup rather than typed here, so a seat added to the use table (which
 * `typesafe_settings.rs` holds the card to) is not a second number to move. */
const { readFileSync } = await import("node:fs");
const SEATS = (readFileSync(join(ROOT, "ui", "index.html"), "utf8").match(/data-jev-seat="/g) ?? []).length;
/* The fewest days with a value a trend is drawn from (t-6243 D4). */
const TREND_DAYS = 3;

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
  modeOf.placement = "auto";
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
      // The id asked and the version that answered, and the version a change
      // of version cut away (t-6187) — what the why column and the card say.
      seat.askedModel = "jev-latest";
      seat.model = "jev-1.13.0";
      seat.verdict = { verdict: "hold", line: "answered", cutModel: "jev-1.12.0" };
      seat.days = days([null, 0.9, 1, 0.8, null, 1, 0.96]);
      seat.recent = args?.recent ? decisions(Math.min(args.recent, 5)) : [];
    }
    if (id === "recall") {
      // A seat the table never promotes: no floor, no verdict, and a
      // person's `shadow` — it records, and the why column says so.
      seat.found = "/h/state/smart-router/rerank-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 211, p95Ms: 232 });
      seat.week = counted(1005, 935, { applied: 845, p50Ms: 211, p95Ms: 400,
        failures: [{ token: "schema", rows: 65 }, { token: "timeout", rows: 5 }] });
      seat.costUsd = 0.31;
      seat.days = days([1, 0.95, 0.9, 1, 0.92, 0.9, 0.93]);
    }
    if (id === "placement") {
      // A seat under its bar: the window is full and the judge holds it on
      // agreement (t-6243 D1's 기준 미달).
      seat.today = counted(4, 4, { p50Ms: 606, p95Ms: 768 });
      seat.week = counted(60, 59, { p50Ms: 606, p95Ms: 768 });
      seat.costUsd = 0.002;
      seat.riseFloorPermille = 800;
      seat.clearsRiseFloor = true;
      seat.rowsToNextJudgment = 5;
      seat.judged = { window: counted(25, 25), windowWanted: 25, agreement: { compared: 21, agreed: 12, lowerBound: 0.37, controlRows: 0 } };
      seat.verdict = { verdict: "hold", line: "agreement" };
      seat.days = days([null, null, null, null, 1, 0.9, 1]);
    }
    if (id === "browser") {
      // A small sample: four answers in the week, too few for a lower bound
      // to be a measurement (t-6243 D3).
      seat.week = counted(4, 4, { p50Ms: 243, p95Ms: 435 });
      seat.days = days([null, null, null, null, null, null, 1]);
    }
    if (id === "notify") {
      // A seat every request of which the door refused for consent: nothing
      // it asked was answered (t-6243 D1's 키·동의 필요).
      seat.today = counted(2, 0, { refused: 2, refusals: [{ token: "not_consented", rows: 2 }], failures: [{ token: "not_consented", rows: 2 }] });
      seat.week = counted(2, 0, { refused: 2, refusals: [{ token: "not_consented", rows: 2 }], failures: [{ token: "not_consented", rows: 2 }] });
      seat.costUsd = 0;
      seat.days = days([null, null, null, null, null, null, 0]);
    }
    if (id === "routing") {
      seat.found = "/h/state/smart-router/decision-shadow.jsonl";
      seat.today = counted(3, 3, { p50Ms: 541, p95Ms: 600 });
      // Three requests the door refused for want of a key, one the wire timed
      // out on (t-6243 D5).
      seat.week = counted(35, 31, { refused: 3, refusals: [{ token: "no_key", rows: 3 }],
        failures: [{ token: "no_key", rows: 3 }, { token: "timeout", rows: 1 }], p50Ms: 500, p95Ms: 884, applied: 23 });
      seat.costUsd = 0.004;
      seat.riseFloorPermille = 950;
      seat.clearsRiseFloor = false;
      // The core's countdown while the window fills is what the window
      // still wants (`promote::rows_to_next_judgment`): 73 − 35.
      seat.rowsToNextJudgment = 38;
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
    // Every seat's comparison sample floor, the use table's
    // (`JevUse::agreement_rows_wanted`, a judgment window's worth).
    switches: seats.map((id) => ({ id, setting: id, mode: modeOf[id], modes, agreementRowsWanted: 20 })),
    classifier: { setting: "autoClassifier", mode: "probed", probes: true, gates: seats[0],
      modes: [{ mode: "probed", runs: true, markers: false, probes: true }] },
  });
  window.__ANSWER__.typesafe_settings = settings;
  // The day's requests from this machine against the person's limit
  // (`jev_day`, t-6243 D1).
  window.__ANSWER__.jev_day = () => ({ sent: 247, most: 500 });
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
    await page.setViewportSize({ width: Math.min(2600, Math.ceil(wanted.width) + 16), height: Math.min(4000, Math.ceil(wanted.height) + 16) });
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
          // One picture per feature with three days of values (t-6243 D4).
          pictures: view.querySelectorAll(".jev-spark").length,
          drawable: [...view.querySelectorAll("[data-jev-dash-row]")].filter((row) =>
            ((jevNumbers ?? []).find((one) => one.id === row.dataset.jevDashRow)?.days ?? [])
              .filter((day) => day.tally.rows > 0).length >= trendDays).length,
          theme: document.documentElement.dataset.theme ?? "dark",
          viewport: [window.innerWidth, window.innerHeight],
        };
      }, TREND_DAYS);
      const path = join(EVIDENCE_DIR, `jev-dashboard-${theme}.png`);
      await page.screenshot({ path });
      shots[theme] = { ...seen, path };
      ok(`the ${theme} dashboard is one readable screen: no text cut, clipped or overlapping`,
        seen.faults.length === 0 && seen.oneScreen && seen.tableFits && seen.rows === SEATS - seen.unused
          && seen.fold === (seen.unused > 0) && seen.pictures === seen.drawable && seen.theme === theme,
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
        card.faults.length === 0 && card.lines === counted, JSON.stringify({ ...card, faults: card.faults.slice(0, 6), path }));
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
        applied: fact("summon", "applied"),
        agreement: fact("summon", "agreement"), cost: cell("summon", "cost").textContent,
        boundTip: view().querySelector('[data-jev-dash-row="summon"] [data-jev-fact="bound"]')?.dataset.tip ?? null,
        mode: cell("summon", "mode").querySelector("select").value,
      };
      const recallWeek = fact("recall", "week");
      // Each feature's state (t-6243 D2): its chip and tone, one reason, the
      // samples still owed while it wants some, and any fact beside them.
      const statusOf = (id) => {
        const holder = cell(id, "status");
        const chip = holder.querySelector(".jev-chip");
        const bar = holder.querySelector(".jev-progress");
        const words = holder.textContent.trim().split(/\s+/).filter((word) => word && !/^[·—–\-/|:]+$/.test(word));
        return {
          chip: chip?.textContent ?? null, tone: chip?.dataset.status ?? null,
          reason: holder.querySelector(".jev-status-reason")?.textContent ?? "",
          progress: bar ? { now: bar.getAttribute("aria-valuenow"), max: bar.getAttribute("aria-valuemax"),
            label: holder.querySelector(".jev-progress-label")?.textContent ?? "" } : null,
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
      const bodyRows = [...view().querySelectorAll("tbody tr")];
      const fold = foldRow ? {
        text: foldRow.textContent, expanded: foldRow.querySelector("button").getAttribute("aria-expanded"),
        at: bodyRows.indexOf(foldRow),
      } : null;
      // The week's picture (t-6243 D4): how many, which lines with how many
      // points, and what the cell says when there is none.
      const trendOf = (id) => {
        const holder = cell(id, "trend");
        return {
          pictures: holder.querySelectorAll("svg").length,
          lines: [...holder.querySelectorAll("svg [data-line]")].map((line) => `${line.dataset.line}:${line.dataset.points}`),
          text: holder.textContent.trim(),
        };
      };
      const trends = Object.fromEntries(["summon", "routing", "notify", "browser"].map((id) => [id, trendOf(id)]));
      // What the rate and accuracy cells say (t-6243 D3).
      const said = (id) => ({ answered: cell(id, "answered").textContent, agreement: fact(id, "agreement") });
      const small = Object.fromEntries(["summon", "placement", "routing", "notify", "browser"].map((id) => [id, said(id)]));
      const cardLine = document.querySelector('[data-jev-seat="summon"]').closest("[data-jev-row]")
        .querySelector("[data-jev-numbers]")?.textContent ?? null;
      return {
        ms, active: activeTabId, visible: !view().hidden, hiddenAttr: view().hidden,
        rowIds: rows.map((row) => row.dataset.jevDashRow),
        rowNames: rows.map((row) => row.querySelector('[data-jev-cell="seat"]').textContent),
        cardNames, cardSeats: window.__JEV__.seats, summon, recallWeek, statuses, caption, cardLine, small, trends,
        strip, fold, bodyRows: bodyRows.length,
        asks: window.__JEV__.asks.slice(), settingsAsks: window.__COUNTS__.typesafe_settings ?? 0,
        summaryAsks: window.__COUNTS__.jev_summary ?? 0, dayAsks: window.__COUNTS__.jev_day ?? 0,
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
        && opened.strip.applying === "3" && opened.strip.recording === "1" && opened.strip.under === "1"
        && opened.strip.blocked === "1" && opened.strip.day === "247 / 500",
      JSON.stringify(opened.strip));
    ok("opening costs one settings ask, one summary ask with the recent list, and one read of the day's count",
      opened.settingsAsks === 1 && opened.summaryAsks === 1 && opened.dayAsks === 1
        && opened.asks.length === 1 && opened.asks[0].recent === opened.recentRows,
      JSON.stringify({ settings: opened.settingsAsks, summary: opened.summaryAsks, day: opened.dayAsks, asks: opened.asks }));

    // A 1080p screen holds every feature in use, one after another, before
    // any scroll (the plan's D1 measure).
    await page.setViewportSize({ width: 1920, height: 1080 });
    const firstScreen = await page.evaluate(async () => {
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      view.scrollTop = 0;
      const box = view.getBoundingClientRect();
      const bottom = Math.min(box.bottom, window.innerHeight);
      const rows = [...view.querySelectorAll("[data-jev-dash-row]")];
      return {
        rows: rows.length,
        inside: rows.filter((row) => { const rect = row.getBoundingClientRect(); return rect.top >= box.top - 1 && rect.bottom <= bottom + 1; }).length,
        // Room under the first row, and the tallest row: nine rows of that
        // height must fit — this machine's features in use on 2026-09-23.
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
    ok("a 1080p screen holds every feature in use before any scroll, and room for nine",
      firstScreen.rows === 6 && firstScreen.inside === 6 && firstScreen.tallest * 9 <= firstScreen.room,
      JSON.stringify(firstScreen));
    ok("from the click to the drawn table is under the design's 200 ms with the backend answering at once",
      opened.ms < 200, `${opened.ms.toFixed(1)} ms`);
    ok("a counted seat's numbers stand in its cells",
      opened.summon.today === "32" && opened.summon.week === "50" && opened.summon.answered.startsWith("96%")
        && opened.summon.answered.includes("84%")
        && opened.summon.applied === "0"
        && opened.summon.agreement === "16/44 (24%)" && opened.summon.cost === "$0.0123" && opened.summon.mode === "on"
        && opened.summon.latency === "230 ms (느릴 때 410)" && opened.recallWeek === "1,005",
      JSON.stringify(opened.summon));
    // One chip, one reason, and — while the feature still wants samples —
    // a bar with how many remain until the check, which then is the reason:
    // nothing said twice (t-6243 D2).
    const st = opened.statuses;
    ok("each feature's state is one chip, one reason and, while it wants samples, how many remain until the check",
      st.summon.chip === "적용 중" && st.summon.tone === "applying" && st.summon.reason === "직접 켰습니다 — 응답률이 기준에 못 미칩니다"
        && st.summon.progress === null && st.summon.facts.join("|") === "이전 버전 jev-1.12.0의 기록은 제외"
        && st.routing.chip === "적용 중" && st.routing.reason === "직접 켰습니다"
        && st.routing.progress?.now === "35" && st.routing.progress?.max === "73" && st.routing.progress?.label === "판정까지 38건"
        && st.recall.chip === "기록 중" && st.recall.tone === "recording" && st.recall.reason === "자동 적용 대상이 아니라 기록만 합니다"
        && st.placement.chip === "기준 미달" && st.placement.tone === "under" && st.placement.reason === "정확도가 기준에 못 미칩니다"
        && st.notify.chip === "키·동의 필요" && st.notify.tone === "blocked"
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
      const consent = { open: !settingsView.hidden, said: document.querySelector("#typesafe-status")?.textContent ?? "" };
      setSettingsOpen(false);
      await window.__PAINTED__();
      return { key, consent, back: activeTabId };
    });
    ok("a key chip opens settings on the key field, and a consent chip says where the folder is consented",
      fixed.key.open && fixed.key.focus === "typesafe-key-input" && fixed.consent.open
        && fixed.consent.said.includes("smart.jev.workspaces") && fixed.back === "jev",
      JSON.stringify(fixed));
    ok("the model is named once, over the table, and a row names a version only when it is another",
      opened.caption === "모델 jev-1.13.0" && Object.values(st).every((one) => !one.facts.some((fact) => fact.startsWith("모델 ")))
        && (opened.cardLine ?? "").includes("모델 jev-1.13.0 · 판정 표본 34건 · 이전 버전 jev-1.12.0의 기록은 제외"),
      JSON.stringify({ caption: opened.caption, card: opened.cardLine }));
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
      return opened;
    });
    const inUse = ["summon", "placement", "recall", "routing", "notify", "browser"];
    const unusedInOrder = opened.cardSeats.filter((id) => !inUse.includes(id));
    ok("one press unfolds the unused features in the card's order, and the next open remembers it",
      unfolded.rows.join(",") === [...inUse, ...unusedInOrder].join(",")
        && unfolded.expanded === "true" && unfolded.stored === "1" && unfolded.reopened.join(",") === unfolded.rows.join(","),
      JSON.stringify({ rows: unfolded.rows, expanded: unfolded.expanded, stored: unfolded.stored, reopened: unfolded.reopened.length }));
    const tr = opened.trends;
    ok("the week is one picture of two lines, response rate and accuracy, drawn only from three days with values",
      tr.summon.pictures === 1 && tr.summon.lines.join(",") === "answered:5,agreement:5"
        && tr.routing.pictures === 1 && tr.routing.lines.join(",") === "answered:7,agreement:7"
        && tr.notify.pictures === 0 && tr.notify.text === "1일치만 있음"
        && tr.browser.pictures === 0 && tr.browser.text === "1일치만 있음"
        && unfolded.quiet.pictures === 0 && unfolded.quiet.trend === "—",
      JSON.stringify({ ...tr, quiet: { pictures: unfolded.quiet.pictures, text: unfolded.quiet.trend } }));
    ok("a seat nothing has asked yet reads as never asked, not as zero",
      unfolded.quiet.week === "0" && unfolded.quiet.chips === 0 && unfolded.quiet.chip === "미사용"
        && unfolded.quiet.tone === "unused" && unfolded.quiet.reason === "",
      JSON.stringify(unfolded.quiet));
    ok("the recent list opens on the first seat with decisions and lists the digest",
      opened.recentSeat === "routing" && opened.recentCount === 1 && opened.recentFirst.includes("task: 68212a1194a4e327")
        && opened.recentFirst.includes("complexity=small") && opened.recentFirst.includes("적용")
        && opened.recentOptions.length === SEATS,
      JSON.stringify({ seat: opened.recentSeat, count: opened.recentCount, first: opened.recentFirst }));
    ok("the tab is titled and the head cells are column heads",
      opened.title !== null && opened.title.length > 0 && opened.heads.every((scope) => scope === "col"), JSON.stringify({ title: opened.title, heads: opened.heads.length }));
    ok("the freshness line names the ask's own cost", /\d+ ms/.test(opened.fresh), opened.fresh);

    // The recent list follows the picker, and a refused row wears its token.
    const picked = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const picker = view.querySelector(".jev-recent-seat");
      // Counted from here: settings opened earlier asked for itself.
      const before = window.__COUNTS__.jev_summary;
      picker.value = "summon";
      picker.dispatchEvent(new Event("change", { bubbles: true }));
      await window.__PAINTED__();
      return {
        count: view.querySelectorAll(".jev-decision").length,
        refused: view.querySelector('.jev-decision[data-outcome="not_consented"] .jev-decision-outcome')?.textContent ?? null,
        marks: [...view.querySelectorAll(".jev-decision-marks")].map((one) => one.textContent),
        asks: window.__COUNTS__.jev_summary - before,
      };
    });
    ok("choosing another seat lists its decisions without asking zo again",
      picked.count === 5 && picked.refused === "not_consented" && picked.asks === 0
        && picked.marks[0].includes("확신도 19%") && picked.marks[0].includes("적용됨") && picked.marks[0].includes("일치")
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

    ok("the dashboard raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
