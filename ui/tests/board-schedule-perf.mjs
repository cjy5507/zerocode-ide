/* 일정 탭의 무게 (t-67482) — 50행을 그리는 ms, 요소 수, 조용한 박자의 쓰기 수, 열어 둔 탭의
 * 쉬는 CPU.
 *
 * 「숫자 없는 개선 금지」의 자다. 50행 픽스처(`boardScheduleFiftyRows`)는 결정적이다: 빌드 실행 4, 게이트
 * 머리 4와 편 게이트의 일감 10, 줄 18(끝난 12 · 돌고 기다리는 6, 나머지는 수), 워커 과업 14. 시각은
 * 지금을 기준으로 지어, 시계를 고정하지 않고도 같은 그림이 선다 — 쉬는 CPU는 실제 시계(1분마다
 * 「지금」 선을 옮기는 박자까지)로 재야 한다.
 *
 * 그림은 둘이다: 「하루」는 안내판과 세로 시간표(t-77628, 블록 — 게이트의 일감은 블록 안의 단계 칸이라 50행
 * 픽스처가 블록 40개로 선다), 「한 주」는 일곱 칸 달력(t-84802, 칸마다 넘치는 블록은 수로 접힌다). 둘 다 재고 둘 다
 * 예산 안이어야 한다. 일이 많은 주(빌드 실행 50, 블록 500 — `boardScheduleHeavyWeek`)는 달력을 따로 잰다: 요소는 같은
 * 예산, 다시 그리기는 한 프레임의 예산이다.
 *
 * 네 수:
 *   1. 그림 ms — 그릇을 처음 그리는 한 번(요소를 짓는다)과, 답이 움직인 박자의 다시 그리기(행
 *      하나의 상태가 바뀜), 각각 그 뒤의 강제 레이아웃까지. 다시 그리기는 p50·p95.
 *   2. 요소 수 — 그릇 안의 모든 요소.
 *   3. 조용한 박자의 쓰기 수 — `unchanged` 답 뒤의 MutationObserver 기록. 0이어야 한다.
 *   4. 쉬는 CPU — 「일정」 탭을 연 채 N초와 「지금」 탭으로 N초, 렌더러의 TaskDuration(CDP) 차이를
 *      벽시계로 나눈 백분율: 5초 박자와 1분 시계가 드는 값이다. 백엔드의 몫(명령 한 번)은 러스트 시험
 *      `schedule_timeline::tests::a_look_costs_little_when_nothing_moved`가 잰다.
 *
 * 저사양 프로필(tools/low-spec/profile.mjs의 두 단계):
 *   taskpolicy -b node ui/tests/board-schedule-perf.mjs            효율 코어·배경 QoS (low)
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/board-schedule-perf.mjs    페이지 CPU 4배 감속 (cpu4x)
 *   node ui/tests/board-schedule-perf.mjs --polls 40 --idle-secs 30 --json out.json
 *   node ui/tests/board-schedule-perf.mjs --line-jobs 860   더 긴 빌드 줄: 답의 일감 상한을 정할 때 잰다 */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { median, quantile } from "./coordinator-desk-perf.mjs";
import { loadNote } from "./machine-load.mjs";

/* 예산: 이 선을 넘으면 무게 시험이 떨어진다. 다시 그리기는 저사양(taskpolicy -b)의 p50으로
 * 판정하고 p95는 기록만 한다 — 코디 10-21 결정(m-68953), 저사양 p50 2.2 ms 실측. 요소 수는
 * 장면 시험(`board-schedule.mjs`)도 이 값으로 잰다. */
export const BOARD_SCHEDULE_BUDGET = Object.freeze({ movedPaintP50Ms: 2.5, elements: 300, idleDeltaPercent: 0.5,
  // A busy week (fifty build runs, five hundred blocks) moves within one frame of the 60 Hz screen, on the low-end profile.
  heavyMovedPaintP50Ms: 16,
  // A press answered within about six frames reads as immediate; one page of a busy day stands inside it.
  heavyPressMs: 100 });

const option = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : fallback;
};

