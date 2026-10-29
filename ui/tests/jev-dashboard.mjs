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
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { openWindowTestPage } from "./window-boot.mjs";
import { realSummary } from "./jev-real-summary.mjs";
import { createRequire } from "node:module";
import { SETTINGS_QUALITY_SPEC, contrastTable, setQualityTheme, settlePaint } from "./settings-quality.mjs";

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
/* The 1080p stage the first screen is measured on (t-9633). */
const FIRST_SCREEN = { width: 1920, height: 1080 };
/* A phone's width, the narrowest a person's decisions are pictured at. */
const PHONE = { width: 390, height: 844 };
/* The features the built-in table names: every one the markup names but the
 * one a person's own decisions stand under (the fixture's
 * `customDecisions.feature`), which the part over the table holds instead
 * (t-62170). */
const BUILT_IN = SEATS - 1;

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
      seat.costUsd = 0;
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
  const rules = {
    feature: "custom", decisionsMax: 3, nameMaxChars: 30, questionMaxChars: 900, meaningMaxChars: 300,
    options: { least: 2, most: 6 }, hourly: { least: 2, most: 90 }, hourlyDefault: 17, hourlyAll: 55,
    actFrom: { least: 0.55, most: 0.9 }, actFromDefault: 0.75, trashKeptDays: 21,
    kinds: ["yes_no", "choice", "score"],
    points: [
      { at: "turn_start", inputs: ["task"] },
      { at: "turn_end", inputs: ["task", "final_text", "evidence", "edited_paths"] },
      { at: "before_command", inputs: ["command", "cwd", "task"] },
      { at: "agent_asks", inputs: ["context", "items"] },
    ],
    modes: modes.filter((choice) => !choice.automatic),
  };
  const store = { decisions: [], trash: [], folders: ["/Users/p/project", "/Users/p/shop"], everywhere: false };
  const custom = { rules, store, saves: [], checks: 0, lines: [], modes: [], deletes: [], restores: [], made: 0, deleted: new Map() };
  window.__JEV__ = { seats, modeOf, asks: [], jev, pressed: [], custom };
  const builtin = { saved: {}, calls: [], failNext: false };
  window.__JEV__.builtin = builtin;
  const provider = { kind: "jev", protocol: "system_one", baseUrl: "https://api.typesafe.ai", model: "jev-latest", requiresAuth: true };
  const settings = () => ({
    keysKeptHere: true, keySaved: true, jev: { ...jev }, customDecisions: structuredClone(rules),
    provider: { ...provider }, jevProvider: { ...provider }, providerIdentity: "jev-fixture",
    model: { model: provider.model, alias: "jev-latest", pinned: false },
    // Every seat's comparison sample floor, the use table's
    // (`JevUse::agreement_rows_wanted`, a judgment window's worth).
    // And the wall the model tier feature's paragraph names
    // (`JevUse` → `worded_wall_ms`, t-45961), a number of this fixture's
    // own so the page can only have read it from here.
    switches: seats.map((id) => ({ id, setting: id, mode: builtin.saved[id]?.deleted ? "off" : (builtin.saved[id]?.mode ?? modeOf[id]),
      decision: builtin.saved[id], modes, written: true, agreementRowsWanted: 20,
      wallMs: id === "model_tiers" ? 2_500 : null })),
    classifier: { setting: "autoClassifier", mode: "probed", probes: true, gates: seats[0],
      modes: [{ mode: "probed", runs: true, markers: false, probes: true }] },
  });
  window.__ANSWER__.typesafe_settings = settings;
  window.__ANSWER__.builtin_decision_definition = ({ id }) => ({
    id, saved: structuredClone(builtin.saved[id] ?? {}), questions: id === "judgment_cache" ? [] : id === "command_guard" ? [
      { id: "irreversible", type: "noul", instructions: "원래 데이터를 되돌릴 수 없나요?", criteria: { true: "원본을 잃는다", false: "원본을 유지한다" } },
      { id: "outside", type: "noul", instructions: "프로젝트 밖을 바꾸나요?", criteria: { true: "밖을 바꾼다", false: "안에서만 바꾼다" } },
    ] : id === "placement" ? [
      { id: "placement", type: "choice", instructions: "어디서 작업하나요?", criteria: { tab: { what: "새 탭", examples: ["검토"] }, split: "옆에서 작업" }, dynamicOptions: true },
    ] : [{ id: "n*", type: "score", instructions: "기본 판단 질문", criteria: ["낮음", "보통", "높음", "매우 높음"], contextual: true }],
    canEditPrompt: id !== "judgment_cache", dynamic: false, mode: builtin.saved[id]?.mode ?? modeOf[id],
    defaultMode: modeOf[id], modes, nameMax: 80, questionMax: 4_000, legacyQuestionMax: 2_000, criteriaMax: 8_000,
  });
  window.__ANSWER__.update_builtin_decision = ({ id, action, expected, draft }) => {
    builtin.calls.push({ id, action, expected: structuredClone(expected), draft: structuredClone(draft) });
    if (builtin.failNext) { builtin.failNext = false; throw { message: "fixture_write_failed" }; }
    const before = builtin.saved[id] ?? {};
    if (JSON.stringify(before) !== JSON.stringify(expected)) throw { message: "builtin_decision_changed" };
    if (action === "save") {
      const next = structuredClone(draft);
      if ((next.question || next.criteria || Object.keys(next.questions ?? {}).length) && (next.question !== (before.question ?? null) || next.criteria !== (before.criteria ?? null) || JSON.stringify(next.questions ?? {}) !== JSON.stringify(before.questions ?? {}))) next.mode = "shadow";
      builtin.saved[id] = next;
    } else if (action === "delete") builtin.saved[id] = { ...before, deleted: true };
    else if (action === "restore") builtin.saved[id] = { ...before, deleted: false };
    else if (action === "reset") delete builtin.saved[id];
    return settings();
  };

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
  const canonical = (value) => (Array.isArray(value) ? value.map(canonical)
    : value && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])])) : value);
  const versionOf = (draft) => {
    let hash = 7;
    const written = canonical({ ...draft, id: null, reads: [...draft.reads].sort(),
      folders: draft.folders ? [...draft.folders].sort() : null });
    for (const character of JSON.stringify(written)) hash = (hash * 31 + character.codePointAt(0)) >>> 0;
    return hash.toString(16).padStart(8, "0");
  };
  const refusal = (word, extra = {}) => ({ word, field: null, line: null, option: null, level: null, ...extra });
  const refusalsOf = (draft) => [
    ...String(draft.question ?? "").split("\n").flatMap((line, at) => (/^authorization:\s*bearer\s/i.test(line.trim())
      ? [refusal("secret_line", { field: "question", line: at + 1 })] : [])),
    ...(store.decisions.some((kept) => kept.draft.name.trim() === draft.name.trim() && kept.draft.id !== draft.id)
      ? [refusal("name_taken")] : []),
  ];
  const stamp = () => new Date().toISOString();
  window.__ANSWER__.list_custom_decisions = () => structuredClone(store);
  window.__ANSWER__.check_custom_decision = ({ draft }) => {
    custom.checks += 1;
    const answer = draft.answer;
    const first = answer.kind === "choice" ? answer.options[0]?.name ?? "" : answer.kind === "score" ? "0" : "yes";
    const line = `${draft.name} / ${first} / ${Math.round(draft.actFrom * 100)}`;
    custom.lines.push(line);
    return { refusals: refusalsOf(draft), version: versionOf(draft), line,
      questions: { [draft.id ?? "draft"]: { question: draft.question, answer } } };
  };
  window.__ANSWER__.save_custom_decision = ({ draft }) => {
    custom.saves.push(structuredClone(draft));
    const refusals = refusalsOf(draft);
    if (refusals.length > 0) throw { refusals };
    const kept = store.decisions.find((one) => one.draft.id === draft.id);
    if (kept) {
      Object.assign(kept, { draft: structuredClone(draft), version: versionOf(draft), updatedAt: stamp() });
    } else {
      custom.made += 1;
      store.decisions.push({
        mode: rules.modes.find((choice) => choice.asks && !choice.applies).mode, version: versionOf(draft),
        createdAt: stamp(), updatedAt: stamp(), draft: { ...structuredClone(draft), id: `cd-${custom.made.toString(16).padStart(8, "0")}` },
      });
    }
    return structuredClone(store);
  };
  window.__ANSWER__.set_custom_decision_mode = ({ id, mode }) => {
    custom.modes.push({ id, mode });
    store.decisions.find((one) => one.draft.id === id).mode = mode;
    return structuredClone(store);
  };
  window.__ANSWER__.delete_custom_decision = ({ id }) => {
    custom.deletes.push(id);
    const [gone] = store.decisions.splice(store.decisions.findIndex((one) => one.draft.id === id), 1);
    custom.deleted.set(id, gone);
    store.trash.push({ id, name: gone.draft.name, deletedAt: stamp() });
    return structuredClone(store);
  };
  window.__ANSWER__.restore_custom_decision = ({ id }) => {
    custom.restores.push(id);
    store.trash = store.trash.filter((one) => one.id !== id);
    store.decisions.push(custom.deleted.get(id));
    store.decisions.sort((one, other) => one.createdAt.localeCompare(other.createdAt));
    return structuredClone(store);
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

/* The evidence a person looks at (t-5807): the dashboard with this
 * machine's numbers in the light and the dark treatment, everything on one
 * screen, no text cut, clipped or overlapping; and the settings card the
 * same numbers stand on. */
async function testJevLongModelIds(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const models = [
    { name: "unbroken", id: `model-${"x".repeat(90)}` },
    { name: "qualified", id: `provider/${"y".repeat(87)}` },
  ];
  try {
    await page.evaluate(jevDashboardFixture);
    await page.evaluate(() => el("nav-jev").click());
    await page.waitForFunction(() => document.querySelector("#jev-view .jev-caption")?.textContent.includes("jev-1.13.0"));
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      for (const model of models) {
        await page.evaluate((id) => {
          jevNumbers = jevNumbers.map((held) => ({ ...held, model: id }));
          paintJevViews();
          document.querySelector("#jev-view").style.maxWidth = "319px";
        }, model.id);
        await standNarrow(page);
        const seen = await page.evaluate((id) => {
          const view = document.querySelector("#jev-view");
          const caption = view.querySelector(".jev-caption");
          const wide = [view, ...view.querySelectorAll(".jev-table-wrap, .jev-table, .jev-caption")]
            .filter((node) => node.scrollWidth > node.clientWidth + 1).map((node) => node.className);
          return {
            // The id stands whole inside the caption's words (t-76598: the
            // caption names the way and the place around it).
            text: caption.textContent, fullModel: caption.textContent.includes(id),
            faults: window.__JEV_TEXT_FAULTS__(view), wide,
            width: view.getBoundingClientRect().width,
            theme: document.documentElement.dataset.theme ?? "dark",
          };
        }, model.id);
        const path = join(EVIDENCE_DIR, `jev-dashboard-model-${theme}-${model.name}.png`);
        await page.screenshot({ path });
        ok(`the ${theme} dashboard preserves a 96-character ${model.name} model ID within 319 pixels`,
          seen.fullModel && seen.width > 0 && seen.width <= 320 && seen.faults.length === 0 && seen.wide.length === 0 && seen.theme === theme,
          JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
      }
    }
    ok("long model ID fixtures have no JavaScript errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The model over the table, as a person reads it (t-76598): the
 * connection's name, the model and the way it judges — never the provider's
 * `judgment-…` identity, which stays in a folded detail; an identity the
 * settings no longer name is 「이전 판단 모델」; since when it counts is said
 * only from the window's record of the apply; and a model applied but not
 * yet answered says the table's numbers are the earlier one's. */
async function testJevModelNames(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const identity = "judgment-0f107e000000a0123456789abcdef";
  try {
    await page.evaluate(jevDashboardFixture);
    await page.evaluate(() => el("nav-jev").click());
    await page.waitForFunction(() => document.querySelector("#jev-view .jev-caption")?.textContent.includes("jev-1.13.0"));
    const seen = (state) => page.evaluate(({ identity, state }) => {
      typesafeState = {
        ...typesafeState, providerIdentity: identity, providerConnectionName: "Fixture Router",
        provider: { kind: "remote", protocol: "openai_json", baseUrl: "http://192.0.2.8:4000/v1", model: "fixture-model", requiresAuth: true },
        model: { ...typesafeState.model, pinned: false },
      };
      jevApplied = state.applied;
      jevNumbers = (state.rows ? jevNumbers.slice(0, state.rows) : jevNumbers).map((held, at) => ({ ...held, model: at === 0 && state.other ? state.other : state.head }));
      paintJevViews();
      const view = document.querySelector("#jev-view");
      const caption = view.querySelector(".jev-caption");
      const detail = caption.querySelector(".jev-caption-detail");
      return {
        words: caption.querySelector(".jev-caption-words")?.textContent ?? caption.textContent,
        detail: detail ? { open: detail.open, raw: detail.querySelector("code")?.textContent ?? "" } : null,
        rows: [...view.querySelectorAll(".jev-status-fact")].map((node) => node.textContent),
      };
    }, { identity, state });
    const named = await seen({ head: `${identity}:fixture-model`, applied: { [identity]: Date.UTC(2026, 9, 27, 0, 35) } });
    ok("the caption names the connection, the model and the general way, never the identity",
      named.words.includes("Fixture Router · fixture-model · 일반 LLM") && !named.words.includes("judgment-"), JSON.stringify(named));
    ok("the identity waits in a folded detail", named.detail?.open === false && named.detail.raw === `${identity}:fixture-model`, JSON.stringify(named.detail));
    ok("since when the model counts is said from the window's record", named.words.includes("바꿈"), named.words);
    const caption = page.locator("#jev-view .jev-caption");
    await caption.scrollIntoViewIfNeeded();
    await page.locator("#jev-view .jev-table").screenshot({ path: join(EVIDENCE_DIR, "jev-dashboard-model-names-caption.png") });
    const unrecorded = await seen({ head: `${identity}:fixture-model`, applied: {} });
    ok("an apply the window did not record says no time", !unrecorded.words.includes("바꿈"), unrecorded.words);
    const earlier = await seen({ head: `${identity}:fixture-model`, other: "judgment-deadbeef:old-router", applied: {} });
    ok("a row answered by a provider the settings no longer name reads 「이전 판단 모델」",
      earlier.rows.some((row) => row === "이전 판단 모델 · old-router") && earlier.rows.every((row) => !row.includes("judgment-")), JSON.stringify(earlier.rows));
    const waiting = await seen({ head: "judgment-deadbeef:old-router", applied: { [identity]: Date.UTC(2026, 9, 27, 0, 35) } });
    ok("a model applied and not yet answered says the numbers are the earlier model's",
      waiting.words.includes("판단 모델: Fixture Router · fixture-model · 일반 LLM") && waiting.words.includes("이전 판단 모델 · old-router의 것") && !waiting.words.includes("judgment-"),
      waiting.words);
    // A mix: one feature answered with the model applied, two still hold the
    // earlier one's newest answer (the head) — the caption may not say the
    // new model has not answered, nor that every number is the earlier one's.
    const mixed = await seen({ head: "judgment-deadbeef:old-router", other: `${identity}:fixture-model`, rows: 3, applied: { [identity]: Date.UTC(2026, 9, 27, 0, 35) } });
    ok("a table mixing one row of the model applied and two of the earlier one says both, not 「아직 답이 없음」",
      mixed.words.includes("판단 모델: Fixture Router · fixture-model · 일반 LLM") && mixed.words.includes("기능 1개는 이 모델이 답했고, 2개는 아직 이전 판단 모델 · old-router의 기록")
        && !mixed.words.includes("아직 이 모델의 답이 없") && mixed.words.includes("바꿈") && !mixed.words.includes("judgment-"),
      mixed.words);
    await page.screenshot({ path: join(EVIDENCE_DIR, "jev-dashboard-model-names.png") });
    ok("model name fixtures have no JavaScript errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testJevDashboardEvidence(browser, origin, ok) {
  await mkdir(EVIDENCE_DIR, { recursive: true });
  await testJevLongModelIds(browser, origin, ok);
  await testJevModelNames(browser, origin, ok);
  const real = realSummary({ root: ROOT });
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
    // A fresh store has no used feature: its empty row and closed unused
    // fold are the loaded dashboard, with no week cell on the page yet.
    await page.waitForFunction(() => document.querySelector("#jev-view [data-jev-dash-row] [data-jev-fact=\"week\"]") !== null
      || (document.querySelector("#jev-view [data-jev-empty]") !== null
        && document.querySelector("#jev-view [data-jev-fold] button") !== null));
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
          unused: (jevNumbers ?? []).filter((one) => one.week.rows === 0 && one.id !== typesafeState.customDecisions?.feature).length,
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
        seen.faults.length === 0 && seen.oneScreen && seen.tableFits && seen.rows === BUILT_IN - seen.unused
          && seen.fold === (seen.unused > 0) && seen.pictures === seen.drawable && seen.strips === seen.asked
          && seen.charts === 2 && seen.theme === theme,
        JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
    }
    // Keep the real-count overview above. Narrow cards use their own page
    // with the same controlled fixture and figures on every machine: opening
    // every unused row in an empty store can exceed the capture height cap.
    {
      const { page, faults: narrowFaults } = await openWindowTestPage(browser, origin);
      try {
        await page.evaluate(jevDashboardFixture);
        await page.evaluate(() => el("nav-jev").click());
        await page.waitForSelector('#jev-view [data-jev-dash-row] [data-jev-fact="week"]');
        // A narrow window (t-9633): the rows stand as cards and the charts one
        // under the other, every word whole, nothing wider than the page — the
        // window as tall as the page, so the picture is the whole of it. Every
        // row holds the same figures, a short line and a long one (a judged
        // window of a hundred), so the check does not ride on what this machine
        // happened to count; they are set again as the page is read, so a
        // refresh in between cannot put the machine's back.
        ok("the narrow dashboard measurements include actual feature rows",
          await page.locator("#jev-view [data-jev-dash-row]").count() > 0);
        await page.evaluate(() => {
          window.__JEV_FIGURES__ = (figures) => {
            for (const row of document.querySelectorAll("#jev-view [data-jev-dash-row]")) {
              const nodes = [row.querySelector('[data-jev-fact="applied"]'), row.querySelector('[data-jev-fact="agreement"]'),
                row.querySelector('[data-jev-cell="cost"]')];
              row.__jevFigures ??= nodes.map((node) => node?.textContent ?? null);
              const texts = figures ? [figures.applied, figures.agreement, figures.cost] : row.__jevFigures;
              nodes.forEach((node, at) => { if (node && texts[at] !== null) node.textContent = texts[at]; });
            }
          };
        });
        const FIGURES = [
          { applied: "7", agreement: "8/9 (52%)", cost: "$0", shot: "" },
          { applied: "1,418", agreement: "98/100 (93%)", cost: "$0.192", shot: "-long" },
        ];
        for (const theme of ["dark", "light"]) {
          await setQualityTheme(page, theme);
          for (const figures of FIGURES) {
            await page.evaluate((one) => window.__JEV_FIGURES__(one), figures);
            await standNarrow(page);
            const seen = await page.evaluate((one) => {
              window.__JEV_FIGURES__(one);
              const view = document.querySelector("#jev-view");
              view.scrollTop = 0;
              const wide = [view, ...view.querySelectorAll(".jev-table-wrap, [data-jev-chart]")]
                .filter((node) => node.scrollWidth > node.clientWidth + 1).map((node) => node.className);
              return {
                faults: window.__JEV_TEXT_FAULTS__(view), wide, width: Math.round(view.getBoundingClientRect().width),
                theme: document.documentElement.dataset.theme ?? "dark",
              };
            }, figures);
            const path = join(EVIDENCE_DIR, `jev-dashboard-narrow-${theme}${figures.shot}.png`);
            await page.screenshot({ path });
            ok(`the ${theme} dashboard in a narrow window keeps every word whole and nothing wider than the page, ${figures.agreement} judged`,
              seen.faults.length === 0 && seen.wide.length === 0 && seen.theme === theme,
              JSON.stringify({ ...seen, faults: seen.faults.slice(0, 6), path }));
          }
        }
        await page.evaluate(() => window.__JEV_FIGURES__(null));
        ok("the narrow dashboard fixture raises no renderer errors", narrowFaults.length === 0, narrowFaults.join("\n"));
      } finally { await page.close(); }
    }
    ok("the evidence carries this machine's own count when zo answers, and says which",
      true,
      real ? `real: ${real.zo}${real.recent ? " --recent 12" : " (no recent list: that zo predates the flag)"}` : "fixture: no zo named in JEV_ZO_BIN, or the one named did not answer");

    await page.setViewportSize(oneScreen);

    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      await page.evaluate(() => setSettingsOpen(true, "jev-enabled"));
      await page.waitForFunction(() => document.getElementById("jev-enabled")?.checked !== undefined);
      await settlePaint(page);
      const basic = await page.evaluate(() => {
        const card = document.querySelector("#typesafe-card");
        // Getting a model and the old editor stand in the 고급 fold (t-75762 v2).
        document.getElementById("model-advanced").open = true;
        card.scrollIntoView({ block: "start" });
        const visible = (id) => {
          const node = document.getElementById(id);
          return Boolean(node && !node.closest("details:not([open])") && node.getClientRects().length > 0);
        };
        return {
          faults: window.__JEV_TEXT_FAULTS__(card),
          sourceControls: ["judgment-source-hub", "judgment-source-local", "judgment-source-jev", "judgment-source-repository"].filter(visible),
          hiddenConnectionFields: ["judgment-provider-kind", "judgment-endpoint", "typesafe-key-input"].filter((id) => !visible(id)),
          primaryActions: card.querySelectorAll(".judgment-model-source .btn--primary").length,
          switches: card.querySelectorAll("[data-jev-switch]").length,
          provider: document.getElementById("judgment-provider-kind").value,
        };
      });
      const path = join(EVIDENCE_DIR, `jev-settings-card-${theme}.png`);
      await page.locator("#typesafe-card").screenshot({ path });
      ok(`the ${theme} basic model card offers download, local folder and Jev without requiring server settings or cutting text`,
        basic.faults.length === 0 && basic.sourceControls.length === 4
          && basic.hiddenConnectionFields.length === 3 && basic.primaryActions === 1 && basic.switches === 1,
        JSON.stringify({ ...basic, faults: basic.faults.slice(0, 6), path, counted }));
      await page.locator("#judgment-connection-settings > summary").click();
      await settlePaint(page);
      const connection = await page.evaluate(() => {
        const card = document.querySelector("#typesafe-card");
        return {
          faults: window.__JEV_TEXT_FAULTS__(card),
          visibleSelectIds: [...card.querySelectorAll("select")]
            .filter((select) => !select.closest("details:not([open])") && select.getClientRects().length > 0)
            .map((select) => select.id),
          switches: card.querySelectorAll("[data-jev-switch]").length,
          keyVisible: document.getElementById("typesafe-key-input").getClientRects().length > 0,
          provider: document.getElementById("judgment-provider-kind").value,
        };
      });
      const connectionPath = join(EVIDENCE_DIR, `jev-settings-connection-${theme}.png`);
      await page.locator("#typesafe-card").screenshot({ path: connectionPath });
      ok(`the ${theme} connection card reveals its provider and key without changing the provider or cutting text`,
        connection.faults.length === 0 && connection.visibleSelectIds.length === 1
          && connection.visibleSelectIds[0] === "judgment-provider-kind" && connection.switches === 1
          && connection.keyVisible && connection.provider === basic.provider,
        JSON.stringify({ ...connection, faults: connection.faults.slice(0, 6), path: connectionPath, counted }));
      await page.locator("#judgment-connection-settings > summary").click();
      await page.evaluate(() => { document.getElementById("model-advanced").open = false; setSettingsOpen(false); });
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
        cardNames, cardSeats: window.__JEV__.seats, personal: window.__JEV__.custom.rules.feature,
        summon, recallWeek, statuses, caption, small, trends, switchOver, tableSelects,
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
        && opened.fold.text.includes(String(BUILT_IN - 6)) && opened.fold.expanded === "false",
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
        // Which cell holds the tallest row up, for the reader of a failure: each cell's own box, and its children's boxes in
        // brackets — a cell that wraps shows its extra line there (the first child alone hides it).
        tallestCells: (() => {
          const row = rows.reduce((a, b) => (b.getBoundingClientRect().height > a.getBoundingClientRect().height ? b : a));
          return [row.dataset.jevDashRow, ...[...row.querySelectorAll("td")].map((td) => {
            const kids = [...td.children].map((kid) => Math.round(kid.getBoundingClientRect().height));
            return `${td.dataset.jevCell}:${Math.round(td.getBoundingClientRect().height)}${kids.length > 0 ? `[${kids.join(",")}]` : ""}`;
          })];
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
          tallestCells: [tallest.dataset.jevDashRow, ...[...tallest.querySelectorAll("td")].map((td) => {
            const kids = [...td.children].map((kid) => Math.round(kid.getBoundingClientRect().height));
            return `${td.dataset.jevCell}:${Math.round(td.getBoundingClientRect().height)}${kids.length > 0 ? `[${kids.join(",")}]` : ""}/${Math.round(td.getBoundingClientRect().width)}`;
          })],
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

    // Who asked a feature a pane's hooks ask (t-10916): under its week's
    // count, each agent with requests by name and count — zo's own among
    // them — and how many agents' work the window cannot see, which and why
    // as that chip's tip; its drawer says every agent's line, a count with
    // what is said beside it, or "못 봄" with why. No other feature shows
    // such a line.
    const askers = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const saved = jevNumbers;
      const lines = [
        { agent: "zo", label: "ZO", requests: 3, notes: ["own_runtime"] },
        { agent: "claude", label: "Claude", requests: 5 },
        { agent: "codex", label: "Codex", requests: 2, notes: ["no_stop_flag"] },
        { agent: "cursor", label: "Cursor", requests: 0 },
        { agent: "antigravity", label: "Antigravity", requests: 0, notes: ["no_event_before", "no_prompt_event", "no_stop_flag"] },
        { agent: "opencode", label: "OpenCode", requests: 0, unseen: "no_hooks" },
      ];
      jevNumbers = saved.map((seat) => (seat.id !== "command_guard" ? seat : {
        ...seat, today: { ...seat.today, rows: 4, answered: 4 }, week: { ...seat.week, rows: 10, answered: 10 },
        askersWeek: { zo: 3, claude: 5, codex: 2 }, askers: lines,
      }));
      paintJevViews();
      await window.__PAINTED__();
      const row = view.querySelector('[data-jev-dash-row="command_guard"]');
      const line = row.querySelector('[data-jev-cell="rows"] [data-jev-askers]');
      const seen = {
        chips: [...(line?.children ?? [])].map((chip) => chip.textContent),
        tip: line?.querySelector('[data-jev-fact="unseen"]')?.dataset.tip ?? null,
        others: view.querySelectorAll('[data-jev-dash-row]:not([data-jev-dash-row="command_guard"]) [data-jev-askers]').length,
      };
      row.querySelector(".jev-row-open").click();
      await window.__PAINTED__();
      const part = view.querySelector('.jev-drawer [data-jev-drawer-part="askers"]');
      seen.drawer = { hidden: part.hidden,
        said: Object.fromEntries([...part.querySelectorAll("dd")].map((said) => [said.dataset.jevAsker, said.textContent])),
        names: [...part.querySelectorAll("dt")].map((name) => name.textContent) };
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      seen.elsewhere = view.querySelector('.jev-drawer [data-jev-drawer-part="askers"]').hidden;
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      jevNumbers = saved;
      paintJevViews();
      await window.__PAINTED__();
      return seen;
    });
    ok("a feature a pane's hooks ask names who asked it and how many agents it cannot see, and its drawer says every agent's line",
      askers.chips.join("|") === "ZO 3|Claude 5|Codex 2|못 봄 1"
        && askers.tip === "OpenCode — 훅을 설치하지 않는 에이전트"
        && askers.others === 0
        && !askers.drawer.hidden && askers.drawer.names.join("|") === "ZO|Claude|Codex|Cursor|Antigravity|OpenCode"
        && askers.drawer.said.zo === "3건 · zo가 직접 물음"
        && askers.drawer.said.claude === "5건"
        && askers.drawer.said.codex === "2건 · 멈춤을 알리지 않음"
        && askers.drawer.said.cursor === "0건"
        && askers.drawer.said.antigravity === "0건 · 실행 전에 알리지 않아 실행 뒤에 물음 · 사람의 요청을 알리지 않음 · 멈춤을 알리지 않음"
        && askers.drawer.said.opencode === "못 봄 — 훅을 설치하지 않는 에이전트"
        && askers.elsewhere,
      JSON.stringify(askers));
    // The completion claim and file pick seats a pane's turns ask too
    // (t-11349): the same line under each, drawn by the same code off the
    // same agent lines the backend writes — a count with what is said beside
    // it, and an agent whose hooks do not report the person's request shown
    // as not seen for the file pick, whose question needs it.
    const turnSeats = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const saved = jevNumbers;
      const lines = {
        claim: [
          { agent: "zo", label: "ZO", requests: 1, notes: ["own_runtime"] },
          { agent: "claude", label: "Claude", requests: 4 },
          { agent: "antigravity", label: "Antigravity", requests: 2, notes: ["no_prompt_event"] },
        ],
        file_pick: [
          { agent: "zo", label: "ZO", requests: 2, notes: ["own_runtime"] },
          { agent: "codex", label: "Codex", requests: 3 },
          { agent: "antigravity", label: "Antigravity", requests: 0, unseen: "no_prompt_event" },
        ],
      };
      jevNumbers = saved.map((seat) => (!lines[seat.id] ? seat : {
        ...seat, today: { ...seat.today, rows: 3, answered: 3 }, week: { ...seat.week, rows: 7, answered: 7 },
        askers: lines[seat.id],
      }));
      paintJevViews();
      await window.__PAINTED__();
      const chips = (id) => [...(view.querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="rows"] [data-jev-askers]`)?.children ?? [])]
        .map((chip) => chip.textContent);
      const seen = {
        claim: chips("claim"),
        filePick: chips("file_pick"),
        tip: view.querySelector('[data-jev-dash-row="file_pick"] [data-jev-askers] [data-jev-fact="unseen"]')?.dataset.tip ?? null,
      };
      view.querySelector('[data-jev-dash-row="claim"] .jev-row-open').click();
      await window.__PAINTED__();
      seen.said = Object.fromEntries([...view.querySelectorAll('.jev-drawer [data-jev-drawer-part="askers"] dd')]
        .map((said) => [said.dataset.jevAsker, said.textContent]));
      view.querySelector('[data-jev-dash-row="claim"] .jev-row-open').click();
      jevNumbers = saved;
      paintJevViews();
      await window.__PAINTED__();
      return seen;
    });
    ok("the claim and file pick seats name who asked them and which agents they cannot see, in the same plain words",
      turnSeats.claim.join("|") === "ZO 1|Claude 4|Antigravity 2"
        && turnSeats.filePick.join("|") === "ZO 2|Codex 3|못 봄 1"
        && turnSeats.tip === "Antigravity — 사람의 요청을 알리지 않음"
        && turnSeats.said.antigravity === "2건 · 사람의 요청을 알리지 않음"
        && turnSeats.said.claude === "4건",
      JSON.stringify(turnSeats));
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
      const key = { open: !settingsView.hidden, focus: document.activeElement?.id ?? null,
        connectionOpen: document.getElementById("judgment-connection-settings").open };
      setSettingsOpen(false);
      await window.__PAINTED__();
      view.querySelector('[data-jev-dash-row="notify"] .jev-token[data-token="not_consented"]').click();
      await window.__PAINTED__();
      const consent = { open: !settingsView.hidden, said: document.querySelector("#model-judgment-status")?.textContent ?? "",
        focus: document.activeElement?.id ?? null };
      setSettingsOpen(false);
      await window.__PAINTED__();
      return { key, consent, back: activeTabId };
    });
    ok("a key chip opens settings on the key field, and a consent chip lands on the switch that consents every folder",
      fixed.key.open && fixed.key.focus === "typesafe-key-input" && fixed.key.connectionOpen && fixed.consent.open
        && fixed.consent.focus === "jev-enabled" && fixed.consent.said.includes("「모든 폴더에서 사용」")
        && fixed.back === "jev",
      JSON.stringify(fixed));
    const legacyProvider = await page.evaluate(() => {
      const saved = typesafeState;
      paintTypeSafe({ ...saved, provider: null, providerIdentity: null });
      const result = {
        disabled: el("typesafe-key-input").disabled,
        reason: el("judgment-provider-draft").textContent,
      };
      paintTypeSafe(saved);
      return result;
    });
    ok("an old response without provider details explains why connection editing is unavailable",
      legacyProvider.disabled && legacyProvider.reason.includes("현재 적용된 모델을 읽지 못했습니다"), JSON.stringify(legacyProvider));
    ok("the model is named once, over the table, and a row names a version only when it is another",
      // t-76598: the caption names the place, the model and the way.
      opened.caption === "판단 모델: Jev 서비스 · jev-1.13.0 · System One" && Object.values(st).every((one) => !one.facts.some((fact) => fact.startsWith("모델 "))),
      JSON.stringify({ caption: opened.caption }));
    ok("the global switch stands over the table while per-feature edits use their own dialog",
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
    const unusedInOrder = opened.cardSeats.filter((id) => !inUse.includes(id) && id !== opened.personal);
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
      // Which edge the toast layer stands on: a drawer docks its buttons at
      // the bottom-right, and the layer steps aside while one stands (t-68117).
      const toastSide = () => (document.querySelector(".toasts").getBoundingClientRect().left < innerWidth / 2 ? "left" : "right");
      const closedSide = toastSide();
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
          toasts: toastSide(),
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
      const escaped = { hidden: view.querySelector(".jev-drawer").hidden, focus: document.activeElement?.closest("[data-jev-dash-row]")?.dataset.jevDashRow ?? null,
        toasts: toastSide() };
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      const toggled = view.querySelector(".jev-drawer").hidden;
      return { routing, summon, browser, pinned, escaped, toggled, closedSide, toggledSide: toastSide(), asks: window.__COUNTS__.jev_summary - before };
    });
    const dr = drawer;
    // The drawer stands deep in its tab, beside the conversations, where a
    // `:has()` would be matched again over the whole window at every `hidden`
    // that changed (t-84136): its own paint tells the layer.
    ok("a feature's drawer steps the toasts to the left edge while it stands, and they come back as it closes",
      dr.closedSide === "right" && dr.routing.toasts === "left" && dr.summon.toasts === "left" && dr.escaped.toasts === "right" &&
        dr.toggledSide === "right",
      JSON.stringify({ closed: dr.closedSide, routing: dr.routing.toasts, summon: dr.summon.toasts, escaped: dr.escaped.toasts, toggled: dr.toggledSide }));
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
      named.rows.length === BUILT_IN && unnamed.length === 0 && named.rows.every((row) => row.tip.length < row.expected.length + 1)
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
      refreshed.reopened === refreshed.after && refreshed.rows === BUILT_IN && refreshed.closedPressed === "false", JSON.stringify(refreshed));
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
      english.head === "Feature" && english.title === "Decisions" && english.refresh === "Refresh" && english.back === "기능",
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
      !refused.hidden && refused.error.includes("refused: jev_summary") && refused.rows === BUILT_IN && refused.week === "50",
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

    // A feature's paragraph names its wall from the backend's row for it
    // (`wallMs`, t-45961) — no catalog writes the number down — and a row
    // that names none fills nothing in.
    const wall = await page.evaluate(() => {
      const hint = jevFeature("model_tiers").hint;
      return {
        said: t(hint.key, hint.source, jevHintVars(jevSeat(typesafeState ?? {}, "model_tiers"))),
        source: hint.source,
        none: jevHintVars(jevSeat(typesafeState ?? {}, "routing")),
      };
    });
    ok("a feature's paragraph says the wall its backend row names, from no number written in the catalogs",
      wall.source.includes("{{seconds}}초") && wall.said.includes("2.5초") && !wall.said.includes("{{")
        && Object.keys(wall.none).length === 0,
      JSON.stringify(wall));

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
        const viewport = page.viewportSize();
        await page.setViewportSize({ ...viewport, height: SETTINGS_QUALITY_SPEC.viewport.height });
        await page.evaluate(async () => {
          document.querySelector("#jev-view .jev-drawer-close").click();
          setSettingsOpen(true, "jev-enabled");
          await window.__PAINTED__();
        });
        const disclosures = await page.evaluateHandle(async () => {
          const saved = [...document.querySelectorAll("#typesafe-card details")]
            .map((element) => ({ element, open: element.open }));
          for (const { element } of saved) element.open = true;
          await window.__PAINTED__();
          return saved;
        });
        const frame = page.locator(".settings-frame");
        await frame.evaluate((node) => node.scrollTo({ top: 0, left: 0, behavior: "instant" }));
        await settlePaint(page);
        const scrollBefore = await frame.evaluate((node) => [node.scrollTop, node.scrollLeft]);
        const clipped = await page.locator("#typesafe-key-input").evaluate((node) => {
          const control = node.getBoundingClientRect();
          const bounds = node.closest(".settings-frame").getBoundingClientRect();
          return control.top >= bounds.bottom || control.bottom <= bounds.top;
        });
        const card = await contrastTable(page, AxeBuilder, "#typesafe-card");
        const scrollAfter = await frame.evaluate((node) => [node.scrollTop, node.scrollLeft]);
        const hiddenEndpoint = card.find((row) => row.target === "#judgment-endpoint");
        ok(`${theme}: the endpoint hidden for the built-in provider is unmeasured, not passed`,
          hiddenEndpoint?.verdict === "hidden" && hiddenEndpoint.ratio === null
            && hiddenEndpoint.why === "display:none at #judgment-endpoint-field",
          JSON.stringify(hiddenEndpoint));
        const key = card.find((row) => row.target === "#typesafe-key-input");
        ok(`${theme}: the offscreen key placeholder is measured and the card's scroll is restored`,
          clipped && key?.ratio >= CONTRAST_FLOOR && key.fg !== null && key.bg !== null
            && JSON.stringify(scrollBefore) === JSON.stringify(scrollAfter),
          JSON.stringify({ key, clipped, scrollBefore, scrollAfter }));

        // A hidden-pass retry must also catch bad ink. Give the same input
        // text whose ink equals its ground; this must remain below the gate.
        const input = page.locator("#typesafe-key-input");
        const saved = await input.evaluate((node) => {
          const held = { value: node.value, style: node.style.cssText };
          node.value = node.placeholder;
          node.style.color = getComputedStyle(node).backgroundColor;
          return held;
        });
        try {
          const bad = await contrastTable(page, AxeBuilder, "#typesafe-card");
          const badKey = bad.find((row) => row.target === key?.target);
          ok(`${theme}: low contrast in the offscreen input still fails the contrast floor`,
            badKey?.ratio !== null && badKey?.ratio < CONTRAST_FLOOR,
            JSON.stringify(badKey));
        } finally {
          await input.evaluate((node, held) => {
            node.value = held.value;
            node.style.cssText = held.style;
          }, saved);
        }
        await disclosures.evaluate((saved) => {
          for (const { element, open } of saved) element.open = open;
        });
        await disclosures.dispose();
        await page.evaluate(() => setSettingsOpen(false));
        await page.setViewportSize(viewport);
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
    const visible = rows.filter((row) => row.verdict !== "hidden");
    const under = visible.filter((row) => row.verdict === "fail" || (row.ratio !== null && row.ratio < CONTRAST_FLOOR));
    const unknown = visible.filter((row) => row.verdict === "unknown" || row.ratio === null);
    ok(`every visible text on the dashboard and the card clears ${CONTRAST_FLOOR}:1 in both treatments`,
      AxeBuilder !== null && visible.length > 0 && under.length === 0 && unknown.length === 0,
      AxeBuilder === null ? "@axe-core/playwright is not installed" : JSON.stringify({
        measured: visible.length, hidden: rows.length - visible.length,
        least: Math.min(...visible.filter((row) => row.ratio !== null).map((row) => row.ratio)),
        under: under.slice(0, 6), unknown: unknown.slice(0, 6) }));

    await testJevReview(page, ok);
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
    const accounting = await page.evaluate(() => {
      const known = { week: { rows: 1 }, costUsd: 0.1, days: [{ startMs: 1, tally: { rows: 1, inputTokens: 100, requests: 1 } }] };
      const unknown = { week: { rows: 1 }, costUsd: null, days: [{ startMs: 1, tally: { rows: 1, inputTokens: 0, requests: 1, unmeteredRequests: 1 } }] };
      const card = document.querySelector('[data-jev-chart="tokens"]').cloneNode(true);
      const days = jevTokenDays([known, unknown]);
      paintJevTokens(card, days, false);
      const mixed = card.querySelector(".jev-chart-sub").textContent;
      paintJevTokens(card, jevTokenDays([unknown]), false);
      return { total: jevTotalCost([known, unknown]), known: jevTotalCost([known]), day: days[0].cost, mixed, empty: card.querySelector(".jev-chart-empty").textContent };
    });
    ok("missing usage stays unknown in the total, daily estimate and empty chart",
      accounting.total === null && accounting.known === 0.1 && accounting.day === null
        && accounting.mixed === "추정 비용 —" && accounting.empty === "요청의 입력 토큰 사용량을 확인하지 못했습니다",
      JSON.stringify(accounting));
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
/* A feature the door rests says so where its state is said (t-58193): a chip
 * in the rest's own word beside its state's, and one sentence — until when,
 * in the person's local time by the window's own clock (`quotaWallClock`);
 * why, in the language's words for the cause, never zo's; and for a rest of
 * its own after how many unanswered requests, by the door's own number,
 * read here from its source (`UNANSWERED_BEFORE_REST`) so a rule that moves
 * moves the words. A wait its server asked for has no count, and a feature
 * whose rest has passed says neither. The rest's chip takes a wait's tone
 * (`--jev-tone-under`), as a state's chip takes its state's. */
export async function testJevDashboardResting(browser, origin, ok) {
  const gate = readFileSync(join(ROOT, "zo-ide", "crates", "tools", "src", "misc_tools", "smart_router", "jev_gate.rs"), "utf8");
  const unanswered = Number(gate.match(/\npub const UNANSWERED_BEFORE_REST: usize = (\d+);/)?.[1]);
  ok("the door's count of unanswered requests was read from its source", Number.isInteger(unanswered) && unanswered > 0, String(unanswered));
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(jevDashboardFixture);
    const seen = await page.evaluate(async (count) => {
      const base = window.__ANSWER__.jev_summary;
      const untilMs = Date.now() + 20 * 60_000;
      const rests = {
        recall: { untilMs, cause: "timeout", unanswered: count },
        routing: { untilMs, cause: "waiting", unanswered: null },
        placement: { untilMs: Date.now() - 1_000, cause: "timeout", unanswered: count },
      };
      window.__ANSWER__.jev_summary = (args) => {
        const answered = base(args);
        return { ...answered, seats: answered.seats.map((seat) => (rests[seat.id] ? { ...seat, resting: rests[seat.id] } : seat)) };
      };
      el("nav-jev").click();
      const view = () => document.querySelector("#jev-view");
      await new Promise((done, fail) => {
        const until = performance.now() + 3_000;
        const look = () => {
          const rows = view()?.querySelectorAll("[data-jev-dash-row]") ?? [];
          if ([...rows].some((row) => row.querySelector('[data-jev-fact="week"]')?.textContent === "50")) done();
          else if (performance.now() > until) fail(new Error("the table never drew"));
          else requestAnimationFrame(look);
        };
        look();
      });
      const time = quotaWallClock(untilMs);
      const wait = document.createElement("span");
      wait.style.color = "var(--jev-tone-under)";
      view().append(wait);
      const waitTone = getComputedStyle(wait).color;
      wait.remove();
      const restOf = (id) => {
        const holder = view().querySelector(`[data-jev-dash-row="${id}"] [data-jev-cell="status"]`);
        const sentence = holder?.querySelector("[data-jev-rest]")?.textContent ?? null;
        const chip = holder?.querySelector(".jev-chip[data-jev-resting]");
        return {
          chip: chip?.textContent ?? null,
          tone: chip ? getComputedStyle(chip).color : null,
          sentence,
          // The numbers the sentence says besides its time.
          numbers: sentence === null ? [] : sentence.replace(time, "").match(/\d[\d,.]*/g) ?? [],
        };
      };
      return {
        recall: restOf("recall"), routing: restOf("routing"), placement: restOf("placement"),
        time, waitTone, count: jevCount(count), word: t("jev.token.resting", "쉬는 중"),
        timeout: t("jev.rest.cause.timeout", "시간 초과"), waiting: t("jev.rest.cause.waiting", "서버가 기다려 달라고 했습니다"),
      };
    }, unanswered).catch((error) => ({ error: String(error) }));
    const { recall, routing, placement } = seen;
    ok("a resting feature says until when, why and after how many unanswered requests, beside the rest's own chip",
      !seen.error && recall.chip === seen.word && Boolean(recall.sentence?.includes(seen.time))
        && recall.sentence.includes(seen.timeout) && recall.numbers.join(",") === seen.count,
      JSON.stringify(seen.error ?? { recall, time: seen.time, cause: seen.timeout, count: seen.count }));
    ok("a feature its server asked to wait says until when and why, and counts nothing",
      !seen.error && routing.chip === seen.word && Boolean(routing.sentence?.includes(seen.time))
        && routing.sentence.includes(seen.waiting) && routing.numbers.length === 0,
      JSON.stringify(seen.error ?? { routing, time: seen.time, cause: seen.waiting }));
    ok("a resting feature's chip takes a wait's tone",
      !seen.error && Boolean(seen.waitTone) && recall.tone === seen.waitTone && routing.tone === seen.waitTone,
      JSON.stringify(seen.error ?? { recall: recall.tone, routing: routing.tone, wait: seen.waitTone }));
    ok("a feature whose rest has passed shows neither the chip nor the sentence",
      !seen.error && placement.chip === null && placement.sentence === null,
      JSON.stringify(seen.error ?? placement));
    ok("the resting dashboard raised no renderer fault", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

async function testJevReview(page, ok) {
  const seen = await page.evaluate(async () => {
    const view = document.querySelector("#jev-view");
    const calls = [];
    const until = (ready) => new Promise((resolve, reject) => {
      const end = performance.now() + 3000;
      const tick = () => {
        if (ready()) resolve();
        else if (performance.now() >= end) reject(new Error("review operation did not reach its expected state"));
        else requestAnimationFrame(tick);
      };
      tick();
    });
    const sample = (id, words) => ({ case: { id, seat: "routing", rubricVersion: 1, model: "jev-test",
      request: { state: words, questions: { q: { type: "noul" } } }, answers: { q: { type: "noul", noul: 0.5 } } },
      reason: "uncertain", remaining: ["q"] });
    const answer = (id, words) => ({ enabled: true, samples: [sample(id, words)], total: 1, capacity: 512, invalid: 0 });
    window.__ANSWER__.jev_review = (args) => { calls.push(args); return answer("case-one", "<img data-review-injection onerror=alert(1)>"); };
    let finishOutcome;
    const outcomes = [];
    window.__ANSWER__.jev_review_outcome = (args) => { outcomes.push(args); return new Promise((resolve) => { finishOutcome = resolve; }); };
    jevToggleDrawer(view, null);
    jevToggleDrawer(view, "routing");
    const drawer = view.querySelector(".jev-drawer");
    const noCallsFromPainting = calls.length === 0;
    await jevLoadReviews(drawer);
    const form = drawer.querySelector(".jev-review-case form");
    const note = form.querySelector("textarea");
    const buttons = [...form.querySelectorAll("button")];
    const emptyHeld = buttons.every((button) => button.disabled);
    note.value = "The observed result contradicted the answer.";
    note.dispatchEvent(new Event("input", { bubbles: true }));
    buttons[1].click();
    await until(() => typeof finishOutcome === "function");
    const pendingHeld = buttons.every((button) => button.disabled) && note.disabled;
    const safeText = !drawer.querySelector("[data-review-injection]") && drawer.querySelector(".jev-review-case pre").textContent.includes("<img");
    // Move the drawer while the write is pending: its captured case must still be the one reviewed.
    jevReviewPart(drawer, "recall");
    finishOutcome(null);
    await new Promise((resolve) => setTimeout(resolve, 0));

    let finishOld;
    window.__ANSWER__.jev_review = () => new Promise((resolve) => { finishOld = resolve; });
    const old = jevLoadReviews(drawer);
    await until(() => typeof finishOld === "function");
    jevReviewPart(drawer, "skills");
    window.__ANSWER__.jev_review = () => answer("case-new", "current evidence");
    await jevLoadReviews(drawer);
    finishOld(answer("case-old", "stale evidence"));
    await old;
    const staleHeld = drawer.querySelector(".jev-review-cases").textContent.includes("current evidence")
      && !drawer.querySelector(".jev-review-cases").textContent.includes("stale evidence");

    const priorScope = jevScopeChoice;
    jevScopeChoice = priorScope === "projects" ? "workspace" : "projects";
    jevReviewPart(drawer, "skills");
    const scopeCleared = !drawer.querySelector(".jev-review-case");
    let askedScope;
    window.__ANSWER__.jev_review = (args) => { askedScope = args.scope; return answer("case-scope", "scoped evidence\n".repeat(80)); };
    await jevLoadReviews(drawer);
    const scopeMatched = askedScope === jevScopeChoice;
    jevScopeChoice = priorScope;
    jevReviewPart(drawer, "skills");
    await jevLoadReviews(drawer);

    window.__ANSWER__.set_jev_review_enabled = () => { throw new Error("settings write refused"); };
    const checkbox = drawer.querySelector(".jev-review-toggle input");
    checkbox.checked = false;
    checkbox.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    return { noCallsFromPainting, calls, emptyHeld, pendingHeld, safeText, staleHeld, scopeCleared, scopeMatched,
      rolledBack: checkbox.checked && !checkbox.disabled, outcomes };
  }).catch((error) => ({ error: String(error) }));
  ok("Jev review reads local evidence only after an explicit load", !seen.error && seen.noCallsFromPainting
    && seen.calls.length === 1 && seen.calls[0].seat === "routing", JSON.stringify(seen));
  ok("a Jev review needs evidence and prevents duplicate writes while saving", !seen.error && seen.emptyHeld && seen.pendingHeld,
    JSON.stringify(seen));
  ok("Jev source text is inert and an outcome stays with the displayed case after navigation", !seen.error && seen.safeText
    && seen.outcomes.length === 1 && seen.outcomes[0].caseId === "case-one" && seen.outcomes[0].question === "q"
    && seen.outcomes[0].correct === false, JSON.stringify(seen));
  ok("an old Jev review response cannot replace a newly selected feature", !seen.error && seen.staleHeld, JSON.stringify(seen));
  ok("Jev review changes scope with the dashboard and clears the old workspace's evidence",
    !seen.error && seen.scopeCleared && seen.scopeMatched, JSON.stringify(seen));
  ok("a refused review setting write restores the previous checkbox", !seen.error && seen.rolledBack, JSON.stringify(seen));
  let keyboard = { error: seen.error ?? null };
  if (!seen.error) {
    try {
      await page.locator(".jev-review-case details").evaluate((node) => { node.open = true; });
      const evidence = page.locator(".jev-review-case pre");
      await evidence.scrollIntoViewIfNeeded();
      await evidence.focus();
      const before = await page.evaluate(() => window.__COUNTS__?.key_input ?? 0);
      await page.keyboard.press("PageDown");
      await page.waitForFunction(() => document.querySelector(".jev-review-case pre")?.scrollTop > 0, null, { timeout: 2000 });
      keyboard = await evidence.evaluate((node, count) => ({
        focused: document.activeElement === node, scrolled: node.scrollTop > 0,
        terminalKeys: (window.__COUNTS__?.key_input ?? 0) - count,
      }), before);
    } catch (error) { keyboard = { error: String(error) }; }
  }
  ok("the Jev evidence owns PageDown and scrolls without sending a key to the terminal",
    !keyboard.error && keyboard.focused && keyboard.scrolled && keyboard.terminalKeys === 0, JSON.stringify(keyboard));
}

/* A person's own decisions on the dashboard (t-62170 stage 3, DESIGN 3):
 * with none, no new line — only 「새 결정」 on the switch line; with some, a
 * part between the charts and the built-in table. A decision is made, refused
 * at a secret line, edited into a new version with its mode kept, deleted
 * behind one question with an undo and a trash, switched between modes, held
 * to the store's limit, and reached from the keyboard. Every number the page
 * says is the fixture's (`customDecisions`, none of them core's) and every
 * refusal the check's. With `JEV_CUSTOM_SHOTS` naming a folder, each state is
 * pictured light and dark, and the switch line, the list and the drawer at a
 * phone's width too. */
export async function testJevCustomDecisions(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  const folder = process.env.JEV_CUSTOM_SHOTS ? resolve(ROOT, process.env.JEV_CUSTOM_SHOTS) : null;
  const pictures = [];
  const picture = async (name, phone = false, shown = null) => {
    if (!folder) return;
    await mkdir(folder, { recursive: true });
    for (const width of phone ? [null, PHONE.width] : [null]) {
      let scrolled = 0;
      if (width !== null) {
        await page.setViewportSize(PHONE);
        scrolled = await page.evaluate(() => {
          const view = document.querySelector("#jev-view");
          const before = view.scrollTop;
          Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "29" });
          return before;
        });
      }
      for (const theme of ["light", "dark"]) {
        await setQualityTheme(page, theme);
        if (width !== null && shown !== null) {
          await page.evaluate((selector) => {
            const view = document.querySelector("#jev-view");
            view.scrollTop += view.querySelector(selector).getBoundingClientRect().top - view.getBoundingClientRect().top;
          }, shown);
        }
        const path = join(folder, `jev-custom-${name}-${theme}${width === null ? "" : `-${width}`}.png`);
        await page.screenshot({ path });
        pictures.push(path);
      }
      if (width !== null) {
        await page.evaluate((before) => {
          const view = document.querySelector("#jev-view");
          Object.assign(view.style, { position: "", inset: "", width: "", height: "", zIndex: "" });
          view.scrollTop = before;
        }, scrolled);
        await page.setViewportSize(FIRST_SCREEN);
        await settlePaint(page);
      }
    }
  };
  const waitFor = (test, argument = null, timeout = 3_000) =>
    page.waitForFunction(test, argument, { timeout }).then(() => true, () => false);
  const checked = () => waitFor(() => el("jev-custom-form")?.getAttribute("aria-busy") === "false");
  const fill = async (values) => {
    for (const [name, value] of Object.entries(values)) {
      await page.fill(`#jev-custom-form [name="${name}"]`, value, { timeout: 1_000 }).catch(() => {});
    }
    return checked();
  };
  const press = (selector) => page.evaluate((target) => document.querySelector(target)?.click(), selector);
  const drawerOpen = () => waitFor(() => el("jev-custom-scrim")?.hidden === false);
  const drawerClosed = () => waitFor(() => el("jev-custom-scrim")?.hidden !== false);
  const rowOf = (index) => page.evaluate((at) => window.__JEV__.custom.store.decisions[at]?.draft.id ?? "", index);
  try {
    await page.setViewportSize(FIRST_SCREEN);
    await page.evaluate(jevDashboardFixture);
    await page.evaluate(() => el("nav-jev").click());
    await waitFor(() => document.querySelector('#jev-view [data-jev-dash-row="summon"] [data-jev-fact="week"]')?.textContent === "50");
    await waitFor(() => (window.__COUNTS__.list_custom_decisions ?? 0) > 0);
    await settlePaint(page);
    const rules = await page.evaluate(() => window.__JEV__.custom.rules);

    const empty = await page.evaluate(async () => {
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      const shown = (node) => Boolean(node) && node.getClientRects().length > 0;
      const row = view.querySelector(".jev-switch [data-jev-switch-row]");
      const guide = view.querySelector("[data-jev-custom-guide]");
      const first = view.querySelector("[data-jev-dash-row]");
      const top = () => Math.round(first.getBoundingClientRect().top - view.getBoundingClientRect().top + view.scrollTop);
      const tall = () => Math.round(row.getBoundingClientRect().height);
      const seen = { asked: window.__COUNTS__.list_custom_decisions ?? 0, guide: shown(guide), part: shown(view.querySelector("[data-jev-custom]")) };
      if (!seen.guide) return seen;
      const box = guide.getBoundingClientRect();
      const line = row.getBoundingClientRect();
      Object.assign(seen, {
        onSwitchLine: guide.closest("[data-jev-switch-row]") === row,
        level: box.top >= line.top - 1 && box.bottom <= line.bottom + 1,
        words: guide.textContent.trim(),
        withGuide: { top: top(), tall: tall() },
      });
      guide.style.display = "none";
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      seen.withoutGuide = { top: top(), tall: tall() };
      guide.style.display = "";
      return seen;
    });
    ok("a person with no decisions sees no new line: 「새 결정」 stands on the switch line, and the first built-in row stands where it would without it",
      empty.asked > 0 && empty.guide && !empty.part && empty.onSwitchLine && empty.level && empty.words.includes("새 결정")
        && empty.withGuide.top === empty.withoutGuide.top && empty.withGuide.tall === empty.withoutGuide.tall,
      JSON.stringify(empty));
    await picture("empty", true);

    await press("#jev-view [data-jev-custom-guide]");
    await drawerOpen();
    await checked();
    const opened = await page.evaluate(() => {
      const form = el("jev-custom-form");
      const field = (name) => form?.querySelector(`[name="${name}"]`) ?? null;
      const chosen = (name) => [...(form?.querySelectorAll(`[name="${name}"]:checked`) ?? [])].map((one) => one.value);
      return {
        open: el("jev-custom-scrim")?.hidden === false, top: window.__MODAL_STACK__().at(-1) ?? null,
        focus: document.activeElement !== null && document.activeElement === field("name"),
        title: el("jev-custom-title")?.textContent.trim() ?? "",
        kind: chosen("kind"), at: chosen("at"), reads: chosen("reads"), folders: chosen("folders"),
        actFrom: [field("actFrom")?.value, field("actFrom")?.min, field("actFrom")?.max].map(Number),
        hourly: [field("maxPerHour")?.value, field("maxPerHour")?.min, field("maxPerHour")?.max].map(Number),
        line: form?.querySelector("[data-jev-custom-line]")?.textContent.trim() ?? null,
        lastLine: window.__JEV__.custom.lines.at(-1) ?? "",
      };
    });
    ok("「새 결정」 opens the drawer as the window's top modal, the keyboard in its name and every default the settings answer's",
      opened.open && opened.top === "jev-custom-scrim" && opened.focus && opened.title === "새 결정"
        && opened.kind.join() === rules.kinds[0] && opened.at.join() === rules.points[0].at
        && opened.reads.join() === rules.points[0].inputs.join() && opened.folders.join() === "consented"
        && opened.actFrom.join() === [rules.actFromDefault, rules.actFrom.least, rules.actFrom.most].join()
        && opened.hourly.join() === [rules.hourlyDefault, rules.hourly.least, rules.hourly.most].join(),
      JSON.stringify(opened));

    const first = { name: "릴리스 노트가 필요한가", question: "이번 턴의 변경이 사용자에게 보이는 동작을 바꿨나요?", yes: "보이는 동작이 바뀌었다", no: "바뀌지 않았다" };
    const firstChecked = await fill(first);
    const previewed = await page.evaluate(() => ({
      line: el("jev-custom-form")?.querySelector("[data-jev-custom-line]")?.textContent.trim() ?? null,
      lastLine: window.__JEV__.custom.lines.at(-1) ?? "",
      questions: el("jev-custom-form")?.querySelector("[data-jev-custom-questions]")?.textContent ?? "",
    }));
    ok("the drawer prints the line an agent is told and the question as the check returned them",
      firstChecked && previewed.lastLine !== "" && previewed.line === previewed.lastLine && previewed.questions.includes(first.question),
      JSON.stringify(previewed));
    await press("#jev-custom-save");
    const madeClosed = await drawerClosed();
    await waitFor(() => document.querySelector("#jev-view [data-jev-custom-row]") !== null);
    const made = await page.evaluate(() => {
      const custom = window.__JEV__.custom;
      const kept = custom.store.decisions[0] ?? null;
      const row = kept ? document.querySelector(`#jev-view [data-jev-custom-row="${kept.draft.id}"]`) : null;
      const radio = row?.querySelector('[data-jev-custom-mode] [role="radio"][aria-checked="true"]') ?? null;
      return {
        saves: custom.saves.length, sent: custom.saves[0] ?? null, checks: custom.checks,
        name: row?.querySelector(".jev-custom-name")?.textContent.trim() ?? null,
        mode: radio?.dataset.mode ?? null, modeWords: radio?.textContent.trim() ?? null,
        recording: custom.rules.modes.find((choice) => choice.asks && !choice.applies).mode,
        selects: row?.querySelectorAll("select").length ?? null,
        group: row?.querySelector("[data-jev-custom-mode]")?.getAttribute("role") ?? null,
        focus: document.activeElement?.matches?.("#jev-view [data-jev-custom-new]") ?? false,
      };
    });
    const sent = made.sent ?? {};
    ok("saving a new decision asks the store once with the draft as written, and its line stands at 「기록만」 on radios, not a select",
      madeClosed && made.saves === 1 && made.checks > 0 && made.name === first.name
        && made.mode === made.recording && made.modeWords === "기록만" && made.selects === 0 && made.group === "radiogroup"
        && (sent.id ?? null) === null && sent.name === first.name && sent.question === first.question
        && sent.answer?.kind === rules.kinds[0] && sent.answer?.yes === first.yes && sent.answer?.no === first.no
        && sent.at === rules.points[0].at && sent.reads?.join() === rules.points[0].inputs.join()
        && sent.folders === null && sent.actFrom === rules.actFromDefault && sent.maxPerHour === rules.hourlyDefault,
      JSON.stringify({ madeClosed, ...made }));
    ok("closing the drawer after the first save hands the keyboard to the part's own 「새 결정」", made.focus, JSON.stringify({ focus: made.focus }));

    const placed = await page.evaluate(async () => {
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      view.scrollTop = 0;
      const shown = (node) => Boolean(node) && node.getClientRects().length > 0;
      const part = view.querySelector("[data-jev-custom]");
      const charts = view.querySelector("[data-jev-charts]");
      const table = view.querySelector(".jev-table-wrap");
      const box = view.getBoundingClientRect();
      const bottom = Math.min(box.bottom, window.innerHeight);
      const chartBox = charts.getBoundingClientRect();
      const partBox = part?.getBoundingClientRect() ?? null;
      return {
        part: shown(part), guide: shown(view.querySelector("[data-jev-custom-guide]")),
        afterCharts: Boolean(part) && Boolean(charts.compareDocumentPosition(part) & Node.DOCUMENT_POSITION_FOLLOWING)
          && partBox.top >= chartBox.bottom - 1,
        beforeTable: Boolean(part) && Boolean(part.compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING)
          && partBox.bottom <= table.getBoundingClientRect().top + 1,
        charts: chartBox.height > 0 && chartBox.top >= box.top - 1 && chartBox.bottom <= bottom + 1,
        count: part?.querySelector("[data-jev-custom-count]")?.textContent.trim() ?? "",
      };
    });
    ok("with a decision the part stands between the charts and the built-in table, the charts still on the first screen and no second 「새 결정」 on the switch line",
      placed.part && !placed.guide && placed.afterCharts && placed.beforeTable && placed.charts
        && placed.count.includes(String(rules.decisionsMax)) && placed.count.includes(String(rules.hourlyAll)),
      JSON.stringify(placed));

    await press("#jev-view [data-jev-custom-new]");
    await drawerOpen();
    const secret = { name: "명령 점검", question: "명령이 저장소 밖을 건드리나요?\nAuthorization: Bearer abc\n답은 예나 아니오로", yes: "건드린다", no: "건드리지 않는다" };
    const secretChecked = await fill(secret);
    const refused = await page.evaluate(() => {
      const form = el("jev-custom-form");
      const said = [...(form?.querySelectorAll("[data-jev-custom-refusal]") ?? [])];
      return {
        words: said.map((one) => one.dataset.jevCustomRefusal), text: said.map((one) => one.textContent.trim()),
        marked: form?.querySelector('[name="question"]')?.getAttribute("aria-invalid") ?? null,
        save: el("jev-custom-save")?.disabled ?? null, saves: window.__JEV__.custom.saves.length,
      };
    });
    await press("#jev-custom-save");
    await settlePaint(page);
    const refusedSaves = await page.evaluate(() => window.__JEV__.custom.saves.length);
    ok("a secret line is named by its field and number, the field marked and 「저장」 off — and a press on it asks the store nothing",
      secretChecked && refused.words.join() === "secret_line" && refused.text[0]?.includes("질문") && refused.text[0]?.includes("2번째 줄")
        && refused.marked === "true" && refused.save === true && refused.saves === 1 && refusedSaves === 1,
      JSON.stringify({ secretChecked, ...refused, refusedSaves }));
    await picture("drawer-refused", true);
    const cleared = await fill({ question: "명령이 저장소 밖을 건드리나요?\n답은 예나 아니오로" });
    const clean = await page.evaluate(() => ({
      refusals: el("jev-custom-form")?.querySelectorAll("[data-jev-custom-refusal]").length ?? null,
      marked: el("jev-custom-form")?.querySelector('[name="question"]')?.getAttribute("aria-invalid") ?? null,
      save: el("jev-custom-save")?.disabled ?? null,
    }));
    ok("once the line is gone the next check clears the refusal and turns 「저장」 back on",
      cleared && clean.refusals === 0 && clean.marked !== "true" && clean.save === false, JSON.stringify({ cleared, ...clean }));
    await press("#jev-custom-save");
    await drawerClosed();
    await waitFor(() => window.__JEV__.custom.store.decisions.length === 2);

    const firstId = await rowOf(0);
    const secondId = await rowOf(1);
    const applying = rules.modes.find((choice) => choice.applies).mode;
    await press(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-mode] [data-mode="${applying}"]`);
    await waitFor((id) => window.__JEV__.custom.modes.length === 1
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"] [role="radio"][aria-checked="true"]`)?.dataset.mode
        === window.__JEV__.custom.modes[0].mode, firstId);
    const switched = await page.evaluate((id) => ({
      calls: window.__JEV__.custom.modes.slice(),
      checked: document.querySelector(`#jev-view [data-jev-custom-row="${id}"] [role="radio"][aria-checked="true"]`)?.textContent.trim() ?? null,
    }), firstId);
    ok("a press on 「켜짐」 switches that one decision through the store, and its line says so",
      switched.calls.length === 1 && switched.calls[0].id === firstId && switched.calls[0].mode === applying && switched.checked === "켜짐",
      JSON.stringify(switched));
    await picture("list", true, "[data-jev-custom]");

    const keptBefore = await page.evaluate(() => window.__JEV__.custom.store.decisions[0]?.version ?? "");
    await press(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-edit]`);
    await drawerOpen();
    await checked();
    const editing = await page.evaluate(() => {
      const form = el("jev-custom-form");
      const notice = form?.querySelector("[data-jev-custom-version]");
      return {
        title: el("jev-custom-title")?.textContent.trim() ?? "", name: form?.querySelector('[name="name"]')?.value ?? null,
        question: form?.querySelector('[name="question"]')?.value ?? null,
        notice: Boolean(notice) && notice.getClientRects().length > 0, remove: el("jev-custom-delete")?.hidden === false,
      };
    });
    const versioned = await fill({ question: "이번 턴의 변경이 사용자에게 보이는 동작이나 문서를 바꿨나요?" });
    const notice = await page.evaluate(() => {
      const node = el("jev-custom-form")?.querySelector("[data-jev-custom-version]");
      return { shown: Boolean(node) && node.getClientRects().length > 0, text: node?.textContent.trim() ?? "" };
    });
    ok("「편집」 opens the decision as written, and a change says saving makes a new version with earlier records kept apart",
      editing.title === "결정 고치기" && editing.name === first.name && editing.question === first.question && !editing.notice
        && editing.remove && versioned && notice.shown && notice.text.includes("새 판"),
      JSON.stringify({ editing, versioned, notice }));
    await picture("drawer-version");
    await press("#jev-custom-save");
    await drawerClosed();
    await waitFor(() => window.__JEV__.custom.saves.length === 3);
    const kept = await page.evaluate((id) => {
      const custom = window.__JEV__.custom;
      const decision = custom.store.decisions.find((one) => one.draft.id === id);
      return {
        sentId: custom.saves.at(-1)?.id ?? null, version: decision?.version ?? "", mode: decision?.mode ?? null,
        row: document.querySelector(`#jev-view [data-jev-custom-row="${id}"] [role="radio"][aria-checked="true"]`)?.dataset.mode ?? null,
      };
    }, firstId);
    ok("saving the edit keeps the same decision under a new version and leaves its mode where it was",
      kept.sentId === firstId && kept.version !== "" && kept.version !== keptBefore && kept.mode === applying && kept.row === applying,
      JSON.stringify({ ...kept, keptBefore }));

    const builtIn = await page.evaluate(async () => {
      const view = document.querySelector("#jev-view");
      const feature = window.__JEV__.custom.rules.feature;
      const week = () => Number(view.querySelector('[data-jev-stat="week"] dd').textContent.replace(/\D/g, ""));
      const before = week();
      const answered = window.__ANSWER__.jev_summary;
      window.__ANSWER__.jev_summary = (args) => {
        const reading = answered(args);
        const seat = reading.seats.find((one) => one.id === feature);
        if (seat) seat.week = { ...seat.week, rows: 7, answered: 7, requests: 7, called: 7 };
        return reading;
      };
      await refreshJevDashboard({ force: true });
      await window.__PAINTED__();
      const after = week();
      window.__ANSWER__.jev_summary = answered;
      const unfolded = !jevUnusedOpen;
      if (unfolded) {
        view.querySelector("[data-jev-fold] button").click();
        await window.__PAINTED__();
      }
      const rows = [...view.querySelectorAll("[data-jev-dash-row]")].map((row) => row.dataset.jevDashRow);
      if (unfolded) {
        view.querySelector("[data-jev-fold] button").click();
        await window.__PAINTED__();
      }
      view.querySelector('[data-jev-dash-row="summon"] .jev-row-open').click();
      await window.__PAINTED__();
      const drawer = view.querySelector(".jev-drawer");
      return {
        feature, before, after, rows: rows.length, listed: rows.includes(feature), selects: view.querySelectorAll(".jev-table select").length,
        open: !drawer.hidden, fields: drawer.querySelectorAll("input, textarea, select").length,
        fixed: drawer.querySelector("[data-jev-drawer-fixed]")?.textContent.trim() ?? "",
      };
    });
    ok("the built-in table excludes personal decisions, counts their requests, and keeps its detail drawer informational",
      !builtIn.listed && builtIn.rows === BUILT_IN && builtIn.selects === 0 && builtIn.after === builtIn.before + 7
        && builtIn.open && builtIn.fields === 0 && builtIn.fixed.length > 0,
      JSON.stringify(builtIn));
    await picture("builtin-drawer");
    await page.evaluate(async () => {
      document.querySelector("#jev-view .jev-drawer-close").click();
      await refreshJevDashboard({ force: true });
      await window.__PAINTED__();
    });

    await press(`#jev-view [data-jev-custom-row="${secondId}"] [data-jev-custom-delete]`);
    await waitFor(() => el("jev-custom-delete-scrim")?.hidden === false);
    const asking = await page.evaluate(() => ({
      open: el("jev-custom-delete-scrim")?.hidden === false, top: window.__MODAL_STACK__().at(-1) ?? null,
      title: el("jev-custom-delete-title")?.textContent ?? "", body: el("jev-custom-delete-body")?.textContent ?? "",
      focus: document.activeElement !== null && document.activeElement === el("jev-custom-delete-cancel"),
    }));
    ok("「삭제」 asks once, naming the decision, that its records stay, and for how many days it can come back",
      asking.open && asking.top === "jev-custom-delete-scrim" && asking.title.includes(secret.name)
        && asking.body.includes(String(rules.trashKeptDays)) && asking.focus,
      JSON.stringify(asking));
    await picture("delete-confirm");
    await press("#jev-custom-delete-ok");
    await waitFor((id) => window.__JEV__.custom.deletes.length === 1
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"]`) === null
      && document.querySelector(`.toast[data-jev-custom-undo="${id}"] .toast-action`) !== null, secondId);
    const deleted = await page.evaluate((id) => ({
      deletes: window.__JEV__.custom.deletes.slice(), row: document.querySelector(`#jev-view [data-jev-custom-row="${id}"]`) !== null,
      confirm: el("jev-custom-delete-scrim")?.hidden === false,
      undo: document.querySelector(`.toast[data-jev-custom-undo="${id}"] .toast-action`)?.textContent.trim() ?? null,
    }), secondId);
    ok("「지우기」 deletes it through the store, and the toast offers 「되돌리기」",
      deleted.deletes.join() === secondId && !deleted.row && !deleted.confirm && deleted.undo === "되돌리기", JSON.stringify(deleted));
    await picture("undo");
    await press(`.toast[data-jev-custom-undo="${secondId}"] .toast-action`);
    const undone = await waitFor((id) => window.__JEV__.custom.restores.length === 1
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"]`) !== null
      && document.querySelector(`.toast[data-jev-custom-undo="${id}"]`) === null, secondId);
    ok("「되돌리기」 brings it back through the store and takes the toast away",
      undone, JSON.stringify(await page.evaluate(() => window.__JEV__.custom.restores)));

    await press(`#jev-view [data-jev-custom-row="${secondId}"] [data-jev-custom-delete]`);
    await waitFor(() => el("jev-custom-delete-scrim")?.hidden === false);
    const held = await page.evaluate(async (id) => {
      const started = performance.now();
      el("jev-custom-delete-ok")?.click();
      const undoMs = typeof JEV_CUSTOM_UNDO_MS === "number" ? JEV_CUSTOM_UNDO_MS : 0;
      await new Promise((done) => setTimeout(done, TOAST_MS + 250));
      const standing = document.querySelector(`.toast[data-jev-custom-undo="${id}"]`) !== null;
      await new Promise((done) => setTimeout(done, Math.max(0, undoMs + 400 - (performance.now() - started))));
      return { standing, gone: document.querySelector(`.toast[data-jev-custom-undo="${id}"]`) === null, undoMs, toastMs: TOAST_MS };
    }, secondId);
    ok("the undo stands past a plain toast's time and leaves at the design's eight seconds",
      held.standing && held.gone && held.undoMs === 8_000 && held.undoMs > held.toastMs, JSON.stringify(held));
    const trash = await page.evaluate((id) => {
      const fold = document.querySelector("#jev-view [data-jev-custom-trash]");
      return {
        shown: Boolean(fold) && fold.getClientRects().length > 0,
        items: [...(fold?.querySelectorAll("[data-jev-custom-restore]") ?? [])].map((one) => one.dataset.jevCustomRestore),
        row: document.querySelector(`#jev-view [data-jev-custom-row="${id}"]`) !== null,
      };
    }, secondId);
    ok("with the undo gone the deleted decision waits in a fold under the list", trash.shown && trash.items.join() === secondId && !trash.row,
      JSON.stringify(trash));
    await page.evaluate(() => {
      const fold = document.querySelector("#jev-view [data-jev-custom-trash]");
      if (fold) fold.open = true;
    });
    await press(`#jev-view [data-jev-custom-restore="${secondId}"]`);
    const restored = await waitFor((id) => window.__JEV__.custom.restores.length === 2
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"]`) !== null
      && (document.querySelector("#jev-view [data-jev-custom-trash]")?.getClientRects().length ?? 0) === 0, secondId);
    ok("「되살리기」 in the fold brings it back through the store", restored,
      JSON.stringify(await page.evaluate(() => window.__JEV__.custom.restores)));

    const full = await page.evaluate(async () => {
      const custom = window.__JEV__.custom;
      while (custom.store.decisions.length < custom.rules.decisionsMax) {
        const copy = structuredClone(custom.store.decisions[0]);
        custom.made += 1;
        copy.draft.id = `cd-${custom.made.toString(16).padStart(8, "0")}`;
        copy.draft.name = `${copy.draft.name} ${custom.made}`;
        copy.createdAt = new Date().toISOString();
        custom.store.decisions.push(copy);
      }
      await refreshJevDashboard({ force: true });
      await window.__PAINTED__();
      const view = document.querySelector("#jev-view");
      const button = view.querySelector("[data-jev-custom-new]");
      const guide = view.querySelector("[data-jev-custom-guide]");
      const seen = {
        disabled: button?.disabled ?? null, tip: button?.dataset.tip ?? "",
        count: view.querySelector("[data-jev-custom-count]")?.textContent.trim() ?? "",
        guide: Boolean(guide) && guide.getClientRects().length > 0, rows: view.querySelectorAll("[data-jev-custom-row]").length,
      };
      custom.store.decisions.splice(2);
      await refreshJevDashboard({ force: true });
      await window.__PAINTED__();
      seen.after = view.querySelector("[data-jev-custom-new]")?.disabled ?? null;
      return seen;
    });
    ok("at the store's limit 「새 결정」 turns off and says the limit, the settings answer's own",
      full.disabled === true && full.tip.includes(String(rules.decisionsMax)) && full.count.includes(String(rules.decisionsMax))
        && !full.guide && full.rows === rules.decisionsMax && full.after === false,
      JSON.stringify(full));

    const focusIs = (selector) => page.evaluate((target) => document.activeElement !== null
      && document.activeElement === document.querySelector(target), selector);
    const keys = {};
    await page.focus("#jev-view [data-jev-custom-new]").catch(() => {});
    await page.keyboard.press("Enter");
    keys.opened = await drawerOpen();
    await checked();
    keys.name = await focusIs('#jev-custom-form [name="name"]');
    const stops = () => page.evaluate(() => {
      const found = typeof modalTabStops === "function" && el("jev-custom-scrim")
        ? modalTabStops({ root: el("jev-custom-scrim") }) : [];
      return { first: found.indexOf(document.activeElement) === 0, last: found.length > 1 && found.indexOf(document.activeElement) === found.length - 1 };
    });
    await page.keyboard.press("Shift+Tab");
    keys.atFirst = (await stops()).first;
    await page.keyboard.press("Shift+Tab");
    keys.wrappedToLast = (await stops()).last;
    await page.keyboard.press("Tab");
    keys.wrappedToFirst = (await stops()).first;
    await page.keyboard.press("Escape");
    keys.escaped = await drawerClosed();
    keys.back = await focusIs("#jev-view [data-jev-custom-new]");
    await page.keyboard.press("Enter");
    await drawerOpen();
    await checked();
    await page.keyboard.type("초안");
    await checked();
    await page.keyboard.press("Escape");
    keys.unsaved = await page.evaluate(() => el("jev-custom-scrim")?.hidden === false
      && el("jev-custom-unsaved")?.hidden === false && el("jev-custom-unsaved").textContent.trim().length > 0);
    await page.keyboard.press("Escape");
    keys.discarded = await drawerClosed();
    keys.backAgain = await focusIs("#jev-view [data-jev-custom-new]");
    ok("the drawer holds the keyboard: Tab wraps both ways, Esc closes it and hands focus back, and unsaved words take a second Esc",
      Object.values(keys).every(Boolean), JSON.stringify(keys));

    const confirm = {};
    await page.focus(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-edit]`).catch(() => {});
    await page.keyboard.press("Enter");
    confirm.opened = await drawerOpen();
    await checked();
    await press("#jev-custom-delete");
    confirm.asked = await waitFor(() => el("jev-custom-delete-scrim")?.hidden === false);
    confirm.cancel = await focusIs("#jev-custom-delete-cancel");
    await page.keyboard.press("Tab");
    confirm.wrapped = await focusIs("#jev-custom-delete-ok");
    await page.keyboard.press("Shift+Tab");
    confirm.wrappedBack = await focusIs("#jev-custom-delete-cancel");
    await page.keyboard.press("Escape");
    confirm.closed = await waitFor(() => el("jev-custom-delete-scrim")?.hidden !== false);
    confirm.drawer = await page.evaluate(() => el("jev-custom-scrim")?.hidden === false && window.__MODAL_STACK__().at(-1) === "jev-custom-scrim");
    confirm.back = await focusIs("#jev-custom-delete");
    confirm.nothingDeleted = await page.evaluate(() => window.__JEV__.custom.deletes.length === 2);
    await page.keyboard.press("Escape");
    confirm.drawerClosed = await drawerClosed();
    confirm.edit = await focusIs(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-edit]`);
    ok("the delete question over the drawer holds the keyboard too, and Esc closes only it, handing focus back to the drawer's 「삭제」",
      Object.values(confirm).every(Boolean), JSON.stringify(confirm));

    const radios = {};
    const recording = rules.modes.find((choice) => choice.asks && !choice.applies).mode;
    const checkedRadio = `#jev-view [data-jev-custom-row="${firstId}"] [role="radio"][aria-checked="true"]`;
    await page.focus(checkedRadio).catch(() => {});
    const modeCalls = await page.evaluate(() => window.__JEV__.custom.modes.length);
    await page.keyboard.press("ArrowLeft");
    radios.left = await waitFor(({ id, mode, before }) => window.__JEV__.custom.modes.length === before + 1
      && window.__JEV__.custom.modes.at(-1).mode === mode
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"] [role="radio"][aria-checked="true"]`)?.dataset.mode === mode,
    { id: firstId, mode: recording, before: modeCalls });
    radios.focused = await page.evaluate((selector) => document.activeElement === document.querySelector(selector), checkedRadio);
    await page.keyboard.press("ArrowRight");
    radios.right = await waitFor(({ id, mode, before }) => window.__JEV__.custom.modes.length === before + 2
      && document.querySelector(`#jev-view [data-jev-custom-row="${id}"] [role="radio"][aria-checked="true"]`)?.dataset.mode === mode,
    { id: firstId, mode: applying, before: modeCalls });
    ok("the arrow keys move a decision's mode one step and keep the keyboard on the mode now chosen",
      Object.values(radios).every(Boolean), JSON.stringify(radios));

    const english = await page.evaluate(async () => {
      setLocale("en");
      await window.__PAINTED__();
      const part = document.querySelector("#jev-view [data-jev-custom]");
      let words = part?.innerText ?? "";
      for (const kept of window.__JEV__.custom.store.decisions) words = words.split(kept.draft.name).join(" ");
      const said = { korean: (words.match(/[가-힣]+/g) ?? []).slice(0, 8), length: words.trim().length };
      setLocale("ko");
      await window.__PAINTED__();
      return said;
    });
    ok("the part speaks English with no Korean of its own left", english.length > 0 && english.korean.length === 0, JSON.stringify(english));

    await press(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-edit]`);
    await drawerOpen(); await checked();
    await page.locator('#jev-custom-form [name="kind"][value="choice"]').check();
    await checked();
    const options = page.locator('#jev-custom-form [data-jev-custom-option]');
    await options.nth(0).locator('[name="option_name"]').fill("수정");
    await options.nth(0).locator('[name="meaning"]').fill("코드 변경 필요");
    await options.nth(1).locator('[name="option_name"]').fill("조사");
    await options.nth(1).locator('[name="meaning"]').fill("원인 파악 필요");
    const choicePreview = page.locator('#jev-custom-form [data-jev-input-preview]');
    ok("custom Choice offers an input preview apart from the sent payload", await choicePreview.count() === 1);
    if (await choicePreview.count()) {
      await choicePreview.locator("summary").click();
      await page.waitForFunction(() => document.querySelector('#jev-custom-form [data-jev-input-preview]')?.textContent.includes("코드 변경 필요"));
      ok("custom Choice preview contains the draft option and criterion", (await choicePreview.textContent()).includes("수정") && (await choicePreview.textContent()).includes("코드 변경 필요"));
    }
    await page.locator('[data-jev-custom-add="choice"]').click();
    ok("custom Choice can add and remove an option", await options.count() === 3);
    await options.nth(2).locator('[data-jev-custom-remove]').click();
    await checked(); await press("#jev-custom-save"); await drawerClosed();
    let typed = await page.evaluate(() => window.__JEV__.custom.saves.at(-1));
    ok("custom Choice saves named options and their meanings", typed.answer.kind === "choice" && typed.answer.options.length === 2 && typed.answer.options[0].means === "코드 변경 필요");
    await press(`#jev-view [data-jev-custom-row="${firstId}"] [data-jev-custom-edit]`);
    await drawerOpen(); await checked();
    await page.locator('#jev-custom-form [name="kind"][value="score"]').check();
    await page.locator('#jev-custom-form [name="level"]').nth(0).fill("여러 파일");
    await page.locator('#jev-custom-form [name="level"]').nth(1).fill("한 파일");
    await page.locator('[data-jev-custom-move="up"]').nth(1).click();
    ok("moving a score level keeps the keyboard on the moved content", await page.evaluate(() => document.activeElement?.name === "level" && document.activeElement.value === "한 파일"));
    await checked(); await press("#jev-custom-save"); await drawerClosed();
    typed = await page.evaluate(() => window.__JEV__.custom.saves.at(-1));
    ok("custom Score saves the explicitly reordered levels", typed.answer.kind === "score" && typed.answer.levels.join("|") === "한 파일|여러 파일", JSON.stringify(typed));

    ok("a person's decisions on the dashboard raise no renderer fault and log no console error",
      faults.length === 0 && consoleErrors.length === 0,
      JSON.stringify({ faults: faults.slice(0, 3), consoleErrors: consoleErrors.slice(0, 3) }));
    if (folder) ok("the pictures of a person's decisions are written", pictures.length > 0, JSON.stringify(pictures));
  } finally {
    await page.close();
  }
}


