/* 글 점검 줄의 무게 — 끝난 행 24개가 선 데스크 한 판으로 재는 수 (t-32786).
 *
 * 「숫자 없는 개선 금지」의 자다. 전은 기준 커밋의 `ui/`를 스냅샷으로 풀어 그 안에서, 후는 이
 * 트리에서 같은 파일로 잰다(`git archive <ref> ui | tar -x` → 이 파일과 `coordinator-desk.mjs`·
 * `coordinator-desk-perf.mjs`를 복사 → 실행, `coordinator-desk-perf.mjs`와 같은 방법). 기준
 * 창에는 글 점검 줄이 없으므로 같은 픽스처의 `writing` 답은 그냥 묻힌다 — 전/후의 차이가 곧 이
 * 줄의 값이다.
 *
 * 수:
 *   1. 단계를 여는 ms — 「보고됨」 칩을 누른 때부터 끝난 행 24개가 선 프레임까지, 그 사이 그리기
 *      JS(`paintCoordinatorDesk`의 가장 바깥 호출)와 그 뒤 강제 레이아웃.
 *   2. 폴당 그리기 ms — 원장 박자 하나가 치르는 그리기 JS와 강제 레이아웃. 조용한 폴과 센 수가
 *      움직인 폴을 따로, p50·p95. 조용한 폴의 mutation은 0이어야 한다.
 *   3. 요소 수 — 데스크 안의 요소, 선 글 점검 줄의 수.
 *   4. 메모리 — 모아 두기(GC) 뒤의 JS 힙: 처음, 단계를 연 뒤, 폴 2×N번 뒤. 폴이 쌓아 두는 것이
 *      없다면 뒤의 둘은 같다.
 *
 * 실행 (`WINDOW_CPU_THROTTLE=4`면 페이지의 CPU를 4배 느리게 — 저사양(cpu4x)):
 *   node ui/tests/writing-badge-perf.mjs                       Chromium, 5판 중앙값
 *   WINDOW_CPU_THROTTLE=4 node ui/tests/writing-badge-perf.mjs
 *   node ui/tests/writing-badge-perf.mjs --rounds 3 --polls 40 --json out.json */
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { installPerfHands, median, quantile } from "./coordinator-desk-perf.mjs";
import { loadNote, machineIsLoud } from "./machine-load.mjs";

/* 끝난 행이 24개(= 백엔드가 한 단계에 싣는 가장 많은 수) 서도록 픽스처의 과업 수를 키운다:
 * 「보고됨」은 60개 중 6개이므로 240개면 24개다. */
const FINISHED_ROWS_TASKS = 240;
const STAGE_CHIP = '#board-view [data-desk-block="pipeline"] [data-stage="reported"]';

/* 센 수가 움직인 폴 하나: 한 행의 센 수와 비용을 함께 바꾼다 — 전의 창은 비용 줄만 다시
 * 쓰고, 후의 창은 둘 다 다시 쓴다. 그 차이가 이 줄의 값이다. */
async function deskPoll(page, moved, index) {
  return page.evaluate(async ({ moved, index }) => {
    if (moved) {
      const noted = window.__DESK__.tasks.filter((one) => one.stage === "reported" && one.writing?.lang === "ko");
      const target = noted[index % noted.length];
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => one === target
          ? { ...one, writing: { ...one.writing, words: one.writing.words + 1 },
            cost: { ...one.cost, jev: { ...one.cost.jev, requests: one.cost.jev.requests + 1 } } } : one) };
    }
    const surface = document.querySelector("#board-view .task-board-surface");
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
    window.__PERF_PAINT__ = 0;
    refreshDeskLedger();
    for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const paint = window.__PERF_PAINT__;
    const layout = window.__PERF_LAYOUT__();
    records.push(...watch.takeRecords());
    watch.disconnect();
    return { paint, layout, mutations: records.length };
  }, { moved, index });
}

/* 모아 두기 뒤의 JS 힙(바이트) — 페이지가 쌓아 두는 것을 재려면 쓰레기가 빠진 뒤의 수여야 한다. */
async function heapBytes(cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  return (await cdp.send("Runtime.getHeapUsage")).usedSize;
}

