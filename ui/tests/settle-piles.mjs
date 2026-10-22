/* 남은 일의 무더기 (t-71220) — 사람 10-22 사진: 아티팩트 흐름의 「지금 · 36」에 몇 주 된 과업이
 * 「차례 기다림」·「검토 대기」로 남아 끝나지 않았다. 아무도 돌아오지 않는 일은 원장이 고른다(core
 * `settle::left_behind` — 데스크의 `task.left`·`left`, 흐름의 `left`·`unreviewed`). 창이 그것을 「지금」
 * 밖의 접힌 줄 둘로 세우는가, 그 줄의 단추가 창의 사람 문(`board_settle`) 하나로만 가는가를 합성 장면으로
 * 잰다. 장면의 수는 사람의 work 프로젝트를 원장 사본에서 센 그대로다: 도는 일 3, 정리 필요 17(닫힐 일
 * 16 · 열린 채 남을 일 1), 검토 없이 끝남 16.
 *
 * - 흐름: 「지금」의 수가 지금의 일만 세고, 무더기 둘이 접혀 서며, 펼치면 오래된 것부터 과업 · 런 · 나이가
 *   선다. 누르면 확인 창이 몇 건과 처음 세 줄을 보이고, 「예」일 때만 `board_settle`이 원장이 고른 이름
 *   전부로 간다. 답의 한 줄과 되돌리기가 서고, 줄은 원장이 적은 뒤(다음 답)에만 빠지며, 되돌리기는 닫았다고
 *   답한 과업을 그 상태 그대로 보낸다. 화면 규칙(12 px 글자 · 28 px 단추)과 브라우저 오류 0.
 * - 상황판: 「지금 도는 일」과 「당신 차례」가 남은 일을 빼고 맨 위 「답할 일」의 수도 뺀다. 남은 일 블록이
 *   두 무더기를 같은 낱말 · 같은 단추로 세운다.
 *
 * `SETTLE_PILES_SHOTS=<folder>` keeps pictures of the flow and the board in both themes, named after
 * `SETTLE_PILES_LABEL`; `SETTLE_PILES_OLD=1` stands the answers the runtime gave before the piles (every line
 * in 「지금」, no `left`), the person's picture of 10-22 — the before of the report.
 *
 *   node ui/tests/settle-piles.mjs */
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { REAL_WINDOW, standCatalog } from "./artifact-cards.mjs";
import { standTasks } from "./artifact-tasks.mjs";
import { openFlow, flowScreenRules } from "./artifact-flow.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { draftScene } from "./board-now.mjs";
import { answerAsk } from "./land-batch.mjs";

/* The work project's piles as the ledger copy counted them on 10-22 (reports/t-71220). */
const NOW_LINES = 3;
const SETTLE_LINES = 16;
const LOOK_LINES = 1;
const UNREVIEWED_LINES = 16;
/* How many rows the confirm names before it says the rest as a number (`SETTLE_ASK_ROWS`). */
const ASK_ROWS = 3;
/* How long a step waits for the window: a frame or two of layout and the fake runtime's answer. */
const SETTLED_MS = 300;
/* Pictures for the report, and the answers of before the piles (see the head). */
const SHOTS = process.env.SETTLE_PILES_SHOTS ?? "";
const LABEL = process.env.SETTLE_PILES_LABEL ?? "after";
const OLD = process.env.SETTLE_PILES_OLD === "1";

/* The flow the runtime answers: three lines of now, and the two piles with every task a press names. The
 * fake `board_settle` answers as the ledger does — what it closed, waived or put back — and writes nothing
 * on its own: the scene's piles move only when the test says the ledger wrote. A page function. */
