/* 코디네이터 데스크 (t-6588) — 작업 상황판이 코디네이터의 물음에 먼저 답하는가.
 *
 * docs/design/agent-board-round4.md. 하루 동안 코디네이터가 원장 CLI와 `df`·
 * `uptime`·레인의 `status.sh`·`simctl`로 물은 다섯(기계 띠·답할 우편·워커·과업
 * 흐름·릴리즈 레인)을 보드 위의 데스크가 그리는지, 조용한 폴이 아무것도 다시
 * 쓰지 않는지, 화면이 원장에 없는 것(ack·판정·초록)을 지어내지 않는지를 잰다.
 *
 * 픽스처(`coordinatorDeskFixture`)는 결정적이고 한 벌이다: 과업 60·워커 5·
 * 우편 20이 기본이고(성능 자 `coordinator-desk-perf.mjs`도 이것을 쓴다), 백엔드의
 * 답은 러스트가 짓는 모양 그대로다 — 셈은 러스트 시험의 것이고 여기서 재는 것은
 * 그림이다.
 *
 *   node ui/tests/coordinator-desk.mjs          이 스위트만
 *   WINDOW_SUITES=coordinator-desk node ui/tests/window.mjs */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";

/* The fixture, run inside the page. Self-contained: a page function carries
 * no closure. `now` is passed so a before/after pair draws the same clocks. */
