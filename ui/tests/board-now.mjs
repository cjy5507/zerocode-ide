/* 작업 상황판 「지금」 (t-67607) — 사람이 통과시킨 시안 B를 창이 그리는가.
 *
 * 시안(아티팩트 p-90883818b247d08e v2, reports/t-67561/b.html)과 같은 예시 상황을
 * 창의 데스크 답으로 세우고(`draftBoardOverlay`), 시안이 스스로 잰 넷을 같은 셈으로
 * 잰다: 1440×900 첫 화면의 숫자(개수·시각·분) ≤ 8, 묶음(카드) 3, 글자 크기 단계 4,
 * 원장의 영어 진단 노출 0. 그리고 네 탭(일정은 t-67482), 당신 차례의 단추가 원장의
 * 문(`desk_reply`·`desk_hold_resolve`·`desk_review_pass`)으로만 가는지, 원장이 없는 창에서 예전
 * 작업 목록이 그대로 서는지, 30과업·7워커의 그림이 저사양(CPU 4배 감속)에서 3 ms·
 * 요소 400 안인지를 잰다.
 *
 *   node ui/tests/board-now.mjs
 *   WINDOW_SUITES=board-now node ui/tests/window.mjs */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";
import { installPerfHands, median } from "./coordinator-desk-perf.mjs";

const SHOTS = "output/playwright/board-now";

/* 시안 B의 예시 상황 — 세 목표, 질문 하나·검토 하나, 도는 일 둘·기다림 하나·멈춤
 * 하나, 끝난 일 셋, 발행 레인 하나. 데스크 픽스처가 세운 답 위에 덮는다(그 픽스처가
 * 응답 손·보드 열기를 맡는다). `extra`는 과업 수를 늘려 성능을 잰다. A page
 * function: no closure. */