/* Fifty rows around `now`, run inside the page: the shapes `schedule_timeline` and the desk send. */
export function boardScheduleFiftyRows(now) {
  const minute = 60_000;
  const at = (minutes) => now + minutes * minute;
  const release = { id: "release", name: { ko: "새 버전 발행" } };
  const step = (name, ko, usualSecs, extra) => ({ name, words: { ko }, expected: { secs: usualSecs, samples: 5 }, ...extra });
  const builds = [
    ...[["aa000001", "1.2.1", -900, -790], ["aa000002", "1.2.2", -600, -490], ["aa000003", "1.2.3", -300, -190]]
      .map(([sha, version, start, end]) => ({
        ...release, label: `v${version}`, sha, started_ms: at(start), updated_ms: at(end), state: "passed",
        steps: [step("archive", "소스 묶기", 60, { state: "done", secs: 60 }),
          step("gate-root", "결합 시험", 3365, { state: "done", secs: (end - start - 1) * 60 })],
      })),
    {
      ...release, label: "v1.2.4", sha: "aa000004", started_ms: at(-20), running_since_ms: at(-19), updated_ms: at(-1),
      state: "running",
      steps: [step("archive", "소스 묶기", 60, { state: "done", secs: 60 }), step("gate-root", "결합 시험", 3365, { state: "running" }),
        step("gate-zo", "zo 시험", 1764, { state: "ahead" }), step("build-app", "앱 빌드", 1122, { state: "ahead" })],
    },
  ];
  const job = (name, state, start, end, extra = {}) => ({
    name, who: extra.who ?? "run1-l70r", state, class: "first", lane: extra.lane ?? 1, cap_secs: 1800,
    queued_ms: at((start ?? 0) - 5), start_ms: start == null ? null : at(start), end_ms: end == null ? null : at(end),
    secs: start != null && end != null ? (end - start) * 60 : null, rc: state === "passed" ? 0 : state === "failed" ? 1 : null,
    kind: null, gate: null, part: null, what: null, expected: null, ...extra,
  });
  const jobs = [];
  for (const [gate, start] of [["l70", -700], ["l71", -500], ["l72", -300]]) {
    for (const step of ["g1-build", "g2-tests", "g3a-zo-lint"]) {
      jobs.push(job(`0-1792${start}-run1-${gate}r-${step}`, "passed", start, start + 20,
        { gate, part: "r", kind: step, who: `run1-${gate}r` }));
    }
  }
  const open = [["g1-build", "passed", -60, -50], ["g1w-wry-clippy", "passed", -50, -45], ["g2-tests", "running", -45, null],
    ["g3a-zo-lint", "running", -10, null], ["g3b1-zo-test-runtime", "waiting", null, null], ["g3b2-zo-test-tools", "waiting", null, null],
    ["g3b3-zo-test-rest", "waiting", null, null], ["g3w-zo-win-check", "waiting", null, null], ["g4-zo-e2e", "waiting", null, null],
    ["g5-window-실측", "held", null, null]];
  for (const [index, [step, state, start, end]] of open.entries()) {
    jobs.push(job(`0-17925${index}-run1-l73z-${step}`, state, start, end, {
      gate: "l73", part: "z", kind: step, who: `run1-l73${index % 2 ? "z" : "r"}`, lane: index % 2 ? 2 : 1,
      expected: ["running", "waiting", "held"].includes(state) ? { secs: 600, samples: 5 } : null,
    }));
  }
  for (let index = 0; index < 20; index += 1) {
    jobs.push(job(`17926${index}-w${index}-t${index}-red`, index % 5 === 0 ? "failed" : "passed",
      -400 + index * 15, -390 + index * 15, { who: `w${index}-t${index}`, class: "plain" }));
  }
  jobs.push(job("17927-w90-t90-walk", "running", -15, null, { who: "w90-t90", class: "plain", expected: { secs: 1800, samples: 0 } }));
  jobs.push(job("17928-w91-t91-red", "running", -5, null, { who: "w91-t91", class: "plain", lane: 2, expected: { secs: 900, samples: 0 } }));
  for (const [index, state] of ["waiting", "waiting", "waiting", "held"].entries()) {
    jobs.push(job(`17929${index}-w9${index + 2}-t9${index + 2}-green`, state, null, null,
      { who: `w9${index + 2}-t9${index + 2}`, class: index === 2 ? "last" : "plain", expected: { secs: 300, samples: 0 } }));
  }
  const answer = {
    revision: 7, unchanged: false, read_ms: now,
    line: { folder: "/work/build-line/build-queue", found: true, paused: false, pause_note: null, disk_floor_gib: 12,
      disk_low: false, quiet_wanted: false, runner_lanes: [1, 2] },
    jobs, builds,
  };
  const stages = ["merged", "merged", "verified", "reported", "dispatched"];
  const tasks = Array.from({ length: 14 }, (_, index) => {
    const stage = stages[index % stages.length];
    const start = -800 + index * 50;
    return {
      run: "run-perf", id: `t-${100 + index}`, title: `일정 무게 과업 ${index + 1}`, stage, gate: null, blocked_by: [],
      closed: null, created_ms: at(start - 5), cost: null, writing: null,
      times: {
        started_ms: at(start), handed_ms: stage === "dispatched" ? null : at(start + 30),
        verified_ms: ["merged", "verified"].includes(stage) ? at(start + 40) : null,
        landed_ms: stage === "merged" ? at(start + 45) : null,
      },
    };
  });
  const desk = { runs: [{ run: "run-perf", name: "무게", seat: false }], tasks, stages: [], mail: [], news: [],
    counts: { mail: 0, news: 0, folded: 0 }, folded_batches: [], land_batches: [] };
  return { answer, desk };
}