export function coordinatorDeskFixture({ tasks = 60, workers = 5, mail = 20, now = Date.now() } = {}) {
  const minute = 60_000;
  const run = "run-desk";
  const checkout = (n) => `/repos/zerocode/workspaces/t-${n}`;
  const agents = ["claude", "codex", "claude", "zo", "claude", "codex"];
  const models = ["claude-opus-5-5", "gpt-6-astra", "claude-fable-5-1", "zo-auto", "claude-sonnet-5", "gpt-6-sol"];
  /* The six words a worker's health takes, in the order the fixture deals
   * them: in a turn, idle, asking, at its quota wall, asleep, dead. */
  const health = ["turn", "idle", "asking", "walled", "asleep", "dead"];
  const card = (term, heading, state, extra = {}) => ({
    pane: `term:${term}`, heading, state, agent: "claude", project: "/repos/zerocode",
    worktree: "main", task: "", ask: "", said: "", you: "", parent: "",
    ledger: "active", at: now - 2 * minute, changed_at: now - 9 * minute, unseen: false, ...extra,
  });
  const columns = { attention: [], working: [], done: [], idle: [] };
  const ledger = [];
  for (let n = 1; n <= workers; n += 1) {
    const word = health[(n - 1) % health.length];
    const term = 200 + n;
    const seated = word !== "asleep" && word !== "dead";
    const title = `데스크 과업 ${n}`;
    ledger.push({
      run, worker: `w-${n}`, agent: agents[(n - 1) % agents.length],
      state: word === "asleep" ? "waiting" : "working",
      ledger: word === "asleep" ? "sleeping" : "active",
      hearing: seated ? "heard" : "gone", hearing_at: now - (n + 1) * minute,
      checkout: checkout(n), task: title, task_id: `t-${n}`,
      reported: word === "idle", dispatch_id: `dp-${n}`, dispatch_started_ms: now - (30 + n) * minute,
      retry_of: null,
      review: { verified: false, merged: false, deployed: false, written: false },
      term: seated ? term : null, at: now - (40 + n) * minute,
      model: models[(n - 1) % models.length], effort: n % 2 ? "max" : "xhigh", pane: `%${n}`,
      asking: word === "asking",
      wall: word === "walled"
        ? { wall: `m-wall-${n}`, observed_at_ms: now - 5 * minute, resets_at_ms: now + 45 * minute,
          stands_until_ms: now + 47 * minute }
        : null,
      quiet_at: word === "idle" ? now - 6 * minute : null,
      pane_missing_since_ms: word === "dead" ? now - 12 * minute : null,
    });
    if (!seated) continue;
    const state = word === "turn" ? "working" : word === "asking" ? "needs-attention"
      : word === "idle" ? "done" : "idle";
    const bucket = state === "working" ? "working" : state === "needs-attention" ? "attention"
      : state === "done" ? "done" : "idle";
    columns[bucket].push(card(term, title, state, {
      agent: agents[(n - 1) % agents.length], task: title, task_id: `t-${n}`,
      said: word === "turn" ? "게이트를 돌리고 있어요." : "",
      ask: word === "asking" ? "기준 커밋을 무엇으로 할까요?" : "",
    }));
  }
  window.__COLUMNS__ = ["attention", "working", "done", "idle"]
    .map((bucket) => ({ bucket, cards: columns[bucket] }));
  window.__PANES__ = [];
  window.__LEDGER__ = ledger;

  /* The ledger's mail to the coordinator, oldest first: questions stand until
   * answered, notices until the coordinator acknowledged them. Twenty rows
   * over five kinds and the three delivery states. */
  const kinds = ["question", "went_quiet", "quota_walled", "worker_died", "went_quiet", "question", "deadlocked"];
  const reasons = ["stalled", "quota_lifted", "pane_missing", "never_spoke"];
  const deskMail = [];
  for (let n = 1; n <= mail; n += 1) {
    const kind = kinds[(n - 1) % kinds.length];
    const worker = `w-${((n - 1) % Math.max(1, workers)) + 1}`;
    const delivery = n % 3 === 0 ? "delivered" : kind === "question" && n % 5 === 0 ? "acked" : "pending";
    deskMail.push({
      run, id: `m-${900 + n}`, kind, from: `worker:${worker}`, worker,
      task_id: `t-${((n - 1) % Math.max(1, workers)) + 1}`,
      task: `데스크 과업 ${((n - 1) % Math.max(1, workers)) + 1}`,
      body: kind === "question" ? `질문 ${n}: 이 변경을 main에 바로 올려도 될까요?` : "",
      reason: kind === "went_quiet" ? reasons[n % reasons.length] : null,
      resets_at_ms: kind === "quota_walled" ? now + (30 + n) * minute : null,
      created_ms: now - (mail - n + 2) * 4 * minute,
      delivery,
      delivery_id: delivery === "delivered" ? "d-990" : null,
      batch: delivery === "delivered" ? Math.floor(mail / 3) : null,
    });
  }
  /* The run's tasks by the pipeline's own stage words, sixty by default. */
  const stages = [
    ["pending", 6], ["ready", 20], ["dispatched", 5], ["reported", 6],
    ["merged", 18], ["gate", 2], ["blocked", 2], ["failed", 1],
  ];
  const total = stages.reduce((sum, [, count]) => sum + count, 0);
  const deskTasks = [];
  let at = 0;
  for (const [stage, count] of stages) {
    const share = Math.round((count * tasks) / total);
    for (let k = 0; k < share; k += 1) {
      at += 1;
      deskTasks.push({
        run, id: `t-${100 + at}`, title: `흐름 과업 ${at} (${stage})`, stage,
        gate: stage === "gate" ? { id: `gate-${700 + at}`, question: `w-${at} 체크아웃을 수확할까요, 다시 보낼까요?` } : null,
        blocked_by: stage === "blocked" ? [`t-${100 + at - 1}`] : [],
        created_ms: now - (tasks - at) * 3 * minute,
      });
    }
  }
  const counts = stages.map(([stage]) => ({ stage, count: deskTasks.filter((one) => one.stage === stage).length }));
  window.__DESK__ = {
    revision: 1,
    runs: [{ run, name: "데스크 픽스처", seat: true }],
    mail: deskMail,
    tasks: deskTasks,
    stages: counts,
  };
  window.__ANSWER__.board_desk = () => window.__DESK__;
  window.__MACHINE__ = {
    disk: { free_bytes: 21 * 1024 ** 3, free: "21.0 GB", at: "/Users/dev/Library/Application Support/dev.zerocode.app",
      room: "tight", held_checkouts: workers },
    load: { one_minute: 55.9, cores: 12, loud: true },
    devices: { ios_booted: 2, android_booted: 0 },
  };
  window.__ANSWER__.machine_load = () => window.__MACHINE__;
  window.__ANSWER__.desk_checkouts = ({ paths } = {}) => (paths ?? []).map((path, index) => ({
    path, base: "main", beyond_base: index + 1, dirty_files: index % 2 ? 0 : index + 2,
  }));
  window.__DESK_SENT__ = [];
  window.__ANSWER__.desk_reply = (args) => (window.__DESK_SENT__.push({ verb: "reply", ...args }), { messageId: "m-reply" });
  window.__ANSWER__.desk_ack = (args) => (window.__DESK_SENT__.push({ verb: "ack", ...args }), { count: 0 });
  window.__RELEASE__ = {
    sha: "ccb6e1bda074f7fc8099f1d057aae2308d175564", phase: "build-app",
    started_at: new Date(now - 6 * minute).toISOString(), updated_at: new Date(now - minute).toISOString(),
    phases: [
      { name: "archive", rc: 0, secs: 1 },
      { name: "gate-root", rc: 0, secs: 0, skipped: "root tree unchanged since ccb6e1bd — 0 path(s) changed, none its own" },
      { name: "gate-zo", rc: 0, secs: 0, skipped: "zo tree unchanged since ccb6e1bd — 0 path(s) changed, none its own" },
      { name: "flakes", rc: 0, secs: 0 },
      { name: "push", rc: 0, secs: 2 },
    ],
    disk_free_gb: 21, outcome: null, reason: "", version: "1.1.20",
  };
  window.__ANSWER__.release_status = () => ({ status: window.__RELEASE__, installed: null, notice: null, workers });
  paneModels.set(201, "claude-opus-5-5");
  agentBoardMode = "tasks";
  taskBoardFilter = "all";
  agentGraphSelectedKey = null;
  boardQuery = "";
  boardBroken = false;
  openBoard();
}

