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
    await window.__UNTIL__(() => document.querySelector("#board-view .board-schedule-row"), "the schedule's rows", 5000);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/* What the schedule drew: per group its rows in DOM order, each with its bar's
 * place and its words; the chart's 「now」; the asks it made. */
export async function readBoardSchedule(page) {
  return page.evaluate(() => {
    const host = document.querySelector("#board-view .task-board-schedule");
    const chart = host.querySelector(".board-schedule-chart");
    const groups = {};
    for (const box of host.querySelectorAll(".board-schedule-group")) {
      const head = box.querySelector(".board-schedule-group-head");
      groups[box.dataset.group] = {
        hidden: box.hidden,
        expanded: head.getAttribute("aria-expanded"),
        head: head.textContent,
        more: box.querySelector(".board-schedule-more").hidden ? "" : box.querySelector(".board-schedule-more").textContent,
        moreExpanded: box.querySelector(".board-schedule-more").getAttribute("aria-expanded"),
        rows: [...box.querySelectorAll(".board-schedule-row")].map((row) => {
          const bar = row.querySelector(".board-schedule-bar");
          return {
            key: row.dataset.key, state: row.dataset.barState, basis: row.dataset.basis, depth: row.dataset.depth,
            expanded: row.getAttribute("aria-expanded"), paused: row.classList.contains("is-paused"),
            label: row.querySelector(".board-schedule-label").textContent,
            // The row's sentence; before the draft's cells (t-71840) it was the words cell's own text.
            said: row.dataset.said ?? row.querySelector(".board-schedule-said").textContent,
            // The passed draft's cells (t-71840): the line under the name, the end cell's two lines, the bar's two names.
            sub: row.querySelector(".board-schedule-label").dataset.sub ?? "",
            eta: row.querySelector(".board-schedule-said").textContent,
            etaSub: row.querySelector(".board-schedule-said").dataset.sub ?? "",
            nowLabel: bar.dataset.nowLabel ?? "",
            aheadLabel: bar.dataset.aheadLabel ?? "",
            from: Number(bar.style.getPropertyValue("--from")), to: Number(bar.style.getPropertyValue("--to")),
            done: Number(bar.style.getPropertyValue("--done")),
          };
        }),
      };
    }
    return {
      groups,
      title: host.querySelector(".board-schedule-title")?.textContent ?? "",
      chip: host.querySelector(".board-schedule-now")?.textContent ?? "",
      now: Number(chart.style.getPropertyValue("--now")),
      ticks: [...host.querySelectorAll(".board-schedule-tick")].map((tick) => tick.textContent),
      note: host.querySelector(".board-schedule-note").hidden ? "" : host.querySelector(".board-schedule-note").textContent,
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
        && document.querySelector("#board-view .board-schedule-row"), "the schedule again");
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
    ok("the gate in play stands open, running, its four jobs under it",
      l61?.state === "running" && l61.expanded === "true"
        && gates.slice(gates.indexOf(l61) + 1, gates.indexOf(l61) + 5).every((row) => row.depth === "1" && row.key.includes("-l61")),
      JSON.stringify(gates.map((row) => [row.key, row.depth, row.state])));
    const l60 = heads.find((row) => row.key === "gate:l60");
    ok("this morning's l60 stands folded and passed — its window measurement failed once and passed on its rerun",
      l60?.expanded === "false" && l60.state === "done" && /^통과 /.test(l60.said), JSON.stringify(l60));
    const lint = gates.find((row) => row.key === "job:0-1792543560-run11955-l61z-g3a-zo-lint");
    ok("a running job past its median says how late it is and reaches 「now」",
      lint?.state === "running" && lint.said === "+1분 지연 중" && near(lint.to, drawn.now), JSON.stringify(lint));
    const held = gates.find((row) => row.key.includes("l61q-g5-window"));
    ok("a job a guard held says so, and its bar is not a start the runner will make",
      held?.state === "held" && held.said.includes("가드"), JSON.stringify(held));

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
    await page.evaluate(() => document.querySelector('#board-view [data-key="gate:l60"]').click());
    drawn = await readBoardSchedule(page);
    const opened = drawn.groups.gates.rows;
    const at60 = opened.findIndex((row) => row.key === "gate:l60");
    ok("a folded gate opens on a press, its jobs under it",
      opened[at60]?.expanded === "true" && opened[at60 + 1]?.key.includes("-l60") && opened[at60 + 1]?.depth === "1"
        && drawn.groups.line.rows.length > 0,
      JSON.stringify(opened.slice(at60, at60 + 3)));

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
          const boxes = [...host.querySelectorAll(".board-schedule-tick")]
            .map((tick) => tick.getBoundingClientRect()).sort((a, b) => a.left - b.left);
          return boxes.slice(1).map((box, at) => Math.round(box.left - boxes[at].right));
        }, { zoom, width });
        if (gaps.length === 0 || gaps.some((gap) => gap <= 4)) crowded.push({ zoom, width, gaps });
      }
    }
    ok("the axis's labels stand apart in 1280px and 640px holders, a week or a day wide", crowded.length === 0,
      JSON.stringify(crowded));
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
        && drawn.chip === "지금" && drawn.title === "지금 도는 일",
      JSON.stringify({ ticks: drawn.ticks, chip: drawn.chip, title: drawn.title, asked: drawn.asked.slice(-2) }));
    // The draft's chip stands beside 「10:00」, never on it: an hour the chip would sit on keeps its line on the
    // track and hides its words, and every hour's words that show stand clear of the chip.
    const beside = await page.evaluate(() => {
      const host = document.querySelector("#board-view .task-board-schedule");
      const chip = host.querySelector(".board-schedule-now").getBoundingClientRect();
      return [...host.querySelectorAll(".board-schedule-tick")].map((tick) => {
        const box = tick.getBoundingClientRect();
        return { label: tick.textContent, shown: getComputedStyle(tick).visibility !== "hidden",
          apart: Math.round(Math.max(chip.left - box.right, box.left - chip.right)) };
      });
    });
    ok("the 「지금」 chip covers no hour's words: an hour under it keeps its line and hides its label",
      beside.filter((one) => one.shown).length >= 3 && beside.every((one) => !one.shown || one.apart > 4),
      JSON.stringify(beside));
    const legend = await page.evaluate(() => document.querySelector("#board-view .board-schedule-legend")?.textContent ?? "");
    ok("the card's head says how a bar reads, in the draft's words, and every group is named alone",
      legend === "막대 끝 = 지난 소요로 잰 예상 · 빗금 = 아직 안 온 구간 · 점선 = 근거 없음"
        && drawn.groups.lanes.head === "발행" && drawn.groups.gates.head === "게이트"
        && drawn.groups.line.head === "빌드 줄" && drawn.groups.workers.head === "워커 과업",
      JSON.stringify({ legend, heads: Object.values(drawn.groups).map((group) => group.head) }));
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
      if (!host.querySelector(".board-schedule-legend")) return { sizes: [], dark: { drawn: NaN }, bright: { drawn: NaN } };
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
      const pick = (selector, pseudo = null) => getComputedStyle(host.querySelector(selector), pseudo);
      const read = () => {
        const deck = getComputedStyle(host.querySelector(".board-schedule-chart")).backgroundColor;
        const band = pick(".board-schedule-group-head").backgroundColor;
        return {
          name: ratio(pick(".board-schedule-label").color, deck),
          sub: ratio(pick(".board-schedule-label", "::after").color, deck),
          end: ratio(pick(".board-schedule-said").color, deck),
          endSub: ratio(pick(".board-schedule-said", "::after").color, deck),
          legend: ratio(pick(".board-schedule-legend").color, deck),
          group: ratio(pick(".board-schedule-group-head").color, band),
          tick: ratio(pick(".board-schedule-tick").color, band),
          chip: ratio(pick(".board-schedule-now").color, pick(".board-schedule-now").backgroundColor),
          running: ratio(pick(".board-schedule-bar", "::before").color, fill("--schedule-run-fill")),
          done: ratio(pick(".board-schedule-bar", "::before").color, fill("--schedule-done-fill")),
          halted: ratio(pick(".board-schedule-bar", "::before").color, fill("--schedule-halt-fill")),
          ahead: ratio(pick(".board-schedule-bar", "::after").color, fill("--surface-well")),
        };
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
