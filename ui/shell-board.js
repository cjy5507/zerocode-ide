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

function refreshDeskAmbient() {
  deskAmbientAt = Date.now();
  void askReleaseStatus().then(scheduleDeskPaint);
}

function deskElement(tag, className, text = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/* 블록 하나: 장의 머리(낱말 · 수) 아래 몸. 한 번 짓고 키로 살아남는다. */
function deskBlock(desk, id) {
  let block = desk.querySelector(`:scope > [data-desk-block="${id}"]`);
  if (!block) {
    block = deskElement("section", "board-desk-block");
    block.dataset.deskBlock = id;
    block.append(deskElement("h3", "task-board-section-head board-desk-head"),
      deskElement("div", "board-desk-body"));
  }
  return block;
}

/* 데스크 블록, 읽는 차례대로 — DOM 순서가 곧 좁은 판의 순서다. 그리는 손은
 * 블록에 쓸 것이 있으면 참을 돌려주고, 거짓이면 그 블록은 접힌다. */
const DESK_BLOCKS = Object.freeze([
  { id: "release", paint: paintDeskRelease },
]);

function paintCoordinatorDesk(view) {
  const desk = view.querySelector(".task-board-desk");
  if (!desk) return;
  if (!deskPainted.has(view)) {
    desk.replaceChildren();
    deskPainted.add(view);
  }
  deskAmbient.sync();
  const now = Date.now();
  if (now - deskAmbientAt >= DESK.ambientEveryMs) refreshDeskAmbient();
  const blocks = DESK_BLOCKS.map(({ id, paint }) => {
    const block = deskBlock(desk, id);
    writeHidden(block, !paint(block, now, view));
    return block;
  });
  reconcileElementOrder(desk, blocks);
  writeHidden(desk, blocks.every((block) => block.hidden));
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
