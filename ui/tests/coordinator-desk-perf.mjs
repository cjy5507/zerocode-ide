/* 작업 상황판의 무게 — 과업 60·워커 5·우편 20 픽스처 하나로 재는 다섯 수 (t-6588).
 *
 * docs/design/agent-board-round4.md §실측. 「숫자 없는 개선 금지」의 자다: 전은
 * 기준 커밋의 `ui/`를 스냅샷으로 풀어 그 안에서, 후는 이 트리에서 같은 파일로
 * 잰다(`git archive <ref> ui … | tar -x` → 이 파일과 `coordinator-desk.mjs`를
 * 복사 → 실행). 기준 창에는 데스크가 없으므로 같은 픽스처에서 데스크의 답은
 * 그냥 묻히지 않는다 — 전/후의 차이가 곧 데스크의 값이다.
 *
 * 다섯 수:
 *   1. 첫 그리기 ms — 보드를 연 때부터 보드(와, 있으면 데스크)가 선 프레임까지,
 *      그 프레임의 스타일·레이아웃을 강제로 한 번 치른 값까지.
 *   2. 폴당 그리기 ms — 원장 박자 하나(`ledger:changed`)와 보드 다시 그리기
 *      하나가 치르는 그리기 JS(`paintAgentGraph`·`paintCoordinatorDesk`의 가장
 *      바깥 호출 합)와 그 뒤 강제 레이아웃. 조용한 폴과 움직인 폴을 따로, p50·p95.
 *   3. 폴당 mutation 수 — 보드 판 위의 MutationObserver 기록. 조용한 폴은 0이어야 한다.
 *   4. 목록 요소 수 — 작업 표면 안의 요소, 그리고 보드 판 전체의 요소.
 *   5. 벽시계 예산 — `machine-load.mjs`: 1분 부하가 코어 수를 넘으면 기록만 한다.
 *
 * 실행:
 *   node ui/tests/coordinator-desk-perf.mjs                  Chromium, 5판 중앙값
 *   node ui/tests/coordinator-desk-perf.mjs --engine webkit  설치 앱 웹뷰의 엔진
 *   node ui/tests/coordinator-desk-perf.mjs --rounds 3 --polls 40 --json out.json */
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { loadNote, machineIsLoud } from "./machine-load.mjs";

const median = (values) => {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};

const quantile = (values, q) => {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
};

/* The page's hands for one measurement: the painters wrapped where they are
 * looked up (a nested desk paint inside the board's paint is counted once, by
 * the outer call), a watch on the board, and a settle that waits for the
 * board's own readiness and — where the window has one — the desk's. */
async function installPerfHands(page) {
  await installBoardWaits(page);
  await page.evaluate(() => {
    window.__PERF_PAINT__ = 0;
    let depth = 0;
    const wrap = (name) => {
      const inner = window[name];
      if (typeof inner !== "function") return;
      window[name] = function measured(...args) {
        if (depth > 0) return inner.apply(this, args);
        depth += 1;
        const from = performance.now();
        try {
          return inner.apply(this, args);
        } finally {
          window.__PERF_PAINT__ += performance.now() - from;
          depth -= 1;
        }
      };
    };
    wrap("paintAgentGraph");
    wrap("paintCoordinatorDesk");
    window.__PERF_SETTLED__ = async () => {
      await window.__BOARD_SETTLED__();
      // The desk's own fetch and frame, where this window has a desk.
      for (let beat = 0; beat < 50; beat += 1) {
        const deskIdle = (typeof deskPaintFrame === "undefined" || deskPaintFrame === null)
          && (typeof deskLedgerAsking === "undefined" || !deskLedgerAsking);
        if (deskIdle) break;
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    };
    window.__PERF_LAYOUT__ = () => {
      const from = performance.now();
      document.querySelector("#board-view").getBoundingClientRect();
      void document.body.offsetHeight;
      return performance.now() - from;
    };
  });
}

/* One poll as the window takes it: the ledger's beat says it moved, and an
 * agent's hook asks the board to repaint. `moved` changes one worker's words
 * and one letter's delivery — what an ordinary beat brings. */
async function onePoll(page, moved, index) {
  return page.evaluate(async ({ moved, index }) => {
    if (moved) {
      const column = window.__COLUMNS__.find((one) => one.cards.length > 0);
      column.cards[0].said = `폴 ${index}: 게이트의 다음 단계를 돌리고 있어요.`;
      const desk = window.__DESK__;
      const letter = desk.mail[index % desk.mail.length];
      letter.delivery = letter.delivery === "pending" ? "delivered" : "pending";
      letter.delivery_id = letter.delivery === "delivered" ? "d-990" : null;
      letter.batch = letter.delivery === "delivered" ? 7 : null;
      desk.revision += 1;
    }
    const view = document.querySelector("#board-view");
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(view, { subtree: true, childList: true, attributes: true, characterData: true });
    window.__PERF_PAINT__ = 0;
    for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 1000 + index });
    scheduleAgentPaint(["board"]);
    await window.__PERF_SETTLED__();
    const paint = window.__PERF_PAINT__;
    const layout = window.__PERF_LAYOUT__();
    records.push(...watch.takeRecords());
    watch.disconnect();
    return { paint, layout, mutations: records.length };
  }, { moved, index });
}

