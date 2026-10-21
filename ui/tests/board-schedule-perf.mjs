/* 일정 탭의 무게 (t-67482) — 50행을 그리는 ms, 요소 수, 조용한 박자의 쓰기 수, 열어 둔 탭의
 * 쉬는 CPU.
 *
 * 「숫자 없는 개선 금지」의 자다. 50행 픽스처(`boardScheduleFiftyRows`)는 결정적이다: 레인 4, 게이트
 * 머리 4와 편 게이트의 일감 10, 줄 18(끝난 12 · 돌고 기다리는 6, 나머지는 수), 워커 과업 14. 시각은
 * 지금을 기준으로 지어, 시계를 고정하지 않고도 같은 그림이 선다 — 쉬는 CPU는 실제 시계(1분마다
 * 「지금」 선을 옮기는 박자까지)로 재야 한다.
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
 *   node ui/tests/board-schedule-perf.mjs --polls 40 --idle-secs 30 --json out.json */
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
export const BOARD_SCHEDULE_BUDGET = Object.freeze({ movedPaintP50Ms: 2.5, elements: 300, idleDeltaPercent: 0.5 });

const option = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : fallback;
};

/* Fifty rows around `now`, run inside the page: the shapes `schedule_timeline` and the desk send. */
export function boardScheduleFiftyRows(now) {
  const minute = 60_000;
  const at = (minutes) => now + minutes * minute;
  const lanes = [
    ...[["aa000001", "1.2.1", -900, -790], ["aa000002", "1.2.2", -600, -490], ["aa000003", "1.2.3", -300, -190]]
      .map(([sha, version, start, end]) => ({
        sha, version, start_ms: at(start), end_ms: at(end), verdict: "green", reason: null, running: false,
        stages: [{ name: "archive", start_ms: at(start), end_ms: at(start + 1), rc: 0, secs: 60 },
          { name: "gate-root", start_ms: at(start + 1), end_ms: at(end), rc: 0, secs: (end - start - 1) * 60 }],
        ahead: [],
      })),
    {
      sha: "aa000004", version: "1.2.4", start_ms: at(-20), end_ms: null, verdict: null, reason: null, running: true,
      stages: [{ name: "archive", start_ms: at(-20), end_ms: at(-19), rc: 0, secs: 60 },
        { name: "gate-root", start_ms: at(-19), end_ms: null, rc: null, secs: null }],
      ahead: [{ name: "gate-root", expected: { secs: 3365, samples: 5 } },
        { name: "gate-zo", expected: { secs: 1764, samples: 5 } }, { name: "build-app", expected: { secs: 1122, samples: 5 } }],
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
      disk_low: false, quiet_wanted: false, runner_lanes: [1, 2], lanes_folder: "/work/release/out" },
    jobs, lanes,
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

async function measure(page, { polls, idleSecs }) {
  await installBoardWaits(page);
  await page.evaluate(({ answer, desk }) => {
    window.__SCHEDULE__ = answer;
    window.__ANSWER__.schedule_timeline = (args = {}) => (args.known === window.__SCHEDULE__.revision
      ? { revision: window.__SCHEDULE__.revision, unchanged: true, read_ms: Date.now(), line: {}, jobs: [], lanes: [] }
      : window.__SCHEDULE__);
    window.__ANSWER__.board_desk = () => desk;
    window.__COLUMNS__ = [];
    window.__PANES__ = [];
    window.__LEDGER__ = [];
    agentBoardMode = "tasks";
    openBoard();
  }, boardScheduleFiftyRows(Date.now()));
  await page.evaluate(async () => {
    const tab = () => document.querySelector('#board-view [data-board-tab-button="schedule"]');
    await window.__UNTIL__(() => tab()?.checkVisibility(), "the schedule's tab");
    tab().click();
    await window.__UNTIL__(() => document.querySelectorAll("#board-view .board-schedule-row").length >= 50, "fifty rows", 5000);
  });
  const paint = await page.evaluate(async (polls) => {
    const view = document.querySelector("#board-view");
    const host = view.querySelector(".task-board-schedule");
    const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    // The first paint of a holder: its elements built, then laid out.
    boardSchedulePainted.delete(host);
    boardScheduleBuilt.delete(host);
    host.replaceChildren();
    await settle();
    let began = performance.now();
    paintBoardScheduleHost(host, view, Date.now());
    host.getBoundingClientRect();
    const first = performance.now() - began;
    const rows = host.querySelectorAll(".board-schedule-row").length;
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
      paintBoardScheduleHost(host, view, Date.now());
      host.getBoundingClientRect();
      moved.push(performance.now() - began);
    }
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
    return { first, rows, elements, moved, quiet, writes: records.length };
  }, polls);
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
  return { ...paint, idleOpenPercent: open, idleClosedPercent: closed, idleDeltaPercent: open - closed };
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
      summary.rows < 50 && `rows ${summary.rows} < 50`,
      summary.elements > budget.elements && `elements ${summary.elements} > ${budget.elements}`,
      summary.movedPaintP50Ms > budget.movedPaintP50Ms
        && `a moved beat paints in ${summary.movedPaintP50Ms} ms > ${budget.movedPaintP50Ms} ms`,
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