export async function testJevBuiltinDecisions(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const previewPicture = async kind => {
      const folder = process.env.JEV_BUILTIN_SHOTS;
      if (!folder) return;
      await mkdir(folder, { recursive: true });
      await setQualityTheme(page, "dark");
      for (const width of kind === "score" ? [320, 768, 1440] : [768]) {
        await page.setViewportSize({ width, height: 1069 });
        await settlePaint(page);
        await page.locator('#jev-builtin-form [data-jev-input-preview]').scrollIntoViewIfNeeded();
        await page.screenshot({ path: join(folder, `preview-${kind}-dark-${width}.png`), animations: "disabled" });
      }
      await page.setViewportSize({ width: 1280, height: 900 });
    };
    await page.evaluate(jevDashboardFixture);
    await page.locator("#nav-jev").click();
    await page.waitForSelector('#jev-view [data-jev-dash-row="recall"]');
    const actions = await page.evaluate(() => {
      const view = document.querySelector("#jev-view");
      return { rows: view.querySelectorAll("[data-jev-dash-row]").length,
        edits: view.querySelectorAll("[data-jev-builtin-edit]").length,
        deletes: view.querySelectorAll("[data-jev-builtin-delete]").length };
    });
    ok("every displayed built-in decision offers edit and delete", actions.rows > 0 && actions.edits === actions.rows && actions.deletes === actions.rows, JSON.stringify(actions));
    if (actions.edits !== actions.rows) return;
    const open = async (id = "recall") => {
      if (!await page.locator(`#jev-view [data-jev-builtin-edit="${id}"]`).count()) await page.locator("#jev-view .jev-fold").click();
      await page.locator(`#jev-view [data-jev-builtin-edit="${id}"]`).click();
      await page.waitForFunction(() => !document.getElementById("jev-builtin-name").disabled);
    };
    const save = async () => {
      await page.locator("#jev-builtin-save").click();
      await page.waitForFunction(() => document.getElementById("jev-builtin-scrim").hidden);
    };
    await open();
    ok("the built-in editor loads the shipped question and criteria", await page.locator('[data-jev-question-instructions="n*"]').inputValue() === "기본 판단 질문"
      && await page.locator('[data-jev-question-criterion="0"]').inputValue() === "낮음"
      && await page.locator('[data-jev-question="n*"]').innerText().then((text) => text.includes("Score")));
    await page.locator("#jev-builtin-name").fill("바꾼 위키 판단");
    await page.locator('[data-jev-question-instructions="n*"]').fill("현재 요청을 직접 뒷받침하는 내용인가?");
    await page.locator('[data-jev-question-criterion="0"]').fill("명시적인 근거가 있는 내용을 우선한다.");
    const inputPreview = page.locator('#jev-builtin-form [data-jev-input-preview]');
    ok("built-in editing offers a model-free input preview", await inputPreview.count() === 1);
    if (await inputPreview.count()) {
      const calls = await page.evaluate(() => window.__JEV__.builtin.calls.length);
      const modelCalls = await page.evaluate(() => ["check_model_connection", "select_model_connection", "preflight_judgment_provider", "check_typesafe_key"].map(command => window.__COUNTS__[command] ?? 0));
      await inputPreview.locator("summary").click();
      await page.waitForFunction(() => document.querySelector('#jev-builtin-form [data-jev-input-preview]')?.textContent.includes("명시적인 근거가 있는 내용을 우선한다."));
      ok("Score preview uses the draft question and ordered criteria", (await inputPreview.textContent()).includes("현재 요청을 직접 뒷받침하는 내용인가?") && (await inputPreview.textContent()).includes("명시적인 근거가 있는 내용을 우선한다."));
      ok("preview neither saves nor calls a model", await page.evaluate(() => window.__JEV__.builtin.calls.length) === calls);
      ok("input preview sends no model IPC", JSON.stringify(await page.evaluate(() => ["check_model_connection", "select_model_connection", "preflight_judgment_provider", "check_typesafe_key"].map(command => window.__COUNTS__[command] ?? 0))) === JSON.stringify(modelCalls));
      await previewPicture("score");
    }
    await save();
    let saved = await page.evaluate(() => window.__JEV__.builtin.saved.recall);
    ok("editing a built-in question saves the wording and starts recording", saved.questions["n*"].instructions === "현재 요청을 직접 뒷받침하는 내용인가?"
      && saved.questions["n*"].criteria.join("|") === "명시적인 근거가 있는 내용을 우선한다.|보통|높음|매우 높음" && saved.mode === "shadow", JSON.stringify(saved));
    await open();
    ok("reopening a built-in editor retains the saved wording", await page.locator("#jev-builtin-name").inputValue() === "바꾼 위키 판단"
      && await page.locator('[data-jev-question-instructions="n*"]').inputValue() === saved.questions["n*"].instructions);
    const shots = process.env.JEV_BUILTIN_SHOTS;
    const AxeBuilder = axeBuilder();
    for (const theme of ["dark", "light"]) {
      await setQualityTheme(page, theme);
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 860 });
        await settlePaint(page);
        const layout = await page.evaluate(() => {
          const dialog = document.querySelector("#jev-builtin-scrim [role=dialog]");
          const box = dialog.getBoundingClientRect();
          const form = document.getElementById("jev-builtin-form");
          return { fits: box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight,
            scroll: form.scrollWidth <= form.clientWidth + 1, save: !document.getElementById("jev-builtin-save").getBoundingClientRect().width };
        });
        ok(`the ${theme} built-in editor fits ${width}px with its actions visible`, layout.fits && layout.scroll && !layout.save, JSON.stringify(layout));
        if (shots) {
          await mkdir(shots, { recursive: true });
          await page.screenshot({ path: join(shots, `builtin-${theme}-${width}.png`) });
        }
      }
      if (AxeBuilder) {
        const results = await new AxeBuilder({ page }).include("#jev-builtin-scrim").analyze();
        ok(`the ${theme} built-in editor has no accessibility violations`, results.violations.length === 0, JSON.stringify(results.violations.map((v) => v.id)));
      }
    }
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.locator("#jev-builtin-cancel").click();
    await page.locator('#jev-view [data-jev-builtin-delete="recall"]').click();
    await page.locator("#jev-custom-delete-ok").click();
    await page.waitForFunction(() => !document.querySelector('#jev-view [data-jev-dash-row="recall"]'));
    ok("deleting a built-in decision removes its row and offers restoration", await page.locator('#jev-view [data-jev-builtin-restore="recall"]').count() === 1);
    const toggle = page.locator("#jev-view [data-jev-switch]");
    await toggle.uncheck(); await toggle.check();
    saved = await page.evaluate(() => window.__JEV__.builtin.saved.recall);
    ok("turning all decisions on does not resurrect a deleted built-in decision", saved.deleted && await page.locator('#jev-view [data-jev-dash-row="recall"]').count() === 0);
    await page.locator('#jev-view [data-jev-builtin-restore="recall"]').click();
    await page.waitForSelector('#jev-view [data-jev-dash-row="recall"]');
    await open();
    ok("restoring a deleted built-in decision preserves its saved question", await page.locator('[data-jev-question-instructions="n*"]').inputValue() === saved.questions["n*"].instructions);
    await page.locator("#jev-builtin-reset").click();
    await save();
    await open();
    ok("loading and saving defaults restores the shipped question", await page.locator('[data-jev-question-instructions="n*"]').inputValue() === "기본 판단 질문"
      && await page.evaluate(() => !window.__JEV__.builtin.saved.recall));
    await page.locator("#jev-builtin-cancel").click();
    await open("command_guard");
    await page.locator('#jev-builtin-form [data-jev-input-preview] > summary').click();
    await page.waitForSelector('[data-preview-type="noul"]');
    ok("Noul preview separates both criteria", (await page.locator('[data-jev-input-preview]').textContent()).includes("원본을 잃는다"));
    await previewPicture("noul");
    const irreversible = page.locator('[data-jev-question="irreversible"]');
    ok("both command-guard Nouls expose separate yes and no meanings", await page.locator("[data-jev-question]").count() === 2
      && await irreversible.locator('[data-jev-question-criterion="true"]').inputValue() === "원본을 잃는다"
      && (await irreversible.innerText()).includes("Noul"));
    await irreversible.locator('[data-jev-question-criterion="true"]').fill("복원할 방법이 없는 경우");
    await save();
    const noul = await page.evaluate(() => window.__JEV__.builtin.saved.command_guard);
    ok("editing one Noul criterion leaves its neighboring question on defaults", noul.questions.irreversible.criteria.true === "복원할 방법이 없는 경우"
      && !Object.hasOwn(noul.questions, "outside") && noul.question === null && noul.criteria === null);
    await open("placement");
    await page.locator('#jev-builtin-form [data-jev-input-preview] > summary').click();
    await page.waitForSelector('[data-preview-type="choice"]');
    ok("Choice preview preserves option keys and criteria", (await page.locator('[data-jev-input-preview]').textContent()).includes("tab"));
    await previewPicture("choice");
    const choice = page.locator('[data-jev-question="placement"]');
    const tab = choice.locator('[data-jev-question-criterion="tab"]');
    ok("Choice shows its keys and preserves a structured option description", (await choice.innerText()).includes("Choice")
      && JSON.parse(await tab.inputValue()).what === "새 탭");
    await tab.fill('{"what":"함께 보기","examples":["비교"]}');
    await save();
    const chosen = await page.evaluate(() => window.__JEV__.builtin.saved.placement.questions.placement);
    ok("a structured Choice description stays JSON in the saved draft", chosen.type === "choice"
      && chosen.criteria.tab.what === "함께 보기" && chosen.criteria.tab.examples[0] === "비교" && !Object.hasOwn(chosen.criteria, "split"));
    await open("placement");
    await tab.fill("{");
    ok("invalid structured content cannot be saved", await page.locator("#jev-builtin-save").isDisabled());
    await page.locator("#jev-builtin-reset").click();
    await save();
    await page.evaluate(() => { window.__JEV__.builtin.saved.recall = { question: "이전 공통 지침", criteria: "이전 공통 기준" }; });
    await open();
    ok("legacy common instructions stay visible and are not reinterpreted as typed questions", await page.locator("#jev-builtin-question").inputValue() === "이전 공통 지침"
      && await page.locator('[data-jev-question-instructions="n*"]').inputValue() === "기본 판단 질문");
    ok("the legacy prompt keeps its old bound while typed instructions fit the shipped wording", await page.locator("#jev-builtin-question").getAttribute("maxlength") === "2000"
      && await page.locator('[data-jev-question-instructions="n*"]').getAttribute("maxlength") === "4000");
    await page.locator("#jev-builtin-reset").click();
    await save();
    await open();
    await page.locator("#jev-builtin-name").fill("저장 실패 초안");
    await page.evaluate(() => { window.__JEV__.builtin.failNext = true; });
    await page.locator("#jev-builtin-save").click();
    await page.waitForSelector("#jev-builtin-status.is-error");
    ok("a failed built-in save preserves the open draft and persisted state", await page.locator("#jev-builtin-name").inputValue() === "저장 실패 초안"
      && await page.evaluate(() => !window.__JEV__.builtin.saved.recall));
    await page.evaluate(() => { window.__JEV__.builtin.saved.recall = { name: "다른 창의 수정" }; });
    await page.locator("#jev-builtin-save").click();
    await page.waitForFunction(() => document.getElementById("jev-builtin-status").textContent.includes("다른 창"));
    ok("a stale built-in editor cannot overwrite another window", await page.evaluate(() => window.__JEV__.builtin.saved.recall.name === "다른 창의 수정"));
    ok("built-in decision editing raises no JavaScript errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