function standLeftFlow(counts) {
  const now = Date.now();
  const day = 86_400_000;
  const chips = { report: null, page: null, pairs: 0, singles: 0, sessions: 0, attempts: 0,
    parts: { reports: 0, pages: 0, files: 0, pictures: 0, logs: 0, other: 0, missing: 0 } };
  const line = (task, said, stage, left, run, days) => ({
    task, run, title: said, said, said_from: "title", gain: null, stage, ledger_stage: stage, left, three: null,
    summary: null, opened_ms: now - days * day, last_ms: now - days * day, attempts: [], hand_in: null, review: null,
    land: null, ship: null, chips,
  });
  const current = [...Array(counts.now).keys()].map((n) =>
    line(`t-7034${n}`, `코디가 이번 주에 적은 일 ${n + 1}을 기다린다`, "ready", null, "run-53371", n + 1));
  const settle = [...Array(counts.settle).keys()].map((n) =>
    line(`t-212${String(n).padStart(2, "0")}`, `몇 주 전에 적고 아무도 시작하지 않은 일 ${n + 1}`, "ready", "settle", "run-18477", 30 - n));
  const look = [...Array(counts.look).keys()].map((n) =>
    line(`t-574${n}`, `넘긴 것이 있는데 다시 열린 일 ${n + 1}`, "ready", "look", "run-53371", 9));
  const unreviewed = [...Array(counts.unreviewed).keys()].map((n) =>
    line(`t-191${String(n).padStart(2, "0")}`, `넘겼지만 아무도 검토하지 않은 일 ${n + 1}`, "reported", "unreviewed", "run-17424", 21 - n));
  const named = (lines) => lines.map((one) => ({ run: one.run, task: one.task }));
  const pile = (lines, closes) => ({ tasks: lines, more: 0, named: named(lines.filter((one) => one.left === closes)) });
  const byAge = (lines) => [...lines].sort((a, b) => a.last_ms - b.last_ms);
  const flow = {
    root: activeProjectPath, base: null, git: "outside", ledger: true,
    now: { tasks: current, more: 0, records: [] }, next: { tasks: [], more: 0, records: [] }, releases: [], older: null,
    left: pile(byAge([...settle, ...look]), "settle"), unreviewed: pile(byAge(unreviewed), "unreviewed"),
    press_max: 256, quiet_days: 7, sources: { said: {}, land: {}, ship: {} },
  };
  if (counts.old) {
    // Before the piles every one of them stood in 「지금」, newest first, and nothing said it was left.
    flow.now.tasks = [...current, ...settle, ...look, ...unreviewed].map((one) => ({ ...one, left: null }))
      .sort((a, b) => b.last_ms - a.last_ms);
    for (const key of ["left", "unreviewed", "press_max", "quiet_days"]) delete flow[key];
  }
  const seen = (window.__FLOW__ = { asks: [], flow });
  window.__ANSWER__.artifact_flow = (args) => {
    seen.asks.push(JSON.parse(JSON.stringify(args?.ask ?? null)));
    return JSON.parse(JSON.stringify(seen.flow));
  };
  window.__SETTLE__ = [];
  window.__ANSWER__.board_settle = (args) => {
    window.__SETTLE__.push(JSON.parse(JSON.stringify(args)));
    const tasks = args?.tasks ?? [];
    if (args?.press === "settle") return { closed: tasks.map((one) => ({ ...one, was: "ready" })), waived: [], reopened: [], kept: [] };
    if (args?.press === "waive") return { closed: [], waived: tasks, reopened: [], kept: [] };
    return { closed: [], waived: [], reopened: tasks, kept: [] };
  };
}

/* The ledger wrote what a press asked: the named tasks leave their pile, and the window hears the ledger
 * move. A page function. */
async function ledgerWrote(settled) {
  const flow = window.__FLOW__.flow;
  const gone = new Set(window.__SETTLE__.flatMap((call) => (call.press === "undo" ? [] : call.tasks.map((one) => one.task))));
  for (const pile of [flow.left, flow.unreviewed]) {
    pile.tasks = pile.tasks.filter((one) => !gone.has(one.task));
    pile.named = pile.named.filter((one) => !gone.has(one.task));
  }
  for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
  await window.__UNTIL__(() => !artifactFlowAsking, "the flow's next answer");
  await new Promise((done) => setTimeout(done, settled));
}

