/* 코디네이터 데스크 (t-6588) — 작업 상황판이 코디네이터의 물음에 먼저 답하는가.
 *
 * docs/design/agent-board-round4.md. 하루 동안 코디네이터가 원장 CLI와 `df`·
 * `uptime`·레인의 `status.sh`·`simctl`로 물은 다섯(기계 띠·답할 우편·워커·과업
 * 흐름·빌드)을 보드 위의 데스크가 그리는지, 조용한 폴이 아무것도 다시
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
      progress: n === 1 ? { stage: "report_only", claimed: "merged", attempt: `dp-${n}` }
        : word === "idle" ? { stage: "report_only", attempt: `dp-${n}` } : { stage: "working", attempt: `dp-${n}` },
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
    ["pending", 6], ["ready", 20], ["dispatched", 5], ["reported", 6], ["verified", 0],
    ["merged", 18], ["deployed", 0], ["gate", 2], ["blocked", 2], ["stale", 0], ["failed", 1], ["unreviewable", 0],
    ["waived", 0], ["nothing_to_land", 0], ["unknown", 0], ["closed", 0],
  ];
  const head = (n) => n.toString(16).padStart(7, "0").repeat(6).slice(0, 40);
  const progressOf = (stage, n) => stage === "reported"
    ? { stage: "report_only", attempt: `dp-${100 + n}`, source: head(n), next: `zerocode-orc worktree-evidence --worker w-${100 + n}` }
    : stage === "merged" ? { stage: "merged", attempt: `dp-${100 + n}`, source: head(n), merge_head: head(n + 1) }
    : stage === "failed" ? { stage: "failed", attempt: `dp-${100 + n}`, next: `zerocode-orc worker-transcript --worker w-${100 + n}` }
    : stage === "dispatched" ? { stage: "working", attempt: `dp-${100 + n}` }
    : { stage: "unknown" };
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
        gate: stage === "gate" ? { id: `gate-${700 + at}`, question: `w-${at} 체크아웃을 수확할까요, 다시 보낼까요?`, kind: "asked",
          options: [], worker: null, checkout: null, examined: null } : null,
        blocked_by: stage === "blocked" ? [`t-${100 + at - 1}`] : [],
        created_ms: now - (tasks - at) * 3 * minute,
        cost: stage === "reported" || stage === "merged" ? cost(at) : null,
        progress: progressOf(stage, at),
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
  /* The lines each tree moved since its fork (t-70264, the worker table's 바뀐 줄): the third checkout's
   * git could not compare, so it carries none. */
  const linesMoved = [[1204, 0], [312, 41], null, [74, 12], [96, 58]];
  window.__ANSWER__.desk_checkouts = ({ paths } = {}) => (paths ?? []).map((path, index) => {
    const moved = linesMoved[index % linesMoved.length];
    return {
      path, base: "main", beyond_base: index + 1, dirty_files: index % 2 ? 0 : index + 2,
      lines: moved ? { added: moved[0], removed: moved[1] } : null,
    };
  });
  window.__DESK_SENT__ = [];
  window.__ANSWER__.desk_reply = (args) => (window.__DESK_SENT__.push({ verb: "reply", ...args }), { messageId: "m-reply" });
  // The hand the page says made the call (t-75209): the person's press on a
  // batch's 「확인」 is the person's.
  window.__ANSWER__.desk_ack = (args, options) => (window.__DESK_SENT__.push({
    verb: "ack", ...args, hand: options?.headers?.["zerocode-hand"] ?? null,
  }), { count: 0 });
  /* The project's builds as their writers recorded them (`board_builds`, zerocode_core::builds) — tools the
   * window knows nothing of: an app build of four steps on its second, each step beside what it usually takes
   * (the last never timed, and never worded), and an image build that passed. The answer carries a revision,
   * and a window holding it is told `unchanged`, as the backend does; the asks are kept to read which
   * checkout the window names. */
  const sha = "ccb6e1bda074f7fc8099f1d057aae2308d175564";
  const usual = (secs) => ({ secs, samples: 5 });
  window.__BUILDS__ = [{
    id: "app", name: { ko: "앱 빌드", en: "App build" }, label: "1.1.20", tool: "gradle", sha,
    started_ms: now - 6 * minute, running_since_ms: now - 2 * minute, updated_ms: now - minute, state: "running",
    steps: [
      { name: "compile", words: { ko: "컴파일", en: "Compile" }, state: "done", secs: 38, expected: usual(40) },
      { name: "test", words: { ko: "단위 시험", en: "Unit tests" }, state: "running", expected: usual(240) },
      { name: "integration", words: { ko: "통합 시험" }, state: "ahead", expected: usual(141) },
      { name: "package", state: "ahead", expected: { secs: 0, samples: 0 } },
    ],
  }, {
    id: "image", name: "도커 이미지", tool: "docker", sha, started_ms: now - 40 * minute, updated_ms: now - 37 * minute,
    state: "passed", steps: [{ name: "build", words: { ko: "이미지 빌드" }, state: "done", secs: 171, expected: usual(170) }],
  }];
  window.__BUILDS_ASKED__ = [];
  // What the project's order file says of itself and what its root suggests ride the same answer.
  window.__ANSWER__.board_builds = ({ worktree = null, known = null } = {}) => {
    window.__BUILDS_ASKED__.push({ worktree, known });
    const said = JSON.stringify({
      builds: window.__BUILDS__ ?? [], file: window.__BUILDS_FILE__ ?? { state: "none" }, suggest: window.__BUILDS_SUGGEST__ ?? [],
      runnable: window.__BUILDS_RUNNABLE__ ?? [], report_acts: window.__BUILDS_REPORT_ACTS__ ?? {},
      ...(window.__BUILDS_LINE__ ? { line: window.__BUILDS_LINE__ } : {}),
    });
    if (said !== window.__BUILDS_SAID__) {
      window.__BUILDS_SAID__ = said;
      window.__BUILDS_REVISION__ = (window.__BUILDS_REVISION__ ?? 0) + 1;
    }
    const revision = window.__BUILDS_REVISION__;
    return known === revision ? { revision, unchanged: true, builds: [] }
      : { revision, unchanged: false, ...JSON.parse(said) };
  };
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
    // The first frame the board stands in already carries the desk: what the
    // person sees first — on a window with a ledger run, the Now tab's first
    // block (t-67607) — is neither swapped for another nor moved down when the
    // desk's answers land.
    await page.evaluate(() => {
      window.__FIRST_SEEN__ = null;
      window.__FIRST_SEEN_OF__ = () => {
        const surface = document.querySelector("#board-view .task-board-surface");
        const first = surface && [...surface.querySelectorAll(".board-desk-block, .task-board-row")]
          .find((one) => one.checkVisibility());
        return first ? { key: first.dataset.deskBlock ?? first.dataset.taskKey, top: first.getBoundingClientRect().top } : null;
      };
      const watch = () => {
        window.__FIRST_SEEN__ = window.__FIRST_SEEN_OF__();
        if (!window.__FIRST_SEEN__) requestAnimationFrame(watch);
      };
      requestAnimationFrame(watch);
    });
    await page.evaluate(coordinatorDeskFixture);
    await page.waitForFunction(() => window.__FIRST_SEEN__ !== null);
    const firstFrame = await page.evaluate(async () => {
      await window.__BOARD_SETTLED__();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const first = window.__FIRST_SEEN__;
      const settled = window.__FIRST_SEEN_OF__();
      return { first, settled, moved: settled ? Math.round(settled.top - first.top) : null };
    });
    ok("the desk stands in the board's first frame: the first thing shown is the same and does not move when the desk's answers land",
      firstFrame.settled?.key === firstFrame.first.key && firstFrame.moved === 0, JSON.stringify(firstFrame));
    await page.evaluate(async () => {
      await askBoardBuilds();
      await paintBoardView();
    });

    /* The board's three tabs (t-67607): a block is drawn on its own tab, so each
     * part below chooses the tab it reads before it reads. */
    const tab = (id) => page.evaluate((id) => chooseBoardTab(document.querySelector("#board-view"), id, false), id);

    /* A person's press on a part the t-70264 tables draw — a real click where the part stands; on a board
     * without it nothing is pressed, so the assertions after it say what is missing one by one. */
    const press = async (selector) => {
      if (await page.$(selector)) await page.click(selector);
    };

    /* ---- 기계 띠: `df`·`uptime`·`simctl`의 자리, 빌린 기기는 상태 바의 문장 — 「워커」 탭 ---- */
    await tab("workers");
    const machine = await page.evaluate(async () => {
      paintEmulatorLoans({ count: 1, lastUsedMs: Date.now() });
      askMachineLoad();
      await new Promise((done) => setTimeout(done, 0));
      paintCoordinatorDesk(document.querySelector("#board-view"));
      const strip = document.querySelector('#board-view [data-desk-block="machine"]');
      const segment = (id) => strip?.querySelector(`[data-segment="${id}"]`);
      // A cell a fact (11-08 draft v3): its value, then the line under it — read as one sentence.
      const said = (node) => [...(node?.querySelectorAll(".board-desk-machine-value, .board-desk-machine-note") ?? [])]
        .filter((part) => !part.hidden && part.textContent).map((part) => part.textContent).join(" · ");
      const read = (id) => ({ text: said(segment(id)), tone: segment(id)?.className,
        meter: segment(id)?.querySelector(".board-desk-machine-meter:not([hidden]) > i")?.style.getPropertyValue("--fill") ?? "" });
      return { shown: Boolean(strip) && !strip.hidden, first: strip?.dataset.boardTabOf === "workers" && strip.checkVisibility(),
        disk: read("disk"), load: read("load"), ios: read("ios"), android: read("android"), loans: read("loans"),
        chip: document.querySelector("#sb-loans-words").textContent,
        order: [...(strip?.querySelectorAll("[data-segment]") ?? [])].map((node) => node.dataset.segment) };
    });
    ok("the machine strip answers df, uptime and simctl: disk with the ledger's worktree rule, load against the cores, booted devices",
      machine.shown && machine.first &&
      machine.disk.text === "디스크 21.0 GB 남음 · 워커 체크아웃 5개가 자라면 모자람" && machine.disk.tone.includes("is-wait") &&
      machine.load.text === "부하 55.9 · 코어 12 · 붐빔" && machine.load.tone.includes("is-wait") &&
      machine.ios.text === "iOS 시뮬레이터 2" && machine.android.text === "Android 에뮬레이터 0" &&
      machine.load.meter === "100%" && machine.ios.meter === "" &&
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
      const said = (node) => [...(node?.querySelectorAll(".board-desk-machine-value, .board-desk-machine-note") ?? [])]
        .filter((part) => !part.hidden && part.textContent).map((part) => part.textContent).join(" · ");
      const out = { disk: said(segment("disk")), tone: segment("disk").className, load: said(segment("load")),
        loadTone: segment("load").className, ios: Boolean(segment("ios")), android: said(segment("android")) };
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

    /* ---- 한눈 (t-67607): 「지금」 탭에서 지금 도는 일의 표가 첫 화면에 선다 ------------- */
    await tab("now");
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
      const running = view.querySelector('[data-desk-block="running"] .board-run').getBoundingClientRect();
      view.setAttribute("style", held);
      return { firstRunRowTop: Math.round(running.top), screen: window.innerHeight };
    });
    await page.setViewportSize({ width: 1280, height: 860 });
    ok("on a 1998×1069 board the Now tab leaves the first running row on the first screen",
      glance.firstRunRowTop < glance.screen, JSON.stringify(glance));

    /* ---- 답할 우편 (t-67607): 질문은 「지금」 탭의 당신 차례에, 소식은 「기록」 탭에 —
     * 답하기와 확인은 원장의 뜻 그대로 ------ */
    const settleMail = () => page.evaluate(async () => {
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    });
    await settleMail();
    /* One reader for both places a letter stands: the 당신 차례 card's questions and the
     * 소식 block's notices — each drawn on its own tab, so the reader reads the tab chosen. */
    const readMail = () => page.evaluate(() => {
      const asks = document.querySelector('#board-view [data-desk-block="asks"]');
      const block = document.querySelector('#board-view [data-desk-block="mail"]');
      const read = (row) => ({
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
      });
      const onNow = asks?.checkVisibility() ?? false;
      const shownText = (node) => (node && !node.hidden ? node.textContent : "");
      const more = block?.querySelector(".board-desk-letters-more");
      return {
        tab: document.querySelector("#board-view .task-board-surface").dataset.boardTab,
        title: document.querySelector("#board-view .board-now-title").textContent,
        asksHead: asks?.querySelector(".board-desk-head-word")?.textContent,
        asks: onNow ? [...asks.querySelectorAll(".board-asks > li")].map((row) => row.dataset.ask) : [],
        asksMore: onNow ? shownText(asks.querySelector(".board-asks-more")) : "",
        asksFoot: onNow ? asks.querySelector(".board-asks-foot").textContent : "",
        questions: onNow ? [...asks.querySelectorAll(".board-desk-letter")].map(read) : [],
        shown: Boolean(block) && !block.hidden && block.checkVisibility(),
        head: block?.querySelector(".board-desk-head")?.textContent,
        owed: shownText(block?.querySelector(".board-desk-news-owed")),
        letters: block?.checkVisibility() ? [...block.querySelectorAll(".board-desk-letter")].map(read) : [],
        more: more && !more.hidden ? more.textContent : "",
        unseated: shownText(block?.querySelector(".board-desk-unseated")),
        folded: shownText(block?.querySelector(".board-desk-news-folded")),
        news: [...(block?.querySelectorAll(".board-desk-letters.is-news .board-desk-letter") ?? [])].map((row) => row.dataset.letter),
      };
    });
    const shut = await readMail();
    ok("the questions to answer wait under 당신 차례 with the decisions and the reports, oldest first, four at a time",
      shut.asksHead === "당신 차례" && shut.asks.length === 4 && shut.asksMore === "10개 더 보기" &&
      shut.title === "답할 일 14개 — 나머지는 저절로 돕니다" &&
      shut.asksFoot === "이 밖에 당신을 기다리는 결정은 없습니다.", JSON.stringify(shut));
    await page.click('#board-view [data-desk-block="asks"] .board-asks-more');
    await settleMail();
    const asked = await readMail();
    ok("opened, 당신 차례 holds every question the ledger owes (t-9456) — the questions alone among the letters, oldest first",
      asked.asks.length === 14 && asked.asksMore === "접기" &&
      asked.questions.map((one) => one.key).join() === [901, 906, 908, 913, 915, 920].map((n) => `run-desk/m-${n}`).join() &&
      asked.questions.every((one) => one.kind === "질문"), JSON.stringify(asked));
    await page.click('#board-view [data-desk-block="asks"] .board-asks-more');
    await tab("history");
    await settleMail();
    const told = await readMail();
    ok("the news stands on the History tab under its own head with the backend's numbers, five at a time, pointing to the questions",
      told.shown && told.head === "소식 · 14" && told.letters.length === 5 && told.more === "9통 더 보기" &&
      told.owed === "답할 우편 6통은 「지금」의 당신 차례에 있습니다" && told.folded === "" &&
      !told.letters.some((one) => one.kind === "질문"), JSON.stringify(told));
    await page.click('#board-view [data-desk-block="mail"] .board-desk-letters-more');
    await settleMail();
    const mail = await readMail();
    await tab("now");
    await page.click('#board-view [data-desk-block="asks"] .board-asks-more');
    await settleMail();
    const questions = (await readMail()).questions;
    const letter = (id) => [...questions, ...mail.letters].find((one) => one.key === `run-desk/${id}`);
    ok("opened, the news is all fourteen, and one quiet episode is one line",
      mail.news.length === 14 && mail.more === "접기", JSON.stringify(mail));
    // A question under 당신 차례 carries the draft's line instead of its first one (t-70028): the task,
    // the asking worker's model, its age — the inbox's word stays on the row, out of sight.
    ok("each letter says its kind, whom it concerns, its age and where it stands in the coordinator's inbox",
      letter("m-901")?.kind === "질문" && letter("m-901").who.startsWith("데스크 과업 1 ") &&
      /· \d+(분|시간) 전$/.test(letter("m-901").who) &&
      /^\d+(분|시간) 전$/.test(letter("m-901").age) && letter("m-901").body.startsWith("질문 1:") &&
      letter("m-901").delivery === "배달 전" && letter("m-901").deliveryTip === "배달 전 · 코디네이터가 아직 안 읽음" &&
      letter("m-902")?.kind === "조용해짐" && letter("m-902").body === "판이 보이지 않음 · 알림 3통" &&
      letter("m-905")?.kind === "조용해짐" && letter("m-905").body === "한도가 풀린 뒤에도 멈춰 있음 · 알림 2통" &&
      letter("m-903")?.kind === "한도 도달" && letter("m-903").body.startsWith("재설정까지") &&
      letter("m-903").delivery === "받음 d-990" && letter("m-903").deliveryTip === "받음 · 묶음 d-990" &&
      letter("m-904")?.kind === "워커 끝남" && letter("m-904").body === "보고 전에 판이 끝났어요",
      JSON.stringify(mail.letters));
    ok("a question is answered where it stands; a notice the coordinator holds acknowledges its whole batch, one it has not read offers nothing",
      letter("m-901").act === "답하기" && letter("m-903").act === "확인 · 이 묶음 6통" &&
      letter("m-902").act === "" && letter("m-904").act === "", JSON.stringify(mail.letters));
    await tab("history");
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

    await tab("now");
    await settleMail();
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
    const replied = afterReply.questions.find((one) => one.key === "run-desk/m-901");
    ok("a reply goes to the ledger as the run's coordinator seat, and an uncertain one keeps its request name for the retry",
      failed.error.includes("refused: desk_reply") && sent.length === 1 &&
      sent[0].run === "run-desk" && sent[0].message === "m-901" && sent[0].body === "그대로 main에 올리세요." &&
      sent[0].retryRequest === failed.first && /^ui-desk-reply-/.test(sent[0].retryRequest),
      JSON.stringify({ failed, sent }));
    ok("the screen invents no answer: the letter stays until the ledger records it",
      replied && replied.note === "답을 보냄 · 원장에 적히면 목록에서 빠져요" && replied.act === "" && !replied.form,
      JSON.stringify(replied));
    await tab("history");
    await settleMail();
    await page.click('#board-view [data-letter="run-desk/m-903"] .board-desk-letter-act');
    await settleMail();
    const acked = await page.evaluate(() => window.__DESK_SENT__.filter((one) => one.verb === "ack"));
    const afterAck = await readMail();
    const held = afterAck.letters.find((one) => one.key === "run-desk/m-903");
    ok("the screen invents no acknowledgement: the batch is asked of the ledger, and the letter stays until the ledger says so",
      acked.length === 1 && acked[0].run === "run-desk" && acked[0].delivery === "d-990" &&
      held && held.note === "묶음을 확인함 · 원장에 적히면 목록에서 빠져요" && afterAck.head === "소식 · 14",
      JSON.stringify({ acked, held }));
    ok("the person's 「확인」 on a batch says person", acked.length === 1 && acked[0].hand === "person", JSON.stringify(acked));
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
    await tab("now");
    await settleMail();
    const clearedAsks = await readMail();
    ok("once the ledger records the answer and the acknowledgement, those letters leave the desk",
      cleared.head === `소식 · ${landed[1]}` && clearedAsks.title === `답할 일 ${landed[0] + 8}개 — 나머지는 저절로 돕니다` &&
      !cleared.letters.some((one) => one.key === "run-desk/m-903") &&
      !clearedAsks.questions.some((one) => one.key === "run-desk/m-901"),
      JSON.stringify({ cleared, clearedAsks }));
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
    await tab("history");

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
    await tab("workers");
    await page.evaluate(async () => {
      askDeskCheckouts();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleMail();
    const readWorkers = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="workers"]');
      const shown = (node) => Boolean(node) && node.checkVisibility();
      const said = (row, name) => row.querySelector(name)?.textContent ?? null;
      return {
        shown: Boolean(block) && !block.hidden,
        head: block?.querySelector(".board-desk-head")?.textContent,
        note: block?.querySelector(".board-desk-block-note")?.textContent ?? null,
        columns: [...(block?.querySelectorAll("thead th") ?? [])].map((cell) => cell.textContent),
        runs: [...(block?.querySelectorAll(".board-desk-worker-run") ?? [])].map((head) => `${head.dataset.run}|${head.textContent}`),
        order: [...(block?.children[0] ? block.querySelectorAll(".board-desk-worker") : [])].map((row) => row.dataset.worker),
        rows: Object.fromEntries([...(block?.querySelectorAll(".board-desk-worker") ?? [])].map((row) => [
          row.dataset.worker.split("/")[1], {
            health: row.querySelector(".board-desk-worker-health").textContent,
            dot: row.querySelector(".board-desk-worker-health").dataset.dot ?? null,
            healthTip: row.querySelector(".board-desk-worker-health").dataset.tip ?? "",
            cls: row.className.replace("board-desk-worker", "").trim(),
            // The details are built when a row opens (t-70264): a closed row's are not there to read.
            age: said(row, ".board-desk-worker-age"),
            task: row.querySelector(".board-desk-worker-task").textContent,
            facts: said(row, ".board-desk-worker-facts"),
            disabled: row.querySelector(".board-desk-worker-main")?.disabled ?? null,
            // The draft's seven cells, in the order the row draws them (11-08 draft v3).
            cells: [...(row.querySelector(".board-desk-worker-row")?.cells ?? [])].map((cell) => cell.textContent),
            work: said(row, ".board-desk-worker-work"),
            openPane: shown(row.querySelector(".board-desk-worker-open")) ? said(row, ".board-desk-worker-open") : "",
            model: said(row, ".board-desk-worker-model"), id: said(row, ".board-desk-worker-id"),
            verdict: shown(row.querySelector(".board-desk-worker-verdict")) ? said(row, ".board-desk-worker-verdict") : "",
            costTip: row.querySelector(".board-desk-worker-cost")?.dataset.tip ?? null,
            expanded: row.querySelector(".board-desk-worker-fold")?.getAttribute("aria-expanded") ?? null,
            details: shown(row.querySelector(".board-desk-worker-more")),
            heard: shown(row.querySelector(".board-desk-worker-heard")) ? said(row, ".board-desk-worker-heard") : "",
          }])),
      };
    });
    const roster = await readWorkers();
    ok("the roster answers worker-list with the unhealthy first: wall, question, asleep, in a turn, idle",
      roster.shown && roster.head === "워커 5" &&
      roster.order.join() === "run-desk/w-4,run-desk/w-3,run-desk/w-5,run-desk/w-1,run-desk/w-2", JSON.stringify(roster));
    /* ---- 시안 v3의 표 (11-08): 제목 밑에 상태별 수, 런마다 머리 행, 일곱 칸 — 워커 칸의 둘째 줄이 과업 ---- */
    ok("the Workers tab is the draft's table: the title with the count, the states counted under it, seven columns in the draft's order, a head row per run",
      roster.note === "한도 도달 1 · 답을 기다림 1 · 잠듦 1 · 일하는 중 1 · 쉬는 중 1" &&
      roster.columns.join("|") === "워커 · 과업|상태|지금 하는 일|경과|비용|바뀐 줄|할 일" &&
      roster.runs.length === 1 && roster.runs[0].startsWith("run-desk|") && roster.runs[0].endsWith(" · 워커 5"),
      JSON.stringify(roster));
    ok("a worker's row is the draft's seven cells: model and effort beside the worker with its task under them, state, what it does now, elapsed, cost, lines since the fork, the door to its pane — what nobody read is a dash, never a zero",
      roster.rows["w-1"]?.model === "claude-opus-5-5 · max" && roster.rows["w-1"].id === "워커 w-1" &&
      roster.rows["w-1"].task === "데스크 과업 1 · 병합됐다 함" && roster.rows["w-1"].cells.length === 7 &&
      roster.rows["w-1"].cells[1] === "일하는 중" && /^3\d분$/.test(roster.rows["w-1"].cells[3]) &&
      JSON.stringify(roster.rows["w-1"].cells.slice(4, 6)) === JSON.stringify(["—", "+1,204 −0"]) &&
      roster.rows["w-2"]?.cells[5] === "+312 −41" && roster.rows["w-3"]?.cells[5] === "—" &&
      roster.rows["w-1"].openPane === "판 열기" && roster.rows["w-5"]?.openPane === "", JSON.stringify(roster.rows));
    ok("what each worker does now is one line: a question waits in your turn, the wall says the Now tab's words, a resting worker says when it was last heard",
      roster.rows["w-3"]?.work === "답을 기다립니다 · 당신 차례에 있습니다" &&
      roster.rows["w-4"]?.work === "사용 한도에 닿았습니다 · 한도가 풀리면 이어 갑니다" &&
      /^마지막 활동 · \d+분 전$/.test(roster.rows["w-2"]?.work ?? ""), JSON.stringify(roster.rows));
    ok("each state wears the draft's dot beside its word: stopped red, waiting on you blue, in a turn amber, resting grey",
      roster.rows["w-4"]?.dot === "halt" && roster.rows["w-3"]?.dot === "you" && roster.rows["w-1"]?.dot === "run" &&
      roster.rows["w-5"]?.dot === "queue" && roster.rows["w-2"]?.dot === "queue", JSON.stringify(roster.rows));
    // The state cell stands at every width (the pane's steps back on a narrow board).
    await press('#board-view [data-worker="run-desk/w-1"] .board-desk-worker-row .board-desk-worker-state');
    await settleMail();
    const rowOpened = await readWorkers();
    await press('#board-view [data-worker="run-desk/w-1"] .board-desk-worker-fold');
    await settleMail();
    const rowFolded = await readWorkers();
    ok("pressing a row opens its details under it — the facts, the last word heard, the door to the task — and pressing it again folds them",
      roster.rows["w-1"].expanded === "false" && !roster.rows["w-1"].details &&
      rowOpened.rows["w-1"].expanded === "true" && rowOpened.rows["w-1"].details &&
      /^마지막 활동 \d+분 전$/.test(rowOpened.rows["w-1"].heard) && !rowOpened.rows["w-2"].details &&
      rowFolded.rows["w-1"].expanded === "false" && !rowFolded.rows["w-1"].details, JSON.stringify({ rowOpened, rowFolded }));
    /* Every row opened, as a person presses each closed one — the details are built when a row opens
     * (t-70264), and what they say is read from there. */
    const openAllWorkers = () => page.evaluate(() => {
      for (const fold of document.querySelectorAll('#board-view .board-desk-worker-fold[aria-expanded="false"]')) fold.click();
    });
    await openAllWorkers();
    await settleMail();
    const rosterOpen = await readWorkers();
    ok("each worker's health is one word from the ledger's facts, the wall with its reset",
      /^한도 도달 · 재설정까지 4\d분$/.test(rosterOpen.rows["w-4"].health) && rosterOpen.rows["w-4"].cls === "is-walled" &&
      rosterOpen.rows["w-3"].health === "답을 기다림" && rosterOpen.rows["w-5"].health === "잠듦" &&
      rosterOpen.rows["w-1"].health === "일하는 중" && rosterOpen.rows["w-2"].health === "쉬는 중" &&
      /^\d+분 전$/.test(rosterOpen.rows["w-1"].age ?? ""), JSON.stringify(rosterOpen.rows));
    ok("each worker says its agent, model and effort, pane, checkout, commits ahead and changed files, and the ledger's review word — a worker's own claim in the claim's words",
      rosterOpen.rows["w-1"].facts === "Claude · claude-opus-5-5 · max · 판 201 · t-1 · 커밋 1개 앞섬 · 바뀐 파일 2" &&
      (rosterOpen.rows["w-5"].facts ?? "").includes("%5") && (rosterOpen.rows["w-5"].facts ?? "").includes("t-5") &&
      rosterOpen.rows["w-2"].task === "데스크 과업 2 · 검증 대기" &&
      rosterOpen.rows["w-1"].task === "데스크 과업 1 · 병합됐다 함" &&
      rosterOpen.rows["w-5"].disabled === true && rosterOpen.rows["w-3"].disabled === false,
      JSON.stringify(rosterOpen.rows));
    // The door to the task stands in the row's details (t-70264): the row is open, then the door.
    await page.evaluate(() => {
      const fold = document.querySelector('#board-view [data-worker="run-desk/w-3"] .board-desk-worker-fold');
      if (fold?.getAttribute("aria-expanded") === "false") fold.click();
    });
    await settleMail();
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
      // A board without the tables (a red tree) has no opened rows to forget.
      deskChoice.workersOpen?.clear();
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
      turned && after.order[0] === "run-desk/w-2" && after.rows["w-2"].health === "창이 닫힘" &&
      after.rows["w-2"].work === "워커 화면이 사라졌습니다 · 코디가 다시 띄웁니다" &&
      after.rows["w-2"].cls === "is-gone" && after.rows["w-4"].health === "쉬는 중", JSON.stringify(after));

    /* ---- 끝난 워커 (t-94842): 회수를 기다리는 판은 「끝남」, 회수가 거절되면 그 까닭이 한 줄로 ---- */
    const finish = (held) => page.evaluate(async (reason) => {
      const w4 = window.__LEDGER__.find((row) => row.worker === "w-4");
      w4.ledger = "reclaimable";
      if (reason === null) delete w4.release_held;
      else w4.release_held = reason;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return true;
    }, held);
    await finish(null);
    await settleMail();
    const finished = await readWorkers();
    await finish("this pane has no session identity yet");
    await settleMail();
    const heldBack = await readWorkers();
    await page.evaluate(async () => {
      const w4 = window.__LEDGER__.find((row) => row.worker === "w-4");
      w4.ledger = "active";
      delete w4.release_held;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleMail();
    ok("a finished worker waiting for the window to reclaim its pane reads 끝남 with the done dot, never 쉬는 중 (t-94842)",
      finished.rows["w-4"].health === "끝남" && finished.rows["w-4"].dot === "done" &&
      finished.rows["w-4"].cls === "is-finished" && finished.rows["w-4"].healthTip === "", JSON.stringify(finished.rows["w-4"]));
    ok("a refused reclaim says 회수 보류 beside 끝남 and its reason in the tip (t-94842)",
      heldBack.rows["w-4"].health === "끝남 · 회수 보류" &&
      heldBack.rows["w-4"].healthTip === "창이 이 판을 닫지 못했습니다: this pane has no session identity yet",
      JSON.stringify(heldBack.rows["w-4"]));

    /* ---- 검토 기록을 받을 수 없는 일 (t-19328): 워커 줄도 검증 대기라 하지 않는다 ---- */
    const restate = (change) => page.evaluate(async (changed) => {
      const w2 = window.__LEDGER__.find((row) => row.worker === "w-2");
      const { unreviewable, superseded_by, author, attempt, source, ...rest } = w2.review ?? {};
      w2.review = changed.stage === "unreviewable" ? { ...rest, unreviewable: true }
        : changed.stage === "stale" ? { ...rest, author: "coordinator", attempt: "dp-old", source: "1111111", superseded_by: "dp-2" }
        : rest;
      w2.progress = changed.stage === "stale" ? { stage: "stale", attempt: "dp-2", reviewed: "1111111" }
        : changed.stage ? { stage: changed.stage, attempt: "dp-2" } : undefined;
      w2.failed = changed.failed === true;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return true;
    }, change);
    await restate({ stage: "unreviewable" });
    await settleMail();
    const unrecorded = await readWorkers();
    await restate({ stage: "unreviewable", failed: true });
    await settleMail();
    const unrecordedAfterFailure = await readWorkers();
    await restate({});
    await settleMail();
    const unread = await readWorkers();
    await restate({ stage: "report_only" });
    await settleMail();
    const reportedAgain = await readWorkers();
    ok("a worker whose task was done by hand with nothing handed in reads 종료 — 검토 자료 없음, never 검증 대기 and never 실패; a row the ledger sent no reading for says no stage at all",
      unrecorded.rows["w-2"].task === "데스크 과업 2 · 종료 — 검토 자료 없음" &&
      unrecordedAfterFailure.rows["w-2"].task === "데스크 과업 2 · 종료 — 검토 자료 없음" &&
      unread.rows["w-2"].task === "데스크 과업 2" && reportedAgain.rows["w-2"].task === "데스크 과업 2 · 검증 대기",
      JSON.stringify({ unrecorded, unrecordedAfterFailure, unread, reportedAgain }));

    /* ---- 옛 커밋을 가리키는 검토 (t-47538): 워커 줄이 그냥 검증 대기라 하지 않는다 ---- */
    const heldW2 = await page.evaluate(async () => {
      const w2 = window.__LEDGER__.find((row) => row.worker === "w-2");
      const held = { review: w2.review, progress: w2.progress };
      w2.review = { ...w2.review, author: "coordinator", attempt: "dp-old", source: "1111111", superseded_by: "dp-2" };
      w2.progress = { stage: "stale", attempt: "dp-2", reviewed: "1111111" };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      return held;
    });
    await settleMail();
    const staleWorker = await readWorkers();
    await page.evaluate(async (held) => {
      const w2 = window.__LEDGER__.find((row) => row.worker === "w-2");
      w2.review = held.review;
      w2.progress = held.progress;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    }, heldW2);
    await settleMail();
    ok("a worker whose review names an older attempt or commit than it handed in reads 다시 검토 대기 — never the plain 검증 대기 of a review nobody wrote (t-47538)",
      staleWorker.rows["w-2"].task === "데스크 과업 2 · 다시 검토 대기", JSON.stringify(staleWorker.rows["w-2"]));

    /* ---- 끝난 일 (11-08 시안 v3): 기록 탭의 첫 블록이 날마다 묶은 기록이다 ---------------------------
     * 착지한 과업·발행된 과업·착지할 것 없이 끝난 과업이 늦게 끝난 것부터 날 머리 밑에 서고, 오늘은 펼쳐져
     * 있고 지난날은 접혀 있다. 결과 거르개와 찾기 칸이 제목 밑에 선다. 행을 누르면 그 과업의 줄들이 펼쳐진다.
     * 누적 칸은 없어졌다 — 그 수는 과업 흐름의 칩이 말한다. */
    await page.evaluate(async () => {
      const held = window.__DESK__;
      window.__HELD_DESK__ = held;
      const now = Date.now();
      const midnight = new Date(now);
      midnight.setHours(0, 0, 0, 0);
      // Inside today whatever the clock says, and in this order even just after midnight.
      const today = (minutes) => Math.max(midnight.getTime() + (100 - minutes) * 1000, now - minutes * 60_000);
      const run = held.runs[0].run;
      const finished = (id, title, stage, times, extra = {}) => ({ run, id, title, stage, gate: null, blocked_by: [],
        closed: null, created_ms: 1, cost: null, progress: null, times, ...extra });
      const rows = [
        finished("t-981", "패널 정지 하나", "merged", { started_ms: today(52) - 50 * 60_000, landed_ms: today(52) }),
        finished("t-982", "작업 브라우저 혼자 쓰기", "deployed", { started_ms: today(40) - 130 * 60_000, landed_ms: today(40) }, {
          cost: held.tasks.find((one) => one.cost)?.cost ?? null,
          hand_in: { conditions: [{ number: 1, text: "빨강 시험이 초록이 됨", state: "checked" }], decisions: "", blocked: "", next: "" },
          kept: { state: "kept", kept: 3, bytes: 23_552, left_out: 0, left_out_bytes: 0, masked: 0, reasons: [],
            cap_bytes: 25_165_824, file_cap_bytes: 8_388_608, detail: null, at_ms: now, report: "a-982", refused: 0 },
        }),
        finished("t-983", "상황판 시안 1차", "nothing_to_land", { started_ms: today(10) - 35 * 60_000, verified_ms: today(10) }),
        finished("t-984", "어제 끝난 일", "merged",
          { started_ms: midnight.getTime() - 3 * 3_600_000, landed_ms: midnight.getTime() - 3_600_000 }),
      ];
      const added = (stage) => rows.filter((one) => one.stage === stage).length;
      window.__DESK__ = { ...held, revision: held.revision + 1, tasks: [...held.tasks, ...rows],
        stages: held.stages.map((one) => ({ ...one, count: one.count + added(one.stage) + (one.stage === "closed" ? 201 : 0) })) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await tab("history");
    await settleMail();
    const readToday = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="today"]');
      const shown = (node) => Boolean(node) && node.checkVisibility();
      const text = (node) => (shown(node) ? node.textContent : "");
      return {
        shown: shown(block), head: block?.querySelector(".board-desk-head")?.textContent ?? null,
        filter: [...(block?.querySelectorAll(".board-history-result") ?? [])].map((one) =>
          `${one.textContent}:${one.getAttribute("aria-pressed")}`),
        search: block?.querySelector(".board-history-search")?.placeholder ?? null,
        columns: [...(block?.querySelectorAll("thead th") ?? [])].map((cell) => cell.textContent),
        days: [...(block?.querySelectorAll(".board-today-day") ?? [])].map((one) =>
          `${one.querySelector(".board-today-day-name").textContent}|${one.querySelector(".board-today-day-count").textContent}|` +
          one.querySelector(".board-today-day-fold").getAttribute("aria-expanded")),
        rows: [...(block?.querySelectorAll(".board-today-task") ?? [])].map((row) => ({
          key: row.dataset.task,
          cells: [...row.querySelector(".board-today-row").cells].map((cell) => cell.textContent),
          dot: row.querySelector(".board-today-word")?.dataset.dot ?? null,
          expanded: row.querySelector(".board-today-fold")?.getAttribute("aria-expanded") ?? null,
          details: shown(row.querySelector(".board-today-more")),
          conditions: [...row.querySelectorAll(".board-desk-condition")].filter(shown).map((one) => one.textContent),
          cost: text(row.querySelector(".board-desk-task-cost")),
          door: text(row.querySelector(".board-desk-kept-door")),
        })),
        empty: text(block?.querySelector(".board-today-empty")),
        order: [...document.querySelectorAll("#board-view .task-board-desk > .board-desk-block")].filter(shown)
          .map((one) => one.dataset.deskBlock),
      };
    });
    const todayRead = await readToday();
    ok("the History tab opens on the record — 끝난 일, the result filter with its counts and the search under the title, four columns — before the news and the task flow; 누적 is gone",
      todayRead.shown && todayRead.head === "끝난 일" &&
      todayRead.filter.join() === "전체 4:true,발행됨 1:false,착지 2:false,착지할 것 없이 끝남 1:false" &&
      todayRead.search === "과업·id 찾기" &&
      todayRead.columns.join("|") === "과업|결과|걸린 시간|시각" &&
      todayRead.order.join() === "today,mail,pipeline", JSON.stringify(todayRead));
    const clock = /^\d{2}:\d{2}$/;
    const todayRow = (id) => todayRead.rows.find((one) => one.key === `run-desk/${id}`) ?? { cells: [] };
    ok("the record holds what finished — landed, shipped, or done with nothing to land — newest first under its day, each with its result and dot, how long it took and when; today stands open and yesterday waits folded",
      todayRead.days.join() === "오늘|3개|true,어제|1개|false" &&
      todayRead.rows.map((one) => one.key).join() === "run-desk/t-983,run-desk/t-982,run-desk/t-981" &&
      JSON.stringify(todayRow("t-983").cells.slice(0, 3)) === JSON.stringify(["상황판 시안 1차 t-983", "완료 — 착지할 것 없음", "35분"]) &&
      JSON.stringify(todayRow("t-982").cells.slice(0, 3)) === JSON.stringify(["작업 브라우저 혼자 쓰기 t-982", "발행됨", "2시간 10분"]) &&
      JSON.stringify(todayRow("t-981").cells.slice(0, 3)) === JSON.stringify(["패널 정지 하나 t-981", "착지 · 다음 발행에", "50분"]) &&
      todayRead.rows.every((one) => clock.test(one.cells[3])) &&
      todayRow("t-983").dot === "queue" && todayRow("t-982").dot === "done" && todayRow("t-981").dot === "done" &&
      todayRead.empty === "", JSON.stringify(todayRead.rows));
    await tab("now");
    await settleMail();
    const foot = await page.evaluate(() => document.querySelector("#board-view .board-runs-foot")?.textContent ?? null);
    await tab("history");
    await settleMail();
    // The count is this tab's (11-08): the Now tab's foot is the door to it.
    ok("the Now tab's foot is the door to those same rows", foot === "오늘 끝난 일 · 기록에서 보기", String(foot));
    await press('#board-view [data-task="run-desk/t-982"] .board-today-row .board-today-result');
    await settleMail();
    const todayOpened = (await readToday()).rows.find((one) => one.key === "run-desk/t-982");
    await press('#board-view [data-task="run-desk/t-982"] .board-today-fold');
    await settleMail();
    const todayFolded = (await readToday()).rows.find((one) => one.key === "run-desk/t-982");
    ok("pressing a finished row opens that task's lines under it — the pass conditions, what it cost, the kept report's door — and pressing again folds them",
      todayRow("t-982").expanded === "false" && !todayRow("t-982").details &&
      todayOpened?.expanded === "true" && todayOpened.details &&
      todayOpened.conditions.join() === "1. 빨강 시험이 초록이 됨확인됨" && todayOpened.cost.startsWith("시도 ") &&
      todayOpened.door === "열기" && todayFolded?.expanded === "false" && !todayFolded.details,
      JSON.stringify({ todayOpened, todayFolded }));
    // A past day opens on a press and folds again; today folds the same way.
    const yesterdayKey = await page.evaluate(() => deskDayKey(Date.now() - 86_400_000));
    const yesterdayFold = `#board-view [data-desk-block="today"] .board-today-day[data-day="${yesterdayKey}"] .board-today-day-fold`;
    await press(yesterdayFold);
    await settleMail();
    const yesterdayOpened = await readToday();
    await press(yesterdayFold);
    await settleMail();
    const yesterdayFolded = await readToday();
    ok("pressing a past day's head opens its rows under it, and pressing again folds them",
      yesterdayOpened.days.join() === "오늘|3개|true,어제|1개|true" &&
      yesterdayOpened.rows.map((one) => one.key).join() === "run-desk/t-983,run-desk/t-982,run-desk/t-981,run-desk/t-984" &&
      yesterdayFolded.days.join() === "오늘|3개|true,어제|1개|false" && yesterdayFolded.rows.length === 3,
      JSON.stringify({ yesterdayOpened, yesterdayFolded }));
    // The result filter keeps one kind; the search finds by title or id and opens the days it finds in.
    await press('#board-view [data-desk-block="today"] .board-history-result[data-result="merged"]');
    await settleMail();
    const landedOnly = await readToday();
    await press('#board-view [data-desk-block="today"] .board-history-result[data-result="all"]');
    await page.fill('#board-view [data-desk-block="today"] .board-history-search', "어제 끝난");
    await settleMail();
    const found = await readToday();
    await page.fill('#board-view [data-desk-block="today"] .board-history-search', "없는 과업");
    await settleMail();
    const foundNone = await readToday();
    await page.fill('#board-view [data-desk-block="today"] .board-history-search', "");
    await settleMail();
    ok("the filter keeps one result and says it is pressed; the search opens the day it finds a task in, and a search that finds nothing says so",
      landedOnly.filter.join() === "전체 4:false,발행됨 1:false,착지 2:true,착지할 것 없이 끝남 1:false" &&
      landedOnly.days.join() === "오늘|1개|true,어제|1개|false" &&
      landedOnly.rows.map((one) => one.key).join() === "run-desk/t-981" &&
      found.days.join() === "어제|1개|true" && found.rows.map((one) => one.key).join() === "run-desk/t-984" &&
      foundNone.days.length === 0 && foundNone.rows.length === 0 &&
      foundNone.empty === "거르개와 찾는 말에 맞는 일이 없습니다", JSON.stringify({ landedOnly, found, foundNone }));
    const historyWidth = page.viewportSize();
    const narrowHistory = {};
    for (const width of [560, 360]) {
      await page.setViewportSize({ width, height: 900 });
      narrowHistory[width] = await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const surface = view.querySelector(".task-board-surface");
        const desk = view.querySelector(".task-board-desk");
        // A card clips what runs past it, so the cards are asked too — a table cut at the edge is still a fault.
        const cards = [...view.querySelectorAll(".board-table-card, .board-desk-block")].filter((one) => one.checkVisibility());
        const out = { surface: surface.scrollWidth - surface.clientWidth, desk: desk.scrollWidth - desk.clientWidth,
          cards: Math.max(0, ...cards.map((one) => one.scrollWidth - one.clientWidth)),
          columns: [...view.querySelectorAll('[data-desk-block="today"] thead th')].filter((one) => one.checkVisibility()).length };
        Object.assign(view.style, { position: "", inset: "", width: "", height: "", zIndex: "" });
        return out;
      });
    }
    if (historyWidth) await page.setViewportSize(historyWidth);
    ok("at 560 and 360 px the History tab's table folds to the draft's narrow columns and nothing runs sideways",
      Object.values(narrowHistory).every((one) => one.surface <= 1 && one.desk <= 1 && one.cards <= 1 && one.columns === 3),
      JSON.stringify(narrowHistory));
    await page.evaluate(async () => {
      window.__DESK__ = { ...window.__HELD_DESK__, revision: window.__DESK__.revision + 1 };
      window.__HELD_DESK__ = null;
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    });
    await settleMail();
    const noneToday = await readToday();
    ok("a record with nothing finished says so in the card, under the same title, and draws no rows",
      noneToday.shown && noneToday.rows.length === 0 && noneToday.days.length === 0 &&
      noneToday.empty === "끝난 일이 아직 없습니다", JSON.stringify(noneToday));

    /* ---- 과업 흐름: `task-list`의 자리, 멈춰 선 단계가 먼저 펼쳐진다 ---------- */
    await tab("history");
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
        tones: [...(block?.querySelectorAll(".board-desk-stage") ?? [])].map((chip) => `${chip.dataset.stage}:${chip.dataset.tone}`),
        bar: [...(block?.querySelectorAll(".board-desk-flowbar-part") ?? [])].map((part) =>
          `${part.dataset.part}:${part.dataset.tone}:${part.style.getPropertyValue("--share")}`),
        rows: [...(block?.querySelectorAll(".board-desk-task") ?? [])].map((row) => ({
          id: row.querySelector(".board-desk-task-id").textContent,
          note: row.querySelector(".board-desk-task-note").hidden ? "" : row.querySelector(".board-desk-task-note").textContent })),
        more: block?.querySelector(".board-desk-more")?.hidden ? "" : block?.querySelector(".board-desk-more")?.textContent,
      };
    });
    const pipeline = await readPipeline();
    ok("the task flow counts every stage that holds a task, in the flow's order, stuck stages last — a stage at an exact zero draws no chip (11-08 draft v3)",
      pipeline.shown && pipeline.head === "과업 흐름 · 60" &&
      pipeline.chips.map((chip) => chip.split(":").slice(0, 2).join(":")).join() ===
        "pending:6,ready:20,dispatched:5,reported:6,merged:18,gate:2,blocked:2,failed:1" &&
      pipeline.words.join() === "선행 대기,준비,진행,보고됨,병합,게이트,막힘,실패",
      JSON.stringify(pipeline));
    ok("one bar over the chips shows where the tasks stand by share, in the board's status colours, and each chip wears its segment's colour as its legend",
      pipeline.bar.join() ===
        "pending:queue:6,ready:queue:20,dispatched:run:5,reported:you:6,merged:ok:18,gate:you:2,blocked:halt:2,failed:halt:1" &&
      pipeline.tones.join() === "pending:queue,ready:queue,dispatched:run,reported:you,merged:ok,gate:you,blocked:halt,failed:halt",
      JSON.stringify(pipeline));
    ok("a stuck stage with tasks wears its signal and opens first, naming the gate and its question",
      pipeline.chips.includes("gate:2:true:is-wait") && pipeline.chips.includes("failed:1:false:is-halt") &&
      pipeline.chips.includes("blocked:2:false:is-wait") && pipeline.chips.includes("ready:20:false:is-flow") &&
      // The gate is said in the person's words, as the Now tab says it (11-08 draft v3) — an asked one in its asker's.
      pipeline.rows.length === 2 && pipeline.rows.every((row) => /^w-\d+ 체크아웃을 수확할까요/.test(row.note)),
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
    ok("work nothing can review stands in its own stage — 종료 — 검토 자료 없음, not 보고됨 (검증 대기), not counted as open, in no signal tone — and its rows say what the task cost",
      noRecordStage.head === "과업 흐름 · 84" && noRecordStage.chips.includes("unreviewable:3:true:") &&
      noRecordStage.chips.includes("reported:6:false:is-flow") && noRecordStage.chips.includes("closed:3:false:") &&
      noRecordStage.rows.length === 3 && noRecordStage.rows.every((row) => row.note === ""),
      JSON.stringify(noRecordStage));

    /* ---- 어디까지 왔는가 (t-47538): 검증됨·배포됨·다시 검토 대기·상태 모름이 제 단계에 서고, 행이 커밋·담당·막힘·다음을 말한다 ---- */
    const sha = { reviewed: "abcdef1234567890abcdef1234567890abcdef12", now: "fedcba9876543210fedcba9876543210fedcba98",
      landed: "0123456789abcdef0123456789abcdef01234567" };
    await page.evaluate(async (heads) => {
      const run = window.__DESK__.runs[0].run;
      const w4 = window.__LEDGER__.find((one) => one.worker === "w-4");
      w4.wall = { ...w4.wall, stands_until_ms: Date.now() + 47 * 60_000 };
      const rows = [
        { id: "t-961", stage: "verified", next_step: "land", progress: { stage: "verified", attempt: "dp-961", source: heads.reviewed,
          next: `zerocode-orc land-check --task t-961 --head ${heads.reviewed} --retry-request land-check-t-961-abcdef123456` } },
        { id: "t-962", stage: "deployed", next_step: "over", progress: { stage: "deployed", attempt: "dp-962", source: heads.reviewed, merge_head: heads.landed } },
        { id: "t-963", stage: "stale", next_step: "review_new_head", progress: { stage: "stale", attempt: "dp-963", source: heads.now, reviewed: heads.reviewed,
          next: "zerocode-orc worktree-evidence --worker w-963" } },
        { id: "t-964", stage: "unknown", next_step: "unknown", progress: { stage: "unknown" } },
        { id: "t-4", stage: "dispatched", next_step: "wait", progress: { stage: "working", attempt: "dp-4" } },
        { id: "t-3", stage: "dispatched", next_step: "wait", progress: { stage: "working", attempt: "dp-3" } },
        { id: "t-2", stage: "failed", next_step: "retry_or_close", progress: { stage: "failed", attempt: "dp-2", next: "zerocode-orc worker-transcript --worker w-2" } },
        { id: "t-970", stage: "failed", next_step: "retry_or_close", progress: { stage: "failed", attempt: "dp-970" } },
      ].map((one) => ({ run, title: `단계 ${one.id}`, gate: null, blocked_by: [], closed: null, created_ms: 1, cost: null, ...one }));
      const added = (stage) => rows.filter((one) => one.stage === stage).length;
      const known = new Set(window.__DESK__.stages.map((one) => one.stage));
      const fresh = [...new Set(rows.map((one) => one.stage))].filter((stage) => !known.has(stage))
        .map((stage) => ({ stage, count: 0 }));
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: [...window.__DESK__.tasks, ...rows],
        stages: [...window.__DESK__.stages, ...fresh].map((one) => ({ ...one, count: one.count + added(one.stage) })) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    }, sha);
    await settleDesk();
    const readStage = async (stage) => {
      const chip = await page.$(`#board-view [data-desk-block="pipeline"] [data-stage="${stage}"]`);
      if (chip && (await chip.getAttribute("aria-pressed")) !== "true") await chip.click();
      await settleDesk();
      return page.evaluate(() => [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')]
        .map((row) => {
          const line = (name) => {
            const node = row.querySelector(name);
            return node && !node.hidden ? node.textContent : "";
          };
          const copy = row.querySelector(".board-desk-task-next-copy");
          return { id: line(".board-desk-task-id"), work: line(".board-desk-task-work"), owner: line(".board-desk-task-owner"),
            next: line(".board-desk-task-next-command"), label: line(".board-desk-task-next-label"),
            step: line(".board-desk-task-step"),
            copy: copy && !copy.hidden ? copy.textContent : "" };
        }));
    };
    const flow = await readPipeline();
    const verifiedRows = await readStage("verified");
    const deployedRows = await readStage("deployed");
    const staleRows = await readStage("stale");
    const unknownRows = await readStage("unknown");
    const carriedRows = await readStage("dispatched");
    const failedRows = await readStage("failed");
    const row = (rows, id) => rows.find((one) => one.id === id) ?? {};
    ok("verified, deployed, a review that names an older commit and work the ledger cannot read stand in stages of their own, in the flow's order, and only the last is left out of the count",
      flow.chips.includes("verified:1:false:is-flow") && flow.chips.includes("deployed:1:false:is-flow") &&
      flow.chips.includes("stale:1:false:is-wait") && flow.chips.includes("unknown:1:false:") &&
      flow.words.join().includes("보고됨,검증됨,병합,배포됨") && flow.head === "과업 흐름 · 91",
      JSON.stringify(flow));
    ok("a verified row names the commit the review read and the landing check to run next; a deployed one names the commit it shipped and the one it landed as",
      row(verifiedRows, "t-961").work === "검토한 커밋 abcdef123" &&
      row(verifiedRows, "t-961").next === `zerocode-orc land-check --task t-961 --head ${sha.reviewed} --retry-request land-check-t-961-abcdef123456` &&
      row(verifiedRows, "t-961").copy === "명령 복사" && row(verifiedRows, "t-961").label === "다음" &&
      row(deployedRows, "t-962").work === "검토한 커밋 abcdef123 · 병합 커밋 012345678" && row(deployedRows, "t-962").next === "",
      JSON.stringify({ verifiedRows, deployedRows }));
    ok("a stale row names both commits — the one the review read and the one handed in now — and what to read next; a row the ledger cannot read says no commit and offers nothing",
      row(staleRows, "t-963").work === "검토한 커밋 abcdef123 · 지금 커밋 fedcba987" &&
      row(staleRows, "t-963").next === "zerocode-orc worktree-evidence --worker w-963" &&
      row(unknownRows, "t-964").work === "" && row(unknownRows, "t-964").next === "" && row(unknownRows, "t-964").copy === "",
      JSON.stringify({ staleRows, unknownRows }));
    ok("a carried task names who carries it and the cause the ledger witnessed — a quota wall, a pane proven gone; one whose worker asks points at the mail to answer instead of a command; a failed one with no witness says its cause needs checking",
      row(carriedRows, "t-4").owner.startsWith("담당 w-4 · ") && row(carriedRows, "t-4").owner.endsWith("막힘: 한도 도달") &&
      row(carriedRows, "t-3").label === "「지금」의 당신 차례에서 질문에 답하세요" && row(carriedRows, "t-3").next === "" &&
      row(carriedRows, "t-3").copy === "" &&
      row(failedRows, "t-2").owner.startsWith("담당 w-2 · ") && row(failedRows, "t-2").owner.endsWith("막힘: 창이 닫힘") &&
      row(failedRows, "t-2").next === "zerocode-orc worker-transcript --worker w-2" &&
      row(failedRows, "t-970").owner === "막힘: 원인 확인 필요" && row(failedRows, "t-970").next === "",
      JSON.stringify({ carriedRows, failedRows }));
    ok("every task row says its next step in one line of words, from the word the backend laid on it — a stage's step is never a colour and never a table kept in the window",
      row(verifiedRows, "t-961").step === "다음 조치: 착지" && row(deployedRows, "t-962").step === "다음 조치: 할 일 없음" &&
      row(staleRows, "t-963").step === "다음 조치: 새 커밋 검증" && row(unknownRows, "t-964").step === "다음 조치: 확인 불가" &&
      row(carriedRows, "t-4").step === "다음 조치: 워커 작업 기다리기" &&
      row(failedRows, "t-2").step === "다음 조치: 다시 맡기기 또는 닫기",
      JSON.stringify({ verifiedRows, deployedRows, staleRows, unknownRows, carriedRows, failedRows }));
    const stepWords = await page.evaluate(() => {
      const words = ["await_dependencies", "summon", "wait", "decide", "mend_dependency", "review", "retry_or_close",
        "restart_or_close", "review_new_head", "land", "ship", "over", "unknown"];
      const out = {};
      for (const locale of ["ko", "en", "ja", "zh", "es"]) {
        setLocale(locale, { persist: false });
        out[locale] = words.map((word) => deskNextStepWords({ next_step: word }));
      }
      setLocale("ko", { persist: false });
      return { words, out, unmapped: deskNextStepWords({ next_step: "from_a_newer_window" }) };
    });
    const hangul = /[\u3131-\u318e\uac00-\ud7a3]/u;
    ok("all thirteen next steps are worded in the five languages: none empty, none a bare catalog key, each language its own (no Korean in en or es), and a word from a newer window says nothing instead of guessing",
      stepWords.words.every((word, at) => ["ko", "en", "ja", "zh", "es"].every((locale) => {
        const said = stepWords.out[locale][at];
        return said !== "" && !said.includes("board.desk") && !said.includes("{{") && !said.includes("_") && (stepWords.out.ko[at] !== said || locale === "ko");
      })) && stepWords.out.en.every((said) => !hangul.test(said)) && stepWords.out.es.every((said) => !hangul.test(said)) &&
      stepWords.unmapped === "",
      JSON.stringify(stepWords));
    const costShapes = await page.evaluate(() => {
      const out = {};
      const shapes = {
        direct: { attempts: 0, shape: { kind: "direct" }, generation: { usd: null, usdReason: "coordinator_conversation", sessionsLinked: 0, sessionsKnown: 0 }, jev: { requests: 2 } },
        delegated: { attempts: 3, wallMs: 60_000, shape: { kind: "delegated", workers: 2, overlapMs: 120_000 }, generation: { usd: 1.5, sessionsLinked: 2, sessionsKnown: 2, inputTokens: 10 }, jev: { requests: 0 } },
        unknown: { attempts: 0, shape: { kind: "unknown" }, jev: { requests: 0 } },
      };
      for (const locale of ["ko", "en", "ja", "zh", "es"]) {
        setLocale(locale, { persist: false });
        out[locale] = Object.fromEntries(Object.entries(shapes).map(([name, cost]) => [name, taskCostWords(cost)]));
      }
      setLocale("ko", { persist: false });
      return out;
    });
    ok("a task the coordinator did itself says 직접(코디) — 비용 미확인 and never a zero; a delegated one says 위임 N명 and the overlap; an unreadable one says 미확인 — in the five languages",
      costShapes.ko.direct.text.startsWith("직접(코디) — 비용 미확인") && !/\$\s?0|0원|\$—/u.test(costShapes.ko.direct.text) &&
      costShapes.ko.direct.tip.includes("코디 대화는 여러 과업에 걸쳐") &&
      costShapes.ko.delegated.text.startsWith("위임 2명 · ") && costShapes.ko.delegated.tip.includes("겹친 시간 2분") &&
      costShapes.ko.unknown.text.startsWith("미확인") &&
      ["en", "ja", "zh", "es"].every((locale) => ["direct", "delegated", "unknown"].every((name) => {
        const said = costShapes[locale][name].text;
        return said !== costShapes.ko[name].text && !said.includes("board.desk") && !said.includes("{{");
      })) && !hangul.test(costShapes.en.direct.text + costShapes.en.delegated.text + costShapes.en.unknown.text +
        costShapes.es.direct.text + costShapes.es.delegated.text + costShapes.es.unknown.text),
      JSON.stringify(costShapes));
    await readStage("verified");
    const stays = await page.evaluate(async () => {
      const find = () => [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')]
        .find((one) => one.querySelector(".board-desk-task-id")?.textContent === "t-961");
      const before = find();
      const copy = before.querySelector(".board-desk-task-next-copy");
      copy.focus();
      const first = before.querySelector(".board-desk-task-step").textContent;
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => (one.id === "t-961" ? { ...one, next_step: "ship" } : one)) };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const after = find();
      return { sameNode: before === after, focused: document.activeElement === copy, first,
        then: after?.querySelector(".board-desk-task-step").textContent };
    });
    ok("when a task's step changes the row keeps its node and the keyboard focus on its copy button — only the words are written",
      stays.sameNode && stays.focused && stays.first === "다음 조치: 착지" && stays.then === "다음 조치: 발행", JSON.stringify(stays));
    await readStage("verified");
    const copied = await page.evaluate(async () => {
      window.__CLIPBOARD_WRITES__ = [];
      const button = [...document.querySelectorAll('#board-view [data-desk-block="pipeline"] .board-desk-task')]
        .find((one) => one.querySelector(".board-desk-task-id")?.textContent === "t-961")
        ?.querySelector(".board-desk-task-next-copy") ?? null;
      button?.focus();
      return { focused: button !== null && document.activeElement === button, type: button?.type ?? null,
        name: button?.textContent ?? null, sent: window.__DESK_SENT__.length };
    });
    if (copied.focused) await page.keyboard.press("Enter");
    const afterCopy = await page.evaluate(async () => {
      for (let tries = 0; tries < 40 && window.__CLIPBOARD_WRITES__.length === 0; tries += 1) {
        await new Promise((done) => setTimeout(done, 10));
      }
      return { writes: [...window.__CLIPBOARD_WRITES__], said: document.activeElement?.textContent ?? "",
        sent: window.__DESK_SENT__.length };
    });
    ok("the next command's copy is a button the keyboard reaches: Enter copies the command, word for word, and the button says it did — nothing runs",
      copied.focused && copied.type === "button" && copied.name === "명령 복사" &&
      afterCopy.writes.length === 1 && afterCopy.writes[0] === `zerocode-orc land-check --task t-961 --head ${sha.reviewed} --retry-request land-check-t-961-abcdef123456` &&
      afterCopy.said === "명령을 복사했습니다" && afterCopy.sent === copied.sent,
      JSON.stringify({ copied, afterCopy }));
    await readStage("stale");
    const viewport = page.viewportSize();
    const fit = {};
    for (const width of [320, 360, 560]) {
      await page.setViewportSize({ width, height: 900 });
      for (const theme of ["dark", "light"]) {
        for (const locale of ["ko", "en", "ja", "zh", "es"]) {
          fit[`${width}-${theme}-${locale}`] = await page.evaluate(async ({ theme, locale }) => {
            document.documentElement.dataset.theme = theme;
            setLocale(locale, { persist: false });
            const view = document.querySelector("#board-view");
            Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
            paintCoordinatorDesk(view);
            await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
            const desk = view.querySelector(".task-board-desk");
            const strip = view.querySelector(".board-desk-stages");
            const task = [...view.querySelectorAll(".board-desk-task")].find((one) => one.querySelector(".board-desk-task-id").textContent === "t-963");
            const box = task?.getBoundingClientRect();
            const outside = [".board-desk-task-work", ".board-desk-task-step", ".board-desk-task-next-command", ".board-desk-task-next-copy"].filter((name) => {
              const part = task?.querySelector(name)?.getBoundingClientRect();
              return !part || part.right > box.right + 0.5 || part.left < box.left - 0.5;
            });
            const chips = [...view.querySelectorAll(".board-desk-stage")].filter((chip) => chip.scrollWidth > chip.clientWidth + 1).length;
            return { desk: desk.scrollWidth - desk.clientWidth, strip: strip.scrollWidth - strip.clientWidth, outside, chips,
              stale: view.querySelector('[data-stage="stale"] .board-desk-stage-word')?.textContent ?? "" };
          }, { theme, locale });
        }
      }
    }
    await page.evaluate(() => {
      document.documentElement.dataset.theme = "dark";
      setLocale("ko", { persist: false });
      Object.assign(document.querySelector("#board-view").style, { position: "", inset: "", width: "", height: "", zIndex: "" });
    });
    if (viewport) await page.setViewportSize(viewport);
    const staleWords = new Set(Object.entries(fit).filter(([key]) => key.startsWith("360-dark-")).map(([, one]) => one.stale));
    ok("at 320, 360 and 560 px, in both treatments and the five languages, the new stages and the row's commits, next step, command and copy stay inside the desk — nothing overflows and no chip is cut — and each language says the stage in its own word",
      Object.values(fit).every((one) => one.desk <= 1 && one.strip <= 1 && one.outside.length === 0 && one.chips === 0) &&
      staleWords.size === 5, JSON.stringify(fit));

    /* ---- 상태 모름은 증거 상태 (t-47538): 정확히 센 0이면 칩이 없고, 셈이 없으면 「—」, 과업이 있으면 이름과 수 ---- */
    const setUnknown = (entry) => page.evaluate(async (entry) => {
      const rest = window.__DESK__.stages.filter((one) => one.stage !== "unknown");
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        stages: entry === "absent" ? rest : [...rest, { stage: "unknown", count: entry }] };
      refreshDeskLedger();
      await new Promise((done) => setTimeout(done, 0));
    }, entry);
    const readUnknown = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="pipeline"]');
      const chip = block.querySelector('[data-stage="unknown"]');
      return { head: block.querySelector(".board-desk-head").textContent, drawn: Boolean(chip),
        word: chip?.querySelector(".board-desk-stage-word")?.textContent ?? null,
        count: chip?.querySelector(".board-desk-stage-count")?.textContent ?? null,
        type: chip?.type ?? null, reachable: chip ? chip.tabIndex >= 0 : null };
    });
    const heldUnknown = await page.evaluate(() => window.__DESK__.stages.find((one) => one.stage === "unknown")?.count ?? "absent");
    await setUnknown(1);
    await settleDesk();
    const positive = await readUnknown();
    const positiveRows = await readStage("unknown");
    await setUnknown(0);
    await settleDesk();
    const zero = await readUnknown();
    await setUnknown("absent");
    await settleDesk();
    const unlisted = await readUnknown();
    await setUnknown(null);
    await settleDesk();
    const uncounted = await readUnknown();
    ok("상태 모름 is evidence, not a stage of the flow: an exact zero draws no chip, a count the ledger did not give says — and never 0, and work in it keeps its chip with its name and number, a button the keyboard reaches that opens its rows — the flow's count is the same in every case",
      positive.drawn && positive.word === "상태 모름" && positive.count === "1" && positive.type === "button" && positive.reachable &&
      positiveRows.some((row) => row.id === "t-964") && !zero.drawn &&
      unlisted.drawn && unlisted.word === "상태 모름" && unlisted.count === "—" && uncounted.drawn && uncounted.count === "—" &&
      [zero.head, unlisted.head, uncounted.head].every((head) => head === positive.head),
      JSON.stringify({ positive, positiveRows, zero, unlisted, uncounted }));
    await setUnknown(1);
    await settleDesk();
    await page.evaluate(() => {
      window.__HELD__ = document.querySelector('#board-view [data-desk-block="pipeline"] [data-stage="closed"]');
      window.__HELD__.focus();
    });
    const kept = [];
    for (const entry of [0, 1]) {
      await setUnknown(entry);
      await settleDesk();
      kept.push(await page.evaluate(() => ({ drawn: Boolean(document.querySelector('#board-view [data-desk-block="pipeline"] [data-stage="unknown"]')),
        kept: window.__HELD__.isConnected && document.activeElement === window.__HELD__ })));
    }
    ok("a poll that takes the zero 상태 모름 chip away or brings it back keeps the focus on the chip the person left it on",
      kept.length === 2 && !kept[0].drawn && kept[1].drawn && kept.every((one) => one.kept), JSON.stringify(kept));
    await page.evaluate(() => {
      window.__HELD__.blur();
      window.__HELD__ = null;
    });
    await setUnknown(heldUnknown);
    await settleDesk();

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
      costs.some((row) => row.cost.includes("$— 사용량 원장 없는 에이전트")) && costs.some((row) => row.cost.includes("걸린 시간 —")) &&
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
    /* ---- 통과 조건과 넘김 메모 (t-26587): 조건마다 한 줄, 상태는 낱말로, 메모는 그 옆에 ---- */
    const conditioned = await page.evaluate(async () => {
      // The row must be on the screen, so the target is a task of the stage the flow has open.
      const open = document.querySelector('#board-view [data-desk-block="pipeline"] .board-desk-stage[aria-pressed="true"]')?.dataset.stage;
      const target = window.__DESK__.tasks.find((one) => one.stage === open);
      window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
        tasks: window.__DESK__.tasks.map((one) => one === target
          ? { ...one, hand_in: { conditions: [
              { number: 1, text: "cargo test -p zerocode-core is green", state: "checked" },
              { number: 2, text: "cargo clippy is clean", state: "claimed" },
              { number: 3, text: "the page renders", state: "missing" },
            ], decisions: "keep the parser", blocked: null, next: "review" } } : one) };
      refreshDeskLedger();
      for (let beat = 0; beat < 20 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
      }
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const row = document.querySelector(`#board-view [data-desk-block="pipeline"] .board-desk-task[data-task="${target.run}/${target.id}"]`);
      const list = row?.querySelector(".board-desk-task-conditions");
      const handIn = row?.querySelector(".board-desk-task-handin");
      return {
        id: target.id,
        shown: Boolean(list) && !list.hidden,
        items: [...(list?.querySelectorAll(".board-desk-condition") ?? [])].map((item) =>
          `${item.dataset.state}:${item.querySelector(".board-desk-condition-text").textContent}:${item.querySelector(".board-desk-condition-state").textContent}`),
        handIn: handIn && !handIn.hidden ? handIn.textContent : "",
      };
    });
    ok("a task with conditions shows one line per condition with its state in words, and its notes beside them",
      conditioned.shown &&
        conditioned.items.join() === "checked:1. cargo test -p zerocode-core is green:확인됨,claimed:2. cargo clippy is clean:주장,missing:3. the page renders:증거 없음" &&
        conditioned.handIn === "결정: keep the parser · 다음: review",
      JSON.stringify(conditioned));
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
    await tab("workers");
    await openAllWorkers();
    await settleMail();
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
    await tab("history");
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

    /* ---- 빌드: 이 프로젝트의 빌드 기록 그대로, 「지금」 탭의 카드 ----
     * 빌드마다 한 장, 기록의 차례대로: 이름과 표지·도구·오른쪽 끝의 상태(점과 낱말), 커밋과 몇째 단계와
     * 경과, 칸 길이가 평소 시간인 단계 띠. 단계마다의 시간은 펼쳐야 선다. 창은 어느 도구의 단계 이름도
     * 모른다 — 이름은 기록의 말이고, 말이 없으면 기록의 이름 그대로다. */
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
    // The step list is built for whoever opens it — open it on request, read, and fold it back.
    const readBuilds = (open = false) => page.evaluate(async (open) => {
      const view = document.querySelector("#board-view");
      const block = view.querySelector('[data-desk-block="builds"]');
      for (const log of block?.querySelectorAll(".board-build-log") ?? []) log.open = open;
      paintCoordinatorDesk(view);
      const shown = (node) => Boolean(node) && !node.hidden;
      const said = {
        shown: shown(block), head: block?.querySelector(".board-desk-head")?.textContent,
        builds: [...(block?.querySelectorAll(".board-build") ?? [])].map((card) => ({
          id: card.dataset.build, cls: card.className,
          line: [...card.querySelector(".board-build-line").children].filter(shown).map((node) => node.textContent),
          dot: card.querySelector(".board-build-state").dataset.dot,
          meta: [...card.querySelector(".board-build-meta").children].filter(shown).map((node) => node.textContent),
          band: [...card.querySelectorAll(".board-build-seg")].map((seg) => ({
            cls: seg.className.replace("board-build-seg ", ""), name: seg.children[1].textContent,
            time: seg.children[2].textContent, grow: Number(seg.style.flexGrow), fill: seg.style.getPropertyValue("--fill"),
          })),
          note: shown(card.querySelector(".board-build-note")) ? card.querySelector(".board-build-note").textContent : null,
          tone: card.querySelector(".board-build-note").className,
          log: card.querySelector(".board-build-log-head").textContent,
          rows: [...card.querySelectorAll(".board-build-log-step")]
            .map((row) => [row.dataset.mark, row.children[1].textContent, row.children[2].textContent]),
        })),
      };
      for (const log of block?.querySelectorAll(".board-build-log") ?? []) log.open = false;
      paintCoordinatorDesk(view);
      return said;
    }, open);
    // A changed record (`change`, a page function) is asked for and drawn the way the window takes it: the
    // answer, then the paint.
    const recordBuilds = async (change) => {
      await page.evaluate(change);
      await page.evaluate(async () => {
        await askBoardBuilds();
        paintCoordinatorDesk(document.querySelector("#board-view"));
      });
    };
    const builds = await readBuilds();
    const [app, image] = builds.builds;
    ok("the build card stands one card per recorded build in the record's order, each with its state dot, name, label, tool and state word at its right",
      builds.shown && builds.head === "빌드" && JSON.stringify(builds.builds.map((one) => one.id)) === JSON.stringify(["app", "image"])
        && JSON.stringify(app.line) === JSON.stringify(["", "앱 빌드", "1.1.20", "gradle", "도는 중"]) && app.dot === "run"
        && app.cls === "board-build is-running" && image.line[1] === "도커 이미지" && image.line[2] === "docker"
        && /^통과 · \d{2}:\d{2} 끝남$/.test(image.line[3]) && image.dot === "done" && image.cls === "board-build is-passed is-ended",
      JSON.stringify(builds));
    ok("its second line is the commit and which of how many steps it is on — the running step's own segment says how long; a finished one says what it took",
      JSON.stringify(app.meta) === JSON.stringify(["ccb6e1bda", "네 단계 중 둘째"])
        && JSON.stringify(image.meta) === JSON.stringify(["ccb6e1bda", "3분 걸림"]),
      JSON.stringify({ app: app.meta, image: image.meta }));
    ok("the band lays each step as long as it usually takes, fills the running one by its share, and names a step the record left unworded by its own name",
      JSON.stringify(app.band.map(({ cls, name, time }) => [cls, name, time])) === JSON.stringify([
        ["is-done", "컴파일", "1분 미만"], ["is-running", "단위 시험", "2분째 · 평소 4분"], ["is-ahead", "통합 시험", "약 2분"],
        ["is-ahead", "package", "—"]])
        && JSON.stringify(app.band.map((seg) => seg.grow)) === JSON.stringify([40, 240, 141, 140.33]) && app.band[1].fill === "50%",
      JSON.stringify(app.band));
    const unfolded = await readBuilds(true);
    ok("behind 「단계 N개」 each step has its mark and its time — built only when opened",
      app.log === "단계 4개" && app.rows.length === 0 && JSON.stringify(unfolded.builds[0].rows) === JSON.stringify([
        ["done", "컴파일", "1분 미만"], ["running", "단위 시험", "2분째 · 평소 4분"], ["ahead", "통합 시험", "약 2분"], ["ahead", "package", "—"]]),
      JSON.stringify({ folded: app.rows, opened: unfolded.builds[0].rows }));
    await recordBuilds(() => {
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : {
        ...one, state: "failed", reason: "unit tests failed: 3 of 212", updated_ms: one.started_ms + 5 * 60_000, running_since_ms: undefined,
        steps: [one.steps[0], { ...one.steps[1], state: "failed", secs: 95 },
          { ...one.steps[2], state: "skipped" }, { ...one.steps[3], state: "skipped" }],
      });
    });
    const failedBuild = (await readBuilds(true)).builds[0];
    ok("a failed build says so, when it ended and what it took, with the record's reason; the failed step says 실패 and its time",
      /^실패 · \d{2}:\d{2} 끝남$/.test(failedBuild.line.at(-1)) && failedBuild.dot === "halt" && failedBuild.cls === "board-build is-failed is-ended"
        && JSON.stringify(failedBuild.meta) === JSON.stringify(["ccb6e1bda", "5분 걸림"])
        && failedBuild.note === "unit tests failed: 3 of 212" && failedBuild.tone === "board-build-note is-failed"
        && JSON.stringify(failedBuild.band.map((seg) => seg.cls)) === JSON.stringify(["is-done", "is-failed", "is-skipped", "is-skipped"])
        && JSON.stringify(failedBuild.rows[1]) === JSON.stringify(["failed", "단위 시험", "실패 · 1분"]) && failedBuild.rows[2][2] === "건너뜀",
      JSON.stringify(failedBuild));
    /* 실패 까닭 (11-08 시안 v3): 기록의 `failures` — 자리가 있는 것은 단추라 누르면 편집기가 그 파일의 그 줄을 연다. */
    await recordBuilds(() => {
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : {
        ...one, failures: [
          { what: "OrderServiceTest.refundsTwice", file: "/Users/dev/shop/src/test/java/shop/OrderServiceTest.java", line: 42 },
          { what: "TS2322: Type 'string' is not assignable to type 'number'.", file: "src/cart.ts", line: 7 },
          { what: "unit tests failed" },
        ],
      });
    });
    const reasons = await page.evaluate(async () => {
      const list = document.querySelector('#board-view [data-build="app"] .board-build-failures');
      const opened = [];
      const held = window.openPath;
      window.openPath = (path, opts) => opened.push([path, opts?.line ?? null]);
      list?.querySelector("button")?.click();
      window.openPath = held;
      return {
        shown: list?.checkVisibility() ?? false,
        label: list?.getAttribute("aria-label") ?? "",
        items: [...(list?.querySelectorAll(".board-build-failure-part") ?? [])].map((one) => `${one.tagName}:${one.textContent}`),
        opened,
      };
    });
    ok("a failed build lists why under its note — each failure and its file:line, a press on one with a whole path opening the editor at that line",
      reasons.shown && reasons.label === "실패 까닭 3"
        && JSON.stringify(reasons.items) === JSON.stringify([
          "BUTTON:OrderServiceTest.refundsTwice · OrderServiceTest.java:42",
          "SPAN:TS2322: Type 'string' is not assignable to type 'number'. · cart.ts:7",
          "SPAN:unit tests failed"])
        && JSON.stringify(reasons.opened) === JSON.stringify([["/Users/dev/shop/src/test/java/shop/OrderServiceTest.java", 42]]),
      JSON.stringify(reasons));

    /* 빌드 줄 (11-08 시안 v3): 기계 하나에 줄 하나 — 도는 일 · 기다리는 일(차례) · 끝난 일, 그 자리의 단추와 줄 멈추기.
     * 단추는 `zo line`을 부르는 명령(`line_act`·`line_pause`)으로만 간다. */
    await recordBuilds(() => {
      const at = Date.now();
      const job = (id, state, extra = {}) => ({ id, project: "/Users/dev/shop/.git", checkout: "/Users/dev/shop", owner: "run-1/w-2",
        what: `${id} 일`, cap_secs: 600, command: ["cargo", "test"], state, queued_ms: at - 30 * 60_000, ...extra });
      window.__BUILDS_LINE__ = {
        jobs: [job("j-run", "running", { started_ms: at - 12 * 60_000, slot: 0 }), job("j-wait", "waiting"),
          job("j-fail", "failed", { started_ms: at - 20 * 60_000, ended_ms: at - 15 * 60_000, rc: 1 })],
        places: { "j-wait": 1 },
      };
      window.__LINE_SENT__ = [];
      window.__ANSWER__.line_act = (asked) => { window.__LINE_SENT__.push(asked); return null; };
      window.__ANSWER__.line_pause = (asked) => { window.__LINE_SENT__.push(asked); return null; };
    });
    const readLine = () => page.evaluate(() => {
      const line = document.querySelector("#board-view .board-line");
      const fold = line?.querySelector(".board-line-ended");
      return {
        ended: fold?.checkVisibility() ? [fold.textContent, fold.getAttribute("aria-expanded")] : null,
        shown: line?.checkVisibility() ?? false,
        head: line?.querySelector(".board-line-head")?.textContent ?? "",
        jobs: [...(line?.querySelectorAll(".board-line-job") ?? [])].map((one) => [one.dataset.job,
          one.querySelector(".board-line-state").textContent, one.querySelector(".board-line-what").textContent,
          one.querySelector(".board-line-where").textContent, one.querySelector(".board-line-when").textContent,
          [...one.querySelectorAll(".board-line-act")].map((button) => button.textContent).join("|")]),
        paused: line?.querySelector(".board-line-paused")?.checkVisibility() ? line.querySelector(".board-line-paused").textContent : "",
      };
    });
    // A line with a job running stands over the cards and folds what ended (11-11): the fold is pressed open to read the whole line.
    const foldedLine = await readLine();
    ok("while a job runs on it the build line shows what runs and what waits, and folds what ended behind one line",
      foldedLine.shown && JSON.stringify(foldedLine.jobs.map((one) => one[0])) === JSON.stringify(["j-run", "j-wait"])
        && JSON.stringify(foldedLine.ended) === JSON.stringify(["끝난 일감 1개", "false"]),
      JSON.stringify(foldedLine));
    await page.click("#board-view .board-line-ended");
    await page.waitForFunction(() => document.querySelectorAll("#board-view .board-line-job").length === 3);
    const lineRead = await readLine();
    ok("pressed open, the machine's build line stands whole: what runs and for how long, what waits and its place, what ended — each with its own buttons",
      lineRead.shown && lineRead.head.startsWith("빌드 줄 · 이 기계도는 1 · 기다림 1") && lineRead.head.endsWith("줄 멈추기")
        && JSON.stringify(lineRead.ended) === JSON.stringify(["끝난 일감 1개", "true"])
        && JSON.stringify(lineRead.jobs.map((one) => [one[0], one[1], one[2], one[3], one[5]])) === JSON.stringify([
          ["j-run", "도는 중", "j-run 일", "shop · run-1/w-2", "멈추기"],
          ["j-wait", "차례 기다림", "j-wait 일", "shop · run-1/w-2", "맨 앞으로|빼기"],
          ["j-fail", "실패", "j-fail 일", "shop · run-1/w-2", "다시|지우기"]])
        && lineRead.jobs[0][4] === "12분째" && lineRead.jobs[1][4] === "1번째" && /^\d{2}:\d{2}$/.test(lineRead.jobs[2][4])
        && lineRead.paused === "",
      JSON.stringify(lineRead));
    // One press at a time: a press holds the line's buttons until its answer is read again.
    for (const selector of ['#board-view .board-line-job[data-job="j-wait"] .board-line-act[data-verb="front"]',
      '#board-view .board-line-job[data-job="j-fail"] .board-line-act[data-verb="retry"]', "#board-view .board-line-pause"]) {
      await page.click(selector);
      await page.waitForFunction(() => !linePressing);
    }
    const lineSent = await page.evaluate(() => window.__LINE_SENT__);
    ok("a press on the line runs `zo line` through the window's commands: the job and its verb, the pause with the board's reason",
      JSON.stringify(lineSent) === JSON.stringify([{ job: "j-wait", verb: "front" }, { job: "j-fail", verb: "retry" },
        { pause: true, why: "상황판에서 멈춤" }]),
      JSON.stringify(lineSent));
    await recordBuilds(() => {
      window.__BUILDS_LINE__ = { ...window.__BUILDS_LINE__, pause: { by: "person", since_ms: Date.now(), why: "release lane" } };
    });
    const pausedLine = await readLine();
    ok("a paused line says so and why, and its button opens it again",
      pausedLine.paused === "줄 멈춤 · release lane" && pausedLine.head.endsWith("줄 다시 열기"), JSON.stringify(pausedLine));
    await recordBuilds(() => {
      window.__BUILDS_LINE__ = null;
      deskChoice.lineEnded = false;
    });

    /* 「멈춘 듯」은 기록이 적은 그대로다: 도는 단계의 명령이 오래 아무것도 쓰지 않았고, 쓰는 쪽이 그 단계를
     * 실패로 끝낼 상한이 있으면 그것도. 「21분」은 그리는 시계로 센다 — 시계를 한 시각에 묶는다. */
    const minute = 60_000;
    const stalled = await page.evaluate(async (minute) => {
      const trueNow = Date.now;
      const pinned = trueNow.call(Date);
      Date.now = () => pinned;
      const note = () => document.querySelector('#board-view [data-build="app"] .board-build-note');
      const draw = async (stalled) => {
        window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : { ...one, state: "running",
          running_since_ms: pinned - 25 * minute, stalled,
          steps: one.steps.map((step, at) => ({ ...step, state: ["done", "running", "ahead", "ahead"][at] })) });
        await askBoardBuilds();
        paintCoordinatorDesk(document.querySelector("#board-view"));
        return { text: note()?.textContent, hidden: note()?.hidden, tone: note()?.className, ink: getComputedStyle(note()).color };
      };
      try {
        const quiet = { step: "test", what: "gradle test", quiet_since_ms: pinned - 21 * minute - 5_000, cap_secs: 2400 };
        const withCap = await draw(quiet);
        const noCap = await draw({ ...quiet, cap_secs: 0 });
        const cleared = await draw(null);
        const probe = document.createElement("span");
        probe.style.color = "var(--signal-wait-ink)";
        document.body.append(probe);
        const wait = getComputedStyle(probe).color;
        probe.remove();
        return { withCap, noCap, cleared, wait };
      } finally { Date.now = trueNow; }
    }, minute);
    ok("a step the record says has written nothing for long stands under the card in its words, with the cap it will end failed at",
      stalled.withCap.text === "멈춘 듯: gradle test 21분 출력 없음 · 40분 넘으면 실패로 끝냄" && stalled.withCap.hidden === false
        && stalled.withCap.tone === "board-build-note is-stalled" && stalled.withCap.ink === stalled.wait
        && stalled.noCap.text === "멈춘 듯: gradle test 21분 출력 없음" && stalled.cleared.hidden === true,
      JSON.stringify(stalled));

    /* 이름은 기록의 말: 창의 말이 있으면 그것, 없으면 원문(한국어), 그것도 없으면 아무 말, 말이 없으면 기록의 이름. */
    const spoken = await page.evaluate(() => {
      const held = locale;
      locale = "en";
      try {
        paintCoordinatorDesk(document.querySelector("#board-view"));
        const card = document.querySelector('#board-view [data-build="app"]');
        return {
          head: document.querySelector('#board-view [data-desk-block="builds"] .board-desk-head').textContent,
          name: card.querySelector(".board-build-name").textContent,
          steps: [...card.querySelectorAll(".board-build-seg-name")].map((node) => node.textContent),
          only: buildWords({ ja: "コンパイル" }, "compile"), none: buildWords(null, "compile"),
        };
      } finally {
        locale = held;
        paintCoordinatorDesk(document.querySelector("#board-view"));
      }
    });
    ok("a build's words are the window's language, else the source's Korean, else any it wrote, else the record's own name",
      spoken.head === "Builds" && spoken.name === "App build"
        && JSON.stringify(spoken.steps) === JSON.stringify(["Compile", "Unit tests", "통합 시험", "package"])
        && spoken.only === "コンパイル" && spoken.none === "compile",
      JSON.stringify(spoken));

    /* 단계가 띠의 폭을 넘으면(`BUILD_BAND_MOST`) 이어진 끝난 단계끼리, 남은 단계끼리 한 칸이다 — 도는 칸은 제 칸. */
    await recordBuilds(() => {
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : { ...one, stalled: null,
        steps: Array.from({ length: 10 }, (_, at) => ({
          name: `step-${at + 1}`, state: at < 3 ? "done" : at === 3 ? "running" : "ahead",
          ...(at < 3 ? { secs: 120 } : {}), expected: { secs: 120, samples: 3 },
        })) });
    });
    const longBuild = (await readBuilds()).builds[0];
    ok("a build of more steps than the band can name folds the done ones and the ones ahead into one segment each, the running one its own",
      JSON.stringify(longBuild.band.map(({ cls, name, time }) => [cls, name, time])) === JSON.stringify([
        ["is-done", "끝난 3단계", "6분"], ["is-running", "step-4", "25분째 · 평소 2분"], ["is-ahead", "남은 6단계", "약 12분"]])
        && JSON.stringify(longBuild.band.map((seg) => seg.grow)) === JSON.stringify([360, 120, 720]) && longBuild.meta[1] === "열 단계 중 넷째",
      JSON.stringify(longBuild));

    /* ---- 이 프로젝트의 빌드만, 그것을 물은 체크아웃과 함께 ----
     * 기록이 하나도 없는 프로젝트는 카드를 접고, 접힌 카드는 앞 프로젝트의 카드를 들고 있지 않다. 답은 그것을
     * 물은 체크아웃과 함께 쥐고, 체크아웃이 바뀌면 그 체크아웃으로 다시 묻는다. 이미 쥔 판이면 답은 한 마디다. */
    const recorded = await page.evaluate(() => window.__BUILDS__);
    await recordBuilds(() => { window.__BUILDS__ = []; });
    const none = await readBuilds();
    const emptied = await page.evaluate(() =>
      document.querySelector('#board-view [data-desk-block="builds"] .board-desk-body').childElementCount);
    ok("a project with no build record folds the card, and the folded card holds no card of the project before",
      !none.shown && none.builds.length === 0 && emptied === 0, JSON.stringify({ none, emptied }));
    const switched = await page.evaluate(async (records) => {
      window.__BUILDS__ = records;
      const keep = activeWorktreePath;
      activeWorktreePath = "/repos/zerocode/workspaces/t-1";
      await askBoardBuilds();
      const here = buildsHere()?.map((one) => one.id) ?? null;
      window.__BUILDS_ASKED__ = [];
      activeWorktreePath = "/repos/other";
      const before = buildsHere();
      await askBoardBuilds();
      const after = buildsHere()?.map((one) => one.id) ?? null;
      await askBoardBuilds();
      const asked = window.__BUILDS_ASKED__.map(({ worktree, known }) => [worktree, known === window.__BUILDS_REVISION__]);
      activeWorktreePath = keep;
      await askBoardBuilds();
      return { here, before, after, asked };
    }, recorded);
    ok("the builds are held with the checkout they were asked for: after a switch there are none until asked for the new one, and a held answer is asked again by its revision",
      JSON.stringify(switched.here) === JSON.stringify(["app", "image"]) && switched.before === null
        && JSON.stringify(switched.after) === JSON.stringify(["app", "image"])
        && JSON.stringify(switched.asked) === JSON.stringify([["/repos/other", false], ["/repos/other", true]]),
      JSON.stringify(switched));

    /* ---- 순서 정하기 (빌드 카드 계획 5단계) ----
     * 순서 파일이 없는 프로젝트의 카드 밑에는 그 뿌리에서 찾은 빌드가 서고 — 기록이 하나도 없어도 —, 「순서 정하기」는
     * 그 초안으로 편집기를 연다. 기록만 있고 초안에 없는 빌드는 다른 스크립트가 알리는 자리로 끝에 붙어 온다. 차례를
     * 바꿀 때마다 쓰일 파일을 Rust에 다시 묻고, 그대로 돌릴 수 없는 순서면 까닭을 말하고 쓰지 않는다. 「이대로 쓰기」는
     * 미리 본 그 순서를 연 때의 판과 함께 쓰고, 카드는 다시 묻는다. 그대로 돌릴 수 없는 순서 파일은 까닭과 「파일 열기」다. */
    const readFoot = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="builds"]');
      const foot = block?.querySelector(".board-build-foot");
      return {
        shown: Boolean(block) && !block.hidden, cards: block?.querySelectorAll(".board-build").length ?? 0,
        words: foot && !foot.children[0].hidden ? foot.children[0].textContent : null,
        act: foot?.querySelector('.board-build-foot-act:not([data-act="build"])')?.textContent ?? null,
        refused: foot?.classList.contains("is-refused") ?? null, last: block?.querySelector(".board-desk-body")?.lastElementChild === foot,
      };
    });
    await page.evaluate(() => {
      window.__BUILDS__ = [];
      window.__BUILDS_SUGGEST__ = [
        { id: "gradle", name: { ko: "Gradle 빌드", en: "Gradle build" }, tool: "gradle" },
        { id: "image", name: { ko: "도커 이미지" }, tool: "docker" },
      ];
    });
    await recordBuilds(() => {});
    const offered = await readFoot();
    ok("a project with no order file and no build record still stands the card: the builds its root suggests, and 「순서 정하기」",
      offered.shown && offered.cards === 0 && offered.words === "이 프로젝트에서 찾은 빌드 2개 · Gradle 빌드 · 도커 이미지"
        && offered.act === "순서 정하기" && offered.refused === false && offered.last,
      JSON.stringify(offered));

    await page.evaluate(() => {
      window.__BUILD_ORDER__ = { drafts: [], previews: [], writes: [], opened: [] };
      const step = (id, ko, extra = {}) => ({ id, name: { ko }, run: `run ${id}`, ...extra });
      window.__ORDER_DRAFT__ = {
        from: "suggestion", path: "/repos/zerocode/workspaces/t-1/.zo/build.toml", name: ".zo/build.toml", version: null, comments: false,
        file: { build: [
          { id: "gradle", name: { ko: "Gradle 빌드" }, tool: "gradle", step: [step("compile", "컴파일"), step("test", "시험")] },
          { id: "image", name: { ko: "도커 이미지" }, tool: "docker", after: "gradle",
            step: [step("build", "이미지 빌드"), step("push", "레지스트리에 올리기", { when: "manual" })] },
          { id: "docs", name: "문서 배포", source: "report" },
        ] },
      };
      window.__ANSWER__.build_order_draft = ({ worktree = null } = {}) => {
        window.__BUILD_ORDER__.drafts.push(worktree);
        return structuredClone(window.__ORDER_DRAFT__);
      };
      // Rust's check, as the editor meets it: a build above the one it waits for cannot run as written.
      window.__ANSWER__.build_order_preview = ({ file }) => {
        window.__BUILD_ORDER__.previews.push(file.build.map((one) => one.id));
        const above = new Set();
        for (const build of file.build) {
          if (build.after && !above.has(build.after)) {
            throw new Error(`the build \`${build.id}\` waits for \`${build.after}\`, which is not a build above it`);
          }
          above.add(build.id);
        }
        return file.build.map((one) => `[[build]]\nid = "${one.id}"\n`).join("\n");
      };
      window.__ANSWER__.build_order_write = ({ worktree, file, version }) => {
        window.__BUILD_ORDER__.writes.push({ worktree, ids: file.build.map((one) => one.id), version, steps: file.build.map((one) => one.step?.length ?? 0) });
        return "written";
      };
    });
    const readOrder = () => page.evaluate(() => ({
      open: !el("build-order-scrim").hidden,
      from: el("build-order-from").textContent,
      rows: [...el("build-order-list").children].map((row) => ({
        at: row.querySelector(".build-order-at").textContent,
        name: row.querySelector(".build-order-name").textContent,
        tool: row.querySelector(".board-build-tool").hidden ? "" : row.querySelector(".board-build-tool").textContent,
        steps: row.querySelector(".build-order-steps").textContent,
        off: [...row.querySelectorAll("[data-order-act]")].filter((one) => one.disabled).map((one) => one.dataset.orderAct),
      })),
      note: el("build-order-note").hidden ? null : el("build-order-note").textContent,
      preview: el("build-order-preview-box").hidden ? null : el("build-order-preview").textContent,
      save: !el("build-order-save").hidden && !el("build-order-save").disabled,
      open_file: !el("build-order-open").hidden,
      focus: document.activeElement?.dataset?.orderAct ?? null,
      focusRow: document.activeElement?.closest?.(".build-order-row")?.dataset.build ?? null,
    }));
    // A press on a row's button, then the preview the move asked for.
    const pressOrder = async (row, act) => {
      const asked = await page.evaluate(() => window.__BUILD_ORDER__.previews.length);
      await page.click(`#build-order-list .build-order-row:nth-child(${row}) [data-order-act="${act}"]`);
      await page.waitForFunction((asked) => window.__BUILD_ORDER__.previews.length > asked && buildOrder && !buildOrder.pending, asked);
      return readOrder();
    };
    await page.click('#board-view [data-desk-block="builds"] .board-build-foot-act[data-act="order"]');
    await page.waitForFunction(() => buildOrder && !buildOrder.pending && !el("build-order-preview-box").hidden);
    const editorOpened = await readOrder();
    ok("「순서 정하기」 opens the editor on what the project's root suggests, for the checkout on screen: each build its number, name, tool and steps — a step a person presses says so, a build another script reports says so, one that waits names what it waits for — and the file to be written",
      editorOpened.open && editorOpened.from.includes(".zo/build.toml") && editorOpened.from.startsWith("이 프로젝트에서 찾은 빌드예요")
        && JSON.stringify(editorOpened.rows.map((row) => [row.at, row.name, row.tool])) === JSON.stringify([["1", "Gradle 빌드", "gradle"], ["2", "도커 이미지", "docker"], ["3", "문서 배포", ""]])
        && editorOpened.rows[0].steps === "컴파일 · 시험" && editorOpened.rows[1].steps === "이미지 빌드 · 레지스트리에 올리기 (손으로) · Gradle 빌드 뒤에 돌아요"
        && editorOpened.rows[2].steps === "다른 스크립트가 알려요" && editorOpened.rows[0].off.join() === "up" && editorOpened.rows[2].off.join() === "down"
        && editorOpened.preview?.includes('id = "gradle"') && editorOpened.save && !editorOpened.open_file && editorOpened.note === null
        && (await page.evaluate(() => JSON.stringify(window.__BUILD_ORDER__.drafts) === JSON.stringify([activeWorktreePath]))),
      JSON.stringify(editorOpened));
    const broke = await pressOrder(2, "up");
    ok("a build moved above the build it waits for cannot run as written: the editor says why, shows no file and writes nothing — and the focus stays on that build",
      JSON.stringify(broke.rows.map((row) => row.name)) === JSON.stringify(["도커 이미지", "Gradle 빌드", "문서 배포"])
        && broke.note?.includes("waits for `gradle`") && broke.preview === null && !broke.save
        && broke.focusRow === "image" && broke.focus === "down",
      JSON.stringify(broke));
    const mended = await pressOrder(1, "down");
    ok("moved back below it the order runs again, and the file to be written stands",
      JSON.stringify(mended.rows.map((row) => row.name)) === JSON.stringify(["Gradle 빌드", "도커 이미지", "문서 배포"])
        && mended.note === null && mended.preview !== null && mended.save && mended.focusRow === "image" && mended.focus === "down",
      JSON.stringify(mended));
    const trimmed = await pressOrder(3, "remove");
    ok("a build taken out leaves the order, and the rest renumber",
      JSON.stringify(trimmed.rows.map((row) => [row.at, row.name])) === JSON.stringify([["1", "Gradle 빌드"], ["2", "도커 이미지"]])
        && trimmed.save && trimmed.rows[1].off.join() === "down",
      JSON.stringify(trimmed));
    const askedBefore = await page.evaluate(() => window.__BUILDS_ASKED__.length);
    await page.click("#build-order-save");
    await page.waitForFunction((before) => el("build-order-scrim").hidden && window.__BUILDS_ASKED__.length > before, askedBefore);
    const written = await page.evaluate(() => ({ writes: window.__BUILD_ORDER__.writes, here: activeWorktreePath, open: buildOrder !== null }));
    ok("「이대로 쓰기」 writes the order it previewed, once, for the checkout it was opened for and over no file, closes the editor and asks the card again",
      JSON.stringify(written.writes) === JSON.stringify([{ worktree: written.here, ids: ["gradle", "image"], version: null, steps: [2, 2] }])
        && !written.open,
      JSON.stringify(written));

    await page.evaluate(() => {
      window.__BUILDS__ = [];
      window.__BUILDS_SUGGEST__ = [];
      window.__BUILDS_FILE__ = { state: "refused", reason: "the build `app` has no step to run" };
      window.__ORDER_DRAFT__ = { ...window.__ORDER_DRAFT__, from: "refused", file: {}, version: 7, reason: window.__BUILDS_FILE__.reason };
      window.__HELD_OPEN_FILE__ = openFile;
      openFile = async (path) => { window.__BUILD_ORDER__.opened.push(path); };
    });
    await recordBuilds(() => {});
    const refusedFoot = await readFoot();
    await page.click('#board-view [data-desk-block="builds"] .board-build-foot-act[data-act="file"]');
    await page.waitForFunction(() => window.__BUILD_ORDER__.opened.length === 1);
    const openedFile = await page.evaluate(() => {
      openFile = window.__HELD_OPEN_FILE__;
      return { opened: window.__BUILD_ORDER__.opened, editor: !el("build-order-scrim").hidden };
    });
    ok("an order file that cannot run as written stands the card with its reason and 「파일 열기」, which opens that file — never the editor",
      refusedFoot.shown && refusedFoot.refused && refusedFoot.act === "파일 열기"
        && refusedFoot.words === "순서 파일을 그대로 돌릴 수 없어요 · the build `app` has no step to run"
        && JSON.stringify(openedFile.opened) === JSON.stringify(["/repos/zerocode/workspaces/t-1/.zo/build.toml"]) && !openedFile.editor,
      JSON.stringify({ refusedFoot, openedFile }));
    await page.evaluate((records) => {
      window.__BUILDS__ = records;
      window.__BUILDS_FILE__ = { state: "read" };
    }, recorded);
    await recordBuilds(() => {});
    const readFile = await readFoot();
    ok("with an order file that reads the card's last line is 「순서 정하기」 alone, under the cards",
      readFile.cards === 2 && readFile.words === null && readFile.act === "순서 정하기" && !readFile.refused && readFile.last,
      JSON.stringify(readFile));

    /* ---- 빌드 단추 (빌드 카드 계획 6단계) ----
     * 단추는 `zo build`가 돌리는 빌드에만 선다: 도는 빌드는 「멈추기」, 끝난 빌드는 러너가 이 프로젝트를 쥐지 않을
     * 때만 「다시」, 사람이 누를 단계에서 기다리는 빌드는 그 단계와 「멈추기」, 다른 스크립트가 알리는 빌드는 아무것도.
     * 카드 밑의 「빌드」는 러너가 없을 때 `zo build`가 돌리는 빌드 모두를 화면의 체크아웃으로 띄운다. 아무도 허락하지 않은
     * 명령이 있으면 아무것도 돌지 않고 그 명령을 보여 주며, 「허락하고 빌드」는 본 그대로를 허락으로 보낸다. */
    await page.evaluate(() => {
      window.__PRESSES__ = [];
      window.__ANSWER__.build_start = ({ worktree, builds, step = null, from = null, allowed = [] }) => {
        window.__PRESSES__.push({ verb: "start", worktree, builds, step, from, allowed });
        return allowed.length === 0 && window.__UNALLOWED__.length > 0
          ? { state: "approve", commands: window.__UNALLOWED__ }
          : { state: "started" };
      };
      window.__ANSWER__.build_stop = ({ worktree, builds }) => {
        window.__PRESSES__.push({ verb: "stop", worktree, builds, step: null, from: null, allowed: [] });
        return null;
      };
      window.__ANSWER__.build_act = ({ worktree, build, act, allowed = [] }) => {
        window.__PRESSES__.push({ verb: "act", worktree, builds: [build], act, allowed });
        return { state: "started" };
      };
      window.__ANSWER__.build_forget = ({ worktree, build, startedMs }) => {
        window.__PRESSES__.push({ verb: "forget", worktree, builds: [build], startedMs });
        return null;
      };
      window.__ANSWER__.build_restore = ({ worktree, build, startedMs }) => {
        window.__PRESSES__.push({ verb: "restore", worktree, builds: [build], startedMs });
        return null;
      };
      window.__BUILDS_RUNNABLE__ = ["app", "image"];
      window.__UNALLOWED__ = ["./gradlew test", "docker build -t shop:${sha8} ."];
    });
    await recordBuilds(() => {});
    const readActs = () => page.evaluate(() => {
      const block = document.querySelector('#board-view [data-desk-block="builds"]');
      const acts = (id) => [...block.querySelectorAll(`.board-build[data-build="${id}"] .board-build-act`)]
        .filter((button) => !button.hidden && !button.parentElement.hidden).map((button) => button.textContent);
      const run = block.querySelector('.board-build-foot-act[data-act="build"]');
      return { app: acts("app"), image: acts("image"), run: run && !run.hidden ? run.textContent : null };
    });
    const runningActs = await readActs();
    ok("a running build of `zo build` offers 「멈추기」 alone; while a runner holds the project an ended build offers no 「다시」 and the foot no 「빌드」",
      JSON.stringify(runningActs) === JSON.stringify({ app: ["멈추기"], image: [], run: null }), JSON.stringify(runningActs));

    await recordBuilds(() => {
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : {
        ...one, state: "failed", running_since_ms: null, reason: "`test` exited 1", log: true,
        steps: one.steps.map((step) => step.state === "running" ? { ...step, state: "failed", secs: 12 }
          : step.state === "ahead" ? { ...step, state: "skipped" } : step),
      });
    });
    const endedActs = await readActs();
    ok("with no runner holding the project each ended build offers 「다시」 — from the step a run failed at when that is not its first — and the foot 「빌드 실행」",
      JSON.stringify(endedActs) === JSON.stringify({ app: ["step-4부터 다시"], image: ["다시"], run: "빌드 실행" }), JSON.stringify(endedActs));

    await page.click('#board-view [data-desk-block="builds"] .board-build-foot-act[data-act="build"]');
    await page.waitForFunction(() => !el("build-approve-scrim").hidden && !buildPressing);
    const approval = await page.evaluate(() => ({
      listed: [...el("build-approve-list").querySelectorAll("code")].map((code) => code.textContent),
      presses: window.__PRESSES__.slice(), here: activeWorktreePath,
      focused: document.activeElement?.id ?? null,
    }));
    await page.click("#build-approve-yes");
    await page.waitForFunction(() => window.__PRESSES__.length === 2 && el("build-approve-scrim").hidden && !buildPressing);
    const allowedPress = await page.evaluate(() => window.__PRESSES__[1]);
    ok("「빌드」 asks for every build `zo build` runs, for the checkout on screen; commands no one allowed open a list, nothing runs until 「허락하고 빌드」, which sends exactly those as allowed",
      JSON.stringify(approval.listed) === JSON.stringify(["./gradlew test", "docker build -t shop:${sha8} ."])
        && JSON.stringify(approval.presses) === JSON.stringify([
          { verb: "start", worktree: approval.here, builds: ["app", "image"], step: null, from: null, allowed: [] }])
        && approval.focused === "build-approve-cancel"
        && JSON.stringify(allowedPress) === JSON.stringify(
          { verb: "start", worktree: approval.here, builds: ["app", "image"], step: null, from: null, allowed: approval.listed }),
      JSON.stringify({ approval, allowedPress }));

    await recordBuilds(() => {
      window.__UNALLOWED__ = [];
      window.__PRESSES__ = [];
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "image" ? one : {
        ...one, state: "waiting", waiting: "push",
        steps: [...one.steps, { name: "push", words: { ko: "레지스트리 올리기" }, state: "ahead", expected: { secs: 0, samples: 0 } }],
      });
    });
    const manualActs = await readActs();
    const manualCard = await page.evaluate(() => {
      const card = document.querySelector('#board-view .board-build[data-build="image"]');
      const state = card.querySelector(".board-build-state");
      return { state: state.textContent, dot: state.dataset.dot, note: card.querySelector(".board-build-note").textContent };
    });
    ok("a build waiting at a step a person presses is their turn: its note names the step and it offers that step and 「멈추기」, while the foot still offers 「빌드」",
      JSON.stringify(manualActs) === JSON.stringify({ app: ["step-4부터 다시"], image: ["레지스트리 올리기 실행", "멈추기"], run: "빌드 실행" })
        && manualCard.state === "누를 차례" && manualCard.dot === "you" && manualCard.note === "레지스트리 올리기 단계는 누를 때 돌아요",
      JSON.stringify({ manualActs, manualCard }));

    for (const [id, act] of [["image", "step"], ["image", "stop"], ["app", "again"]]) {
      const before = await page.evaluate(() => window.__PRESSES__.length);
      await page.click(`#board-view .board-build[data-build="${id}"] .board-build-act[data-act="${act}"]`);
      await page.waitForFunction((before) => window.__PRESSES__.length > before && !buildPressing, before);
    }
    const cardPresses = await page.evaluate(() => window.__PRESSES__.map(({ verb, builds, step, from }) => ({ verb, builds, step, from })));
    ok("a card's buttons name their own build: the waiting step goes on from that step, 「멈추기」 stops that build, 「…부터 다시」 runs it again from the step it failed at",
      JSON.stringify(cardPresses) === JSON.stringify([
        { verb: "start", builds: ["image"], step: "push", from: null }, { verb: "stop", builds: ["image"], step: null, from: null },
        { verb: "start", builds: ["app"], step: null, from: "step-4" }]),
      JSON.stringify(cardPresses));

    /* ---- 다시 ▾ · 로그 · ⋯ (11-08 시안 v3) ----
     * ▾는 처음부터 다시를 고르게 하고, 「로그」는 그 실행의 로그 끝을 카드 안에 편다(일정 탭과 같은 물음), ⋯의 「기록 지우기」는
     * `build_forget`으로 가고 5초 동안 「되돌리기」가 `build_restore`로 간다. 다른 스크립트가 알리는 빌드는 순서 파일이 다시
     * 명령을 적었을 때만 「다시」를 들고, 그것은 `build_act`로 간다. */
    await page.evaluate(() => {
      window.__PRESSES__ = [];
      window.__LOG_ASKED__ = [];
      window.__ANSWER__.schedule_timeline = (args = {}) => {
        if (args.log) window.__LOG_ASKED__.push(args.log);
        return { revision: 1, unchanged: false, read_ms: Date.now(), line: { found: false }, jobs: [], builds: [],
          ...(args.log ? { log: { build: args.log.build, lines: ["> Task :test", "UnitTest > adds() FAILED", "BUILD FAILED"], cut: false, missing: false } } : {}) };
      };
    });
    await page.click('#board-view .board-build[data-build="app"] .board-build-caret');
    await page.waitForSelector('#board-view .board-build[data-build="app"] .board-build-menu-item');
    const againMenu = await page.evaluate(() => [...document.querySelectorAll('#board-view .board-build[data-build="app"] .board-build-menu-item')]
      .map((item) => item.textContent));
    await page.click('#board-view .board-build[data-build="app"] .board-build-menu-item[data-item="again-start"]');
    await page.waitForFunction(() => window.__PRESSES__.length === 1 && !buildPressing);
    await page.click('#board-view .board-build[data-build="app"] .board-build-aux[data-act="log"]');
    await page.waitForFunction(() => document.querySelector('#board-view .board-build[data-build="app"] .board-build-tail')?.textContent.includes("BUILD FAILED"));
    const tail = await page.evaluate(() => ({
      asked: window.__LOG_ASKED__.slice(),
      text: document.querySelector('#board-view .board-build[data-build="app"] .board-build-tail').textContent,
      started: Number(document.querySelector('#board-view .board-build[data-build="app"]').dataset.started),
    }));
    await page.click('#board-view .board-build[data-build="app"] .board-build-aux[data-act="more"]');
    await page.waitForSelector('#board-view .board-build[data-build="app"] .board-build-menu-item[data-item="forget"]');
    const moreMenu = await page.evaluate(() => [...document.querySelectorAll('#board-view .board-build[data-build="app"] .board-build-menu-item')]
      .map((item) => [item.dataset.item, item.textContent]));
    await page.click('#board-view .board-build[data-build="app"] .board-build-menu-item[data-item="forget"]');
    await page.waitForFunction(() => window.__PRESSES__.length === 2 && !buildPressing);
    const undo = await page.evaluate(() => {
      const action = [...document.querySelectorAll(".toast-action")].find((button) => button.textContent === "되돌리기");
      action?.click();
      return Boolean(action);
    });
    await page.waitForFunction(() => window.__PRESSES__.length === 3);
    const menuPresses = await page.evaluate(() => window.__PRESSES__.map(({ verb, builds, from, startedMs }) => ({ verb, builds, from: from ?? null, startedMs: startedMs ?? null })));
    ok("▾ offers the start and the failed step, 「처음부터 다시」 runs it with no step; 「로그」 opens that run's log end in the card; ⋯ 「기록 지우기」 forgets that run and the toast's 「되돌리기」 brings it back",
      JSON.stringify(againMenu) === JSON.stringify(["처음부터 다시", "step-4부터 다시"])
        && JSON.stringify(menuPresses) === JSON.stringify([
          { verb: "start", builds: ["app"], from: null, startedMs: null },
          { verb: "forget", builds: ["app"], from: null, startedMs: tail.started },
          { verb: "restore", builds: ["app"], from: null, startedMs: tail.started }])
        && tail.asked.length >= 1 && tail.asked[0].build?.id === "app" && tail.asked[0].build?.started_ms === tail.started
        && tail.text.endsWith("BUILD FAILED") && undo
        && JSON.stringify(moreMenu.map(([id]) => id)) === JSON.stringify(["forget"]),
      JSON.stringify({ againMenu, menuPresses, tail, moreMenu, undo }));

    await recordBuilds(() => {
      window.__BUILDS_RUNNABLE__ = ["app"];
      window.__BUILDS_REPORT_ACTS__ = { image: { retry: true, stop: false } };
      window.__PRESSES__ = [];
      // The image run another script reports has ended: its retry stands.
      window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "image" ? one : { ...one, state: "passed", waiting: null });
    });
    const reportedRetry = await readActs();
    await page.click('#board-view .board-build[data-build="image"] .board-build-act[data-act="again"]');
    await page.waitForFunction(() => window.__PRESSES__.length === 1 && !buildPressing);
    const actPress = await page.evaluate(() => window.__PRESSES__[0]);
    ok("a build another script reports offers 「다시」 when the order file names its retry, and it goes to `build_act`",
      JSON.stringify(reportedRetry.image) === JSON.stringify(["다시"]) && actPress.verb === "act"
        && actPress.act === "retry" && JSON.stringify(actPress.builds) === JSON.stringify(["image"]),
      JSON.stringify({ reportedRetry, actPress }));
    await page.evaluate(() => { delete window.__BUILDS_REPORT_ACTS__; });

    await recordBuilds(() => { window.__BUILDS_RUNNABLE__ = ["app"]; });
    const reportedActs = await readActs();
    ok("a build another script reports offers no button — that script runs it",
      JSON.stringify(reportedActs.image) === "[]" && JSON.stringify(reportedActs.app) === JSON.stringify(["step-4부터 다시"]),
      JSON.stringify(reportedActs));
    await page.evaluate((records) => {
      window.__BUILDS__ = records;
      delete window.__BUILDS_RUNNABLE__;
    }, recorded);

    /* ---- 단계 안 진행 (빌드 카드 계획 7단계) ----
     * 도는 단계의 출력이 말한 단위와 그 전체가 있으면 도는 칸은 그것을 말하고 그만큼 찬다. 전체를 넘어섰거나 전체를
     * 모르면 시간이 말한다. */
    const runningSegmentWith = async (progress) => {
      await page.evaluate((progress) => {
        window.__BUILDS__ = window.__BUILDS__.map((one) => one.id !== "app" ? one : {
          ...one,
          steps: one.steps.map(({ progress: held, ...step }) => (step.state === "running" && progress ? { ...step, progress } : step)),
        });
      }, progress);
      await recordBuilds(() => {});
      const [app] = (await readBuilds()).builds;
      return app.band.find((seg) => seg.cls.startsWith("is-running"));
    };
    const halfway = await runningSegmentWith({ done: 212, of: 480 });
    const beyond = await runningSegmentWith({ done: 500, of: 480 });
    const wholeless = await runningSegmentWith({ done: 5 });
    const byTime = await runningSegmentWith(null);
    ok("a running step whose output says how far it is says it and fills that far; past its whole or with no whole, time says it as before",
      halfway.time === `212/480 · ${byTime.time.split(" · ")[0]}` && halfway.fill === "44%" && halfway.cls === "is-running"
        && [beyond, wholeless].every((seg) => seg.time === byTime.time && seg.fill === byTime.fill),
      JSON.stringify({ halfway, beyond, wholeless, byTime }));
    await page.evaluate(() => {
      delete window.__BUILDS_FILE__;
      delete window.__BUILDS_SUGGEST__;
    });
    await recordBuilds(() => {});

    /* ---- 계속 판정 (t-26583): 워커 줄 밑의 판정 한 줄과 그 근거 ------------------
     *
     * 판정·사유·숫자는 백엔드의 것이고(`continue_gate`) 행의 `gate`로 온다; 여기서 재는 것은
     * 그 낱말과 서식이다. 판정을 받은 워커에만 줄이 서고, 「계속」이어도 읽은 숫자가 보이며,
     * 비용을 못 읽으면 0원이 아니라 까닭이 보이고, 조용한 폴은 아무것도 다시 쓰지 않는다. */
    await tab("workers");
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
    await openAllWorkers();
    await settleMail();
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
    const gatedRows = (await readWorkers()).rows;
    // The calls the gate read moved from a cell of their own into the row's facts (11-08 draft v3: seven cells).
    ok("the row's cost is the number the gate read — an unread cost a dash with its reason —, its calls stand in the row's facts, and a gate that holds the worker says so beside its state: a pause, a stop, never going on",
      gatedRows["w-1"]?.cells[4] === "$3.41" && (gatedRows["w-1"].facts ?? "").includes("호출 142") && gatedRows["w-1"].verdict === "" &&
      gatedRows["w-3"]?.verdict === "멈춤" && gatedRows["w-5"]?.verdict === "중지" &&
      gatedRows["w-4"]?.cells[4] === "—" && gatedRows["w-4"].costTip === "이 CLI의 기록 형식은 아직 못 읽음" &&
      (gatedRows["w-4"].facts ?? "").includes("호출 90") && !(gatedRows["w-2"]?.facts ?? "").includes("호출"), JSON.stringify(gatedRows));
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
      await askBoardBuilds();
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
        // The workers' table stands in a card that clips (t-70264): what runs past the card is asked of the card.
        const cards = [...view.querySelectorAll(".board-table-card, .board-desk-block")].filter((one) => one.checkVisibility());
        return { width: surface.clientWidth, scroll: surface.scrollWidth, desk: desk.scrollWidth, deskWidth: desk.clientWidth,
          cards: Math.max(0, ...cards.map((one) => one.scrollWidth - one.clientWidth)) };
      });
      ok(`the desk has no horizontal overflow at ${width}px`,
        size.scroll <= size.width + 1 && size.desk <= size.deskWidth + 1 && size.cards <= 1, JSON.stringify(size));
      await page.screenshot({ path: `output/playwright/coordinator-desk/dark-${width}.png` });
      // The same width with what a real desk holds and this fixture does not: a task title and a run's name longer
      // than the card, a state said without a space (a Japanese or Chinese sentence breaks at no word), and a host
      // whose fallback glyphs run wider than this one's. The hosted Windows runner's Hangul put the two columns of
      // the workers' table 4px past its card at 360px (v1.1.135–138), and a title that long pushed the table out of
      // the card at every width — 384px at 360, 178px at 1280.
      const stretched = await page.evaluate(async () => {
        const view = document.querySelector("#board-view");
        const pastTheCards = () => Math.max(0, ...[...view.querySelectorAll(".board-table-card, .board-desk-block")]
          .filter((one) => one.checkVisibility()).map((one) => one.scrollWidth - one.clientWidth));
        const frames = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const held = [];
        const say = (selector, words) => {
          const node = view.querySelector(selector);
          if (node === null) return false;
          held.push([node, node.textContent]);
          node.textContent = words;
          return true;
        };
        const said = {
          title: say(".board-desk-worker-task", "v1.1.130 Windows 일정 보드 조용한 보기가 5 ms를 넘음 — Book::read 몫 측정 뒤 고침 또는 OS별 상한"),
          run: say(".board-desk-worker-run-name", "lowspec-20261105: 저사양 1~6(상태 파일·테두리·CEF·부팅 JS·원장·쉬는 에이전트) · 워커 5"),
          state: say(".board-desk-worker-state .board-state", "制限に達しましたのでリセットまであと四十四分お待ちください"),
        };
        await frames();
        const longWords = pastTheCards();
        for (const [node, words] of held) node.textContent = words;
        const wider = document.createElement("style");
        wider.textContent = "*, *::before, *::after { letter-spacing: 0.1em !important; }";
        document.head.appendChild(wider);
        await frames();
        const widerGlyphs = pastTheCards();
        wider.remove();
        await frames();
        return { said, longWords, widerGlyphs };
      });
      ok(`the desk keeps a long title, a long run name and an unbroken state inside their cards at ${width}px`,
        Object.values(stretched.said).every(Boolean) && stretched.longWords <= 1, JSON.stringify(stretched));
      ok(`the desk has no horizontal overflow at ${width}px where the host's glyphs run wider`,
        stretched.widerGlyphs <= 1, JSON.stringify(stretched));
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
