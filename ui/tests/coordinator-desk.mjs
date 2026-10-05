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
export function coordinatorDeskFixture({ tasks = 60, workers = 5, mail = 20, folded = 0, gates = false, now = Date.now() } = {}) {
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
  /* The continue gate's reading of a worker, as the backend lays it on a row
   * (t-26583) — asked for by the measurements and by no suite that counts a
   * row's words: three in four calm, one in four repeating itself, and one of
   * those over its budget with the restore point it was saved under. */
  const gateOf = (n) => {
    const steps = 40 + n * 17;
    const calm = n % 4 !== 0;
    const over = n % 8 === 0;
    return {
      mode: "stop", verdict: over ? "stop" : calm ? "continue" : "pause",
      reasons: over ? [{ code: "task_budget_stop", value: 21.6, limit: 20 }]
        : calm ? [] : [{ code: "rework_loop", value: 333, limit: 300 }],
      metrics: {
        steps, since_checkpoint: steps % 200, checkpoints: Math.floor(steps / 200),
        rework_permille: calm ? 83 : 333, rise_permille: null, step_usd: 0.04,
        spent_usd: 3.41 + n, unpriced_calls: n % 3,
      },
      cost: "read", task_spent_usd: 3.41 + n, task_limit_usd: 20, day_spent_usd: 7.5 + n, day_limit_usd: 60,
      snapshot: { at_ms: now - 4 * minute, reference: `refs/zerocode/checkpoints/w-${n}/1`, error: null },
      at_ms: now,
    };
  };
  /* What the ledger kept of what a task's workers handed in, as `hand_in_keep` lays it on a row (t-32798):
   * one in four finished rows kept all of it, one kept part and left out a file over the cap and a file
   * named like a credential store, one could not write its keeping (the cleanup of its checkout is held),
   * and one handed in nothing by name. */
  const keptOf = (n) => {
    const base = { cap_bytes: 25_165_824, file_cap_bytes: 8_388_608, detail: null, at_ms: now, report: `a-${n}`, refused: 0 };
    switch (n % 4) {
      case 1: return { ...base, state: "kept", kept: 3, bytes: 23_552, left_out: 0, left_out_bytes: 0, masked: 4, reasons: [] };
      case 2: return { ...base, state: "partial", kept: 2, bytes: 2_048, left_out: 3, left_out_bytes: 9_000_000, masked: 0,
        refused: 2, reasons: ["over_file_cap", "secret_name"] };
      case 3: return { ...base, state: "failed", kept: 1, bytes: 10, left_out: 0, left_out_bytes: 0, masked: 0, report: null,
        reasons: ["copy_failed"], detail: "No space left on device (os error 28)" };
      default: return null;
    }
  };
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
      /* The first worker's report carried its own `merged: true`: the
       * ledger keeps that as the worker's claim (t-6815), never the fact. */
      review: n === 1
        ? { verified: false, merged: false, deployed: false, written: false,
          claimed_verified: true, claimed_merged: true, claimed_deployed: false, author: "worker" }
        : { verified: false, merged: false, deployed: false, written: false },
      term: seated ? term : null, at: now - (40 + n) * minute,
      model: models[(n - 1) % models.length], effort: n % 2 ? "max" : "xhigh", pane: `%${n}`,
      asking: word === "asking",
      wall: word === "walled"
        ? { wall: `m-wall-${n}`, observed_at_ms: now - 5 * minute, resets_at_ms: now + 45 * minute,
          stands_until_ms: now + 47 * minute }
        : null,
      quiet_at: word === "idle" ? now - 6 * minute : null,
      pane_missing_since_ms: word === "dead" ? now - 12 * minute : null,
      ...(gates ? { gate: gateOf(n) } : {}),
      ...(n === 2 ? { kept: keptOf(3) } : n === 3 ? { kept: keptOf(1) } : {}),
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

  /* The ledger's letters to the coordinator, oldest first, as `desk_letters`
   * hands them over (t-9456): the questions that wait on an answer are the
   * mail, the notices owed an acknowledgement the news — a quiet worker's
   * notices already one line per episode, saying how many it stands for — and
   * the three numbers are the backend's. Twenty rows over five kinds and the
   * three delivery states. */
  const kinds = ["question", "went_quiet", "quota_walled", "worker_died", "went_quiet", "question", "deadlocked"];
  const reasons = ["stalled", "quota_lifted", "pane_missing", "never_spoke"];
  const deskMail = [];
  const deskNews = [];
  for (let n = 1; n <= mail; n += 1) {
    const kind = kinds[(n - 1) % kinds.length];
    const worker = `w-${((n - 1) % Math.max(1, workers)) + 1}`;
    const delivery = n % 3 === 0 ? "delivered" : kind === "question" && n % 5 === 0 ? "acked" : "pending";
    (kind === "question" ? deskMail : deskNews).push({
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
      notices: kind === "went_quiet" ? 1 + (n % 4) : 1,
    });
  }
  /* The run's tasks by the pipeline's own stage words, sixty by default. */
  const stages = [
    ["pending", 6], ["ready", 20], ["dispatched", 5], ["reported", 6],
    ["merged", 18], ["gate", 2], ["blocked", 2], ["failed", 1], ["unreviewable", 0], ["nothing_to_land", 0],
    ["closed", 0],
  ];
  const total = stages.reduce((sum, [, count]) => sum + count, 0);
  /* What a finished task cost, as `task_cost` hands it over (t-9470): one in
   * four ran on an agent with no usage ledger, one in five is still open to
   * the clock. Moving tasks carry none. */
  const cost = (n) => ({
    attempts: 1 + (n % 3),
    wallMs: n % 5 === 0 ? null : (192 + n) * minute,
    generation: {
      sessionsKnown: 1 + (n % 2), sessionsLinked: 1 + (n % 2),
      inputTokens: 1_000 * n, outputTokens: 20_000, cacheReadTokens: 1_200_000, cacheWriteTokens: 40_000,
      usd: n % 4 === 0 ? null : 1.5 + n / 10, usdReason: n % 4 === 0 ? "unsupported_agent" : null,
    },
    jev: { requests: n % 6, stampedSeats: 4, unstampedSeats: 23, inputTokens: null },
  });
  /* What the writing lint counted in a finished task's summary, as `plain_text::lint` hands it over
   * (t-32786): a Korean one with notes, an English one with none, and one in three has none because its worker
   * wrote no summary. Moving tasks carry none. */
  const writing = (n) => (n % 3 === 0 ? null : n % 2 === 0
    ? { lang: "en", sentences: 7, avg_len: 9, longest: 11, limit: 25, long_sentences: 0, words: 0, patterns: 0,
      hits: [], cut: false }
    : { lang: "ko", sentences: 6, avg_len: 61, longest: 77, limit: 60, long_sentences: 4, words: 10, patterns: 13,
      hits: [{ kind: "word", find: "박자", plain: "주기", count: 2 },
        { kind: "pattern", find: "~하는 것이다", plain: "서술어로 바로 끝낸다 (~한다)", count: 2 }], cut: false });
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
        cost: stage === "reported" || stage === "merged" ? cost(at) : null,
        writing: stage === "reported" ? writing(at) : null,
        ...(stage === "reported" && keptOf(at) ? { kept: keptOf(at) } : {}),
      });
    }
  }
  const counts = stages.map(([stage]) => ({ stage, count: deskTasks.filter((one) => one.stage === stage).length }));
  window.__DESK__ = {
    revision: 1,
    runs: [{ run, name: "데스크 픽스처", seat: true }],
    mail: deskMail,
    news: deskNews,
    counts: { mail: deskMail.length, news: deskNews.length, folded },
    folded_batches: [],
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
      const newsHead = block?.querySelector(".board-desk-news-head");
      const folded = block?.querySelector(".board-desk-news-folded");
      return { shown: Boolean(block) && !block.hidden, head: block?.querySelector(".board-desk-head")?.textContent,
        letters, more: more && !more.hidden ? more.textContent : "",
        unseated: unseated && !unseated.hidden ? unseated.textContent : "",
        newsHead: newsHead && !newsHead.hidden ? newsHead.textContent : "",
        folded: folded && !folded.hidden ? folded.textContent : "",
        news: [...(block?.querySelectorAll(".board-desk-letters.is-news .board-desk-letter") ?? [])]
          .map((row) => row.dataset.letter),
        order: [...document.querySelectorAll("#board-view .task-board-desk > .board-desk-block:not([hidden])")].map((node) => node.dataset.deskBlock) };
    });
    const shut = await readMail();
    ok("the mail to answer is the questions alone (t-9456) — oldest first, five at a time, the news behind them, with how many more",
      shut.shown && shut.head === "답할 우편 · 6" && shut.letters.length === 5 && shut.more === "15통 더 보기" &&
      shut.letters.map((one) => one.key).join() === [901, 906, 908, 913, 915].map((n) => `run-desk/m-${n}`).join() &&
      shut.letters.every((one) => one.kind === "질문") && shut.newsHead === "" && shut.folded === "" &&
      shut.order.indexOf("mail") === shut.order.indexOf("machine") + 1, JSON.stringify(shut));
    await page.click('#board-view [data-desk-block="mail"] .board-desk-letters-more');
    await settleMail();
    const mail = await readMail();
    const letter = (id) => mail.letters.find((one) => one.key === `run-desk/${id}`);
    ok("opened, the news stands under its own head with the backend's number, and one quiet episode is one line",
      mail.head === "답할 우편 · 6" && mail.newsHead === "소식 · 14" && mail.letters.length === 20 &&
      mail.news.length === 14 && mail.more === "접기" && !mail.news.some((key) => letter(key.split("/")[1])?.kind === "질문"),
      JSON.stringify(mail));
    ok("each letter says its kind, whom it concerns, its age and where it stands in the coordinator's inbox",
      letter("m-901")?.kind === "질문" && letter("m-901").who === "w-1 · 데스크 과업 1" &&
      /^\d+(분|시간) 전$/.test(letter("m-901").age) && letter("m-901").body.startsWith("질문 1:") &&
      letter("m-901").delivery === "배달 전" && letter("m-901").deliveryTip === "배달 전 · 코디네이터가 아직 안 읽음" &&
      letter("m-902")?.kind === "조용해짐" && letter("m-902").body === "판이 보이지 않음 · 알림 3통" &&
      letter("m-905")?.kind === "조용해짐" && letter("m-905").body === "한도가 풀린 뒤에도 멈춰 있음 · 알림 2통" &&
      letter("m-903")?.kind === "한도 벽" && letter("m-903").body.startsWith("재설정까지") &&
      letter("m-903").delivery === "받음 d-990" && letter("m-903").deliveryTip === "받음 · 묶음 d-990" &&
      letter("m-904")?.kind === "워커 끝남" && letter("m-904").body === "보고 전에 판이 끝났어요",
      JSON.stringify(mail.letters));
    ok("a question is answered where it stands; a notice the coordinator holds acknowledges its whole batch, one it has not read offers nothing",
      letter("m-901").act === "답하기" && letter("m-903").act === "확인 · 이 묶음 6통" &&
      letter("m-902").act === "" && letter("m-904").act === "", JSON.stringify(mail.letters));
    const counted = await page.evaluate(async () => {
      const held = window.__DESK__;
      window.__DESK__ = { ...held, revision: held.revision + 1, counts: { ...held.counts, folded: 41 } };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const line = document.querySelector('#board-view [data-desk-block="mail"] .board-desk-news-folded');
      const said = line && !line.hidden ? line.textContent : "";
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1, counts: held.counts };
      refreshDeskLedger();
      return said;
    });
    await settleMail();
    ok("the notices no line stands for are the backend's count, drawn and never listed",
      counted === "접힌 소식 41통 · 하루 지났거나 끝난 침묵" && (await readMail()).folded === "", counted);

    /* 분류기 거절과 그 때문에 바뀐 모델(t-6747): 둘째 줄은 원장이 적은 사실 그대로 —
     * 분류, 거절 뒤에 무엇이 서 있는지, 어느 모델로 얼마 동안 바뀌었는지. */
    const declined = await page.evaluate(() => {
      const now = Date.now();
      return {
        kind: t(DESK_MAIL.classifier_declined.key, DESK_MAIL.classifier_declined.word),
        switchKind: t(DESK_MAIL.model_deviated.key, DESK_MAIL.model_deviated.word),
        stands: deskLetterDetail({ kind: "classifier_declined", category: "reasoning_extraction", routed: false, rung: "notify" }, now),
        handover: deskLetterDetail({ kind: "classifier_declined", category: "cyber", routed: true, rung: "handover" }, now),
        undeclared: deskLetterDetail({ kind: "classifier_declined", category: "cyber", routed: true, rung: "notify" }, now),
        unnamed: deskLetterDetail({ kind: "classifier_declined", category: null, routed: false, rung: "notify" }, now),
        switched: deskLetterDetail({ kind: "model_deviated", category: "cyber", switched_to: "claude-opus-4-8", scope: "session" }, now),
        local: deskLetterDetail({ kind: "model_deviated", category: "cyber", switched_to: "claude-opus-4-8", scope: "local" }, now),
      };
    });
    ok("a classifier's decline and the switch of model it caused say their category, what stands and for how long",
      declined.kind === "분류기 거절" && declined.switchKind === "모델 바뀜" &&
      declined.stands === "reasoning_extraction · 다른 모델로 가지 않는 분류라 거절이 그대로예요" &&
      declined.handover === "cyber · 선언된 워커에게 넘기는 중" &&
      declined.undeclared === "cyber · 넘길 워커를 선언하지 않았어요" &&
      declined.unnamed === "분류 없음 · 다른 모델로 가지 않는 분류라 거절이 그대로예요" &&
      declined.switched === "cyber → claude-opus-4-8 · 이 대화 끝까지" &&
      declined.local === "cyber → claude-opus-4-8 · 응답 하나만", JSON.stringify(declined));

    /* 쉬는 워커의 알림(t-15313): 둘째 줄은 그 워커가 무엇을 두고 쉬는지다 — 안 읽은
     * 편지, 또는 제 check --wait. 이유의 날것 낱말(unread_mail)은 사람에게 보이지
     * 않는다. */
    const resting = await page.evaluate(() => {
      const now = Date.now();
      return {
        unread: deskLetterDetail({ kind: "went_quiet", reason: "unread_mail", notices: 1 }, now),
        waiting: deskLetterDetail({ kind: "went_quiet", reason: "waiting_on_mail", notices: 1 }, now),
      };
    });
    ok("a worker at rest says what it rests on: unread mail, or its own wait",
      resting.unread === "안 읽은 편지를 둔 채 쉬고 있음" &&
      resting.waiting === "제 check --wait에서 편지를 기다리며 쉬고 있음", JSON.stringify(resting));

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
      held && held.note === "묶음을 확인함 · 원장에 적히면 목록에서 빠져요" && afterAck.head === "답할 우편 · 6",
      JSON.stringify({ acked, held }));
    const landed = await page.evaluate(async () => {
      const kept = (one) => one.id !== "m-901" && one.delivery_id !== "d-990";
      const mail = window.__DESK__.mail.filter(kept);
      const news = window.__DESK__.news.filter(kept);
      window.__DESK__ = { ...window.__DESK__, mail, news,
        counts: { ...window.__DESK__.counts, mail: mail.length, news: news.length } };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return [mail.length, news.length];
    });
    await settleMail();
    const cleared = await readMail();
    ok("once the ledger records the answer and the acknowledgement, those letters leave the desk",
      cleared.head === `답할 우편 · ${landed[0]}` && cleared.newsHead === `소식 · ${landed[1]}` &&
      !cleared.letters.some((one) => one.key === "run-desk/m-901" || one.key === "run-desk/m-903"),
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

    /* 접힌 소식의 묶음 확인 (t-9548): 코디네이터가 받아 둔 묶음의 소식이 전부 접혔으면 접힌 수
     * 옆에서 그 묶음을 통째로 확인한다 — 어느 묶음인지는 백엔드가 고르고, 자리 없는 창은 내밀지
     * 않는다. */
    const foldedAck = await page.evaluate(async () => {
      const held = window.__DESK__;
      const beat = async (desk) => {
        window.__DESK__ = { ...desk, revision: window.__DESK__.revision + 1 };
        refreshDeskLedger();
        await new Promise((done) => setTimeout(done, 0));
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const line = document.querySelector('#board-view [data-desk-block="mail"] .board-desk-news-folded');
        const act = line?.querySelector(".board-desk-news-folded-ack");
        return { line, act, shown: act && !act.hidden ? act.textContent : "" };
      };
      const folded = { ...held, counts: { ...held.counts, folded: 3 },
        folded_batches: [{ run: "run-desk", delivery_id: "d-777", batch: 3 }] };
      const offered = await beat(folded);
      const said = { word: offered.line?.firstElementChild?.textContent ?? "", act: offered.shown };
      offered.act?.click();
      await new Promise((done) => setTimeout(done, 0));
      said.sent = window.__DESK_SENT__.filter((one) => one.verb === "ack" && one.delivery === "d-777").length;
      window.__DESK_SENT__ = window.__DESK_SENT__.filter((one) => one.delivery !== "d-777");
      said.unseated = (await beat({ ...folded, runs: held.runs.map((one) => ({ ...one, seat: false })) })).shown;
      said.none = (await beat({ ...held, counts: { ...held.counts, folded: 3 } })).shown;
      await beat(held);
      return said;
    });
    ok("folded notices in a batch the coordinator holds offer that whole batch beside the count; none where it holds no seat or no batch (t-9548)",
      foldedAck.word === "접힌 소식 3통 · 하루 지났거나 끝난 침묵" && foldedAck.act === "확인 · 이 묶음 3통" &&
      foldedAck.sent === 1 && foldedAck.unseated === "" && foldedAck.none === "", JSON.stringify(foldedAck));
    await settleMail();
    // Folded again, as the rest of the suite found it.
    await page.click('#board-view [data-desk-block="mail"] .board-desk-letters-more');
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
    ok("each worker says its agent, model and effort, pane, checkout, commits ahead and changed files, and the ledger's review word — a worker's own claim in the claim's words",
      roster.rows["w-1"].facts === "Claude · claude-opus-5-5 · max · 판 201 · t-1 · 커밋 1개 앞섬 · 바뀐 파일 2" &&
      roster.rows["w-5"].facts.includes("%5") && roster.rows["w-5"].facts.includes("t-5") &&
      roster.rows["w-2"].task === "데스크 과업 2 · 검증 대기" &&
      roster.rows["w-1"].task === "데스크 과업 1 · 병합됐다 함" &&
      roster.rows["w-5"].disabled && !roster.rows["w-3"].disabled,
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

    /* ---- 검토 기록을 받을 수 없는 일 (t-19328): 워커 줄도 검증 대기라 하지 않는다 ---- */
    const restate = (change) => page.evaluate(async (changed) => {
      const w2 = window.__LEDGER__.find((row) => row.worker === "w-2");
      // The row as an older ledger sent it carries no such key at all.
      const { unreviewable, ...rest } = w2.review ?? {};
      w2.review = changed.unreviewable ? { ...rest, unreviewable: true } : rest;
      w2.failed = changed.failed === true;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return true;
    }, change);
    await restate({ unreviewable: true });
    await settleMail();
    const unrecorded = await readWorkers();
    await restate({ unreviewable: true, failed: true });
    await settleMail();
    const unrecordedAfterFailure = await readWorkers();
    await restate({});
    await settleMail();
    const older = await readWorkers();
    ok("a worker whose task was done by hand with nothing handed in reads 완료 — 검토 기록 없음, never 검증 대기 and never 실패; a row from an older ledger, without the fact, reads as it did",
      unrecorded.rows["w-2"].task === "데스크 과업 2 · 완료 — 검토 기록 없음" &&
      unrecordedAfterFailure.rows["w-2"].task === "데스크 과업 2 · 완료 — 검토 기록 없음" &&
      older.rows["w-2"].task === "데스크 과업 2 · 검증 대기", JSON.stringify({ unrecorded, unrecordedAfterFailure, older }));

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
        "pending:6,ready:20,dispatched:5,reported:6,merged:18,gate:2,blocked:2,failed:1,unreviewable:0,nothing_to_land:0,closed:0" &&
      pipeline.words.join() === "선행 대기,준비,진행,보고됨,병합,게이트,막힘,실패,완료 — 검토 기록 없음,완료 — 착지할 것 없음,닫힘",
      JSON.stringify(pipeline));
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

    /* ---- 닫힘 (t-19159): 실패가 아니고, 까닭을 낱말로, 열린 수에 들지 않는다 ---- */
    await page.evaluate(async () => {
      const closed = [
        { id: "t-901", closed: { kind: "folded", into: "t-902" } },
        { id: "t-903", closed: { kind: "handed-over", to: "run-9" } },
        { id: "t-904", closed: { kind: "outdated", why: "다른 과업이 덮음" } },
      ].map((one) => ({ run: window.__DESK__.runs[0].run, title: `닫힌 ${one.id}`, stage: "closed", gate: null,
        blocked_by: [], created_ms: 1, cost: null, ...one }));
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: [...window.__DESK__.tasks, ...closed],
        stages: window.__DESK__.stages.map((one) => one.stage === "closed" ? { ...one, count: 3 } : one) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleDesk();
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="closed"]');
    const closedStage = await readPipeline();
    ok("closed tasks stand in their own stage — not failed, not counted as open — and each says why in words",
      closedStage.head === "과업 흐름 · 84" && closedStage.chips.includes("closed:3:true:") &&
      closedStage.chips.includes("failed:1:false:is-halt") &&
      closedStage.rows.map((row) => row.note).join("|") === "접힘 → t-902|넘김 → run-9|낡음 · 다른 과업이 덮음",
      JSON.stringify(closedStage));

    /* ---- 완료 — 검토 기록 없음 (t-19328): 검증 대기(보고됨)가 아니고, 열린 수에 들지 않는다 ---- */
    await page.evaluate(async () => {
      const cost = { attempts: 2, wallMs: 90 * 60_000,
        generation: { sessionsKnown: 0, sessionsLinked: 0, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
          cacheWriteTokens: 0, usd: null, usdReason: "unlinked" },
        jev: { requests: 0, stampedSeats: 4, unstampedSeats: 23, inputTokens: null } };
      const unrecorded = [1, 2, 3].map((n) => ({ run: window.__DESK__.runs[0].run, id: `t-95${n}`,
        title: `손으로 끝낸 ${n}`, stage: "unreviewable", gate: null, blocked_by: [], closed: null,
        created_ms: 10 + n, cost }));
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: [...window.__DESK__.tasks, ...unrecorded],
        stages: window.__DESK__.stages.map((one) => one.stage === "unreviewable" ? { ...one, count: 3 } : one) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleDesk();
    const noRecordChip = await page.$('#board-view [data-desk-block="pipeline"] [data-stage="unreviewable"]');
    if (noRecordChip) await noRecordChip.click();
    const noRecordStage = await readPipeline();
    ok("work nothing can review stands in its own stage — 완료 — 검토 기록 없음, not 보고됨 (검증 대기), not counted as open, in no signal tone — and its rows say what the task cost",
      noRecordStage.head === "과업 흐름 · 84" && noRecordStage.chips.includes("unreviewable:3:true:") &&
      noRecordStage.chips.includes("reported:6:false:is-flow") && noRecordStage.chips.includes("closed:3:false:") &&
      noRecordStage.rows.length === 3 && noRecordStage.rows.every((row) => row.note === ""),
      JSON.stringify(noRecordStage));

    /* ---- 과업이 든 비용 (t-9470): 끝난 행마다 한 줄, 모르는 것은 「—」와 그 까닭 ---- */
    const readCosts = () => page.evaluate(() =>
      [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')].map((row) => {
        const line = row.querySelector(".board-desk-task-cost");
        return { id: row.querySelector(".board-desk-task-id").textContent,
          cost: line && !line.hidden ? line.textContent : "", tip: line?.dataset.tip ?? "" };
      }));
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="reported"]');
    await settleDesk();
    const costs = await readCosts();
    const shape = /^시도 \d · \d+시간 \d+분\(대기 포함\) · 생성 [\d.]+[kMB] 토큰 · \$\d+\.\d{2} · Jev 요청 \d$/;
    ok("a finished task's row says what it cost: attempts, the wall clock with its waits, the generation tokens at the API rate, the Jev requests",
      costs.length === 6 && costs.some((row) => shape.test(row.cost)) &&
      costs.every((row) => row.tip.includes("시도별 기록된 세션의 합") && row.tip.includes("API 환산가(구독 사용자는 청구액 아님)") &&
        row.tip.includes("스탬프 좌석 4개") && row.tip.includes("토큰 미기록")), JSON.stringify(costs));
    ok("what is not known is said as — with its reason, never as a guess",
      costs.some((row) => row.cost.includes("$— 사용량 원장 없는 에이전트")) && costs.some((row) => row.cost.includes("벽시계 —")) &&
      !costs.some((row) => row.cost.includes("NaN") || row.cost.includes("undefined")), JSON.stringify(costs));
    /* ---- 글 점검 (t-32786): 끝난 행마다, 일꾼이 쓴 요약을 센 수 한 줄 — 거절 없이 ---- */
    const readWriting = () => page.evaluate(() =>
      [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')].map((row) => {
        const line = row.querySelector(".board-desk-task-writing");
        return { id: row.querySelector(".board-desk-task-id").textContent,
          text: line && !line.hidden ? line.textContent : "", tip: line?.dataset.tip ?? "" };
      }));
    const writings = await readWriting();
    const noted = writings.find((row) => row.text.includes("직역투 13"));
    ok("a finished task whose worker wrote a summary says what the writing lint counted in it: the mean sentence, the long ones, the words to replace and the translationese — and a clean one says so",
      writings.length === 6 &&
      writings.filter((row) => row.text === "글 점검 · 문장 평균 61자 · 긴 문장 4 · 바꿀 말 10 · 직역투 13").length === 2 &&
      writings.filter((row) => row.text === "글 점검 · 문장 평균 9단어 · 걸린 것 없음").length === 2 &&
      writings.filter((row) => row.text === "").length === 2, JSON.stringify(writings));
    ok("the line's tip says how many sentences were read, what makes one long, that it only counts, and which rules were hit with what to write instead",
      Boolean(noted) && noted.tip.includes("6문장") && noted.tip.includes("60자를 넘으면") &&
      noted.tip.includes("거절하지 않") && noted.tip.includes("‘박자’ → 주기 ×2") &&
      noted.tip.includes("‘~하는 것이다’ → 서술어로 바로 끝낸다 (~한다) ×2"), JSON.stringify(noted));
    const writingMoved = await page.evaluate(async () => {
      const surface = document.querySelector("#board-view .task-board-surface");
      const records = [];
      const watch = new MutationObserver((batch) => records.push(...batch));
      watch.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
      const target = window.__DESK__.tasks.find((one) => one.stage === "reported" && one.writing?.lang === "ko");
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => one === target
          ? { ...one, writing: { ...one.writing, words: one.writing.words + 5 } } : one) };
      refreshDeskLedger();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      records.push(...watch.takeRecords());
      watch.disconnect();
      const where = (record) => (record.target.nodeType === 1 ? record.target : record.target.parentElement);
      return { id: target.id, records: records.map((record) =>
        `${record.type}:${where(record)?.className}:${where(record)?.closest(".board-desk-task")?.dataset.task ?? ""}`) };
    });
    ok("a count that moved rewrites that one line and nothing else",
      writingMoved.records.length > 0 &&
        writingMoved.records.every((one) => one.includes("board-desk-task-writing") && one.endsWith(`run-desk/${writingMoved.id}`)),
      JSON.stringify(writingMoved));
    const costMoved = await page.evaluate(async () => {
      const surface = document.querySelector("#board-view .task-board-surface");
      const records = [];
      const watch = new MutationObserver((batch) => records.push(...batch));
      watch.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
      const target = window.__DESK__.tasks.find((one) => one.stage === "reported");
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => one === target
          ? { ...one, cost: { ...one.cost, jev: { ...one.cost.jev, requests: one.cost.jev.requests + 5 } } } : one) };
      refreshDeskLedger();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      records.push(...watch.takeRecords());
      watch.disconnect();
      const where = (record) => (record.target.nodeType === 1 ? record.target : record.target.parentElement);
      return { id: target.id, records: records.map((record) =>
        `${record.type}:${where(record)?.className}:${where(record)?.closest(".board-desk-task")?.dataset.task ?? ""}`) };
    });
    ok("a cost that moved rewrites that one line and nothing else",
      costMoved.records.length > 0 &&
        costMoved.records.every((one) => one.includes("board-desk-task-cost") && one.endsWith(`run-desk/${costMoved.id}`)),
      JSON.stringify(costMoved));
    /* ---- 남긴 것 (t-32798): 끝난 행마다 한 줄 — 무엇을 남겼고, 무엇을 왜 못 담았는지, 정리가 막혔는지 ---- */
    const readKept = () => page.evaluate(() =>
      [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')].map((row) => {
        const line = row.querySelector(".board-desk-task-kept");
        const door = line?.querySelector(".board-desk-kept-door");
        return { id: row.querySelector(".board-desk-task-id").textContent, shown: Boolean(line) && !line.hidden,
          text: line && !line.hidden ? line.firstElementChild.textContent : "", state: line?.dataset.state ?? "",
          tip: line?.dataset.tip ?? "", door: line && !line.hidden && door && !door.hidden ? door.textContent : null,
          aria: door?.getAttribute("aria-label") ?? "" };
      }));
    const kepts = await readKept();
    const shownKepts = kepts.filter((row) => row.shown);
    ok("a finished task whose workers handed files in says what was kept: how many, how big, how many private values were masked",
      kepts.length === 6 && shownKepts.length === 4 &&
      shownKepts.filter((row) => row.text === "보관됨 · 3개 · 23.0 KB · 가린 값 4개" && row.state === "kept").length === 2,
      JSON.stringify(kepts));
    ok("a keeping that left files out says so on the row, and why — the cap with its number, the refusal by name — and still opens what it kept",
      shownKepts.some((row) => row.state === "partial" &&
        row.text === "일부만 보관 · 2개 보관 · 3개 못 담음 · 파일 하나가 8.0 MB 초과 · 비밀 파일 이름이라 보관 안 함" &&
        row.door === "열기"), JSON.stringify(shownKepts));
    ok("a keeping that failed says the cleanup is held, with its reason on the row and the system's own words in its tip",
      shownKepts.some((row) => row.state === "failed" && row.text === "보관 실패 · 정리가 막혀 있음 · 보관소에 쓰지 못함" &&
        row.tip.includes("No space left on device (os error 28)")), JSON.stringify(shownKepts));
    ok("a row with nothing handed in carries no kept line, and every line that opens something is a button with a name",
      kepts.filter((row) => !row.shown).every((row) => row.text === "" && row.door === null) &&
      shownKepts.every((row) => row.door === "열기" && row.aria === "보관한 보고서와 증거를 아티팩트에서 열기"),
      JSON.stringify(kepts));
    const opened = await page.evaluate(() => {
      window.__OPENED__ = [];
      const keep = window.openArtifacts;
      window.openArtifacts = (arg) => window.__OPENED__.push(arg);
      const row = [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')]
        .find((one) => one.querySelector(".board-desk-task-kept")?.dataset.state === "kept");
      const door = row?.querySelector(".board-desk-kept-door") ?? null;
      const focusable = Boolean(door) && door.tabIndex >= 0 && door.tagName === "BUTTON";
      door?.click();
      window.openArtifacts = keep;
      return { id: row?.querySelector(".board-desk-task-id")?.textContent ?? null,
        title: row?.querySelector(".board-desk-task-title")?.textContent ?? null, calls: window.__OPENED__, focusable };
    });
    ok("the door opens the Artifacts tab filtered to that task and selects the kept report, and is a button the keyboard reaches",
      opened.focusable && opened.calls.length === 1 && opened.calls[0]?.origin?.field === "task" &&
      opened.calls[0].origin.value === opened.id && opened.calls[0].origin.label === opened.title &&
      /^a-\d+$/.test(opened.calls[0].select ?? ""), JSON.stringify(opened));
    const keptMoved = await page.evaluate(async () => {
      const surface = document.querySelector("#board-view .task-board-surface");
      const records = [];
      const watch = new MutationObserver((batch) => records.push(...batch));
      watch.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
      const target = window.__DESK__.tasks.find((one) => one.kept?.state === "kept");
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => one === target
          ? { ...one, kept: { ...one.kept, kept: one.kept.kept + 1 } } : one) };
      refreshDeskLedger();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      records.push(...watch.takeRecords());
      watch.disconnect();
      const where = (record) => (record.target.nodeType === 1 ? record.target : record.target.parentElement);
      return { id: target.id, records: records.map((record) =>
        `${record.type}:${where(record)?.className}:${where(record)?.closest(".board-desk-task")?.dataset.task ?? ""}`) };
    });
    ok("a keeping that moved rewrites that one line and nothing else",
      keptMoved.records.length > 0 && keptMoved.records.every((one) => one.includes("board-desk-kept-text") && one.endsWith(`run-desk/${keptMoved.id}`)),
      JSON.stringify(keptMoved));
    const workerKepts = await page.evaluate(() =>
      Object.fromEntries([...document.querySelectorAll('#board-view [data-desk-block="workers"] .board-desk-worker')].map((row) => {
        const line = row.querySelector(".board-desk-worker-kept");
        return [row.dataset.worker, { shown: Boolean(line) && !line.hidden, state: line?.dataset.state ?? "",
          text: line && !line.hidden ? line.firstElementChild.textContent : "",
          inButton: Boolean(line?.closest("button.board-desk-worker-main")), door: Boolean(line?.querySelector("button")) }];
      })));
    ok("a worker row says a keeping that failed — its checkout's cleanup is held — and is quiet about one that was kept cleanly, which the task row says",
      workerKepts["run-desk/w-2"]?.state === "failed" &&
      workerKepts["run-desk/w-2"].text === "보관 실패 · 정리가 막혀 있음 · 보관소에 쓰지 못함" &&
      workerKepts["run-desk/w-3"]?.shown === false &&
      Object.entries(workerKepts).filter(([, row]) => row.shown).length === 1 &&
      Object.values(workerKepts).every((row) => !row.door), JSON.stringify(workerKepts));
    /* The rule on its own: every state a keeping can be in, painted on a worker's line — only the clean one is not said. */
    const workerStates = await page.evaluate(() => {
      if (typeof deskKeptLine !== "function" || typeof paintDeskKept !== "function") return [];
      const line = deskKeptLine("board-desk-worker-kept", false);
      const base = { cap_bytes: 25_165_824, file_cap_bytes: 8_388_608, detail: null, at_ms: 1, report: null, refused: 0,
        kept: 1, bytes: 10, left_out: 1, left_out_bytes: 10, masked: 0, reasons: [] };
      return ["keeping", "kept", "partial", "withheld", "failed"].map((state) => {
        paintDeskKept(line, { ...base, state }, null);
        return `${state}:${line.hidden ? "quiet" : "said"}`;
      });
    });
    ok("a worker's line stays quiet for a keeping that is whole and says every other state",
      workerStates.join() === "keeping:said,kept:quiet,partial:said,withheld:said,failed:said", JSON.stringify(workerStates));
    const keptWords = await page.evaluate(() => {
      const catalogs = ["en", "ja", "zh", "es"];
      /* The tables are page globals the new code declares; a page without them has no sentences, not an exception. */
      const states = typeof KEPT_STATES === "undefined" ? {} : KEPT_STATES;
      const reasonTable = typeof KEPT_REASONS === "undefined" ? {} : KEPT_REASONS;
      const entries = [...Object.values(states), ...Object.values(reasonTable),
        { key: "board.desk.kept.masked", word: "가린 값 {{count}}개" }, { key: "board.desk.kept.open", word: "열기" },
        { key: "board.desk.kept.openAria", word: "보관한 보고서와 증거를 아티팩트에서 열기" },
        { key: "board.desk.kept.tip", word: "체크아웃이 지워져도 남도록 아티팩트 저장소에 복사한 보고서와 증거입니다." }];
      const holes = [];
      for (const { key, word } of entries) {
        const marks = (text) => [...String(text).matchAll(/\{\{(\w+)\}\}/g)].map((hit) => hit[1]).sort().join();
        for (const code of catalogs) {
          const said = CATALOG[code][key];
          if (typeof said !== "string" || said === "") holes.push(`${code}:${key}`);
          else if (key !== "board.desk.kept.tip" && marks(said) !== marks(word)) holes.push(`${code}:${key}:{{}}`);
        }
      }
      return { count: entries.length, holes, reasons: Object.keys(reasonTable).sort().join() };
    });
    ok("every state and reason the backend can send has its sentence in the four catalogs beside the Korean one, with the same blanks",
      keptWords.holes.length === 0 && keptWords.count === 22 &&
      keptWords.reasons === "copy_failed,gone,link,not_kept,outside_roots,over_file_cap,over_total_cap,remote,secret_name,store_unavailable,too_many_files,unreadable,unsafe",
      JSON.stringify(keptWords));
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="ready"]');
    await settleDesk();
    const movingCosts = await readCosts();
    ok("a task still moving carries no cost line",
      movingCosts.length > 0 && movingCosts.every((row) => row.cost === ""), JSON.stringify(movingCosts));
    const movingWritings = await readWriting();
    ok("a task still moving carries no writing line either",
      movingWritings.length > 0 && movingWritings.every((row) => row.text === "" && row.tip === ""),
      JSON.stringify(movingWritings));
    const movingKepts = await readKept();
    ok("a task still moving carries no kept line either",
      movingKepts.length > 0 && movingKepts.every((row) => !row.shown && row.text === "" && row.door === null),
      JSON.stringify(movingKepts));
    await page.click('#board-view [data-desk-block="pipeline"] [data-stage="ready"]');
    await settleDesk();

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

    /* ---- 계속 판정 (t-26583): 워커 줄 밑의 판정 한 줄과 그 근거 ------------------
     *
     * 판정·사유·숫자는 백엔드의 것이고(`continue_gate`) 행의 `gate`로 온다; 여기서 재는 것은
     * 그 낱말과 서식이다. 판정을 받은 워커에만 줄이 서고, 「계속」이어도 읽은 숫자가 보이며,
     * 비용을 못 읽으면 0원이 아니라 까닭이 보이고, 조용한 폴은 아무것도 다시 쓰지 않는다. */
    const metricsOf = (extra = {}) => ({
      steps: 142, since_checkpoint: 142, checkpoints: 0, rework_permille: 83, rise_permille: null,
      step_usd: 0.04, spent_usd: 3.41, unpriced_calls: 0, ...extra,
    });
    const gateOf = (verdict, extra = {}) => ({
      mode: "notify", verdict, reasons: [], metrics: metricsOf(), cost: "read",
      task_spent_usd: 3.41, task_limit_usd: 20, day_spent_usd: 7.5, day_limit_usd: null,
      snapshot: null, at_ms: 1_800_000_000_000, ...extra,
    });
    const gates = {
      "w-1": gateOf("continue"),
      "w-3": gateOf("pause", {
        mode: "stop",
        reasons: [{ code: "rework_loop", value: 333, limit: 300 }],
        metrics: metricsOf({ steps: 61, since_checkpoint: 12, checkpoints: 1, rework_permille: 333 }),
        task_spent_usd: 1.2, task_limit_usd: null,
        snapshot: { at_ms: 1_800_000_000_000, reference: "refs/zerocode/checkpoints/w-3/1", error: null },
      }),
      "w-4": gateOf("continue", {
        cost: "no_reader", task_spent_usd: 0, task_limit_usd: null,
        metrics: metricsOf({ steps: 90, since_checkpoint: 90, rework_permille: null, spent_usd: null }),
      }),
      "w-5": gateOf("stop", {
        mode: "stop",
        reasons: [{ code: "task_budget_stop", value: 21.6, limit: 20 }],
        metrics: metricsOf({ steps: 310, since_checkpoint: 110, checkpoints: 1, rework_permille: null, spent_usd: 18.9 }),
        task_spent_usd: 18.9, day_spent_usd: 42, day_limit_usd: 60,
      }),
    };
    await page.evaluate(async (held) => {
      for (const [worker, gate] of Object.entries(held)) {
        window.__LEDGER__.find((row) => row.worker === worker).gate = gate;
      }
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    }, gates);
    await settleMail();
    const readGates = () => page.evaluate(() => Object.fromEntries(
      [...document.querySelectorAll('#board-view [data-desk-block="workers"] .board-desk-worker')].map((row) => {
        const line = row.querySelector(".board-desk-worker-gate");
        return [row.dataset.worker.split("/")[1], line ? {
          hidden: line.hidden,
          verdict: line.querySelector(".board-desk-worker-gate-verdict")?.textContent ?? null,
          why: line.querySelector(".board-desk-worker-gate-why")?.textContent ?? null,
          basis: line.querySelector(".board-desk-worker-gate-basis")?.textContent ?? null,
          state: line.dataset.verdict ?? null,
          tip: line.title,
        } : null];
      })));
    const judged = await readGates();
    ok("a worker the gate judged shows its verdict and the numbers it was read at, even when the verdict is to go on; a worker it did not judge shows no line",
      judged["w-1"]?.hidden === false && judged["w-1"].verdict === "계속" && judged["w-1"].state === "continue" &&
      judged["w-1"].why === "" &&
      judged["w-1"].basis === "호출 142번 · 되풀이 8% · 이 과업 $3.41 / 예산 $20.00" &&
      judged["w-2"]?.hidden === true, JSON.stringify(judged));
    ok("a pause names its reason in words with the number and the line it was held to, and the checkpoint the gate saved",
      judged["w-3"]?.verdict === "멈춤" && judged["w-3"].state === "pause" &&
      judged["w-3"].why === "되풀이·실패 33% (한도 30%)" &&
      judged["w-3"].basis === "호출 61번 · 되풀이 33% · 이 과업 $1.20 · 체크포인트 1회" &&
      judged["w-3"].tip.includes("refs/zerocode/checkpoints/w-3/1"), JSON.stringify(judged["w-3"]));
    ok("a stop names the budget it would have passed, with the day's budget beside the task's",
      judged["w-5"]?.verdict === "중지" && judged["w-5"].state === "stop" &&
      judged["w-5"].why === "과업 예산 $20.00 초과 예상 — $21.60" &&
      judged["w-5"].basis === "호출 310번 · 이 과업 $18.90 / 예산 $20.00 · 오늘 $42.00 / 예산 $60.00 · 체크포인트 1회",
      JSON.stringify(judged["w-5"]));
    ok("a cost the window cannot read says why and is never drawn as nothing spent",
      judged["w-4"]?.basis === "호출 90번 · 비용 모름 — 이 CLI의 기록 형식은 아직 못 읽음" &&
      !(judged["w-4"]?.basis ?? "").includes("$0"), JSON.stringify(judged["w-4"]));
    const sentences = await page.evaluate(() => typeof deskGateWords !== "function" ? [] : ["checkpoint_due", "cost_rising",
      "rework_loop", "task_budget_near", "day_budget_near", "task_budget_stop", "day_budget_stop"].map((code) => deskGateWords({
      verdict: "pause", mode: "notify", reasons: [{ code, value: 1500, limit: 2000 }], metrics: {}, cost: "read",
      task_spent_usd: 0, task_limit_usd: null, day_spent_usd: 0, day_limit_usd: null, snapshot: null,
    }, Date.now()).why));
    ok("every reason the backend can give has a sentence of its own, never the raw code",
      sentences.length === 7 && sentences.every((one) => typeof one === "string" && one !== "" && !/[a-z]+_[a-z]+/.test(one)),
      JSON.stringify(sentences));
    const gateQuiet = await deskMutations(page, async () => {
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    ok("a poll that brings the same judgements writes nothing", gateQuiet === 0, `mutations ${gateQuiet}`);
    const gateMoved = await deskMutations(page, async () => {
      window.__LEDGER__.find((row) => row.worker === "w-1").gate = {
        ...window.__LEDGER__.find((row) => row.worker === "w-1").gate,
        metrics: { ...window.__LEDGER__.find((row) => row.worker === "w-1").gate.metrics, steps: 143 },
      };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    const afterMove = await readGates();
    ok("a poll that brings one more call writes that worker's basis and nothing else",
      gateMoved > 0 && gateMoved <= 2 && afterMove["w-1"]?.basis?.startsWith("호출 143번"), `mutations ${gateMoved} ${afterMove["w-1"]?.basis}`);
    await page.evaluate(async () => {
      window.__LEDGER__.find((row) => row.worker === "w-1").gate = null;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleMail();
    const withdrawn = await readGates();
    ok("a gate that is switched off takes its line away", withdrawn["w-1"]?.hidden === true, JSON.stringify(withdrawn["w-1"]));
    await page.evaluate(async (held) => {
      window.__LEDGER__.find((row) => row.worker === "w-1").gate = held;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    }, gates["w-1"]);
    await settleMail();

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
