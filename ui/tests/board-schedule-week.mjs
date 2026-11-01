/* 일정의 「한 주」 (t-84802) — 가로 차트가 아니라 요일별 일곱 칸 달력으로 서는가.
 *
 * 사람이 통과시킨 일정 B의 둘째 판이다. 잰다: 칸 일곱과 요일 머리와 오늘, 칸마다 그날의 레인·게이트·과업 블록,
 * 칸을 넘는 블록의 수와 펴기, 날을 넘는 일의 이어짐, 블록의 누름과 Enter, 실린 과업, 안내판, 범위 선택, 시간대,
 * 다섯 언어의 주 시작과 요일 이름, 좁은 폭의 쌓임, 다크·라이트, 요소 수, 조용한 박자, 키보드, axe.
 *
 *   node ui/tests/board-schedule-week.mjs */
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { boardScheduleFixture, installBoardSchedule, openBoardSchedule, SCHEDULE_NOW } from "./board-schedule.mjs";
import { BOARD_SCHEDULE_BUDGET, boardScheduleHeavyWeek } from "./board-schedule-perf.mjs";
import { axeViolations, setQualityTheme } from "./settings-quality.mjs";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
/* The fixture's moment is Wednesday 2026-10-21 09:50 in Seoul: its calendar week, Sunday first, starts 10-18 00:00. */
const SEOUL = "Asia/Seoul";
const SUNDAY = Date.UTC(2026, 9, 17, 15);
const MONDAY = SUNDAY + DAY;

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
      const more = day.querySelector(".board-schedule-more");
      return {
        at: Number(day.dataset.day), end: Number(day.dataset.dayEnd), current: day.getAttribute("aria-current"),
        label: day.getAttribute("aria-label"), tab: day.tabIndex, box: box(day),
        head: head.textContent, chip: head.dataset.now ?? "", count: Number(head.dataset.count),
        more: more.hidden ? "" : more.textContent, moreName: more.getAttribute("aria-label") ?? "",
        moreExpanded: more.getAttribute("aria-expanded"),
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
    ok("in Korean the week starts on the Sunday of the week now is in, each cell one local day after the last",
      week.days[0]?.at === SUNDAY && week.days.every((day, at) => day.at === SUNDAY + at * DAY && day.end === day.at + DAY),
      JSON.stringify(week.days.map((day) => [day.at, day.end])));
    ok("the seven heads read the weekdays in order with their dates",
      JSON.stringify(week.days.map((day) => day.head.slice(0, 1))) === JSON.stringify(["일", "월", "화", "수", "목", "금", "토"])
        && week.days[3].head.includes("21"), JSON.stringify(week.days.map((day) => day.head)));
    ok("today alone is marked: aria-current=date, a chip with words, and its name says today",
      week.days.filter((day) => day.current === "date").length === 1 && week.days[3].current === "date"
        && week.days[3].chip === "지금 09:50" && week.days[3].label.includes("오늘")
        && week.days.filter((day, at) => at !== 3).every((day) => day.chip === "" && !day.label.includes("오늘")),
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
    ok("the week's ask starts at the calendar week's first day, with no revision",
      week.asked.some((one) => one.known === null && one.fromMs === SUNDAY), JSON.stringify(week.asked.slice(-3)));
    ok("the legend says what a flat end means", week.legend.includes("이어짐"), week.legend);

    /* ---- 칸마다 그날의 일 ---------------------------------------------------------------- */
    const inDay = (at, key) => week.days[at].blocks.find((block) => block.key === key || block.key.startsWith(key));
    const sundayLanes = week.days[0].blocks.filter((block) => block.group === "lanes");
    ok("Sunday holds Sunday's release lanes, v1.1.77's three runs among them",
      sundayLanes.filter((block) => block.title.startsWith("v1.1.77 ")).length === 3 && inDay(0, "lane:") !== undefined,
      JSON.stringify(sundayLanes.map((block) => block.title)));
    ok("Monday holds the task that landed that day", week.days[1].blocks.some((block) => block.key.endsWith("t-62493"))
      || week.days[1].more !== "", JSON.stringify(week.days[1].blocks.map((block) => block.key)));
    ok("Wednesday holds today's running lane, and it reads as running",
      week.days[3].blocks.some((block) => block.group === "lanes" && block.state === "running"),
      JSON.stringify(week.days[3].blocks.map((block) => [block.key, block.state])));
    ok("a day with no work stands empty and says zero",
      week.days[4].blocks.length === 0 && week.days[4].count === 0, JSON.stringify(week.days[4]));

    /* ---- 날을 넘는 일: 잘리지 않고 이어진다 ---------------------------------------------- */
    const overnight = fixture.answer.lanes.filter((run) => run.version === "1.1.77").at(-1);
    const overnightKey = `lane:${overnight.sha}:${overnight.start_ms}`;
    const before = inDay(0, overnightKey);
    const after = inDay(1, overnightKey);
    ok("a lane over midnight stands in both days it touches",
      Boolean(before) && Boolean(after), JSON.stringify({ before, after }));
    ok("the first day's piece says since when and flows out, the next day's says until when and flows in",
      before?.flow === "out" && before.sub.startsWith("22:48부터") && after?.flow === "in" && after.sub.startsWith("00:42까지"),
      JSON.stringify({ before: [before?.flow, before?.sub], after: [after?.flow, after?.sub] }));
    ok("each piece's name tells a screen reader it continues",
      before?.name.includes("다음 날로 이어짐") && after?.name.includes("앞 날에서 이어짐"),
      JSON.stringify({ before: before?.name, after: after?.name }));
    ok("the two pieces stand on one line across the midnight, and meet at the cell's edge",
      Math.abs(before.box.top - after.box.top) <= 1 && Math.abs(before.box.height - after.box.height) <= 1
        && after.box.left - before.box.right <= 1, JSON.stringify({ before: before?.box, after: after?.box }));

    const tasks = [
      ...fixture.desk.tasks,
      task("t-90001", "dispatched", { started_ms: SCHEDULE_NOW - 2 * DAY }),
      task("t-90002", "dispatched", { started_ms: SUNDAY - 3 * DAY }),
    ];
    const lanes = [...fixture.answer.lanes.map((run) => (run.running ? {
      ...run,
      ahead: [run.ahead[0], { name: "soak", expected: { secs: 5 * 86_400, samples: 3 } }],
    } : run))];
    await moveWeek(page, { tasks, lanes });
    week = await readWeek(page);
    const middle = week.days.map((day) => day.blocks.find((block) => block.key.endsWith("t-90001")));
    ok("a task running since two days ago stands in each day to today, the day between saying only that it continues",
      middle[1]?.flow === "out" && middle[2]?.flow === "both" && middle[2].sub.startsWith("이어짐") && middle[3]?.flow === "in"
        && middle[4] === undefined, JSON.stringify(middle.map((one) => [one?.flow, one?.sub])));
    ok("a piece before now is filled, today's is filled to now",
      middle[2]?.done === 100 && middle[3]?.done > 0 && middle[3].done < 100, JSON.stringify(middle.map((one) => one?.done)));
    const cutStart = week.days[0].blocks.find((block) => block.key.endsWith("t-90002"));
    ok("a task begun before the week is toothed on the week's first day and says since when, with the date",
      cutStart?.cutStart === true && cutStart.flow === "both" && cutStart.sub.includes("부터") && cutStart.sub.includes("15"),
      JSON.stringify(cutStart));
    const cutEnd = week.days[6].blocks.find((block) => block.group === "lanes" && block.state === "running");
    ok("a lane expected past the week is toothed on the week's last day, and the days between hold it",
      cutEnd?.cutEnd === true && cutEnd.flow === "both" && week.days[5].blocks.some((block) => block.key === cutEnd.key),
      JSON.stringify(week.days.map((day) => day.blocks.filter((block) => block.state === "running" && block.group === "lanes").map((block) => block.flow))));
    await moveWeek(page, { tasks: fixture.desk.tasks, lanes: fixture.answer.lanes });

    /* ---- 칸을 넘는 블록: 수를 말하고, 누르면 다 선다 ---------------------------------------- */
    const crowd = [
      ...fixture.desk.tasks,
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
      crowded.count === 21 && hidden > 0 && crowded.more === `+${hidden}개 더` && crowded.moreExpanded === "false",
      JSON.stringify({ count: crowded.count, shown: crowded.blocks.length, more: crowded.more }));
    ok("the more button's name says which day and how many", crowded.moreName.includes(String(hidden)) && crowded.moreName.includes("22"),
      crowded.moreName);
    ok("what waits for someone is never the block folded away: the waiting task stands before the finished ones",
      crowded.blocks[0]?.key.endsWith("t-91999"), JSON.stringify(crowded.blocks.map((block) => block.key)));
    await page.evaluate(() => document.querySelectorAll("#board-view .board-schedule-week-day")[4].querySelector(".board-schedule-more").click());
    await page.evaluate(settle);
    week = await readWeek(page);
    ok("a press stands every block of that day, and the button now folds them",
      week.days[4].blocks.length === 21 && week.days[4].moreExpanded === "true" && week.days[4].more.includes("접기"),
      JSON.stringify({ shown: week.days[4].blocks.length, more: week.days[4].more }));
    ok("no block is cut by the cell: each one's full height stands inside its day",
      week.days[4].blocks.every((block) => block.box.top >= week.days[4].box.top - 1 && block.box.bottom <= week.days[4].box.bottom + 1),
      JSON.stringify(week.days[4].box));
    await page.evaluate(() => document.querySelectorAll("#board-view .board-schedule-week-day")[4].querySelector(".board-schedule-more").click());
    await page.evaluate(settle);
    week = await readWeek(page);
    ok("a second press folds them again", week.days[4].blocks.length < 21 && week.days[4].moreExpanded === "false", String(week.days[4].blocks.length));
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
    await pressGate();
    week = await readWeek(page);
    const carried = week.days.flatMap((day) => day.blocks).filter((block) => block.key.endsWith("t-67102") || block.key.endsWith("t-66539"));
    ok("a task carried by the gate says so on its block in the week, as in the day",
      carried.length > 0 && carried.every((block) => block.why.includes("l61")), JSON.stringify(carried.map((block) => block.why)));
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

    /* ---- 키보드: 달력은 탭 멈춤 하나, 안에서는 방향키 ---------------------------------------- */
    const focused = () => page.evaluate(() => {
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
    week = await readWeek(page);
    const stops = week.days.flatMap((day) => [day.tab, ...day.blocks.map((block) => block.tab)]).filter((tab) => tab === 0);
    ok("the calendar is one Tab stop, on today's cell at first", stops.length === 1 && week.days[3].tab === 0,
      JSON.stringify(week.days.map((day) => [day.tab, day.blocks.filter((block) => block.tab === 0).length])));
    await page.evaluate(() => document.querySelector('#board-view [data-group="workers"] .board-schedule-group-head').focus());
    await page.keyboard.press("Tab");
    let where = await focused();
    ok("Tab from the group heads lands on today's cell, its name read and its focus shown",
      where.day === 3 && where.cell && where.name.includes("오늘") && where.outline !== "none", JSON.stringify(where));
    await page.keyboard.press("ArrowLeft");
    where = await focused();
    ok("← goes to the day before", where.day === 2 && where.cell, JSON.stringify(where));
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowDown");
    where = await focused();
    ok("↓ on a cell goes into its first block", where.day === 3 && where.block && where.key !== "", JSON.stringify(where));
    const firstBlock = where.key;
    await page.keyboard.press("ArrowDown");
    where = await focused();
    ok("↓ on a block goes to the next one in the day", where.day === 3 && where.block && where.key !== firstBlock, JSON.stringify(where));
    await page.keyboard.press("ArrowUp");
    await page.keyboard.press("ArrowUp");
    where = await focused();
    ok("↑ from the first block goes back to the cell", where.day === 3 && where.cell, JSON.stringify(where));
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowLeft");
    where = await focused();
    ok("← on a block goes to the block at the same place the day before", where.day === 2 && where.block, JSON.stringify(where));
    await page.keyboard.press("Home");
    where = await focused();
    ok("Home goes to the week's first cell", where.day === 0 && where.cell, JSON.stringify(where));
    await page.keyboard.press("End");
    where = await focused();
    ok("End goes to the week's last cell", where.day === 6 && where.cell, JSON.stringify(where));
    week = await readWeek(page);
    ok("the Tab stop follows the focus: still one, on the cell last visited",
      week.days[6].tab === 0 && week.days.flatMap((day) => [day.tab, ...day.blocks.map((block) => block.tab)]).filter((tab) => tab === 0).length === 1,
      JSON.stringify(week.days.map((day) => day.tab)));
    await page.keyboard.press("Tab");
    where = await focused();
    ok("Tab leaves the calendar instead of walking its blocks", where.day === -1, JSON.stringify(where));
    const nameless = await page.evaluate(() => [...document.querySelectorAll("#board-view .board-schedule-week button, #board-view .board-schedule-week-day")]
      .filter((node) => node.checkVisibility() && !(node.getAttribute("aria-label") ?? node.textContent).trim()).length);
    ok("every cell and every button of the calendar has a name", nameless === 0, String(nameless));

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
      week.days[3].chip === "지금 09:51" && nextMinute < 60, String(nextMinute));
    await page.clock.setFixedTime(new Date(SCHEDULE_NOW));
    await page.evaluate(settle);

    /* ---- 요소 수 ---------------------------------------------------------------------------- */
    week = await readWeek(page);
    ok(`the fixture's week stays within ${BOARD_SCHEDULE_BUDGET.elements} elements`, week.elements <= BOARD_SCHEDULE_BUDGET.elements, String(week.elements));
    const heavy = boardScheduleHeavyWeek(SCHEDULE_NOW, SUNDAY);
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
    const sunday = stackedGaps.days[0].blocks;
    ok("stacked, a day's blocks follow one another with no empty slots between",
      sunday.every((block, at) => at === 0 || block.box.top - sunday[at - 1].box.bottom <= 8),
      JSON.stringify(sunday.map((block) => [block.box.top, block.box.bottom])));

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
    ok("the calendar raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }

  /* ---- 언어: 주 시작과 요일 이름 ---------------------------------------------------------- */
  const starts = [["ko", 0, "일"], ["en", 0, "Sun"], ["ja", 0, "日"], ["es", 1, "lun"]];
  for (const [tongue, startDay, first] of starts) {
    const local = await freshWeek(browser, origin, fixture, { tongue });
    try {
      const seen = await readWeek(local);
      const asked = startDay === 0 ? SUNDAY : MONDAY;
      ok(`in ${tongue} the week starts on the day the language's calendar starts it, named in that language`,
        seen.days[0]?.at === asked && seen.days[0].head.startsWith(first)
          && seen.asked.some((one) => one.known === null && one.fromMs === asked),
        JSON.stringify({ at: seen.days[0]?.at, head: seen.days[0]?.head, asked: seen.asked.slice(-2) }));
      ok(`in ${tongue} today is still the one marked`, seen.days.filter((day) => day.current === "date").length === 1
        && seen.days.find((day) => day.current === "date").at <= SCHEDULE_NOW
        && SCHEDULE_NOW < seen.days.find((day) => day.current === "date").end, JSON.stringify(seen.days.map((day) => day.current)));
    } finally {
      await local.close();
    }
  }
  // zh follows the same source as the others: whatever the browser's own calendar data says for it.
  const zh = await freshWeek(browser, origin, fixture, { tongue: "zh" });
  try {
    const seen = await readWeek(zh);
    const expected = await zh.evaluate(() => {
      const locale = new Intl.Locale(document.documentElement.lang);
      return (locale.getWeekInfo?.() ?? locale.weekInfo).firstDay % 7;
    });
    const firstDay = await zh.evaluate((at) => new Date(at).getDay(), seen.days[0].at);
    ok("in zh the week starts where the browser's calendar data says", firstDay === expected, JSON.stringify({ firstDay, expected }));
    // A language whose week starts earlier asks again: the answer held starts a day late.
    const asks = await zh.evaluate(async () => {
      setLocale("es", { refresh: false, persist: false });
      paintBoardScheduleHosts();
      await window.__UNTIL__(() => !boardScheduleAsking, "the language's answer");
      window.__SCHEDULE_ASKED__.length = 0;
      setLocale("ko", { refresh: false, persist: false });
      refreshBoardSchedule();
      await window.__UNTIL__(() => !boardScheduleAsking, "the earlier week's answer");
      return window.__SCHEDULE_ASKED__.slice();
    });
    if (firstDay !== 0) ok("a language change that moves the week earlier reads that earlier day", true, "");
    ok("a beat in a language whose week starts earlier asks afresh, with no revision",
      asks.some((one) => one.known === null && one.fromMs === SUNDAY), JSON.stringify(asks));
  } finally {
    await zh.close();
  }

  /* ---- 시간대: 칸의 경계는 그 기계의 자정이다 ------------------------------------------------- */
  const utc = await freshWeek(browser, origin, fixture, { timezoneId: "UTC" });
  try {
    const seen = await readWeek(utc);
    // Wed 00:50 UTC: its Sunday is 10-18 00:00 UTC, nine hours after Seoul's.
    ok("in UTC the cells turn at UTC's midnight and the chip reads UTC's clock",
      seen.days[0].at === Date.UTC(2026, 9, 18) && seen.days.find((day) => day.current === "date")?.chip === "지금 00:50",
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
  // New York leaves summer time on Sunday 2026-11-01: that Sunday has twenty-five hours, and midnight is still midnight.
  const newYork = await freshWeek(browser, origin, fixture, { timezoneId: "America/New_York", now: Date.UTC(2026, 10, 2, 15) });
  try {
    const seen = await readWeek(newYork);
    const lengths = seen.days.map((day) => (day.end - day.at) / HOUR);
    const midnights = await newYork.evaluate((days) => days.map((day) => new Date(day.at).getHours() + new Date(day.at).getMinutes()), seen.days);
    ok("over a clock change every cell still runs midnight to midnight: that Sunday has twenty-five hours",
      lengths[0] === 25 && lengths.slice(1).every((one) => one === 24) && midnights.every((one) => one === 0),
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
