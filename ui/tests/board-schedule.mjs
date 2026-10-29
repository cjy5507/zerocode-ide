/* 일정 (t-67482) — 작업 상황판의 「일정」 탭(시안 B의 네 탭 가운데 둘째, t-67607)이 빌드 줄·결합
 * 게이트·발행 레인·워커 과업을 시간축 위 제자리에 세우는가.
 *
 * 답은 러스트가 픽스처(`ui/tests/fixtures/board-schedule/`)에서 지은 그대로다: `answer.json`은
 * `schedule_timeline::tests::the_fixture_reads_into_the_answer_the_board_draws`가 값 하나까지
 * 맞추고, 워커 과업은 데스크의 답(`desk.json`, 과업 행의 `times`)이다. 그 순간은 2026-10-21
 * 09:50 KST — l61 게이트가 PAUSE 뒤에 서 있고, 그 전에 시작한 일감 둘이 돌고, v1.1.83 레인이 루트
 * 게이트에 있다. 셈(지난 소요의 중앙값)은 러스트 시험의 것이고, 여기서 재는 것은 배치와 그림이다:
 * 묶음마다 행 수, 막대의 시작과 끝, 예상 끝과 남은 시간, 기다리는 일감이 실행기 기록이 말한 줄에
 * 서는 차례와 시작을 모를 때의 말, 묶기와 펴기, 「지금」 선, 조용한 박자, 좁은 그릇, 눈금 글 사이의 틈,
 * 그리고 탭: 일정 탭만 그리고 묻고, 다른 탭으로 가면 숨고 멈추며, 워커 과업은 기록 탭의 그 과업으로 간다.
 *
 *   node ui/tests/board-schedule.mjs */
