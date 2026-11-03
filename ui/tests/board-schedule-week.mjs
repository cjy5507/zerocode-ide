/* 일정의 「한 주」 (t-84802) — 가로 차트가 아니라 요일별 일곱 칸 달력으로 서는가.
 *
 * 사람이 통과시킨 일정 B의 둘째 판이다. 잰다: 5일 전부터 내일까지 칸 일곱과 요일 머리와 오늘, 칸마다 그날의 레인·게이트·과업 블록,
 * 칸의 상태 순서, 칸을 넘는 블록의 수와 펴기, 날을 넘는 일의 이어짐, 블록의 누름과 Enter, 실린 과업, 안내판, 범위 선택, 시간대,
 * 자정을 넘는 창, 답이 싣지 못한 일감의 말, 다섯 언어의 요일 이름, 좁은 폭의 쌓임, 다크·라이트, 요소 수, 조용한 박자, 키보드, axe.
 *
 *   node ui/tests/board-schedule-week.mjs */
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { boardScheduleFixture, installBoardSchedule, openBoardSchedule, SCHEDULE_NOW, SEOUL } from "./board-schedule.mjs";
import { BOARD_SCHEDULE_BUDGET, boardScheduleHeavyWeek } from "./board-schedule-perf.mjs";
import { axeViolations, setQualityTheme } from "./settings-quality.mjs";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
/* The fixture's moment is Wednesday 2026-10-21 09:50 in Seoul. The seven days run from five days before it to the day after:
 * Friday 10-16 00:00 to the end of Thursday 10-22, so today is the sixth cell and tomorrow the seventh. */
const FIRST = Date.UTC(2026, 9, 15, 15);
const TODAY = 5;
const SUNDAY_CELL = 2;
const MONDAY_CELL = 3;
const TUESDAY_CELL = 4;
const TOMORROW = 6;
/* The Sunday of the fixture's week, for the scenes that move the clock. */
const SUNDAY = FIRST + SUNDAY_CELL * DAY;

/* The calendar as a person sees it: each day's box, head and blocks, in the DOM's order. */
export async function readWeek(page) {
  return page.evaluate(() => {
    const host = document.querySelector("#board-view .task-board-schedule");
    const week = host.querySelector(".board-schedule-week");
    const box = (node) => {
      const rect = node.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width, height: rect.height };
    };
    const days = [...host.querySelectorAll(".board-schedule-week-day")].map((day) => {
      const head = day.querySelector(".board-schedule-week-head");
      const more = day.querySelector(".board-schedule-more:not(.board-schedule-week-less)");
      const less = day.querySelector(".board-schedule-week-less");
      return {
        at: Number(day.dataset.day), end: Number(day.dataset.dayEnd), current: day.getAttribute("aria-current"),
        label: day.getAttribute("aria-label"), tab: day.tabIndex, box: box(day),
        head: head.textContent, chip: head.dataset.now ?? "", count: Number(head.dataset.count),
        partial: head.dataset.partial ?? "", tip: head.dataset.tip ?? "",
        more: more.hidden ? "" : more.textContent, moreName: more.getAttribute("aria-label") ?? "",
        less: less.hidden ? "" : less.textContent, lessName: less.getAttribute("aria-label") ?? "",
        blocks: [...day.querySelectorAll(".board-schedule-block")].map((block) => {
          const label = block.querySelector(".board-schedule-label");
          return {
            key: block.dataset.key, state: block.dataset.barState, depth: block.dataset.depth, flow: block.dataset.flow ?? "",
            group: block.dataset.scheduleGroup, name: block.getAttribute("aria-label"), expanded: block.getAttribute("aria-expanded"),
            title: label.textContent, sub: label.dataset.sub ?? "", why: block.dataset.why ?? "", act: block.dataset.act ?? "",
            cutStart: block.classList.contains("is-cut"), cutEnd: block.classList.contains("is-cut-end"),
            done: Number(block.style.getPropertyValue("--done")), box: box(block), tab: block.tabIndex,
          };
        }),
      };
    });
    return {
      shown: week?.checkVisibility() ?? false, days, zoom: host.dataset.scheduleZoom,
      scroll: host.scrollWidth, client: host.clientWidth,
      weekScroll: week?.scrollWidth ?? 0, weekClient: week?.clientWidth ?? 0, weekBox: week ? box(week) : null,
      elements: host.querySelectorAll("*").length,
      chart: host.querySelectorAll(".board-schedule-row, .board-schedule-chart, .board-schedule-axis").length,
      lines: [...host.querySelectorAll(".board-schedule-line")].filter((line) => line.checkVisibility())
        .map((line) => line.dataset.line),
      groups: [...host.querySelectorAll(".board-schedule-group")].filter((one) => one.checkVisibility()).map((one) => ({
        id: one.dataset.group, expanded: one.querySelector(".board-schedule-group-head").getAttribute("aria-expanded"),
        sub: one.querySelector(".board-schedule-group-head").dataset.sub ?? "",
      })),
      legend: host.querySelector(".board-schedule-cal-legend")?.textContent ?? "",
      name: week?.getAttribute("aria-label") ?? "",
      note: host.querySelector(".board-schedule-note").hidden ? "" : host.querySelector(".board-schedule-note").textContent,
      log: !host.querySelector(".board-schedule-log").hidden,
      asked: window.__SCHEDULE_ASKED__,
    };
  });
}

const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));