/* Every mutation the task surface takes while `act` (a page function) runs —
 * a quiet poll's must be 0. Three evaluations: the watch stands before the act
 * and is read two frames after it, so a paint the act scheduled is counted. */
export async function deskMutations(page, act) {
  await page.evaluate(() => {
    const surface = document.querySelector("#board-view .task-board-surface");
    window.__DESK_RECORDS__ = [];
    window.__DESK_WATCH__ = new MutationObserver((batch) => window.__DESK_RECORDS__.push(...batch));
    window.__DESK_WATCH__.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
  });
  await page.evaluate(act);
  return page.evaluate(async () => {
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    window.__DESK_RECORDS__.push(...window.__DESK_WATCH__.takeRecords());
    window.__DESK_WATCH__.disconnect();
    return window.__DESK_RECORDS__.length;
  });
}

/* One ledger beat and one hook repaint, as the window takes them, with the
 * whole board watched — head, list, desk and inspector. `change` (a page
 * function) moves whatever the beat is to bring before it lands. */
export async function boardPollMutations(page, change = null) {
  if (change) await page.evaluate(change);
  return page.evaluate(async () => {
    const view = document.querySelector("#board-view");
    const records = [];
    const watch = new MutationObserver((batch) => records.push(...batch));
    watch.observe(view, { subtree: true, childList: true, attributes: true, characterData: true });
    for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: Date.now() });
    scheduleAgentPaint(["board"]);
    await window.__BOARD_SETTLED__();
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    records.push(...watch.takeRecords());
    watch.disconnect();
    return records.map((record) => `${record.type}:${record.target.className || record.target.parentElement?.className}:${record.attributeName ?? ""}`);
  });
}