/* A busy week (t-84802), the same shapes: fifty build runs, ten gates of five jobs, 140 line jobs and three hundred worker
 * tasks — five hundred blocks — spread from `weekFrom` to `now`, the past five cells and today, one task in five over several
 * days. `lineJobs` asks for a longer build line, the length the answer's job cap is measured at. */
export function boardScheduleHeavyWeek(now, weekFrom, lineJobs = 140) {
  const hour = 3_600_000;
  const at = (index, count) => weekFrom + Math.floor((index / count) * (now - weekFrom)) + 600_000;
  const past = (ms) => Math.min(ms, now - 60_000);
  const builds = Array.from({ length: 50 }, (_, index) => {
    const start = past(at(index, 50));
    const end = Math.min(start + 2 * hour, now - 1);
    return { id: "release", name: { ko: "새 버전 발행" }, label: `v2.0.${index}`, sha: `bb${String(index).padStart(6, "0")}`,
      started_ms: start, updated_ms: end, state: "passed",
      steps: [{ name: "gate-root", words: { ko: "결합 시험" }, state: "done", secs: 7_200, expected: { secs: 7_200, samples: 5 } }] };
  });
  const job = (name, state, start, end, extra = {}) => ({
    name, who: extra.who ?? "run1", state, class: "plain", lane: 1, cap_secs: 1800, queued_ms: start - 60_000, start_ms: start,
    end_ms: end, secs: end ? (end - start) / 1000 : null, rc: state === "passed" ? 0 : state === "failed" ? 1 : null, kind: null,
    gate: null, part: null, what: null, expected: null, ...extra,
  });
  const jobs = [];
  for (let gate = 0; gate < 10; gate += 1) {
    for (let step = 0; step < 5; step += 1) {
      const start = past(at(gate, 10) + step * 600_000);
      jobs.push(job(`0-${gate}${step}-run1-l${80 + gate}z-g${step}`, "passed", start, start + 500_000,
        { gate: `l${80 + gate}`, part: "z", kind: `g${step}`, who: `run1-l${80 + gate}z` }));
    }
  }
  for (let index = 0; index < lineJobs; index += 1) {
    const start = past(at(index, lineJobs));
    jobs.push(job(`3${index}-w${index}-t${index}-red`, index % 7 === 0 ? "failed" : "passed", start, start + 300_000, { who: `w${index}` }));
  }
  const stages = ["merged", "merged", "merged", "verified", "reported", "dispatched"];
  const tasks = Array.from({ length: 300 }, (_, index) => {
    const stage = stages[index % stages.length];
    const start = past(at(index, 300));
    const end = Math.min(start + (index % 5 === 0 ? 40 : 3) * hour, now - 1);
    return {
      run: "run-heavy", id: `t-${5000 + index}`, title: `바쁜 주 과업 ${index + 1}`, stage, gate: null, blocked_by: [], closed: null,
      created_ms: start - 60_000, cost: null, writing: null,
      times: { started_ms: start, handed_ms: stage === "dispatched" ? null : end, verified_ms: ["merged", "verified"].includes(stage) ? end : null,
        landed_ms: stage === "merged" ? end : null, ended_ms: null, release: null },
    };
  });
  const answer = { revision: 9000, unchanged: false, read_ms: now,
    line: { folder: "/work/build-line/build-queue", found: true, paused: false, pause_note: null, disk_floor_gib: 12, disk_low: false,
      quiet_wanted: false, runner_lanes: [1, 2] }, jobs, builds };
  return { answer, desk: { runs: [{ run: "run-heavy", name: "바쁜 주", seat: false }], tasks, stages: [], mail: [], news: [],
    counts: { mail: 0, news: 0, folded: 0 }, folded_batches: [], land_batches: [] } };
}