async function zoomTo(page, zoom) {
  await page.evaluate(async (zoom) => {
    const button = document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`);
    if (button.getAttribute("aria-pressed") !== "true") {
      button.click();
      await window.__UNTIL__(() => !boardScheduleAsking, `the ${zoom} answer`);
    }
    paintBoardScheduleHosts();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, zoom);
}

/* A new answer and desk, as the beat would bring them. */
async function moveWeek(page, { jobs, lanes, tasks } = {}) {
  await page.evaluate(async ({ jobs, lanes, tasks }) => {
    const held = window.__SCHEDULE__;
    window.__SCHEDULE__ = { ...held, revision: held.revision + 1, jobs: jobs ?? held.jobs, lanes: lanes ?? held.lanes };
    if (tasks) {
      window.__DESK__ = { ...window.__DESK__, tasks };
      deskLedger = window.__DESK__;
    }
    refreshBoardSchedule();
    await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
    paintBoardScheduleHosts();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  }, { jobs: jobs ?? null, lanes: lanes ?? null, tasks: tasks ?? null });
}

/* Every block of one day stands, or folds back: what the cell holds past its six is read this way. */
const standDay = (page, at, stand = true) => page.evaluate(async ({ at, stand }) => {
  const cell = document.querySelectorAll("#board-view .board-schedule-week-day")[at];
  const more = cell.querySelector(".board-schedule-more:not(.board-schedule-week-less)");
  const less = cell.querySelector(".board-schedule-week-less");
  if (stand) {
    while (!more.hidden) {
      more.click();
      await new Promise((done) => requestAnimationFrame(done));
    }
  } else if (!less.hidden) less.click();
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
}, { at, stand });

const task = (id, stage, times) => ({
  run: "run-4275", id, title: `주 ${id}`, stage, gate: null, blocked_by: [], closed: null,
  created_ms: times.started_ms - 60_000, cost: null, writing: null,
  times: { handed_ms: null, verified_ms: null, landed_ms: null, ended_ms: null, release: null, ...times },
});

async function freshWeek(browser, origin, fixture, { timezoneId = SEOUL, now = SCHEDULE_NOW, faults, tongue = "ko" } = {}) {
  const opened = await openWindowTestPage(browser, origin, { faults, context: { timezoneId } });
  const { page } = opened;
  await installBoardWaits(page);
  await page.clock.setFixedTime(new Date(now));
  await page.evaluate((tongue) => setLocale(tongue, { refresh: false, persist: false }), tongue);
  await page.evaluate(installBoardSchedule, fixture);
  await openBoardSchedule(page);
  await zoomTo(page, "week");
  return page;
}

export async function testBoardScheduleWeek(browser, origin, ok) {
  const fixture = await boardScheduleFixture();
  const faults = [];
  const page = await freshWeek(browser, origin, fixture, { faults });
  try {
    /* ---- 일곱 칸, 요일 머리, 오늘 ------------------------------------------------------ */
    let week = await readWeek(page);
    ok("the week is a calendar of seven day cells, and no horizontal chart stands anywhere",
      week.shown && week.days.length === 7 && week.chart === 0, JSON.stringify({ shown: week.shown, days: week.days.length, chart: week.chart }));
    ok("the seven cells run from five days ago to tomorrow, each one local day after the last",
      week.days[0]?.at === FIRST && week.days.every((day, at) => day.at === FIRST + at * DAY && day.end === day.at + DAY),
      JSON.stringify(week.days.map((day) => [day.at, day.end])));
    ok("the seven heads read the weekdays in order with their dates",
      JSON.stringify(week.days.map((day) => day.head.slice(0, 1))) === JSON.stringify(["금", "토", "일", "월", "화", "수", "목"])
        && week.days[TODAY].head.includes("21"), JSON.stringify(week.days.map((day) => day.head)));
    ok("today alone is marked, on the sixth cell: aria-current=date, a chip with words, and its name says today",
      week.days.filter((day) => day.current === "date").length === 1 && week.days[TODAY].current === "date"
        && week.days[TODAY].chip === "지금 09:50" && week.days[TODAY].label.includes("오늘")
        && week.days.filter((day, at) => at !== TODAY).every((day) => day.chip === "" && !day.label.includes("오늘")),
      JSON.stringify(week.days.map((day) => [day.current, day.chip, day.label])));
    const lefts = week.days.map((day) => Math.round(day.box.left));
    ok("in a wide holder the seven cells stand side by side at one width",
      lefts.every((left, at) => at === 0 || left > lefts[at - 1])
        && week.days.every((day) => Math.abs(day.box.width - week.days[0].box.width) <= 1 && Math.abs(day.box.top - week.days[0].box.top) <= 1),
      JSON.stringify(week.days.map((day) => day.box)));

    /* ---- 안내판과 묶음과 범위 선택은 그대로 --------------------------------------------- */
    ok("the departure board stands over the calendar with its four lines, as on the other zooms",
      week.lines.join(",") === "lane,gate,waiting,ended", JSON.stringify(week.lines));
    ok("the four groups' heads stand over the calendar and count their blocks",
      week.groups.map((one) => one.id).join(",") === "lanes,gates,line,workers" && week.groups.every((one) => one.sub !== ""),
      JSON.stringify(week.groups));
    ok("the week's ask starts at the first cell, five days back, with no revision",
      week.asked.some((one) => one.known === null && one.fromMs === FIRST), JSON.stringify(week.asked.slice(-3)));
    ok("the legend says what a flat end means, and calls the teeth the range's edge, as the other zooms do",
      week.legend.includes("이어짐") && week.legend.includes("범위") && !week.legend.includes("이 주"), week.legend);
    ok("the calendar's name says what it covers and that the arrow keys walk it",
      week.name.includes("5일 전") && week.name.includes("내일") && week.name.includes("방향키"), week.name);

    /* ---- 칸마다 그날의 일 ---------------------------------------------------------------- */
    const inDay = (at, key) => week.days[at].blocks.find((block) => block.key === key || block.key.startsWith(key));
    const sundayLanes = week.days[SUNDAY_CELL].blocks.filter((block) => block.group === "lanes");
    ok("Sunday holds Sunday's release lanes, v1.1.77's three runs among them",
      sundayLanes.filter((block) => block.title.startsWith("v1.1.77 ")).length === 3 && inDay(SUNDAY_CELL, "lane:") !== undefined,
      JSON.stringify(sundayLanes.map((block) => block.title)));
    await standDay(page, MONDAY_CELL);
    const mondayAll = (await readWeek(page)).days[MONDAY_CELL].blocks.map((block) => block.key);
    await standDay(page, MONDAY_CELL, false);
    ok("Monday holds the task that landed that day", mondayAll.some((key) => key.endsWith("t-62493")), JSON.stringify(mondayAll));
    ok("Wednesday holds today's running lane, and it reads as running",
      week.days[TODAY].blocks.some((block) => block.group === "lanes" && block.state === "running"),
      JSON.stringify(week.days[TODAY].blocks.map((block) => [block.key, block.state])));
    ok("a day with no work stands empty and says zero",
      week.days[TOMORROW].blocks.length === 0 && week.days[TOMORROW].count === 0, JSON.stringify(week.days[TOMORROW]));

    /* ---- 날을 넘는 일: 잘리지 않고 이어진다 ---------------------------------------------- */
    const overnight = fixture.answer.lanes.filter((run) => run.version === "1.1.77").at(-1);
    const overnightKey = `lane:${overnight.sha}:${overnight.start_ms}`;
    const before = inDay(SUNDAY_CELL, overnightKey);
    const after = inDay(MONDAY_CELL, overnightKey);
    ok("a lane over midnight stands in both days it touches",
      Boolean(before) && Boolean(after), JSON.stringify({ before, after }));
    ok("the first day's piece says since when and flows out, the next day's says until when and flows in",
      before?.flow === "out" && before.sub.startsWith("22:48부터") && after?.flow === "in" && after.sub.startsWith("00:42까지"),
      JSON.stringify({ before: [before?.flow, before?.sub], after: [after?.flow, after?.sub] }));
    const pastPieces = week.days.slice(0, TODAY).flatMap((day) => day.blocks).filter((block) => !block.cutStart);
    ok("a past cell's blocks speak in hours and minutes: the cell is already that date, so no year stands in their line (review P3-4)",
      pastPieces.length > 0 && pastPieces.every((block) => !/20\d\d/.test(block.sub) && !/20\d\d/.test(block.name)),
      JSON.stringify(pastPieces.filter((block) => /20\d\d/.test(block.sub)).map((block) => block.sub)));
    const oneDayEnded = pastPieces.find((block) => block.flow === "" && block.state === "done");
    ok("a finished block of one day says when it began and when it ended",
      /^\d\d:\d\d · /.test(oneDayEnded?.sub ?? "") && / · \d\d:\d\d$/.test(oneDayEnded?.sub ?? ""), oneDayEnded?.sub);
    ok("a piece flowing in says its end once", (after?.sub.match(/00:42/g) ?? []).length === 1, after?.sub);
    ok("each piece's name tells a screen reader it continues",
      before?.name.includes("다음 날로 이어짐") && after?.name.includes("앞 날에서 이어짐"),
      JSON.stringify({ before: before?.name, after: after?.name }));
    ok("the two pieces stand on one line across the midnight, and meet at the cell's edge",
      Boolean(before && after) && Math.abs(before.box.top - after.box.top) <= 1 && Math.abs(before.box.height - after.box.height) <= 1
        && after.box.left - before.box.right <= 1, JSON.stringify({ before: before?.box, after: after?.box }));

    const tasks = [
      ...fixture.desk.tasks,
      task("t-90001", "dispatched", { started_ms: SCHEDULE_NOW - 2 * DAY }),
      task("t-90002", "dispatched", { started_ms: FIRST - 3 * DAY }),
    ];
    const lanes = [...fixture.answer.lanes.map((run) => (run.running ? {
      ...run,
      ahead: [run.ahead[0], { name: "soak", expected: { secs: 5 * 86_400, samples: 3 } }],
    } : run))];
    await moveWeek(page, { tasks, lanes });
    week = await readWeek(page);
    const middle = week.days.map((day) => day.blocks.find((block) => block.key.endsWith("t-90001")));
    ok("a task running since two days ago stands in each day to today, the day between saying only that it continues",
      middle[MONDAY_CELL]?.flow === "out" && middle[TUESDAY_CELL]?.flow === "both" && middle[TUESDAY_CELL].sub.startsWith("이어짐")
        && middle[TODAY]?.flow === "in" && middle[TOMORROW] === undefined, JSON.stringify(middle.map((one) => [one?.flow, one?.sub])));
    ok("what is still going says it goes on until now, not a clock that moves every minute (review P3-3)",
      middle[TODAY]?.sub.startsWith("지금까지") && !/^\d\d:\d\d/.test(middle[TODAY]?.sub ?? ""), middle[TODAY]?.sub);
    const runningLane = week.days[TODAY].blocks.find((block) => block.group === "lanes" && block.state === "running");
    const laneTomorrow = week.days[TOMORROW].blocks.find((block) => block.key === runningLane?.key);
    ok("a piece before now is filled, today's expected work is filled to now, and tomorrow's is all ahead",
      middle[TUESDAY_CELL]?.done === 100 && runningLane?.done > 0 && runningLane.done < 100 && laneTomorrow?.done === 0,
      JSON.stringify({ gone: middle[TUESDAY_CELL]?.done, today: runningLane?.done, tomorrow: laneTomorrow?.done }));
    const cutStart = week.days[0].blocks.find((block) => block.key.endsWith("t-90002"));
    ok("a task begun before the range is toothed on the first cell and says since when, with the date",
      cutStart?.cutStart === true && cutStart.flow === "both" && cutStart.sub.includes("부터") && cutStart.sub.includes("13"),
      JSON.stringify(cutStart));
    const cutEnd = week.days[TOMORROW].blocks.find((block) => block.group === "lanes" && block.state === "running");
    ok("a lane expected past the range is toothed on the last cell, and today holds it too",
      cutEnd?.cutEnd === true && cutEnd.flow === "both" && week.days[TODAY].blocks.some((block) => block.key === cutEnd.key),
      JSON.stringify(week.days.map((day) => day.blocks.filter((block) => block.state === "running" && block.group === "lanes").map((block) => block.flow))));
    await moveWeek(page, { tasks: fixture.desk.tasks, lanes: fixture.answer.lanes });

    /* ---- 칸의 차례는 상태의 것이다: 도는 일이 「+N개 더」 뒤로 숨지 않는다 (검수 r1 P2-1) ------------- */
    const rank = { running: 0, failed: 1, waiting: 2, held: 2, idle: 3, done: 4 };
    const runningFirst = (blocks) => {
      const states = blocks.map((block) => block.state);
      return blocks[0]?.group === "lanes" && states[0] === "running" && blocks[1]?.group === "gates" && states[1] === "running"
        && states.every((state, at) => at === 0 || rank[state] >= rank[states[at - 1]]);
    };
    const oldWaits = Array.from({ length: 6 }, (_, at) => task(`t-92${at}`, "reported", {
      started_ms: SCHEDULE_NOW - 3 * DAY - at * HOUR, handed_ms: SCHEDULE_NOW - 3 * DAY,
    }));
    await moveWeek(page, { tasks: [...fixture.desk.tasks, ...oldWaits] });
    week = await readWeek(page);
    ok("six tasks waiting since three days ago do not take today's lines from the running lane and gate",
      runningFirst(week.days[TODAY].blocks), JSON.stringify(week.days[TODAY].blocks.map((block) => [block.key, block.state])));
    const overnightEnds = Array.from({ length: 6 }, (_, at) => task(`t-93${at}`, "merged", {
      started_ms: SCHEDULE_NOW - DAY + at * 60_000, landed_ms: SCHEDULE_NOW - 3_600_000 + at * 60_000,
    }));
    await moveWeek(page, { tasks: [...fixture.desk.tasks, ...overnightEnds] });
    week = await readWeek(page);
    ok("six tasks that began yesterday and ended today do not stand before the running lane and gate",
      runningFirst(week.days[TODAY].blocks), JSON.stringify(week.days[TODAY].blocks.map((block) => [block.key, block.state])));
    const allEnded = [...fixture.desk.tasks, ...Array.from({ length: 12 }, (_, at) => task(`t-94${String(at).padStart(2, "0")}`, "merged", {
      started_ms: FIRST + DAY + (8 + at) * HOUR, landed_ms: FIRST + DAY + (8.5 + at) * HOUR,
    }))];
    await moveWeek(page, { tasks: allEnded });
    week = await readWeek(page);
    const saturdayEnded = week.days[1].blocks.filter((block) => block.group === "workers").map((block) => block.key);
    ok("among what ended the newest stands first: the last thing to finish that day is not behind the count",
      saturdayEnded.some((key) => key.endsWith("t-9411")) && saturdayEnded.findIndex((key) => key.endsWith("t-9411")) < saturdayEnded.findIndex((key) => key.endsWith("t-9410")),
      JSON.stringify(saturdayEnded));
    await moveWeek(page, { tasks: fixture.desk.tasks });

    /* ---- 칸을 넘는 블록: 수를 말하고, 누르면 다 선다 ---------------------------------------- */
    const crowd = [
      ...Array.from({ length: 20 }, (_, at) => task(`t-91${String(at).padStart(3, "0")}`, "merged", {
        started_ms: SUNDAY + 4 * DAY + (9 + at / 4) * HOUR, landed_ms: SUNDAY + 4 * DAY + (9.1 + at / 4) * HOUR,
      })),
      task("t-91999", "reported", { started_ms: SUNDAY + 4 * DAY + 23 * HOUR, handed_ms: SUNDAY + 4 * DAY + 23.5 * HOUR }),
    ];
    await page.clock.setFixedTime(new Date(SUNDAY + 5 * DAY + 9 * HOUR));
    await moveWeek(page, { tasks: crowd });
    week = await readWeek(page);
    const crowded = week.days[4];
    const hidden = crowded.count - crowded.blocks.length;
    ok("a cell holds what fits and says how many more, in words a person can read",
      crowded.count >= 21 && hidden > 0 && crowded.more === `+${hidden}개 더` && crowded.less === "",
      JSON.stringify({ count: crowded.count, shown: crowded.blocks.length, more: crowded.more }));
    ok("the more button's name says which day and how many", crowded.moreName.includes(String(hidden)) && crowded.moreName.includes("22"),
      crowded.moreName);
    const waitingAt = crowded.blocks.findIndex((block) => block.key.endsWith("t-91999"));
    const firstEnded = crowded.blocks.findIndex((block) => block.state === "done");
    ok("what waits for someone is never the block folded away: the waiting task stands before the finished ones",
      waitingAt >= 0 && (firstEnded < 0 || waitingAt < firstEnded), JSON.stringify(crowded.blocks.map((block) => [block.key, block.state])));
    await page.evaluate(() => document.querySelectorAll("#board-view .board-schedule-week-day")[4].querySelector(".board-schedule-more").click());
    await page.evaluate(settle);
    week = await readWeek(page);
    ok("a press stands the rest of that day, and a button now folds them, named for the day",
      week.days[4].blocks.length === crowded.count && week.days[4].more === "" && week.days[4].less === `${hidden}개 접기`
        && week.days[4].lessName.includes("22"),
      JSON.stringify({ shown: week.days[4].blocks.length, more: week.days[4].more, less: week.days[4].less }));
    ok("no block is cut by the cell: each one's full height stands inside its day",
      week.days[4].blocks.every((block) => block.box.top >= week.days[4].box.top - 1 && block.box.bottom <= week.days[4].box.bottom + 1),
      JSON.stringify(week.days[4].box));
    await page.evaluate(() => document.querySelectorAll("#board-view .board-schedule-week-day")[4].querySelector(".board-schedule-week-less").click());
    await page.evaluate(settle);
    week = await readWeek(page);
    ok("the fold button folds them again", week.days[4].blocks.length === crowded.blocks.length && week.days[4].more === crowded.more && week.days[4].less === "",
      JSON.stringify({ shown: week.days[4].blocks.length, more: week.days[4].more }));
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));
    await moveWeek(page, { tasks: fixture.desk.tasks });

    /* ---- 묶음 머리는 그 묶음의 블록을 칸에서 뺐다 넣는다 ------------------------------------ */
    await page.evaluate(() => document.querySelector('#board-view [data-group="lanes"] .board-schedule-group-head').click());
    await page.evaluate(settle);
    week = await readWeek(page);
    ok("a group's head takes its blocks out of every cell",
      week.groups.find((one) => one.id === "lanes").expanded === "false" && week.days.every((day) => day.blocks.every((block) => block.group !== "lanes")),
      JSON.stringify(week.groups));
    await page.evaluate(() => document.querySelector('#board-view [data-group="lanes"] .board-schedule-group-head').click());
    await page.evaluate(settle);

    /* ---- 블록의 누름과 Enter, 게이트의 일감, 실린 과업 ---------------------------------------- */
    await page.evaluate(() => {
      window.__SCHEDULE_ASKED__.length = 0;
      document.querySelector('#board-view .board-schedule-week-day [data-key^="lane:"]').click();
    });
    await page.evaluate(() => window.__UNTIL__(() => window.__SCHEDULE_ASKED__.some((one) => one.log?.lane), "the lane's log"));
    week = await readWeek(page);
    ok("a press on a lane's block opens its log under the calendar, through the same command",
      week.log && week.asked.some((one) => one.log?.lane), JSON.stringify(week.asked));
    await page.evaluate(() => document.querySelector("#board-view .board-schedule-log-close").click());
    const gateKey = "gate:l61";
    const gateAt = (await readWeek(page)).days.findIndex((day) => day.blocks.some((block) => block.key === gateKey));
    const pressGate = () => page.evaluate(async ({ gateAt, gateKey }) => {
      document.querySelectorAll("#board-view .board-schedule-week-day")[gateAt]
        .querySelector(`[data-key="${gateKey}"]`).click();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, { gateAt, gateKey });
    week = await readWeek(page);
    const closed = week.days[gateAt].blocks.find((block) => block.key === gateKey);
    ok("a gate's block is folded at first and its chip says it unfolds",
      closed?.expanded === "false" && closed.act === "펴기 →" && week.days[gateAt].blocks.every((block) => block.depth === "0"),
      JSON.stringify(closed));
    await pressGate();
    week = await readWeek(page);
    const unfolded = week.days[gateAt].blocks.find((block) => block.key === gateKey);
    const jobs = week.days.flatMap((day) => day.blocks).filter((block) => block.depth === "1");
    const l61jobs = fixture.answer.jobs.filter((job) => job.gate === "l61").length;
    ok("a press unfolds the gate: its jobs stand as blocks of their own, none folded behind the count",
      unfolded?.expanded === "true" && unfolded.act === "접기 →" && new Set(jobs.map((block) => block.key)).size === l61jobs,
      JSON.stringify({ jobs: jobs.length, l61jobs }));
    await page.evaluate(async () => {
      window.__SCHEDULE_ASKED__.length = 0;
      document.querySelector('#board-view .board-schedule-week-day [data-depth="1"]').click();
      await window.__UNTIL__(() => window.__SCHEDULE_ASKED__.some((one) => one.log?.job), "the job's log");
    });
    week = await readWeek(page);
    ok("a gate job's block opens that job's log", week.log && week.asked.some((one) => one.log?.job?.includes("l61")),
      JSON.stringify(week.asked));
    await page.evaluate(() => document.querySelector("#board-view .board-schedule-log-close").click());
    // An unfolded gate pushed behind the count takes its jobs with it: a job never stands with no head (review P3-2).
    const busyLanes = [...fixture.answer.lanes, ...Array.from({ length: 7 }, (_, at) => {
      const run = fixture.answer.lanes.find((one) => one.running);
      return { ...run, sha: `cc00000${at}`, version: `9.9.${at}`, start_ms: run.start_ms + at * 1_000 };
    })];
    await moveWeek(page, { lanes: busyLanes });
    week = await readWeek(page);
    const gateDay = week.days[gateAt];
    ok("an unfolded gate behind the count leaves no job of it standing with no head",
      !gateDay.blocks.some((block) => block.key === gateKey) && gateDay.blocks.every((block) => block.depth === "0") && gateDay.more !== "",
      JSON.stringify(gateDay.blocks.map((block) => [block.key, block.depth])));
    await page.evaluate(async (gateAt) => {
      const cell = document.querySelectorAll("#board-view .board-schedule-week-day")[gateAt];
      cell.querySelector(".board-schedule-more:not(.board-schedule-week-less)").click();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, gateAt);
    week = await readWeek(page);
    const standing = week.days[gateAt].blocks.map((block) => block.key);
    const headAt = standing.indexOf(gateKey);
    ok("pressed out of the count, the gate stands first and its jobs right behind it",
      headAt >= 0 && week.days[gateAt].blocks.slice(headAt + 1, headAt + 1 + l61jobs).every((block) => block.depth === "1"),
      JSON.stringify(week.days[gateAt].blocks.map((block) => [block.key, block.depth])));
    await page.evaluate(async (gateAt) => {
      const cell = document.querySelectorAll("#board-view .board-schedule-week-day")[gateAt];
      cell.querySelector(".board-schedule-week-less").click();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, gateAt);
    await moveWeek(page, { lanes: fixture.answer.lanes });
    await pressGate();
    // The gate carries two tasks by its running job's words, the way the timetable's scene does.
    const lint = fixture.answer.jobs.find((job) => job.gate === "l61" && job.state === "running");
    await moveWeek(page, { jobs: fixture.answer.jobs.map((job) => (job.name === lint.name
      ? { ...job, what: "v1.1.84: t-66539/7102 ctty helper" } : job)) });
    await standDay(page, TODAY);
    week = await readWeek(page);
    await standDay(page, TODAY, false);
    const carried = week.days.flatMap((day) => day.blocks).filter((block) => block.key.endsWith("t-67102") || block.key.endsWith("t-66539"));
    ok("a task carried by the gate says so on its block in the week, as in the day",
      carried.length >= 2 && carried.every((block) => block.why.includes("l61에 실림")), JSON.stringify(carried.map((block) => block.why)));
    await moveWeek(page, { jobs: fixture.answer.jobs });
    // A real Enter, on a block holding the focus: the board's keys are the person's (t-84799 ③).
    await page.evaluate(() => {
      window.__SCHEDULE_ASKED__.length = 0;
      document.querySelector('#board-view .board-schedule-week-day [data-key^="lane:"]').focus();
    });
    await page.keyboard.press("Enter");
    await page.evaluate(() => window.__UNTIL__(() => window.__SCHEDULE_ASKED__.some((one) => one.log?.lane), "the log by Enter"));
    ok("Enter on a focused block does what a press does", (await readWeek(page)).log, "");
    await page.evaluate(() => document.querySelector("#board-view .board-schedule-log-close").click());
    await page.evaluate(() => document.querySelector('#board-view .board-schedule-week-day [data-key$="t-62493"]')?.click());
    const went = await page.evaluate(async () => {
      await new Promise((done) => requestAnimationFrame(done));
      return document.querySelector("#board-view .task-board-surface").dataset.boardTab;
    });
    ok("a worker task's block goes to that task on the History tab", went === "history", went);
    await openBoardSchedule(page);
    await zoomTo(page, "day");
    ok("the range selector leaves the calendar for the timetable",
      await page.evaluate(() => !document.querySelector("#board-view .board-schedule-week").checkVisibility()
        && document.querySelector("#board-view .board-schedule-day").checkVisibility()), "");
    await zoomTo(page, "week");

    /* ---- 키보드: 달력은 탭 멈춤 하나, 안에서는 방향키 — 멈춤이 처음 서는 자리를 보려 새 페이지에서 ---- */
    const keys = await freshWeek(browser, origin, fixture);
    const focused = () => keys.evaluate(() => {
      const node = document.activeElement;
      const day = node?.closest?.(".board-schedule-week-day");
      const days = [...document.querySelectorAll("#board-view .board-schedule-week-day")];
      return {
        day: day ? days.indexOf(day) : -1, cell: node?.classList?.contains("board-schedule-week-day") ?? false,
        block: node?.classList?.contains("board-schedule-block") ?? false,
        key: node?.dataset?.key ?? "", name: node?.getAttribute?.("aria-label") ?? "",
        outline: node ? getComputedStyle(node).outlineStyle : "",
      };
    });
    week = await readWeek(keys);
    const stops = week.days.flatMap((day) => [day.tab, ...day.blocks.map((block) => block.tab)]).filter((tab) => tab === 0);
    ok("the calendar is one Tab stop, on today's cell at first", stops.length === 1 && week.days[TODAY].tab === 0,
      JSON.stringify(week.days.map((day) => [day.tab, day.blocks.filter((block) => block.tab === 0).length])));
    await keys.evaluate(() => document.querySelector('#board-view [data-group="workers"] .board-schedule-group-head').focus());
    await keys.keyboard.press("Tab");
    let where = await focused();
    ok("Tab from the group heads lands on today's cell, its name read and its focus shown",
      where.day === TODAY && where.cell && where.name.includes("오늘") && where.outline !== "none", JSON.stringify(where));
    await keys.keyboard.press("ArrowLeft");
    where = await focused();
    ok("← goes to the day before", where.day === TODAY - 1 && where.cell, JSON.stringify(where));
    await keys.keyboard.press("ArrowRight");
    await keys.keyboard.press("ArrowDown");
    where = await focused();
    ok("↓ on a cell goes into its first block", where.day === TODAY && where.block && where.key !== "", JSON.stringify(where));
    const firstBlock = where.key;
    await keys.keyboard.press("ArrowDown");
    where = await focused();
    ok("↓ on a block goes to the next one in the day", where.day === TODAY && where.block && where.key !== firstBlock, JSON.stringify(where));
    await keys.keyboard.press("ArrowUp");
    await keys.keyboard.press("ArrowUp");
    where = await focused();
    ok("↑ from the first block goes back to the cell", where.day === TODAY && where.cell, JSON.stringify(where));
    await keys.keyboard.press("ArrowDown");
    await keys.keyboard.press("ArrowLeft");
    where = await focused();
    ok("← on a block goes to the block at the same place the day before", where.day === TODAY - 1 && where.block, JSON.stringify(where));
    await keys.keyboard.press("Home");
    where = await focused();
    ok("Home goes to the first cell", where.day === 0 && where.cell, JSON.stringify(where));
    await keys.keyboard.press("End");
    where = await focused();
    ok("End goes to the last cell", where.day === 6 && where.cell, JSON.stringify(where));
    week = await readWeek(keys);
    ok("the Tab stop follows the focus: still one, on the cell last visited",
      week.days[6].tab === 0 && week.days.flatMap((day) => [day.tab, ...day.blocks.map((block) => block.tab)]).filter((tab) => tab === 0).length === 1,
      JSON.stringify(week.days.map((day) => day.tab)));
    await keys.keyboard.press("Tab");
    where = await focused();
    ok("Tab leaves the calendar instead of walking its blocks", where.day === -1, JSON.stringify(where));
    const nameless = await keys.evaluate(() => [...document.querySelectorAll("#board-view .board-schedule-week button, #board-view .board-schedule-week-day")]
      .filter((node) => node.checkVisibility() && !(node.getAttribute("aria-label") ?? node.textContent).trim()).length);
    ok("every cell and every button of the calendar has a name", nameless === 0, String(nameless));

    await keys.evaluate((at) => document.querySelectorAll("#board-view .board-schedule-week-day")[at].querySelector(".board-schedule-more").focus(), TUESDAY_CELL);
    for (let press = 0; press < 8; press += 1) {
      if (await keys.evaluate(() => document.activeElement.classList.contains("board-schedule-week-less"))) break;
      await keys.keyboard.press("Enter");
    }
    const handed = await keys.evaluate((at) => ({
      less: document.activeElement.classList.contains("board-schedule-week-less"),
      moreHidden: document.querySelectorAll("#board-view .board-schedule-week-day")[at].querySelector(".board-schedule-more").hidden,
    }), TUESDAY_CELL);
    ok("when the last page stands, the focus goes from the gone button to the fold button", handed.less && handed.moreHidden, JSON.stringify(handed));
    await keys.keyboard.press("Enter");
    const backed = await keys.evaluate(() => document.activeElement.className);
    ok("and folding hands it back", backed === "board-schedule-more", backed);
    await keys.evaluate((at) => document.querySelectorAll("#board-view .board-schedule-week-day")[at].focus(), TODAY);
    await keys.keyboard.press("Enter");
    where = await focused();
    ok("Enter on a cell goes into its first block rather than pressing anything", where.day === TODAY && where.block, JSON.stringify(where));
    await keys.close();

    /* ---- 조용한 박자는 아무것도 쓰지 않는다 ------------------------------------------------- */
    const writes = async (act) => page.evaluate(async (source) => {
      const host = document.querySelector("#board-view .task-board-schedule");
      const records = [];
      const watch = new MutationObserver((batch) => records.push(...batch));
      watch.observe(host, { subtree: true, childList: true, attributes: true, characterData: true });
      await new Function(`return (async () => { ${source} })()`)();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      records.push(...watch.takeRecords());
      watch.disconnect();
      return records.length;
    }, act);
    const quiet = await writes(`refreshBoardSchedule(); await window.__UNTIL__(() => !boardScheduleAsking, "quiet"); paintBoardScheduleHosts();`);
    ok("a quiet beat writes nothing", quiet === 0, String(quiet));
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW + 30_000));
    const sameMinute = await writes(`paintBoardScheduleHosts();`);
    ok("a beat in the same minute writes nothing", sameMinute === 0, String(sameMinute));
    const sized = await writes(`document.querySelector("#board-view .task-board-schedule").style.width = "900px"; paintBoardScheduleHosts();`);
    ok("a new width alone draws nothing again: the holder's own attribute is the only write", sized <= 1, String(sized));
    await page.evaluate(() => { document.querySelector("#board-view .task-board-schedule").style.width = ""; });
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW + 60_000));
    const nextMinute = await writes(`paintBoardScheduleHosts();`);
    week = await readWeek(page);
    ok("the next minute moves only what the minute moved: today's chip says it, and far fewer writes than blocks",
      week.days[TODAY].chip === "지금 09:51" && nextMinute < 60, String(nextMinute));
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));
    await page.evaluate(settle);

    /* ---- 요소 수 ---------------------------------------------------------------------------- */
    week = await readWeek(page);
    ok(`the fixture's week stays within ${BOARD_SCHEDULE_BUDGET.elements} elements`, week.elements <= BOARD_SCHEDULE_BUDGET.elements, String(week.elements));
    const heavy = boardScheduleHeavyWeek(SCHEDULE_NOW, FIRST);
    await page.evaluate(async ({ answer, desk }) => {
      window.__SCHEDULE__ = answer;
      window.__DESK__ = desk;
      deskLedger = desk;
      refreshBoardSchedule({ fresh: true });
      await window.__UNTIL__(() => boardScheduleAnswer.revision === answer.revision, "the heavy week");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, heavy);
    week = await readWeek(page);
    const heavyCount = week.days.reduce((sum, day) => sum + day.count, 0);
    ok(`fifty lanes and five hundred blocks stay within ${BOARD_SCHEDULE_BUDGET.elements} elements, every day counting all it holds`,
      week.elements <= BOARD_SCHEDULE_BUDGET.elements && heavyCount >= 500 && week.days.some((day) => day.more !== ""),
      JSON.stringify({ elements: week.elements, heavyCount }));
    ok("fifty lanes never push the page wide", week.scroll <= week.client + 1, JSON.stringify([week.scroll, week.client]));
    // A press on the busiest day stands one page, not all: the press's cost does not grow with the day (t-84802).
    const paged = await page.evaluate(async () => {
      const buttons = [...document.querySelectorAll("#board-view .board-schedule-more:not(.board-schedule-week-less)")].filter((one) => !one.hidden);
      const busiest = buttons.sort((a, b) => parseInt(b.textContent.slice(1), 10) - parseInt(a.textContent.slice(1), 10))[0];
      const cell = busiest.closest(".board-schedule-week-day");
      const before = cell.querySelectorAll(".board-schedule-block").length;
      const hiddenBefore = parseInt(busiest.textContent.slice(1), 10);
      busiest.focus();
      busiest.click();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const after = cell.querySelectorAll(".board-schedule-block").length;
      const less = cell.querySelector(".board-schedule-week-less");
      const left = parseInt(busiest.textContent.slice(1), 10);
      const shownLess = !less.hidden;
      less.click();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return { added: after - before, hiddenBefore, left, shownLess, back: cell.querySelectorAll(".board-schedule-block").length === before,
        focus: document.activeElement === busiest };
    });
    ok("on the busiest day of a heavy week a press stands one page and says what is left, and the fold takes them back",
      paged.added === 24 && paged.left === paged.hiddenBefore - 24 && paged.shownLess && paged.back, JSON.stringify(paged));
    await page.evaluate(async ({ answer, desk }) => {
      window.__SCHEDULE__ = { ...answer, revision: answer.revision + 1 };
      window.__DESK__ = desk;
      deskLedger = desk;
      refreshBoardSchedule({ fresh: true });
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the fixture again");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, { answer: { ...fixture.answer, revision: heavy.answer.revision + 1 }, desk: fixture.desk });

    /* ---- 좁은 폭: 가로 스크롤 없이 쌓는다, 다섯 언어 ---------------------------------------- */
    const narrowFaults = [];
    for (const width of [390, 320]) {
      await page.setViewportSize({ width, height: 800 });
      for (const tongue of ["ko", "en", "ja", "zh", "es"]) {
        await page.evaluate(async (tongue) => {
          setLocale(tongue, { refresh: false, persist: false });
          paintBoardScheduleHosts();
          await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        }, tongue);
        const seen = await readWeek(page);
        const stacked = seen.days.every((day, at) => at === 0 || day.box.top >= seen.days[at - 1].box.bottom - 1);
        const fullWidth = seen.days.every((day) => Math.abs(day.box.width - seen.weekBox.width) <= 2);
        const overflow = seen.scroll > seen.client + 1 || seen.weekScroll > seen.weekClient + 1;
        const inside = seen.days.every((day) => day.blocks.every((block) => block.box.left >= day.box.left - 1 && block.box.right <= day.box.right + 1));
        if (!stacked || !fullWidth || overflow || !inside) narrowFaults.push({ width, tongue, stacked, fullWidth, overflow, inside });
      }
    }
    ok("at 390 and 320 px the days stack, full width, with no sideways scroll, in all five languages",
      narrowFaults.length === 0, JSON.stringify(narrowFaults));
    await page.evaluate(() => setLocale("ko", { refresh: false, persist: false }));
    await page.setViewportSize({ width: 390, height: 800 });
    await page.evaluate(() => paintBoardScheduleHosts());
    await page.evaluate(settle);
    const stackedGaps = await readWeek(page);
    const sunday = stackedGaps.days[SUNDAY_CELL].blocks;
    ok("stacked, a day's blocks follow one another with no empty slots between",
      sunday.every((block, at) => at === 0 || block.box.top - sunday[at - 1].box.bottom <= 8),
      JSON.stringify(sunday.map((block) => [block.box.top, block.box.bottom])));

    /* ---- 쌓인 폭에서 달력을 열면 오늘이 보인다, 한 번만 (검수 r1 P3-5) --------------------------------- */
    const scrolled = await page.evaluate(async () => {
      const host = document.querySelector("#board-view .task-board-schedule");
      const frame = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const press = async (zoom) => {
        document.querySelector(`#board-view [data-schedule-zoom="${zoom}"]`).click();
        await window.__UNTIL__(() => !boardScheduleAsking, "the answer");
        await frame();
      };
      const scroller = () => {
        for (let node = host; node; node = node.parentElement) {
          if (node.scrollHeight > node.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(node).overflowY)) return node;
        }
        return document.scrollingElement;
      };
      await press("day");
      await press("week");
      const today = document.querySelector('#board-view .board-schedule-week-day[aria-current="date"]');
      const box = scroller().getBoundingClientRect();
      const seen = today.getBoundingClientRect();
      const opened = seen.top < box.bottom && seen.top >= box.top - 1;
      // The person scrolls away; the next beats and the next minute leave the scroll where they put it.
      scroller().scrollTop = 0;
      const put = scroller().scrollTop;
      paintBoardScheduleHosts();
      window.__SCHEDULE__ = { ...window.__SCHEDULE__, revision: window.__SCHEDULE__.revision + 1 };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "a new answer");
      paintBoardScheduleHosts();
      await frame();
      return { opened, top: seen.top, bottom: box.bottom, put, after: scroller().scrollTop };
    });
    ok("opening the calendar stacked brings today into view, and no beat scrolls after that",
      scrolled.opened && scrolled.after === scrolled.put, JSON.stringify(scrolled));

    /* ---- 다크·라이트, axe -------------------------------------------------------------------- */
    const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
    const AxeBuilder = axeModule.default ?? axeModule;
    for (const [width, height] of [[1280, 860], [390, 800]]) {
      await page.setViewportSize({ width, height });
      for (const theme of ["dark", "light"]) {
        await setQualityTheme(page, theme);
        await page.evaluate(settle);
        const violations = await axeViolations(page, AxeBuilder, "#board-view .board-schedule-week");
        ok(`the calendar passes axe at ${width}px in ${theme}`, violations.length === 0, JSON.stringify(violations).slice(0, 900));
      }
    }
    await setQualityTheme(page, "dark");

    /* ---- 기다리는 블록의 칠은 글 뒤를 지나지 않는다 (검수 r1 P3-7) ------------------------------------- */
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.evaluate(settle);
    await standDay(page, TODAY);
    const waitingPaint = await page.evaluate(() => {
      const block = document.querySelector('#board-view .board-schedule-week [data-bar-state="waiting"]');
      if (!block) return null;
      const style = getComputedStyle(block);
      const label = block.querySelector(".board-schedule-label").getBoundingClientRect();
      const box = block.getBoundingClientRect();
      const strip = parseFloat(style.backgroundSize.split(",")[0].trim().split(" ")[1]);
      return { size: style.backgroundSize, position: style.backgroundPosition, strip, height: box.height, labelTop: label.top - box.top };
    });
    await standDay(page, TODAY, false);
    ok("a waiting block's fill is a strip along its foot, so no words stand on it",
      waitingPaint && waitingPaint.strip > 0 && waitingPaint.strip < waitingPaint.height / 2 && waitingPaint.position.includes("100%"),
      JSON.stringify(waitingPaint));
    ok("the calendar raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }

  /* ---- 언어: 범위는 언어를 타지 않고, 요일 이름만 그 언어다 ---------------------------------------- */
  const tongues = [["ko", "금"], ["en", "Fri"], ["ja", "金"], ["zh", "周五"], ["es", "vie"]];
  for (const [tongue, first] of tongues) {
    const local = await freshWeek(browser, origin, fixture, { tongue });
    try {
      const seen = await readWeek(local);
      ok(`in ${tongue} the seven days are the same days, the first head named in that language`,
        seen.days[0]?.at === FIRST && seen.days[0].head.startsWith(first)
          && seen.asked.some((one) => one.known === null && one.fromMs === FIRST),
        JSON.stringify({ at: seen.days[0]?.at, head: seen.days[0]?.head, asked: seen.asked.slice(-2) }));
      ok(`in ${tongue} today is the sixth cell, and the calendar's name is in that language`,
        seen.days[TODAY].current === "date" && seen.days.filter((day) => day.current === "date").length === 1
          && seen.name !== "" && (tongue === "ko" || !seen.name.includes("방향키")),
        JSON.stringify({ current: seen.days.map((day) => day.current), name: seen.name }));
    } finally {
      await local.close();
    }
  }
  const switcher = await freshWeek(browser, origin, fixture);
  try {
    const asks = await switcher.evaluate(async () => {
      window.__SCHEDULE_ASKED__.length = 0;
      setLocale("es", { refresh: false, persist: false });
      paintBoardScheduleHosts();
      refreshBoardSchedule();
      await window.__UNTIL__(() => !boardScheduleAsking, "the beat's answer");
      return { asked: window.__SCHEDULE_ASKED__.slice(), weekStartRule: typeof boardScheduleWeekFirstDay };
    });
    ok("changing the language moves no cell, so the beat asks with the revision it holds, and no week-start rule is left",
      asks.asked.every((one) => one.known !== null && one.fromMs === FIRST) && asks.weekStartRule === "undefined", JSON.stringify(asks));
  } finally {
    await switcher.close();
  }

  /* ---- 달력 주의 구멍이 없다: 일요일과 월요일 아침에도 지난 닷새가 보인다 (검수 r1 P1-1) ------------------- */
  const saturdayLate = Date.UTC(2026, 9, 24, 14, 50); // Saturday 10-24 23:50 in Seoul.
  const fridayNoon = Date.UTC(2026, 9, 23, 3);
  const endOfWeek = [
    task("t-95001", "merged", { started_ms: saturdayLate - HOUR, landed_ms: saturdayLate }),
    task("t-95002", "merged", { started_ms: fridayNoon - HOUR, landed_ms: fridayNoon }),
    // One an hour after midnight on each day from 10-20, the first cell of the Sunday scene.
    ...Array.from({ length: 7 }, (_, at) => task(`t-9510${at}`, "merged", {
      started_ms: Date.UTC(2026, 9, 19, 16) + at * DAY, landed_ms: Date.UTC(2026, 9, 19, 16, 30) + at * DAY,
    })),
  ];
  const quietFixture = { answer: { ...fixture.answer, jobs: [], lanes: [] }, desk: { ...fixture.desk, tasks: endOfWeek } };
  for (const [word, now, saturdayCell] of [["Sunday", Date.UTC(2026, 9, 25, 0), 4], ["Monday", Date.UTC(2026, 9, 26, 0), 3]]) {
    const morning = await freshWeek(browser, origin, quietFixture, { now });
    try {
      const seen = await readWeek(morning);
      const held = (cell, id) => seen.days[cell].blocks.some((block) => block.key.endsWith(id));
      ok(`on ${word} morning no cell before tomorrow stands empty, and today is the sixth cell`,
        seen.days.slice(0, TOMORROW).every((day) => day.count > 0) && seen.days[TODAY].current === "date",
        JSON.stringify(seen.days.map((day) => [day.head, day.count])));
      ok(`on ${word} morning Saturday night's task and Friday's are in their days`,
        held(saturdayCell, "t-95001") && held(saturdayCell - 1, "t-95002"),
        JSON.stringify(seen.days.map((day) => day.blocks.map((block) => block.key))));
      const ended = await morning.evaluate(() => document.querySelector('#board-view [data-line="ended"]').getAttribute("aria-label"));
      ok(`on ${word} morning the board's ended line counts them instead of saying the range holds none`,
        !ended.includes("이 범위에 없음"), ended);
    } finally {
      await morning.close();
    }
  }

  /* ---- 창을 켜 둔 채 자정을 넘으면 칸이 넘어간다 ---------------------------------------------------- */
  const midnight = await freshWeek(browser, origin, fixture);
  try {
    const was = await readWeek(midnight);
    const nextDay = Date.UTC(2026, 9, 21, 15, 5); // Thursday 10-22 00:05 in Seoul.
    await midnight.clock.setFixedTime(new Date(nextDay));
    await midnight.evaluate(async () => {
      window.__SCHEDULE_ASKED__.length = 0;
      refreshBoardSchedule();
      await window.__UNTIL__(() => !boardScheduleAsking, "the beat after midnight");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    const now = await readWeek(midnight);
    ok("past midnight every cell moves one day: yesterday's today is the fifth cell, and the sixth is the new today",
      now.days[0].at === FIRST + DAY && now.days[TODAY].at === was.days[TOMORROW].at && now.days[TODAY].current === "date"
        && now.days[TODAY].chip === "지금 00:05" && now.days.filter((day) => day.current === "date").length === 1,
      JSON.stringify(now.days.map((day) => [day.head, day.current, day.chip])));
    const finished = was.days[TUESDAY_CELL].blocks.filter((block) => block.state === "done").map((block) => block.key);
    ok("what was in a cell has moved with its day", finished.every((key) => now.days[TUESDAY_CELL - 1].blocks.some((block) => block.key === key)),
      JSON.stringify({ finished, there: now.days[TUESDAY_CELL - 1].blocks.map((block) => block.key) }));
    ok("the one Tab stop is still today's cell: a person who never moved it is not left on a day gone by (review P3-1)",
      now.days[TODAY].tab === 0 && now.days.filter((day) => day.tab === 0).length === 1, JSON.stringify(now.days.map((day) => day.tab)));
    ok("the beat after midnight still holds its revision: the range only moved forward", now.asked.every((one) => one.known !== null),
      JSON.stringify(now.asked));
  } finally {
    await midnight.close();
  }

  /* ---- 일감이 답에 다 못 실리면 말한다 (검수 r1 P2-2) --------------------------------------------- */
  const leftAt = FIRST + MONDAY_CELL * DAY + 6 * HOUR;
  const clipped = await freshWeek(browser, origin, { ...fixture, answer: { ...fixture.answer, jobs_left_out: { count: 198, before_ms: leftAt } } });
  try {
    let seen = await readWeek(clipped);
    ok("an answer that left finished jobs out says how many and from when, over the calendar",
      seen.note.includes("198") && seen.note.includes("일감"), seen.note);
    ok("every cell the cut reaches marks its count and says so; the cells after it say nothing",
      seen.days.slice(0, MONDAY_CELL + 1).every((day) => day.partial === "+" && day.label.includes("일부") && day.tip.includes("일부"))
        && seen.days.slice(MONDAY_CELL + 1).every((day) => day.partial === "" && !day.label.includes("일부")),
      JSON.stringify(seen.days.map((day) => [day.partial, day.label, day.tip])));
    const marked = await clipped.evaluate(() => getComputedStyle(document.querySelector("#board-view .board-schedule-week-head"), "::before").content);
    ok("the mark stands beside the count on screen", marked.includes("+"), marked);
    await clipped.evaluate(async (before) => {
      window.__SCHEDULE__ = { ...window.__SCHEDULE__, revision: window.__SCHEDULE__.revision + 1, jobs_left_out: { count: 3, before_ms: before } };
      refreshBoardSchedule();
      await window.__UNTIL__(() => boardScheduleAnswer.revision === window.__SCHEDULE__.revision, "the moved answer");
      paintBoardScheduleHosts();
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    }, FIRST - HOUR);
    seen = await readWeek(clipped);
    ok("a cut older than the first cell touches nothing on screen, so nothing is said",
      !seen.note.includes("일감") && seen.days.every((day) => day.partial === ""), JSON.stringify({ note: seen.note }));
  } finally {
    await clipped.close();
  }

  /* ---- 시간대: 칸의 경계는 그 기계의 자정이다 ------------------------------------------------- */
  const utc = await freshWeek(browser, origin, fixture, { timezoneId: "UTC" });
  try {
    const seen = await readWeek(utc);
    // Wed 00:50 UTC: five days back is 10-16 00:00 UTC, nine hours after Seoul's.
    ok("in UTC the cells turn at UTC's midnight and the chip reads UTC's clock",
      seen.days[0].at === Date.UTC(2026, 9, 16) && seen.days[TODAY].at === Date.UTC(2026, 9, 21)
        && seen.days.find((day) => day.current === "date")?.chip === "지금 00:50",
      JSON.stringify({ at: seen.days[0].at, chip: seen.days.map((day) => day.chip) }));
    const overnight = fixture.answer.lanes.filter((run) => run.version === "1.1.77").at(-1);
    const key = `lane:${overnight.sha}:${overnight.start_ms}`;
    // 10-18 22:48 KST to 10-19 00:42 KST is 13:48 to 15:42 UTC: one day there.
    ok("the lane that crossed Seoul's midnight is one piece in UTC",
      seen.days.flatMap((day) => day.blocks).filter((block) => block.key === key).length === 1,
      JSON.stringify(seen.days.flatMap((day) => day.blocks).filter((block) => block.key === key)));
  } finally {
    await utc.close();
  }
  // New York leaves summer time on Sunday 2026-11-01: on Monday that Sunday is the fifth cell, twenty-five hours long, and midnight is still midnight.
  const newYork = await freshWeek(browser, origin, fixture, { timezoneId: "America/New_York", now: Date.UTC(2026, 10, 2, 15) });
  try {
    const seen = await readWeek(newYork);
    const lengths = seen.days.map((day) => (day.end - day.at) / HOUR);
    const midnights = await newYork.evaluate((days) => days.map((day) => new Date(day.at).getHours() + new Date(day.at).getMinutes()), seen.days);
    ok("over a clock change every cell still runs midnight to midnight: that Sunday has twenty-five hours",
      lengths[4] === 25 && lengths.every((one, at) => at === 4 || one === 24) && midnights.every((one) => one === 0),
      JSON.stringify({ lengths, midnights }));
  } finally {
    await newYork.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testBoardScheduleWeek(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${!pass && detail ? `  — ${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