export async function testCoordinatorDesk(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await installBoardWaits(page);
    // The first frame the list stands in already carries the desk: the rows
    // never move down under a person's eye when the desk's answers land.
    await page.evaluate(() => {
      window.__FIRST_ROW_TOP__ = null;
      const watch = () => {
        const row = document.querySelector("#board-view .task-board-row");
        if (row) window.__FIRST_ROW_TOP__ = row.getBoundingClientRect().top;
        else requestAnimationFrame(watch);
      };
      requestAnimationFrame(watch);
    });
    await page.evaluate(coordinatorDeskFixture);
    await page.waitForSelector(".task-board-row");
    const firstFrame = await page.evaluate(async () => {
      await window.__BOARD_SETTLED__();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const settled = document.querySelector("#board-view .task-board-row").getBoundingClientRect().top;
      return { first: window.__FIRST_ROW_TOP__, settled, moved: Math.round(settled - window.__FIRST_ROW_TOP__) };
    });
    ok("the desk stands in the board's first frame: the first task row does not move when the desk's answers land",
      firstFrame.moved === 0, JSON.stringify(firstFrame));
    await page.evaluate(async () => {
      await askReleaseStatus();
      await paintBoardView();
    });

    /* ---- 기계 띠: `df`·`uptime`·`simctl`의 자리, 빌린 기기는 상태 바의 문장 ---- */
    const machine = await page.evaluate(async () => {
      paintEmulatorLoans({ count: 1, lastUsedMs: Date.now() });
      askMachineLoad();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      const strip = document.querySelector('#board-view [data-desk-block="machine"]');
      const segment = (id) => strip?.querySelector(`[data-segment="${id}"]`);
      const read = (id) => ({ text: segment(id)?.textContent, tone: segment(id)?.className });
      return { shown: Boolean(strip) && !strip.hidden, first: strip === strip?.parentElement.firstElementChild,
        disk: read("disk"), load: read("load"), ios: read("ios"), android: read("android"), loans: read("loans"),
        chip: document.querySelector("#sb-loans-words").textContent,
        order: [...(strip?.querySelectorAll("[data-segment]") ?? [])].map((node) => node.dataset.segment) };
    });
    ok("the machine strip answers df, uptime and simctl: disk with the ledger's worktree rule, load against the cores, booted devices",
      machine.shown && machine.first &&
      machine.disk.text === "디스크 21.0 GB 남음 · 워커 체크아웃 5개가 자라면 모자람" && machine.disk.tone.includes("is-wait") &&
      machine.load.text === "부하 55.9 · 코어 12 · 붐빔" && machine.load.tone.includes("is-wait") &&
      machine.ios.text === "iOS 시뮬레이터 2" && machine.android.text === "Android 에뮬레이터 0" &&
      machine.order.join() === "disk,load,ios,android,loans", JSON.stringify(machine));
    ok("lent devices are the status bar's own sentence from the same state, not a second count",
      machine.loans.text === machine.chip && machine.chip === "빌린 기기 1 · 방금 사용", JSON.stringify(machine));
    const refused = await page.evaluate(async () => {
      window.__MACHINE__ = { disk: { ...window.__MACHINE__.disk, free: "8.0 GB", room: "refused" },
        load: { one_minute: 3.2, cores: 12, loud: false }, devices: { ios_booted: null, android_booted: 1 } };
      askMachineLoad();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      const strip = document.querySelector('#board-view [data-desk-block="machine"]');
      const segment = (id) => strip.querySelector(`[data-segment="${id}"]`);
      const out = { disk: segment("disk").textContent, tone: segment("disk").className, load: segment("load").textContent,
        loadTone: segment("load").className, ios: Boolean(segment("ios")), android: segment("android")?.textContent };
      window.__MACHINE__ = null;
      paintEmulatorLoans({ count: 0 });
      askMachineLoad();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      out.gone = strip.hidden;
      return out;
    });
    ok("a disk too small for one worktree is said in the halt ink; an unknown count is not drawn; nothing known folds the strip",
      refused.disk === "디스크 8.0 GB 남음 · 새 워크트리 하나도 들어가지 않음" && refused.tone.includes("is-halt") &&
      refused.load === "부하 3.2 · 코어 12" && !refused.loadTone.includes("is-wait") && !refused.ios &&
      refused.android === "Android 에뮬레이터 1" && refused.gone, JSON.stringify(refused));

    /* ---- 한눈: 데스크가 작업 목록을 첫 화면 밖으로 밀어내지 않는다 ------------- */
    await page.evaluate(async () => {
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await page.setViewportSize({ width: 1998, height: 1069 });
    const glance = await page.evaluate(async () => {
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      const view = document.querySelector("#board-view");
      const held = view.getAttribute("style") ?? "";
      Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const desk = view.querySelector(".task-board-desk").getBoundingClientRect();
      const row = view.querySelector(".task-board-row").getBoundingClientRect();
      view.setAttribute("style", held);
      return { deskHeight: Math.round(desk.height), firstTaskRowTop: Math.round(row.top), screen: window.innerHeight };
    });
    await page.setViewportSize({ width: 1280, height: 860 });
    ok("on a 1998×1069 board the desk leaves the first task row on the first screen",
      glance.firstTaskRowTop < glance.screen, JSON.stringify(glance));

    /* ---- 답할 우편: `check --peek`의 자리, 답하기와 확인은 원장의 뜻 그대로 ------ */
    const settleMail = () => page.evaluate(async () => {
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    await settleMail();
    const readMail = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="mail"]');
      const letters = [...(block?.querySelectorAll(".board-desk-letter") ?? [])].map((row) => ({
        key: row.dataset.letter,
        kind: row.querySelector(".board-desk-letter-kind").textContent,
        who: row.querySelector(".board-desk-letter-who").textContent,
        age: row.querySelector(".board-desk-letter-age").textContent,
        body: row.querySelector(".board-desk-letter-body").hidden ? "" : row.querySelector(".board-desk-letter-body").textContent,
        delivery: row.querySelector(".board-desk-letter-delivery").textContent,
        deliveryTip: row.querySelector(".board-desk-letter-delivery").dataset.tip,
        note: row.querySelector(".board-desk-letter-note").hidden ? "" : row.querySelector(".board-desk-letter-note").textContent,
        act: row.querySelector(".board-desk-letter-act").hidden ? "" : row.querySelector(".board-desk-letter-act").textContent,
        form: Boolean(row.querySelector(".board-desk-reply:not([hidden])")),
      }));
      const more = block?.querySelector(".board-desk-letters-more");
      const unseated = block?.querySelector(".board-desk-unseated");
      return { shown: Boolean(block) && !block.hidden, head: block?.querySelector(".board-desk-head")?.textContent,
        letters, more: more && !more.hidden ? more.textContent : "",
        unseated: unseated && !unseated.hidden ? unseated.textContent : "",
        order: [...document.querySelectorAll("#board-view .task-board-desk > .board-desk-block:not([hidden])")].map((node) => node.dataset.deskBlock) };
    });
    const mail = await readMail();
    const letter = (id) => mail.letters.find((one) => one.key === `run-desk/${id}`);
    ok("the mail the coordinator owes stands oldest first, five at a time, with how many more",
      mail.shown && mail.head === "답할 우편 · 20" && mail.letters.length === 5 && mail.more === "15통 더 보기" &&
      mail.letters.map((one) => one.key).join() === [901, 902, 903, 904, 905].map((n) => `run-desk/m-${n}`).join() &&
      mail.order.indexOf("mail") === mail.order.indexOf("machine") + 1, JSON.stringify(mail));
    ok("each letter says its kind, whom it concerns, its age and where it stands in the coordinator's inbox",
      letter("m-901")?.kind === "질문" && letter("m-901").who === "w-1 · 데스크 과업 1" &&
      /^\d+(분|시간) 전$/.test(letter("m-901").age) && letter("m-901").body.startsWith("질문 1:") &&
      letter("m-901").delivery === "배달 전" && letter("m-901").deliveryTip === "배달 전 · 코디네이터가 아직 안 읽음" &&
      letter("m-902")?.kind === "조용해짐" && letter("m-902").body === "판이 보이지 않음" &&
      letter("m-903")?.kind === "한도 벽" && letter("m-903").body.startsWith("재설정까지") &&
      letter("m-903").delivery === "받음 d-990" && letter("m-903").deliveryTip === "받음 · 묶음 d-990" &&
      letter("m-904")?.kind === "워커 끝남" && letter("m-904").body === "보고 전에 판이 끝났어요",
      JSON.stringify(mail.letters));
    ok("a question is answered where it stands; a notice the coordinator holds acknowledges its whole batch, one it has not read offers nothing",
      letter("m-901").act === "답하기" && letter("m-903").act === "확인 · 이 묶음 6통" &&
      letter("m-902").act === "" && letter("m-904").act === "", JSON.stringify(mail.letters));

    await page.click('#board-view [data-letter="run-desk/m-901"] .board-desk-letter-act');
    await page.fill('#board-view [data-letter="run-desk/m-901"] .board-desk-reply-field', "그대로 main에 올리세요.");
    const drafted = await page.evaluate(async () => {
      // An unrelated ledger beat must not take the words or the focus away.
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1 };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const field = document.querySelector('#board-view [data-letter="run-desk/m-901"] .board-desk-reply-field');
      return { value: field.value, focused: document.activeElement === field };
    });
    ok("a reply being written keeps its words and its focus through a ledger beat", drafted.value === "그대로 main에 올리세요." &&
      drafted.focused, JSON.stringify(drafted));
    await page.evaluate(() => window.__FAIL__.add("desk_reply"));
    await page.click('#board-view [data-letter="run-desk/m-901"] .board-desk-reply-send');
    await settleMail();
    const failed = await page.evaluate(() => ({
      error: document.querySelector('#board-view [data-letter="run-desk/m-901"] .board-desk-reply-error').textContent,
      first: deskDrafts.get("m-901")?.retry?.id,
    }));
    await page.evaluate(() => window.__FAIL__.delete("desk_reply"));
    await page.click('#board-view [data-letter="run-desk/m-901"] .board-desk-reply-send');
    await settleMail();
    const sent = await page.evaluate(() => window.__DESK_SENT__.filter((one) => one.verb === "reply"));
    const afterReply = await readMail();
    const replied = afterReply.letters.find((one) => one.key === "run-desk/m-901");
    ok("a reply goes to the ledger as the run's coordinator seat, and an uncertain one keeps its request name for the retry",
      failed.error.includes("refused: desk_reply") && sent.length === 1 &&
      sent[0].run === "run-desk" && sent[0].message === "m-901" && sent[0].body === "그대로 main에 올리세요." &&
      sent[0].retryRequest === failed.first && /^ui-desk-reply-/.test(sent[0].retryRequest),
      JSON.stringify({ failed, sent }));
    ok("the screen invents no answer: the letter stays until the ledger records it",
      replied && replied.note === "답을 보냄 · 원장에 적히면 목록에서 빠져요" && replied.act === "" && !replied.form,
      JSON.stringify(replied));
    await page.click('#board-view [data-letter="run-desk/m-903"] .board-desk-letter-act');
    await settleMail();
    const acked = await page.evaluate(() => window.__DESK_SENT__.filter((one) => one.verb === "ack"));
    const afterAck = await readMail();
    const held = afterAck.letters.find((one) => one.key === "run-desk/m-903");
    ok("the screen invents no acknowledgement: the batch is asked of the ledger, and the letter stays until the ledger says so",
      acked.length === 1 && acked[0].run === "run-desk" && acked[0].delivery === "d-990" &&
      held && held.note === "묶음을 확인함 · 원장에 적히면 목록에서 빠져요" && afterAck.head === "답할 우편 · 20",
      JSON.stringify({ acked, held }));
    const landed = await page.evaluate(async () => {
      window.__DESK__ = { ...window.__DESK__, mail: window.__DESK__.mail.filter((one) =>
        one.id !== "m-901" && one.delivery_id !== "d-990") };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return window.__DESK__.mail.length;
    });
    await settleMail();
    const cleared = await readMail();
    ok("once the ledger records the answer and the acknowledgement, those letters leave the desk",
      cleared.head === `답할 우편 · ${landed}` && !cleared.letters.some((one) => one.key === "run-desk/m-901" || one.key === "run-desk/m-903"),
      JSON.stringify(cleared));
    const unseated = await page.evaluate(async () => {
      window.__DESK__ = { ...window.__DESK__, runs: window.__DESK__.runs.map((one) => ({ ...one, seat: false })) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const row = document.querySelector("#board-view .board-desk-letter.is-question");
      const unseated = document.querySelector("#board-view .board-desk-unseated");
      const out = { said: unseated.hidden ? "" : unseated.textContent,
        act: row.querySelector(".board-desk-letter-act").hidden };
      window.__DESK__ = { ...window.__DESK__, runs: window.__DESK__.runs.map((one) => ({ ...one, seat: true })) };
      refreshDeskLedger();
      return out;
    });
    ok("a run whose coordinator seat this window does not hold offers no answer and says why",
      unseated.act && unseated.said === "이 창에 그 런의 코디네이터 자리가 없어 여기서는 답할 수 없어요", JSON.stringify(unseated));
    await settleMail();

    /* ---- 워커: `worker-list`의 자리, 건강이 나쁜 워커가 먼저 ------------------- */
    await page.evaluate(async () => {
      askDeskCheckouts();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleMail();
    const readWorkers = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="workers"]');
      return {
        shown: Boolean(block) && !block.hidden,
        head: block?.querySelector(".board-desk-head")?.textContent,
        order: [...(block?.children[0] ? block.querySelectorAll(".board-desk-worker") : [])].map((row) => row.dataset.worker),
        rows: Object.fromEntries([...(block?.querySelectorAll(".board-desk-worker") ?? [])].map((row) => [
          row.dataset.worker.split("/")[1], {
            health: row.querySelector(".board-desk-worker-health").textContent,
            cls: row.className.replace("board-desk-worker", "").trim(),
            age: row.querySelector(".board-desk-worker-age").textContent,
            task: row.querySelector(".board-desk-worker-task").textContent,
            facts: row.querySelector(".board-desk-worker-facts").textContent,
            disabled: row.querySelector(".board-desk-worker-main").disabled,
          }])),
      };
    });
    const roster = await readWorkers();
    ok("the roster answers worker-list with the unhealthy first: wall, question, asleep, in a turn, idle",
      roster.shown && roster.head === "워커 · 5" &&
      roster.order.join() === "run-desk/w-4,run-desk/w-3,run-desk/w-5,run-desk/w-1,run-desk/w-2", JSON.stringify(roster));
    ok("each worker's health is one word from the ledger's facts, the wall with its reset",
      /^한도 벽 · 재설정까지 4\d분$/.test(roster.rows["w-4"].health) && roster.rows["w-4"].cls === "is-walled" &&
      roster.rows["w-3"].health === "답 기다림" && roster.rows["w-5"].health === "잠듦" &&
      roster.rows["w-1"].health === "턴 중" && roster.rows["w-2"].health === "유휴" &&
      /^\d+분 전$/.test(roster.rows["w-1"].age), JSON.stringify(roster.rows));
    ok("each worker says its agent, model and effort, pane, checkout, commits ahead and changed files, and the ledger's review word",
      roster.rows["w-1"].facts === "Claude · claude-opus-5-5 · max · 판 201 · t-1 · 커밋 1개 앞섬 · 바뀐 파일 2" &&
      roster.rows["w-5"].facts.includes("%5") && roster.rows["w-5"].facts.includes("t-5") &&
      roster.rows["w-2"].task === "데스크 과업 2 · 검증 대기" && roster.rows["w-5"].disabled && !roster.rows["w-3"].disabled,
      JSON.stringify(roster.rows));
    await page.click('#board-view [data-worker="run-desk/w-3"] .board-desk-worker-main');
    const picked = await page.evaluate(() => ({
      key: agentGraphSelectedKey,
      open: document.querySelector("#board-view").classList.contains("is-inspector-open"),
      title: document.querySelector("#board-view .agent-inspector-title").textContent,
    }));
    ok("a worker with a pane opens its task in the inspector", picked.key === "agent:term:203" && picked.open &&
      picked.title === "데스크 과업 3", JSON.stringify(picked));
    await page.evaluate(async () => {
      agentGraphSelectedKey = null;
      setAgentGraphInspectorOpen(document.querySelector("#board-view"), false);
      await paintBoardView(boardTab(), { force: true });
    });
    const turned = await page.evaluate(async () => {
      const w2 = window.__LEDGER__.find((row) => row.worker === "w-2");
      const w4 = window.__LEDGER__.find((row) => row.worker === "w-4");
      w2.pane_missing_since_ms = Date.now() - 60_000;
      w4.wall = { ...w4.wall, stands_until_ms: Date.now() - 1 };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return true;
    });
    await settleMail();
    const after = await readWorkers();
    ok("a pane the reconciler proved gone comes first in the halt ink; a wall that no longer stands stops explaining the silence",
      turned && after.order[0] === "run-desk/w-2" && after.rows["w-2"].health === "판 없음" &&
      after.rows["w-2"].cls === "is-gone" && after.rows["w-4"].health === "유휴", JSON.stringify(after));

    /* ---- 과업 흐름: `task-list`의 자리, 멈춰 선 단계가 먼저 펼쳐진다 ---------- */
    const settleDesk = () => page.evaluate(async () => {
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    await settleDesk();
    const readPipeline = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="pipeline"]');
      return {
        shown: Boolean(block) && !block.hidden,
        head: block?.querySelector(".board-desk-head")?.textContent,
        chips: [...(block?.querySelectorAll(".board-desk-stage") ?? [])].map((chip) =>
          `${chip.dataset.stage}:${chip.querySelector(".board-desk-stage-count").textContent}:${chip.getAttribute("aria-pressed")}:${chip.className.replace("board-desk-stage", "").trim()}`),
        words: [...(block?.querySelectorAll(".board-desk-stage-word") ?? [])].map((node) => node.textContent),
        rows: [...(block?.querySelectorAll(".board-desk-task") ?? [])].map((row) => ({
          id: row.querySelector(".board-desk-task-id").textContent,
          note: row.querySelector(".board-desk-task-note").hidden ? "" : row.querySelector(".board-desk-task-note").textContent })),
        more: block?.querySelector(".board-desk-more")?.hidden ? "" : block?.querySelector(".board-desk-more")?.textContent,
      };
    });
    const pipeline = await readPipeline();
    ok("the task flow counts every stage the ledger gives, in the flow's order, stuck stages last",
      pipeline.shown && pipeline.head === "과업 흐름 · 60" &&
      pipeline.chips.map((chip) => chip.split(":").slice(0, 2).join(":")).join() ===
        "pending:6,ready:20,dispatched:5,reported:6,merged:18,gate:2,blocked:2,failed:1" &&
      pipeline.words.join() === "선행 대기,준비,진행,보고됨,병합,게이트,막힘,실패", JSON.stringify(pipeline));
    ok("a stuck stage with tasks wears its signal and opens first, naming the gate and its question",
      pipeline.chips.includes("gate:2:true:is-wait") && pipeline.chips.includes("failed:1:false:is-halt") &&
      pipeline.chips.includes("blocked:2:false:is-wait") && pipeline.chips.includes("ready:20:false:is-flow") &&
      pipeline.rows.length === 2 && pipeline.rows.every((row) => /^gate-\d+ · w-\d+ 체크아웃을 수확할까요/.test(row.note)),
      JSON.stringify(pipeline));
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="blocked"]');
    const blocked = await readPipeline();
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="blocked"]');
    const closed = await readPipeline();
    ok("a pressed stage lists its tasks with what holds them; pressing it again folds the list",
      blocked.chips.includes("blocked:2:true:is-wait") && blocked.chips.includes("gate:2:false:is-wait") &&
      blocked.rows.length === 2 && blocked.rows.every((row) => row.note.startsWith("실패한 선행: t-")) &&
      closed.rows.length === 0 && closed.chips.every((chip) => chip.split(":")[2] === "false"),
      JSON.stringify({ blocked, closed }));
    const long = await page.evaluate(async () => {
      window.__DESK__ = { ...window.__DESK__, stages: window.__DESK__.stages.map((one) =>
        one.stage === "ready" ? { ...one, count: 44 } : one) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return true;
    });
    await settleDesk();
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="ready"]');
    const window44 = await readPipeline();
    ok("a stage longer than the rows the ledger sent says how many more the ledger holds",
      long && window44.rows.length === 20 && window44.more === "24개 더 — 원장에 있음" &&
      window44.head === "과업 흐름 · 84", JSON.stringify(window44));

    /* ---- 릴리즈 레인: 레인의 `status.json` 그대로 ---------------------- */
    const release = await page.evaluate(() => {
      const card = document.querySelector('#board-view [data-desk-block="release"]');
      return {
        shown: Boolean(card) && !card.hidden,
        head: card?.querySelector(".board-desk-head")?.textContent,
        version: card?.querySelector(".board-desk-release-version")?.textContent,
        sha: card?.querySelector(".board-desk-release-sha")?.textContent,
        verdict: card?.querySelector(".board-desk-release-verdict")?.textContent,
        clock: card?.querySelector(".board-desk-release-clock")?.textContent,
        phases: [...(card?.querySelectorAll(".board-desk-release-phase") ?? [])].map((node) => node.textContent),
        running: card?.querySelector(".board-desk-release-phase.is-running")?.textContent,
        skips: [...(card?.querySelectorAll(".board-desk-release-skips li") ?? [])].map((node) => node.textContent),
        reasonHidden: card?.querySelector(".board-desk-release-reason")?.hidden,
      };
    });
    ok("the release card reads the lane's own status: version, sha, the phase running and how long",
      release.shown && release.head === "릴리즈 레인" && release.version === "1.1.20" && release.sha === "ccb6e1bd" &&
      release.verdict === "진행 중 · build-app" && /^\d+분째$/.test(release.clock ?? "") &&
      release.running === "build-app",
      JSON.stringify(release));
    ok("a skipped phase says it was skipped and why, and a running lane invents no verdict",
      release.phases.includes("gate-root 건너뜀") && release.phases.includes("archive 1초") &&
      release.skips.length === 2 && release.skips[0].startsWith("gate-root: root tree unchanged") &&
      release.reasonHidden === true, JSON.stringify(release));
    const ended = await page.evaluate(async () => {
      window.__RELEASE__ = { ...window.__RELEASE__, outcome: "red", reason: "gate-zo rc=101",
        phases: [...window.__RELEASE__.phases, { name: "build-app", rc: 101, secs: 288 }] };
      await askReleaseStatus();
      paintCoordinatorDesk(document.querySelector("#board-view"));
      const card = document.querySelector('#board-view [data-desk-block="release"]');
      const red = { verdict: card.querySelector(".board-desk-release-verdict").textContent,
        reason: card.querySelector(".board-desk-release-reason").textContent,
        reasonShown: !card.querySelector(".board-desk-release-reason").hidden,
        failed: card.querySelector(".board-desk-release-phase.is-failed")?.textContent,
        mark: card.querySelector(".agent-graph-node-state").className,
        clock: card.querySelector(".board-desk-release-clock").textContent };
      window.__RELEASE__ = null;
      await askReleaseStatus();
      paintCoordinatorDesk(document.querySelector("#board-view"));
      return { red, gone: card.hidden };
    });
    ok("a red lane says red with the lane's reason and the failed phase; no lane folds the card",
      ended.red.verdict === "빨강" && ended.red.reasonShown && ended.red.reason === "gate-zo rc=101" &&
      ended.red.failed === "build-app 288초" && ended.red.mark.includes("is-failed") &&
      ended.red.clock.includes("걸림") && ended.gone, JSON.stringify(ended));

    /* ---- 조용한 폴은 아무것도 다시 쓰지 않는다 ------------------------------ */
    await page.evaluate(async () => {
      window.__RELEASE__ = { ...window.__RELEASE__ ?? {}, sha: "ccb6e1bda074f7fc8099f1d057aae2308d175564",
        version: "1.1.20", phase: "build-app", outcome: null, phases: [], started_at: new Date().toISOString() };
      await askReleaseStatus();
      await paintBoardView();
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    const quiet = await deskMutations(page, async () => {
      await paintBoardView();
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    ok("a quiet poll repaints the board and the desk with zero mutations", quiet === 0, `mutations ${quiet}`);
    await installBoardWaits(page);
    await boardPollMutations(page);
    const beat = await boardPollMutations(page);
    ok("a quiet ledger beat writes nothing anywhere on the board, its head included", beat.length === 0,
      JSON.stringify(beat));
    const moved = await boardPollMutations(page, () => {
      window.__COLUMNS__.find((column) => column.cards.length > 0).cards[0].said = "한 워커의 말만 바뀐 박자";
    });
    ok("a beat that moved one worker's words writes those words and the board's signature, nothing else",
      moved.length <= 3 && moved.some((one) => one.includes("task-board-message")), JSON.stringify(moved));

    /* ---- 폭: 어느 티어도 가로로 흐르지 않는다 --------------------------------- */
    await mkdir("output/playwright/coordinator-desk", { recursive: true });
    for (const [width, height] of [[1998, 1069], [1280, 800], [900, 900], [560, 900], [360, 800]]) {
      await page.setViewportSize({ width, height });
      const size = await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const surface = view.querySelector(".task-board-surface");
        const desk = view.querySelector(".task-board-desk");
        return { width: surface.clientWidth, scroll: surface.scrollWidth, desk: desk.scrollWidth, deskWidth: desk.clientWidth };
      });
      ok(`the desk has no horizontal overflow at ${width}px`,
        size.scroll <= size.width + 1 && size.desk <= size.deskWidth + 1, JSON.stringify(size));
      await page.screenshot({ path: `output/playwright/coordinator-desk/dark-${width}.png` });
    }
    ok("the coordinator desk raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testCoordinatorDesk(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