export function draftBoardOverlay({ now = Date.now(), holds = false, extra = 0, ledger = true } = {}) {
  const minute = 60_000;
  const run = "run-desk";
  const goal = (parent, parentTitle) => ({ parent, parent_title: parentTitle });
  const ship = goal("t-g1", "새 버전 내보내기");
  const board = goal("t-g2", "작업 상황판을 한눈에");
  const hands = goal("t-g3", "컴퓨터 사용을 사람처럼");
  const head = (n) => n.toString(16).padStart(7, "0").repeat(6).slice(0, 40);
  const task = (id, title, stage, extra = {}) => ({
    run, id, title, stage, parent: null, parent_title: null, gate: null, blocked_by: [], closed: null,
    created_ms: now - 90 * minute, cost: null, writing: null, progress: null, ...extra,
  });
  const tasks = [
    task("t-66912", "재선택·대기 고침", "ready", ship),
    task("t-67561", "상황판 디자인 시안 2차", "dispatched", { ...board, progress: { stage: "working", attempt: "dp-67562" } }),
    task("t-66788", "반복 관측 줄이기", "dispatched", { ...hands, progress: { stage: "working", attempt: "dp-66789" } }),
    task("t-66410", "증거 계약 설계", "failed", { ...hands, progress: { stage: "failed", attempt: "dp-66411" } }),
    task("t-66790", "헬퍼 회차 A·B·C", "reported", {
      ...hands, created_ms: now - 12 * minute,
      progress: { stage: "report_only", attempt: "dp-66791", source: head(7) },
      hand_in: { conditions: [{ number: 1, text: "빨강 시험이 초록이 됨", state: "checked" }],
        decisions: "헬퍼가 같은 창을 세 번 보던 것을 한 번으로", blocked: "", next: "" },
    }),
    task("t-62493", "컴퓨터 사용 실제 입력", "merged", hands),
    task("t-62872", "패널 정지 하나", "merged", hands),
    task("t-67064", "작업 브라우저 혼자 쓰기", "deployed", hands),
  ];
  if (holds) {
    tasks.push(task("t-66500", "남긴 폴더 보기", "gate", {
      ...hands,
      gate: { id: "gate-701", kind: "unlanded", worker: "w-66501", checkout: "/repos/zerocode/workspaces/t-66500",
        options: [], examined: { kind: "unlanded", branch: "wt/t-66500", base: "main", commits: 2 },
        question: "w-66501 exited leaving 2 commit(s) on wt/t-66500 that main does not have — land them, then resolve this with \"harvest\"; or \"dispatch again\" to hand the task out afresh" },
    }));
    tasks.push(task("t-66600", "기준 커밋 고르기", "gate", {
      ...board,
      gate: { id: "gate-702", kind: "asked", worker: null, checkout: null, examined: null,
        options: ["main 기준", "릴리스 기준"], question: "기준 커밋을 main으로 할까요, 지난 릴리스로 할까요?" },
    }));
  }
  for (let n = 0; n < extra; n += 1) {
    tasks.push(task(`t-${70000 + n}`, `더 도는 과업 ${n + 1}`, n % 3 === 0 ? "ready" : "dispatched",
      { ...(n % 2 ? board : hands), progress: { stage: "working", attempt: `dp-${80000 + n}` } }));
  }
  const workers = [
    ["w-67562", "t-67561", "상황판 디자인 시안 2차", "claude", "claude-opus-5-5", 241, 21, null],
    ["w-67491", "t-67490", "일정 타임라인", "claude", "claude-opus-5-5", 239, 48, null],
    ["w-66791", "t-66790", "헬퍼 회차 A·B·C", "claude", "claude-sonnet-5-5", 233, 64, null],
    ["w-66789", "t-66788", "반복 관측 줄이기", "claude", "claude-sonnet-5-5", 236, 26, null],
    ["w-66411", "t-66410", "증거 계약 설계", "codex", "gpt-6-astra", null, 120, now - 30 * minute],
    ["w-66913", "t-66912", "재선택·대기 고침", "claude", "claude-opus-5-5", 228, 52, null],
    ["w-66501", "t-66500", "남긴 폴더 보기", "codex", "gpt-6-astra", null, 70, null],
  ];
  for (let n = 0; n < extra && workers.length < 7; n += 1) workers.push([`w-9${n}`, `t-${70000 + n}`, `더 도는 과업 ${n + 1}`, "claude", "claude-opus-5-5", 260 + n, 5, null]);
  window.__LEDGER__ = workers.map(([worker, taskId, title, agent, model, term, age, missing]) => ({
    run, worker, agent, state: "working", ledger: "active", hearing: term ? "heard" : "gone", hearing_at: now - minute,
    checkout: `/repos/zerocode/workspaces/${taskId}`, task: title, task_id: taskId, reported: false,
    dispatch_id: `dp-${worker.slice(2)}`, dispatch_started_ms: now - age * minute, retry_of: null,
    review: { verified: false, merged: false, deployed: false, written: false },
    progress: { stage: "working", attempt: `dp-${worker.slice(2)}` }, term, at: now - 2 * minute,
    model, effort: "high", pane: `%${term ?? 0}`, asking: false, wall: null, quiet_at: null,
    pane_missing_since_ms: missing,
  }));
  window.__COLUMNS__ = [{ bucket: "attention", cards: [] }, { bucket: "working", cards: workers
    .filter(([, , , , , term]) => term)
    .map(([, taskId, title, agent, , term]) => ({
      pane: `term:${term}`, heading: title, state: "working", agent, project: "/repos/zerocode", worktree: "main",
      task: title, task_id: taskId, ask: "", said: "", you: "", parent: "", ledger: "active",
      at: now - minute, changed_at: now - 9 * minute, unseen: false,
    })) }, { bucket: "done", cards: [] }, { bucket: "idle", cards: [] }];
  const mail = [{
    run, id: "m-67495", kind: "question", from: "worker:w-67491", worker: "w-67491", task_id: "t-67490",
    task: "일정 타임라인", body: "끝날 시각을 단계별 중앙값으로 셀까요, 전체 걸린 시간으로 셀까요?",
    reason: null, resets_at_ms: null, created_ms: now - 3 * minute, delivery: "delivered", delivery_id: "d-67496",
    batch: 1, notices: 1,
  }];
  const stages = ["pending", "ready", "dispatched", "reported", "verified", "merged", "deployed", "gate", "blocked",
    "stale", "failed", "unreviewable", "nothing_to_land", "unknown", "closed"];
  window.__DESK__ = {
    revision: 1,
    runs: ledger ? [{ run, name: "시안 B", seat: true }] : [],
    mail: ledger ? mail : [], news: [], counts: { mail: ledger ? mail.length : 0, news: 0, folded: 0 },
    folded_batches: [], land_batches: [],
    tasks: ledger ? tasks : [],
    stages: stages.map((stage) => ({ stage, count: ledger ? tasks.filter((one) => one.stage === stage).length : 0 })),
  };
  window.__RELEASE__ = {
    sha: "3e13aaef1aa0b3e2c4d5e6f708192a3b4c5d6e7f", phase: "zo-test", version: "1.1.84",
    started_at: new Date(now - 41 * minute).toISOString(), updated_at: new Date(now - minute).toISOString(),
    phases: [{ name: "gate-root", rc: 0, secs: 720 }, { name: "build-app", rc: 0, secs: 1260 }],
    disk_free_gb: 21, outcome: null, reason: "",
  };
  window.__ANSWER__.desk_hold_resolve = (args) => (window.__DESK_SENT__.push({ verb: "hold", ...args }), { gateId: args.gate });
  window.__ANSWER__.desk_review_pass = (args) => (window.__DESK_SENT__.push({ verb: "pass", ...args }), { taskId: args.task });
  for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: now });
}