/* What the flow draws of its now and its piles. A page function. */
function readFlowPiles() {
  const view = artifactsView();
  const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
  const now = view?.querySelector('.artifact-flow-group[data-group="now"]');
  const pile = (kind) => {
    const item = view?.querySelector(`.artifact-flow-pile[data-pile="${kind}"]`);
    if (!item) return null;
    const press = item.querySelector(".artifact-flow-pile-press");
    const said = item.querySelector(".artifact-flow-pile-said");
    const rows = [...item.querySelectorAll(".artifact-flow-item.is-pile-row")].filter(shown);
    return {
      name: item.querySelector(".artifact-flow-release-name")?.textContent ?? "",
      tally: item.querySelector(".artifact-flow-release-count")?.textContent ?? "",
      note: item.querySelector(".artifact-flow-release-digest")?.textContent ?? "",
      open: item.getAttribute("aria-expanded"),
      press: shown(press) ? press.textContent : "",
      said: shown(said) ? said.querySelector(".artifact-flow-pile-words")?.textContent ?? "" : "",
      undo: shown(item.querySelector(".artifact-flow-pile-undo")),
      rows: rows.map((row) => ({
        task: row.dataset.pileTask, who: row.querySelector(".artifact-flow-who")?.textContent ?? "",
        look: row.querySelector(".artifact-flow-pile-look")?.textContent ?? "",
      })),
    };
  };
  return {
    nowCount: now?.querySelector(".artifact-flow-group-head b")?.textContent ?? "",
    nowLines: [...(now?.querySelectorAll(".artifact-flow-item[data-task]") ?? [])].filter(shown).length,
    groups: [...(view?.querySelectorAll(".artifact-flow-group-head") ?? [])].filter(shown).length,
    settle: pile("settle"),
    unreviewed: pile("unreviewed"),
  };
}

async function settle(page) {
  await page.evaluate((ms) => new Promise((done) => setTimeout(done, ms)), SETTLED_MS);
}

/* A press on what the window drew — a missing target is not pressed, and the check after it says so with a
 * FAIL line; the run goes on to its end. */
async function pressIn(page, selector) {
  const target = page.locator(selector).first();
  if ((await target.count()) === 0) return false;
  await target.click({ timeout: 2_000 }).catch(() => {});
  return true;
}

/* One picture of `selector` in each theme, when a folder is named. */
async function shoot(page, name, selector) {
  if (!SHOTS) return;
  for (const theme of ["dark", "light"]) {
    await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
    await settle(page);
    const target = page.locator(selector).first();
    if ((await target.count()) === 0 || !(await target.isVisible())) continue;
    await target.screenshot({ path: `${SHOTS}/${LABEL}-${name}-${theme}.png` }).catch(() => {});
  }
  await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
  await settle(page);
}

