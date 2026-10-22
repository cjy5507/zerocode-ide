/* 통합 큐의 묶음 카드 (t-45315, 설계 14절의 25·26·27·28번) — 사람이 상황판에서 묶음이 어디에
 * 섰는지 읽고, 원장이 받을 단추만 눌러, 무엇을 누가 했는지 보는가.
 *
 *   25 꺼짐이면 아무것도 없다: 묶음이 없거나 끝난 묶음만 남은 꺼진 저장소에는 카드·단추·칩이
 *      0개다. 켰다가 끈 저장소에 열린 묶음이 있으면 「꺼짐: 열린 묶음 N개」 한 줄과 취소 단추만
 *      선다.
 *   26 상태와 단추: 상태마다 칩의 말, 단추의 켜짐·꺼짐, 꺼진 단추의 까닭 — 원장이 할 거절과 같은
 *      규칙(core `promotable`·`searchable`·`restackable`·`cancellable`)이다.
 *   27 확인 창과 누른 쪽: 확인 창의 「예」 없이는 명령이 불리지 않고, 두 번 눌러도 한 번이며,
 *      답과 거절이 카드에 남고, 고른 쪽과 누른 쪽이 보인다.
 *   28 번역 계약: 창이 부르는 board.batch.* 키가 ko·en·ja·zh·es 다섯 목록에 다 있고, 영어 창의
 *      카드에는 한글이 없다.
 *
 * 묶음은 `board_desk`가 보내는 모양 그대로다(`DeskSnapshot.land_batches`의 `{run, on, batch}`,
 * batch는 core `BatchState`의 camelCase). 받아들임·거절의 판단은 러스트 시험의 것이고, 여기서
 * 재는 것은 그림과 사람의 손이다. 상태마다 밝은·어두운·좁은 창의 사진을, 그리고 프로젝트 설정의
 * 스위치 줄 사진을 남긴다.
 *
 *   node ui/tests/land-batch.mjs                   이 스위트만
 *   WINDOW_SUITES='^land-batch$' node ui/tests/window.mjs */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture } from "./coordinator-desk.mjs";

const OUT = "output/playwright/land-batch";
const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MINUTE = 60_000;
const LANGUAGES = ["ko", "en", "ja", "zh", "es"];
const BLOCK = '#board-view [data-desk-block="batches"]';

/* 창이 부르는 board.batch.* 키 — 판의 표, 설정 줄의 손, 그리고 마크업에서. */
async function batchKeys() {
  const keys = new Set();
  for (const file of ["shell-board.js", "shell-workspace.js", "index.html"]) {
    const text = await readFile(resolve(UI, file), "utf8");
    for (const [, key] of text.matchAll(/"(board\.batch\.[A-Za-z0-9]+)"/g)) keys.add(key);
  }
  return [...keys].sort();
}