export async function measureDesk(page, { polls = 40, now } = {}) {
  await installPerfHands(page);
  // The fixture is handed over as source (a page function carries no closure)
  // and started inside the timed evaluation, so the clock does not include
  // the round trip that carried it.
  await page.evaluate(`window.__DESK_FIXTURE__ = ${coordinatorDeskFixture.toString()};`);
  const first = await page.evaluate(async ({ now }) => {
    // Where the first task row stood on the first frame it existed, and where
    // it stands once everything settled: the difference is the layout shift a
    // person sees when the desk arrives after the list.
    let firstTop = null;
    const watch = () => {
      const row = document.querySelector("#board-view .task-board-row");
      if (row) firstTop = row.getBoundingClientRect().top;
      else requestAnimationFrame(watch);
    };
    requestAnimationFrame(watch);
    window.__PERF_PAINT__ = 0;
    const from = performance.now();
    window.__DESK_FIXTURE__({ now });
    await window.__PERF_SETTLED__();
    const js = window.__PERF_PAINT__;
    const layout = window.__PERF_LAYOUT__();
    const settledTop = document.querySelector("#board-view .task-board-row")?.getBoundingClientRect().top ?? null;
    return { ms: performance.now() - from, js, layout,
      shift: firstTop === null || settledTop === null ? null : Math.round(settledTop - firstTop) };
  }, { now });
  // Two more beats for anything the first paint deferred (a lazy fetch).
  await page.evaluate(async () => { await window.__PERF_SETTLED__(); await window.__PERF_SETTLED__(); });
  const dom = await page.evaluate(() => {
    const view = document.querySelector("#board-view");
    const surface = view.querySelector(".task-board-surface");
    const desk = view.querySelector(".task-board-desk");
    return {
      rows: surface.querySelectorAll(".task-board-row").length,
      surfaceElements: surface.getElementsByTagName("*").length,
      boardElements: view.getElementsByTagName("*").length,
      deskElements: desk ? desk.getElementsByTagName("*").length : 0,
      deskBlocks: desk ? [...desk.children].filter((one) => !one.hidden).map((one) => one.dataset.deskBlock) : [],
    };
  });
  const quiet = [];
  const moved = [];
  for (let index = 0; index < polls; index += 1) quiet.push(await onePoll(page, false, index));
  for (let index = 0; index < polls; index += 1) moved.push(await onePoll(page, true, index));
  const pick = (rows, key) => rows.map((row) => row[key]);
  return {
    firstPaintMs: first.ms,
    firstPaintJsMs: first.js,
    firstLayoutMs: first.layout,
    firstRowShiftPx: first.shift,
    ...dom,
    quietPaintP50: median(pick(quiet, "paint")),
    quietPaintP95: quantile(pick(quiet, "paint"), 0.95),
    quietLayoutP50: median(pick(quiet, "layout")),
    quietMutationsMax: Math.max(...pick(quiet, "mutations")),
    movedPaintP50: median(pick(moved, "paint")),
    movedPaintP95: quantile(pick(moved, "paint"), 0.95),
    movedLayoutP50: median(pick(moved, "layout")),
    movedLayoutP95: quantile(pick(moved, "layout"), 0.95),
    movedMutationsP50: median(pick(moved, "mutations")),
    movedMutationsMax: Math.max(...pick(moved, "mutations")),
    polls,
  };
}