async function measure(page, { polls, idleSecs }) {
  await installBoardWaits(page);
  await page.evaluate(({ answer, desk }) => {
    window.__SCHEDULE__ = answer;
    window.__ANSWER__.schedule_timeline = (args = {}) => (args.known === window.__SCHEDULE__.revision
      ? { revision: window.__SCHEDULE__.revision, unchanged: true, read_ms: Date.now(), line: {}, jobs: [], builds: [] }
      : window.__SCHEDULE__);
    window.__ANSWER__.board_desk = () => desk;
    window.__COLUMNS__ = [];
    window.__PANES__ = [];
    window.__LEDGER__ = [];
    agentBoardMode = "tasks";
    // The fifty rows span the day before now; the tab opens on 「지금 앞뒤」 (t-71840), so the weight is read a day wide.
    boardScheduleChoice.zoom = "day";
    openBoard();
  }, boardScheduleFiftyRows(Date.now()));
  await page.evaluate(async () => {
    const tab = () => document.querySelector('#board-view [data-board-tab-button="schedule"]');
    await window.__UNTIL__(() => tab()?.checkVisibility(), "the schedule's tab");
    tab().click();
    await window.__UNTIL__(() => document.querySelectorAll("#board-view .board-schedule-block").length >= 40, "forty blocks", 5000);
  });
  // One drawing's weight: the first paint and the moved beats, in the zoom asked for.
  const paintFor = (zoom, least) => page.evaluate(async ({ polls, zoom, least }) => {
    const view = document.querySelector("#board-view");
    const host = view.querySelector(".task-board-schedule");
    const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    if (boardScheduleChoice.zoom !== zoom) {
      document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`).click();
      await window.__UNTIL__(() => !boardScheduleAsking, "the zoom's answer");
      await window.__UNTIL__(() => host.querySelectorAll(".board-schedule-row, .board-schedule-block").length >= least, `${least} rows`, 5000);
    }
    // The busy week is read as of its last day, when every cell holds its work: `__WEEK_NOW__` stands for the clock.
    const clock = () => window.__WEEK_NOW__ ?? Date.now();
    // The first paint of a holder: its elements built, then laid out.
    boardSchedulePainted.delete(host);
    boardScheduleBuilt.delete(host);
    host.replaceChildren();
    await settle();
    let began = performance.now();
    paintBoardScheduleHost(host, view, clock());
    host.getBoundingClientRect();
    const first = performance.now() - began;
    const rows = host.querySelectorAll(".board-schedule-row, .board-schedule-block").length;
    const elements = host.querySelectorAll("*").length;
    // A beat whose answer moved: one row's state, then layout.
    const moved = [];
    for (let poll = 0; poll < polls; poll += 1) {
      await settle();
      const held = window.__SCHEDULE__;
      const jobs = held.jobs.map((one, index) => index === poll % held.jobs.length && one.state === "passed"
        ? { ...one, state: "failed", rc: 1 } : one);
      window.__SCHEDULE__ = { ...held, revision: held.revision + 1, jobs };
      boardScheduleAnswer = window.__SCHEDULE__;
      began = performance.now();
      paintBoardScheduleHost(host, view, clock());
      host.getBoundingClientRect();
      moved.push(performance.now() - began);
    }
    return { first, rows, elements, moved };
  }, { polls, zoom, least });
  const week = await paintFor("week", 1);
  const day = await paintFor("day", 40);
  const paint = await page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    const host = view.querySelector(".task-board-schedule");
    const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    // A quiet beat: the command says unchanged, and nothing is written. Kept off a minute's turn,
    // when the view moves its 「now」 line on purpose.
    while (Date.now() % 60_000 > 55_000) await new Promise((done) => setTimeout(done, 200));
    paintBoardScheduleHost(host, view, Date.now());
    await settle();
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(host, { subtree: true, childList: true, attributes: true, characterData: true });
    began = performance.now();
    refreshBoardSchedule();
    await new Promise((done) => setTimeout(done, 0));
    await window.__UNTIL__(() => !boardScheduleAsking, "the quiet answer");
    const quiet = performance.now() - began;
    await settle();
    records.push(...watch.takeRecords());
    watch.disconnect();
    return { quiet, writes: records.length };
  });
  // The idle cost: the schedule's tab open, then the board's 「now」 tab, each for idleSecs.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const taskSeconds = async () => (await cdp.send("Performance.getMetrics")).metrics
    .find((metric) => metric.name === "TaskDuration").value;
  const idle = async () => {
    const [cpu0, wall0] = [await taskSeconds(), Date.now()];
    await new Promise((done) => setTimeout(done, idleSecs * 1000));
    const [cpu1, wall1] = [await taskSeconds(), Date.now()];
    return ((cpu1 - cpu0) / ((wall1 - wall0) / 1000)) * 100;
  };
  const open = await idle();
  await page.evaluate(() => document.querySelector('#board-view [data-board-tab-button="now"]').click());
  const closed = await idle();
  // The busy week, last: the same drawing, fifty build runs and five hundred blocks in the answer and the desk, from the
  // first cell to now, the middle of the sixth cell.
  const weekFrom = await page.evaluate(() => boardScheduleWeekBoundaries(Date.now())[0]);
  const weekNow = weekFrom + 5.5 * 86_400_000;
  await page.evaluate(({ answer, desk, weekNow }) => {
    window.__WEEK_NOW__ = weekNow;
    window.__SCHEDULE__ = answer;
    window.__ANSWER__.board_desk = () => desk;
    deskLedger = desk;
    boardScheduleAnswer = answer;
    document.querySelector('#board-view [data-board-tab-button="schedule"]').click();
    boardScheduleChoice.zoom = "day";
  }, { ...boardScheduleHeavyWeek(weekNow, weekFrom, Number(option("line-jobs", 140))), weekNow });
  const heavy = await paintFor("week", 1);
  // What one changed answer costs the page before any drawing: its size and reading it.
  heavy.answer = await page.evaluate(() => {
    const text = JSON.stringify(window.__SCHEDULE__);
    const runs = [];
    for (let run = 0; run < 15; run += 1) {
      const began = performance.now();
      JSON.parse(text);
      runs.push(performance.now() - began);
    }
    return { bytes: text.length, parseMs: runs.sort((a, b) => a - b)[7] };
  });
  // A press on the busiest day's 「+N」 stands one page of it; the old chart has no such press.
  heavy.expand = await page.evaluate(async () => {
    const more = [...document.querySelectorAll("#board-view .board-schedule-week-day .board-schedule-more:not(.board-schedule-week-less)")]
      .filter((one) => !one.hidden).sort((a, b) => parseInt(b.textContent.slice(1), 10) - parseInt(a.textContent.slice(1), 10))[0];
    if (!more) return null;
    const host = document.querySelector("#board-view .task-board-schedule");
    const began = performance.now();
    more.click();
    host.getBoundingClientRect();
    const ms = performance.now() - began;
    return { ms, elements: host.querySelectorAll("*").length };
  });
  return { ...day, week, heavy, ...paint, idleOpenPercent: open, idleClosedPercent: closed, idleDeltaPercent: open - closed };
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const polls = Number(option("polls", 40));
  const idleSecs = Number(option("idle-secs", 30));
  const json = option("json", null);
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, faults } = await openWindowTestPage(browser, origin);
    const result = await measure(page, { polls, idleSecs });
    await page.close();
    const budget = BOARD_SCHEDULE_BUDGET;
    const summary = {
      throttle: Number(process.env.WINDOW_CPU_THROTTLE ?? 1),
      load: loadNote(),
      budget,
      rows: result.rows,
      elements: result.elements,
      firstPaintMs: Number(result.first.toFixed(2)),
      movedPaintP50Ms: Number(median(result.moved).toFixed(2)),
      movedPaintP95Ms: Number(quantile(result.moved, 0.95).toFixed(2)),
      // The week's calendar, beside the day's timetable (blocks) the fields above read, and the busy week.
      week: {
        rows: result.week.rows,
        elements: result.week.elements,
        firstPaintMs: Number(result.week.first.toFixed(2)),
        movedPaintP50Ms: Number(median(result.week.moved).toFixed(2)),
        movedPaintP95Ms: Number(quantile(result.week.moved, 0.95).toFixed(2)),
      },
      heavyWeek: {
        rows: result.heavy.rows,
        elements: result.heavy.elements,
        firstPaintMs: Number(result.heavy.first.toFixed(2)),
        movedPaintP50Ms: Number(median(result.heavy.moved).toFixed(2)),
        movedPaintP95Ms: Number(quantile(result.heavy.moved, 0.95).toFixed(2)),
        expandPaintMs: result.heavy.expand ? Number(result.heavy.expand.ms.toFixed(2)) : null,
        expandedElements: result.heavy.expand?.elements ?? null,
        answerBytes: result.heavy.answer.bytes,
        answerParseMs: Number(result.heavy.answer.parseMs.toFixed(2)),
      },
      quietBeatMs: Number(result.quiet.toFixed(2)),
      quietWrites: result.writes,
      idleOpenPercent: Number(result.idleOpenPercent.toFixed(3)),
      idleClosedPercent: Number(result.idleClosedPercent.toFixed(3)),
      idleDeltaPercent: Number(result.idleDeltaPercent.toFixed(3)),
      faults: faults.length,
    };
    console.log(JSON.stringify(summary, null, 2));
    if (json) writeFileSync(json, `${JSON.stringify(summary, null, 2)}\n`);
    const fail = [
      summary.rows < 40 && `day blocks ${summary.rows} < 40`,
      summary.week.rows < 1 && "the week drew no block",
      summary.heavyWeek.elements > budget.elements && `busy week elements ${summary.heavyWeek.elements} > ${budget.elements}`,
      summary.heavyWeek.expandPaintMs > budget.heavyPressMs
        && `a press on a busy day takes ${summary.heavyWeek.expandPaintMs} ms > ${budget.heavyPressMs} ms`,
      summary.heavyWeek.movedPaintP50Ms > budget.heavyMovedPaintP50Ms
        && `a moved beat of the busy week paints in ${summary.heavyWeek.movedPaintP50Ms} ms > ${budget.heavyMovedPaintP50Ms} ms`,
      summary.elements > budget.elements && `day elements ${summary.elements} > ${budget.elements}`,
      summary.week.elements > budget.elements && `week elements ${summary.week.elements} > ${budget.elements}`,
      summary.movedPaintP50Ms > budget.movedPaintP50Ms
        && `a moved beat of the day view paints in ${summary.movedPaintP50Ms} ms > ${budget.movedPaintP50Ms} ms`,
      summary.week.movedPaintP50Ms > budget.movedPaintP50Ms
        && `a moved beat of the week view paints in ${summary.week.movedPaintP50Ms} ms > ${budget.movedPaintP50Ms} ms`,
      summary.quietWrites !== 0 && `a quiet beat wrote ${summary.quietWrites} times`,
      summary.idleDeltaPercent > budget.idleDeltaPercent
        && `the open view costs ${summary.idleDeltaPercent} % CPU idle > ${budget.idleDeltaPercent} %`,
      summary.faults !== 0 && `${summary.faults} browser errors`,
    ].filter(Boolean);
    for (const line of fail) console.log(`FAIL  ${line}`);
    if (fail.length) process.exitCode = 1;
  } finally {
    await browser.close();
    files.close();
  }
}