export async function measureWritingBadge(page, { polls = 40, now } = {}) {
  const cdp = await page.context().newCDPSession(page);
  await installPerfHands(page);
  await page.evaluate(`window.__DESK_FIXTURE__ = ${coordinatorDeskFixture.toString()};`);
  await page.evaluate(async ({ now, tasks }) => {
    window.__DESK_FIXTURE__({ tasks, now });
    await window.__PERF_SETTLED__();
  }, { now, tasks: FINISHED_ROWS_TASKS });
  const heapStart = await heapBytes(cdp);
  const opened = await page.evaluate(async (selector) => {
    window.__PERF_PAINT__ = 0;
    const from = performance.now();
    document.querySelector(selector).click();
    await window.__PERF_SETTLED__();
    const js = window.__PERF_PAINT__;
    const layout = window.__PERF_LAYOUT__();
    return { ms: performance.now() - from, js, layout };
  }, STAGE_CHIP);
  const dom = await page.evaluate(() => {
    const rows = [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')];
    const shown = rows.filter((row) => {
      const line = row.querySelector(".board-desk-task-writing");
      return line !== null && !line.hidden;
    });
    return {
      rows: rows.length,
      writingLines: shown.length,
      deskElements: document.querySelector("#board-view .task-board-desk")?.getElementsByTagName("*").length ?? 0,
    };
  });
  const heapOpened = await heapBytes(cdp);
  const quiet = [];
  const moved = [];
  for (let index = 0; index < polls; index += 1) quiet.push(await deskPoll(page, false, index));
  for (let index = 0; index < polls; index += 1) moved.push(await deskPoll(page, true, index));
  const heapEnd = await heapBytes(cdp);
  await cdp.detach();
  const pick = (rows, key) => rows.map((row) => row[key]);
  return {
    openMs: opened.ms,
    openJsMs: opened.js,
    openLayoutMs: opened.layout,
    ...dom,
    quietPaintP50: median(pick(quiet, "paint")),
    quietPaintP95: quantile(pick(quiet, "paint"), 0.95),
    quietLayoutP50: median(pick(quiet, "layout")),
    quietMutationsMax: Math.max(...pick(quiet, "mutations")),
    movedPaintP50: median(pick(moved, "paint")),
    movedPaintP95: quantile(pick(moved, "paint"), 0.95),
    movedLayoutP50: median(pick(moved, "layout")),
    movedMutationsP50: median(pick(moved, "mutations")),
    movedMutationsMax: Math.max(...pick(moved, "mutations")),
    heapStartKb: heapStart / 1024,
    heapOpenedKb: heapOpened / 1024,
    heapEndKb: heapEnd / 1024,
    heapGrowthKb: (heapEnd - heapOpened) / 1024,
    polls,
  };
}

export async function measureWritingRounds({ rounds = 5, polls = 40, width = 1280, height = 800 } = {}) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const taken = [];
  // One clock for every round, so the same words are drawn each time.
  const now = Date.now();
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width, height });
        taken.push(await measureWritingBadge(page, { polls, now }));
        if (faults.length) taken.at(-1).faults = faults.slice(0, 3);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const throttle = Number(process.env.WINDOW_CPU_THROTTLE ?? "1") || 1;
  const summary = { engine: "chromium", rounds, width, height, cpuThrottle: throttle, load: loadNote(), loud: machineIsLoud };
  for (const key of Object.keys(taken[0]).filter((one) => typeof taken[0][one] === "number")) {
    const values = taken.map((one) => one[key]);
    summary[key] = median(values);
  }
  return { summary, taken };
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const { summary, taken } = await measureWritingRounds({
    rounds: Number(option("--rounds", "5")),
    polls: Number(option("--polls", "40")),
  });
  const ms = (value) => (value === null ? "—" : `${value.toFixed(2)} ms`);
  const kb = (value) => `${value.toFixed(0)} KB`;
  console.log(`chromium · cpu ×${summary.cpuThrottle} · ${summary.rounds} rounds (medians) · ${summary.width}×${summary.height} · ${summary.load}`);
  console.log(`  open the 24-row stage: ${ms(summary.openMs)} (paint JS ${ms(summary.openJsMs)}, forced layout after it ${ms(summary.openLayoutMs)})`);
  console.log(`  rows ${summary.rows} · writing lines shown ${summary.writingLines} · desk elements ${summary.deskElements}`);
  console.log(`  quiet poll: paint p50 ${ms(summary.quietPaintP50)} · p95 ${ms(summary.quietPaintP95)} · layout p50 ${ms(summary.quietLayoutP50)} · mutations max ${summary.quietMutationsMax}`);
  console.log(`  moved poll: paint p50 ${ms(summary.movedPaintP50)} · p95 ${ms(summary.movedPaintP95)} · layout p50 ${ms(summary.movedLayoutP50)} · mutations p50 ${summary.movedMutationsP50} max ${summary.movedMutationsMax}`);
  console.log(`  JS heap after GC: start ${kb(summary.heapStartKb)} · stage open ${kb(summary.heapOpenedKb)} · after ${2 * summary.polls} polls ${kb(summary.heapEndKb)} (grew ${kb(summary.heapGrowthKb)})`);
  console.log(`WRITING_BADGE_NUMBERS ${JSON.stringify(summary)}`);
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify({ summary, taken }, null, 2)}\n`);
}