function webkitType() {
  const require = createRequire(import.meta.url);
  let entry;
  try {
    entry = require.resolve("playwright");
  } catch {
    const roots = execFileSync("npm", ["root", "-g"], { encoding: "utf8" }).trim();
    entry = require.resolve(`${roots}/playwright`);
  }
  return require(entry).webkit;
}

export async function measureDeskRounds({ engine = "chromium", rounds = 5, polls = 40, width = 1280, height = 800 } = {}) {
  const { files, origin } = await createWindowServer();
  const browserType = engine === "webkit" ? webkitType() : chromium;
  const browser = await browserType.launch({ headless: true });
  const taken = [];
  // One clock for every round, so the same words are drawn each time.
  const now = Date.now();
  try {
    for (let round = 0; round < rounds; round += 1) {
      const { page, faults } = await openWindowTestPage(browser, origin);
      try {
        await page.setViewportSize({ width, height });
        taken.push(await measureDesk(page, { polls, now }));
        if (faults.length) taken.at(-1).faults = faults.slice(0, 3);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
    files.close();
  }
  const summary = { engine, rounds, width, height, load: loadNote(), loud: machineIsLoud };
  for (const key of Object.keys(taken[0]).filter((one) => typeof taken[0][one] === "number")) {
    summary[key] = median(taken.map((one) => one[key]));
  }
  summary.deskBlocks = taken[0].deskBlocks;
  return { summary, taken };
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = argv.indexOf(name);
    return at >= 0 && at + 1 < argv.length ? argv[at + 1] : fallback;
  };
  const { summary, taken } = await measureDeskRounds({
    engine: option("--engine", "chromium"),
    rounds: Number(option("--rounds", "5")),
    polls: Number(option("--polls", "40")),
  });
  const ms = (value) => (value === null ? "—" : `${value.toFixed(2)} ms`);
  console.log(`engine ${summary.engine}, ${summary.rounds} rounds (medians), ${summary.width}×${summary.height}, ${summary.load}`);
  console.log(`  first paint ${ms(summary.firstPaintMs)} (paint JS ${ms(summary.firstPaintJsMs)}, forced layout after it ${ms(summary.firstLayoutMs)}, first task row moved ${summary.firstRowShiftPx} px after it first stood)`);
  console.log(`  rows ${summary.rows} · surface elements ${summary.surfaceElements} · board elements ${summary.boardElements} · desk elements ${summary.deskElements} [${summary.deskBlocks.join(",")}]`);
  console.log(`  quiet poll: paint p50 ${ms(summary.quietPaintP50)} · p95 ${ms(summary.quietPaintP95)} · layout p50 ${ms(summary.quietLayoutP50)} · mutations max ${summary.quietMutationsMax}`);
  console.log(`  moved poll: paint p50 ${ms(summary.movedPaintP50)} · p95 ${ms(summary.movedPaintP95)} · layout p50 ${ms(summary.movedLayoutP50)} p95 ${ms(summary.movedLayoutP95)} · mutations p50 ${summary.movedMutationsP50} max ${summary.movedMutationsMax}`);
  const out = option("--json", null);
  if (out) writeFileSync(out, `${JSON.stringify({ summary, taken }, null, 2)}\n`);
}