/* 상태마다 한 벌. 이름은 사진 파일의 이름이기도 하다. */
const STATES = [
  { name: "stacking", chip: "쌓는 중", why: "",
    acts: [["promote", true], ["restack", true], ["cancel", false]],
    held: ["main으로 올리기, 새 main 위에 다시 쌓기: 쌓는 중"] },
  { name: "stack_stopped", chip: "쌓기 멈춤", why: "S2 t-102에서 충돌 — 파일 1개: shared.txt",
    acts: [["promote", true], ["restack-drop", false], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 쌓은 것이 없음"] },
  { name: "stack_abandoned", chip: "쌓기 끊김", why: "쌓기가 끊김: 다시 쌓기",
    acts: [["promote", true], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 쌓은 것이 없음"] },
  { name: "stacked", chip: "쌓음 · 게이트 기다림", why: "",
    acts: [["promote", true], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 끝의 초록 게이트가 아직 없음"] },
  { name: "gating", chip: "게이트 중", why: "",
    acts: [["promote", true], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 끝의 초록 게이트가 아직 없음"] },
  { name: "green", chip: "초록", why: "",
    acts: [["promote", false], ["restack", false], ["cancel", false]],
    held: [] },
  { name: "red", chip: "빨강", why: "",
    acts: [["promote", true], ["bisect", false], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 게이트가 빨강 — 범인을 찾거나 다시 쌓으세요"] },
  { name: "searching", chip: "범인 찾는 중", why: "다음 게이트: S1 · 구간 S0–S3",
    acts: [["promote", true], ["bisect", true], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 게이트가 빨강 — 범인을 찾거나 다시 쌓으세요", "범인 찾기: 범인 찾는 중"] },
  { name: "culprit_found", chip: "범인: t-102", why: "범인 t-102 — S2에서 빨강이 됨",
    acts: [["promote", true], ["bisect", true], ["restack-drop", false], ["restack", false], ["cancel", false]],
    held: ["main으로 올리기: 게이트가 빨강 — 범인을 찾거나 다시 쌓으세요", "범인 찾기: 범인을 이미 찾았음"] },
  { name: "promoting", chip: "올리는 중", why: "",
    acts: [["promote", true], ["restack", true], ["cancel", true]],
    held: ["main으로 올리기, 새 main 위에 다시 쌓기, 취소: 올리는 중 — 끝나야 결과가 정해집니다"] },
  { name: "main_moved", chip: "올리기 멈춤", why: "main이 움직였어요(0909090 → 7777777). 새 main 위에 다시 쌓기",
    acts: [["promote", false], ["restack", false], ["cancel", false]],
    held: [] },
  { name: "unfinished_files", chip: "올리기 멈춤",
    why: "main 체크아웃에 고치다 만 파일 3개: a.rs, b.rs, c.rs. 정리하고 다시 누르세요",
    acts: [["promote", false], ["restack", false], ["cancel", false]],
    held: [] },
  { name: "promoted", chip: "main에 올림", why: "", acts: [], held: [] },
  { name: "cancelled", chip: "취소됨", why: "", acts: [], held: [] },
];

/* 묶음의 모양: 원장이 접은 `BatchState` 그대로. 상태 이름은 위 표의 이름이다. */
function installBatchScene({ now, minute }) {
  const sha = (seed) => seed.repeat(20).slice(0, 40);
  const coordinator = { kind: "coordinator", run: "run-desk", seat: "seat-1", generation: 3 };
  const person = { kind: "person" };
  const members = [
    { task: "t-101", head: sha("a1") },
    { task: "t-102", head: sha("b2") },
    { task: "t-103", head: sha("c3") },
  ];
  const stack = { steps: [sha("d4"), sha("e5"), sha("f6")], tip: sha("f6"), tree: sha("7a"), tookMs: 2300, overlaps: 1 };
  const gate = (verdict) => ({
    check: "lc-1", tree: stack.tree, verdict, rc: verdict === "green" ? 0 : 1, tookMs: 4 * minute, log: "/tmp/lc-1.log",
  });
  const stacking = { operation: sha("0f"), leaseUntilMs: now + 9 * minute, by: coordinator };
  const batch = (standing, over = {}) => ({
    batch: "lb-1791940000000-1", task: "t-90", generation: 1, members, baseBranch: "main", base: sha("09"),
    repository: "/repos/zerocode", chosenBy: coordinator, chosenAt: now - 30 * minute, operations: [sha("0f")],
    dropped: [], stacking: null, stacked: null, stackHalt: null, stackAbandoned: false, prepared: null,
    tipGate: null, search: null, culprit: null, promoting: null, promoteHalt: null, promotion: null,
    cancelled: null, mergedRecorded: false, swept: false, standing, lastAt: now - 5 * minute, ...over,
  });
  const stacked = { stacking, stacked: stack };
  window.__BATCH_OF__ = {
    stacking: batch("stacking", { stacking }),
    stack_stopped: batch("stack_stopped", {
      stacking, stackHalt: { step: 2, task: "t-102", why: "conflict", files: ["shared.txt"], total: 1 },
    }),
    stack_abandoned: batch("stack_abandoned", { stacking, stackAbandoned: true }),
    stacked: batch("stacked", stacked),
    gating: batch("gating", {
      ...stacked,
      prepared: { check: "lc-1", step: 3, target: stack.tip, tree: stack.tree, folder: "/tmp/lc-1", log: "/tmp/lc-1.log" },
    }),
    green: batch("green", { ...stacked, tipGate: gate("green") }),
    red: batch("red", { ...stacked, tipGate: gate("red") }),
    searching: batch("searching", {
      ...stacked, tipGate: gate("red"), search: { lo: 0, hi: 3, next: 1, steps: [], by: person },
    }),
    culprit_found: batch("culprit_found", {
      ...stacked, tipGate: gate("red"),
      search: {
        lo: 1, hi: 2, next: null, by: person,
        steps: [{ step: 1, check: "lc-2", verdict: "green" }, { step: 2, check: "lc-3", verdict: "red" }],
      },
      culprit: { task: "t-102", step: 2, green: stack.steps[0], red: stack.steps[1], mainFirst: false },
    }),
    promoting: batch("promoting", {
      ...stacked, tipGate: gate("green"),
      promoting: {
        by: person, at: now - 2 * minute, target: stack.tip, expect: sha("09"), baseBranch: "main",
        leaseUntilMs: now + 9 * minute,
      },
    }),
    main_moved: batch("promote_stopped", {
      ...stacked, tipGate: gate("green"), promoteHalt: { why: "main_moved", files: [], total: 0, seen: sha("77") },
    }),
    unfinished_files: batch("promote_stopped", {
      ...stacked, tipGate: gate("green"),
      promoteHalt: { why: "unfinished_files", files: ["a.rs", "b.rs", "c.rs"], total: 3, seen: null },
    }),
    promoted: batch("promoted", {
      ...stacked, tipGate: gate("green"),
      promotion: {
        by: coordinator, at: now - minute, from: sha("09"), to: stack.tip, settledLater: true,
        members: members.map((member) => ({ ...member, mergedAs: stack.tip })),
      },
    }),
    cancelled: batch("cancelled", { ...stacked, cancelled: coordinator }),
  };
  window.__ENTRY__ = (name, over = {}) => ({ run: "run-desk", on: true, batch: { ...window.__BATCH_OF__[name] }, ...over });

  /* One ledger answer the way the desk takes it: the batches change, the desk reads them again and paints.
   * Settled when the desk holds THESE batches, no read is in flight and no paint is pending. */
  window.__SET_BATCHES__ = async (entries) => {
    const started = performance.now();
    window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1, land_batches: entries };
    refreshDeskLedger();
    const want = JSON.stringify(entries);
    for (let frame = 0; frame < 300; frame += 1) {
      if (JSON.stringify(deskLedger?.land_batches ?? null) === want && !deskLedgerAsking && deskPaintFrame === null) break;
      await new Promise((done) => requestAnimationFrame(done));
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    return Math.round(performance.now() - started);
  };

  window.__READ_BATCHES__ = () => {
    const view = document.querySelector("#board-view");
    const block = document.querySelector('#board-view [data-desk-block="batches"]');
    const shown = (node) => node !== null && node !== undefined && !node.hidden
      && getComputedStyle(node).display !== "none" && node.getClientRects().length > 0;
    const text = (node) => (shown(node) ? node.textContent : "");
    const offLine = block?.querySelector(".board-batch-off") ?? null;
    return {
      blockShown: shown(block),
      head: shown(block) ? block.querySelector(".board-desk-head")?.textContent ?? "" : "",
      off: text(offLine),
      cardsInDom: view?.querySelectorAll(".board-batch").length ?? 0,
      buttons: [...(view?.querySelectorAll(".board-batch-act") ?? [])].filter(shown).length,
      chips: [...(view?.querySelectorAll(".board-batch-chip") ?? [])].filter(shown).length,
      cards: [...(view?.querySelectorAll(".board-batch") ?? [])].filter(shown).map((card) => ({
        key: card.dataset.batch ?? "",
        chip: text(card.querySelector(".board-batch-chip")),
        chosen: text(card.querySelector(".board-batch-chosen")),
        pressed: text(card.querySelector(".board-batch-pressed")),
        why: text(card.querySelector(".board-batch-why")),
        said: text(card.querySelector(".board-batch-said")),
        tone: card.querySelector(".board-batch-said")?.dataset.tone ?? "",
        members: [...card.querySelectorAll(".board-batch-members > li")].map((item) => item.dataset.task ?? ""),
        acts: [...card.querySelectorAll(".board-batch-act")].filter(shown).map((button) => ({
          act: button.dataset.act ?? "", text: button.textContent, disabled: button.disabled, tip: button.dataset.tip ?? "",
        })),
        held: [...card.querySelectorAll(".board-batch-held > li")].filter(shown).map((item) => item.textContent),
        all: card.textContent,
      })),
    };
  };

  window.__SENT__ = [];
  for (const command of ["land_batch_promote", "land_batch_bisect", "land_batch_restack", "land_batch_cancel"]) {
    window.__ANSWER__[command] = (args) => {
      window.__SENT__.push({ command, ...args });
      return { state: "stacking", batch: args.batch, generation: 2 };
    };
  }
}

async function frames(page, count) {
  await page.evaluate(async (wanted) => {
    for (let frame = 0; frame < wanted; frame += 1) await new Promise((done) => requestAnimationFrame(done));
  }, count);
}

/* 단추가 없으면 누르지 않고 지나간다 — 그 빈자리는 뒤의 단언이 FAIL로 적고, 스위트는 끝까지 돈다. */
async function press(page, selector) {
  const target = page.locator(selector).first();
  if ((await target.count()) === 0) return false;
  await target.click({ timeout: 2_000 }).catch(() => {});
  return true;
}

/* 확인 창이 서기까지 기다렸다가 제목과 본문을 읽고, 「예」나 「아니요」로 답한다. */
export async function answerAsk(page, yes) {
  const shown = await page.waitForSelector("#ask-yes", { state: "visible", timeout: 5_000 }).then(() => true, () => false);
  if (!shown) return { shown, title: "", body: "", danger: false };
  const asked = await page.evaluate(() => ({
    title: document.querySelector("#ask-title, .ask-title")?.textContent ?? "",
    body: document.querySelector("#ask-body")?.textContent ?? "",
    danger: document.querySelector("#ask-yes")?.classList.contains("btn--halt") ?? false,
  }));
  await page.click(yes ? "#ask-yes" : "#ask-no");
  await frames(page, 20);
  return { shown, ...asked };
}

async function shoot(page, path, selector = BLOCK) {
  const target = page.locator(selector);
  if ((await target.count()) > 0 && (await target.first().isVisible())) {
    await target.first().screenshot({ path });
    return true;
  }
  await page.screenshot({ path });
  return false;
}

export async function testLandBatch({ browser, origin, ok, faults: shared }) {
  const { page, faults } = await openWindowTestPage(browser, origin, shared ? { faults: shared } : {});
  const measured = {};
  await mkdir(OUT, { recursive: true });
  try {
    await installBoardWaits(page);
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.waitForFunction(() => projectsRead);
    const now = Date.now();
    await page.evaluate(coordinatorDeskFixture, { tasks: 8, workers: 2, mail: 2, now });
    // The board has painted its list — on a window with a ledger run it stands on the 기록 tab
    // (t-67607), and the batch card read below is the 지금 tab's, where the board opens.
    await page.waitForSelector(".task-board-row", { state: "attached" });
    await page.evaluate(installBatchScene, { now, minute: MINUTE });

    /* ---- 25 꺼짐이면 아무것도 없다 ------------------------------------------------ */
    const off = {};
    off.none = await page.evaluate(async () => (await window.__SET_BATCHES__([]), window.__READ_BATCHES__()));
    await page.screenshot({ path: `${OUT}/off-window.png` });
    off.on = await page.evaluate(async () => (await window.__SET_BATCHES__([window.__ENTRY__("green")]), window.__READ_BATCHES__()));
    off.turnedOff = await page.evaluate(async () => (
      await window.__SET_BATCHES__([window.__ENTRY__("green", { on: false })]), window.__READ_BATCHES__()));
    await shoot(page, `${OUT}/off-open-batch.png`);
    off.endedOff = await page.evaluate(async () => (
      await window.__SET_BATCHES__([window.__ENTRY__("promoted", { on: false }), window.__ENTRY__("cancelled", { on: false })]),
      window.__READ_BATCHES__()));
    ok("25 with no batch the desk draws no batch card, button or chip",
      !off.none.blockShown && off.none.cardsInDom === 0 && off.none.buttons === 0 && off.none.chips === 0, JSON.stringify(off.none));
    ok("25 a batch in a repository with the queue on stands as a card with its chip",
      off.on.blockShown && off.on.cards.length === 1 && off.on.cards[0].chip === "초록" && off.on.off === "", JSON.stringify(off.on));
    ok("25 turned off with a batch still open: one line 「꺼짐: 열린 묶음 1개」 and its cancel alone",
      off.turnedOff.off === "꺼짐: 열린 묶음 1개" && off.turnedOff.chips === 0 && off.turnedOff.buttons === 1
        && off.turnedOff.cards[0]?.acts.length === 1 && off.turnedOff.cards[0]?.acts[0].act === "cancel"
        && off.turnedOff.cards[0]?.acts[0].disabled === false,
      JSON.stringify(off.turnedOff));
    ok("25 off with only ended batches: no card, button, chip or line — nothing is in the DOM",
      !off.endedOff.blockShown && off.endedOff.cardsInDom === 0 && off.endedOff.buttons === 0 && off.endedOff.chips === 0
        && off.endedOff.off === "",
      JSON.stringify(off.endedOff));

    /* ---- 26 상태와 단추 ----------------------------------------------------------- */
    const theme0 = await page.evaluate(() => theme);
    await page.evaluate(() => { theme = "light"; applyTheme(); });
    const seen = {};
    for (const state of STATES) {
      seen[state.name] = await page.evaluate(async (name) => {
        const ms = await window.__SET_BATCHES__([window.__ENTRY__(name)]);
        return { ms, read: window.__READ_BATCHES__() };
      }, state.name);
      await shoot(page, `${OUT}/${state.name}-light.png`);
      const card = seen[state.name].read.cards[0] ?? null;
      const acts = card ? card.acts.map((one) => [one.act, one.disabled]) : null;
      ok(`26 ${state.name}: the chip says 「${state.chip}」`, card?.chip === state.chip, JSON.stringify(card));
      ok(`26 ${state.name}: the buttons on and off are the ledger's`, JSON.stringify(acts) === JSON.stringify(state.acts),
        JSON.stringify({ acts, want: state.acts }));
      ok(`26 ${state.name}: every button held says why`, JSON.stringify(card?.held ?? null) === JSON.stringify(state.held)
        && (card?.acts ?? []).every((one) => one.disabled === (one.tip !== "")),
      JSON.stringify({ held: card?.held, acts: card?.acts, want: state.held }));
      ok(`26 ${state.name}: the stop or the search reads ${state.why ? `「${state.why}」` : "nothing"}`, (card?.why ?? null) === state.why,
        JSON.stringify(card?.why));
    }
    measured.cardMs = Math.max(...Object.values(seen).map((one) => one.ms));
    ok("26 a card follows the ledger's beat within a second", measured.cardMs < 1000, String(measured.cardMs));
    ok("26 the members are named on the card, in the stack's order",
      JSON.stringify(seen.green.read.cards[0]?.members ?? null) === JSON.stringify(["t-101", "t-102", "t-103"]),
      JSON.stringify(seen.green.read.cards[0]?.members));

    /* ---- 27 확인 창과 누른 쪽 ----------------------------------------------------- */
    const who = await page.evaluate(async () => {
      const read = async (name) => (await window.__SET_BATCHES__([window.__ENTRY__(name)]), window.__READ_BATCHES__().cards[0] ?? {});
      return {
        green: await read("green"), promoting: await read("promoting"), searching: await read("searching"),
        promoted: await read("promoted"), cancelled: await read("cancelled"),
      };
    });
    ok("27 the card says who chose the batch: 「고른 쪽: 코디」", String(who.green.chosen).startsWith("고른 쪽: 코디 · "),
      JSON.stringify(who.green));
    ok("27 a person's promote shows who pressed it: 「올리기를 누른 쪽: 사람」",
      String(who.promoting.pressed).startsWith("올리기를 누른 쪽: 사람 · "), JSON.stringify(who.promoting));
    ok("27 a person's search shows who pressed it: 「범인 찾기를 누른 쪽: 사람」",
      who.searching.pressed === "범인 찾기를 누른 쪽: 사람", JSON.stringify(who.searching));
    ok("27 a promote the next run settled names its presser and says the next run confirmed it",
      String(who.promoted.pressed).startsWith("올린 쪽: 코디 · ") && String(who.promoted.pressed).endsWith(" · 다음 실행이 확인함"),
      JSON.stringify(who.promoted));
    ok("27 a cancel shows who cancelled: 「취소한 쪽: 코디」", who.cancelled.pressed === "취소한 쪽: 코디",
      JSON.stringify(who.cancelled));

    await page.evaluate(async () => {
      const entry = (name, id) => window.__ENTRY__(name, { batch: { ...window.__BATCH_OF__[name], batch: id } });
      window.__THREE__ = [entry("green", "lb-1791940000000-1"), entry("red", "lb-1791940000000-2"), entry("culprit_found", "lb-1791940000000-3")];
      await window.__SET_BATCHES__(window.__THREE__);
    });
    await shoot(page, `${OUT}/three-cards-light.png`);
    const card = (id) => `#board-view .board-batch[data-batch="run-desk/${id}"]`;
    const act = (id, name) => `${card(id)} .board-batch-act[data-act="${name}"]`;
    const sent = () => page.evaluate(() => [...window.__SENT__]);

    await press(page, act("lb-1791940000000-1", "promote"));
    const denied = await answerAsk(page, false);
    const afterDeny = await sent();
    ok("27 the promote asks first, and says what it does before it is done",
      denied.shown && denied.title === "main으로 올릴까요?" && denied.body.includes("lb-1791940000000-1")
        && denied.body.includes("f6f6f6f") && denied.body.includes("3개"),
      JSON.stringify(denied));
    ok("27 a press the person answers 아니요 calls nothing", afterDeny.length === 0, JSON.stringify(afterDeny));

    const promoted = await page.evaluate(async () => {
      window.__ANSWER__.land_batch_promote = (args) => {
        window.__SENT__.push({ command: "land_batch_promote", ...args });
        return new Promise((done) => {
          window.__BATCH_RELEASE__ = () => done({ state: "promoted", batch: args.batch, generation: 1, from: "0909090", to: "f6f6f6f" });
        });
      };
      return true;
    });
    await press(page, act("lb-1791940000000-1", "promote"));
    const asked = await answerAsk(page, true);
    const sending = await page.evaluate(() => window.__READ_BATCHES__().cards[0]);
    await page.click(act("lb-1791940000000-1", "promote"), { force: true, timeout: 2_000 }).catch(() => {});
    await frames(page, 10);
    const askedAgain = await page.evaluate(() => document.querySelector("#ask-yes")?.offsetParent !== null);
    await page.evaluate(() => window.__BATCH_RELEASE__?.());
    await frames(page, 30);
    const done = await page.evaluate(() => ({ sent: [...window.__SENT__], card: window.__READ_BATCHES__().cards[0] }));
    ok("27 a yes calls land_batch_promote once, for the run and the batch",
      promoted && asked.shown && done.sent.length === 1 && done.sent[0].command === "land_batch_promote"
        && done.sent[0].run === "run-desk" && done.sent[0].batch === "lb-1791940000000-1",
      JSON.stringify(done.sent));
    ok("27 pressed twice while the first is in flight, the button holds and nothing is asked again",
      sending?.said === "보내는 중…" && sending?.acts.find((one) => one.act === "promote")?.disabled === true && !askedAgain,
      JSON.stringify({ sending, askedAgain }));
    ok("27 the card says what came of the press", done.card?.said === "눌렀습니다 — main에 올림" && done.card?.tone === "ok",
      JSON.stringify(done.card));

    await page.evaluate(() => {
      window.__ANSWER__.land_batch_promote = (args) => {
        window.__SENT__.push({ command: "land_batch_promote", ...args });
        return { state: "promoting", still: true, watched: true, batch: args.batch, generation: 1 };
      };
    });
    await press(page, act("lb-1791940000000-1", "promote"));
    await answerAsk(page, true);
    const still = await page.evaluate(() => window.__READ_BATCHES__().cards[0]);
    ok("27 a promote still running past its budget says so and is not called stopped",
      still?.said === "아직 올리는 중 — git이 끝나면 적습니다" && still?.tone === "ok", JSON.stringify(still));

    await page.evaluate(() => {
      window.__ANSWER__.land_batch_promote = (args) => {
        window.__SENT__.push({ command: "land_batch_promote", ...args });
        return Promise.reject("batch lb-1791940000000-1 is already promoting");
      };
    });
    await press(page, act("lb-1791940000000-1", "promote"));
    await answerAsk(page, true);
    const refused = await page.evaluate(() => window.__READ_BATCHES__().cards[0]);
    ok("27 a press the ledger refused says why, in the ledger's words, on the card",
      refused?.said === "안 됨 — batch lb-1791940000000-1 is already promoting" && refused?.tone === "refused",
      JSON.stringify(refused));

    await page.evaluate(() => { window.__SENT__ = []; });
    await press(page, act("lb-1791940000000-2", "bisect"));
    const bisectAsk = await answerAsk(page, true);
    await press(page, act("lb-1791940000000-3", "restack-drop"));
    const dropAsk = await answerAsk(page, true);
    await press(page, act("lb-1791940000000-3", "restack"));
    const restackAsk = await answerAsk(page, true);
    await press(page, act("lb-1791940000000-1", "cancel"));
    const cancelAsk = await answerAsk(page, true);
    const presses = await sent();
    ok("27 each button asks its own question", bisectAsk.title === "범인을 찾을까요?" && dropAsk.title === "범인을 빼고 다시 쌓을까요?"
      && dropAsk.body.includes("t-102") && restackAsk.title === "새 main 위에 다시 쌓을까요?"
      && cancelAsk.title === "묶음을 취소할까요?" && cancelAsk.danger === true,
    JSON.stringify({ bisectAsk, dropAsk, restackAsk, cancelAsk }));
    ok("27 the four presses carry their batch, and a restack names what it drops",
      JSON.stringify(presses) === JSON.stringify([
        { command: "land_batch_bisect", run: "run-desk", batch: "lb-1791940000000-2" },
        { command: "land_batch_restack", run: "run-desk", batch: "lb-1791940000000-3", drop: ["t-102"] },
        { command: "land_batch_restack", run: "run-desk", batch: "lb-1791940000000-3", drop: [] },
        { command: "land_batch_cancel", run: "run-desk", batch: "lb-1791940000000-1" },
      ]),
      JSON.stringify(presses));
    const heldPress = await page.evaluate(async () => {
      await window.__SET_BATCHES__([window.__ENTRY__("promoting")]);
      window.__SENT__ = [];
      document.querySelector('#board-view .board-batch-act[data-act="cancel"]')?.click();
      for (let frame = 0; frame < 20; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      return { sent: window.__SENT__.length, asked: document.querySelector("#ask-yes")?.offsetParent !== null };
    });
    ok("27 a held button calls nothing and asks nothing", heldPress.sent === 0 && heldPress.asked === false, JSON.stringify(heldPress));

    /* ---- 28 번역 계약 -------------------------------------------------------------- */
    const keys = await batchKeys();
    const missing = await page.evaluate(({ keys, languages }) => Object.fromEntries(
      languages.map((language) => [language, keys.filter((key) => !Object.hasOwn(CATALOG[language] ?? {}, key))]),
    ), { keys, languages: LANGUAGES });
    ok("28 the window names its batch words by board.batch.* keys", keys.length >= 40, `${keys.length} keys`);
    ok("28 every board.batch.* key the window names is in all five catalogs (ko, en, ja, zh, es)",
      LANGUAGES.every((language) => missing[language].length === 0), JSON.stringify(missing));
    const english = await page.evaluate(async () => {
      setLocale("en", { persist: false });
      await window.__SET_BATCHES__([window.__ENTRY__("culprit_found")]);
      scheduleDeskPaint();
      for (let frame = 0; frame < 10; frame += 1) await new Promise((done) => requestAnimationFrame(done));
      const read = window.__READ_BATCHES__();
      return { head: read.head, card: read.cards[0] ?? null };
    });
    await shoot(page, `${OUT}/culprit_found-english.png`);
    await page.evaluate(() => { setLocale("ko", { persist: false }); scheduleDeskPaint(); });
    await frames(page, 10);
    ok("28 in English the card carries no Korean", Boolean(english.card) && !/[\uac00-\ud7a3]/.test(english.card.all + english.head),
      JSON.stringify(english));

    /* ---- 사진: 상태마다 어두운 창과 좁은 창 ------------------------------------------ */
    await page.evaluate(() => { theme = "dark"; applyTheme(); });
    for (const state of STATES) {
      await page.evaluate(async (name) => window.__SET_BATCHES__([window.__ENTRY__(name)]), state.name);
      await shoot(page, `${OUT}/${state.name}-dark.png`);
    }
    await page.evaluate(() => { theme = "light"; applyTheme(); });
    await page.setViewportSize({ width: 560, height: 1200 });
    await frames(page, 6);
    for (const state of STATES) {
      await page.evaluate(async (name) => window.__SET_BATCHES__([window.__ENTRY__(name)]), state.name);
      await shoot(page, `${OUT}/${state.name}-narrow.png`);
    }
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.evaluate((held) => { theme = held; applyTheme(); }, theme0);

    /* ---- 사진: 프로젝트 설정의 스위치 줄 ----------------------------------------------- */
    const row = "label[for='project-integration-queue']";
    for (const [name, answer, mode, width] of [
      ["setting-off-light", { enabled: false, supported: true }, "light", 1280],
      ["setting-on-light", { enabled: true, supported: true }, "light", 1280],
      ["setting-on-dark", { enabled: true, supported: true }, "dark", 1280],
      ["setting-off-narrow", { enabled: false, supported: true }, "light", 560],
      ["setting-locked-light", { enabled: false, supported: false }, "light", 1280],
    ]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(({ answer, mode }) => {
        window.__ANSWER__.integration_queue_setting = () => answer;
        theme = mode;
        applyTheme();
        setSettingsOpen(true);
        showSettingsPane("project");
      }, { answer, mode });
      await frames(page, 12);
      await page.evaluate(() => document.getElementById("project-integration-queue")?.scrollIntoView({ block: "center" }));
      await frames(page, 4);
      await shoot(page, `${OUT}/${name}.png`, row);
      await page.evaluate(() => setSettingsOpen(false));
    }
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.evaluate((held) => { theme = held; applyTheme(); }, theme0);

    measured.at = new Date().toISOString();
    await writeFile(`${OUT}/measure.json`, `${JSON.stringify(measured, null, 2)}\n`);
    ok("the batch card raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testLandBatch({
      browser,
      origin,
      ok: (name, pass, detail = "") => {
        console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
        if (!pass) failures++;
      },
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