async function testFlowPiles(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize(REAL_WINDOW);
    await installHarnessWaits(page);
    await standCatalog(page);
    await standTasks(page);
    await page.evaluate(standLeftFlow, { now: NOW_LINES, settle: SETTLE_LINES, look: LOOK_LINES, unreviewed: UNREVIEWED_LINES, old: OLD });
    await openFlow(page);
    await shoot(page, "flow", ".artifacts-flow");
    // The answers of before the piles are for the pictures alone: nothing in them can be pressed.
    if (OLD) return;

    const first = await page.evaluate(readFlowPiles);
    ok(`「지금」 counts the work somebody is on and nothing left behind (${NOW_LINES})`,
      first.nowCount === String(NOW_LINES) && first.nowLines === NOW_LINES, JSON.stringify(first));
    ok("the work nobody is coming back to stands in two folded piles under 「지금」, named and counted",
      first.settle?.name === "정리 필요" && first.settle?.tally === `과업 ${SETTLE_LINES + LOOK_LINES}`
        && first.unreviewed?.name === "검토 없이 끝남" && first.unreviewed?.tally === `과업 ${UNREVIEWED_LINES}`
        && first.settle.open === "false" && first.settle.rows.length === 0 && first.unreviewed.rows.length === 0,
      JSON.stringify({ settle: first.settle, unreviewed: first.unreviewed }));
    ok("each pile says what is in it, with the ledger's quiet days",
      first.settle?.note.includes("7일") && first.unreviewed?.note.includes("7일"), JSON.stringify([first.settle?.note, first.unreviewed?.note]));
    ok("each pile's one press says what it does to how many — only what a settle closes",
      first.settle?.press === `${SETTLE_LINES}건 닫기` && first.unreviewed?.press === `${UNREVIEWED_LINES}건 검토 생략`,
      JSON.stringify([first.settle?.press, first.unreviewed?.press]));
    ok("the piles are rows of 「지금」, not groups of their own — one group head stands", first.groups === 1, JSON.stringify(first));

    await pressIn(page, '[data-flow-pile="settle"]');
    await settle(page);
    await shoot(page, "flow-open", ".artifacts-flow");
    const opened = await page.evaluate(readFlowPiles);
    const rows = opened.settle?.rows ?? [];
    ok("unfolded, the pile stands every line oldest first, each with its run and age",
      rows.length === SETTLE_LINES + LOOK_LINES && rows[0]?.task === "t-21200"
        && rows.every((row) => row.who.includes("run-") && row.who.includes("전")),
      JSON.stringify(rows.slice(0, 3)));
    ok("a line with work on record says it stays open for a look",
      rows.filter((row) => row.look === "검토 필요").length === LOOK_LINES, JSON.stringify(rows.filter((row) => row.look)));

    // No: nothing is sent.
    await pressIn(page, '[data-flow-pile-press="settle"]');
    const refused = await answerAsk(page, false);
    const sentAfterNo = await page.evaluate(() => window.__SETTLE__.length);
    ok("the press asks first, and 「그대로 두기」 sends nothing", refused.shown && sentAfterNo === 0, JSON.stringify(refused));
    // Yes: the ledger's own list goes, whole.
    await pressIn(page, '[data-flow-pile-press="settle"]');
    const asked = await answerAsk(page, true);
    await settle(page);
    ok(`the confirm says how many it closes and names the first ${ASK_ROWS} with their run and age`,
      asked.title === `${SETTLE_LINES}건을 닫을까요?` && asked.body.includes("t-21200 · run-18477")
        && asked.body.includes(`그 밖에 ${SETTLE_LINES - ASK_ROWS}건`) && !asked.body.includes("t-5740"),
      JSON.stringify(asked));
    const sent = await page.evaluate(() => window.__SETTLE__.slice());
    ok("「예」 sends one press through the window's own door, naming every task the ledger said a settle closes",
      sent.length === 1 && sent[0].press === "settle" && sent[0].tasks.length === SETTLE_LINES
        && sent[0].tasks.every((one) => one.run === "run-18477" && one.task.startsWith("t-212")),
      JSON.stringify(sent.map((call) => ({ press: call.press, n: call.tasks.length, first: call.tasks[0] }))));
    const answered = await page.evaluate(readFlowPiles);
    ok("the ledger's answer stands beside the pile with an undo, and no line leaves before the ledger writes",
      answered.settle?.said === `${SETTLE_LINES}건을 닫았습니다` && answered.settle.undo
        && answered.settle.rows.length === SETTLE_LINES + LOOK_LINES,
      JSON.stringify(answered.settle));
    const rules = await page.evaluate(flowScreenRules);
    ok("the unfolded pile keeps the screen rules: text at 12 px or more, presses at 28 px or more",
      rules.small.length === 0 && rules.short.length === 0, JSON.stringify(rules));

    await page.evaluate(ledgerWrote, SETTLED_MS);
    const written = await page.evaluate(readFlowPiles);
    ok("once the ledger wrote, the closed lines are gone and the one kept for a look stays",
      written.settle?.rows.length === LOOK_LINES && written.settle.tally === `과업 ${LOOK_LINES}` && written.settle.press === ""
        && written.nowCount === String(NOW_LINES),
      JSON.stringify(written.settle));

    await pressIn(page, '[data-flow-pile-undo="settle"]');
    await settle(page);
    const undone = await page.evaluate(() => window.__SETTLE__.at(-1));
    ok("the undo sends back exactly what the settle closed, each with the status it was closed from",
      undone?.press === "undo" && undone.tasks.length === SETTLE_LINES && undone.tasks.every((one) => one.was === "ready"),
      JSON.stringify({ press: undone?.press, n: undone?.tasks?.length, first: undone?.tasks?.[0] }));

    await pressIn(page, '[data-flow-pile-press="unreviewed"]');
    const waive = await answerAsk(page, true);
    await settle(page);
    const waived = await page.evaluate(() => window.__SETTLE__.at(-1));
    ok("the finished pile's press waives the review of every task it names, after its own confirm",
      waive.title === `${UNREVIEWED_LINES}건의 검토를 생략할까요?` && waived?.press === "waive"
        && waived.tasks.length === UNREVIEWED_LINES,
      JSON.stringify({ waive, press: waived?.press, n: waived?.tasks?.length }));

    ok("the flow's piles raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The board's desk with work left behind laid over the draft's scene: three tasks a settle closes and one
 * it keeps for a look in a run nobody sits in, and two finished ones no review stands for. The stage counts
 * carry how many of each stage are left, as the desk counts them. A page function. */
function standLeftDesk(old) {
  const now = Date.now();
  const day = 86_400_000;
  const run = "run-18477";
  const desk = window.__DESK__;
  const row = (id, title, stage, left, days, extra = {}) => ({
    run, id, title, stage, parent: null, parent_title: null, gate: null, blocked_by: [], closed: null,
    created_ms: now - days * day, cost: null, writing: null, progress: null, left, touched_ms: now - days * day, ...extra,
  });
  const left = [
    row("t-21251", "옛 런의 과업 하나", "ready", "settle", 20),
    row("t-21256", "옛 런의 과업 둘", "ready", "settle", 19),
    row("t-52365", "옛 런의 과업 셋", "ready", "settle", 8),
    row("t-57345", "넘긴 뒤 다시 열린 과업", "ready", "look", 9),
    row("t-21915", "읽기 전용 검토", "reported", "unreviewed", 20, { progress: { stage: "report_only", attempt: "dp-21917", source: "abc1234" } }),
    row("t-53119", "통계 화면", "reported", "unreviewed", 8, { progress: { stage: "report_only", attempt: "dp-53121", source: "def5678" } }),
  ];
  const named = (kind) => left.filter((one) => one.left === kind).map((one) => ({ run, task: one.id }));
  if (old) {
    // Before the piles the desk sent the same rows with no word of who is coming back to them.
    for (const one of left) {
      delete one.left;
      delete one.touched_ms;
    }
    window.__DESK__ = {
      ...desk,
      revision: desk.revision + 1,
      runs: [...desk.runs, { run, name: "zo delegation", seat: false }],
      tasks: [...desk.tasks, ...left],
      stages: desk.stages.map((one) => ({ ...one, count: one.count + left.filter((task) => task.stage === one.stage).length })),
    };
    return;
  }
  window.__DESK__ = {
    ...desk,
    revision: desk.revision + 1,
    runs: [...desk.runs, { run, name: "zo delegation", seat: false }],
    tasks: [...desk.tasks, ...left],
    stages: desk.stages.map((one) => {
      const mine = left.filter((task) => task.stage === one.stage).length;
      return { ...one, count: one.count + mine, left: mine };
    }),
    left: { settle: named("settle"), look: named("look"), unreviewed: named("unreviewed"), press_max: 256, quiet_days: 7 },
  };
  window.__SETTLE__ = [];
  window.__ANSWER__.board_settle = (args) => {
    window.__SETTLE__.push(JSON.parse(JSON.stringify(args)));
    const tasks = args?.tasks ?? [];
    return args?.press === "settle"
      ? { closed: tasks.map((one) => ({ ...one, was: "ready" })), waived: [], reopened: [], kept: [] }
      : { closed: [], waived: tasks, reopened: [], kept: [] };
  };
}

/* What the board draws of the work left behind. A page function. */
function readBoardPiles() {
  const view = document.querySelector("#board-view");
  const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
  const block = view.querySelector('[data-desk-block="left"]');
  const heads = [...(block?.querySelectorAll(".board-goal.is-left") ?? [])];
  return {
    title: view.querySelector(".board-now-title")?.textContent ?? "",
    running: [...view.querySelectorAll('[data-desk-block="running"] .board-run[data-task]')].map((node) => node.dataset.task),
    asks: [...view.querySelectorAll('[data-desk-block="asks"] .board-asks > li')].map((node) => node.dataset.ask),
    block: shown(block),
    heads: heads.map((head) => ({
      goal: head.dataset.goal,
      name: head.querySelector(".board-goal-name")?.textContent ?? "",
      press: shown(head.querySelector(".board-left-press")) ? head.querySelector(".board-left-press").textContent : "",
      said: shown(head.querySelector(".board-left-said")) ? head.querySelector(".board-left-said").textContent : "",
      undo: shown(head.querySelector(".board-left-undo")),
    })),
    rows: [...(block?.querySelectorAll(".board-run.is-left") ?? [])].filter(shown).map((node) => ({
      task: node.dataset.task, state: node.querySelector(".board-run-state")?.textContent ?? "",
      end: node.querySelector(".board-run-end")?.textContent ?? "",
    })),
  };
}

async function repaintBoard(page) {
  await page.evaluate(async () => {
    refreshDeskLedger();
    await window.__UNTIL__(() => !deskLedgerAsking, "the desk's answer");
    paintCoordinatorDesk(document.querySelector("#board-view"));
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

async function testBoardPiles(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 0 });
    await draftScene(page);
    const before = await page.evaluate(readBoardPiles);
    await page.evaluate(standLeftDesk, OLD);
    await repaintBoard(page);
    if (OLD) {
      await shoot(page, "board", "#board-view .task-board-desk");
      return;
    }
    // The draft's scene lays the board over the whole window (`fullBoard`); the window's own confirm stands
    // under it, so the board goes back to its pane before anything asks.
    await page.evaluate(() => document.querySelector("#board-view").removeAttribute("style"));
    await repaintBoard(page);
    await shoot(page, "board", "#board-view .task-board-desk");
    const after = await page.evaluate(readBoardPiles);
    ok("「지금 도는 일」 leaves out the work nobody is coming back to",
      after.running.length > 0 && !after.running.some((task) => task.startsWith("run-18477/")),
      JSON.stringify(after.running));
    ok("「당신 차례」 asks no review of finished work nobody is coming back to",
      !after.asks.some((key) => key.includes("run-18477/")), JSON.stringify(after.asks));
    ok("the head's 「답할 일」 does not count it either — the desk's own per-stage count of what is left",
      before.title !== "" && after.title === before.title, JSON.stringify([before.title, after.title]));
    ok("the two piles stand under the board's now with the flow's own words and presses",
      after.block && after.heads.length === 2
        && after.heads[0].name === "정리 필요 4" && after.heads[0].press === "3건 닫기"
        && after.heads[1].name === "검토 없이 끝남 2" && after.heads[1].press === "2건 검토 생략",
      JSON.stringify(after.heads));

    await pressIn(page, '.board-goal.is-left[data-goal="board:settle"] .board-goal-fold');
    await repaintBoard(page);
    await shoot(page, "board-open", '#board-view [data-desk-block="left"]');
    const opened = await page.evaluate(readBoardPiles);
    ok("unfolded, the pile's rows are oldest first and say which stays open for a look",
      opened.rows.length === 4 && opened.rows[0].task === "run-18477/t-21251"
        && opened.rows.filter((row) => row.state === "검토 필요").length === 1 && opened.rows.every((row) => row.end.includes("전")),
      JSON.stringify(opened.rows));

    await pressIn(page, '.board-goal.is-left[data-goal="board:settle"] .board-left-press');
    const asked = await answerAsk(page, true);
    await repaintBoard(page);
    const sent = await page.evaluate(() => window.__SETTLE__.slice());
    const answered = await page.evaluate(readBoardPiles);
    ok("the board's press asks first and then names, through the same door, what a settle closes",
      asked.title === "3건을 닫을까요?" && sent.length === 1 && sent[0].press === "settle"
        && sent[0].tasks.map((one) => one.task).join(",") === "t-21251,t-21256,t-52365",
      JSON.stringify({ asked, sent }));
    ok("and says the ledger's answer with an undo beside it",
      answered.heads[0]?.said === "3건을 닫았습니다" && answered.heads[0].undo, JSON.stringify(answered.heads[0]));

    ok("the board's piles raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

export async function testSettlePiles(browser, origin, ok) {
  await testFlowPiles(browser, origin, ok);
  await testBoardPiles(browser, origin, ok);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await launchWindowBrowser();
  let failures = 0;
  try {
    await testSettlePiles(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${!pass && detail ? `  — ${detail}` : ""}`);
      if (!pass) failures += 1;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures > 0) process.exitCode = 1;
}
