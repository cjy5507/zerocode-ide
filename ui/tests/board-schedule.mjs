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
        rows: [...box.querySelectorAll(".board-schedule-row")].map((row) => {
          const bar = row.querySelector(".board-schedule-bar");
          return {
            key: row.dataset.key, state: row.dataset.barState, basis: row.dataset.basis, depth: row.dataset.depth,
            expanded: row.getAttribute("aria-expanded"), paused: row.classList.contains("is-paused"),
            marks: row.dataset.marks ?? "",
            label: row.querySelector(".board-schedule-label").textContent,
            said: row.querySelector(".board-schedule-said").textContent,
            from: Number(bar.style.getPropertyValue("--from")), to: Number(bar.style.getPropertyValue("--to")),
            done: Number(bar.style.getPropertyValue("--done")),
          };
        }),
      };
    }
    return {
      groups,
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
      lanes.map((row) => row.label).join(",") === "v1.1.82,v1.1.83", JSON.stringify(lanes.map((row) => row.label)));
    const running = answer.lanes.find((run) => run.running);
    const current = running.stages[running.stages.length - 1];
    const due = current.start_ms + running.ahead[0].expected.secs * 1000;
    const laneEnd = Math.max(SCHEDULE_NOW, due) + running.ahead.slice(1).reduce((sum, stage) => sum + stage.expected.secs * 1000, 0);
    const lane = lanes.find((row) => row.label === "v1.1.83");
    ok("the running lane's bar ends where its stages' medians end: the root gate's from its start, then each stage after it",
      lane?.state === "running" && near(lane.to, placed(laneEnd, from, to)) && near(lane.from, placed(running.start_ms, from, to)),
      JSON.stringify({ lane, laneEnd: new Date(laneEnd).toISOString() }));
    ok("v1.1.83 is expected to end near 11:34 KST, and says the stage it is in and the time left",
      Math.abs(laneEnd - (SCHEDULE_NOW + 104 * 60_000)) <= 5 * 60_000 && lane?.said.includes("gate-root"),
      JSON.stringify({ end: new Date(laneEnd).toISOString(), said: lane?.said }));
    const ended = lanes.find((row) => row.label === "v1.1.82");
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
    ok("a landed task's bar runs from its first start to its landing, its hand-in and verification marked on it",
      landed?.state === "done" && near(landed.from, placed(landedTask.times.started_ms, from, to))
        && near(landed.to, placed(landedTask.times.landed_ms, from, to)) && landed.marks === "handed verified",
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
        && drawn.groups.lanes.rows.filter((row) => row.label === "v1.1.77").length === 3,
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
    const lateLane = drawn.groups.lanes.rows.find((row) => row.label === "v1.1.83");
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
    const busyLane = busyLoad.groups.lanes.rows.find((row) => row.label === "v1.1.83");
    const quietLane = quietLoad.groups.lanes.rows.find((row) => row.label === "v1.1.83");
    const endedLane = busyLoad.groups.lanes.rows.find((row) => row.label === "v1.1.82");
    ok("with the load over the cores a lane still ahead says so beside its estimate, and its bar does not move",
      busyLane?.said === `${quietLane?.said} · 부하 높음·늦어질 수 있음` && near(busyLane.to, quietLane.to)
        && !endedLane?.said.includes("부하"),
      JSON.stringify({ busyLane, quietLane, endedLane }));
    await loudRead(false);

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