/* 시안의 셈(reports/t-67561/src/common.js `firstScreen`)을 창의 판에: 보이는 첫 화면
 * (창 높이 안)의 숫자 낱말, 카드, 글자 크기 단계, 원장의 영어 진단, 요소 수. 숫자는
 * 시각·분·개수만 센다 — 과업 id·sha·폴더 이름은 숫자가 아니라 이름이다(`code`와 글자
 * 뒤에 붙은 수). */
function boardFirstScreen() {
  const view = document.querySelector("#board-view");
  const surface = view.querySelector(".task-board-surface");
  const fold = window.innerHeight;
  const visible = (el) => el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })
    && el.getBoundingClientRect().top < fold && el.getBoundingClientRect().bottom > 0;
  const all = [...surface.querySelectorAll("*")].filter(visible);
  const walker = document.createTreeWalker(surface, NodeFilter.SHOW_TEXT);
  let figures = 0;
  let raw = 0;
  const words = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const host = node.parentElement;
    if (!host || !node.textContent.trim() || !visible(host) || host.closest("code, pre, details:not([open])")) continue;
    words.push(node.textContent);
    figures += (node.textContent.match(/(?<![\p{L}\-.\d])\d+(?:[:.,]\d+)*/gu) ?? []).length;
    if (/exited in|nobody has looked|before reporting|dispatch again|resolve this/i.test(node.textContent)) raw += 1;
  }
  const sizes = new Set(all.filter((el) => [...el.childNodes].some((one) => one.nodeType === 3 && one.textContent.trim()))
    .map((el) => parseFloat(getComputedStyle(el).fontSize)));
  const cards = all.filter((el) => el.matches('.board-desk-block:is([data-desk-block="asks"], [data-desk-block="release"], [data-desk-block="running"])'));
  return { figures, raw, sizes: [...sizes].sort((a, b) => b - a), cards: cards.length, elements: all.length, words: words.join(" ¦ ") };
}