import { mkdir, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { BOARD_SCHEDULE_BUDGET } from "./board-schedule-perf.mjs";

/* 한 행이나 한 블록: 「한 주」는 가로 차트의 행(`.board-schedule-row`), 「지금 앞뒤」와 「하루」는 세로 시간표의
 * 블록(`.board-schedule-block`, t-77628)이다. 둘 다 같은 자료(`data-*`와 커스텀 속성)를 들어, 장면은 둘을 한
 * 모양으로 읽는다(`readBoardSchedule`). */

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE = resolve(HERE, "fixtures/board-schedule");
/* 2026-10-21 09:50:00 KST — the fixture's moment. */
export const SCHEDULE_NOW = 1_792_543_800_000;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

export async function boardScheduleFixture() {
  const [answer, desk] = await Promise.all(["answer.json", "desk.json"].map(async (name) =>
    JSON.parse(await readFile(resolve(FIXTURE, name), "utf8"))));
  return { answer, desk };
}

/* The window as the fixture's moment saw it, run inside the page: the board on
 * its task view, the command answering the fixture (unchanged for the revision
 * it already gave), the desk answering its tasks. Every ask is kept. */
export function installBoardSchedule({ answer, desk }) {
  window.__SCHEDULE__ = answer;
  window.__SCHEDULE_ASKED__ = [];
  window.__ANSWER__.schedule_timeline = (args = {}) => {
    window.__SCHEDULE_ASKED__.push(JSON.parse(JSON.stringify(args)));
    const held = window.__SCHEDULE__;
    const log = args.log
      ? { job: args.log.job ?? null, lane: args.log.lane ?? null, lines: ["first line", "last line"], cut: false, missing: false }
      : undefined;
    if (args.known === held.revision) {
      return { revision: held.revision, unchanged: true, read_ms: Date.now(), line: {}, jobs: [], lanes: [],
        ...(log ? { log } : {}) };
    }
    return { ...held, ...(log ? { log } : {}) };
  };
  window.__DESK__ = desk;
  window.__ANSWER__.board_desk = () => window.__DESK__;
  window.__COLUMNS__ = [];
  window.__PANES__ = [];
  window.__LEDGER__ = [];
  agentBoardMode = "tasks";
  boardNowTab = "now";
  boardScheduleChoice.zoom = "day";
  boardScheduleChoice.folded.clear();
  boardScheduleChoice.opened.clear();
  boardScheduleChoice.log = null;
  openBoard();
}

/* Choose the schedule's tab the way a person does — once the tabs stand, which is once the desk's
 * runs arrived — and wait until its rows stand. */
export async function openBoardSchedule(page) {
  await page.evaluate(async () => {
    const tab = () => document.querySelector('#board-view [data-board-tab-button="schedule"]');
    await window.__UNTIL__(() => tab()?.checkVisibility(), "the schedule's tab");
    tab().click();
    await window.__UNTIL__(() => document.querySelector("#board-view .board-schedule-row, #board-view .board-schedule-block"), "the schedule's rows", 5000);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/* What the schedule drew: per group its rows in DOM order, each with its bar's
 * place and its words; the chart's 「now」; the asks it made. */
export async function readBoardSchedule(page) {
  return page.evaluate(() => {
    const host = document.querySelector("#board-view .task-board-schedule");
    const vertical = host.dataset.scheduleZoom !== "week";
    const chart = host.querySelector(".board-schedule-chart");
    const body = host.querySelector(".board-schedule-cal-body");
    const shown = (node) => node?.checkVisibility() ?? false;
    // A row or a block, in the one shape: the words and the place the view wrote on it.
    const readRow = (row) => {
      const bar = row.querySelector(".board-schedule-bar") ?? row;
      const said = row.querySelector(".board-schedule-said");
      const label = row.querySelector(".board-schedule-label");
      const own = (name) => Number(row.style.getPropertyValue(name) || 0);
      return {
        key: row.dataset.key, state: row.dataset.barState, basis: row.dataset.basis, depth: row.dataset.depth,
        expanded: row.getAttribute("aria-expanded"), paused: row.classList.contains("is-paused"),
        late: row.classList.contains("is-late"),
        label: label.dataset.id ? `${label.dataset.id} ${label.textContent}` : label.textContent,
        // The row's sentence; before the draft's cells (t-71840) it was the words cell's own text.
        said: row.dataset.said ?? said?.textContent ?? "",
        // The passed draft's cells (t-71840): the line under the name, the end cell's two lines, the bar's two names.
        sub: label.dataset.sub ?? "",
        eta: said ? said.textContent : row.dataset.eta ?? "",
        etaSub: said ? said.dataset.sub ?? "" : row.dataset.etaSub ?? "",
        why: row.dataset.why ?? "",
        nowLabel: bar.dataset.nowLabel ?? "",
        aheadLabel: bar.dataset.aheadLabel ?? "",
        from: Number(bar.style.getPropertyValue("--from")), to: Number(bar.style.getPropertyValue("--to")),
        done: Number(bar.style.getPropertyValue("--done")),
        // The vertical timetable's block (t-77628): its sub-column, its cut top, the gate's time it inherited.
        block: row.classList.contains("board-schedule-block"),
        col: own("--col"), cols: own("--cols") || 1, mark: own("--mark"),
        cut: row.classList.contains("is-cut"), inherited: row.classList.contains("is-inherited"),
        steps: row.dataset.steps ?? "", act: row.dataset.act ?? "", rideState: row.dataset.rideState ?? "",
      };
    };
    // A gate's jobs are its block's step cells in the vertical view (`data-steps`); rows are read off the drawing only.
    const groups = {};
    for (const box of host.querySelectorAll(".board-schedule-group")) {
      const head = box.querySelector(".board-schedule-group-head");
      const drawn = new Map([...box.querySelectorAll(".board-schedule-row, .board-schedule-block")].map((row) => [row.dataset.key, readRow(row)]));
      const rows = [...drawn.values()];
      groups[box.dataset.group] = {
        hidden: box.hidden,
        expanded: head.getAttribute("aria-expanded"),
        head: head.textContent,
        headSub: head.dataset.sub ?? "",
        more: box.querySelector(".board-schedule-more").hidden ? "" : box.querySelector(".board-schedule-more").textContent,
        moreExpanded: box.querySelector(".board-schedule-more").getAttribute("aria-expanded"),
        rows,
      };
    }
    const lines = [...host.querySelectorAll(".board-schedule-line")].map((line) => {
      const [what, big, note] = line.children;
      return { id: line.dataset.line, tone: line.dataset.tone, what: what.textContent, whatSub: what.dataset.sub ?? "",
        big: big.textContent, word: big.dataset.word ?? "", note: note.textContent, act: line.dataset.act ?? "" };
    });
    return {
      groups,
      vertical,
      title: vertical
        ? host.querySelector(".board-schedule-board-now")?.textContent ?? ""
        : host.querySelector(".board-schedule-title")?.textContent ?? "",
      chip: [...host.querySelectorAll(".board-schedule-now")].find(shown)?.textContent ?? "",
      now: Number((vertical ? body : chart).style.getPropertyValue("--now")),
      hours: Number(body.style.getPropertyValue("--schedule-hours") || 0),
      ticks: [...host.querySelectorAll(".board-schedule-tick")].filter(shown).map((tick) => tick.textContent),
      note: host.querySelector(".board-schedule-note").hidden ? "" : host.querySelector(".board-schedule-note").textContent,
      board: { now: host.querySelector(".board-schedule-board-now")?.textContent ?? "",
        sum: host.querySelector(".board-schedule-board-sum")?.textContent ?? "", lines },
      aheadEmpty: shown(host.querySelector(".board-schedule-ahead-empty")),
      calLegend: host.querySelector(".board-schedule-cal-legend")?.textContent ?? "",
      elements: host.querySelectorAll("*").length,
      asked: window.__SCHEDULE_ASKED__,
    };
  });
}

/* The answer moves: a new revision whose line facts are `facts` over the last ones. The view asks
 * again, and what it drew once the moved answer stood is read. */
async function moveScheduleLine(page, facts) {
  await page.evaluate(async (facts) => {
    window.__SCHEDULE__ = { ...window.__SCHEDULE__, revision: window.__SCHEDULE__.revision + 1,
      line: { ...window.__SCHEDULE__.line, ...facts } };
    refreshBoardSchedule();
    await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
    await new Promise((done) => requestAnimationFrame(done));
  }, facts);
  return readBoardSchedule(page);
}

/* A lane of the answer changed by `patch` (found by its version), a new revision, and what the view drew
 * once the moved answer stood. */
async function patchScheduleLane(page, version, patch) {
  await page.evaluate(async ({ version, patch }) => {
    const held = window.__SCHEDULE__;
    window.__SCHEDULE__ = { ...held, revision: held.revision + 1,
      lanes: held.lanes.map((run) => (run.version === version ? { ...run, ...patch } : run)) };
    refreshBoardSchedule();
    await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
    paintBoardScheduleHosts();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, { version, patch });
  return readBoardSchedule(page);
}

/* Jobs of the answer changed by name (`[[name, patch], …]`), a new revision, and what the view drew once the
 * moved answer stood. A patch value of null clears the field. */
async function patchScheduleJobs(page, edits) {
  await page.evaluate(async (edits) => {
    const held = window.__SCHEDULE__;
    const byName = new Map(edits);
    window.__SCHEDULE__ = { ...held, revision: held.revision + 1,
      jobs: held.jobs.map((job) => (byName.has(job.name) ? { ...job, ...byName.get(job.name) } : job)) };
    refreshBoardSchedule();
    await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
    paintBoardScheduleHosts();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, edits);
  return readBoardSchedule(page);
}

/* Finished worker tasks around `now` (t-71840), each ending the one way the person's photo showed wrong —
 * no landing in the ledger — and an answer saying what git says of them: t-80001 deployed, git merged it
 * seven hours ago; t-80002 nothing to land, verified four hours ago; t-80003 failed, its attempt ended two
 * hours ago; t-80004 deployed, git names no merge but the release it shipped in, nine hours ago; t-80005
 * deployed with no end written anywhere. Fourteen merged ones from fifteen hours back fill the range, and
 * one task runs and one waits. */
export function boardScheduleEnds(now) {
  const hour = 3_600_000;
  const minute = 60_000;
  const task = (id, stage, times) => ({
    run: "run-4275", id, title: `끝 ${id}`, stage, gate: null, blocked_by: [], closed: null,
    created_ms: times.started_ms - minute, cost: null, writing: null,
    times: { handed_ms: null, verified_ms: null, landed_ms: null, ended_ms: null, release: null, ...times },
  });
  const tasks = [
    task("t-80001", "deployed", { started_ms: now - 10 * hour, handed_ms: now - 8 * hour, ended_ms: now - 8 * hour }),
    task("t-80002", "nothing_to_land", {
      started_ms: now - 6 * hour, handed_ms: now - 5 * hour, verified_ms: now - 4 * hour, ended_ms: now - 5 * hour,
    }),
    task("t-80003", "failed", { started_ms: now - 3 * hour, ended_ms: now - 2 * hour }),
    task("t-80004", "deployed", {
      started_ms: now - 12 * hour, handed_ms: now - 11 * hour, ended_ms: now - 11 * hour, release: "v1.1.83",
    }),
    task("t-80005", "deployed", { started_ms: now - 5 * hour }),
    task("t-80010", "dispatched", { started_ms: now - hour }),
    task("t-80011", "reported", { started_ms: now - 2 * hour, handed_ms: now - 30 * minute, ended_ms: now - 30 * minute }),
    ...Array.from({ length: 14 }, (_, index) => {
      const start = now - 15 * hour + index * 30 * minute;
      return task(`t-801${String(index).padStart(2, "0")}`, "merged", {
        started_ms: start, handed_ms: start + 10 * minute, verified_ms: start + 15 * minute,
        landed_ms: start + 20 * minute, ended_ms: start + 10 * minute,
      });
    }),
  ];
  const landings = {
    "t-80001": { landed_ms: now - 7 * hour, released_ms: now - 5 * hour },
    "t-80004": { landed_ms: null, released_ms: now - 9 * hour },
  };
  return { tasks, landings };
}

/* The desk and the answer move together: a new revision carrying `landings`, a desk holding `tasks`.
 * The view asks again, and what it drew once both stood is read. */
async function moveScheduleTasks(page, { tasks, landings }) {
  await page.evaluate(async ({ tasks, landings }) => {
    window.__SCHEDULE__ = { ...window.__SCHEDULE__, revision: window.__SCHEDULE__.revision + 1, landings };
    window.__DESK__ = { ...window.__DESK__, tasks };
    deskLedger = window.__DESK__;
    refreshBoardSchedule();
    await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
    paintBoardScheduleHosts();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, { tasks, landings });
  return readBoardSchedule(page);
}

/* A press on a group's folded line of finished rows, and what the schedule drew after it. */
async function pressScheduleMore(page, group) {
  await page.evaluate(async (group) => {
    document.querySelector(`#board-view [data-group="${group}"] .board-schedule-more`).click();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, group);
  return readBoardSchedule(page);
}

/* Every mutation the schedule takes while `act` (a page function) runs. */
async function scheduleMutations(page, act) {
  await page.evaluate(() => {
    const host = document.querySelector("#board-view .task-board-schedule");
    window.__SCHEDULE_RECORDS__ = [];
    window.__SCHEDULE_WATCH__ = new MutationObserver((batch) => window.__SCHEDULE_RECORDS__.push(...batch));
    window.__SCHEDULE_WATCH__.observe(host, { subtree: true, childList: true, attributes: true, characterData: true });
  });
  await page.evaluate(act);
  return page.evaluate(async () => {
    await new Promise((done) => setTimeout(done, 0));
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    window.__SCHEDULE_RECORDS__.push(...window.__SCHEDULE_WATCH__.takeRecords());
    window.__SCHEDULE_WATCH__.disconnect();
    return window.__SCHEDULE_RECORDS__.map((record) =>
      `${record.type}:${record.target.className || record.target.parentElement?.className}:${record.attributeName ?? ""}`);
  });
}

/* A percent of the day view's span, the way the view places a moment. */
function placed(ms, from, to) {
  return Math.min(100, Math.max(0, ((ms - from) / (to - from)) * 100));
}

const near = (a, b, within = 0.02) => Math.abs(a - b) <= within;

/* A release lane's row by its version — rows are named the passed draft's way, 「v1.1.83 레인」. */
const laneOf = (rows, version) => rows.find((row) => row.label.startsWith(`v${version} `));

export async function testBoardSchedule(browser, origin, ok) {
  const fixture = await boardScheduleFixture();
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));
    await page.evaluate(installBoardSchedule, fixture);
    await openBoardSchedule(page);
    let drawn = await readBoardSchedule(page);
    const { answer } = fixture;
    const from = SCHEDULE_NOW - 16 * HOUR;
    const to = SCHEDULE_NOW + 8 * HOUR;

    ok("the view asks the window's one command for a day back, holding no revision",
      drawn.asked[0]?.known === null && drawn.asked[0]?.fromMs === from, JSON.stringify(drawn.asked[0]));
    ok("「now」 stands two thirds of the way across a day view: sixteen hours behind it, eight ahead",
      near(drawn.now, placed(SCHEDULE_NOW, from, to)), String(drawn.now));

    /* ---- 탭: 상황판의 네 탭 가운데 둘째, 그 탭에서만 그리고 묻는다 ------------------------ */
    const board = () => page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      const shown = (selector) => surface.querySelector(selector)?.checkVisibility() ?? false;
      return {
        tab: surface.dataset.boardTab,
        tabs: [...surface.querySelectorAll(".board-now-tab")].map((one) =>
          [one.dataset.boardTabButton, one.getAttribute("aria-selected"), one.tabIndex, one.textContent]),
        focused: document.activeElement?.dataset.boardTabButton ?? null,
        schedule: shown(".task-board-schedule"),
        hidden: surface.querySelector(".task-board-schedule").hidden,
        intro: shown(".task-board-intro"),
        desk: shown(".task-board-desk"),
        list: shown(".task-board-filters") || shown(".task-board-sections"),
        switches: surface.querySelectorAll("[data-task-board-view]").length,
        // The beat's own test (`boardSchedulePoller`'s `wanted`): a schedule standing on screen.
        beating: boardScheduleHosts().length,
        asked: window.__SCHEDULE_ASKED__.length,
      };
    });
    const onTab = await board();
    ok("the schedule is the second of the board's four tabs, and its tab shows the schedule alone",
      onTab.tab === "schedule" && JSON.stringify(onTab.tabs) === JSON.stringify([["now", "false", -1, "지금"],
        ["schedule", "true", 0, "일정"], ["workers", "false", -1, "워커"], ["history", "false", -1, "기록"]])
        && onTab.schedule && !onTab.intro && !onTab.desk && !onTab.list && onTab.switches === 0 && onTab.beating === 1,
      JSON.stringify(onTab));
    // The person's hand on the Now tab, then the arrow key from there (the tab strip's own keys, t-67607).
    await page.evaluate(() => {
      const now = document.querySelector('#board-view [data-board-tab-button="now"]');
      now.focus();
      now.click();
    });
    const away = await board();
    ok("on another tab the schedule is hidden and its five-second beat has nothing to read",
      away.tab === "now" && away.hidden && !away.schedule && away.intro && away.beating === 0, JSON.stringify(away));
    await page.keyboard.press("ArrowRight");
    const fresh = await page.evaluate(async (since) => {
      await window.__UNTIL__(() => !document.querySelector("#board-view .task-board-schedule").hidden
        && document.querySelector("#board-view .board-schedule-row, #board-view .board-schedule-block"), "the schedule again");
      await window.__UNTIL__(() => !boardScheduleAsking, "the reopened tab's answer");
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return window.__SCHEDULE_ASKED__.slice(since).filter((one) => one.known === null);
    }, away.asked);
    const again = await board();
    ok("an arrow key from Now opens the schedule's tab, which asks once afresh for the range it shows",
      again.tab === "schedule" && again.focused === "schedule" && again.schedule && again.beating === 1
        && fresh.length === 1 && fresh[0].fromMs === from,
      JSON.stringify({ again, fresh }));

    /* ---- 발행 레인: 끝난 v1.1.82와 루트 게이트에 있는 v1.1.83 -------------------- */
    const lanes = drawn.groups.lanes.rows;
    ok("the day view holds the two lanes that ended or run after its left edge",
      lanes.map((row) => row.label).join(",") === "v1.1.82 레인,v1.1.83 레인", JSON.stringify(lanes.map((row) => row.label)));
    const running = answer.lanes.find((run) => run.running);
    const current = running.stages[running.stages.length - 1];
    const due = current.start_ms + running.ahead[0].expected.secs * 1000;
    const laneEnd = Math.max(SCHEDULE_NOW, due) + running.ahead.slice(1).reduce((sum, stage) => sum + stage.expected.secs * 1000, 0);
    const lane = laneOf(lanes, "1.1.83");
    ok("the running lane's bar ends where its stages' medians end: the root gate's from its start, then each stage after it",
      lane?.state === "running" && near(lane.to, placed(laneEnd, from, to)) && near(lane.from, placed(running.start_ms, from, to)),
      JSON.stringify({ lane, laneEnd: new Date(laneEnd).toISOString() }));
    ok("v1.1.83 is expected to end near 11:34 KST, and says the stage it is in and the time left",
      Math.abs(laneEnd - (SCHEDULE_NOW + 104 * 60_000)) <= 5 * 60_000 && lane?.said.includes("gate-root"),
      JSON.stringify({ end: new Date(laneEnd).toISOString(), said: lane?.said }));
    const ended = laneOf(lanes, "1.1.82");
    const v82 = answer.lanes.find((run) => run.version === "1.1.82");
    ok("an ended lane's bar is its start to its verdict, and says green",
      ended?.state === "done" && near(ended.from, placed(v82.start_ms, from, to)) && near(ended.to, placed(v82.end_ms, from, to)),
      JSON.stringify(ended));

    /* ---- 결합 게이트: l61은 돌고 펴져 있고, 지난 게이트는 접혀 있다 ---------------- */
    const gates = drawn.groups.gates.rows;
    const heads = gates.filter((row) => row.depth === "0");
    const l61 = heads.find((row) => row.key === "gate:l61");
    // The timetable draws a gate as one block with its step cells; its jobs are read off the model under it.
    const l61jobCount = answer.jobs.filter((job) => job.gate === "l61").length;
    ok("the gate in play is one running block whose step cells are its jobs, one each, and no job row stands under it",
      l61?.state === "running" && l61.block && l61.steps.split(",").length === l61jobCount
        && gates.every((row) => row.depth === "0" && row.block),
      JSON.stringify(gates.map((row) => [row.key, row.depth, row.state, row.steps])));
    const l60 = heads.find((row) => row.key === "gate:l60");
    ok("this morning's l60 is one passed block — its window measurement failed once and passed on its rerun",
      l60?.block && l60.state === "done" && /^통과 /.test(l60.said) && l60.act === "로그 →", JSON.stringify(l60));

    /* ---- 빌드 줄: 도는 것·기다리는 것·거절된 것, 끝난 것은 최근의 몇만 ------------- */
    const line = drawn.groups.line.rows;
    const walk = line.find((row) => row.key === "job:1792543500-run11955-t-63492-walk");
    ok("a job with no past is bounded by its cap and drawn dotted",
      walk?.state === "running" && walk.basis === "cap"
        && near(walk.to, placed(1792543520000 + 1800 * 1000, from, to)) && walk.said.includes("25분"),
      JSON.stringify(walk));
    const waiting = line.filter((row) => row.state === "waiting");
    ok("while the line is paused a waiting job says its start is not known",
      waiting.length === 2 && waiting.every((row) => row.paused && row.said.includes("멈춤")), JSON.stringify(waiting));
    ok("a refused job stands as a refusal", line.some((row) => row.key.includes("t67482-probe") && row.state === "failed"),
      JSON.stringify(line.map((row) => row.key)));
    ok("the line's finished jobs are the newest few, the rest a count",
      line.filter((row) => row.state === "done" || row.state === "failed").length <= 13 && drawn.groups.line.more !== "",
      JSON.stringify({ more: drawn.groups.line.more, rows: line.length }));
    ok("the line's paused queue is said once, in the coordinator's words", drawn.note.includes("fc4abd39"), drawn.note);

    /* ---- 워커 과업: 데스크의 과업 행이 시작부터 착지까지 -------------------------- */
    const workers = drawn.groups.workers.rows;
    ok("the day view holds the five tasks carried since its left edge, in the order they started",
      workers.map((row) => row.key).join(",")
        === "task:run-4275/t-66539,task:run-4275/t-66982,task:run-4275/t-67064,task:run-4275/t-67102,task:run-4275/t-63492",
      JSON.stringify(workers.map((row) => row.key)));
    const landed = workers.find((row) => row.key.endsWith("t-67064"));
    const landedTask = fixture.desk.tasks.find((one) => one.id === "t-67064");
    ok("a landed task's bar runs from its first start to its landing, and its end cell says it ended and how long it took",
      landed?.state === "done" && near(landed.from, placed(landedTask.times.started_ms, from, to))
        && near(landed.to, placed(landedTask.times.landed_ms, from, to)) && landed.eta === "끝"
        && landed.etaSub === "5시간 15분 걸림",
      JSON.stringify(landed));
    const reported = workers.find((row) => row.key.endsWith("t-67102"));
    ok("a reported task says how long it has waited for its review",
      reported?.state === "waiting" && reported.said.includes("45분"), JSON.stringify(reported));

    /* ---- 조용한 박자는 아무것도 쓰지 않는다 ------------------------------------- */
    const quiet = await scheduleMutations(page, async () => {
      refreshBoardSchedule();
      await new Promise((done) => setTimeout(done, 0));
    });
    drawn = await readBoardSchedule(page);
    ok("a quiet beat asks with the revision it holds and writes nothing",
      quiet.length === 0 && drawn.asked.at(-1)?.known === answer.revision, JSON.stringify({ quiet, asked: drawn.asked.at(-1) }));

    /* ---- 묶기와 펴기 ----------------------------------------------------------------- */
    await page.evaluate(() => document.querySelector('#board-view [data-group="line"] .board-schedule-group-head').click());
    drawn = await readBoardSchedule(page);
    ok("a group's head folds its rows away", drawn.groups.line.expanded === "false" && drawn.groups.line.rows.length === 0,
      JSON.stringify(drawn.groups.line));
    await page.evaluate(() => document.querySelector('#board-view [data-group="line"] .board-schedule-group-head').click());
    // A gate's block has no rows to unfold (t-77628 review P2-1): a press opens the log of the job that tells its
    // story — the step that failed, else the one running, else the one queued last.
    const l60jobs = answer.jobs.filter((job) => job.gate === "l60");
    const l60telling = l60jobs.slice().sort((a, b) => (Number(b.queued_ms) || 0) - (Number(a.queued_ms) || 0))[0];
    await page.evaluate(() => document.querySelector('#board-view [data-key="gate:l60"]').click());
    await page.evaluate(() => window.__UNTIL__(() => !document.querySelector("#board-view .board-schedule-log").hidden, "the gate's log"));
    drawn = await readBoardSchedule(page);
    ok("a gate block opens the log of the job that tells its story — a passed gate's last step",
      drawn.asked.some((one) => one.log?.job === l60telling.name) && drawn.groups.line.rows.length > 0,
      JSON.stringify({ asked: drawn.asked.at(-1), telling: l60telling.name }));
    await page.evaluate(() => document.querySelector('#board-view [data-key="gate:l60"]').click());

    /* ---- 로그: 누른 일감의 끝 줄이 일정 아래 칸에 ---------------------------------- */
    await page.evaluate(() => document.querySelector('#board-view [data-key="job:1792543500-run11955-t-63492-walk"]').click());
    await page.evaluate(() => window.__UNTIL__(() => !document.querySelector("#board-view .board-schedule-log").hidden
      && document.querySelector("#board-view .board-schedule-log-lines").textContent.includes("last line"), "the log"));
    drawn = await readBoardSchedule(page);
    ok("a job's row opens its log through the same command, by the job's name",
      drawn.asked.some((one) => one.log?.job === "1792543500-run11955-t-63492-walk"), JSON.stringify(drawn.asked.at(-1)));
    await page.evaluate(() => document.querySelector('#board-view [data-key="job:1792543500-run11955-t-63492-walk"]').click());
    ok("a second press closes the log",
      await page.evaluate(() => document.querySelector("#board-view .board-schedule-log").hidden), "");

    /* The queue's order and a paused line's words are read off the gate's job rows — drawn in the week chart. */
    const zoomTo = async (zoom) => page.evaluate(async (zoom) => {
      const button = document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`);
      if (button.getAttribute("aria-pressed") !== "true") {
        button.click();
        await window.__UNTIL__(() => !boardScheduleAsking, `the ${zoom} answer`);
      }
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, zoom);
    await zoomTo("week");
    drawn = await readBoardSchedule(page);
    const weekGates = drawn.groups.gates.rows;
    const lint = weekGates.find((row) => row.key === "job:0-1792543560-run11955-l61z-g3a-zo-lint");
    ok("in the week chart a running job past its median says how late it is and reaches 「now」",
      lint?.state === "running" && lint.said === "+1분 지연 중" && near(lint.to, drawn.now), JSON.stringify(lint));
    const held = weekGates.find((row) => row.key.includes("l61q-g5-window"));
    ok("a job a guard held says so, and its bar is not a start the runner will make",
      held?.state === "held" && held.said.includes("가드"), JSON.stringify(held));
    /* ---- 줄이 풀리면: 기다리는 일감이 비는 줄에 차례로 선다 -------------------------- */
    drawn = await moveScheduleLine(page, { paused: false, pause_note: null });
    const queued = (key) => [...drawn.groups.gates.rows, ...drawn.groups.line.rows].find((row) => row.key === key);
    const BUILD = "job:0-1792543700-run11955-l61r-g1-build";
    const TESTS = "job:0-1792543701-run11955-l61r-g2-tests";
    const PLAIN = "job:1792543750-w67501-t67482-red";
    const [build, tests, plain] = [BUILD, TESTS, PLAIN].map(queued);
    ok("with the line free the first waiting job starts where the late lint left lane 2: 「now」",
      near(build?.from, drawn.now) && build?.said.includes("시작 예상") && !build?.paused, JSON.stringify(build));
    ok("the same owner's next job waits for the first: the root tests start where the root build ends",
      tests && build && near(tests.from, build.to), JSON.stringify({ build, tests }));
    ok("on the runner's two lanes the plain job waits until the root tests end",
      plain && tests && near(plain.from, tests.to), JSON.stringify({ plain, tests }));

    /* ---- 줄은 실행기 기록이 말한 것이다: 셋째 줄을 말하면 기다리는 일감이 거기에도 선다 ------ */
    drawn = await moveScheduleLine(page, { runner_lanes: [1, 2, 3] });
    const [build3, tests3, plain3] = [BUILD, TESTS, PLAIN].map(queued);
    ok("on a third lane the runner named the root tests wait only for the root build, and the plain job starts when it ends",
      build3 && tests3 && plain3 && near(build3.from, drawn.now) && near(tests3.from, build3.to) && near(plain3.from, build3.to),
      JSON.stringify({ build3, tests3, plain3 }));

    /* ---- 시작을 모를 때: 측정이 조용한 기계를 기다리거나, 디스크가 하한 아래거나, 실행기가 아직 어느
     * 줄도 말하지 않았다. 기다리는 행은 시각 대신 「시작 모름」이고, 시작 시각을 말하는 행은 없다 ---- */
    for (const [facts, note] of [[{ quiet_wanted: true }, "측정이 조용한 기계를 기다림"],
      [{ disk_low: true }, "디스크가 하한 아래라 줄이 기다림"], [{ runner_lanes: [] }, ""]]) {
      drawn = await moveScheduleLine(page, { runner_lanes: answer.line.runner_lanes, quiet_wanted: false, disk_low: false, ...facts });
      const rows = Object.values(drawn.groups).flatMap((group) => group.rows);
      // The job rows: a worker task waiting for its review is a waiting bar too, and says how long it waited.
      const waitingNow = rows.filter((row) => row.state === "waiting" && row.key.startsWith("job:"));
      ok(`with ${JSON.stringify(facts)} every waiting job says its start is not known, and no row says when one starts`,
        waitingNow.length === 4 && waitingNow.every((row) => row.paused && row.said.includes("시작 모름"))
          && rows.every((row) => !row.said.includes("시작 예상")) && drawn.note === note,
        JSON.stringify({ note: drawn.note, waiting: waitingNow.map((row) => [row.key, row.said, row.paused]) }));
    }
    drawn = await moveScheduleLine(page, { runner_lanes: answer.line.runner_lanes, quiet_wanted: false, disk_low: false });

    await zoomTo("day");
    /* ---- 워커 과업을 누르면 「기록」 탭의 그 과업으로 — 넘김·검증·비용이 서는 곳 ------------ */
    await page.evaluate(() => document.querySelector('#board-view [data-key="task:run-4275/t-67102"]').click());
    const went = await page.evaluate(async () => {
      await new Promise((done) => requestAnimationFrame(done));
      const surface = document.querySelector("#board-view .task-board-surface");
      return {
        tab: surface.dataset.boardTab,
        chosen: surface.querySelector('[data-board-tab-button="history"]').getAttribute("aria-selected"),
        schedule: surface.querySelector(".task-board-schedule").hidden,
        stage: deskChoice.stage,
        row: surface.querySelector('.board-desk-task[data-task="run-4275/t-67102"]')?.checkVisibility() ?? false,
      };
    });
    ok("a worker task's row goes to that task on the History tab, its stage open in the task flow",
      went.tab === "history" && went.chosen === "true" && went.schedule && went.stage === "reported" && went.row,
      JSON.stringify(went));

    /* ---- 한 주로 넓히면 ------------------------------------------------------------ */
    await openBoardSchedule(page);
    await page.evaluate(async (weekFrom) => {
      window.__WEEK_FROM__ = weekFrom;
      document.querySelector('#board-view [data-schedule-zoom="week"]').click();
      await window.__UNTIL__(() => window.__SCHEDULE_ASKED__.some((one) => one.known === null
        && one.fromMs === window.__WEEK_FROM__), "the week's ask");
      await window.__UNTIL__(() => !boardScheduleAsking, "the week's answer");
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, SCHEDULE_NOW - 5 * DAY);
    drawn = await readBoardSchedule(page);
    ok("the week view asks five days back with no revision",
      drawn.asked.some((one) => one.known === null && one.fromMs === SCHEDULE_NOW - 5 * DAY), JSON.stringify(drawn.asked.slice(-3)));
    ok("the week view holds every lane run since — v1.1.77's three runs are three rows",
      drawn.groups.lanes.rows.length === answer.lanes.length
        && drawn.groups.lanes.rows.filter((row) => row.label.startsWith("v1.1.77 ")).length === 3,
      JSON.stringify(drawn.groups.lanes.rows.map((row) => row.label)));
    ok("the week's ticks are days, not hours", drawn.ticks.length > 0 && drawn.ticks.every((tick) => !tick.includes(":")),
      JSON.stringify(drawn.ticks));
    ok("the week view holds the task that landed two days ago",
      drawn.groups.workers.rows.some((row) => row.key.endsWith("t-62493")), JSON.stringify(drawn.groups.workers.rows.map((row) => row.key)));
    ok(`the schedule stays within ${BOARD_SCHEDULE_BUDGET.elements} elements`, drawn.elements <= BOARD_SCHEDULE_BUDGET.elements,
      String(drawn.elements));

    /* ---- 좁은 그릇: 「지금 도는 일」 단 하나 폭 ------------------------------------- */
    await mkdir("output/playwright/board-schedule", { recursive: true });
    for (const width of [1280, 640, 320]) {
      const size = await page.evaluate(async (width) => {
        const host = document.querySelector("#board-view .task-board-schedule");
        host.style.width = `${width}px`;
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const row = host.querySelector(".board-schedule-row");
        return { width: host.clientWidth, scroll: host.scrollWidth, areas: getComputedStyle(row).gridTemplateAreas };
      }, width);
      ok(`the schedule has no horizontal overflow in a ${width}px holder`, size.scroll <= size.width + 1, JSON.stringify(size));
      if (width === 320) {
        ok("in a narrow holder a row's name and words stand over its bar", size.areas.includes("bar bar"), size.areas);
      }
      await page.screenshot({ path: `output/playwright/board-schedule/dark-${width}.png` });
    }
    /* ---- 눈금 글: 넓은 그릇에서도 하나하나 떨어져 선다 ------------------------------- */
    // 넓은 그릇의 축은 이름 칸과 말 칸을 뺀 자리다. 눈금 수를 그릇 폭으로 세면 1440 px 창에서
    // 「오후 06:00오후 09:00…」처럼 붙어 섰다. 두 글 사이에 4 px 넘는 틈이 있어야 따로 읽힌다.
    const crowded = [];
    for (const zoom of ["week", "day"]) {
      for (const width of [1280, 640]) {
        const gaps = await page.evaluate(async ({ zoom, width }) => {
          const host = document.querySelector("#board-view .task-board-schedule");
          host.style.width = `${width}px`;
          const button = document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`);
          if (button.getAttribute("aria-pressed") !== "true") {
            button.click();
            await window.__UNTIL__(() => !boardScheduleAsking, "the zoom's answer");
          }
          paintBoardScheduleHosts();
          await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
          const vertical = zoom !== "week";
          const boxes = [...host.querySelectorAll(".board-schedule-tick")].filter((tick) => tick.checkVisibility())
            .map((tick) => tick.getBoundingClientRect()).sort((a, b) => (vertical ? a.top - b.top : a.left - b.left));
          return boxes.slice(1).map((box, at) => Math.round(vertical ? box.top - boxes[at].bottom : box.left - boxes[at].right));
        }, { zoom, width });
        if (gaps.length === 0 || gaps.some((gap) => gap <= 4)) crowded.push({ zoom, width, gaps });
      }
    }
    ok("the axis's labels stand apart in 1280px and 640px holders, a week or a day wide", crowded.length === 0,
      JSON.stringify(crowded));
    /* 좁은 그릇과 큰 글자: 글 폭은 잰다 — 고정한 자리가 아니라 (검토 t-69069 P3 ⑧). 480 px와 360 px의 그릇에서
     * 다섯 언어의 세 확대가, 그리고 글꼴이 커진 축에서도 글 사이에 4 px 넘는 틈이 서야 한다. */
    const narrowCrowded = [];
    for (const big of [false, true]) {
      await page.evaluate((big) => {
        document.getElementById("schedule-test-text")?.remove();
        if (!big) return;
        const style = document.createElement("style");
        style.id = "schedule-test-text";
        style.textContent = ".board-schedule-tick { font-size: 22px !important; }";
        document.head.append(style);
      }, big);
      for (const tongue of ["ko", "en", "ja", "zh", "es"]) {
        for (const zoom of ["now", "day", "week"]) {
          for (const width of [480, 360]) {
            const seen = await page.evaluate(async ({ tongue, zoom, width }) => {
              setLocale(tongue, { refresh: false, persist: false });
              const host = document.querySelector("#board-view .task-board-schedule");
              host.style.width = `${width}px`;
              const button = document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`);
              if (button.getAttribute("aria-pressed") !== "true") {
                button.click();
                await window.__UNTIL__(() => !boardScheduleAsking, "the zoom's answer");
              }
              paintBoardScheduleHosts();
              await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
              const vertical = zoom !== "week";
              const boxes = [...host.querySelectorAll(".board-schedule-tick:not(.is-covered)")].filter((tick) => tick.checkVisibility())
                .map((tick) => tick.getBoundingClientRect()).sort((a, b) => (vertical ? a.top - b.top : a.left - b.left));
              // Every tick the active axis drew counts, a covered one too — it keeps its line on the track.
              return { count: [...host.querySelectorAll(".board-schedule-tick")].filter((tick) => tick.checkVisibility()).length,
                gaps: boxes.slice(1).map((box, at) => Math.round(vertical ? box.top - boxes[at].bottom : box.left - boxes[at].right)) };
            }, { tongue, zoom, width });
            if (seen.count === 0 || seen.gaps.some((gap) => gap <= 4)) narrowCrowded.push({ big, tongue, zoom, width, ...seen });
          }
        }
      }
    }
    await page.evaluate(() => {
      document.getElementById("schedule-test-text")?.remove();
      setLocale("ko", { refresh: false, persist: false });
      document.querySelector("#board-view .task-board-schedule").style.width = "";
    });
    ok("the axis's labels stand apart in 480px and 360px holders in all five languages, at the type's size and a larger one",
      narrowCrowded.length === 0, JSON.stringify(narrowCrowded.slice(0, 6)));
    /* ---- 360 px 창: 좁은 배치에서도 일정의 행이 서고 닿는다 (검토 t-69069 P3 ⑨) ------------------
     * 창의 좁은 배치는 상황판을 위에 쌓고 그 아래 목록과 파일 칸을 쌓는다 — 높이 800 px 창에서 무대는
     * 약 350 px, 일정 그릇의 스크롤 칸은 약 190 px다. 일정의 몫은 가로로 넘치지 않고 모든 행이 그 칸을
     * 스스로 스크롤해 닿는 것이다. 칸이 낮은 까닭은 창의 배치 규칙이라 여기서 고치지 않고 숫자로 남긴다. */
    await page.setViewportSize({ width: 360, height: 800 });
    const small = await page.evaluate(async () => {
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const host = document.querySelector("#board-view .task-board-schedule");
      const surface = host.closest(".task-board-surface");
      const rows = [...host.querySelectorAll(".board-schedule-row, .board-schedule-block")];
      const unreached = [];
      for (const row of rows) {
        row.scrollIntoView({ block: "nearest", inline: "nearest" });
        const seat = surface.getBoundingClientRect();
        const box = row.getBoundingClientRect();
        const narrow = row.classList.contains("board-schedule-row") ? box.width < 200 : box.width < 40;
        if (box.top < seat.top - 1 || box.bottom > seat.bottom + 1 || narrow) unreached.push(row.dataset.key);
      }
      surface.scrollTop = 0;
      return {
        innerWidth, rows: rows.length, unreached: unreached.slice(0, 4),
        hostScroll: host.scrollWidth, hostClient: host.clientWidth,
        surfaceScroll: surface.scrollWidth, surfaceClient: surface.clientWidth,
        surfaceHeight: Math.round(surface.clientHeight),
      };
    });
    await page.screenshot({ path: "output/playwright/board-schedule/dark-window-360.png" });
    await page.setViewportSize({ width: 1280, height: 860 });
    ok("in a 360px window the schedule's rows stand, none wider than the holder, and the board's own scroll reaches every one",
      small.innerWidth === 360 && small.rows > 10 && small.unreached.length === 0
        && small.hostScroll <= small.hostClient + 1 && small.surfaceScroll <= small.surfaceClient + 1,
      JSON.stringify(small));

    /* 하루 보기로 돌아와 레인 행을 바꾼 답으로 읽는다. 답의 판이 올라가므로 조용한 박자 시험 뒤에 선다. */
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-schedule-zoom="day"]').click();
      await window.__UNTIL__(() => !boardScheduleAsking, "the day's answer");
    });
    /* ---- 레인의 끝 말은 답의 `result`가 말한다: 낱말을 비교하지 않는다 (검토 t-69069 P3 ⑦) ------ */
    const other = await patchScheduleLane(page, "1.1.82", { verdict: "shipped", result: "passed" });
    const shipped = laneOf(other.groups.lanes.rows, "1.1.82");
    ok("a lane the answer says passed reads as done in the board's words, whatever word the lane itself used",
      shipped?.state === "done" && shipped.said.startsWith("초록 · "), JSON.stringify(shipped));
    const broke = await patchScheduleLane(page, "1.1.82", { verdict: "aborted", result: "failed" });
    const failedRun = laneOf(broke.groups.lanes.rows, "1.1.82");
    ok("a lane the answer says failed reads as failed in the board's words",
      failedRun?.state === "failed" && failedRun.said.startsWith("빨강 · "), JSON.stringify(failedRun));
    const wordOnly = await patchScheduleLane(page, "1.1.82", { verdict: "green", result: "ended" });
    const unknownRun = laneOf(wordOnly.groups.lanes.rows, "1.1.82");
    ok("a lane the answer says only ended is not called green because it used the word",
      unknownRun?.state === "idle" && unknownRun.said.startsWith("green · "), JSON.stringify(unknownRun));
    await patchScheduleLane(page, "1.1.82", { verdict: v82.verdict, result: v82.result });

    /* ---- 레인의 남은 단계가 근거 없이 비면 끝 시각도 근거 없음이다 (검토 t-69069 P3 ⑤) ------- */
    const planned = laneOf((await readBoardSchedule(page)).groups.lanes.rows, "1.1.83");
    const bare = await patchScheduleLane(page, "1.1.83", { ahead_planned: false, ahead: running.ahead.slice(0, 1) });
    const bareLane = laneOf(bare.groups.lanes.rows, "1.1.83");
    ok("a running lane whose stages ahead no lane planned is drawn dotted, as no estimate; planned it stood on its history",
      planned?.basis === "history" && bareLane?.basis === "none", JSON.stringify({ planned: planned?.basis, bare: bareLane?.basis }));
    await patchScheduleLane(page, "1.1.83", { ahead_planned: running.ahead_planned, ahead: running.ahead });

    /* ---- 지연 중 꼬리표: 단계가 제 중앙값을 넘으면 끝은 「지금 + 남은 단계의 중앙값」 ---- */
    await page.evaluate(async () => {
      document.querySelector("#board-view .task-board-schedule").style.width = "";
      document.querySelector('#board-view [data-schedule-zoom="day"]').click();
      await window.__UNTIL__(() => !boardScheduleAsking, "the day's answer");
    });
    const later = SCHEDULE_NOW + HOUR;
    await page.clock.setFixedTime(new Date(later));
    await page.evaluate(async () => {
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    drawn = await readBoardSchedule(page);
    const lateLane = laneOf(drawn.groups.lanes.rows, "1.1.83");
    const lateEnd = later + running.ahead.slice(1).reduce((sum, stage) => sum + stage.expected.secs * 1000, 0);
    ok("a stage past its median is tagged late, and its lane's end is redrawn from now plus the stages ahead",
      lateLane?.said.startsWith("gate-root · +5분 지연 중 · ") && lateLane.paused === false
        && near(lateLane.to, placed(lateEnd, later - 16 * HOUR, later + 8 * HOUR))
        && near(lateLane.from, placed(running.start_ms, later - 16 * HOUR, later + 8 * HOUR)),
      JSON.stringify({ lateLane, lateEnd: new Date(lateEnd).toISOString() }));
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));

    /* ---- 부하 표시: 지금 부하가 코어 수를 넘으면 앞으로 남은 예상 곁에, 값은 그대로 ------- */
    const loudRead = async (loud) => {
      await page.evaluate(async (loud) => {
        deskMachine = { ...(deskMachine ?? {}), load: { one_minute: loud ? 30.2 : 3.1, cores: 12, loud } };
        paintBoardScheduleHosts();
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      }, loud);
      return readBoardSchedule(page);
    };
    const quietLoad = await loudRead(false);
    const busyLoad = await loudRead(true);
    const busyLane = laneOf(busyLoad.groups.lanes.rows, "1.1.83");
    const quietLane = laneOf(quietLoad.groups.lanes.rows, "1.1.83");
    const endedLane = laneOf(busyLoad.groups.lanes.rows, "1.1.82");
    ok("with the load over the cores a lane still ahead says so beside its estimate, and its bar does not move",
      busyLane?.said === `${quietLane?.said} · 부하 높음·늦어질 수 있음` && near(busyLane.to, quietLane.to)
        && !endedLane?.said.includes("부하"),
      JSON.stringify({ busyLane, quietLane, endedLane }));
    await loudRead(false);

    /* ---- 끝난 과업의 끝 (t-71840): git의 착지 → 발행 → 원장의 마지막 기록, 지금은 아니다 ------------ */
    // 사람 10-23 사진: 착지 기록이 없는 끝난 과업(배포됨·착지할 것 없음·실패)의 막대가 「지금」 선까지
    // 늘어나고 「N일 걸림」이 지금까지로 부풀었다. 끝은 git이 말한 착지(병합 커밋의 시각), 그것이 없으면
    // 그 일이 실린 발행, 그것도 없으면 원장이 적은 완료·검증·넘김·시도 끝 가운데 가장 늦은 것이고, 아무
    // 기록이 없으면 끝을 말하지 않는다.
    const ends = boardScheduleEnds(SCHEDULE_NOW);
    drawn = await moveScheduleTasks(page, ends);
    const workerRows = drawn.groups.workers.rows;
    const row = (id) => workerRows.find((one) => one.key === `task:run-4275/${id}`);
    const at = (hours) => placed(SCHEDULE_NOW + hours * HOUR, from, to);
    const over = workerRows.filter((one) => one.state === "done" || one.state === "failed");
    /* 겹치는 블록은 나란히 (t-77628): 도는 것과 기다리는 것이 열을 나누고, 혼자인 것은 열을 다 쓴다. */
    ok("blocks that overlap in time stand side by side — the running and the waiting task share their column, a task alone keeps it whole",
      row("t-80010")?.cols === 2 && row("t-80011")?.cols === 2 && row("t-80010").col !== row("t-80011").col
        && row("t-80003")?.cols === 1 && row("t-80002")?.cols === 2 && row("t-80005")?.cols === 2
        && workerRows.every((one) => one.block && one.col < one.cols),
      JSON.stringify(workerRows.map((one) => [one.key, one.col, one.cols])));
    ok("no finished task's bar reaches 「now」",
      over.length > 0 && over.every((one) => one.to < drawn.now - 0.5),
      JSON.stringify(over.filter((one) => one.to >= drawn.now - 0.5).map((one) => [one.key, one.to, one.said])));
    const deployed = row("t-80001");
    ok("a deployed task with no landing in the ledger ends where git merged it, and took that long",
      deployed?.state === "done" && near(deployed.to, at(-7)) && deployed.said === "배포됨 · 3시간 걸림",
      JSON.stringify(deployed));
    const nothing = row("t-80002");
    ok("a task with nothing to land ends at the last moment the ledger wrote for it — here its verification",
      near(nothing?.to, at(-4)) && nothing.said === "완료 — 착지할 것 없음 · 2시간 걸림",
      JSON.stringify(nothing));
    const failedTask = row("t-80003");
    ok("a failed task ends where its last attempt ended, and says how long it took",
      failedTask?.state === "failed" && near(failedTask.to, at(-2)) && failedTask.said === "실패 · 1시간 걸림",
      JSON.stringify(failedTask));
    const released = row("t-80004");
    ok("a deployed task git names no merge for ends at the release it shipped in",
      near(released?.to, at(-9)) && released.said === "배포됨 · 3시간 걸림", JSON.stringify(released));
    const unknown = row("t-80005");
    ok("a finished task with no end recorded anywhere says so, and its bar does not run to 「now」",
      near(unknown?.to, unknown?.from) && near(unknown.from, at(-5)) && unknown.said === "배포됨 · 끝 기록 없음",
      JSON.stringify(unknown));

    /* ---- 끝난 일은 접힌다: 최근에 끝난 몇만, 도는·기다리는 일은 다 (t-71840) ------------------------- */
    const newest = ["t-80003", "t-80002", "t-80005", "t-80001", "t-80113", "t-80112", "t-80004", "t-80111", "t-80110",
      "t-80109", "t-80108", "t-80107"].map((id) => `task:run-4275/${id}`).sort();
    ok("of nineteen finished tasks the twelve that ended last stand, beside every task still moving",
      JSON.stringify(over.map((one) => one.key).sort()) === JSON.stringify(newest)
        && workerRows.some((one) => one.key.endsWith("t-80010") && one.state === "running")
        && workerRows.some((one) => one.key.endsWith("t-80011") && one.state === "waiting")
        && workerRows.length === 14,
      JSON.stringify(workerRows.map((one) => [one.key, one.state])));
    ok("the seven finished earlier are one folded line, in the words the build line's folded jobs use",
      drawn.groups.workers.more === "+7 끝남" && /^\+\d+ 끝남$/.test(drawn.groups.line.more),
      JSON.stringify({ workers: drawn.groups.workers.more, line: drawn.groups.line.more }));
    ok("every group's rows stand in the order they started",
      Object.values(drawn.groups).every((group) => group.rows.filter((one) => one.depth === "0")
        .every((one, index, rows) => index === 0 || rows[index - 1].from <= one.from)),
      JSON.stringify(Object.fromEntries(Object.entries(drawn.groups).map(([id, group]) =>
        [id, group.rows.filter((one) => one.depth === "0").map((one) => one.from)]))));
    const unfolded = await pressScheduleMore(page, "workers");
    ok("the folded line opens on a press: every finished task in the range stands, and the line folds them again",
      unfolded.groups.workers.rows.length === 21 && unfolded.groups.workers.more === "끝난 7개 접기"
        && unfolded.groups.workers.moreExpanded === "true",
      JSON.stringify({ rows: unfolded.groups.workers.rows.length, more: unfolded.groups.workers.more }));
    const refolded = await pressScheduleMore(page, "workers");
    ok("a second press folds them away", refolded.groups.workers.rows.length === 14
      && refolded.groups.workers.more === "+7 끝남" && refolded.groups.workers.moreExpanded === "false",
    JSON.stringify({ rows: refolded.groups.workers.rows.length, more: refolded.groups.workers.more }));

    /* ---- 시안 v2의 「지금 도는 일」 타임라인 (t-71840 C) ------------------------------------------- */
    // 사람이 통과시킨 상황판 시안 v2(p-7c4a751fccf3359d)의 타임라인 그대로, 입은 것은 시안 B의 토큰과 부품뿐:
    // 정시에 맞춘 지금 앞뒤 축과 「지금」 칩, 카드 머리의 범례, 묶음 머리의 이름, 행의 둘째 줄(상태 점과 낱말),
    // 막대 안의 지금 단계와 앞의 단계 이름, 끝 칸의 굵은 시각과 그 밑 한 줄.
    drawn = await moveScheduleTasks(page, { tasks: fixture.desk.tasks, landings: {} });
    await page.evaluate(async () => {
      document.querySelector('#board-view [data-schedule-zoom="now"]')?.click();
      await window.__UNTIL__(() => !boardScheduleAsking, "the window's answer");
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    drawn = await readBoardSchedule(page);
    const hourFrom = Math.floor(SCHEDULE_NOW / HOUR) * HOUR - HOUR;
    const hours = drawn.ticks.map((tick) => Number(tick.slice(0, 2)));
    ok("「지금 앞뒤」 asks from the hour before the hour now is in, and its axis reads whole hours on the 24-hour clock",
      drawn.asked.some((one) => one.known === null && one.fromMs === hourFrom) && drawn.ticks.length >= 4
        && drawn.ticks.every((tick) => /^\d{2}:00$/.test(tick))
        && hours.every((hour, index) => index === 0 || hour === (hours[index - 1] + 1) % 24)
        && /^지금 \d{2}:\d{2}$/.test(drawn.chip) && drawn.title === drawn.chip && drawn.vertical,
      JSON.stringify({ ticks: drawn.ticks, chip: drawn.chip, title: drawn.title, asked: drawn.asked.slice(-2) }));
    // The draft's chip stands beside 「10:00」, never on it: an hour the chip would sit on keeps its line on the
    // track and hides its words, and every hour's words that show stand clear of the chip.
    const beside = await page.evaluate(() => {
      const host = document.querySelector("#board-view .task-board-schedule");
      const chip = [...host.querySelectorAll(".board-schedule-now")].find((one) => one.checkVisibility()).getBoundingClientRect();
      // The gutter's ticks stand over and under the chip (t-77628): the distance is vertical.
      return [...host.querySelectorAll(".board-schedule-tick")].filter((tick) => tick.checkVisibility()).map((tick) => {
        const box = tick.getBoundingClientRect();
        return { label: tick.textContent, shown: getComputedStyle(tick).visibility !== "hidden",
          apart: Math.round(Math.max(chip.top - box.bottom, box.top - chip.bottom)) };
      });
    });
    ok("the 「지금」 chip covers no hour's words: an hour under it keeps its line and hides its label",
      beside.filter((one) => one.shown).length >= 3 && beside.every((one) => !one.shown || one.apart > 4),
      JSON.stringify(beside));
    const legend = await page.evaluate(() => document.querySelector("#board-view .board-schedule-gutter")?.dataset.tip ?? "");
    const blocksLegend = "칠함 = 지난·끝남 · 빗금 = 앞으로(지난 소요로 잰 예상) · 점선 = 근거 없음 · 톱니 = 범위 앞에서 시작";
    ok("a line under the timetable says how a block reads, in the blocks' words (the gutter's tip says the same), and every column is named alone and counted",
      legend === blocksLegend && drawn.calLegend === blocksLegend
        && drawn.groups.lanes.head === "발행" && drawn.groups.gates.head === "게이트"
        && drawn.groups.line.head === "빌드 줄" && drawn.groups.workers.head === "워커 과업"
        && /^\d+( · 차례 기다림 \d+)?( · 끝남 \d+)?$/.test(drawn.groups.workers.headSub) && drawn.groups.workers.headSub.includes("차례 기다림"),
      JSON.stringify({ legend, heads: Object.values(drawn.groups).map((group) => [group.head, group.headSub]) }));
    const nowLane = laneOf(drawn.groups.lanes.rows, "1.1.83");
    ok("the running lane names the stage it is in inside its bar and the stages ahead in its stripes, the end on the right",
      drawn.groups.lanes.rows.length === 1 && nowLane?.nowLabel === "gate-root"
        && nowLane.aheadLabel === "gate-zo · build-app · publish" && nowLane.sub.startsWith("도는 중 · gate-root ")
        && nowLane.sub.endsWith(" · 다음 gate-zo") && /^\d{2}:\d{2}$/.test(nowLane.eta) && /^1시간 \d+분 남음$/.test(nowLane.etaSub),
      JSON.stringify(nowLane));
    const passedGate = drawn.groups.gates.rows.find((one) => one.key === "gate:l60");
    const runningGate = drawn.groups.gates.rows.find((one) => one.key === "gate:l61");
    ok("a finished gate is one line that says how many went green, that it ended and how long it took",
      passedGate?.label === "l60 결합 게이트" && /^\d+개 초록$/.test(passedGate.nowLabel) && passedGate.eta === "끝"
        && passedGate.etaSub.endsWith(" 걸림") && passedGate.sub.startsWith("끝남 · "),
      JSON.stringify(passedGate));
    ok("the gate in play names its running step and the steps after it, and when it is expected to end",
      runningGate?.nowLabel === "g3a-zo-lint" && runningGate.aheadLabel.includes("g1-build")
        && /^\d{2}:\d{2}$/.test(runningGate.eta) && runningGate.etaSub.endsWith(" 남음"),
      JSON.stringify(runningGate));
    const carried = drawn.groups.workers.rows.find((one) => one.key.endsWith("t-63492"));
    const reviewing = drawn.groups.workers.rows.find((one) => one.key.endsWith("t-67102"));
    ok("a worker task's end cell speaks as the Now tab's does: how long it has run, or that it waits and for how long",
      /째$/.test(carried?.eta ?? "") && carried.sub.startsWith("도는 중 · ") && reviewing?.eta === "대기"
        && reviewing.etaSub === "기다린 지 45분" && reviewing.aheadLabel === "보고됨",
      JSON.stringify({ carried, reviewing }));
    const look = await page.evaluate(async () => {
      const host = document.querySelector("#board-view .task-board-schedule");
      if (!host.querySelector(".board-schedule-block")) return { sizes: [], dark: { drawn: NaN }, bright: { drawn: NaN } };
      // A theme flip eases every colour that carries a transition (the group head's, 150 ms): the words are
      // read once those have settled, not in the frame the flip asked for them. `getAnimations()` flushes the
      // styles first, so the transitions the flip starts are among the ones waited on.
      const settled = () => Promise.race([
        Promise.all(document.getAnimations().filter((one) => one instanceof CSSTransition)
          .map((one) => one.finished.catch(() => null))),
        new Promise((done) => setTimeout(done, 2000)),
      ]);
      const parse = (said) => {
        const rgb = said.match(/^rgba?\(([^)]+)\)$/);
        if (rgb) {
          const [r, g, b, a = "1"] = rgb[1].split(/[\s,/]+/).filter(Boolean);
          return [Number(r) / 255, Number(g) / 255, Number(b) / 255, Number(a)];
        }
        const srgb = said.match(/^color\(srgb ([^)]+)\)$/);
        if (srgb) {
          const [r, g, b, slash, a] = srgb[1].split(/\s+/);
          return [Number(r), Number(g), Number(b), slash === "/" ? Number(a) : 1];
        }
        return null;
      };
      const over = (top, under) => top.slice(0, 3).map((part, at) => part * top[3] + under[at] * (1 - top[3]));
      const light = (rgb) => rgb.map((part) => (part <= 0.04045 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4))
        .reduce((sum, part, at) => sum + part * [0.2126, 0.7152, 0.0722][at], 0);
      const ratio = (ink, ground) => {
        const under = parse(ground);
        const shown = over(parse(ink), under);
        const [a, b] = [light(shown), light(under.slice(0, 3))].sort((x, y) => y - x);
        return (a + 0.05) / (b + 0.05);
      };
      const probe = document.createElement("span");
      host.append(probe);
      const fill = (token) => {
        probe.style.background = `var(${token})`;
        return getComputedStyle(probe).backgroundColor;
      };
      const pick = (selector, pseudo = null) => {
        const node = host.querySelector(selector);
        return node ? getComputedStyle(node, pseudo) : null;
      };
      // The vertical view's words (t-77628): the board's lines on the card, a column's head and the ticks on the
      // band, a block's name on every fill it stands on, its second and third lines on the well, the chips.
      const read = () => {
        const deck = getComputedStyle(host.querySelector(".board-schedule-board")).backgroundColor;
        const band = getComputedStyle(host.querySelector(".board-schedule-gutter")).backgroundColor;
        const well = fill("--surface-well");
        const name = pick(".board-schedule-block .board-schedule-label").color;
        const chip = pick(".board-schedule-gutter-track .board-schedule-now");
        const act = pick(".board-schedule-block", "::after");
        const out = {
          what: ratio(pick(".board-schedule-line-what").color, deck),
          whatSub: ratio(pick(".board-schedule-line-what", "::after").color, deck),
          word: ratio(pick(".board-schedule-line-big", "::after").color, deck),
          note: ratio(pick(".board-schedule-line-note").color, deck),
          group: ratio(pick(".board-schedule-cal-body .board-schedule-group-head").color, band),
          groupSub: ratio(pick(".board-schedule-cal-body .board-schedule-group-head", "::after").color, band),
          tick: ratio(pick(".board-schedule-gutter-track .board-schedule-tick").color, band),
          chip: ratio(chip.color, chip.backgroundColor),
          act: ratio(act.color, act.backgroundColor),
          nameWorked: ratio(name, fill("--schedule-worked-fill")),
          nameRunning: ratio(name, fill("--schedule-run-fill")),
          nameDone: ratio(name, fill("--schedule-done-fill")),
          nameHalted: ratio(name, fill("--schedule-halt-fill")),
          nameWell: ratio(name, well),
          id: ratio(pick(".board-schedule-block .board-schedule-label", "::before").color, well),
          sub: ratio(pick(".board-schedule-block .board-schedule-label", "::after").color, well),
          why: ratio(pick(".board-schedule-block", "::before").color, well),
        };
        for (const tone of ["halt", "wait", "done", "quiet", "run"]) {
          const big = pick(`.board-schedule-line[data-tone="${tone}"] .board-schedule-line-big`);
          if (big) out[`big-${tone}`] = ratio(big.color, deck);
        }
        return out;
      };
      const sizes = new Set();
      for (const node of host.querySelectorAll("*")) {
        if (!node.checkVisibility()) continue;
        if (node.childNodes.length > 0 && [...node.childNodes].some((one) => one.nodeType === Node.TEXT_NODE && one.data.trim())) {
          sizes.add(getComputedStyle(node).fontSize);
        }
        for (const pseudo of ["::before", "::after"]) {
          const style = getComputedStyle(node, pseudo);
          if (style.content.startsWith("attr(") || /^".+"$/.test(style.content)) sizes.add(style.fontSize);
        }
      }
      await settled();
      const dark = read();
      document.documentElement.setAttribute("data-theme", "light");
      await settled();
      const bright = read();
      document.documentElement.removeAttribute("data-theme");
      await settled();
      probe.remove();
      return { sizes: [...sizes].sort(), dark, bright };
    });
    const faint = Object.entries({ ...Object.fromEntries(Object.entries(look.dark).map(([key, value]) => [`dark ${key}`, value])),
      ...Object.fromEntries(Object.entries(look.bright).map(([key, value]) => [`light ${key}`, value])) })
      .filter(([, value]) => !(value >= 4.5));
    ok("every word on the schedule reads at 4.5 : 1 or more in both themes, the bar's names on their fills included",
      faint.length === 0, JSON.stringify({ faint, dark: look.dark, light: look.bright }));
    ok("the schedule speaks in at most four type sizes", look.sizes.length <= 4, JSON.stringify(look.sizes));

    /* ---- 안내판 넉 줄 (t-77628): 다음 발행 · 게이트 · 착지 대기 · 끝남 — 큰 글 하나, 낱말, 비고, 행동 칩 ---- */
    let lines = Object.fromEntries(drawn.board.lines.map((line) => [line.id, line]));
    ok("the board's first line is the lane running: its expected end as a clock, the time left beside it, the stage in its note",
      /^\d{2}:\d{2}$/.test(lines.lane?.big ?? "") && /남음$/.test(lines.lane.word) && lines.lane.note.includes("gate-root")
        && lines.lane.tone === "run" && lines.lane.act === "로그 →",
      JSON.stringify(lines.lane));
    ok("the gate line is the gate in play: running, its green count, and the chip that opens it",
      lines.gate?.what === "게이트 l61" && lines.gate.word === "도는 중" && /\d+\/\d+ 초록/.test(lines.gate.note)
        && lines.gate.tone === "run" && lines.gate.act === "로그 →",
      JSON.stringify(lines.gate));
    const waitingCount = drawn.groups.workers.rows.filter((one) => one.state === "waiting").length;
    ok("the waiting line counts the tasks waiting to land and names the one that waited longest",
      waitingCount > 0 && lines.waiting?.big === `${waitingCount}건` && /^가장 오래 .+ \(t-\d+\)$/.test(lines.waiting.note)
        && lines.waiting.word === "코디 차례" && lines.waiting.tone === "wait" && lines.waiting.act === "기록 →"
        && lines.waiting.whatSub === "보고·검증 뒤 · 코디 차례",
      JSON.stringify({ line: lines.waiting, waitingCount }));
    const endedCount = drawn.groups.workers.rows.filter((one) => one.state === "done" || one.state === "failed").length;
    ok("the finished line counts what ended in the range and says when the last one did",
      lines.ended?.big === `${endedCount}건` && lines.ended.word === "끝남"
        && (endedCount === 0 ? lines.ended.note === "이 범위에 없음" : /^마지막 \d{2}:\d{2} · /.test(lines.ended.note)),
      JSON.stringify({ line: lines.ended, endedCount }));
    ok("the board's head is the clock and one line of the three answers",
      drawn.board.now === drawn.chip && drawn.board.sum.startsWith("다음 발행 ") && drawn.board.sum.includes("게이트 도는 중")
        && drawn.board.sum.endsWith(`착지 대기 ${waitingCount}`),
      JSON.stringify(drawn.board));
    ok("with a lane running the timetable does not say nothing is ahead", drawn.aheadEmpty === false, String(drawn.aheadEmpty));
    ok("the vertical view stays within the element budget", drawn.elements <= BOARD_SCHEDULE_BUDGET.elements, String(drawn.elements));

    /* ---- 범위 앞에서 시작한 블록은 윗변이 톱니이고 언제부터인지 말한다 ---- */
    const blocks = Object.values(drawn.groups).flatMap((group) => group.rows.filter((one) => one.block));
    const cutBlocks = blocks.filter((one) => one.cut);
    ok("a block that began before the range is cut at the top and says since when; one that began inside is not",
      cutBlocks.length > 0 && cutBlocks.every((one) => /부터$/.test(one.sub) && one.from === 0)
        && blocks.filter((one) => !one.cut).every((one) => !/부터$/.test(one.sub)),
      JSON.stringify(blocks.map((one) => [one.key, one.cut, one.from, one.sub])));

    /* ---- 과업↔게이트 (t-77628): 일감의 `# task:` 줄이나 `what` 줄이 적은 과업 id ---- */
    const l61jobs = answer.jobs.filter((job) => job.gate === "l61");
    const lintJob = l61jobs.find((job) => job.kind === "g3a-zo-lint");
    const buildJob = l61jobs.find((job) => job.kind === "g1-build");
    drawn = await patchScheduleJobs(page, [[lintJob.name, { tasks: ["t-67102"] }],
      [buildJob.name, { what: "v1.1.84: t-66539/7102 ctty helper — g1-build" }]]);
    const rode = (id) => drawn.groups.workers.rows.find((one) => one.key.endsWith(id));
    const l61head = drawn.groups.gates.rows.find((one) => one.key === "gate:l61");
    ok("a waiting task a running gate carries says so — by the job's task line or its what line, the slash form too — and its block ends where the gate is expected to",
      rode("t-67102")?.why.startsWith("l61에 실림 → 도는 중 · ") && rode("t-66539")?.why.startsWith("l61에 실림 → 도는 중 · ")
        && near(rode("t-67102").to, l61head.to) && near(rode("t-66539").to, l61head.to)
        && rode("t-67102").inherited && rode("t-67102").mark > 0 && rode("t-67102").basis === l61head.basis
        && rode("t-67102").said.includes("l61에 실림") && rode("t-67102").rideState === "running" && rode("t-66539").rideState === "running",
      JSON.stringify({ t67102: rode("t-67102"), t66539: rode("t-66539"), l61head }));
    lines = Object.fromEntries(drawn.board.lines.map((line) => [line.id, line]));
    ok("the waiting line then says the gate runs and how many ride it, and the gate line counts the tasks it carries",
      lines.waiting.word === "게이트 도는 중" && lines.waiting.note.includes("2건 l61에 실림 → 도는 중")
        && lines.gate.note.includes("과업 2건 실림"),
      JSON.stringify({ waiting: lines.waiting, gate: lines.gate }));
    const stopped = SCHEDULE_NOW - 60_000;
    // Only the running step fails; the steps queued behind it stay waiting and held (review P3-3).
    drawn = await patchScheduleJobs(page, [[lintJob.name, { state: "failed", rc: 1, end_ms: stopped, secs: 60 }]]);
    lines = Object.fromEntries(drawn.board.lines.map((line) => [line.id, line]));
    ok("one failed step stops the gate while its other steps still queue: the tasks it carries say so in the halt ink, end at 「now」, the board says 「막힘」 and the gate line's chip opens the failed job's log",
      rode("t-67102")?.why === "l61에 실림 → 실패로 멈춤" && rode("t-67102").rideState === "failed" && near(rode("t-67102").to, drawn.now)
        && !rode("t-67102").inherited && l61head && drawn.groups.gates.rows.find((one) => one.key === "gate:l61")?.state === "failed"
        && lines.waiting.word === "막힘" && lines.waiting.tone === "halt" && lines.gate.word === "실패" && lines.gate.tone === "halt"
        && lines.gate.act === "로그 →" && lines.gate.note.startsWith("g3a-zo-lint 실패 · 0/4 초록 · 과업 2건 실림"),
      JSON.stringify({ t67102: rode("t-67102"), gate: drawn.groups.gates.rows.find((one) => one.key === "gate:l61"), waiting: lines.waiting, line: lines.gate }));
    const failedGate = drawn.groups.gates.rows.find((one) => one.key === "gate:l61");
    ok("a gate stopped by a failure ends where its last finished step ended — never ahead of 「now」 — and the board's big clock is that moment",
      failedGate && failedGate.to <= drawn.now + 0.01 && lines.gate.big === "09:49" && failedGate.sub.includes("09:49"),
      JSON.stringify({ to: failedGate?.to, now: drawn.now, big: lines.gate.big, sub: failedGate?.sub }));
    // With a gate queued behind the failed one, the board still names the failed gate (review r2 P3-2).
    const behindFailed = await page.evaluate(async (now) => {
      const held = window.__SCHEDULE__;
      const later = { name: "0-1792543900-run11955-l62r-g1-build", who: "run11955-l62r", state: "waiting", class: "first", lane: 1,
        cap_secs: 1800, queued_ms: now - 60_000, start_ms: null, end_ms: null, secs: null, rc: null, kind: "g1-build", gate: "l62",
        part: "r", what: null, expected: { secs: 600, samples: 5 } };
      window.__SCHEDULE__ = { ...held, revision: held.revision + 1, jobs: [...held.jobs, later] };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const line = document.querySelector('#board-view .board-schedule-line[data-line="gate"]');
      const out = { what: line.children[0].textContent, word: line.children[1].dataset.word,
        l62act: document.querySelector('#board-view [data-key="gate:l62"]')?.dataset.act ?? null };
      window.__SCHEDULE__ = { ...held, revision: held.revision + 2 };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the restored answer");
      paintBoardScheduleHosts();
      return out;
    }, SCHEDULE_NOW);
    ok("with a gate queued behind the failed one, the board's gate line keeps the failed gate, and the queued gate's block carries no log chip (nothing of it has run)",
      behindFailed.what === "게이트 l61" && behindFailed.word === "실패" && behindFailed.l62act === "",
      JSON.stringify(behindFailed));
    // A gate that ran after the failed one tells the newer story: passed later, the board says passed (review r3 P2).
    const passedAfter = await page.evaluate(async (now) => {
      const held = window.__SCHEDULE__;
      const later = { name: "0-1792543850-run11955-l62r-g1-build", who: "run11955-l62r", state: "passed", class: "first", lane: 1,
        cap_secs: 1800, queued_ms: now - 90_000, start_ms: now - 60_000, end_ms: now - 30_000, secs: 30, rc: 0, kind: "g1-build",
        gate: "l62", part: "r", what: null, expected: { secs: 600, samples: 5 } };
      window.__SCHEDULE__ = { ...held, revision: held.revision + 1, jobs: [...held.jobs, later] };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const line = document.querySelector('#board-view .board-schedule-line[data-line="gate"]');
      const out = { what: line.children[0].textContent, word: line.children[1].dataset.word, big: line.children[1].textContent,
        l62act: document.querySelector('#board-view [data-key="gate:l62"]')?.dataset.act ?? null };
      window.__SCHEDULE__ = { ...held, revision: held.revision + 2 };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the restored answer");
      paintBoardScheduleHosts();
      return out;
    }, SCHEDULE_NOW);
    ok("a gate that passed after the failed one takes the board's gate line — the failure is no longer the latest word — and its block opens its last job's log",
      passedAfter.what === "게이트 l62" && passedAfter.word === "통과" && passedAfter.big === "09:49" && passedAfter.l62act === "로그 →",
      JSON.stringify(passedAfter));
    drawn = await patchScheduleJobs(page, l61jobs.map((job) => [job.name, {
      state: job.state, rc: job.rc ?? null, end_ms: job.end_ms ?? null, secs: job.secs ?? null, tasks: null, what: job.what ?? null,
    }]));
    ok("the answer restored, nothing rides the gate", !rode("t-67102")?.why && !rode("t-66539")?.why,
      JSON.stringify([rode("t-67102")?.why, rode("t-66539")?.why]));

    /* ---- 안내판의 게이트 줄은 도는 게이트다 (review P2-3): 그 뒤에 줄 선 게이트가 더 늦게 시작해도 ---- */
    const queuedGate = await page.evaluate(async (now) => {
      const held = window.__SCHEDULE__;
      const later = { name: "0-1792543900-run11955-l62r-g1-build", who: "run11955-l62r", state: "waiting", class: "first", lane: 1,
        cap_secs: 1800, queued_ms: now - 60_000, start_ms: null, end_ms: null, secs: null, rc: null, kind: "g1-build", gate: "l62",
        part: "r", what: null, expected: { secs: 600, samples: 5 } };
      window.__SCHEDULE__ = { ...held, revision: held.revision + 1, jobs: [...held.jobs, later] };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const line = document.querySelector('#board-view .board-schedule-line[data-line="gate"]');
      const heads = [...document.querySelectorAll('#board-view [data-group="gates"] .board-schedule-block')].map((one) => one.dataset.key);
      const l62act = document.querySelector('#board-view [data-key="gate:l62"]')?.dataset.act ?? null;
      window.__SCHEDULE__ = { ...held, revision: held.revision + 2 };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the restored answer");
      paintBoardScheduleHosts();
      return { what: line.children[0].textContent, word: line.children[1].dataset.word, heads, l62act };
    }, SCHEDULE_NOW);
    ok("with a gate queued behind the running one, the board's gate line still names the running gate, both stand as blocks, and the queued one carries no log chip",
      queuedGate.what === "게이트 l61" && queuedGate.word === "도는 중" && queuedGate.heads.includes("gate:l62") && queuedGate.l62act === "",
      JSON.stringify(queuedGate));

    /* ---- 안내판 게이트 줄의 규칙 — 한 문장, 스물한 경우의 표 (review r4~r6, t-77851·t-77880·t-77892): 「도는 단계가 있는 게이트
     * → 없으면 실제로 시작한(끝난 단계가 하나라도 있는) 게이트 가운데 마지막 활동이 가장 늦은 것(통과·실패·단계 사이 대기)
     * → 없으면 마지막으로 줄 선 것」. 마지막 활동은 단계마다 마지막 일감 가운데 가장 늦은 시작이다 — 같은 이름으로 다시
     * 도는 게이트는 첫 시도가 아니라 지금 시도로(⑨·⑩), 한 단계만 다시 돈 것도 그 단계로(⑮) 비교된다. 같으면 실패 > 단계
     * 사이 > 통과, 그다음 이름의 수가 큰 것(⑫·⑬·⑭) — 묶음 안 행 차례(시작, 키)와 갈리는 동점이라야 규칙을 고정한다.
     * ⑥은 계획이 l62를 l61 뒤에 세워 시작 차례만으로도 맞는 경우고, 「마지막으로 줄 선 것」 갈래는 계획 시작이 같은 ⑪과
     * 줄 선 시각이 같은 ⑰이 고정한다. 줄의 「걸림」은 지금 시도의 시작부터고(⑯, 픽스처 l56 15:51 → 58분), 단계 사이에서는
     * 첫 단계의 지난 시각이 아니라 다음 단계의 예상을 말한다(⑱, 픽스처 l56 14:56). 두 레인에 겹쳐 돈 게이트는 늦게 시작한
     * 쪽이다 — 늦게 끝난 쪽이 아니다(⑲; 끝으로 비교하면 ⑦·⑬이 깨진다 — 코디가 둔 선택, t-77892 P3-1). 기다리는 게이트의
     * 시간 글은 한 곳에서 짓는다: 행의 said(aria)와 etaSub가 같은 시각을 말하고, 그 시각은 남은 waiting 단계의 가장 이른 예상
     * 시작이다 — 재시도가 줄만 섰으면 그 계획(⑳, 첫 시도의 08:10이 아님), held만 남았으면 「시작 모름」(㉑). 그 게이트에
     * 실린 과업 행의 셋째 줄 「…에 실림 → 차례 기다림 · HH:MM 시작 예상」도 같은 글이다(⑱·⑳의 과업, t-77931 P2). ---- */
    const gateTable = await page.evaluate((now) => {
      const base = boardScheduleAnswer;
      const l61 = base.jobs.filter((job) => job.gate === "l61");
      const minute = 60_000;
      const second = 1_000;
      const job = (name, state, start, end, kind, extra = {}) => ({ name, who: "run11955-l62r", state, class: "first", lane: 1, cap_secs: 1800,
        queued_ms: now - 10 * minute, start_ms: start, end_ms: end, secs: end && start ? Math.round((end - start) / 1000) : null,
        rc: state === "passed" ? 0 : state === "failed" ? 1 : null, kind, gate: "l62", part: "r", what: null,
        expected: { secs: 600, samples: 5 }, ...extra });
      const l61as = (how) => l61.map((one) => (how === "failed"
        ? (one.kind === "g3a-zo-lint" ? { ...one, state: "failed", rc: 1, end_ms: now - minute, secs: 60 } : one)
        : how === "passed"
          ? { ...one, state: "passed", rc: 0, start_ms: one.start_ms ?? now - 8 * minute, end_ms: now - 4 * minute, secs: 240 }
          : how === "between"
            ? (one.kind === "g3a-zo-lint" ? { ...one, state: "passed", rc: 0, end_ms: now - minute, secs: 170 } : one)
            : how === "waiting" ? { ...one, state: "waiting", start_ms: null, end_ms: null, secs: null, rc: null } : one));
      const l62 = {
        queued: [job("0-1792543900-run11955-l62r-g1-build", "waiting", null, null, "g1-build", { queued_ms: now - minute })],
        passed: [job("0-1792543850-run11955-l62r-g1-build", "passed", now - minute, now - 30_000, "g1-build")],
        between: [job("0-1792543850-run11955-l62r-g1-build", "passed", now - 2 * minute, now - minute, "g1-build"),
          job("0-1792543851-run11955-l62r-g2-tests", "waiting", null, null, "g2-tests", { lane: null })],
        running: [job("0-1792543850-run11955-l62r-g1-build", "running", now - minute, null, "g1-build")],
      };
      const mix = (how61, how62) => [...l61as(how61), ...l62[how62]];
      // ⑨·⑩: l61's first attempt failed at its lint three minutes ago, l62 passed after it, and l61 was queued again
      // 80 s ago under the same name — every step once more, so each step's last job is the new attempt's.
      const l61first = l61.filter((one) => one.kind === "g3a-zo-lint")
        .map((one) => ({ ...one, state: "failed", rc: 1, end_ms: now - 3 * minute, secs: 110 }));
      const l62before = [job("0-1792543680-run11955-l62r-g1-build", "passed", now - 2 * minute, now - 90 * second, "g1-build")];
      const l61s = (kind, state, start, end, extra = {}) => job(`0-1792543720-run11955-l61s-${kind}`, state, start, end, kind,
        { gate: "l61", who: "run11955-l61s", lane: start ? 2 : null, queued_ms: now - 80 * second, ...extra });
      const again = (how) => [
        l61s("g3a-zo-lint", "passed", now - 75 * second, now - 60 * second),
        how === "failed" ? l61s("g1-build", "failed", now - 60 * second, now - 30 * second) : l61s("g1-build", "waiting", null, null),
        l61s("g2-tests", "waiting", null, null),
        l61s("g5-window-실측", "held", null, null),
      ];
      // ⑪: one waiting job each, different owners, two lanes free — both are planned to start now; l61 was queued last.
      const queuedOnly = (gate, who, queuedAt, kind) => job(`0-${Math.round(queuedAt / 1000)}-${who}-${kind}`, "waiting", null, null, kind,
        { gate, who, lane: null, queued_ms: queuedAt });
      const l61late = [queuedOnly("l61", "run11955-l61r", now - minute, "g3a-zo-lint")];
      const l62early = [queuedOnly("l62", "run11955-l62r", now - 4 * minute, "g1-build")];
      // ⑫·⑬·⑭: l62's first step began at the very millisecond l61's lint did (the group's row order then puts l61 first).
      const sameStart = l61.find((one) => one.kind === "g3a-zo-lint").start_ms;
      const l62same = (how) => [job("0-1792543569-run11955-l62r-g1-build", how === "failed" ? "failed" : "passed", sameStart, now - 2 * minute, "g1-build"),
        ...(how === "between" ? [job("0-1792543570-run11955-l62r-g2-tests", "waiting", null, null, "g2-tests", { lane: null })] : [])];
      // ⑮: l61 passed its build and failed its tests, l62 passed after it, then only l61's tests ran again and failed.
      const at = (minutesAgo) => now - minutesAgo * minute;
      const l61r = (kind, state, start, end, extra = {}) => job(`0-${Math.round((extra.queued_ms ?? at(50)) / 1000)}-run11955-l61r-${kind}`, state, start, end, kind,
        { gate: "l61", who: "run11955-l61r", queued_ms: at(50), ...extra });
      const oneStepAgain = [
        l61r("g1-build", "passed", at(50), at(48)), l61r("g2-tests", "failed", at(48), at(45)),
        job("0-1792541400-run11955-l62r-g1-build", "passed", at(40), at(35), "g1-build"),
        l61r("g2-tests", "failed", at(10), at(5), { queued_ms: at(11) }),
      ];
      // ⑰: two gates queued at the same moment on one lane — l10's job stands first by name, so l9 is planned later.
      const bothQueued = [queuedOnly("l9", "run11955-l9r", now - minute, "g1-build"), queuedOnly("l10", "run11955-l10r", now - minute, "g1-build")];
      // ⑲ (t-77892 C2, a choice kept): on lane 1 l61 passed its build (-100 → -70 min) and its tests (-70 → -65); on
      // lane 2 l62 passed its build (-90 → -80) and its tests began at -75 and failed at -30. The rule compares the
      // latest step's start, so l61 (-70) wins over l62 (-75) although l62 failed later — comparing ends would break ⑦
      // and ⑬. The row pins the choice.
      const overlapped = [
        l61r("g1-build", "passed", at(100), at(70), { queued_ms: at(101) }), l61r("g2-tests", "passed", at(70), at(65), { queued_ms: at(101) }),
        job("0-1792538400-run11955-l62r-g1-build", "passed", at(90), at(80), "g1-build", { lane: 2, queued_ms: at(91) }),
        job("0-1792538401-run11955-l62r-g2-tests", "failed", at(75), at(30), "g2-tests", { lane: 2, queued_ms: at(91) }),
      ];
      // ⑳ (t-77892 D′): l61 failed its build 90 minutes ago and was queued again a minute ago; nothing of the new attempt
      // has begun, so the gate's start is the new attempt's planned start — now, with a lane free — not the build's 08:10.
      const queuedAgain = [
        l61r("g1-build", "failed", at(100), at(90), { queued_ms: at(101) }),
        l61r("g1-build", "waiting", null, null, { queued_ms: now - minute, lane: null, tasks: ["t-90002"], what: "l61 결합 게이트 g1 — t-90002" }),
      ];
      // A verified task the gate carries (its job names it): its row's third line reads the gate's time word.
      const carried = (id, moment) => ({ run: "run-4275", id, title: "일정 탭의 실린 과업", stage: "verified", gate: null, blocked_by: [], closed: null,
        created_ms: moment - 3 * 3_600_000, cost: null, writing: null,
        times: { started_ms: moment - 3 * 3_600_000, handed_ms: moment - 20 * minute, verified_ms: moment - 10 * minute, landed_ms: null } });
      const carry = (jobs, name, id) => jobs.map((one) => (one.name === name ? { ...one, tasks: [id], what: `${one.what ?? ""} — ${id}` } : one));
      // ㉑ (t-77892 H1): l61 passed its build and only a held step remains — the guard holds it, no lane plans it — so the
      // start is unknown, not the build's past start.
      const heldOnly = [
        l61r("g1-build", "passed", at(20), at(10), { queued_ms: at(21) }),
        l61r("g2-tests", "held", null, null, { queued_ms: at(21), lane: null }),
      ];
      // ⑯·⑱: the fixture's real l56 (its second attempt) and l57, cut to a moment — a job not yet begun waits, one not
      // yet ended runs. At 15:51:40 KST the second attempt's tests have just failed; at 14:56:40 it waits between steps.
      const cut = (gates, moment) => base.jobs.filter((one) => gates.includes(one.gate) && Number(one.queued_ms) <= moment).map((one) => {
        const begun = Number(one.start_ms) || 0;
        const ended = Number(one.end_ms) || 0;
        if (!begun || begun > moment) return { ...one, state: "waiting", start_ms: null, end_ms: null, secs: null, rc: null };
        if (!ended || ended > moment) return { ...one, state: "running", end_ms: null, secs: null, rc: null };
        return one;
      });
      const failedAt = 1_792_479_100_000;
      const betweenAt = 1_792_475_800_000;
      const cases = [
        { name: "① l61 실패 + l62 대기(시작 안 함)", jobs: mix("failed", "queued"), want: ["l61", "실패"] },
        { name: "② l61 실패 → l62 통과", jobs: mix("failed", "passed"), want: ["l62", "통과"] },
        { name: "③ l61 통과 → l62 단계 사이 대기", jobs: mix("passed", "between"), want: ["l62", "차례 기다림"] },
        { name: "④ l61 통과 → l62 도는 중", jobs: mix("passed", "running"), want: ["l62", "도는 중"] },
        { name: "⑤ l61 도는 중 + l62 대기", jobs: mix("running", "queued"), want: ["l61", "도는 중"] },
        { name: "⑥ 모두 대기(시작 안 함) → 마지막으로 줄 선 것", jobs: mix("waiting", "queued"), want: ["l62", "차례 기다림"] },
        { name: "⑦ l61 실패 → l62 단계 사이 대기", jobs: mix("failed", "between"), want: ["l62", "차례 기다림"] },
        { name: "⑧ 범위 안에 게이트 없음", jobs: [], want: ["", ""] },
        { name: "⑨ l61 실패 → l62 통과 → l61 재시도가 l62 뒤에 실패", jobs: [...l61first, ...l62before, ...again("failed")], want: ["l61", "실패"] },
        { name: "⑩ l61 실패 → l62 통과 → l61 재시도가 단계 사이 대기", jobs: [...l61first, ...l62before, ...again("between")], want: ["l61", "차례 기다림"] },
        { name: "⑪ 모두 대기, 계획 시작 같음 → 나중에 줄 선 l61(번호는 작은 쪽)", jobs: [...l62early, ...l61late], want: ["l61", "차례 기다림"] },
        { name: "⑫ 시작 같음: l61 실패 · l62 단계 사이 → 실패가 먼저", jobs: [...l62same("between"), ...l61as("failed")], want: ["l61", "실패"] },
        { name: "⑬ 시작 같음: l61 단계 사이 · l62 실패 → 실패가 먼저(행 차례와 반대)", jobs: [...l61as("between"), ...l62same("failed")], want: ["l62", "실패"] },
        { name: "⑭ 시작 같음: l61 · l62 둘 다 통과 → 수가 큰 l62(행 차례와 반대)", jobs: [...l61as("passed").map((one) => ({ ...one, start_ms: sameStart })), ...l62same("passed")], want: ["l62", "통과"] },
        { name: "⑮ l61 g2 실패 → l62 통과 → l61 g2만 다시 돌아 실패 → 마지막 활동이 늦은 l61", jobs: oneStepAgain, want: ["l61", "실패"] },
        { name: "⑯ 픽스처 15:51 — l56 둘째 시도 g2 실패, 걸림은 지금 시도(14:53)부터 58분", jobs: cut(["l56", "l57"], failedAt), at: failedAt,
          want: ["l56", "실패"], note: (note) => note.includes("58분") },
        { name: "⑰ 줄 선 시각 같음, 줄 하나라 l10이 먼저 서고 l9는 뒤 → 수가 큰 l10", jobs: bothQueued, lanes: [1], want: ["l10", "차례 기다림"] },
        { name: "⑱ 픽스처 14:56 — l56 둘째 시도 단계 사이, 첫 단계의 13:15가 아니라 다음 단계의 예상(실린 과업 행도)",
          jobs: carry(cut(["l56", "l57"], betweenAt), "0-1792475594-run11955-l56r-g1-build", "t-90001"), at: betweenAt, paused: false,
          tasks: [carried("t-90001", betweenAt)], want: ["l56", "차례 기다림"], note: (note) => note.endsWith("다음 단계 14:56 시작 예상"),
          said: (said) => said === "대기 3개 · 14:56 시작 예상", task: "t-90001", why: (why) => why === "l56에 실림 → 차례 기다림 · 14:56 시작 예상" },
        { name: "⑲ 두 레인에 겹침: l61 g2가 나중에 시작해 먼저 통과, l62 g2가 먼저 시작해 늦게 실패 → 늦게 시작한 l61(선택)", jobs: overlapped, want: ["l61", "통과"] },
        { name: "⑳ l61 실패 → 재시도가 줄만 섬 → 시작 예상은 새 시도의 계획(지금), 첫 시도의 08:10이 아님(실린 과업 행도)", jobs: queuedAgain, paused: false,
          tasks: [carried("t-90002", now)], want: ["l61", "차례 기다림"], note: (note) => note.endsWith("09:50 시작 예상") && !note.includes("다음 단계"),
          said: (said) => said === "대기 1개 · 09:50 시작 예상", task: "t-90002", why: (why) => why === "l61에 실림 → 차례 기다림 · 09:50 시작 예상" },
        { name: "㉑ l61 g1 통과 · 남은 단계는 held만 → 시작 모름(지난 09:30이 아님)", jobs: heldOnly, paused: false,
          want: ["l61", "차례 기다림"], note: (note) => note.endsWith("시작 모름"), said: (said) => said === "대기 1개 · 시작 모름" },
      ];
      const choice = { zoom: "now", folded: new Set(), opened: new Set(), log: null };
      return cases.map((one) => {
        const line = { ...base.line, runner_lanes: one.lanes ?? base.line.runner_lanes, paused: one.paused ?? base.line.paused };
        const model = boardScheduleModel({ ...base, line, jobs: one.jobs, lanes: [] }, one.tasks ?? [], one.at ?? now, choice, 900);
        const got = model.board.lines[1];
        const head = model.groups.find((group) => group.id === "gates")?.rows.find((row) => row.key === `gate:${one.want[0]}`) ?? null;
        const task = one.task ? model.groups.find((group) => group.id === "workers")?.rows.find((row) => row.id === one.task) ?? null : null;
        const ok = got.name === one.want[0] && got.word === one.want[1] && (one.note ? one.note(got.note) : true)
          && (one.said ? head !== null && one.said(head.said) : true) && (one.why ? task !== null && one.why(task.why) : true);
        return { name: one.name, want: one.want, got: [got.name, got.word, got.note, head?.said ?? null, task?.why ?? null], ok };
      });
    }, SCHEDULE_NOW);
    ok("the board's gate line follows one rule in all twenty-one cases — running first, then the gate whose latest step began last (a gate or a step run again is its new attempt; on two lanes the later start, not the later end), then the one queued last; a tie is decided, and a waiting gate's row, its line and the task it carries say one present-attempt time, never a past moment as an expected start",
      gateTable.every((row) => row.ok), JSON.stringify(gateTable.filter((row) => !row.ok)));

    /* ---- 비어 있어도 넉 줄: 레인도 게이트도 과업도 없는 답 ---- */
    const emptyAnswer = await page.evaluate((now) => {
      const model = boardScheduleModel({ revision: 1, unchanged: false, read_ms: now, line: { found: false, runner_lanes: [] },
        jobs: [], lanes: [] }, [], now, { zoom: "now", folded: new Set(), opened: new Set(), log: null }, 900, false, null,
      { hourPx: 120, tickPx: 16, leastPx: 24 });
      return { aheadEmpty: model.aheadEmpty, sum: model.board.head.sum, gutter: model.gutter.length, hours: model.hours,
        lines: model.board.lines.map((line) => [line.id, line.big, line.word, line.note, line.act, line.tone]) };
    }, SCHEDULE_NOW);
    ok("with nothing in the range the model's board still has its four lines, each saying there is nothing, and the timetable says nothing is ahead",
      emptyAnswer.aheadEmpty && emptyAnswer.lines[0][1] === "—" && emptyAnswer.lines[0][2] === "잡힌 것 없음" && emptyAnswer.lines[0][3] === "이 범위에 없음"
        && emptyAnswer.lines[1][1] === "—" && emptyAnswer.lines[1][3] === "이 범위에 없음" && emptyAnswer.lines[2][1] === "0건" && emptyAnswer.lines[2][2] === "없음"
        && emptyAnswer.lines[3][1] === "0건" && emptyAnswer.lines[3][3] === "이 범위에 없음" && emptyAnswer.lines.every((line) => line[4] === null && line[5] === "quiet")
        && emptyAnswer.gutter >= 4 && emptyAnswer.hours === 4 && emptyAnswer.sum === "다음 발행 없음 · 착지 대기 0",
      JSON.stringify(emptyAnswer));

    // The same empty answer drawn (review P3-9): the lines, the columns' heads and the empty-future line on the screen.
    const emptyDrawn = await page.evaluate(async (now) => {
      const held = window.__SCHEDULE__;
      const heldDesk = window.__DESK__;
      window.__SCHEDULE__ = { revision: held.revision + 1, unchanged: false, read_ms: now, line: { found: false, runner_lanes: [] }, jobs: [], lanes: [] };
      window.__DESK__ = { ...heldDesk, tasks: [] };
      deskLedger = window.__DESK__;
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the empty answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const host = document.querySelector("#board-view .task-board-schedule");
      const out = {
        lines: [...host.querySelectorAll(".board-schedule-line")].map((line) => [line.dataset.line, line.children[1].textContent, line.children[1].dataset.word, line.children[2].textContent]),
        ahead: host.querySelector(".board-schedule-ahead-empty").checkVisibility(),
        blocks: host.querySelectorAll(".board-schedule-block").length,
        heads: [...host.querySelectorAll(".board-schedule-group-head")].map((head) => head.dataset.sub),
        note: host.querySelector(".board-schedule-note").hidden ? "" : host.querySelector(".board-schedule-note").textContent,
      };
      // A project whose workspace folder holds no queue (t-83513): one sentence, then where it was looked for.
      window.__SCHEDULE__ = { ...window.__SCHEDULE__, revision: held.revision + 2,
        line: { found: false, folder: "/work/other/build-line/build-queue", runner_lanes: [] } };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the answer of a project with no line");
      paintBoardScheduleHosts();
      out.elsewhere = host.querySelector(".board-schedule-note").hidden ? "" : host.querySelector(".board-schedule-note").textContent;
      window.__SCHEDULE__ = { ...held, revision: held.revision + 3 };
      window.__DESK__ = heldDesk;
      deskLedger = heldDesk;
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the restored answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return out;
    }, SCHEDULE_NOW);
    ok("the empty range is drawn, not only modelled: four lines saying nothing, no block, every column head 「이 범위에 없음」, and the empty-future line shown",
      emptyDrawn.lines.length === 4 && emptyDrawn.lines[0][1] === "—" && emptyDrawn.lines[1][1] === "—" && emptyDrawn.lines[2][1] === "0건"
        && emptyDrawn.lines[3][1] === "0건" && emptyDrawn.blocks === 0 && emptyDrawn.heads.every((sub) => sub === "이 범위에 없음") && emptyDrawn.ahead,
      JSON.stringify(emptyDrawn));
    ok("a project with no build line says so in one sentence, and where the line was looked for when the answer names it",
      emptyDrawn.note === "이 프로젝트엔 빌드 줄이 없습니다"
        && emptyDrawn.elsewhere === "이 프로젝트엔 빌드 줄이 없습니다 · /work/other/build-line/build-queue",
      JSON.stringify({ note: emptyDrawn.note, elsewhere: emptyDrawn.elsewhere }));

    /* ---- 좁은 그릇: 안내판 줄은 두 줄로, 시간표는 카드 안에서 옆으로 — 그릇은 넘치지 않고 블록은 닿는다 ---- */
    const narrow = await page.evaluate(async () => {
      const host = document.querySelector("#board-view .task-board-schedule");
      host.style.width = "400px";
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const line = host.querySelector(".board-schedule-line");
      const day = host.querySelector(".board-schedule-day");
      const block = [...host.querySelectorAll(".board-schedule-block")].at(-1);
      block.scrollIntoView({ block: "nearest", inline: "nearest" });
      const seat = day.getBoundingClientRect();
      const box = block.getBoundingClientRect();
      const out = { areas: getComputedStyle(line).gridTemplateAreas, dayScroll: day.scrollWidth, dayClient: day.clientWidth,
        overflowX: getComputedStyle(day).overflowX, overflowY: getComputedStyle(day).overflowY,
        hostScroll: host.scrollWidth, hostClient: host.clientWidth, reached: box.left >= seat.left - 1 && box.right <= seat.right + 1 };
      day.scrollLeft = 0;
      host.style.width = "";
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return out;
    });
    ok("in a narrow holder the board's lines fold to two rows and the timetable scrolls sideways inside its card — the holder never overflows and every block is reachable",
      narrow.areas.includes("note note") && narrow.dayScroll > narrow.dayClient && narrow.hostScroll <= narrow.hostClient + 1 && narrow.reached
        && narrow.overflowX === "auto" && narrow.overflowY === "hidden",
      JSON.stringify(narrow));

    const wheeled = await page.evaluate(async () => {
      const host = document.querySelector("#board-view .task-board-schedule");
      host.style.width = "400px";
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const day = host.querySelector(".board-schedule-day");
      day.scrollLeft = 0;
      // The wheel's scroll is awaited as the holder's own `scroll` event, armed before the wheel — a pass ends at once,
      // only a failure waits out the bound (a fixed wait failed under load, review t-77931).
      window.__scheduleWheelScrolled = new Promise((done) => day.addEventListener("scroll", () => done(day.scrollLeft), { once: true }));
      const box = day.getBoundingClientRect();
      return { x: box.left + 100, y: box.top + 60 };
    });
    await page.mouse.move(wheeled.x, wheeled.y);
    await page.mouse.wheel(300, 0);
    const scrolled = await page.evaluate(async () => {
      const host = document.querySelector("#board-view .task-board-schedule");
      const day = host.querySelector(".board-schedule-day");
      const left = await Promise.race([window.__scheduleWheelScrolled, new Promise((done) => setTimeout(() => done(day.scrollLeft), 5_000))]);
      delete window.__scheduleWheelScrolled;
      day.scrollLeft = 0;
      host.style.width = "";
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return left;
    });
    ok("a person's wheel moves the timetable sideways in a narrow holder", scrolled > 0, String(scrolled));

    /* ---- 키보드: 블록은 단추라 초점이 서고, 서면 할 일과 제 말을 말한다. (Enter의 활성화는 창의 키 처리가
     * 가로채 행도 블록도 시험 창에서 눌리지 않는다 — 블록 이전부터 같았고, 이 과업 밖이다.) ---- */
    const typed = await page.evaluate(() => {
      const block = document.querySelector('#board-view [data-key="job:1792543500-run11955-t-63492-walk"]');
      block.focus();
      return { focused: document.activeElement === block, tag: block.tagName, act: block.dataset.act,
        label: block.getAttribute("aria-label"), ring: getComputedStyle(block).outlineStyle };
    });
    ok("a block takes keyboard focus: a button that says its action and its words",
      typed.focused && typed.tag === "BUTTON" && typed.act === "로그 →" && typed.label.includes("walk") && typed.label.includes("25분"),
      JSON.stringify(typed));

    ok("the schedule raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testBoardSchedule(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${!pass && detail ? `  — ${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
