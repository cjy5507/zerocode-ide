/* ---- 코디네이터 데스크 (t-6588) ---------------------------------------------
 *
 * 작업 상황판 위의 다섯 답. 하루 동안 코디네이터가 원장 CLI(`worker-list`·
 * `check --peek`·`task-list`·`dispatch-show`)와 `df -g`·`uptime`·레인의
 * `status.sh`·`xcrun simctl list`를 수백 번 쳐서 물은 것을, 판이 먼저 답한다
 * (docs/design/agent-board-round4.md): 기계 띠 · 답할 우편 · 워커 · 과업
 * 흐름 · 릴리즈 레인.
 *
 * 새 화면이 아니다. 작업 보드의 머리·레일·인스펙터는 3차 그대로이고, 데스크는
 * 작업 목록 위에 원장의 장(章)처럼 선다 — 위에서 아래로 읽고, 넓은 판에서는
 * 두 칸으로 폭을 나눈다(shell.css 「코디네이터 데스크」 절).
 *
 * 읽는 것은 창이 이미 들고 있는 한 벌뿐이다. 원장은 1초 박자가 게시한 것을
 * 읽고(`ledger_agents`·`board_desk`), 릴리즈 레인은 상태 바가 이미 읽는 그
 * 파일(`release_status`)을 읽는다. 셈(단계·상태·ack 가능 여부)은 백엔드의
 * 것이고 여기는 그 답을 그린다 — 같은 사실을 두 곳에서 세면 두 답이 된다. */

/* 데스크의 박자와 상한, 한 표. */
const DESK = Object.freeze({
  /* 데스크가 보일 때만 도는 느린 박자: 릴리즈 레인과 기계 띠. 상태 바의
   * 릴리즈 박자는 15분(`USAGE_AMBIENT_MS`)이라 도는 레인의 단계를 따라가지
   * 못한다 — 레인의 한 단계는 수 분이다. 같은 문(`askReleaseStatus`)을 더
   * 자주 부를 뿐 읽는 손은 하나다. */
  ambientEveryMs: 60_000,
});

/* 원장의 데스크 읽기(`board_desk`): 판 위의 런, 그 과업의 단계와 수. 1초
 * 박자가 게시한 것을 원장이 움직였다고 말할 때(`ledger:changed`)만 다시 읽는다.
 * `null`은 아직 읽지 않았거나 이 창에 원장이 없는 것이다. */
let deskLedger = null;
let deskLedgerSaid = "";
let deskLedgerAsking = false;
let deskLedgerAgain = false;

/* 사람이 고른 것: 목록을 펼친 단계. 판이 다시 그려져도 남는다 — 창의 기억이지
 * 원장의 것이 아니다. */
const deskChoice = { stage: undefined };

/* 데스크를 그린 적이 있는 판. 복제된 판(`docHost`)은 첫 판의 노드를 죽은
 * 마크업으로 들고 오므로, 처음 그릴 때 한 번 비운다 — 작업 목록과 같은 규칙. */
const deskPainted = new WeakSet();

let deskPaintFrame = null;
/* 느린 박자가 마지막으로 물은 때. 보드를 열면 이것이 박자 하나보다 오래됐을
 * 때만 곧바로 한 번 묻는다 — 열 때마다 묻지도, 1분을 기다리지도 않는다. */
let deskAmbientAt = 0;

/* 데스크가 서 있는 판들: 작업 보기의 보드 한 장(팝아웃이면 그 창의 한 장). */
function deskViews() {
  const tab = boardTab();
  if (!tab) return [];
  const view = docHost(tab.pane, "board");
  return view && !view.hidden && view.classList.contains("is-task-board") ? [view] : [];
}

/* 데스크의 사실만 움직였을 때 — 보드 전체(`board_snapshot`)를 다시 묻지 않고
 * 데스크만 한 프레임에 한 번 다시 그린다. */
function scheduleDeskPaint() {
  if (deskPaintFrame !== null) return;
  deskPaintFrame = requestAnimationFrame(() => {
    deskPaintFrame = null;
    for (const view of deskViews()) paintCoordinatorDesk(view);
  });
}