/* The board alone, the window's size, so the first screen is the board's. */
async function fullBoard(page) {
  await page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
    await window.__BOARD_SETTLED__();
    for (let beat = 0; beat < 30 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

export async function testBoardNow(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(coordinatorDeskFixture, { tasks: 0, workers: 1, mail: 0 });
    await page.evaluate(draftBoardOverlay, {});
    await page.evaluate(async () => { await askReleaseStatus(); await paintBoardView(); });
    await fullBoard(page);

    /* ---- 네 탭과 맨 위 한 문장 ---- */
    const head = await page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      return {
        tab: surface.dataset.boardTab, ledger: surface.dataset.boardLedger,
        tabs: [...surface.querySelectorAll(".board-now-tab")].map((one) => [one.dataset.boardTabButton, one.getAttribute("aria-selected"), one.tabIndex]),
        title: surface.querySelector(".board-now-title").textContent,
        sub: surface.querySelector(".task-board-summary").textContent,
      };
    });
    ok("the board opens on the Now tab of four, the chosen one alone in the Tab order",
      head.tab === "now" && head.ledger === "on" && JSON.stringify(head.tabs)
        === JSON.stringify([["now", "true", 0], ["schedule", "false", -1], ["workers", "false", -1], ["history", "false", -1]]),
      JSON.stringify(head));
    ok("the sentence on top counts what waits on the person — one question and one review",
      head.title === "답할 일 2개 — 나머지는 저절로 돕니다", JSON.stringify(head));
    ok("with no finish time known yet, the second line says what the build line is doing, not a made-up clock",
      head.sub.includes("1.1.84") && head.sub.includes("zo-test"), head.sub);

    /* ---- 시안의 넷: 숫자 ≤ 8 · 묶음 3 · 글자 단계 4 · 영어 진단 0 ---- */
    const first = await page.evaluate(boardFirstScreen);
    ok("the first screen at 1440×900 carries at most eight figures", first.figures <= 8, JSON.stringify(first));
    ok("the first screen is three groups: your turn, the build line, running now", first.cards === 3, JSON.stringify(first));
    ok("the first screen sets its words in at most four sizes", first.sizes.length <= 4, JSON.stringify(first.sizes));
    ok("no English sentence of the ledger stands on the first screen", first.raw === 0, first.words);

    /* ---- 당신 차례: 질문·검토, 행 안의 첫 행동 ---- */
    const asks = await page.evaluate(() => [...document.querySelectorAll('#board-view [data-desk-block="asks"] .board-asks > li')]
      .map((row) => ({ key: row.dataset.ask, buttons: [...row.querySelectorAll("button:not([hidden])")].map((one) => one.textContent) })));
    ok("your turn holds the question first, with its answer button, then the report with read and pass",
      asks.length === 2 && asks[0].key.startsWith("review:") === false && asks[0].buttons.includes("답하기")
        && asks[1].buttons.includes("보고 읽기") && asks[1].buttons.includes("통과"), JSON.stringify(asks));
    await page.click('#board-view [data-ask^="review:"] [data-act="pass"]');
    const passed = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(done));
      return { sent: window.__DESK_SENT__.filter((one) => one.verb === "pass"),
        note: document.querySelector('#board-view [data-ask^="review:"] .board-ask-note').textContent };
    });
    ok("pass goes to the ledger as the seat, naming the attempt and the source it drew, and says so on the row",
      passed.sent.length === 1 && passed.sent[0].attempt === "dp-66791" && passed.sent[0].source?.length === 40
        && passed.sent[0].retryRequest?.startsWith("ui-board-desk_review_pass-") && passed.note.includes("t-66790"),
      JSON.stringify(passed));

    /* ---- 지금 도는 일: 목표별, 멈춘 것이 먼저, 범례 넷 ---- */
    const runs = await page.evaluate(() => {
      const table = document.querySelector('#board-view [data-desk-block="running"] .board-runs');
      return {
        goals: [...table.querySelectorAll(".board-goal:not(.is-done) .board-goal-name")].map((one) => one.textContent),
        rows: [...table.querySelectorAll(".board-run")].map((row) => [row.dataset.task, row.querySelector(".board-run-word").textContent]),
        legend: [...document.querySelectorAll('#board-view [data-desk-block="running"] .board-legend li')].map((one) => one.textContent),
      };
    });
    ok("running now folds its rows under their goals, a stopped one first within its goal",
      runs.goals.length === 3 && runs.rows.findIndex(([id]) => id.endsWith("t-66410")) < runs.rows.findIndex(([id]) => id.endsWith("t-66788")),
      JSON.stringify(runs));
    ok("the legend reads running, waiting its turn, stopped and finished",
      JSON.stringify(runs.legend) === JSON.stringify(["도는 중", "차례 기다림", "멈춤", "끝남"]), JSON.stringify(runs.legend));

    /* ---- 결정: 우리말 한 줄 · 원문은 접힌 안 · 단추는 원장의 두 답 ---- */
    await page.evaluate(draftBoardOverlay, { holds: true });
    await fullBoard(page);
    await page.evaluate(() => { boardNowChoice.asksAll = true; paintCoordinatorDesk(document.querySelector("#board-view")); });
    const hold = await page.evaluate(() => {
      const row = document.querySelector('#board-view [data-ask="hold:run-desk/gate-701"]');
      const asked = document.querySelector('#board-view [data-ask="hold:run-desk/gate-702"]');
      return {
        title: row.querySelector(".board-ask-title").textContent,
        buttons: [...row.querySelectorAll(".board-ask-acts > button:not([hidden])")].map((one) => one.textContent),
        raw: row.querySelector(".board-raw")?.open === false && row.querySelector(".board-raw-text").textContent.includes("exited leaving"),
        asked: [...asked.querySelectorAll(".board-ask-acts > button:not([hidden])")].map((one) => one.textContent),
        askedTitle: asked.querySelector(".board-ask-title").textContent,
        english: /exited|harvest|dispatch again/.test(row.innerText),
      };
    });
    ok("a hold reads in Korean — what happened and what to do — with the ledger's sentence folded behind 원문 보기",
      hold.title.includes("w-66501") && hold.title.includes("2개") && hold.raw && !hold.english, JSON.stringify(hold));
    ok("a hold offers open, collect and start again", JSON.stringify(hold.buttons) === JSON.stringify(["열기", "거두기", "다시 띄우기"]),
      JSON.stringify(hold.buttons));
    ok("a decision somebody opened offers its asker's answers and the person's own words",
      JSON.stringify(hold.asked) === JSON.stringify(["main 기준", "릴리스 기준", "직접 답하기"]) && hold.askedTitle.startsWith("기준 커밋을"),
      JSON.stringify(hold));
    await page.click('#board-view [data-ask="hold:run-desk/gate-701"] [data-act="harvest"]');
    await page.click('#board-view [data-ask="hold:run-desk/gate-702"] [data-act="option-1"]');
    const resolved = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 0));
      return window.__DESK_SENT__.filter((one) => one.verb === "hold").map(({ gate, act, answer }) => [gate, act, answer]);
    });
    ok("the two presses reach gate-resolve as the ledger's answer and the asker's option, nothing invented",
      JSON.stringify(resolved) === JSON.stringify([["gate-701", "harvest", null], ["gate-702", "answer", "릴리스 기준"]]),
      JSON.stringify(resolved));

    /* ---- 탭: 워커는 워커 탭에만, 목록은 기록 탭에 ---- */
    await page.click('#board-view [data-board-tab-button="workers"]');
    const workersTab = await page.evaluate(() => ({
      workers: document.querySelector('#board-view [data-desk-block="workers"]')?.checkVisibility() ?? false,
      asks: document.querySelector('#board-view [data-desk-block="asks"]') !== null,
      head: document.querySelector("#board-view .task-board-intro").checkVisibility(),
    }));
    ok("the Workers tab shows the workers and holds nothing of Now", workersTab.workers && !workersTab.asks && !workersTab.head,
      JSON.stringify(workersTab));
    await page.keyboard.press("ArrowRight");
    const historyTab = await page.evaluate(() => ({
      tab: document.querySelector("#board-view .task-board-surface").dataset.boardTab,
      focused: document.activeElement?.dataset.boardTabButton,
      list: document.querySelector("#board-view .task-board-filters").checkVisibility(),
    }));
    ok("an arrow key moves to History, which holds the agents' list", historyTab.tab === "history"
      && historyTab.focused === "history" && historyTab.list, JSON.stringify(historyTab));
    await page.click('#board-view [data-board-tab-button="now"]');

    /* ---- 다크·라이트 · 1440·720, 가로로 흐르지 않음 ---- */
    await mkdir(SHOTS, { recursive: true });
    for (const theme of ["dark", "light"]) {
      for (const width of [1440, 720]) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate((light) => {
          if (light) document.documentElement.setAttribute("data-theme", "light");
          else document.documentElement.removeAttribute("data-theme");
        }, theme === "light");
        await fullBoard(page);
        const size = await page.evaluate(() => {
          const surface = document.querySelector("#board-view .task-board-surface");
          return { width: surface.clientWidth, scroll: surface.scrollWidth };
        });
        ok(`the board has no horizontal overflow at ${width}px (${theme})`, size.scroll <= size.width + 1, JSON.stringify(size));
        await page.screenshot({ path: `${SHOTS}/board-${width}-${theme}.png` });
      }
    }
    await page.evaluate(() => document.documentElement.removeAttribute("data-theme"));
    await page.setViewportSize({ width: 1440, height: 900 });

    /* ---- 원장이 없는 창: 탭도 문장도 없이 예전 목록 ---- */
    await page.evaluate(draftBoardOverlay, { ledger: false });
    await fullBoard(page);
    const bare = await page.evaluate(() => {
      const surface = document.querySelector("#board-view .task-board-surface");
      return { ledger: surface.dataset.boardLedger, tabs: surface.querySelector(".board-now-tabs").checkVisibility(),
        list: surface.querySelector(".task-board-sections").checkVisibility() };
    });
    ok("a window with no ledger run keeps the agents' list whole, without tabs", bare.ledger === "none" && !bare.tabs && bare.list,
      JSON.stringify(bare));

    /* ---- 30과업·7워커, CPU 4배 감속: 그림 ≤ 3 ms · 요소 ≤ 400 ---- */
    await page.evaluate(draftBoardOverlay, { extra: 22 });
    await installPerfHands(page);
    await fullBoard(page);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    const paints = [];
    for (let round = 0; round < 15; round += 1) {
      paints.push(await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        const from = performance.now();
        paintCoordinatorDesk(view);
        return performance.now() - from;
      }));
    }
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
    const weight = await page.evaluate(() => ({
      tasks: deskLedger.tasks.length, workers: deskAgents.length,
      elements: document.querySelector("#board-view .task-board-surface").querySelectorAll("*").length,
    }));
    const p50 = median(paints);
    ok(`thirty tasks and seven workers paint in at most 3 ms on the low-spec profile (p50 ${p50.toFixed(2)} ms)`,
      weight.tasks >= 30 && weight.workers >= 7 && p50 <= 3, JSON.stringify({ ...weight, paints: paints.map((one) => one.toFixed(2)) }));
    ok(`the board stays within 400 elements (${weight.elements})`, weight.elements <= 400, JSON.stringify(weight));
    ok("the board raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testBoardNow(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