/* 원장의 데스크 읽기를 다시 한 번. 같은 답이면 그리지 않는다 — 게시된 한 벌을
 * 읽는 값은 거의 없고, 그리는 값은 답이 달라졌을 때만 치른다. */
function refreshDeskLedger() {
  if (deskLedgerAsking) {
    deskLedgerAgain = true;
    return;
  }
  deskLedgerAsking = true;
  invoke("board_desk")
    .then((answer) => {
      const said = JSON.stringify(answer ?? null);
      if (said === deskLedgerSaid) return;
      deskLedgerSaid = said;
      deskLedger = answer && typeof answer === "object" ? answer : null;
      scheduleDeskPaint();
    })
    .catch(() => {})
    .finally(() => {
      deskLedgerAsking = false;
      if (deskLedgerAgain) {
        deskLedgerAgain = false;
        refreshDeskLedger();
      }
    });
}

listen("ledger:changed", () => {
  if (deskViews().length > 0) refreshDeskLedger();
});

function refreshDeskAmbient() {
  deskAmbientAt = Date.now();
  void askReleaseStatus().then(scheduleDeskPaint);
  void askMachineLoad();
}

function deskElement(tag, className, text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/* 블록 하나: 장의 머리(낱말 · 수) 아래 몸. 한 번 짓고 키로 살아남는다. 한 줄로
 * 읽히는 블록(기계 띠)의 머리는 낭독기에게만 선다. */
function deskBlock(desk, { id, quietHead = false }) {
  let block = desk.querySelector(`:scope > [data-desk-block="${id}"]`);
  if (!block) {
    block = deskElement("section", "board-desk-block");
    block.dataset.deskBlock = id;
    block.append(deskElement("h3", quietHead ? "sr" : "task-board-section-head board-desk-head"),
      deskElement("div", "board-desk-body"));
  }
  return block;
}

/* 데스크 블록, 읽는 차례대로 — DOM 순서가 곧 좁은 판의 순서다. 그리는 손은
 * 블록에 쓸 것이 있으면 참을 돌려주고, 거짓이면 그 블록은 접힌다. */
const DESK_BLOCKS = Object.freeze([
  { id: "machine", paint: paintDeskMachine, quietHead: true },
  { id: "pipeline", paint: paintDeskPipeline },
  { id: "release", paint: paintDeskRelease },
]);

function paintCoordinatorDesk(view) {
  const desk = view.querySelector(".task-board-desk");
  if (!desk) return;
  if (!deskPainted.has(view)) {
    desk.replaceChildren();
    deskPainted.add(view);
    refreshDeskLedger();
  }
  deskAmbient.sync();
  const now = Date.now();
  if (now - deskAmbientAt >= DESK.ambientEveryMs) refreshDeskAmbient();
  const blocks = DESK_BLOCKS.map((entry) => {
    const block = deskBlock(desk, entry);
    writeHidden(block, !entry.paint(block, now, view));
    return block;
  });
  reconcileElementOrder(desk, blocks);
  writeHidden(desk, blocks.every((block) => block.hidden));
}

/* ---- 기계 띠 ---------------------------------------------------------------
 *
 * `df -g`·`uptime`·`xcrun simctl list`의 자리(`machine_load`): 디스크 여유와 그
 * 말 — 원장이 `--worktree` 소환을 거절하고 경고하는 바로 그 규칙(새 워크트리 하나가
 * 들어가는가, 워커 체크아웃이 다 자라도 들어가는가) — 1분 부하와 코어 수(레인의
 * 조용한 기다림·하네스의 `machine-load.mjs`와 같은 잣대: 코어보다 크면 붐빔), 켠
 * iOS 시뮬레이터와 Android 에뮬레이터 수. 빌린 기기는 상태 바 칩과 같은 상태의 같은
 * 문장(`emulatorLoansWords`)이다 — 따로 세지 않는다. */

/* `machine_load`의 마지막 답. `null`은 아직 묻지 않았거나 답하지 못한 것이다. */
let deskMachine = null;
let deskMachineAsking = false;

/* 디스크의 말 셋: 원장의 워크트리 규칙이 낸 판정(`WorktreeRoom`)과 그 색. */
const DESK_DISK_ROOM = Object.freeze({
  refused: { tone: "halt", key: "board.desk.diskRefused", word: "새 워크트리 하나도 들어가지 않음" },
  tight: { tone: "wait", key: "board.desk.diskTight", word: "워커 체크아웃 {{held}}개가 자라면 모자람" },
  room: { tone: "", key: "", word: "" },
});

function askMachineLoad() {
  if (deskMachineAsking) return;
  deskMachineAsking = true;
  invoke("machine_load")
    .then((answer) => {
      deskMachine = answer && typeof answer === "object" ? answer : null;
      scheduleDeskPaint();
    })
    .catch(() => {})
    .finally(() => { deskMachineAsking = false; });
}

/* 띠의 조각들, 차례대로: 무엇이라 말하고 어떤 색인가. 모르는 조각은 서지 않는다. */
function deskMachineSegments(machine, now) {
  const segments = [];
  const disk = machine?.disk;
  if (disk && typeof disk.free === "string") {
    const room = DESK_DISK_ROOM[disk.room] ?? DESK_DISK_ROOM.room;
    const free = t("board.desk.disk", "디스크 {{free}} 남음", { free: disk.free });
    const note = room.key ? t(room.key, room.word, { held: disk.held_checkouts }) : "";
    segments.push({ id: "disk", tone: room.tone, text: [free, note].filter(Boolean).join(" · "), tip: disk.at ?? "" });
  }
  const load = machine?.load;
  if (load && Number.isFinite(load.one_minute)) {
    const figures = { load: load.one_minute.toFixed(1), cores: load.cores };
    segments.push({
      id: "load", tone: load.loud ? "wait" : "",
      text: load.loud
        ? t("board.desk.loadBusy", "부하 {{load}} · 코어 {{cores}} · 붐빔", figures)
        : t("board.desk.load", "부하 {{load}} · 코어 {{cores}}", figures),
    });
  }
  const devices = machine?.devices ?? {};
  if (Number.isInteger(devices.ios_booted)) {
    segments.push({ id: "ios", tone: "",
      text: t("board.desk.ios", "iOS 시뮬레이터 {{count}}", { count: devices.ios_booted }) });
  }
  if (Number.isInteger(devices.android_booted)) {
    segments.push({ id: "android", tone: "",
      text: t("board.desk.android", "Android 에뮬레이터 {{count}}", { count: devices.android_booted }) });
  }
  const loans = emulatorLoansWords(now);
  if (loans) segments.push({ id: "loans", tone: "", text: loans });
  return segments;
}

function paintDeskMachine(block, now) {
  const segments = deskMachineSegments(deskMachine, now);
  if (segments.length === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.machine", "이 기계"));
  const body = block.lastElementChild;
  let line = body.firstElementChild;
  if (!line) {
    line = deskElement("p", "board-desk-machine");
    body.replaceChildren(line);
  }
  const held = new Map([...line.children].map((node) => [node.dataset.segment, node]));
  reconcileElementOrder(line, segments.map((segment) => {
    const node = held.get(segment.id) ?? deskElement("span", "");
    writeAttribute(node, "data-segment", segment.id);
    writeClassName(node, `board-desk-machine-segment${segment.tone ? ` is-${segment.tone}` : ""}`);
    writeTextContent(node, segment.text);
    if (segment.tip) writeAttribute(node, "data-tip", segment.tip);
    return node;
  }));
  return true;
}

/* 빌린 기기가 바뀌면 띠도 — 상태 바가 먼저 제 상태를 고친 뒤다(리스너 순서). */
listen("emulator:loans", () => scheduleDeskPaint());

/* ---- 과업 흐름 --------------------------------------------------------------
 *
 * `task-list`의 자리: 판 위의 런이 적어 둔 과업을 단계 하나씩으로 센다 —
 * 선행 대기 → 준비 → 진행 → 보고됨 → 병합, 그리고 멈춰 선 셋(게이트·막힘·실패).
 * 단계를 정하는 것은 원장의 사실이고(`desk::stage_of`) 수도 백엔드가 센다; 여기는
 * 그 수를 그리고, 누른 단계의 과업을 펼친다. 멈춰 선 단계에 과업이 있으면 처음부터
 * 펼쳐져 있다 — 코디네이터가 가장 먼저 물은 것이 그것이다. */

/* 단계의 낱말과 색, 백엔드의 `STAGES` 순서 그대로. `flow`는 흐름의 화살표 위에
 * 서는 단계다. 멈춘 단계의 색은 보드의 어휘 그대로: 결정·막힘은 기다림, 실패는 멈춤. */
const DESK_STAGES = Object.freeze([
  { id: "pending", flow: true, tone: "", key: "board.desk.stagePending", word: "선행 대기" },
  { id: "ready", flow: true, tone: "", key: "board.desk.stageReady", word: "준비" },
  { id: "dispatched", flow: true, tone: "", key: "board.desk.stageDispatched", word: "진행" },
  { id: "reported", flow: true, tone: "", key: "board.desk.stageReported", word: "보고됨" },
  { id: "merged", flow: true, tone: "", key: "board.desk.stageMerged", word: "병합" },
  { id: "gate", flow: false, tone: "wait", key: "board.desk.stageGate", word: "게이트" },
  { id: "blocked", flow: false, tone: "wait", key: "board.desk.stageBlocked", word: "막힘" },
  { id: "failed", flow: false, tone: "halt", key: "board.desk.stageFailed", word: "실패" },
]);

/* 처음 펼쳐 둘 단계: 멈춰 선 단계 가운데 과업이 있는 첫째. 없으면 아무것도. */
function deskDefaultStage(counts) {
  return DESK_STAGES.find((stage) => !stage.flow && (counts.get(stage.id) ?? 0) > 0)?.id ?? null;
}

function deskStageChip(host, stage, counts, view) {
  let chip = host.querySelector(`:scope > [data-stage="${stage.id}"]`);
  if (!chip) {
    chip = deskElement("button", "");
    chip.type = "button";
    chip.dataset.stage = stage.id;
    chip.append(deskElement("span", "board-desk-stage-word"), deskElement("strong", "board-desk-stage-count"));
    chip.onclick = () => {
      const counts = new Map((deskLedger?.stages ?? []).map((one) => [one.stage, one.count]));
      const open = deskChoice.stage === undefined ? deskDefaultStage(counts) : deskChoice.stage;
      deskChoice.stage = open === stage.id ? null : stage.id;
      paintCoordinatorDesk(view);
    };
  }
  const count = counts.get(stage.id) ?? 0;
  writeClassName(chip, `board-desk-stage${stage.flow ? " is-flow" : ""}${count > 0 && stage.tone ? ` is-${stage.tone}` : ""}`);
  writeTextContent(chip.firstElementChild, t(stage.key, stage.word));
  writeTextContent(chip.lastElementChild, String(count));
  return chip;
}

function deskTaskRow(held, task, runs) {
  const row = held ?? deskElement("li", "board-desk-task");
  if (!held) row.append(deskElement("code", "board-desk-task-id"), deskElement("span", "board-desk-task-title"),
    deskElement("span", "board-desk-task-note"));
  writeAttribute(row, "data-task", `${task.run}/${task.id}`);
  writeTextContent(row.firstElementChild, task.id);
  writeTextContent(row.children[1], runs > 1 ? `${task.title} · ${task.run}` : task.title);
  const note = task.gate
    ? t("board.desk.taskGate", "{{gate}} · {{question}}", { gate: task.gate.id, question: task.gate.question })
    : task.blocked_by?.length
      ? t("board.desk.taskBlockedBy", "실패한 선행: {{tasks}}", { tasks: task.blocked_by.join(", ") })
      : "";
  writeTextContent(row.lastElementChild, note);
  writeHidden(row.lastElementChild, note === "");
  return row;
}

function paintDeskPipeline(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.stages) || ledger.runs?.length === 0) return false;
  const counts = new Map(ledger.stages.map((one) => [one.stage, one.count]));
  const total = [...counts.values()].reduce((sum, count) => sum + count, 0);
  if (total === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  const body = block.lastElementChild;
  let strip = body.querySelector(":scope > .board-desk-stages");
  if (!strip) {
    strip = deskElement("div", "board-desk-stages");
    strip.setAttribute("role", "group");
    body.replaceChildren(strip, deskElement("ol", "board-desk-tasks"), deskElement("p", "board-desk-more"));
  }
  writeAttribute(strip, "aria-label", t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  let open = deskChoice.stage === undefined ? deskDefaultStage(counts) : deskChoice.stage;
  if (open && (counts.get(open) ?? 0) === 0) open = deskDefaultStage(counts);
  const chips = DESK_STAGES.map((stage) => {
    const chip = deskStageChip(strip, stage, counts, view);
    writeAttribute(chip, "aria-pressed", String(open === stage.id));
    return chip;
  });
  reconcileElementOrder(strip, chips);
  const list = body.querySelector(".board-desk-tasks");
  const rows = open ? (ledger.tasks ?? []).filter((task) => task.stage === open) : [];
  const held = new Map([...list.children].map((node) => [node.dataset.task, node]));
  const runs = ledger.runs?.length ?? 1;
  reconcileElementOrder(list, rows.map((task) => deskTaskRow(held.get(`${task.run}/${task.id}`), task, runs)));
  writeHidden(list, rows.length === 0);
  const more = body.querySelector(".board-desk-more");
  const hidden = open ? (counts.get(open) ?? 0) - rows.length : 0;
  writeTextContent(more, hidden > 0 ? t("board.desk.more", "{{count}}개 더 — 원장에 있음", { count: hidden }) : "");
  writeHidden(more, hidden <= 0);
  return true;
}

/* ---- 릴리즈 레인 ----------------------------------------------------------
 *
 * `tools/release/status.sh`가 읽던 그 파일(레인의 `status.json`, 상태 바의
 * `releaseLane`): sha·버전·단계·경과·판정, 그리고 건너뛴 단계의 사유. 판정은
 * 레인이 적은 `outcome` 그대로 — 끝나지 않은 레인은 진행 중이고, 여기서 초록을
 * 지어내지 않는다. */

/* 판정 셋의 낱말과 상태 표식. 다섯 상태의 표식을 빌린다 — 보드만의 색은 없다. */
const DESK_RELEASE_VERDICTS = Object.freeze({
  running: { state: "working", key: "board.desk.releaseRunning", word: "진행 중 · {{phase}}" },
  green: { state: "done", key: "board.desk.releaseGreen", word: "초록" },
  red: { state: "failed", key: "board.desk.releaseRed", word: "빨강" },
});

function deskReleaseVerdict(status) {
  if (status.outcome === "green") return "green";
  if (status.outcome === "red") return "red";
  return "running";
}

/* 한 단계의 표식: 건너뜀·실패·끝남. 지금 도는 단계는 목록에 아직 없다. */
function deskReleasePhaseState(phase) {
  if (phase.skipped) return "skipped";
  return phase.rc === 0 ? "done" : "failed";
}

function deskReleasePhaseWord(phase, state) {
  if (state === "skipped") return t("board.desk.phaseSkipped", "{{name}} 건너뜀", { name: phase.name });
  if (state === "running" || !Number.isFinite(phase.secs)) return phase.name;
  return t("board.desk.phaseSecs", "{{name}} {{secs}}초", { name: phase.name, secs: phase.secs });
}

function deskReleaseClock(status, verdict, now) {
  const started = Date.parse(status.started_at);
  if (!Number.isFinite(started)) return "";
  if (verdict === "running") {
    return runningWord(started, now) ?? t("board.desk.releaseJustStarted", "방금 시작");
  }
  const updated = Date.parse(status.updated_at);
  if (!Number.isFinite(updated)) return "";
  const took = usageDuration(Math.max(0, updated - started)) ?? t("board.desk.underMinute", "1분 미만");
  return t("board.desk.releaseTook", "{{took}} 걸림 · {{at}} 끝남", { took, at: knowledgeClock(updated, now) });
}

function paintDeskRelease(block, now) {
  const status = releaseLane;
  if (!status || typeof status !== "object" || !status.sha) return false;
  writeTextContent(block.firstElementChild, t("board.desk.release", "릴리즈 레인"));
  const body = block.lastElementChild;
  let card = body.firstElementChild;
  if (!card) {
    card = deskElement("div", "board-desk-release");
    const line = deskElement("p", "board-desk-release-line");
    line.append(agentGraphStateMark("working"), deskElement("strong", "board-desk-release-version"),
      deskElement("code", "board-desk-release-sha"), deskElement("span", "board-desk-release-verdict"),
      deskElement("span", "board-desk-release-clock"));
    card.append(line, deskElement("ol", "board-desk-release-phases"),
      deskElement("p", "board-desk-release-reason"), deskElement("ul", "board-desk-release-skips"));
    body.replaceChildren(card);
  }
  const verdict = deskReleaseVerdict(status);
  const said = DESK_RELEASE_VERDICTS[verdict];
  dressAgentGraphStateMark(card.querySelector(".agent-graph-node-state"), said.state);
  writeClassName(card, `board-desk-release is-${verdict}`);
  writeTextContent(card.querySelector(".board-desk-release-version"), String(status.version || ""));
  writeTextContent(card.querySelector(".board-desk-release-sha"), String(status.sha).slice(0, 8));
  writeTextContent(card.querySelector(".board-desk-release-verdict"),
    t(said.key, said.word, { phase: status.phase || "" }));
  writeTextContent(card.querySelector(".board-desk-release-clock"), deskReleaseClock(status, verdict, now));
  const phases = Array.isArray(status.phases) ? status.phases.filter((one) => one && one.name) : [];
  const running = verdict === "running" && status.phase && !phases.some((one) => one.name === status.phase)
    ? [{ name: String(status.phase), running: true }] : [];
  const list = card.querySelector(".board-desk-release-phases");
  const held = new Map([...list.children].map((node) => [node.dataset.phase, node]));
  reconcileElementOrder(list, [...phases, ...running].map((phase) => {
    const item = held.get(phase.name) ?? deskElement("li", "");
    writeAttribute(item, "data-phase", phase.name);
    const state = phase.running ? "running" : deskReleasePhaseState(phase);
    writeClassName(item, `board-desk-release-phase is-${state}`);
    writeTextContent(item, deskReleasePhaseWord(phase, state));
    return item;
  }));
  const reason = card.querySelector(".board-desk-release-reason");
  writeHidden(reason, verdict !== "red" || !status.reason);
  writeTextContent(reason, verdict === "red" ? String(status.reason || "") : "");
  const skips = card.querySelector(".board-desk-release-skips");
  const skipped = phases.filter((phase) => phase.skipped);
  writeHidden(skips, skipped.length === 0);
  const heldSkips = new Map([...skips.children].map((node) => [node.dataset.phase, node]));
  reconcileElementOrder(skips, skipped.map((phase) => {
    const item = heldSkips.get(phase.name) ?? deskElement("li", "");
    writeAttribute(item, "data-phase", phase.name);
    writeTextContent(item, t("board.desk.skipReason", "{{name}}: {{reason}}",
      { name: phase.name, reason: String(phase.skipped) }));
    return item;
  }));
  return true;
}

/* 데스크가 보이는 동안의 느린 박자. 상태 바의 박자와 같은 문을 부른다. */
const deskAmbient = idlePoller({
  wanted: () => deskViews().length > 0,
  every: DESK.ambientEveryMs,
  tick: refreshDeskAmbient,
  onResume: refreshDeskAmbient,
});
