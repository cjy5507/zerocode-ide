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
  /* 답할 우편이 접히기 전에 보이는 통 수. 나머지는 「N통 더 보기」 뒤에 선다 —
   * 우편이 스무 통이어도 작업 목록이 화면 밖으로 밀려나지 않게. */
  mailShown: 5,
});

/* 원장의 데스크 읽기(`board_desk`): 판 위의 런, 그 과업의 단계와 수. 1초
 * 박자가 게시한 것을 원장이 움직였다고 말할 때(`ledger:changed`)만 다시 읽는다.
 * `null`은 아직 읽지 않았거나 이 창에 원장이 없는 것이다. 워커 행은 사이드바와
 * 보드가 이미 나눠 읽는 그 답(`readLedgerAgents`)이다 — 같은 순간에 물으면 한
 * 번만 간다. */
let deskLedger = null;
let deskAgents = [];
/* 워커 체크아웃의 git 사실(`desk_checkouts`): 경로마다 앞선 커밋·바뀐 파일.
 * 느린 박자에 한 번 — git은 프로세스다. */
const deskCheckouts = new Map();
let deskCheckoutsAsking = false;
let deskLedgerSaid = "";
let deskLedgerAsking = false;
let deskLedgerAgain = false;

/* 사람이 고른 것: 목록을 펼친 단계, 우편을 다 펼쳤는가, 자세히를 펼친 워커와 오늘 끝난
 * 과업(t-70264). 판이 다시 그려져도 남는다 — 창의 기억이지 원장의 것이 아니다. */
const deskChoice = { stage: undefined, mailAll: false, workersOpen: new Set(), todayOpen: new Set() };

/* 쓰는 중인 답(편지 id마다)과 누른 확인(묶음 id마다). 초안은 판이 다시 그려져도
 * 남고, 보낸 요청의 이름(`retry`)은 답이 불확실하면 그대로 남아 다시 누르면 같은
 * 영수증을 되받는다 — 같은 뜻을 두 번 적지 않는다. */
const deskDrafts = new Map();
const deskAcks = new Map();

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
  Promise.all([invoke("board_desk"), readLedgerAgents()])
    .then(([answer, agents]) => {
      const said = JSON.stringify([answer ?? null, agents ?? []]);
      if (said === deskLedgerSaid) return;
      // The roster is the workers still summoned (`worker-list`): a row the
      // ledger keeps only for the finished work in its checkout (`settled`,
      // t-10993) is on the board as that work, not here as a worker.
      const summoned = Array.isArray(agents) ? agents.filter((row) => row?.settled !== true) : [];
      const fresh = deskAgents.length === 0 && summoned.length > 0;
      deskLedgerSaid = said;
      deskLedger = answer && typeof answer === "object" ? answer : null;
      deskAgents = summoned;
      if (fresh) askDeskCheckouts();
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

/* 보드가 작업 보기로 그려지기 **전에** 데스크의 원장 읽기를 한 번 띄운다. 보드의
 * 첫 그림은 제 두 물음(`pane_agents`·`board_snapshot`)을 기다리므로, 그 사이에 이
 * 답이 먼저 와 데스크가 작업 목록과 같은 그림에 선다 — 한 프레임 뒤에 따로 서지
 * 않는다. 이미 읽은 판은 원장 박자가 다시 부른다. */
function primeCoordinatorDesk() {
  if (deskLedgerSaid !== "" || deskLedgerAsking) return;
  refreshDeskLedger();
  if (Date.now() - deskAmbientAt >= DESK.ambientEveryMs) refreshDeskAmbient();
}

function refreshDeskAmbient() {
  deskAmbientAt = Date.now();
  // The gate's numbers (calls, spend) move while the ledger does not, and a ledger
  // that did not move tells the desk nothing — so the rows are read again on the
  // slow beat, unless a read is already out (a first paint asks for one itself).
  if (!deskLedgerAsking) refreshDeskLedger();
  // A lane that moved repaints the desk itself (`absorbReleaseStatus`).
  void askReleaseStatus();
  void askMachineLoad();
  void askDeskCheckouts();
  // The build line's next turn and the running lane's steps ahead — the 빌드 줄 card's foot and
  // list (t-70028) — are the schedule's reading, asked on this beat while 「지금」 stands.
  if (boardNowTab === "now") refreshBoardSchedule();
}

/* 워커 체크아웃마다 앞선 커밋과 바뀐 파일을 묻는다 — 판 위의 워커가 든 것만. */
function askDeskCheckouts() {
  const paths = [...new Set(deskAgents.map((row) => row.checkout).filter(Boolean))];
  if (deskCheckoutsAsking || paths.length === 0) return;
  deskCheckoutsAsking = true;
  invoke("desk_checkouts", { paths })
    .then((rows) => {
      deskCheckouts.clear();
      for (const row of Array.isArray(rows) ? rows : []) deskCheckouts.set(row.path, row);
      scheduleDeskPaint();
    })
    .catch(() => {})
    .finally(() => { deskCheckoutsAsking = false; });
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

/* 데스크 블록, 탭마다 읽는 차례대로 — DOM 순서가 곧 좁은 판의 순서다(t-67607, 시안 B).
 * 「지금」은 당신 차례·빌드 줄·통합 묶음·지금 도는 일, 「워커」는 워커의 표와 이 기계, 「기록」은
 * 오늘 끝난 일의 표·누적 수(시안 B, t-70264)와 소식·과업 흐름이다. 「일정」에는 데스크 블록이
 * 없다 — 그 탭의 블록은 일정의 그릇 하나다(`paintBoardSchedule`, t-67482). 그리는 손은 블록에
 * 쓸 것이 있으면 참을 돌려주고, 거짓이면 그 블록은 접힌다. 판에 선 탭의 블록만 그린다 — 다른
 * 탭은 고를 때 그린다(`chooseBoardTab`). */
const DESK_BLOCKS = Object.freeze([
  { id: "asks", tab: "now", paint: paintDeskAsks },
  { id: "release", tab: "now", paint: paintDeskRelease },
  { id: "batches", tab: "now", paint: paintDeskBatches },
  { id: "running", tab: "now", paint: paintDeskRunning },
  { id: "left", tab: "now", paint: paintDeskLeft, quietHead: true },
  { id: "workers", tab: "workers", paint: paintDeskWorkers },
  { id: "machine", tab: "workers", paint: paintDeskMachine, quietHead: true },
  { id: "today", tab: "history", paint: paintDeskToday },
  { id: "totals", tab: "history", paint: paintDeskTotals },
  { id: "mail", tab: "history", paint: paintDeskMail },
  { id: "pipeline", tab: "history", paint: paintDeskPipeline },
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
  paintBoardTabs(view);
  // The schedule tab's one block mounts here, once the tab is written — its one hook
  // (t-67482, shell-board-schedule.js).
  paintBoardSchedule(view);
  // Only the chosen tab's blocks stand: a block left behind on another tab is
  // taken out and drawn afresh when its tab is chosen, so the workers' rows
  // and the task flow do not ride along unseen under 「지금」.
  const blocks = [];
  for (const entry of DESK_BLOCKS) {
    if (entry.tab !== boardNowTab) {
      desk.querySelector(`:scope > [data-desk-block="${entry.id}"]`)?.remove();
      continue;
    }
    const block = deskBlock(desk, entry);
    writeAttribute(block, "data-board-tab-of", entry.tab);
    writeHidden(block, !entry.paint(block, now, view));
    blocks.push(block);
  }
  reconcileElementOrder(desk, blocks);
  writeHidden(desk, blocks.every((block) => block.hidden));
  paintBoardHead(view, deskOwed(deskLedger), now);
}

/* 맨 위 문장의 N — 사람이 답해야 움직이는 것의 수, 백엔드가 센 그대로: 답할 우편
 * (`counts.mail`)과 결정·검증 대기 단계의 과업 수(`stages`). 당신 차례 카드가 그리는
 * 행은 이 수가 세는 바로 그것들이다(`boardAsks`). */
function deskOwed(ledger) {
  if (!ledger) return 0;
  // 아무도 돌아오지 않는 일(남은 일, t-71220)은 누구의 차례도 아니다 — 백엔드가 단계마다 따로 센
  // 그 수(`left`)를 뺀다.
  const stages = new Map((ledger.stages ?? []).map((one) => [one.stage, (Number(one.count) || 0) - (Number(one.left) || 0)]));
  return (Number(ledger.counts?.mail) || 0) + (stages.get("gate") ?? 0) + (stages.get("reported") ?? 0);
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

/* ---- 답할 우편 --------------------------------------------------------------
 *
 * `check --peek`의 자리: 판 위의 런의 코디네이터가 빚진 편지(`desk_letters`). 두
 * 목록이다(t-9456). 「답할 우편」은 답을 기다리는 질문뿐이고 — 원장이 워커를
 * `went_quiet`에서 빼 주는 바로 그 판정(`Run::awaits_answer`) — 「소식」은 워커가
 * 멈췄다는 원장의 알림(한도 벽·끝남·조용해짐·서로 기다림·분류기 거절)과 소환 때
 * 정한 모델을 벗어났다는 기록(t-6747)이다. 조용해짐은 에피소드마다 한 줄이고 끝난
 * 침묵은 줄이 없으며, 하루 지난 소식은 접혀 개수만 선다 — 그 규칙과 세 숫자는 모두
 * 백엔드의 것이다(`DESK_NEWS`, `counts`). 이 화면은 편지를 다시 세지도, 고르지도
 * 않는다. 오래된 것이 먼저이고, 행마다 받은편지함의 상태(배달 전·받음·확인함)와
 * 나이를 적는다.
 *
 * 답하기와 확인은 원장의 뜻 그대로다(2026-09-24 코디네이터 지시). 질문에는 그
 * 자리에서 답을 적어 원장의 `reply`로 보낸다 — 이 창이 그 런의 코디네이터 자리를
 * 들고 있을 때만(`desk_reply`). 확인은 코디네이터가 이미 받아 둔 묶음(열린 배달)에
 * 그 알림이 있을 때만 그 묶음을 통째로 ack하고, 몇 통인지 누르기 전에 말한다
 * (`desk_ack`). 배달 전 알림에는 단추가 없다 — 코디네이터가 아직 읽지 않았고, 창이
 * 가로채면 코디네이터는 영영 못 받는다. 누른 뒤에도 행은 원장이 말할 때까지 그대로다:
 * 화면은 답도 ack도 지어내지 않는다. */

/* 편지 종류마다 낱말과 다섯 상태의 표식 — 사람이 필요한 것은 기다림, 끝난 워커는
 * 실패, 조용한 판은 유휴의 표식이다. 표식과 낱말뿐이다: 어느 편지가 답을 기다리고
 * 몇 통인지는 백엔드가 준 목록과 수(`counts`)가 말한다. */
const DESK_MAIL = Object.freeze({
  question: { state: "needs-attention", key: "board.desk.mailQuestion", word: "질문" },
  quota_walled: { state: "needs-attention", key: "board.desk.mailWalled", word: "한도 벽" },
  worker_died: { state: "failed", key: "board.desk.mailDied", word: "워커 끝남" },
  went_quiet: { state: "idle", key: "board.desk.mailQuiet", word: "조용해짐" },
  deadlocked: { state: "needs-attention", key: "board.desk.mailDeadlocked", word: "서로 기다림" },
  classifier_declined: { state: "needs-attention", key: "board.desk.mailDeclined", word: "분류기 거절" },
  model_deviated: { state: "needs-attention", key: "board.desk.mailDeviated", word: "모델 바뀜" },
  landing_stalled: { state: "needs-attention", key: "board.desk.mailLanding", word: "착지 점검" },
  land_check: { state: "needs-attention", key: "board.desk.mailLandCheck", word: "착지 전 점검" },
  // A turn its CLI said failed (t-69010): the worker waits on its coordinator, like one that ended.
  turn_failed: { state: "failed", key: "board.desk.mailTurnFailed", word: "턴 실패" },
});

/* 착지 전 점검의 판정 — 창이 준 증거의 낱말마다 한 줄(t-42447). 표에 없는 낱말은 합치기가 끝나지
 * 않은 것으로 읽는다: 통과로 읽지 않는다. */
const DESK_LAND_CHECK = Object.freeze({
  passed: { key: "board.desk.landCheckPassed", word: "점검 통과 (rc {{rc}})" },
  merged: { key: "board.desk.landCheckMerged", word: "충돌 없이 합쳐짐 — 점검 명령은 없음" },
  conflict: { key: "board.desk.landCheckConflict", word: "충돌 {{count}}개 — 합치면 겹쳐요" },
  failed: { key: "board.desk.landCheckFailed", word: "점검 실패 (rc {{rc}})" },
  timed_out: { key: "board.desk.landCheckTimedOut", word: "점검 시간 초과 — 프로세스를 멈췄어요" },
  unstartable: { key: "board.desk.landCheckUnstartable", word: "점검을 시작하지 못했어요" },
  abandoned: { key: "board.desk.landCheckAbandoned", word: "창이 끝나 점검이 중간에 끊겼어요" },
  error: { key: "board.desk.landCheckError", word: "합치기를 끝내지 못했어요" },
});

/* 늦은 착지의 다섯 까닭(t-22105)과 넘길 때 남긴 체크아웃(t-45336) — 원장의 낱말마다. 표에 없는 낱말은 원장의 말
 * 그대로 선다. */
const DESK_LANDING_REASONS = Object.freeze({
  no_review: { key: "board.desk.landingNoReview", word: "보고했는데 검증이 늦어요" },
  no_merge: { key: "board.desk.landingNoMerge", word: "검증했는데 병합 기록이 없어요" },
  merged_unrecorded: { key: "board.desk.landingUnrecorded", word: "git엔 main에 있는데 원장에 병합 기록이 없어요" },
  cleanable: { key: "board.desk.landingCleanable", word: "병합된 작업 폴더가 남아 있어요 (정리 가능)" },
  ledger_merged_git_not: { key: "board.desk.landingGitNot", word: "원장은 병합됐다는데 git엔 그 커밋이 main에 없어요" },
  checkouts_left: { key: "board.desk.landingCheckoutsLeft", word: "넘길 때 같은 과업의 체크아웃을 곁에 남겼어요" },
});

/* 분류기 거절 뒤에 무엇이 서 있는가(t-6747): 선언된 워커에게 넘기는 중, 넘길 곳을
 * 선언하지 않음, 또는 다른 모델로 가지 않는 분류라 거절이 그대로임. */
const DESK_DECLINE_STEPS = Object.freeze({
  handover: { key: "board.desk.declineHandover", word: "{{category}} · 선언된 워커에게 넘기는 중" },
  notify: { key: "board.desk.declineNotify", word: "{{category}} · 넘길 워커를 선언하지 않았어요" },
  stands: { key: "board.desk.declineStands", word: "{{category}} · 다른 모델로 가지 않는 분류라 거절이 그대로예요" },
});

/* 모델이 바뀐 동안 — CLI가 적은 범위마다. */
const DESK_SWITCH_SCOPES = Object.freeze({
  session: { key: "board.desk.switchSession", word: "{{category}} → {{model}} · 이 대화 끝까지" },
  local: { key: "board.desk.switchLocal", word: "{{category}} → {{model}} · 응답 하나만" },
});

function deskCategory(letter) {
  return letter.category || t("board.desk.declineNoCategory", "분류 없음");
}

/* 조용해진 까닭, 원장의 낱말마다. 표에 없는 낱말은 원장의 말 그대로 선다. */
const DESK_QUIET_REASONS = Object.freeze({
  stalled: { key: "board.desk.quietStalled", word: "보고 없이 턴이 멈춤" },
  quota_lifted: { key: "board.desk.quietQuotaLifted", word: "한도가 풀린 뒤에도 멈춰 있음" },
  pane_missing: { key: "board.desk.quietPaneMissing", word: "판이 보이지 않음" },
  never_spoke: { key: "board.desk.quietNeverSpoke", word: "한 번도 보고하지 않음" },
  judged: { key: "board.desk.quietJudged", word: "멈춘 까닭을 따로 판정함" },
  unread_mail: { key: "board.desk.quietUnreadMail", word: "안 읽은 편지를 둔 채 쉬고 있음" },
  waiting_on_mail: { key: "board.desk.quietWaitingOnMail", word: "제 check --wait에서 편지를 기다리며 쉬고 있음" },
});

/* 받은편지함의 세 상태: 행의 첫 줄에 서는 짧은 낱말과, 그 낱말의 팁이 되는 문장. */
const DESK_DELIVERY = Object.freeze({
  pending: {
    key: "board.desk.deliveryPendingShort", word: "배달 전",
    tip: { key: "board.desk.deliveryPending", word: "배달 전 · 코디네이터가 아직 안 읽음" },
  },
  delivered: {
    key: "board.desk.deliveryDeliveredShort", word: "받음 {{delivery}}",
    tip: { key: "board.desk.deliveryDelivered", word: "받음 · 묶음 {{delivery}}" },
  },
  acked: {
    key: "board.desk.deliveryAckedShort", word: "확인함",
    tip: { key: "board.desk.deliveryAcked", word: "받아서 확인함 · 아직 답 없음" },
  },
});

/* 편지의 둘째 줄: 질문은 제 말, 소식은 원장이 적은 사실. */
function deskLetterDetail(letter, now) {
  if (letter.kind === "question") return letter.body || "";
  if (letter.kind === "quota_walled") {
    return Number.isFinite(letter.resets_at_ms) ? usageCountdown(letter.resets_at_ms - now) : "";
  }
  if (letter.kind === "went_quiet") {
    const quiet = DESK_QUIET_REASONS[letter.reason];
    const reason = quiet ? t(quiet.key, quiet.word) : String(letter.reason ?? "");
    return letter.notices > 1
      ? t("board.desk.quietNotices", "{{reason}} · 알림 {{count}}통", { reason, count: letter.notices })
      : reason;
  }
  if (letter.kind === "landing_stalled") {
    const held = DESK_LANDING_REASONS[letter.reason];
    const reason = held ? t(held.key, held.word) : String(letter.reason ?? "");
    // No time when the ledger named none or it is under a minute — never a zero.
    const since = Number.isFinite(letter.since_ms) && now - letter.since_ms >= 60_000
      ? t("board.desk.landingSince", "{{time}}째", { time: agoWord(letter.since_ms, now) }) : "";
    return [reason, since].filter(Boolean).join(" · ");
  }
  if (letter.kind === "land_check") {
    const held = Object.hasOwn(DESK_LAND_CHECK, letter.verdict) ? DESK_LAND_CHECK[letter.verdict] : DESK_LAND_CHECK.error;
    return t(held.key, held.word, { count: letter.clashes ?? 0, rc: letter.rc ?? "?" });
  }
  if (letter.kind === "worker_died") return t("board.desk.mailDiedCopy", "보고 전에 판이 끝났어요");
  if (letter.kind === "deadlocked") return t("board.desk.mailDeadlockedCopy", "서로의 답을 기다리는 고리에 들었어요");
  if (letter.kind === "classifier_declined") {
    const step = DESK_DECLINE_STEPS[letter.routed === false ? "stands" : letter.rung] ?? DESK_DECLINE_STEPS.notify;
    return t(step.key, step.word, { category: deskCategory(letter) });
  }
  if (letter.kind === "model_deviated") {
    const scope = DESK_SWITCH_SCOPES[letter.scope] ?? DESK_SWITCH_SCOPES.session;
    return t(scope.key, scope.word, { category: deskCategory(letter), model: letter.switched_to ?? "" });
  }
  return "";
}

function deskLetterKey(letter) {
  return `${letter.run}/${letter.id}`;
}

/* 편지 한 통의 단추가 무엇을 하는가: 답하기, 묶음 확인, 또는 아무것도. */
function deskLetterAct(letter, seat) {
  if (isPopout || !seat) return null;
  if (letter.kind === "question") return "reply";
  return letter.delivery === "delivered" && letter.delivery_id ? "ack" : null;
}

/* 편지 한 통은 두 줄이다: 종류·누구·받은편지함 상태·나이, 그 아래 둘째 줄과 다음
 * 걸음의 단추. 보냄·확인함·실패처럼 누른 뒤의 말은 그때만 셋째 줄로 선다. */
function deskLetterRow(view) {
  const row = deskElement("li", "board-desk-letter");
  const line = deskElement("p", "board-desk-letter-line");
  line.append(agentGraphStateMark("needs-attention"), deskElement("strong", "board-desk-letter-kind"),
    deskElement("span", "board-desk-letter-who"), deskElement("span", "board-desk-letter-delivery"),
    deskElement("span", "board-desk-letter-age"));
  const lead = deskElement("p", "board-desk-letter-lead");
  const act = deskElement("button", "board-desk-letter-act");
  act.type = "button";
  act.onclick = () => {
    const letter = row.__letter;
    if (!letter) return;
    if (letter.kind === "question") {
      const draft = deskDrafts.get(letter.id) ?? { open: false, text: "", sending: false, sent: false, error: "", retry: null };
      draft.open = !draft.open;
      deskDrafts.set(letter.id, draft);
      paintCoordinatorDesk(view);
      if (draft.open) row.querySelector(".board-desk-reply-field")?.focus();
    } else void ackDeskBatch(view, letter);
  };
  lead.append(deskElement("span", "board-desk-letter-body"), act);
  row.append(line, lead, deskElement("p", "board-desk-letter-note"));
  return row;
}

/* 답을 쓰는 칸 — 처음 펼 때 한 번 짓고, 그 뒤로는 판이 다시 그려져도 같은 칸이다
 * (쓰는 중인 글자와 초점이 남는다). */
function deskReplyForm(row, view) {
  let form = row.querySelector(".board-desk-reply");
  if (form) return form;
  form = deskElement("form", "board-desk-reply");
  const field = deskElement("textarea", "board-desk-reply-field");
  field.rows = 3;
  field.oninput = () => {
    const draft = deskDrafts.get(row.__letter?.id);
    if (draft) draft.text = field.value;
  };
  field.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void sendDeskReply(view, row.__letter);
    }
  };
  const send = deskElement("button", "board-desk-reply-send");
  send.type = "submit";
  const cancel = deskElement("button", "board-desk-reply-cancel");
  cancel.type = "button";
  cancel.onclick = () => {
    const draft = deskDrafts.get(row.__letter?.id);
    if (draft) draft.open = false;
    paintCoordinatorDesk(view);
  };
  form.onsubmit = (event) => {
    event.preventDefault();
    void sendDeskReply(view, row.__letter);
  };
  const actions = deskElement("div", "board-desk-reply-actions");
  actions.append(deskElement("span", "board-desk-reply-error"), cancel, send);
  form.append(field, actions);
  row.append(form);
  return form;
}

async function sendDeskReply(view, letter) {
  const draft = letter && deskDrafts.get(letter.id);
  const body = draft?.text.trim() ?? "";
  if (!draft || draft.sending || body === "") return;
  const signature = JSON.stringify([letter.run, letter.id, body]);
  if (draft.retry?.signature !== signature) {
    draft.retry = { signature, id: `ui-desk-reply-${crypto.randomUUID()}` };
  }
  draft.sending = true;
  draft.error = "";
  paintCoordinatorDesk(view);
  try {
    await invoke("desk_reply", { run: letter.run, message: letter.id, body, retryRequest: draft.retry.id });
    draft.sent = true;
    draft.open = false;
    draft.retry = null;
    refreshDeskLedger();
  } catch (error) {
    // The name stays: pressing again replays the one receipt rather than
    // writing a second answer.
    draft.error = String(error);
  } finally {
    draft.sending = false;
    paintCoordinatorDesk(view);
  }
}

async function ackDeskBatch(view, letter) {
  const key = letter.delivery_id;
  const held = deskAcks.get(key) ?? { sending: false, sent: false, error: "", retry: null };
  if (held.sending) return;
  held.retry ??= { id: `ui-desk-ack-${crypto.randomUUID()}` };
  held.sending = true;
  held.error = "";
  deskAcks.set(key, held);
  paintCoordinatorDesk(view);
  try {
    await invoke("desk_ack", { run: letter.run, delivery: key, retryRequest: held.retry.id }, { hand: handNow() });
    held.sent = true;
    held.retry = null;
    refreshDeskLedger();
  } catch (error) {
    held.error = String(error);
  } finally {
    held.sending = false;
    paintCoordinatorDesk(view);
  }
}

/* `meta`는 당신 차례 카드가 그 편지의 첫 줄 대신 세우는 한 줄의 조각이다(`boardMetaPieces`,
 * t-70028) — 기록 탭의 소식은 워커와 과업 그대로다. */
function paintDeskLetter(row, letter, seat, now, view, meta = null) {
  row.__letter = letter;
  writeAttribute(row, "data-letter", deskLetterKey(letter));
  const kind = DESK_MAIL[letter.kind];
  writeClassName(row, `board-desk-letter is-${letter.kind} is-${letter.delivery}`);
  dressAgentGraphStateMark(row.querySelector(".agent-graph-node-state"), kind?.state ?? "idle");
  writeTextContent(row.querySelector(".board-desk-letter-kind"), kind ? t(kind.key, kind.word) : String(letter.kind));
  const whoLine = row.querySelector(".board-desk-letter-who");
  if (meta) paintBoardMeta(whoLine, meta);
  else writeTextContent(whoLine, [letter.worker, letter.task].filter(Boolean).join(" · "));
  writeTextContent(row.querySelector(".board-desk-letter-age"),
    t("board.desk.mailAge", "{{time}} 전", { time: agoWord(letter.created_ms, now) }));
  const detail = deskLetterDetail(letter, now);
  const body = row.querySelector(".board-desk-letter-body");
  writeTextContent(body, detail);
  writeHidden(body, detail === "");
  const draft = letter.kind === "question" ? deskDrafts.get(letter.id) : null;
  const ack = letter.delivery_id ? deskAcks.get(letter.delivery_id) : null;
  const delivery = DESK_DELIVERY[letter.delivery] ?? DESK_DELIVERY.pending;
  const where = row.querySelector(".board-desk-letter-delivery");
  writeTextContent(where, t(delivery.key, delivery.word, { delivery: letter.delivery_id ?? "" }));
  writeAttribute(where, "data-tip", t(delivery.tip.key, delivery.tip.word, { delivery: letter.delivery_id ?? "" }));
  const note = row.querySelector(".board-desk-letter-note");
  const said = ack?.error || (draft?.sent
    ? t("board.desk.replySent", "답을 보냄 · 원장에 적히면 목록에서 빠져요")
    : ack?.sent ? t("board.desk.ackSent", "묶음을 확인함 · 원장에 적히면 목록에서 빠져요") : "");
  writeTextContent(note, said);
  writeHidden(note, said === "");
  const act = row.querySelector(".board-desk-letter-act");
  const does = deskLetterAct(letter, seat);
  writeHidden(act, does === null || Boolean(draft?.sent) || Boolean(ack?.sent));
  writeDisabled(act, Boolean(draft?.sending || ack?.sending));
  writeTextContent(act, does === "reply"
    ? t("board.desk.reply", "답하기")
    : ack?.sending
      ? t("board.desk.acking", "확인하는 중…")
      : t("board.desk.ack", "확인 · 이 묶음 {{count}}통", { count: letter.batch ?? 0 }));
  writeAttribute(act, "aria-expanded", String(does === "reply" && Boolean(draft?.open)));
  const open = does === "reply" && Boolean(draft?.open);
  const form = open ? deskReplyForm(row, view) : row.querySelector(".board-desk-reply");
  if (form) {
    writeHidden(form, !open);
    const field = form.querySelector(".board-desk-reply-field");
    writeAttribute(field, "placeholder", t("board.desk.replyPlaceholder", "코디네이터로서 답을 적어요 — 원장의 reply로 워커에게 갑니다"));
    writeAttribute(field, "aria-label", t("board.desk.reply", "답하기"));
    if (draft && field.value !== draft.text && document.activeElement !== field) field.value = draft.text;
    writeDisabled(field, Boolean(draft?.sending));
    const send = form.querySelector(".board-desk-reply-send");
    writeDisabled(send, Boolean(draft?.sending));
    writeTextContent(send, draft?.sending ? t("board.desk.replySending", "보내는 중…") : t("board.desk.replySend", "답 보내기"));
    writeTextContent(form.querySelector(".board-desk-reply-cancel"), t("board.desk.replyCancel", "취소"));
    const error = form.querySelector(".board-desk-reply-error");
    writeTextContent(error, draft?.error ?? "");
    writeHidden(error, !draft?.error);
  }
}

/* 편지 목록 하나를 그린다. */
function paintDeskLetterList(list, letters, seats, now, view) {
  const held = new Map([...list.children].map((node) => [node.dataset.letter, node]));
  reconcileElementOrder(list, letters.map((letter) => {
    const row = held.get(deskLetterKey(letter)) ?? deskLetterRow(view);
    paintDeskLetter(row, letter, seats.get(letter.run) ?? false, now, view);
    return row;
  }));
  writeHidden(list, letters.length === 0);
}

/* 소식 목록(t-67607 뒤): 답할 우편은 「지금」 탭의 당신 차례 카드에 서고(`paintDeskAsks`),
 * 이 블록은 소식만 든다 — 처음 `DESK.mailShown`통, 그 뒤는 「N통 더 보기」 하나다. 세
 * 숫자(답할 우편·소식·접힘)는 백엔드의 `counts` 그대로다. */
function paintDeskMail(block, now, view) {
  const letters = Array.isArray(deskLedger?.mail) ? deskLedger.mail : [];
  const news = Array.isArray(deskLedger?.news) ? deskLedger.news : [];
  const counts = deskLedger?.counts ?? {};
  const owed = Number(counts.mail) || 0;
  const told = Number(counts.news) || 0;
  const folded = Number(counts.folded) || 0;
  if (told + folded === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.news", "소식 · {{count}}", { count: told }));
  const body = block.lastElementChild;
  let list = body.querySelector(":scope > .board-desk-letters.is-news");
  if (!list) {
    list = deskElement("ol", "board-desk-letters is-news");
    const more = deskElement("button", "board-desk-letters-more");
    more.type = "button";
    more.onclick = () => {
      deskChoice.mailAll = !deskChoice.mailAll;
      paintCoordinatorDesk(view);
    };
    const foldedLine = deskElement("p", "board-desk-news-folded");
    foldedLine.append(deskElement("span", "board-desk-news-folded-word"));
    body.replaceChildren(deskElement("p", "board-desk-unseated"), deskElement("p", "board-desk-news-owed"), list,
      more, foldedLine);
  }
  const [unseated, owedLine, , more, foldedLine] = body.children;
  const seats = new Map((deskLedger.runs ?? []).map((run) => [run.run, run.seat === true]));
  const shownNews = news.slice(0, deskChoice.mailAll ? Infinity : DESK.mailShown);
  paintDeskLetterList(list, shownNews, seats, now, view);
  const strangers = !isPopout && shownNews.some((letter) => !(seats.get(letter.run) ?? false));
  writeTextContent(unseated, strangers
    ? t("board.desk.noSeat", "이 창에 그 런의 코디네이터 자리가 없어 여기서는 답할 수 없어요") : "");
  writeHidden(unseated, !strangers);
  writeTextContent(owedLine, t("board.desk.newsOwed", "답할 우편 {{count}}통은 「지금」의 당신 차례에 있습니다",
    { count: owed }));
  writeHidden(owedLine, owed === 0);
  const rest = told - DESK.mailShown;
  writeHidden(more, rest <= 0);
  writeTextContent(more, deskChoice.mailAll ? t("board.desk.mailFewer", "접기")
    : t("board.desk.mailMore", "{{count}}통 더 보기", { count: Math.max(0, rest) }));
  writeAttribute(more, "aria-expanded", String(deskChoice.mailAll));
  writeTextContent(foldedLine.firstElementChild, t("board.desk.newsFolded", "접힌 소식 {{count}}통 · 하루 지났거나 끝난 침묵",
    { count: folded }));
  const foldedBatches = Array.isArray(deskLedger?.folded_batches) ? deskLedger.folded_batches : [];
  paintDeskFoldedAcks(foldedLine, foldedBatches, seats, view);
  writeHidden(foldedLine, folded === 0);
  // Batches that left the ledger's lists take their presses with them.
  const batches = new Set([...letters, ...news, ...foldedBatches].map((letter) => letter.delivery_id).filter(Boolean));
  for (const id of deskAcks.keys()) if (!batches.has(id)) deskAcks.delete(id);
  return true;
}

/* 접힌 소식 옆의 확인 (t-9548): 코디네이터가 받아 둔 묶음의 소식이 전부 접혀 그 묶음을
 * 내미는 줄이 없을 때, 접힌 수 옆에서 그 묶음을 통째로 확인한다. 어느 묶음인지는 백엔드가
 * 고르고(`DESK_NEWS.folded_ack` — 받아 둔 묶음만, 이미 확인한 것은 빚이 아니다), 누를 수
 * 있는가는 줄의 단추와 같은 규칙(`deskLetterAct`), 누르는 손도 같다(`ackDeskBatch`). */
function paintDeskFoldedAcks(line, batches, seats, view) {
  const held = new Map([...line.querySelectorAll(":scope > .board-desk-news-folded-ack")]
    .map((node) => [node.dataset.batch, node]));
  const offered = [];
  for (const batch of batches) {
    const letter = { run: batch.run, delivery: "delivered", delivery_id: batch.delivery_id, batch: batch.batch };
    if (deskLetterAct(letter, seats.get(batch.run) ?? false) !== "ack") continue;
    const key = `${batch.run}/${batch.delivery_id}`;
    let act = held.get(key);
    if (!act) {
      act = deskElement("button", "board-desk-letter-act board-desk-news-folded-ack");
      act.type = "button";
      act.dataset.batch = key;
      act.onclick = () => { if (act.__letter) void ackDeskBatch(view, act.__letter); };
    }
    act.__letter = letter;
    const ack = deskAcks.get(batch.delivery_id);
    writeDisabled(act, Boolean(ack?.sending));
    writeHidden(act, Boolean(ack?.sent));
    writeTextContent(act, ack?.sending ? t("board.desk.acking", "확인하는 중…")
      : t("board.desk.ack", "확인 · 이 묶음 {{count}}통", { count: batch.batch ?? 0 }));
    offered.push(act);
  }
  reconcileElementOrder(line, [line.firstElementChild, ...offered]);
}

/* ---- 워커 ------------------------------------------------------------------
 *
 * `worker-list`의 자리: 원장이 아직 부르고 있는 워커마다 한 줄 — 건강 한 낱말,
 * 마지막 활동의 나이, 에이전트·모델·판·체크아웃, 그 체크아웃의 앞선 커밋과 바뀐
 * 파일. 건강의 사실은 원장의 것이다(판 없음은 조정자의 증명, 한도 벽은 원장이 다시
 * 읽은 벽, 답 기다림은 원장의 `awaiting_reply`, 잠듦은 원장의 낱말). 턴 중과
 * 유휴는 판의 훅이 말한 상태다. 이 표는 그 사실들에 붙는 낱말과 차례일 뿐이다 —
 * 위의 것이 먼저 맞으면 아래는 묻지 않는다. `dot`은 시안 B의 상태 점이다(t-70264):
 * 멈춘 것은 빨강, 사람의 답을 기다리는 것은 파랑, 턴 중은 호박, 쉬는 것은 회색. */
const DESK_HEALTH = Object.freeze([
  { id: "gone", state: "failed", dot: "halt", key: "board.desk.healthGone", word: "판 없음",
    holds: (row) => Number.isFinite(row.pane_missing_since_ms) },
  { id: "walled", state: "needs-attention", dot: "halt", key: "board.desk.healthWalled", word: "한도 벽",
    holds: (row, card, now) => Boolean(row.wall) && now < row.wall.stands_until_ms },
  { id: "asking", state: "needs-attention", dot: "you", key: "board.desk.healthAsking", word: "답 기다림",
    holds: (row, card) => row.asking === true || card?.state === "needs-attention" },
  { id: "asleep", state: "idle", dot: "queue", key: "board.desk.healthAsleep", word: "잠듦",
    holds: (row) => row.ledger === "sleeping" },
  { id: "turn", state: "working", dot: "run", key: "board.desk.healthTurn", word: "턴 중",
    holds: (row, card) => card?.state === "working" },
  { id: "idle", state: "idle", dot: "queue", key: "board.desk.healthIdle", word: "유휴", holds: () => true },
]);

function deskWorkerHealth(row, card, now) {
  return DESK_HEALTH.find((health) => health.holds(row, card, now));
}

/* 마지막 활동: 판의 훅이 말한 때, 없으면 원장이 들은 가장 늦은 때. */
function deskWorkerAt(row, card) {
  return Math.max(Number(card?.at) || 0, Number(row.quiet_at) || 0, Number(row.hearing_at) || 0,
    Number(row.dispatch_started_ms) || 0, Number(row.at) || 0);
}

function deskWorkerFacts(row, now) {
  const model = (row.term != null ? paneModels.get(row.term) : null) ?? row.model ?? "";
  const git = deskCheckouts.get(row.checkout);
  return [
    agentName(row.agent),
    model && row.effort ? t("board.desk.workerModel", "{{model}} · {{effort}}", { model, effort: row.effort }) : model,
    row.term != null ? t("board.desk.workerTerm", "판 {{term}}", { term: row.term }) : row.pane,
    row.checkout ? basename(row.checkout) : "",
    Number.isInteger(git?.beyond_base) ? t("board.desk.workerAhead", "커밋 {{count}}개 앞섬", { count: git.beyond_base }) : "",
    Number.isInteger(git?.dirty_files) ? t("board.desk.workerDirty", "바뀐 파일 {{count}}", { count: git.dirty_files }) : "",
  ].filter(Boolean).join(" · ");
}

/* ---- 계속 판정 (t-26583) -----------------------------------------------------
 *
 * 워커 줄 밑의 한 줄: 창의 계속 판정이 그 워커를 어떻게 보는가와 그 근거. 판정·사유·숫자는
 * 백엔드의 것이고(`continue_gate`, 행의 `gate`로 온다) 여기는 낱말과 서식만 붙인다 — 같은
 * 사실을 두 곳에서 세면 두 답이 된다. 근거 줄은 판정이 「계속」일 때도 그린다: 말없이 두는 것도
 * 읽은 숫자 위의 판정이라 무엇을 읽었는지 보여야 믿을 수 있다. 비용을 못 읽는 에이전트는 0원이
 * 아니라 「비용 모름」과 그 까닭으로 적는다. 아래 표의 이름은 백엔드가 직렬화하는 그대로이고
 * (`Verdict::word`·`Code::word`·`CostNote`), 모든 이름에 낱말이 있는지는 소스 계약이 지킨다. */
const GATE_VERDICTS = Object.freeze({
  continue: { key: "board.desk.gate.continue", word: "계속" },
  checkpoint: { key: "board.desk.gate.checkpoint", word: "체크포인트" },
  pause: { key: "board.desk.gate.pause", word: "멈춤" },
  stop: { key: "board.desk.gate.stop", word: "중지" },
});

/* 사유의 `value`·`limit`은 사유마다 단위가 다르다(`Code`의 문서): 횟수, 천분율(되풀이)과 천분율로
 * 적은 배수(비용 상승), 달러. `show`가 그 단위를 읽는 말로 바꾼다. 한국어 문장은 `key:`와 같은 줄에 둔다 —
 * 「창에 박힌 라벨」 검사(`no_label_reaches_the_window_hardcoded`)가 줄 단위로 읽는다. */
const gateCount = (value) => String(Math.round(value));
const gatePercent = (permille) => String(Math.round(permille / 10));
const gateTimes = (permille) => (permille / 1000).toFixed(1);
const GATE_REASONS = Object.freeze({
  checkpoint_due: { show: gateCount,
    key: "board.desk.gate.reasonCheckpoint", word: "마지막 체크포인트 뒤 호출 {{value}}번 (간격 {{limit}}번)" },
  cost_rising: { show: gateTimes,
    key: "board.desk.gate.reasonRising", word: "호출당 비용이 직전의 {{value}}배 (기준 {{limit}}배)" },
  rework_loop: { show: gatePercent,
    key: "board.desk.gate.reasonRework", word: "되풀이·실패 {{value}}% (한도 {{limit}}%)" },
  task_budget_near: { show: formatCost,
    key: "board.desk.gate.reasonTaskNear", word: "과업 예산 {{limit}} 중 {{value}} 씀" },
  day_budget_near: { show: formatCost,
    key: "board.desk.gate.reasonDayNear", word: "오늘 예산 {{limit}} 중 {{value}} 씀" },
  task_budget_stop: { show: formatCost,
    key: "board.desk.gate.reasonTaskStop", word: "과업 예산 {{limit}} 초과 예상 — {{value}}" },
  day_budget_stop: { show: formatCost,
    key: "board.desk.gate.reasonDayStop", word: "오늘 예산 {{limit}} 초과 예상 — {{value}}" },
});

/* 비용이 숫자가 아닌 까닭, 창의 미터가 말하는 그대로(`CostNote`). `read`만 숫자다. */
const GATE_COST_NOTES = Object.freeze({
  no_reader: { key: "board.desk.gate.costNoReader", word: "이 CLI의 기록 형식은 아직 못 읽음" },
  no_transcript: { key: "board.desk.gate.costNoTranscript", word: "이 CLI가 대화 기록 위치를 알려 주지 않음" },
  unreadable: { key: "board.desk.gate.costUnreadable", word: "대화 기록을 열 수 없음" },
});

/* 새 백엔드가 준 사유를 이 화면이 아직 모르면 그 이름 그대로 둔다 — 지어낸 문장보다 낫다. */
function gateReasonWords(reason) {
  const held = GATE_REASONS[reason.code];
  if (!held) return String(reason.code);
  return t(held.key, held.word, { value: held.show(reason.value), limit: held.show(reason.limit) });
}

/* 이 과업이 쓴 비용 — 가격표 밖의 호출이 있으면 「+」가 붙는다. 근거 줄과 워커 표의 비용 칸이 같은 말이다. */
function gateSpentWords(gate) {
  return formatCost(gate.task_spent_usd) + (gate.metrics?.unpriced_calls > 0 ? "+" : "");
}

/* 비용이 숫자가 아닌 까닭 한 마디(`CostNote`) — 근거 줄의 문장과 워커 표의 비용 칸의 팁. */
function gateCostNoteWords(gate) {
  const note = GATE_COST_NOTES[gate.cost];
  return note ? t(note.key, note.word) : String(gate.cost);
}

/* 읽은 숫자: 호출 수, 되풀이 비율, 과업이 쓴 비용과 예산, 하루 합계와 예산, 저장한 체크포인트. */
function gateBasisWords(gate) {
  const metrics = gate.metrics ?? {};
  const known = Number.isFinite;
  const spent = gateSpentWords(gate);
  let cost;
  if (gate.cost !== "read") {
    cost = t("board.desk.gate.costUnknown", "비용 모름 — {{why}}", { why: gateCostNoteWords(gate) });
  } else if (known(gate.task_limit_usd)) {
    cost = t("board.desk.gate.basisTaskOf", "이 과업 {{spent}} / 예산 {{limit}}",
      { spent, limit: formatCost(gate.task_limit_usd) });
  } else {
    cost = t("board.desk.gate.basisTask", "이 과업 {{spent}}", { spent });
  }
  return [
    t("board.desk.gate.basisCalls", "호출 {{count}}번", { count: metrics.steps ?? 0 }),
    known(metrics.rework_permille)
      ? t("board.desk.gate.basisRework", "되풀이 {{percent}}%", { percent: gatePercent(metrics.rework_permille) }) : "",
    cost,
    known(gate.day_limit_usd)
      ? t("board.desk.gate.basisDay", "오늘 {{spent}} / 예산 {{limit}}",
        { spent: formatCost(gate.day_spent_usd), limit: formatCost(gate.day_limit_usd) }) : "",
    metrics.checkpoints > 0
      ? t("board.desk.gate.basisCheckpoints", "체크포인트 {{count}}회", { count: metrics.checkpoints }) : "",
  ].filter(Boolean).join(" · ");
}

/* 줄에 다 안 들어가는 것: 모드가 무엇을 하는가, 가격표 밖 호출, 저장한 복원점이나 못 만든 까닭. */
function gateTipWords(gate) {
  const saved = gate.snapshot;
  const snapshot = saved?.reference
    ? t("board.desk.gate.tipSnapshot", "복원점 {{ref}}", { ref: saved.reference })
    : saved?.error
      ? t("board.desk.gate.tipSnapshotFailed", "복원점을 못 만듦 — {{error}}", { error: saved.error })
      : saved ? t("board.desk.gate.tipSnapshotClean", "복원점: 저장할 변경 없음") : "";
  return [
    gate.mode === "stop"
      ? t("board.desk.gate.tipModeStop", "예산을 넘기 전에 중지하는 모드")
      : t("board.desk.gate.tipModeNotify", "알리기만 하는 모드 — 워커를 끝내지 않음"),
    gate.metrics?.unpriced_calls > 0
      ? t("board.desk.gate.tipUnpriced", "가격표에 없는 호출 {{count}}번은 비용에서 빠짐", { count: gate.metrics.unpriced_calls }) : "",
    snapshot,
  ].filter(Boolean).join(" · ");
}

function deskGateWords(gate) {
  const verdict = GATE_VERDICTS[gate.verdict];
  return {
    verdict: verdict ? t(verdict.key, verdict.word) : String(gate.verdict),
    why: (gate.reasons ?? []).map(gateReasonWords).join(" · "),
    basis: gateBasisWords(gate),
    tip: gateTipWords(gate),
  };
}

function paintDeskGate(line, gate) {
  writeHidden(line, !gate);
  if (!gate) return;
  const words = deskGateWords(gate);
  writeAttribute(line, "data-verdict", gate.verdict);
  writeAttribute(line, "title", words.tip);
  writeTextContent(line.querySelector(".board-desk-worker-gate-verdict"), words.verdict);
  writeTextContent(line.querySelector(".board-desk-worker-gate-why"), words.why);
  writeTextContent(line.querySelector(".board-desk-worker-gate-basis"), words.basis);
}

/* ---- 표 (t-70264) ------------------------------------------------------------
 *
 * 워커 탭과 기록 탭의 표 — 시안 B의 `g-table`: 머리 줄의 칸 이름, 행마다 칸 하나씩. 칸의 정의
 * (`columns`)가 머리 줄과 행의 칸을 함께 짓는다 — `number`는 오른쪽에 붙는 수의 칸, `wide`는 좁은
 * 판(1012 미만)에서 물러나는 칸이다(시안이 좁을 때 지우는 그것들, shell.css). 행 하나는 행 묶음
 * (`tbody`) 하나다: 칸들의 행과, 행을 누르면 그 밑에 펼쳐지는 자세히. */
function deskTableCell(tag, column, className = "") {
  return deskElement(tag, [className, `is-${column.id}`, column.number ? "is-number" : "", column.wide ? "is-wide" : ""]
    .filter(Boolean).join(" "));
}

function deskTable(className, columns) {
  const table = deskElement("table", `board-table ${className}`);
  const line = table.createTHead().insertRow();
  for (const column of columns) {
    const head = deskTableCell("th", column);
    head.scope = "col";
    line.append(head);
  }
  return table;
}

/* 표가 서는 블록의 몸: 시안의 제목 밑 한 줄(`board-desk-block-note`)과 표를 든 카드. 한 번 짓고,
 * 머리 줄의 낱말은 그릴 때마다 창의 언어로 쓴다(같은 낱말이면 쓰지 않는다). */
function deskTableBlock(block, className, columns) {
  const body = block.lastElementChild;
  let table = body.querySelector(`:scope > .board-table-card > .${className}`);
  if (!table) {
    table = deskTable(className, columns);
    const card = deskElement("div", "board-table-card");
    card.append(table);
    body.replaceChildren(deskElement("p", "board-desk-block-note"), card);
  }
  const heads = table.tHead.rows[0].cells;
  columns.forEach((column, at) => writeTextContent(heads[at], t(column.key, column.word)));
  return table;
}

/* 행을 누르면 자세히가 펼쳐지고 다시 누르면 접힌다. 행의 어디를 눌러도 되고, 키보드는 첫 칸의
 * 단추(`fold`)로 닿는다 — 행 안의 다른 단추·선택 칸은 제 일을 한다. 펼친 행은 창의 기억이다
 * (`opened`, 판이 다시 그려져도 남는다). */
function deskTableFold(view, group, row, fold, opened, key) {
  const toggle = () => {
    const at = group.dataset[key];
    if (opened.has(at)) opened.delete(at);
    else opened.add(at);
    paintCoordinatorDesk(view);
  };
  fold.type = "button";
  fold.insertAdjacentHTML("afterbegin", icon("chevron", false));
  fold.onclick = toggle;
  row.onclick = (event) => {
    if (!event.target.closest("button, select, a, summary")) toggle();
  };
}

/* 자세히의 행: 표의 폭 전체를 쓰는 칸 하나. 처음엔 접혀 있고 비어 있다 — 그 안은 행이 처음 펼쳐질 때
 * 짓는다(스무 명의 표가 아무도 펼치지 않은 스무 벌의 줄을 들고 다니지 않게). */
function deskTableMore(className, columns) {
  const more = deskElement("tr", className);
  more.hidden = true;
  const cell = deskElement("td", "");
  cell.colSpan = columns.length;
  more.append(cell);
  return more;
}

/* 펼친 행의 자세히 그릇 — 처음이면 `build()`로 짓는다. */
function deskTableDetail(group, build) {
  const cell = group.lastElementChild.firstElementChild;
  return cell.firstElementChild ?? cell.appendChild(build());
}

/* 펼침의 상태를 행에: 단추의 aria-expanded, 꺾쇠, 자세히의 행. */
function paintDeskTableFold(group, open) {
  const fold = group.querySelector(":scope > tr > td > button");
  writeAttribute(fold, "aria-expanded", String(open));
  fold.querySelector(".icon")?.classList.toggle("is-open", open);
  writeHidden(group.lastElementChild, !open);
}

/* 워커 표의 여덟 칸, 시안 B의 차례 그대로(t-70264): 워커 · 모델, 판, 과업, 상태, 경과, 비용, 호출,
 * 바뀐 줄. 비용과 호출은 계속 판정이 읽은 그 수(`row.gate`), 바뀐 줄은 체크아웃이 갈라져 나온 뒤의
 * 줄(`desk_checkouts`의 `lines`)이다 — 모르면 「—」이고 0이라 쓰지 않는다. */
const DESK_WORKER_COLUMNS = Object.freeze([
  { id: "who", key: "board.workers.colWho", word: "워커 · 모델" },
  { id: "pane", key: "board.workers.colPane", word: "판", wide: true },
  { id: "task", key: "board.workers.colTask", word: "과업", wide: true },
  { id: "state", key: "board.workers.colState", word: "상태" },
  { id: "took", key: "board.workers.colTook", word: "경과", number: true, wide: true },
  { id: "cost", key: "board.workers.colCost", word: "비용", number: true, wide: true },
  { id: "calls", key: "board.workers.colCalls", word: "호출", number: true, wide: true },
  { id: "lines", key: "board.workers.colLines", word: "바뀐 줄", number: true, wide: true },
]);

/* 바뀐 줄 — 「+1,204 −0」: 더한 줄과 뺀 줄, 창의 언어로 자릿수를 끊는다. */
function deskLinesWords(lines) {
  const tongue = document.documentElement.lang || undefined;
  return `+${(Number(lines.added) || 0).toLocaleString(tongue)} −${(Number(lines.removed) || 0).toLocaleString(tongue)}`;
}

/* 워커 한 명은 행 묶음 하나다: 칸 여덟의 행, 한도 벽의 띠(벽이 서 있을 때만 — 사람이 누를 단추라
 * 접어 두지 않는다), 그리고 행을 누르면 펼쳐지는 자세히 — 에이전트·모델·판·체크아웃·앞선 커밋·바뀐
 * 파일, 마지막 활동, 계속 판정과 그 근거, 남긴 것, 그 작업을 인스펙터에 여는 단추. 띠와 자세히는 행
 * 밖의 제 줄이다 — 단추 안의 단추는 없다. */
function deskWorkerRow(view) {
  const group = deskElement("tbody", "board-desk-worker");
  const row = deskElement("tr", "board-desk-worker-row");
  const fold = deskElement("button", "board-desk-worker-fold");
  // The space is for whoever reads the button aloud or copies it; the flex gap draws the same space.
  fold.append(deskElement("strong", "board-desk-worker-model"), document.createTextNode(" "),
    deskElement("code", "board-desk-worker-id"));
  deskTableFold(view, group, row, fold, deskChoice.workersOpen, "worker");
  const [who, pane, task, state, took, cost, calls, lines] = DESK_WORKER_COLUMNS;
  const cell = (column, className, ...parts) => {
    const one = deskTableCell("td", column, className);
    one.append(...parts);
    return one;
  };
  row.append(cell(who, "board-desk-worker-who", fold), cell(pane, "board-desk-worker-pane"),
    cell(task, "board-desk-worker-task"),
    cell(state, "board-desk-worker-state", deskElement("span", "board-desk-worker-health board-state"),
      deskElement("span", "board-desk-worker-verdict")),
    cell(took, "board-desk-worker-took"), cell(cost, "board-desk-worker-cost"),
    cell(calls, "board-desk-worker-calls"), cell(lines, "board-desk-worker-lines"));
  // The details, built the first time the row opens (`deskTableDetail`).
  group.__detail = () => {
    const heard = deskElement("p", "board-desk-worker-heard");
    heard.append(deskElement("span", "board-desk-worker-heard-word"), document.createTextNode(" "),
      deskElement("span", "board-desk-worker-age"));
    const gate = deskElement("p", "board-desk-worker-gate");
    gate.hidden = true;
    gate.append(deskElement("strong", "board-desk-worker-gate-verdict"),
      deskElement("span", "board-desk-worker-gate-why"), deskElement("span", "board-desk-worker-gate-basis"));
    const open = deskElement("button", "board-btn board-desk-worker-main");
    open.type = "button";
    open.onclick = () => {
      const term = group.__term;
      if (term != null) selectTaskBoardMember(view, `agent:term:${term}`);
    };
    const detail = deskElement("div", "board-desk-worker-detail");
    detail.append(deskElement("p", "board-desk-worker-facts"), heard, gate, deskKeptLine("board-desk-worker-kept", false),
      open);
    return detail;
  };
  group.append(row, deskWallStrip(view, group), deskTurnStrip(view, group),
    deskTableMore("board-desk-worker-more", DESK_WORKER_COLUMNS));
  return group;
}

function paintDeskWorkers(block, now, view) {
  const rows = deskAgents;
  if (rows.length === 0) return false;
  writeTextContent(block.firstElementChild, t("board.workers.title", "워커 {{count}}", { count: rows.length }));
  const table = deskTableBlock(block, "board-desk-workers", DESK_WORKER_COLUMNS);
  writeTextContent(table.parentElement.previousElementSibling,
    t("board.workers.note", "모델·비용·호출 수·바뀐 줄은 여기서만 봅니다 — 첫 화면에서는 뺐습니다."));
  const model = agentGraphModels.get(view);
  const cards = new Map((model?.agents ?? []).map((entry) => [entry.card.pane, entry.card]));
  const judged = rows.map((row) => {
    const card = row.term != null ? cards.get(`term:${row.term}`) : null;
    return { row, card, health: deskWorkerHealth(row, card, now) };
  }).sort((a, b) => DESK_HEALTH.indexOf(a.health) - DESK_HEALTH.indexOf(b.health) ||
    (Number(a.row.at) || 0) - (Number(b.row.at) || 0));
  const held = new Map([...table.tBodies].map((node) => [node.dataset.worker, node]));
  const keys = new Set();
  reconcileElementOrder(table, [table.tHead, ...judged.map(({ row, card, health }) => {
    const key = `${row.run}/${row.worker}`;
    keys.add(key);
    const node = held.get(key) ?? deskWorkerRow(view);
    node.__term = card ? row.term : null;
    writeAttribute(node, "data-worker", key);
    writeClassName(node, `board-desk-worker is-${health.id}`);
    const open = deskChoice.workersOpen.has(key);
    paintDeskTableFold(node, open);
    // The first cell is the draft's: the model's catalog name and the effort it was summoned at
    // (「Opus 5.5 · max」), then the worker in the sidebar's own words (t-45938) — 워커 w-….
    const modelName = boardModelWords(row);
    writeTextContent(node.querySelector(".board-desk-worker-model"), modelName && row.effort
      ? t("board.desk.workerModel", "{{model}} · {{effort}}", { model: modelName, effort: row.effort }) : modelName);
    writeTextContent(node.querySelector(".board-desk-worker-id"),
      workerRoleWords(ledgerWorkerRole({ worker: row.worker, takenOver: row.taken_over === true })));
    writeTextContent(node.querySelector(".board-desk-worker-pane"), row.term != null ? String(row.term) : row.pane || "—");
    const review = ledgerReviewWord(row);
    writeTextContent(node.querySelector(".board-desk-worker-task"),
      [row.task || row.task_id, review, ledgerClosedReason(row.closed)].filter(Boolean).join(" · "));
    // The state is the health's word beside its dot; a gate that holds the worker says so beside it.
    const word = t(health.key, health.word);
    const reset = health.id === "walled" && Number.isFinite(row.wall?.resets_at_ms)
      ? usageCountdown(row.wall.resets_at_ms - now) : "";
    const healthNode = node.querySelector(".board-desk-worker-health");
    writeAttribute(healthNode, "data-dot", health.dot);
    writeTextContent(healthNode, reset ? [word, reset].join(" · ") : word);
    const gate = row.gate;
    const holding = gate?.verdict === "pause" || gate?.verdict === "stop" ? GATE_VERDICTS[gate.verdict] : null;
    const verdict = node.querySelector(".board-desk-worker-verdict");
    writeTextContent(verdict, holding ? t(holding.key, holding.word) : "");
    writeAttribute(verdict, "data-verdict", holding ? gate.verdict : "");
    writeHidden(verdict, holding === null);
    const from = Number(row.dispatch_started_ms) || 0;
    writeTextContent(node.querySelector(".board-desk-worker-took"), from > 0 ? deskSpanWords(now - from) : "—");
    const costCell = node.querySelector(".board-desk-worker-cost");
    writeTextContent(costCell, gate?.cost === "read" ? gateSpentWords(gate) : "—");
    writeAttribute(costCell, "data-tip", gate && gate.cost !== "read" ? gateCostNoteWords(gate) : "");
    writeTextContent(node.querySelector(".board-desk-worker-calls"), gate ? String(gate.metrics?.steps ?? 0) : "—");
    const lines = deskCheckouts.get(row.checkout)?.lines;
    writeTextContent(node.querySelector(".board-desk-worker-lines"), lines ? deskLinesWords(lines) : "—");
    paintDeskWall(node, row, now);
    paintDeskTurn(node, row);
    // The details, while the row is open: the facts the row has no cell for, the last word heard, the gate's
    // line with its basis, the keeping, the door to the task.
    if (open) {
      const detail = deskTableDetail(node, node.__detail);
      writeTextContent(detail.querySelector(".board-desk-worker-facts"), deskWorkerFacts(row, now));
      const at = deskWorkerAt(row, card);
      const heard = detail.querySelector(".board-desk-worker-heard");
      writeHidden(heard, at <= 0);
      writeTextContent(heard.firstElementChild, t("board.workers.heard", "마지막 활동"));
      writeTextContent(detail.querySelector(".board-desk-worker-age"), at > 0
        ? t("board.desk.mailAge", "{{time}} 전", { time: agoWord(at, now) }) : "");
      paintDeskGate(detail.querySelector(".board-desk-worker-gate"), row.gate);
      paintDeskKept(detail.querySelector(".board-desk-worker-kept"), row.kept, null);
      const main = detail.querySelector(".board-desk-worker-main");
      writeTextContent(main, t("board.tasks.details", "작업 자세히 보기"));
      writeDisabled(main, node.__term == null);
    }
    return node;
  })]);
  // A worker that left the roster takes its opened details with it.
  for (const key of deskChoice.workersOpen) if (!keys.has(key)) deskChoice.workersOpen.delete(key);
  return true;
}

/* ---- 한도 벽 칩 (t-39771) ----------------------------------------------------
 *
 * 한도 벽에 선 워커가 다는 칩과 그 곁의 두 단추. 사실은 모두 원장의 것이다 — 칩은 행의
 * `quota_wall`(`wall_chip_for`)이고, 단추를 눌렀을 때 받아들여질지, 아니라면 왜인지도 같은
 * 읽기가 정한다. 여기는 낱말과 차례뿐이다. 원장이 재개 시각을 모르면 모른다고 적고 시각을
 * 지어내지 않는다. 사이드바의 행(`dressWorktreeWall`)이 같은 문장을 쓴다. */

/* 칩이 지금 서 있는가: 원장이 실어 보냈고, 그 벽이 아직 침묵을 설명할 때. */
function quotaWallStanding(chip, now) {
  return Boolean(chip) && now < chip.stands_until_ms;
}

/* 사람의 현지 시각, 시:분. */
function quotaWallClock(ms) {
  const at = new Date(ms);
  return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
}

/* 칩의 문장 — 재개 시각을 알면 그 시각을, 모르면 모른다고. */
function quotaWallWords(chip) {
  return Number.isFinite(chip.resets_at_ms)
    ? t("quotaWall.chipAt", "한도 대기 · {{time}} 재개 예정", { time: quotaWallClock(chip.resets_at_ms) })
    : t("quotaWall.chipUnknown", "한도 대기 · 재개 시각 모름");
}

/* 벽은 원장의 박자가 아니라 시계로 끝난다. 가장 가까운 끝에 타이머 하나를 걸어, 칩을 다는
 * 두 표면(사이드바의 행과 이 데스크의 워커 줄)을 그때 다시 그린다. */
let quotaWallExpiry = { at: Infinity, timer: 0 };
function noteQuotaWallUntil(until) {
  if (!Number.isFinite(until) || until >= quotaWallExpiry.at) return;
  clearTimeout(quotaWallExpiry.timer);
  quotaWallExpiry = {
    at: until,
    timer: setTimeout(() => {
      quotaWallExpiry = { at: Infinity, timer: 0 };
      scheduleAgentPaint(["cards"]);
      scheduleDeskPaint();
    }, Math.max(0, until - Date.now()) + 50),
  };
}

/* 누른 결과, 워커마다 그 벽 하나에 대해: 보내는 중, 됨, 안 됨과 그 까닭. 원장이 다음 벽의
 * 칩을 실어 보내면 벽의 행 id가 바뀌므로 지난 벽의 말은 저절로 내려간다. */
const deskWallSaid = new Map();

/* 넘길 곳의 선택지: 원장이 아는 선언된 대안이 먼저, 그 뒤로 이 기계에 깔린 에이전트 가운데 그
 * 워커 자신의 것이 아닌 것. 누름은 에이전트를 대신 고르지 않는다 — 고르는 것은 사람이다. */
function quotaWallTargets(row, chip) {
  const declared = chip.declared_alt;
  const spelled = declared ? [declared.agent, declared.model, declared.effort].filter(Boolean).join(":") : "";
  const listed = spelled
    ? [{ value: spelled, label: t("quotaWall.declared", "{{agent}} (지정된 대안)", { agent: spelled }) }]
    : [];
  for (const agent of installedAgents()) {
    if (agent.id === row.agent || agent.id === declared?.agent) continue;
    listed.push({ value: agent.id, label: agentSaidName(agent.id) });
  }
  return listed;
}

function paintQuotaWallTargets(select, row, chip) {
  const targets = quotaWallTargets(row, chip);
  const said = targets.map((one) => `${one.value}=${one.label}`).join("|");
  if (select.dataset.said !== said) {
    const kept = select.value;
    select.replaceChildren(...targets.map((one) => {
      const option = document.createElement("option");
      option.value = one.value;
      option.textContent = one.label;
      return option;
    }));
    if (targets.some((one) => one.value === kept)) select.value = kept;
    select.dataset.said = said;
  }
  writeHidden(select, targets.length === 0);
  writeAttribute(select, "aria-label", t("quotaWall.target", "넘길 에이전트"));
  return targets.length;
}

/* 줄 밑의 띠: 칩, 지금 다시 시도, 넘길 곳, 넘기기, 안 되는 까닭, 누른 결과. 워커 표에서는 그 워커의
 * 행 밑에 선 행 하나다(t-70264) — 펼치지 않아도 보이고, 단추는 행의 단추 밖에 선다. */
function deskWallStrip(view, row) {
  const strip = deskElement("tr", "board-desk-worker-wall");
  strip.hidden = true;
  const cell = deskElement("td", "board-desk-worker-wall-cell");
  cell.colSpan = DESK_WORKER_COLUMNS.length;
  strip.append(cell);
  const retry = deskElement("button", "board-btn board-desk-worker-wall-retry");
  retry.type = "button";
  retry.onclick = () => void pressQuotaWall(view, row.__wallRow, "retry", null);
  const to = deskElement("select", "board-desk-worker-wall-to");
  const hand = deskElement("button", "board-btn board-desk-worker-wall-handover");
  hand.type = "button";
  hand.onclick = () => void pressQuotaWall(view, row.__wallRow, "handover", to.value);
  cell.append(deskElement("strong", "board-desk-worker-wall-chip"), retry, to, hand,
    deskElement("span", "board-desk-worker-wall-why"), deskElement("span", "board-desk-worker-wall-said"));
  return strip;
}

/* 턴이 실패한 워커의 띠(t-69010): 실패와 그 오류의 첫 말, 다시 띄우기, 누른 결과. 원장이 그 실패를
 * 지금도 세워 두는 동안만 선다(`turn_failed`, 시도의 가장 새 편지) — 워커가 다시 말하면 띠는 내려간다.
 * 한도 벽의 「지금 다시 시도」와 같은 길이다: 사람이 확인해야 창의 명령을 부르고, 그 명령은 누른 때의
 * 원장을 다시 읽어 거절할 것은 거절한다. */
const deskTurnSaid = new Map();

function deskTurnStrip(view, row) {
  const strip = deskElement("tr", "board-desk-worker-turn");
  strip.hidden = true;
  const cell = deskElement("td", "board-desk-worker-turn-cell");
  cell.colSpan = DESK_WORKER_COLUMNS.length;
  strip.append(cell);
  const retry = deskElement("button", "board-btn board-desk-worker-turn-retry");
  retry.type = "button";
  retry.onclick = () => void pressTurnFailed(view, row.__turnRow);
  cell.append(deskElement("strong", "board-desk-worker-turn-chip"), retry,
    deskElement("span", "board-desk-worker-turn-said"));
  return strip;
}

function paintDeskTurn(node, row) {
  const strip = node.querySelector(".board-desk-worker-turn");
  const failed = row.turn_failed && typeof row.turn_failed === "object" ? row.turn_failed : null;
  writeHidden(strip, !failed);
  if (!failed) return;
  node.__turnRow = row;
  const error = String(failed.error ?? "").trim();
  writeTextContent(strip.querySelector(".board-desk-worker-turn-chip"), error === ""
    ? t("sidebar.team.turnFailed", "턴 실패")
    : t("sidebar.team.turnFailedLine", "턴 실패 — {{error}}", { error }));
  const held = deskTurnSaid.get(`${row.run}/${row.worker}`);
  const mine = held?.at === failed.at_ms ? held : null;
  const retry = strip.querySelector(".board-desk-worker-turn-retry");
  writeTextContent(retry, t("turnFailed.retry", "다시 띄우기"));
  writeDisabled(retry, mine?.sending === true || mine?.ok === true);
  writeAttribute(retry, "data-tip", t("turnFailed.retryTip", "입력 칸에 이어서 하라는 한 줄을 넣습니다 — 이 실패에는 한 번만"));
  const saidLine = strip.querySelector(".board-desk-worker-turn-said");
  writeTextContent(saidLine, mine?.text ?? "");
  writeHidden(saidLine, !mine);
}

async function pressTurnFailed(view, row) {
  const failed = row?.turn_failed;
  if (!failed) return;
  const key = `${row.run}/${row.worker}`;
  if (deskTurnSaid.get(key)?.sending) return;
  const worker = row.worker;
  const said = await askConfirm({
    title: t("turnFailed.retryTitle", "이 워커를 다시 띄울까요?"),
    body: t("turnFailed.retryBody", "이 워커({{worker}})의 지난 차례가 실패했습니다. 입력 칸에 이어서 하라는 한 줄을 넣습니다 — 이 실패에는 한 번만 넣습니다.", { worker }),
    confirm: t("turnFailed.retry", "다시 띄우기"),
    deny: t("app.cancel", "취소"),
    cancel: false,
  });
  if (said !== true) return;
  const held = { at: failed.at_ms, sending: true, ok: false, text: t("quotaWall.sending", "보내는 중…") };
  deskTurnSaid.set(key, held);
  paintCoordinatorDesk(view);
  try {
    // The failure the person was shown, not the one standing when the answer lands (the review R2, P2-10).
    await invoke("turn_failed_retry", { run: row.run, worker, dispatch: failed.dispatch, letter: failed.letter });
    held.ok = true;
    held.text = t("quotaWall.retried", "이어서 하라는 한 줄을 넣었습니다");
    refreshDeskLedger();
  } catch (error) {
    held.text = t("quotaWall.refused", "안 됨 — {{why}}", { why: quotaWallRefusalWords(error) });
  } finally {
    held.sending = false;
    paintCoordinatorDesk(view);
  }
}

/* 원장과 창은 거절을 문장이 아니라 코드와 그 문장이 이름하는 사실로 보낸다(t-47935). 이 표가 그
 * 코드를 사람의 언어로 말하는 한 곳이다 — 원장의 `WallRefusal`과 창의 `WindowRefusal`이 가진
 * 코드마다 한 줄이고, 소스 계약이 두 열거형에서 코드를 읽어 빠진 줄을 잡는다. */
const QUOTA_WALL_REFUSALS = {
  worker_not_live: (said) => said.state === "sleeping"
    ? t("quotaWall.why.workerSleeping", "워커 {{worker}}가 계정 이동으로 잠들어 있습니다", { worker: said.worker })
    : said.state === "released"
      ? t("quotaWall.why.workerReleased", "워커 {{worker}}는 이미 손을 놓았습니다 — 끝났거나 다른 에이전트로 넘어갔습니다", { worker: said.worker })
      : t("quotaWall.why.workerReleasing", "워커 {{worker}}가 손을 놓는 중이거나, 놓았는지 아직 확인되지 않았습니다", { worker: said.worker }),
  pane_taken_over: (said) => t("quotaWall.why.paneTakenOver", "워커 {{worker}}의 판을 사람이 가져갔습니다 — 그 터미널은 이제 사람의 것입니다", { worker: said.worker }),
  no_open_attempt: (said) => t("quotaWall.why.noOpenAttempt", "워커 {{worker}}에게 열린 시도가 없습니다", { worker: said.worker }),
  no_wall: (said) => t("quotaWall.why.noWall", "원장이 본 한도 벽이 워커 {{worker}}에게 없습니다", { worker: said.worker }),
  wall_lifted: (said) => t("quotaWall.why.wallLifted", "워커 {{worker}}의 한도가 이미 풀렸습니다", { worker: said.worker }),
  account_moved: (said) => t("quotaWall.why.accountMoved", "워커 {{worker}}는 이 벽 뒤에 다른 계정으로 다시 앉았습니다 — 그 벽은 떠난 계정의 것입니다", { worker: said.worker }),
  pane_not_here: (said) => t("quotaWall.why.paneNotHere", "이 창은 워커 {{worker}}의 판을 갖고 있지 않습니다", { worker: said.worker }),
  no_transient_order: (said) => t("quotaWall.why.noTransientOrder", "일시 오류 뒤에 이어서 하라는 지시가 이 실행에 없습니다"),
  unknown_run: (said) => t("quotaWall.why.unknownRun", "실행 {{run}}을 원장에서 찾을 수 없습니다", { run: said.run }),
  unknown_worker: (said) => t("quotaWall.why.unknownWorker", "워커 {{worker}}를 원장에서 찾을 수 없습니다", { worker: said.worker }),
  unknown_dispatch: (said) => t("quotaWall.why.unknownDispatch", "시도 {{dispatch}}를 원장에서 찾을 수 없습니다", { dispatch: said.dispatch }),
  unknown_task: (said) => t("quotaWall.why.unknownTask", "과업 {{task}}를 원장에서 찾을 수 없습니다", { task: said.task }),
  seat_lost: (said) => t("quotaWall.why.seatLost", "워커 {{worker}}의 판을 다른 워커가 차지했습니다", { worker: said.worker }),
  awaiting_answer: (said) => t("quotaWall.why.awaitingAnswer", "워커 {{worker}}가 자기가 물은 질문의 답을 기다리고 있습니다", { worker: said.worker }),
  at_wall: (said) => t("quotaWall.why.atWall", "시도 {{dispatch}}는 한도 벽에 서 있습니다 — 넘기기가 그 답입니다", { dispatch: said.dispatch }),
  wall_not_now: (said) => t("quotaWall.why.wallNotNow", "이 누름은 시도 {{dispatch}}가 지금 서 있지 않은 지난 벽에 대한 것입니다", { dispatch: said.dispatch }),
  turn_not_now: (said) => t("quotaWall.why.turnNotNow", "이 누름은 시도 {{dispatch}}가 지금 서 있지 않은 지난 실패에 대한 것입니다 — 워커가 그 뒤에 다시 말했습니다", { dispatch: said.dispatch }),
  resume_in_flight: (said) => t("quotaWall.why.resumeInFlight", "이어서 하라는 한 줄을 지금 넣는 중입니다"),
  resume_ceiling: (said) => t("quotaWall.why.resumeCeiling", "시도 {{dispatch}}에 이미 {{count}}번 이어 쓰기를 넣어 상한에 닿았습니다", { dispatch: said.dispatch, count: said.count }),
  already_typed: (said) => t("quotaWall.why.alreadyTyped", "이 벽에 대해 시도 {{dispatch}}에 이미 한 줄을 넣었습니다 — 그 글이 입력 칸에 남아 있을 수 있습니다", { dispatch: said.dispatch }),
  too_soon: (said) => t("quotaWall.why.tooSoon", "시도 {{dispatch}}의 마지막 이어 쓰기가 {{seconds}}초 전이라 아직 간격이 지나지 않았습니다",
    { dispatch: said.dispatch, seconds: Math.ceil(Number(said.typed_ms_ago) / 1_000) }),
  coordinator_seat_elsewhere: (said) => t("quotaWall.why.coordinatorSeatElsewhere", "이 창은 실행 {{run}}의 코디네이터 자리를 갖고 있지 않습니다", { run: said.run }),
  no_live_coordinator: (said) => t("quotaWall.why.noLiveCoordinator", "실행 {{run}}의 코디네이터 자리가 비어 있습니다", { run: said.run }),
  handover_waits_for_typing: (said) => t("quotaWall.why.handoverWaitsForTyping", "시도 {{dispatch}}에 이어 쓰기를 넣는 중입니다 — 넘기기는 그것이 끝난 뒤에 됩니다", { dispatch: said.dispatch }),
  attempt_ended: (said) => t("quotaWall.why.attemptEnded", "시도 {{dispatch}}는 이미 끝났습니다 — 넘기기는 열린 시도에만 됩니다", { dispatch: said.dispatch }),
  no_checkout: (said) => t("quotaWall.why.noCheckout", "워커 {{worker}}가 체크아웃을 알리지 않아 물려줄 것이 없습니다", { worker: said.worker }),
  already_walked: (said) => t("quotaWall.why.alreadyWalked", "시도 {{dispatch}}는 이미 넘어갔습니다 — 한 시도는 한 번만 넘깁니다", { dispatch: said.dispatch }),
  handover_ceiling: (said) => t("quotaWall.why.handoverCeiling", "과업 {{task}}는 이미 {{count}}번 넘어가 상한에 닿았습니다", { task: said.task, count: said.count }),
  wall_row_gone: (said) => t("quotaWall.why.wallRowGone", "벽 {{wall}}의 기록이 원장에서 사라졌습니다", { wall: said.wall }),
  no_runtime: (said) => t("quotaWall.why.noRuntime", "이 창에서 오케스트레이션이 시작되지 않았습니다"),
  ledger_unread: (said) => t("quotaWall.why.ledgerUnread", "원장을 읽지 못했습니다: {{detail}}", { detail: said.detail }),
  composer_never_heard: (said) => t("quotaWall.why.composerNeverHeard", "이 창이 워커 {{worker}}의 판에서 아직 아무것도 듣지 못해 넣지 않았습니다", { worker: said.worker }),
  composer_held_by_person: (said) => t("quotaWall.why.composerHeldByPerson", "워커 {{worker}}의 지난 차례를 사람이 끊었습니다 — 그 입력 칸은 사람의 것입니다", { worker: said.worker }),
  composer_shell_in_front: (said) => t("quotaWall.why.composerShellInFront", "워커 {{worker}}의 판 앞에 에이전트가 아니라 셸이 있습니다 — 한 줄이 명령으로 실행되므로 넣지 않았습니다", { worker: said.worker }),
  composer_turn_running: (said) => t("quotaWall.why.composerTurnRunning", "워커 {{worker}}의 판에서 차례가 진행 중입니다", { worker: said.worker }),
  switch_in_flight: (said) => t("quotaWall.why.switchInFlight", "계정 전환이 지금 판을 옮기고 있습니다 — 잠시 뒤 다시 누르세요"),
  attempt_changed: (said) => t("quotaWall.why.attemptChanged", "누르는 사이에 워커나 그 자리, 그 시도가 바뀌었습니다 — 다시 누르세요"),
  not_addressed: (said) => t("quotaWall.why.notAddressed", "판에 글을 보낼 수 없어 아무것도 넣지 않았습니다"),
  target_unreadable: (said) => t("quotaWall.why.targetUnreadable", "넘길 에이전트 {{to}}를 읽을 수 없습니다", { to: said.to }),
  no_alternative: (said) => t("quotaWall.why.noAlternative", "이 워커에게 지정된 대안이 없습니다 — 과업을 받을 에이전트를 고르세요"),
  at_work_again: (said) => t("quotaWall.why.atWorkAgain", "워커 {{worker}}가 다시 일하고 있어 넘기지 않았습니다", { worker: said.worker }),
  stop_refused: (said) => t("quotaWall.why.stopRefused", "워커를 멈추지 못했습니다: {{detail}}", { detail: said.detail }),
  replacement_not_started: (said) => t("quotaWall.why.replacementNotStarted", "시도 {{dispatch}}는 끝났지만 대신할 워커가 시작되지 않았습니다 — 과업은 다시 대기 중입니다: {{detail}}",
    { dispatch: said.dispatch, detail: said.detail }),
  press_failed: (said) => t("quotaWall.why.pressFailed", "누르기가 이뤄지지 않았습니다: {{detail}}", { detail: said.detail }),
};

/* 거절 하나를 사람의 말로. 표에 없는 코드나 코드가 아닌 답은 모른다고 말한다 — 날것의 코드도
 * 영어 문장도 화면에 오르지 않는다. */
function quotaWallRefusalWords(said) {
  const code = typeof said?.code === "string" ? said.code : "";
  return Object.prototype.hasOwnProperty.call(QUOTA_WALL_REFUSALS, code)
    ? QUOTA_WALL_REFUSALS[code](said)
    : t("quotaWall.why.unknown", "까닭을 이 창이 알아듣지 못했습니다");
}

function paintDeskWall(node, row, now) {
  const strip = node.querySelector(".board-desk-worker-wall");
  const chip = quotaWallStanding(row.quota_wall, now) ? row.quota_wall : null;
  writeHidden(strip, !chip);
  if (!chip) return;
  node.__wallRow = row;
  noteQuotaWallUntil(chip.stands_until_ms);
  writeTextContent(strip.querySelector(".board-desk-worker-wall-chip"), quotaWallWords(chip));
  const held = deskWallSaid.get(`${row.run}/${row.worker}`);
  const mine = held?.wall === chip.wall ? held : null;
  const sending = mine?.sending === true;
  const retry = strip.querySelector(".board-desk-worker-wall-retry");
  writeTextContent(retry, t("quotaWall.retry", "지금 다시 시도"));
  writeDisabled(retry, sending || Boolean(chip.retry_refusal));
  writeAttribute(retry, "data-tip", t("quotaWall.retryTip", "입력 칸에 이어서 하라는 한 줄을 넣습니다 — 이 벽에서는 한 번만"));
  const targets = paintQuotaWallTargets(strip.querySelector(".board-desk-worker-wall-to"), row, chip);
  const hand = strip.querySelector(".board-desk-worker-wall-handover");
  writeTextContent(hand, t("quotaWall.handOver", "다른 에이전트로 넘기기"));
  writeDisabled(hand, sending || Boolean(chip.hand_over_refusal) || targets === 0);
  writeAttribute(hand, "data-tip", t("quotaWall.handOverTip", "이 워커를 멈추고 같은 체크아웃에서 고른 에이전트에게 과업을 넘깁니다"));
  const why = [
    chip.retry_refusal
      ? t("quotaWall.retryRefused", "다시 시도 안 됨 — {{why}}", { why: quotaWallRefusalWords(chip.retry_refusal) })
      : "",
    chip.hand_over_refusal
      ? t("quotaWall.handOverRefused", "넘기기 안 됨 — {{why}}", { why: quotaWallRefusalWords(chip.hand_over_refusal) })
      : targets === 0 ? t("quotaWall.noTarget", "넘길 에이전트가 이 기계에 없음") : "",
  ].filter(Boolean).join(" · ");
  const whyLine = strip.querySelector(".board-desk-worker-wall-why");
  writeTextContent(whyLine, why);
  writeHidden(whyLine, why === "");
  const saidLine = strip.querySelector(".board-desk-worker-wall-said");
  writeTextContent(saidLine, mine?.text ?? "");
  writeHidden(saidLine, !mine);
  writeAttribute(saidLine, "data-tone", mine ? (mine.sending ? "sending" : mine.ok ? "ok" : "refused") : "");
}

/* 칩 곁의 두 단추. 사람이 확인 창에서 「예」라고 해야만 창의 명령을 부른다 — 그 명령은 주
 * 창에서만 받고, 눌린 때의 원장을 다시 읽어 거절할 것은 거절한다. 답은 그 벽에 대한 말로 띠에
 * 남고(안 됐으면 그 까닭의 코드를 사람의 말로), 원장을 다시 읽는다. */
async function pressQuotaWall(view, row, act, to) {
  const started = handNow();
  const chip = row?.quota_wall;
  if (!chip) return;
  const key = `${row.run}/${row.worker}`;
  if (deskWallSaid.get(key)?.sending) return;
  const worker = row.worker;
  const agent = act === "handover" ? agentSaidName(String(to ?? "").split(":")[0]) : "";
  const said = await askConfirm(act === "retry"
    ? {
      title: t("quotaWall.retryTitle", "지금 다시 시도할까요?"),
      body: t("quotaWall.retryBody", "이 워커({{worker}})의 입력 칸에 이어서 하라는 한 줄을 넣습니다. 한도가 아직 풀리지 않았으면 다시 멈출 수 있고, 이 벽에서는 한 번만 넣습니다.", { worker }),
      confirm: t("quotaWall.retry", "지금 다시 시도"),
      deny: t("app.cancel", "취소"),
      cancel: false,
    }
    : {
      title: t("quotaWall.handOverTitle", "다른 에이전트로 넘길까요?"),
      body: t("quotaWall.handOverBody", "이 워커({{worker}})를 멈추고 같은 체크아웃에서 {{agent}}에게 과업을 넘깁니다. 커밋하지 않은 변경은 그대로 남습니다.", { worker, agent }),
      confirm: t("quotaWall.handOver", "다른 에이전트로 넘기기"),
      deny: t("app.cancel", "취소"),
      danger: true,
      cancel: false,
    });
  if (said !== true) return;
  // The press's hand, joined by the answer's (shell-boot.js `flowHand`).
  const hand = flowHand(started);
  const held = { wall: chip.wall, sending: true, ok: false, text: t("quotaWall.sending", "보내는 중…") };
  deskWallSaid.set(key, held);
  paintCoordinatorDesk(view);
  try {
    const answer = act === "retry"
      ? await invoke("quota_wall_retry", { run: row.run, worker })
      : await invoke("quota_wall_handover", { run: row.run, worker, to }, { hand });
    held.ok = true;
    held.text = act === "retry"
      ? t("quotaWall.retried", "이어서 하라는 한 줄을 넣었습니다")
      : answer?.already
        ? t("quotaWall.handedOverAlready", "이미 {{worker}}에게 넘어갔습니다", { worker: String(answer?.replacement ?? "") })
        : t("quotaWall.handedOver", "{{worker}}에게 넘겼습니다", { worker: String(answer?.replacement ?? "") });
    refreshDeskLedger();
  } catch (error) {
    held.text = t("quotaWall.refused", "안 됨 — {{why}}", { why: quotaWallRefusalWords(error) });
  } finally {
    held.sending = false;
    paintCoordinatorDesk(view);
  }
}

/* ---- 묶음 통합 큐의 카드 (t-45315) ---------------------------------------------
 *
 * 코디가 고른 준비된 과업 여럿이 main 위에 함께 쌓이고, 끝의 트리가 한 번 게이트되고, 초록이면
 * main이 빨리감기된다. 카드는 원장이 접은 묶음(`DeskSnapshot.land_batches`의 `batch`, core
 * `BatchState`)을 그대로 읽는다. 칩은 원장의 한 낱말(`standing`)이고, 단추의 켜짐·꺼짐은 원장이 그
 * 누름을 받을 규칙(core `promotable`·`searchable`·`restackable`·`cancellable`)과 같다 — 원장이
 * 거절할 단추를 켜 두지 않고, 받을 단추를 꺼 두지 않는다. 꺼진 단추는 까닭을 팁과 단추 밑의 목록에
 * 적는다. 큐가 꺼진 저장소의 묶음은 그리지 않는다 — 켰다가 끈 저장소에 열린 묶음이 남았으면
 * 「꺼짐: 열린 묶음 N개」 한 줄과 그 묶음의 취소만 선다(꺼진 큐가 받는 누름은 취소뿐이다). */

/* 원장의 낱말마다 칩의 말과 신호. */
const BATCH_CHIPS = Object.freeze({
  chosen: { key: "board.batch.chipChosen", word: "고름 · 쌓기 기다림", tone: "wait" },
  stacking: { key: "board.batch.chipStacking", word: "쌓는 중", tone: "wait" },
  stack_stopped: { key: "board.batch.chipStackStopped", word: "쌓기 멈춤", tone: "halt" },
  stack_abandoned: { key: "board.batch.chipStackAbandoned", word: "쌓기 끊김", tone: "halt" },
  stacked: { key: "board.batch.chipStacked", word: "쌓음 · 게이트 기다림", tone: "ready" },
  gating: { key: "board.batch.chipGating", word: "게이트 중", tone: "wait" },
  green: { key: "board.batch.chipGreen", word: "초록", tone: "done" },
  red: { key: "board.batch.chipRed", word: "빨강", tone: "halt" },
  searching: { key: "board.batch.chipSearching", word: "범인 찾는 중", tone: "wait" },
  culprit_found: { key: "board.batch.chipCulprit", word: "범인: {{task}}", tone: "halt" },
  promoting: { key: "board.batch.chipPromoting", word: "올리는 중", tone: "wait" },
  promote_stopped: { key: "board.batch.chipPromoteStopped", word: "올리기 멈춤", tone: "halt" },
  promoted: { key: "board.batch.chipPromoted", word: "main에 올림", tone: "done" },
  cancelled: { key: "board.batch.chipCancelled", word: "취소됨", tone: "quiet" },
});

/* 카드의 단추, 늘 이 차례로. */
const BATCH_ACTS = Object.freeze(["promote", "bisect", "restack-drop", "restack", "cancel"]);

/* 꺼진 단추의 까닭. 보내는 중은 팁에만 서고 목록에는 서지 않는다 — 누른 결과 줄이 이미 말한다. */
const BATCH_HELD = Object.freeze({
  promoting: { key: "board.batch.heldPromoting", word: "올리는 중 — 끝나야 결과가 정해집니다" },
  stacking: { key: "board.batch.heldStacking", word: "쌓는 중" },
  noStack: { key: "board.batch.heldNoStack", word: "쌓은 것이 없음" },
  red: { key: "board.batch.heldRed", word: "게이트가 빨강 — 범인을 찾거나 다시 쌓으세요" },
  noGreen: { key: "board.batch.heldNoGreen", word: "끝의 초록 게이트가 아직 없음" },
  searching: { key: "board.batch.heldSearching", word: "범인 찾는 중" },
  culpritFound: { key: "board.batch.heldCulpritFound", word: "범인을 이미 찾았음" },
  nothingLeft: { key: "board.batch.heldNothingLeft", word: "빼고 나면 남는 과업이 없음" },
  sending: { key: "board.batch.heldSending", word: "보내는 중…" },
});

/* 빠진 과업의 까닭, 원장의 낱말(`DropWhy`)마다. */
const BATCH_DROPPED = Object.freeze({
  culprit: { key: "board.batch.droppedCulprit", word: "범인" },
  asked: { key: "board.batch.droppedAsked", word: "빼 달라고 함" },
  on_main: { key: "board.batch.droppedOnMain", word: "이미 main에 있음" },
});

/* 누른 결과는 카드가 누른 때의 단계나 답이 말한 단계에 있는 동안만 남는다 — 원장이 그 뒤로 더
 * 움직였으면 칩과 줄이 이미 새 사실을 말한다. 답의 낱말마다 그 단계들. */
const BATCH_ANSWER_STANDINGS = Object.freeze({
  stacking: ["chosen", "stacking", "stacked"],
  stacked: ["stacked"],
  searching: ["searching", "gating"],
  culprit_found: ["culprit_found"],
  cancelled: ["cancelled"],
  promoting: ["promoting"],
  promoted: ["promoted"],
  promote_stopped: ["promote_stopped"],
  stack_stopped: ["stack_stopped"],
});

/* 누른 결과, 묶음마다(`run/batch`). 다음 누름이 갈아 끼운다. 말은 그릴 때 짓는다 — 언어를 바꾸면
 * 지난 답도 그 언어로 읽힌다. */
const deskBatchSaid = new Map();

function batchEnded(batch) {
  return Boolean(batch.promotion) || Boolean(batch.cancelled);
}

/* 쌓기가 아직 열려 있다: 시작했고, 끝나지도 멈추지도 끊기지도 않았다. */
function batchStackingOpen(batch) {
  return Boolean(batch.stacking) && !batch.stacked && !batch.stackHalt && batch.stackAbandoned !== true;
}

/* 다시 쌓기가 뺄 과업: 찾은 범인, 아니면 쌓기를 멈춘 과업. */
function batchDropTask(batch) {
  return String(batch.culprit?.task || batch.stackHalt?.task || "");
}

/* 카드에 서는 단추. 끝난 묶음에는 없고, 큐가 꺼진 저장소의 열린 묶음에는 취소 하나다. */
function batchActs(batch, on) {
  if (batchEnded(batch)) return [];
  if (!on) return ["cancel"];
  return BATCH_ACTS.filter((act) => (act !== "bisect" || batch.tipGate?.verdict === "red")
    && (act !== "restack-drop" || batchDropTask(batch) !== ""));
}

/* 그 단추를 원장이 지금 받지 않을 까닭(`BATCH_HELD`의 이름), 받으면 null. */
function batchHeldReason(batch, act) {
  if (batch.promoting) return "promoting";
  if (act === "cancel") return null;
  if (act === "promote") {
    if (batchStackingOpen(batch)) return "stacking";
    if (!batch.stacked) return "noStack";
    const gate = batch.tipGate?.tree === batch.stacked.tree ? batch.tipGate : null;
    if (gate?.verdict === "green") return null;
    return gate?.verdict === "red" ? "red" : "noGreen";
  }
  if (act === "bisect") {
    if (batch.culprit) return "culpritFound";
    return batch.search ? "searching" : null;
  }
  if (batchStackingOpen(batch)) return "stacking";
  return act === "restack-drop" && (batch.members ?? []).length <= 1 ? "nothingLeft" : null;
}

function batchActLabel(batch, act) {
  switch (act) {
    case "promote": return t("board.batch.actPromote", "main으로 올리기");
    case "bisect": return t("board.batch.actBisect", "범인 찾기");
    case "restack-drop": return batch.culprit
      ? t("board.batch.actDropCulprit", "범인 빼고 다시 쌓기")
      : t("board.batch.actDropTask", "{{task}} 빼고 다시 쌓기", { task: batchDropTask(batch) });
    case "restack": return t("board.batch.actRestack", "새 main 위에 다시 쌓기");
    default: return t("board.batch.actCancel", "취소");
  }
}

function batchWho(by) {
  if (by?.kind === "person") return t("board.batch.whoPerson", "사람");
  if (by?.kind === "coordinator") return t("board.batch.whoCoordinator", "코디");
  return t("board.batch.whoApp", "앱");
}

/* 카드의 시각: 하루 안이면 시:분, 그 밖이면 날짜 — 브라우저의 언어가 아니라 창이 고른 언어로. */
function batchClock(ms, now) {
  if (!ms) return "—";
  const tongue = document.documentElement.lang || undefined;
  const when = new Date(ms);
  return Math.abs(now - ms) < 86_400_000
    ? when.toLocaleTimeString(tongue, { hour: "2-digit", minute: "2-digit" })
    : when.toLocaleDateString(tongue);
}

/* 원장이 적은 이름은 앞의 몇 개, 수는 전체다 — 다 적지 못했으면 줄임표가 붙는다. */
function batchFiles(files, total) {
  const names = Array.isArray(files) ? files.map(String) : [];
  return (Number(total) > names.length ? [...names, "…"] : names).join(", ");
}

function batchStackStop(halt) {
  if (!halt) return "";
  const said = { step: halt.step, task: halt.task, total: halt.total, files: batchFiles(halt.files, halt.total) };
  switch (halt.why) {
    case "conflict": return t("board.batch.stopConflict", "S{{step}} {{task}}에서 충돌 — 파일 {{total}}개: {{files}}", said);
    case "git_silent": return t("board.batch.stopGitSilent", "S{{step}} {{task}}에서 git이 한도 안에 답하지 않음", said);
    case "git_refused": return t("board.batch.stopGitRefused", "S{{step}} {{task}}에서 git이 거절함", said);
    default: return t("board.batch.stopPublishRefused", "쌓은 끝을 통합 ref로 게시하지 못함 — 다시 쌓기", said);
  }
}

function batchPromoteStop(halt, base) {
  if (!halt) return "";
  switch (halt.why) {
    case "main_moved":
      return halt.seen
        ? t("board.batch.stopMainMoved", "main이 움직였어요({{from}} → {{to}}). 새 main 위에 다시 쌓기",
          { from: String(base ?? "").slice(0, 7), to: String(halt.seen).slice(0, 7) })
        : t("board.batch.stopMainMovedPlain", "main이 움직였어요. 새 main 위에 다시 쌓기");
    case "unfinished_files":
      return t("board.batch.stopUnfinished", "main 체크아웃에 고치다 만 파일 {{total}}개: {{files}}. 정리하고 다시 누르세요",
        { total: halt.total, files: batchFiles(halt.files, halt.total) });
    case "two_checkouts":
      return t("board.batch.stopTwoCheckouts", "main이 체크아웃 여러 곳에 나가 있어요 — 한 곳만 남기고 다시 누르세요");
    case "git_refused":
      return t("board.batch.stopPromoteRefused", "git이 빨리감기를 거절했어요 — 아무것도 바뀌지 않았습니다");
    case "git_silent":
      return t("board.batch.stopPromoteSilent", "git이 한도 안에 답하지 않았어요 — 아무것도 바뀌지 않았습니다");
    default:
      return t("board.batch.stopUnknownState", "main의 상태를 알 수 없어요 — 손으로 확인하세요");
  }
}

/* 멈춤·끊김·범인 찾기의 한 줄. 칩이 말하는 단계의 것만 선다. */
function batchWhy(batch) {
  switch (batch.standing) {
    case "stack_stopped": return batchStackStop(batch.stackHalt);
    case "stack_abandoned": return t("board.batch.stopAbandoned", "쌓기가 끊김: 다시 쌓기");
    case "searching": {
      const search = batch.search;
      if (!search) return "";
      const span = { lo: search.lo, hi: search.hi, next: search.next };
      return search.next == null
        ? t("board.batch.searchSpan", "구간 S{{lo}}–S{{hi}} · 다음 게이트를 고르는 중", span)
        : t("board.batch.searchNext", "다음 게이트: S{{next}} · 구간 S{{lo}}–S{{hi}}", span);
    }
    case "culprit_found": {
      const culprit = batch.culprit;
      if (!culprit) return "";
      const line = t("board.batch.culpritLine", "범인 {{task}} — S{{step}}에서 빨강이 됨", { task: culprit.task, step: culprit.step });
      return culprit.mainFirst ? `${line} · ${t("board.batch.culpritMainFirst", "main 자체가 초록인지 먼저 확인하세요")}` : line;
    }
    case "promote_stopped": return batchPromoteStop(batch.promoteHalt, batch.base);
    default: return "";
  }
}

function batchFacts(batch) {
  const said = {
    generation: batch.generation, branch: batch.baseBranch, base: String(batch.base ?? "").slice(0, 7),
    tip: String(batch.stacked?.tip ?? "").slice(0, 7),
  };
  const line = batch.stacked
    ? t("board.batch.factsStacked", "세대 {{generation}} · {{branch}} {{base}} → 끝 {{tip}}", said)
    : t("board.batch.facts", "세대 {{generation}} · {{branch}} {{base}}", said);
  const overlaps = Number(batch.stacked?.overlaps ?? 0);
  return overlaps > 0 ? `${line} · ${t("board.batch.overlaps", "양쪽이 고친 파일 {{count}}개", { count: overlaps })}` : line;
}

function batchDropped(batch) {
  const dropped = Array.isArray(batch.dropped) ? batch.dropped : [];
  if (dropped.length === 0) return "";
  const tasks = dropped.map((one) => {
    const why = BATCH_DROPPED[one.why];
    return why ? t("board.batch.droppedTask", "{{task}}({{why}})", { task: one.task, why: t(why.key, why.word) }) : String(one.task);
  }).join(", ");
  return t("board.batch.droppedLine", "빠진 과업: {{tasks}}", { tasks });
}

/* 누가 눌렀는가: 취소, 올림, 올리는 중, 범인 찾기 — 원장이 적은 것만. */
function batchPressed(batch, now) {
  if (batch.cancelled) return t("board.batch.cancelledBy", "취소한 쪽: {{who}}", { who: batchWho(batch.cancelled) });
  const promotion = batch.promotion;
  if (promotion) {
    const line = t("board.batch.promotedBy", "올린 쪽: {{who}} · {{at}}", { who: batchWho(promotion.by), at: batchClock(promotion.at, now) });
    return promotion.settledLater ? `${line} · ${t("board.batch.settledLater", "다음 실행이 확인함")}` : line;
  }
  if (batch.promoting) {
    return t("board.batch.pressedPromote", "올리기를 누른 쪽: {{who}} · {{at}}",
      { who: batchWho(batch.promoting.by), at: batchClock(batch.promoting.at, now) });
  }
  return batch.search ? t("board.batch.pressedSearch", "범인 찾기를 누른 쪽: {{who}}", { who: batchWho(batch.search.by) }) : "";
}

/* 창의 답(`state`)마다 누른 결과의 말과 신호. 아직 올리는 중은 멈춤이 아니다 — git은 끝까지 돈다. */
function batchAnswerWords(answer, batch) {
  switch (answer?.state) {
    case "promoted": return { tone: "ok", say: () => t("board.batch.saidPromoted", "눌렀습니다 — main에 올림") };
    case "promoting": return { tone: "ok", say: () => t("board.batch.saidPromoting", "아직 올리는 중 — git이 끝나면 적습니다") };
    case "stacking": return { tone: "ok", say: () => t("board.batch.saidStacking", "눌렀습니다 — 다시 쌓는 중") };
    case "stacked": return { tone: "ok", say: () => t("board.batch.saidStacked", "눌렀습니다 — 다시 쌓음 · 게이트 기다림") };
    case "searching": return { tone: "ok", say: () => t("board.batch.saidSearching", "눌렀습니다 — 범인 찾기를 시작함") };
    case "culprit_found": return { tone: "ok", say: () => t("board.batch.saidCulprit", "눌렀습니다 — 범인을 찾음") };
    case "cancelled": return { tone: "ok", say: () => t("board.batch.saidCancelled", "눌렀습니다 — 취소됨") };
    case "promote_stopped": {
      const halt = { why: answer.why, files: answer.files, total: answer.total, seen: answer.seen };
      return { tone: "refused", say: () => t("board.batch.saidStopped", "멈춤 — {{why}}", { why: batchPromoteStop(halt, batch.base) }) };
    }
    case "stack_stopped": {
      const halt = { step: answer.step, task: answer.task, why: answer.why, files: answer.files, total: answer.total };
      return { tone: "refused", say: () => t("board.batch.saidStopped", "멈춤 — {{why}}", { why: batchStackStop(halt) }) };
    }
    case "refused": {
      const why = String(answer.why ?? "");
      return { tone: "refused", say: () => t("board.batch.saidRefused", "안 됨 — {{why}}", { why }) };
    }
    default: return { tone: "ok", say: () => t("board.batch.saidDone", "눌렀습니다") };
  }
}

/* 단추마다 확인 창의 물음: 무엇을 하는지 하기 전에 말한다. 취소만 되돌릴 수 없는 단추의 옷을 입는다. */
function batchAsk(batch, act, drop) {
  const count = (batch.members ?? []).length;
  const deny = t("board.batch.askKeep", "그대로 두기");
  const next = Number(batch.generation ?? 1) + 1;
  const branch = String(batch.baseBranch ?? "");
  switch (act) {
    case "promote": return {
      title: t("board.batch.askPromoteTitle", "main으로 올릴까요?"),
      body: t("board.batch.askPromoteBody", "{{branch}} 가지를 묶음 {{batch}}의 끝 {{tip}}까지 빨리감기합니다 — 과업 {{count}}개가 한 번에 올라갑니다. 새 커밋은 생기지 않고, 그 가지의 체크아웃 파일도 함께 바뀝니다.",
        { branch, batch: batch.batch, tip: String(batch.stacked?.tip ?? "").slice(0, 7), count }),
      confirm: batchActLabel(batch, act), deny, cancel: false,
    };
    case "bisect": return {
      title: t("board.batch.askBisectTitle", "범인을 찾을까요?"),
      body: t("board.batch.askBisectBody", "끝의 게이트가 빨강입니다. 쌓은 차례를 반씩 나눠 다시 게이트해 처음 빨강이 되는 과업을 찾습니다 — 과업 {{count}}개면 게이트가 {{gates}}번쯤 더 듭니다.",
        { count, gates: Math.ceil(Math.log2(Math.max(count, 1))) }),
      confirm: batchActLabel(batch, act), deny, cancel: false,
    };
    case "restack-drop": {
      const task = drop[0] ?? "";
      return {
        title: batch.culprit
          ? t("board.batch.askDropCulpritTitle", "범인을 빼고 다시 쌓을까요?")
          : t("board.batch.askDropTaskTitle", "{{task}} 빼고 다시 쌓을까요?", { task }),
        body: t("board.batch.askDropBody", "{{task}} 없이 나머지 과업 {{count}}개를 지금 {{branch}} 위에 {{generation}}번째 세대로 다시 쌓습니다. 빠진 과업의 가지는 그대로 남습니다.",
          { task, count: Math.max(count - drop.length, 0), branch, generation: next }),
        confirm: batchActLabel(batch, act), deny, cancel: false,
      };
    }
    case "restack": return {
      title: t("board.batch.askRestackTitle", "새 main 위에 다시 쌓을까요?"),
      body: t("board.batch.askRestackBody", "과업 {{count}}개를 지금 {{branch}} 위에 {{generation}}번째 세대로 다시 쌓습니다. 지금 세대의 게이트 결과는 쓰지 않습니다.",
        { count, branch, generation: next }),
      confirm: batchActLabel(batch, act), deny, cancel: false,
    };
    default: return {
      title: t("board.batch.askCancelTitle", "묶음을 취소할까요?"),
      body: t("board.batch.askCancelBody", "이 묶음({{batch}})을 닫고 이 세대의 게시에 묘비를 세웁니다. 과업의 가지와 워커 체크아웃은 그대로 둡니다.",
        { batch: batch.batch }),
      confirm: t("board.batch.askCancelYes", "묶음 취소"), deny, danger: true, cancel: false,
    };
  }
}

/* 그릴 묶음: 큐가 켜진 저장소의 묶음은 다, 꺼진 저장소의 묶음은 아직 열린 것만. */
function deskBatchEntries() {
  const all = Array.isArray(deskLedger?.land_batches) ? deskLedger.land_batches : [];
  return all.filter((entry) => entry?.batch && (entry.on === true || !batchEnded(entry.batch)));
}

/* 묶음 하나도 그릴 것이 없으면 블록은 접히고 그 안의 카드는 DOM에서 나간다 — 꺼진 큐는 카드도,
 * 단추도, 칩도 남기지 않는다. */
function paintDeskBatches(block, now, view) {
  const entries = deskBatchEntries();
  const body = block.lastElementChild;
  if (entries.length === 0) {
    if (body.childElementCount > 0) body.replaceChildren();
    return false;
  }
  writeTextContent(block.firstElementChild, t("board.batch.head", "묶음 통합"));
  let off = body.firstElementChild;
  let list = body.lastElementChild;
  if (!off?.classList.contains("board-batch-off") || !list?.classList.contains("board-batch-list")) {
    off = deskElement("p", "board-batch-off");
    list = deskElement("div", "board-batch-list");
    body.replaceChildren(off, list);
  }
  const resting = entries.filter((entry) => entry.on !== true).length;
  writeHidden(off, resting === 0);
  writeTextContent(off, resting === 0 ? "" : t("board.batch.off", "꺼짐: 열린 묶음 {{count}}개", { count: resting }));
  const held = new Map([...list.children].map((node) => [node.dataset.batch, node]));
  reconcileElementOrder(list, entries.map((entry) => {
    const card = held.get(`${entry.run}/${entry.batch.batch}`) ?? deskBatchCard(view);
    paintDeskBatchCard(card, entry, now);
    return card;
  }));
  return true;
}

/* 카드 하나: 이름과 칩, 멈춤의 한 줄, 과업들, 사실, 빠진 과업, 고른 쪽과 누른 쪽, 단추, 꺼진 단추의
 * 까닭, 누른 결과. 한 번 짓고 묶음의 키(`run/batch`)로 살아남는다. */
function deskBatchCard(view) {
  const card = deskElement("article", "board-batch");
  const top = deskElement("p", "board-batch-top");
  top.append(deskElement("code", "board-batch-name"), deskElement("strong", "board-batch-chip"));
  const acts = deskElement("div", "board-batch-acts");
  for (const act of BATCH_ACTS) {
    const button = deskElement("button", "board-desk-letter-act board-batch-act");
    button.type = "button";
    button.dataset.act = act;
    button.hidden = true;
    button.onclick = () => void pressLandBatch(view, card.__batchEntry, act);
    acts.append(button);
  }
  card.append(top, deskElement("p", "board-batch-why"), deskElement("ol", "board-batch-members"),
    deskElement("p", "board-batch-facts"), deskElement("p", "board-batch-dropped"), deskElement("p", "board-batch-chosen"),
    deskElement("p", "board-batch-pressed"), acts, deskElement("ul", "board-batch-held"), deskElement("p", "board-batch-said"));
  return card;
}

function writeBatchLine(node, text) {
  writeHidden(node, text === "");
  writeTextContent(node, text);
}

/* 꺼진 단추만 팁을 단다 — 켜진 단추의 빈 팁은 빈 말풍선이 된다. */
function writeBatchTip(button, tip) {
  if (tip) writeAttribute(button, "data-tip", tip);
  else if (button.hasAttribute("data-tip")) button.removeAttribute("data-tip");
}

function paintDeskBatchCard(card, entry, now) {
  const batch = entry.batch;
  const on = entry.on === true;
  const key = `${entry.run}/${batch.batch}`;
  card.__batchEntry = entry;
  writeAttribute(card, "data-batch", key);
  writeAttribute(card, "data-standing", on ? String(batch.standing ?? "") : "off");
  writeTextContent(card.querySelector(".board-batch-name"), String(batch.batch));
  const chipSaid = on ? BATCH_CHIPS[batch.standing] ?? null : null;
  const chip = card.querySelector(".board-batch-chip");
  writeHidden(chip, !chipSaid);
  writeTextContent(chip, chipSaid ? t(chipSaid.key, chipSaid.word, { task: batch.culprit?.task ?? "" }) : "");
  writeAttribute(chip, "data-tone", chipSaid?.tone ?? "");
  writeBatchLine(card.querySelector(".board-batch-why"), on ? batchWhy(batch) : "");
  const list = card.querySelector(".board-batch-members");
  const items = new Map([...list.children].map((node) => [node.dataset.task, node]));
  reconcileElementOrder(list, (batch.members ?? []).map((member) => {
    const item = items.get(String(member.task)) ?? deskElement("li", "");
    writeAttribute(item, "data-task", String(member.task));
    writeTextContent(item, String(member.task));
    return item;
  }));
  writeBatchLine(card.querySelector(".board-batch-facts"), on ? batchFacts(batch) : "");
  writeBatchLine(card.querySelector(".board-batch-dropped"), on ? batchDropped(batch) : "");
  writeBatchLine(card.querySelector(".board-batch-chosen"), on
    ? t("board.batch.chosen", "고른 쪽: {{who}} · {{at}}", { who: batchWho(batch.chosenBy), at: batchClock(batch.chosenAt, now) })
    : "");
  writeBatchLine(card.querySelector(".board-batch-pressed"), on ? batchPressed(batch, now) : "");
  const said = deskBatchSaid.get(key);
  const mine = said && (said.sending || (said.generation === batch.generation && said.standings.includes(batch.standing)))
    ? said : null;
  const shown = batchActs(batch, on);
  const groups = new Map();
  for (const button of card.querySelectorAll(".board-batch-act")) {
    const act = button.dataset.act;
    const stands = shown.includes(act);
    writeHidden(button, !stands);
    if (!stands) {
      writeTextContent(button, "");
      writeDisabled(button, true);
      writeBatchTip(button, "");
      continue;
    }
    const label = batchActLabel(batch, act);
    writeTextContent(button, label);
    const reason = batchHeldReason(batch, act);
    if (reason) groups.set(reason, [...(groups.get(reason) ?? []), label]);
    const held = reason ? BATCH_HELD[reason] : mine?.sending ? BATCH_HELD.sending : null;
    writeDisabled(button, held !== null);
    writeBatchTip(button, held ? t(held.key, held.word) : "");
  }
  const heldList = card.querySelector(".board-batch-held");
  const lines = [...groups].map(([reason, names]) => t("board.batch.heldLine", "{{acts}}: {{why}}",
    { acts: names.join(", "), why: t(BATCH_HELD[reason].key, BATCH_HELD[reason].word) }));
  writeHidden(heldList, lines.length === 0);
  reconcileElementOrder(heldList, lines.map((line, at) => {
    const item = heldList.children[at] ?? deskElement("li", "");
    writeTextContent(item, line);
    return item;
  }));
  const saidLine = card.querySelector(".board-batch-said");
  writeHidden(saidLine, !mine);
  writeTextContent(saidLine, mine ? mine.say() : "");
  writeAttribute(saidLine, "data-tone", mine ? mine.tone : "");
}

/* 카드의 단추. 사람이 확인 창에서 「예」라고 해야만 창의 명령을 부른다 — 명령은 주 창에서만 받고,
 * 눌린 때의 원장을 다시 읽어 코디의 동사와 같은 규칙으로 거절할 것은 거절한다. 보내는 동안 그 카드의
 * 단추는 다 잠긴다 — 두 번 눌러도 한 번이다. 답과 거절은 그 카드에 남고, 원장을 다시 읽는다. */
async function pressLandBatch(view, entry, act) {
  const started = handNow();
  const batch = entry?.batch;
  if (!batch) return;
  const key = `${entry.run}/${batch.batch}`;
  if (deskBatchSaid.get(key)?.sending) return;
  if (!batchActs(batch, entry.on === true).includes(act) || batchHeldReason(batch, act) !== null) return;
  const drop = act === "restack-drop" ? [batchDropTask(batch)] : [];
  const said = await askConfirm(batchAsk(batch, act, drop));
  if (said !== true) return;
  // The press's hand, joined by the answer's (shell-boot.js `flowHand`).
  const hand = flowHand(started);
  const held = {
    sending: true, tone: "sending", generation: batch.generation, standings: [batch.standing],
    say: () => t("board.batch.saidSending", "보내는 중…"),
  };
  deskBatchSaid.set(key, held);
  paintCoordinatorDesk(view);
  try {
    const run = entry.run;
    const answer = act === "promote" ? await invoke("land_batch_promote", { run, batch: batch.batch }, { hand })
      : act === "bisect" ? await invoke("land_batch_bisect", { run, batch: batch.batch }, { hand })
        : act === "cancel" ? await invoke("land_batch_cancel", { run, batch: batch.batch }, { hand })
          : await invoke("land_batch_restack", { run, batch: batch.batch, drop }, { hand });
    const words = batchAnswerWords(answer, batch);
    const generation = Number(answer?.generation ?? batch.generation);
    held.tone = words.tone;
    held.say = words.say;
    held.generation = generation;
    held.standings = [...(generation === batch.generation ? [batch.standing] : []), ...(BATCH_ANSWER_STANDINGS[answer?.state] ?? [])];
    refreshDeskLedger();
  } catch (error) {
    const why = String(error);
    held.tone = "refused";
    held.say = () => t("board.batch.saidRefused", "안 됨 — {{why}}", { why });
  } finally {
    held.sending = false;
    paintCoordinatorDesk(view);
  }
}

/* ---- 과업이 든 비용 (t-9470) --------------------------------------------------
 *
 * 끝난 과업 하나가 든 것 한 줄: 시도 수(재작업), 첫 시작부터 마지막 끝까지의 벽시계(대기
 * 포함), 생성 모델 토큰과 API 환산가, 그 과업 id가 찍힌 Jev 요청. 숫자는 백엔드의 것이고
 * (`task_cost`), 서식은 이미 있는 손들이다(`formatTokens`·`formatCost`·`usageDuration`).
 * 모르는 것은 「—」와 그 까닭으로 적고 추정으로 채우지 않는다. 데스크의 끝난 행과 작업
 * 보기의 과업 카드가 같은 한 줄을 쓴다. */

/* 달러가 숫자가 아닌 까닭, 백엔드의 낱말(`UsdReason`)마다. */
const TASK_COST_REASONS = Object.freeze({
  unsupported_agent: { key: "board.desk.cost.reasonUnsupported", word: "사용량 원장 없는 에이전트" },
  unscanned: { key: "board.desk.cost.reasonUnscanned", word: "끝난 뒤 읽은 사용량 스캔 없음 — 통계에서 읽으면 채워짐" },
  unlinked: { key: "board.desk.cost.reasonUnlinked", word: "잇지 못한 세션" },
  incomplete_session_history: { key: "board.desk.cost.reasonIncompleteHistory", word: "과거 세션 이력이 완전하지 않음" },
  invalid_usage: { key: "board.desk.cost.reasonInvalidUsage", word: "사용량 수치를 검증할 수 없음" },
  outside_attempt: { key: "board.desk.cost.reasonOutsideAttempt", word: "세션 사용량이 과업 시도 시간 안에만 속하는지 확인할 수 없음" },
  mixed_models: { key: "board.desk.cost.reasonMixed", word: "세션 중 모델이 바뀜" },
  unpriced_model: { key: "board.desk.cost.reasonUnpriced", word: "가격표에 없는 모델" },
  coordinator_conversation: { key: "board.desk.cost.reasonCoordinator", word: "코디 대화는 여러 과업에 걸쳐 있어 과업마다 나눌 수 없음" },
});

/* 과업을 누가 어떻게 이끌었는가(백엔드 `TaskShape`, t-73910 W3-1) — 비용 줄의 맨 앞 말. 직접 한 일은 비용을
 * 0으로 쓰지 않고 모른다고 쓴다. 기록에 없는 낱말·모양(옛 창이 보낸 답)은 말하지 않는다. */
function taskShapeWords(shape) {
  switch (shape?.kind) {
    case "direct": return t("board.desk.cost.shapeDirect", "직접(코디) — 비용 미확인");
    case "delegated": return t("board.desk.cost.shapeDelegated", "위임 {{count}}명", { count: Number(shape.workers) || 0 });
    case "unknown": return t("board.desk.cost.shapeUnknown", "미확인");
    default: return "";
  }
}

/* 다음 조치의 낱말(백엔드 `NextStep`, t-73910 W4-1) — 단계마다 무엇을 할지는 백엔드가 한 곳에서 정하고,
 * 여기는 그 낱말을 번역할 뿐이다. 표를 따로 두지 않는다: 모르는 낱말은 말하지 않는다. */
const DESK_NEXT_STEPS = Object.freeze({
  await_dependencies: { key: "board.desk.step.awaitDependencies", word: "선행 과업이 끝나길 기다리기" },
  summon: { key: "board.desk.step.summon", word: "워커를 부르거나 직접 하기" },
  wait: { key: "board.desk.step.wait", word: "워커 작업 기다리기" },
  decide: { key: "board.desk.step.decide", word: "결정에 답하기" },
  mend_dependency: { key: "board.desk.step.mendDependency", word: "선행 과업을 고치거나 이 과업 닫기" },
  review: { key: "board.desk.step.review", word: "코디 검증" },
  retry_or_close: { key: "board.desk.step.retryOrClose", word: "다시 맡기기 또는 닫기" },
  restart_or_close: { key: "board.desk.step.restartOrClose", word: "다시 띄우기 또는 닫기" },
  review_new_head: { key: "board.desk.step.reviewNewHead", word: "새 커밋 검증" },
  land: { key: "board.desk.step.land", word: "착지" },
  ship: { key: "board.desk.step.ship", word: "발행" },
  over: { key: "board.desk.step.over", word: "할 일 없음" },
  unknown: { key: "board.desk.step.unknown", word: "확인 불가" },
});

/* 과업 행·코디 카드의 다음 조치 한 줄. 색이 아니라 글이다. */
function deskNextStepWords(task) {
  const step = DESK_NEXT_STEPS[task?.next_step];
  return step ? t("board.desk.step.line", "다음 조치: {{step}}", { step: t(step.key, step.word) }) : "";
}

/* 비용 한 줄과 그 팁 — 백엔드가 끝난 과업에 실어 보낸 비용 하나에서. */
function taskCostWords(cost) {
  const generation = cost.generation ?? {};
  const jev = cost.jev ?? {};
  const wall = Number.isFinite(cost.wallMs) ? usageDuration(Math.max(1, cost.wallMs)) : null;
  const completed = Number.isFinite(cost.completionWallMs)
    ? usageDuration(Math.max(1, cost.completionWallMs)) : null;
  const tokens = (Number(generation.inputTokens) || 0) + (Number(generation.outputTokens) || 0) +
    (Number(generation.cacheReadTokens) || 0) + (Number(generation.cacheWriteTokens) || 0);
  const reason = TASK_COST_REASONS[generation.usdReason];
  const shape = taskShapeWords(cost.shape);
  const jevWords = t("board.desk.cost.jev", "Jev 요청 {{count}}", { count: Number(jev.requests) || 0 });
  // A task nobody attempted has no attempts, wall clock or tokens to count, and no dollar to divide out of the
  // coordinator's conversation: its line says so rather than a row of dashes.
  const ownLine = cost.shape?.kind === "direct" || cost.shape?.kind === "unknown";
  const text = ownLine ? [shape, jevWords].join(" · ") : [
    ...(shape ? [shape] : []),
    t("board.desk.cost.attempts", "시도 {{count}}", { count: Number(cost.attempts) || 0 }),
    wall ? t("board.desk.cost.wall", "{{time}}(대기 포함)", { time: wall }) : t("board.desk.cost.wallOpen", "벽시계 —"),
    Number(generation.sessionsLinked) > 0
      ? t("board.desk.cost.tokens", "생성 {{tokens}} 토큰", { tokens: formatTokens(tokens) })
      : t("board.desk.cost.tokensNone", "생성 —"),
    Number.isFinite(generation.usd) ? formatCost(generation.usd)
      : t("board.desk.cost.usdUnknown", "$— {{reason}}", { reason: reason ? t(reason.key, reason.word) : "" }).trim(),
    jevWords,
  ].join(" · ");
  const seats = { stamped: Number(jev.stampedSeats) || 0, unstamped: Number(jev.unstampedSeats) || 0 };
  const tip = [
    completed
      ? t("board.desk.cost.tipCompletion", "첫 시도부터 검증·반영 확인까지 {{time}}", { time: completed })
      : t("board.desk.cost.tipCompletionUnknown", "검증·반영 완료 시간 미기록 — 시도 종료 시간과 다름"),
    ...(cost.shape?.kind === "direct" ? [t(TASK_COST_REASONS.coordinator_conversation.key, TASK_COST_REASONS.coordinator_conversation.word)] : []),
    ...(cost.shape?.kind === "delegated" && Number(cost.shape.overlapMs) > 0
      ? [t("board.desk.cost.tipOverlap", "시도 둘 이상이 함께 열려 있던 겹친 시간 {{time}}", { time: usageDuration(cost.shape.overlapMs) })] : []),
    ...(ownLine ? [] : [
      t("board.desk.cost.tipSessions", "생성은 시도별 기록된 세션의 합 · 이은 세션 {{linked}}/{{known}}",
        { linked: Number(generation.sessionsLinked) || 0, known: Number(generation.sessionsKnown) || 0 }),
      ...(generation.historyComplete === false ? [t("board.desk.cost.reasonIncompleteHistory", "과거 세션 이력이 완전하지 않음")] : []),
      t("board.desk.cost.tipUsd", "API 환산가(구독 사용자는 청구액 아님)"),
    ]),
    Number.isFinite(jev.inputTokens)
      ? t("board.desk.cost.tipJevTokens", "Jev 요청은 스탬프 좌석 {{stamped}}개만 셈 — 나머지 {{unstamped}}좌석과 zo 원장은 과업 id가 없음 · 입력 {{tokens}} 토큰",
        { ...seats, tokens: formatTokens(jev.inputTokens) })
      : t("board.desk.cost.tipJev", "Jev 요청은 스탬프 좌석 {{stamped}}개만 셈 — 나머지 {{unstamped}}좌석과 zo 원장은 과업 id가 없음 · 토큰 미기록", seats),
  ].join(" · ");
  return { text, tip };
}

/* ---- 남긴 것 (t-32798) ------------------------------------------------------
 *
 * 워커가 worker_done으로 밝힌 보고서와 증거는 체크아웃과 함께 사라지기 전에 원장이 창의
 * 아티팩트 저장소에 가려서 남긴다(`hand_in_keep`). 과업 행과 워커 행은 그 기록에서 한 번
 * 센 사실 하나(`kept`)를 달고 온다 — 여기는 그 말을 풀어 쓸 뿐이다. 무엇을 남겼고 무엇을
 * 못 담았는지, 정리를 막는지도 백엔드가 정한다. 못 담은 것에는 이유가 낱말로 붙어 오고, 낱말마다
 * 한 문장이다. 남긴 파일은 아티팩트 탭의 같은 행이라, 문은 그 탭을 이 과업으로 걸러 연다. */

/* 상태 낱말(백엔드 `Facts::state`), 낱말마다 한 문장. 모르는 낱말은 그대로 쓴다. */
const KEPT_STATES = Object.freeze({
  keeping: { key: "board.desk.kept.keeping", word: "보관하는 중…" },
  kept: { key: "board.desk.kept.kept", word: "보관됨 · {{count}}개 · {{size}}" },
  partial: { key: "board.desk.kept.partial", word: "일부만 보관 · {{count}}개 보관 · {{left}}개 못 담음" },
  withheld: { key: "board.desk.kept.withheld", word: "보관한 것 없음 · {{left}}개 못 담음" },
  failed: { key: "board.desk.kept.failed", word: "보관 실패 · 정리가 막혀 있음" },
});

/* 못 담은 이유(백엔드 `Why::word`와 `Fault::word`), 낱말마다 한 문장. 한도는 백엔드가 실어 보낸
 * 숫자(`file_cap_bytes`·`cap_bytes`)다. */
const KEPT_REASONS = Object.freeze({
  over_file_cap: { key: "board.desk.kept.reasonOverFileCap", word: "파일 하나가 {{fileCap}} 초과" },
  over_total_cap: { key: "board.desk.kept.reasonOverTotalCap", word: "한 번에 {{totalCap}}까지만 보관" },
  too_many_files: { key: "board.desk.kept.reasonTooManyFiles", word: "파일이 너무 많음" },
  not_kept: { key: "board.desk.kept.reasonNotKept", word: "글도 그림도 아닌 파일은 못 읽어 보관 안 함" },
  secret_name: { key: "board.desk.kept.reasonSecretName", word: "비밀 파일 이름이라 보관 안 함" },
  outside_roots: { key: "board.desk.kept.reasonOutsideRoots", word: "작업 폴더 밖의 파일이라 보관 안 함" },
  link: { key: "board.desk.kept.reasonLink", word: "바로가기는 따라가지 않음" },
  unsafe: { key: "board.desk.kept.reasonUnsafe", word: "가려도 비공개 값이 남아 보관 안 함" },
  gone: { key: "board.desk.kept.reasonGone", word: "원본이 이미 없음" },
  unreadable: { key: "board.desk.kept.reasonUnreadable", word: "원본을 읽을 수 없음" },
  remote: { key: "board.desk.kept.reasonRemote", word: "다른 기계의 파일" },
  copy_failed: { key: "board.desk.kept.reasonCopyFailed", word: "보관소에 쓰지 못함" },
  store_unavailable: { key: "board.desk.kept.reasonStoreUnavailable", word: "보관소가 아직 열리지 않음" },
});

/* 한 줄과 그 팁 — 백엔드가 과업·워커 행에 실어 보낸 사실 하나에서. 사실이 없으면 `null`이고
 * 부르는 쪽이 줄을 숨긴다. 이유는 줄에 그대로 서서, 막힌 정리의 까닭이 팁 뒤에 숨지 않는다. */
function keptWords(kept) {
  if (!kept || typeof kept !== "object") return null;
  const state = KEPT_STATES[kept.state];
  const head = state
    ? t(state.key, state.word, {
      count: Number(kept.kept) || 0, left: Number(kept.left_out) || 0, size: artifactBytesWord(Number(kept.bytes) || 0),
    })
    : String(kept.state);
  const caps = { fileCap: artifactBytesWord(Number(kept.file_cap_bytes) || 0), totalCap: artifactBytesWord(Number(kept.cap_bytes) || 0) };
  const reasons = (Array.isArray(kept.reasons) ? kept.reasons : []).map((word) => {
    const said = KEPT_REASONS[word];
    return said ? t(said.key, said.word, caps) : String(word);
  });
  const masked = Number(kept.masked) > 0
    ? t("board.desk.kept.masked", "가린 값 {{count}}개", { count: Number(kept.masked) }) : "";
  return {
    state: String(kept.state),
    text: [head, masked, ...reasons].filter(Boolean).join(" · "),
    tip: [
      t("board.desk.kept.tip", "체크아웃이 지워져도 남도록 아티팩트 저장소에 복사한 보고서와 증거입니다. 글은 홈 경로·메일·사설 주소·자격 증명을 가린 사본이고, 그림은 글자를 읽지 못해 그대로입니다."),
      kept.detail ? String(kept.detail) : "",
    ].filter(Boolean).join(" "),
  };
}

/* 줄 하나를 사실에 맞게 쓴다 — 바뀐 것만 쓰는 손(`write*`)으로, 조용한 박자가 아무것도 다시 쓰지
 * 않게. `task`가 있으면 줄의 문이 선다: 아티팩트 탭을 이 과업으로 걸러 열어, 남긴 보고서를 고른다. */
function paintDeskKept(line, kept, task) {
  /* 워커 행은 깨끗이 남긴 것(`kept`)을 말하지 않는다 — 그 말은 과업 행에 서 있고, 행마다 한 줄을 더하면 데스크가
   * 길어져 「첫 과업 행은 첫 화면」(t-6588)이 깨진다. 보관하는 중·일부만·없음·실패만 말한다. */
  const words = keptWords(task === null && kept?.state === "kept" ? null : kept);
  writeHidden(line, words === null);
  if (words === null) return;
  writeAttribute(line, "data-state", words.state);
  writeAttribute(line, "data-tip", words.tip);
  writeTextContent(line.firstElementChild, words.text);
  const door = line.querySelector(".board-desk-kept-door");
  if (!door) return;
  writeHidden(door, !(Number(kept.kept) > 0));
  writeTextContent(door, t("board.desk.kept.open", "열기"));
  writeAttribute(door, "aria-label", t("board.desk.kept.openAria", "보관한 보고서와 증거를 아티팩트에서 열기"));
  door.__task = task;
  door.__report = kept.report ?? null;
}

/* 한 줄의 뼈대: 글 칸 하나, 그리고 문이 있는 줄이면 눌리는 단추 하나. 한 번 짓고 박자마다 옷만 갈아입는다. */
function deskKeptLine(className, withDoor) {
  const line = deskElement("span", className);
  line.hidden = true;
  line.append(deskElement("span", "board-desk-kept-text"));
  if (withDoor) {
    const door = deskElement("button", "board-desk-kept-door");
    door.type = "button";
    door.onclick = () => {
      const task = door.__task;
      if (!task) return;
      leavePagesForStage();
      openArtifacts({ origin: { field: "task", value: task.id, label: task.title }, select: door.__report });
    };
    line.append(door);
  }
  return line;
}

/* ---- 과업 흐름 --------------------------------------------------------------
 *
 * `task-list`의 자리: 판 위의 런이 적어 둔 과업을 단계 하나씩으로 센다 —
 * 선행 대기 → 준비 → 진행 → 보고됨 → 병합, 그리고 멈춰 선 셋(게이트·막힘·실패).
 * 단계를 정하는 것은 원장의 사실이고(`desk::stage_of`) 수도 백엔드가 센다; 여기는
 * 그 수를 그리고, 누른 단계의 과업을 펼친다. 멈춰 선 단계에 과업이 있으면 처음부터
 * 펼쳐져 있다 — 코디네이터가 가장 먼저 물은 것이 그것이다. */

/* 단계의 낱말과 색, 백엔드의 `STAGES` 순서 그대로. `flow`는 흐름의 화살표 위에
 * 서는 단계다. 멈춘 단계의 색은 보드의 어휘 그대로: 결정·막힘은 기다림, 실패는 멈춤.
 * 검증 대기(`보고됨`)에는 코디네이터가 아직 검토할 수 있는 일만 든다 — 모든 시도가
 * 아무것도 넘기지 않고 끝난 완료 과업은 검토 기록이 영영 적힐 수 없어, 원장이 그렇게
 * 읽어 보낸 대로 제 단계(`unreviewable`)에 선다. */
const DESK_STAGES = Object.freeze([
  { id: "pending", flow: true, tone: "", key: "board.desk.stagePending", word: "선행 대기" },
  { id: "ready", flow: true, tone: "", key: "board.desk.stageReady", word: "준비" },
  { id: "dispatched", flow: true, tone: "", key: "board.desk.stageDispatched", word: "진행" },
  { id: "reported", flow: true, tone: "", key: "board.desk.stageReported", word: "보고됨" },
  { id: "verified", flow: true, tone: "", key: "board.verified", word: "검증됨" },
  { id: "merged", flow: true, tone: "", key: "board.desk.stageMerged", word: "병합" },
  { id: "deployed", flow: true, tone: "", key: "board.deployed", word: "배포됨" },
  { id: "gate", flow: false, tone: "wait", key: "board.desk.stageGate", word: "게이트" },
  { id: "blocked", flow: false, tone: "wait", key: "board.desk.stageBlocked", word: "막힘" },
  { id: "stale", flow: false, tone: "wait", key: "board.staleReview", word: "다시 검토 대기" },
  { id: "failed", flow: false, tone: "halt", key: "board.desk.stageFailed", word: "실패" },
  { id: "unreviewable", flow: false, tone: "", key: "board.noReviewRecord", word: "종료 — 검토 자료 없음" },
  { id: "waived", flow: false, tone: "", key: "board.reviewWaived", word: "종료 — 검토 생략(사람)" },
  { id: "nothing_to_land", flow: false, tone: "", key: "board.nothingToLand", word: "완료 — 착지할 것 없음" },
  { id: "unknown", flow: false, tone: "", key: "board.stageUnknown", word: "상태 모름", evidence: true },
  { id: "closed", flow: false, tone: "", key: "board.closed", word: "닫힘" },
]);

/* 흐름을 떠난 단계: 닫힘, 검토 기록을 영영 받을 수 없어 완료로만 남은 것(t-19328), 사람이 검토를
 * 생략한 것(t-71220). 「과업 흐름 · N」의 N은 아직 흐름에 있는 과업만 센다. */
const DESK_OVER_STAGES = Object.freeze(["unreviewable", "waived", "nothing_to_land", "unknown", "closed"]);

/* 처음 펼쳐 둘 단계: 멈춰 선 단계 가운데 과업이 있는 첫째. 없으면 아무것도. */
function deskDefaultStage(counts) {
  return DESK_STAGES.find((stage) => !stage.flow && stage.tone !== "" && (counts.get(stage.id) ?? 0) > 0)?.id ?? null;
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
  writeTextContent(chip.lastElementChild, stage.evidence && !Number.isInteger(counts.get(stage.id)) ? "—" : String(count));
  return chip;
}

/* ---- 통과 조건과 넘김 메모 (t-26587) ----------------------------------------------
 * 백엔드(`hand_in`)가 과업의 마지막 worker_done에서 읽어 준 것을 그린다: 조건마다 번호·글·상태
 * (증거 없음·주장·확인됨), 그리고 넘김 글이 남긴 메모(결정·막힌 것·다음). 세지도 판정하지도
 * 않는다 — 상태는 원장이 정했고, 여기서는 그 낱말만 고른다. */
const DESK_CONDITION_WORDS = {
  missing: { key: "board.desk.condition.missing", word: "증거 없음" },
  claimed: { key: "board.desk.condition.claimed", word: "주장" },
  checked: { key: "board.desk.condition.checked", word: "확인됨" },
};

function paintDeskConditions(list, conditions) {
  writeHidden(list, conditions.length === 0);
  // Rebuilt only when the rows changed: writing the same rows again would be a mutation every quiet poll leaves behind.
  const signature = conditions.map((condition) => `${condition.number}|${condition.state}|${condition.text}`).join("\n");
  if (list.__signature === signature) return;
  list.__signature = signature;
  if (conditions.length === 0) {
    list.replaceChildren();
    return;
  }
  writeAttribute(list, "aria-label", t("board.desk.conditions", "통과 조건 · {{count}}", { count: conditions.length }));
  list.replaceChildren(...conditions.map((condition) => {
    const item = deskElement("li", "board-desk-condition");
    writeAttribute(item, "data-state", condition.state);
    const text = deskElement("span", "board-desk-condition-text");
    writeTextContent(text, `${condition.number}. ${condition.text}`);
    const mark = deskElement("span", "board-desk-condition-state");
    const word = DESK_CONDITION_WORDS[condition.state];
    writeTextContent(mark, word ? t(word.key, word.word) : String(condition.state));
    item.append(text, mark);
    return item;
  }));
}

function deskHandInWords(handIn) {
  if (!handIn) return "";
  return [
    handIn.decisions ? t("board.desk.handIn.decisions", "결정: {{text}}", { text: handIn.decisions }) : "",
    handIn.blocked ? t("board.desk.handIn.blocked", "막힌 것: {{text}}", { text: handIn.blocked }) : "",
    handIn.next ? t("board.desk.handIn.next", "다음: {{text}}", { text: handIn.next }) : "",
  ].filter(Boolean).join(" · ");
}

const DESK_TASK_BLOCKERS = Object.freeze(["gone", "walled", "asking", "asleep"]);

function deskTaskCarrier(task) {
  let carrier = null;
  for (const row of deskAgents) {
    if (row.run !== task.run || row.task_id !== task.id) continue;
    if (row.dispatch_id === task.progress?.attempt) return row;
    if (!carrier || (Number(row.dispatch_started_ms) || 0) > (Number(carrier.dispatch_started_ms) || 0)) carrier = row;
  }
  return carrier;
}

function deskTaskWorkWords(progress) {
  const short = (sha) => (COMMIT_ID.test(sha ?? "") ? sha.slice(0, SHORT_COMMIT_LENGTH) : "");
  const source = short(progress?.source);
  if (progress?.stage === "stale") {
    const reviewed = short(progress.reviewed);
    return reviewed || source
      ? t("board.desk.workStale", "검토한 커밋 {{reviewed}} · 지금 커밋 {{source}}", { reviewed: reviewed || "—", source: source || "—" })
      : "";
  }
  if (source === "") return "";
  const said = WORK_STAGES[progress.stage]?.phase === "vouched"
    ? t("board.desk.workReviewed", "검토한 커밋 {{sha}}", { sha: source })
    : t("board.desk.workHandedIn", "넘긴 커밋 {{sha}}", { sha: source });
  const merged = short(progress.merge_head);
  return merged && merged !== source ? `${said} · ${t("board.desk.workMerged", "병합 커밋 {{sha}}", { sha: merged })}` : said;
}

function deskTaskBlocker(task, health) {
  if (task.stage !== "dispatched" && task.stage !== "failed") return "";
  if (health && DESK_TASK_BLOCKERS.includes(health.id)) return t(health.key, health.word);
  return task.stage === "failed" ? t("board.desk.causeUnknown", "원인 확인 필요") : "";
}

function deskTaskOwnerWords(task, carrier, health) {
  const cause = deskTaskBlocker(task, health);
  return [
    carrier ? t("board.desk.taskOwner", "담당 {{worker}} · {{agent}}", { worker: carrier.worker, agent: agentName(carrier.agent) }) : "",
    cause ? t("board.desk.taskBlocker", "막힘: {{cause}}", { cause }) : "",
  ].filter(Boolean).join(" · ");
}

function deskTaskNextLine() {
  const line = deskElement("p", "board-desk-task-next");
  const copy = deskElement("button", "board-desk-task-next-copy");
  copy.type = "button";
  copy.dataset.keyboardOwner = "true";
  copy.onclick = async () => {
    const command = line.__command;
    if (!command || !(await clipboardText.write(command))) return;
    copy.__copied = true;
    writeTextContent(copy, t("vault.copied", "명령을 복사했습니다"));
    setTimeout(() => {
      copy.__copied = false;
      writeTextContent(copy, t("vault.copy", "명령 복사"));
    }, VAULT_COPY_FEEDBACK_MS);
  };
  line.append(deskElement("span", "board-desk-task-next-label"), deskElement("code", "board-desk-task-next-command"), copy);
  return line;
}

function paintDeskTaskNext(line, task, health) {
  const asking = task.stage === "dispatched" && health?.id === "asking";
  const command = asking ? "" : task.progress?.next ?? "";
  const [label, code, copy] = line.children;
  line.__command = command;
  writeTextContent(label, asking ? t("board.desk.nextAnswer", "「지금」의 당신 차례에서 질문에 답하세요") : t("board.desk.taskNext", "다음"));
  writeTextContent(code, command);
  writeHidden(code, command === "");
  if (!copy.__copied) writeTextContent(copy, t("vault.copy", "명령 복사"));
  writeHidden(copy, command === "");
  writeHidden(line, command === "" && !asking);
}

/* 과업 하나의 줄들 — 무엇이 막는가, 어느 커밋인가, 누가 들었는가, 다음 명령, 든 비용, 글 점검, 통과
 * 조건, 넘김 메모, 남긴 것. 과업 흐름의 행과 「오늘 끝난 일」 표의 펼침(t-70264)이 같은 줄을 같은
 * 손으로 그린다. 한 번 짓고, 줄들은 그 그릇에 이름으로 매달린다(`__lines`). */
function deskTaskLines(host) {
  const lines = {
    note: deskElement("span", "board-desk-task-note"), work: deskElement("span", "board-desk-task-work"),
    owner: deskElement("span", "board-desk-task-owner"), step: deskElement("span", "board-desk-task-step"),
    next: deskTaskNextLine(),
    cost: deskElement("span", "board-desk-task-cost"), writing: deskElement("span", "board-desk-task-writing"),
    conditions: deskElement("ol", "board-desk-task-conditions"), handIn: deskElement("p", "board-desk-task-handin"),
    kept: deskKeptLine("board-desk-task-kept", true),
  };
  host.append(...Object.values(lines));
  host.__lines = lines;
  return host;
}

function deskTaskRow(held, task, runs, now) {
  const row = held ?? deskTaskLines(deskElement("li", "board-desk-task"));
  if (!held) row.prepend(deskElement("code", "board-desk-task-id"), deskElement("span", "board-desk-task-title"));
  writeAttribute(row, "data-task", `${task.run}/${task.id}`);
  writeTextContent(row.firstElementChild, task.id);
  writeTextContent(row.children[1], runs > 1 ? `${task.title} · ${task.run}` : task.title);
  paintDeskTaskLines(row, task, now);
  return row;
}

function paintDeskTaskLines(host, task, now) {
  const { note: noteLine, work: workLine, owner: ownerLine, step: stepLine, next: nextLine, cost: costLine, writing: writingLine,
    conditions: conditionsLine, handIn: handInLine, kept: keptLine } = host.__lines;
  const note = task.gate
    ? t("board.desk.taskGate", "{{gate}} · {{question}}", { gate: task.gate.id, question: task.gate.question })
    : task.closed
      ? [ledgerClosedReason(task.closed), task.closed.why].filter(Boolean).join(" · ")
    : task.blocked_by?.length
      ? t("board.desk.taskBlockedBy", "실패한 선행: {{tasks}}", { tasks: task.blocked_by.join(", ") })
      : "";
  writeTextContent(noteLine, note);
  writeHidden(noteLine, note === "");
  const work = deskTaskWorkWords(task.progress);
  writeTextContent(workLine, work);
  writeHidden(workLine, work === "");
  const carrier = deskTaskCarrier(task);
  const health = carrier ? deskWorkerHealth(carrier, null, now) : null;
  const owner = deskTaskOwnerWords(task, carrier, health);
  writeTextContent(ownerLine, owner);
  writeHidden(ownerLine, owner === "");
  const step = deskNextStepWords(task);
  writeTextContent(stepLine, step);
  writeAttribute(stepLine, "data-step", task.next_step ?? "");
  writeHidden(stepLine, step === "");
  paintDeskTaskNext(nextLine, task, health);
  // A finished row carries what its task cost (t-9470); a moving one carries none.
  const cost = task.cost && typeof task.cost === "object" ? taskCostWords(task.cost) : null;
  writeTextContent(costLine, cost?.text ?? "");
  writeAttribute(costLine, "data-tip", cost?.tip ?? "");
  writeHidden(costLine, cost === null);
  // 일꾼이 쓴 요약을 센 수 한 줄(t-32786): 끝난 행에만 서고, 센 수가 없으면 숨는다.
  const writing = writingBadgeWords(task.writing);
  writeTextContent(writingLine, writing?.text ?? "");
  writeAttribute(writingLine, "data-tip", writing?.tip ?? "");
  writeHidden(writingLine, writing === null);
  // 과업의 통과 조건과 마지막 넘김 글의 메모(t-26587): 원장이 읽어 준 것을 그대로 그린다.
  paintDeskConditions(conditionsLine, task.hand_in?.conditions ?? []);
  const handIn = deskHandInWords(task.hand_in);
  writeTextContent(handInLine, handIn);
  writeHidden(handInLine, handIn === "");
  // 워커가 밝힌 보고서와 증거를 원장이 남겼으면 한 줄과 그것을 여는 문(t-32798).
  paintDeskKept(keptLine, task.kept, task);
}

function paintDeskPipeline(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.stages) || ledger.runs?.length === 0) return false;
  const counts = new Map(ledger.stages.map((one) => [one.stage, one.count]));
  // A task that is over — closed, or completed with nothing a coordinator can ever
  // review — is not in play: the pipeline's count is what still is.
  const total = [...counts].reduce((sum, [stage, count]) => sum + (DESK_OVER_STAGES.includes(stage) ? 0 : count), 0);
  const over = DESK_OVER_STAGES.reduce((sum, stage) => sum + (counts.get(stage) ?? 0), 0);
  if (total === 0 && over === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  const body = block.lastElementChild;
  let strip = body.querySelector(":scope > .board-desk-stages");
  if (!strip) {
    strip = deskElement("div", "board-desk-stages");
    strip.setAttribute("role", "group");
    // The flow and the stuck stages are two runs of chips that wrap as one
    // line: the stuck run starts beside the flow's last chip, one space on.
    strip.append(deskElement("span", "board-desk-stage-run is-flow"), deskElement("span", "board-desk-stage-run"));
    body.replaceChildren(strip, deskElement("ol", "board-desk-tasks"), deskElement("p", "board-desk-more"));
  }
  writeAttribute(strip, "aria-label", t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  let open = deskChoice.stage === undefined ? deskDefaultStage(counts) : deskChoice.stage;
  if (open && (counts.get(open) ?? 0) === 0) open = deskDefaultStage(counts);
  const [flowRun, stuckRun] = strip.children;
  for (const [run, flow] of [[flowRun, true], [stuckRun, false]]) {
    const chips = DESK_STAGES.filter((stage) => stage.flow === flow && !(stage.evidence && counts.get(stage.id) === 0))
      .map((stage) => {
        const chip = deskStageChip(run, stage, counts, view);
        writeAttribute(chip, "aria-pressed", String(open === stage.id));
        return chip;
      });
    for (const gone of [...run.children]) if (!chips.includes(gone)) gone.remove();
    reconcileElementOrder(run, chips);
  }
  const list = body.querySelector(".board-desk-tasks");
  const rows = open ? (ledger.tasks ?? []).filter((task) => task.stage === open) : [];
  const held = new Map([...list.children].map((node) => [node.dataset.task, node]));
  const runs = ledger.runs?.length ?? 1;
  reconcileElementOrder(list, rows.map((task) => deskTaskRow(held.get(`${task.run}/${task.id}`), task, runs, now)));
  writeHidden(list, rows.length === 0);
  const more = body.querySelector(".board-desk-more");
  const hidden = open ? (counts.get(open) ?? 0) - rows.length : 0;
  writeTextContent(more, hidden > 0 ? t("board.desk.more", "{{count}}개 더 — 원장에 있음", { count: hidden }) : "");
  writeHidden(more, hidden <= 0);
  return true;
}

/* ---- 오늘 끝난 일과 누적 (시안 B의 기록 탭, t-70264) -----------------------------
 *
 * 기록 탭의 첫 둘은 시안 그대로다. 「오늘 끝난 일」은 오늘 끝난 과업 — 착지했거나 착지할 것 없이
 * 끝난 것(`boardFinishedAt`) — 의 표다(과업 · 결과 · 걸린 시간 · 시각, 늦게 끝난 것이 위). 「지금」
 * 표의 발이 말하는 그 수의 행들이다(`boardFinishedTodayTasks`). 행을 누르면 그 과업의 줄들(커밋·
 * 비용·글 점검·통과 조건·넘김 메모·남긴 것)이 펼쳐진다: 과업 흐름의 행과 같은 손(`deskTaskLines`)이고,
 * 처음 펼칠 때 짓는다. 「누적」은 첫 화면에서 옮겨 온, 흐름을 떠난 과업의 수다. */
const DESK_TODAY_COLUMNS = Object.freeze([
  { id: "task", key: "board.history.colTask", word: "과업" },
  { id: "result", key: "board.history.colResult", word: "결과" },
  { id: "took", key: "board.history.colTook", word: "걸린 시간", number: true, wide: true },
  { id: "at", key: "board.history.colAt", word: "시각", number: true },
]);

/* 끝난 과업의 결과 — 시안의 말과 점: 착지한 과업은 다음 발행을 기다리고 발행된 과업은 발행됨(둘 다
 * 끝남의 청록). 그 밖의 단계(착지할 것 없이 끝난 과업)는 과업 흐름의 칩과 같은 말에 회색 점이다
 * (`deskStageWord`). */
const DESK_TODAY_RESULTS = Object.freeze({
  merged: { dot: "done", key: "board.history.landed", word: "착지 · 다음 발행에" },
  deployed: { dot: "done", key: "board.now.doneShipped", word: "발행됨" },
});

function deskTodayRow(view) {
  const group = deskElement("tbody", "board-today-task");
  const row = deskElement("tr", "board-today-row");
  const fold = deskElement("button", "board-today-fold");
  fold.append(deskElement("span", "board-today-name"), document.createTextNode(" "), deskElement("code", "board-today-id"));
  deskTableFold(view, group, row, fold, deskChoice.todayOpen, "task");
  const [task, result, took, at] = DESK_TODAY_COLUMNS;
  const what = deskTableCell("td", task, "board-today-what");
  what.append(fold);
  const said = deskTableCell("td", result, "board-today-result");
  said.append(deskElement("span", "board-today-word board-state"));
  row.append(what, said, deskTableCell("td", took, "board-today-took"), deskTableCell("td", at, "board-today-at"));
  group.append(row, deskTableMore("board-today-more", DESK_TODAY_COLUMNS));
  return group;
}

function paintDeskToday(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.tasks)) return false;
  writeTextContent(block.firstElementChild, t("board.history.today", "오늘 끝난 일"));
  const table = deskTableBlock(block, "board-today", DESK_TODAY_COLUMNS);
  const card = table.parentElement;
  writeTextContent(card.previousElementSibling, t("board.history.todayNote", "끝나는 대로 위에 쌓입니다."));
  const empty = card.querySelector(":scope > .board-today-empty") ?? card.appendChild(deskElement("p", "board-today-empty"));
  const tasks = boardFinishedTodayTasks(ledger, now);
  const runs = ledger.runs?.length ?? 1;
  const held = new Map([...table.tBodies].map((node) => [node.dataset.task, node]));
  const keys = new Set();
  reconcileElementOrder(table, [table.tHead, ...tasks.map((task) => {
    const key = `${task.run}/${task.id}`;
    keys.add(key);
    const node = held.get(key) ?? deskTodayRow(view);
    writeAttribute(node, "data-task", key);
    const open = deskChoice.todayOpen.has(key);
    paintDeskTableFold(node, open);
    writeTextContent(node.querySelector(".board-today-name"), runs > 1 ? `${task.title} · ${task.run}` : task.title);
    writeTextContent(node.querySelector(".board-today-id"), task.id);
    const result = DESK_TODAY_RESULTS[task.stage];
    const word = node.querySelector(".board-today-word");
    writeAttribute(word, "data-dot", result?.dot ?? "queue");
    writeTextContent(word, result ? t(result.key, result.word) : deskStageWord(task.stage));
    const started = Number(task.times?.started_ms) || 0;
    const finished = boardFinishedAt(task);
    writeTextContent(node.querySelector(".board-today-took"),
      started > 0 && finished > started ? deskSpanWords(finished - started) : "—");
    writeTextContent(node.querySelector(".board-today-at"), boardClock(finished, now));
    if (open) paintDeskTaskLines(deskTableDetail(node, () => deskTaskLines(deskElement("div", "board-today-detail"))), task, now);
    return node;
  })]);
  // A task that left today's list takes its opened lines with it.
  for (const key of deskChoice.todayOpen) if (!keys.has(key)) deskChoice.todayOpen.delete(key);
  writeHidden(table, tasks.length === 0);
  writeTextContent(empty, t("board.now.runsTodayNone", "오늘 끝난 일은 아직 없습니다"));
  writeHidden(empty, tasks.length > 0);
  return true;
}

/* 누적 — 배포됨과 흐름을 떠난 단계(`DESK_OVER_STAGES`)의 과업 수: 시안의 셋(배포 · 종료 · 닫힘)에 그 뒤
 * 제 단계가 된 「완료 — 착지할 것 없음」(t-34501)과 「종료 — 검토 생략(사람)」(t-71220)이 더해진 것이다.
 * 상태 모름은 증거라 빠진다. 목록을 그 단계표에서 끌어 써서, 새 끝 단계가 생기면 누적에도 저절로 선다.
 * 수는 원장이 센 그대로(`stages`)이고 낱말은 과업 흐름의 칩과 같은 말이다. 원장이 세지 않은 단계는
 * 0이 아니라 「—」. */
const DESK_TOTAL_STAGES = Object.freeze(["deployed",
  ...DESK_OVER_STAGES.filter((stage) => !DESK_STAGES.find((one) => one.id === stage)?.evidence)]);

function paintDeskTotals(block) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.stages)) return false;
  const counts = new Map(ledger.stages.map((one) => [one.stage, one.count]));
  const head = block.firstElementChild;
  if (!head.firstElementChild) {
    head.append(deskElement("span", "board-desk-head-word"), deskElement("span", "board-desk-head-note"));
  }
  writeTextContent(head.firstElementChild, t("board.history.totals", "누적"));
  writeTextContent(head.lastElementChild,
    t("board.history.totalsNote", "누적 수는 첫 화면에서 이리로 옮겼습니다 — 지금 일과 겨루지 않게."));
  const body = block.lastElementChild;
  const stats = body.firstElementChild ?? body.appendChild(deskElement("dl", "board-totals"));
  const held = new Map([...stats.children].map((node) => [node.dataset.stage, node]));
  reconcileElementOrder(stats, DESK_TOTAL_STAGES.map((stage) => {
    let node = held.get(stage);
    if (!node) {
      node = deskElement("div", "board-total");
      node.dataset.stage = stage;
      node.append(deskElement("dt", "board-total-word"), deskElement("dd", "board-total-count"));
    }
    const count = counts.get(stage);
    writeTextContent(node.firstElementChild, deskStageWord(stage));
    writeTextContent(node.lastElementChild, Number.isInteger(count) ? String(count) : "—");
    return node;
  }));
  return true;
}

/* ---- 릴리즈 레인 ----------------------------------------------------------
 *
 * `tools/release/status.sh`가 읽던 그 파일(레인의 `status.json`, 상태 바의
 * `releaseLane`): sha·버전·단계·경과·판정, 그리고 건너뛴 단계의 사유. 판정은
 * 레인이 적은 `outcome` 그대로 — 끝나지 않은 레인은 진행 중이고, 여기서 초록을
 * 지어내지 않는다. */

/* 시안 B의 카드 (t-70028): 첫 줄은 「새 버전 발행 v1.1.84 … ● 도는 중」, 둘째 줄은 커밋과
 * 차례(「네 단계 중 셋째」), 그 아래 단계 목록 — 끝난 단계·도는 단계·남은 단계(「대기」). 남은
 * 단계와 그 평소 소요는 일정의 답이 안다(`deskReleaseAhead`, t-67482) — 그 답이 없으면 레인
 * 파일이 말한 단계만 선다. 끝난 레인은 접힌다: 「● 초록 · 07:31 끝남」과 커밋 · 걸린 시간의 두
 * 줄이고, 단계는 「단계별 시간 · 원문 로그」 안에 있다(빨강 레인은 실패한 단계를 남긴다). 카드의
 * 발은 빌드 줄의 다음 차례다(`deskLineFoot`). */

/* 판정 셋의 낱말과 상태 점. */
const DESK_RELEASE_VERDICTS = Object.freeze({
  running: { dot: "run", key: "board.now.stateRun", word: "도는 중" },
  green: { dot: "done", key: "board.desk.releaseGreen", word: "초록" },
  red: { dot: "halt", key: "board.desk.releaseRed", word: "빨강" },
});

/* 일정의 답이 레인 한 번을 어떻게 끝냈다고 말하는가(`result`) → 이 표의 낱말. 일정은 레인의 낱말을
 * 직접 비교하지 않고 이 표만 읽는다(검토 t-69069 P3 ⑦). */
const DESK_RELEASE_OF_RESULT = Object.freeze({ passed: "green", failed: "red" });

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

/* 단계마다 표식(묶음 아이콘, 도는 단계는 호박 점)과 오른쪽 낱말: 끝난 단계는 「끝」, 도는 단계는
 * 제 경과, 남은 단계는 「대기」. 단계마다 걸린 시간은 「단계별 시간 · 원문 로그」 안에 있다. */
const DESK_RELEASE_STEPS = Object.freeze({
  done: { icon: "circle-check", key: "board.now.stepDone", word: "끝" },
  skipped: { icon: "circle-dashed", key: "board.now.stepSkipped", word: "건너뜀" },
  failed: { icon: "circle-x", key: "board.now.stepFailed", word: "실패" },
  ahead: { icon: "circle", key: "board.now.stepAhead", word: "대기" },
  running: { icon: null },
});

/* 레인 단계의 우리말 이름 — 레인(tools/release/lane.sh의 `PHASES`)이 적는 이름마다 하나. 이 표가
 * 모르는 단계는 레인이 적은 이름 그대로 서고, 원문 로그는 언제나 레인의 이름이다. */
const DESK_RELEASE_PHASES = Object.freeze({
  archive: { key: "board.now.phaseArchive", word: "소스 묶기" },
  "gate-root": { key: "board.now.phaseGateRoot", word: "결합 시험" },
  "gate-zo": { key: "board.now.phaseGateZo", word: "zo 시험" },
  flakes: { key: "board.now.phaseFlakes", word: "흔들린 시험 다시" },
  push: { key: "board.now.phasePush", word: "올리기" },
  "build-app": { key: "board.now.phaseBuildApp", word: "앱 빌드" },
  "swap-app": { key: "board.now.phaseSwapApp", word: "앱 바꿔 넣기" },
  "build-zo": { key: "board.now.phaseBuildZo", word: "zo 빌드" },
  "swap-zo": { key: "board.now.phaseSwapZo", word: "zo 바꿔 넣기" },
  "bundle-updater": { key: "board.now.phaseBundle", word: "업데이트 묶기" },
  publish: { key: "board.now.phasePublish", word: "발행" },
  sweep: { key: "board.now.phaseSweep", word: "정리" },
});

/* A release lane's name: its version, or its commit when it has none yet. */
function deskReleaseVersionWord(lane) {
  return lane.version ? `v${lane.version}` : String(lane.sha).slice(0, SHORT_COMMIT_LENGTH);
}

function deskReleasePhaseWord(name) {
  const known = DESK_RELEASE_PHASES[name];
  return known ? t(known.key, known.word) : String(name);
}

/* 「네 단계 중 셋째」 — 시안은 차례를 숫자 대신 우리말 셈으로 적는다. 셈말은 말마다 번역 목록이
 * 가진다(`board.orbit.particle`처럼 `|`로 가른 목록): 한국어만 낱말이 있고, 다른 말과 목록을 넘는
 * 수는 숫자다. */
function deskStepOfWords(at, total) {
  const tally = t("board.now.lineTally",
    "|한|두|세|네|다섯|여섯|일곱|여덟|아홉|열|열한|열두|열세|열네|열다섯|열여섯|열일곱|열여덟|열아홉|스무").split("|");
  const ordinal = t("board.now.lineOrdinal",
    "|첫째|둘째|셋째|넷째|다섯째|여섯째|일곱째|여덟째|아홉째|열째|열한째|열두째|열셋째|열넷째|열다섯째|열여섯째"
    + "|열일곱째|열여덟째|열아홉째|스무째").split("|");
  const said = tally[total] && ordinal[at] ? { at: ordinal[at], total: tally[total] } : { at, total };
  return t("board.now.lineStepOf", "{{total}} 단계 중 {{at}}", said);
}

/* 일정의 답에서 이 레인 — 같은 sha로 지금 도는 것. 지금 단계부터 남은 단계와 그 평소 소요(지난
 * 초록 레인들의 가운데 값)를 든다(`schedule_timeline`의 `ahead`). */
function deskReleaseAhead(status) {
  const lanes = Array.isArray(boardScheduleAnswer?.lanes) ? boardScheduleAnswer.lanes : [];
  const lane = lanes.find((one) => one.running && String(status.sha).startsWith(String(one.sha)));
  return lane && Array.isArray(lane.ahead) && lane.ahead.length > 0 ? lane : null;
}

/* 길이 한 마디: 1분이 안 되면 그렇다고. */
function deskSpanWords(ms) {
  return ms >= 60_000 ? usageDuration(ms) : t("board.desk.underMinute", "1분 미만");
}

/* 단계별 시간의 한 칸: 끝난 단계는 걸린 시간, 도는 단계는 경과와 평소, 남은 단계는 평소. */
function deskReleaseTime(step, now) {
  const usual = Number(step.expected?.samples) > 0 ? deskSpanWords(Number(step.expected.secs) * 1000) : "";
  if (step.state === "running") {
    const ran = runningWord(step.from, now) ?? t("board.desk.releaseJustStarted", "방금 시작");
    return usual ? t("board.now.lineRanUsual", "{{ran}} · 평소 {{usual}}", { ran, usual }) : ran;
  }
  if (step.state === "ahead") return usual ? t("board.now.lineAbout", "약 {{time}}", { time: usual }) : "—";
  if (step.state === "skipped") return t("board.now.stepSkipped", "건너뜀");
  const took = deskSpanWords((Number(step.secs) || 0) * 1000);
  return step.state === "failed" ? t("board.now.lineFailedTook", "실패 · {{time}}", { time: took }) : took;
}

/* 한 단계의 표식 — 행의 첫 칸, 상태가 바뀔 때만 다시 짓는다: 묶음 아이콘이거나 도는 단계의 점. */
function dressDeskStepMark(item, state) {
  if (item.dataset.mark === state) return;
  item.dataset.mark = state;
  const glyph = DESK_RELEASE_STEPS[state].icon;
  const holder = document.createElement("template");
  holder.innerHTML = glyph ? icon(glyph) : '<span class="board-dot is-run" aria-hidden="true"></span>';
  item.firstElementChild.replaceWith(holder.content.firstElementChild);
}

/* 단계별 시간 — 펼친 사람에게만 짓는다: 접힌 기록은 그 표와 원문을 들고 다니지 않는다. */
function paintDeskReleaseTimes(log, steps, phases, now) {
  if (!log.open) {
    for (const node of log.querySelectorAll(":scope > :not(summary)")) node.remove();
    return;
  }
  let times = log.querySelector(":scope > .board-desk-release-times");
  if (!times) {
    times = deskElement("table", "board-desk-release-times");
    times.append(deskElement("tbody", ""));
    log.append(times, deskElement("pre", "board-raw-text"));
  }
  const rows = times.firstElementChild;
  const heldRows = new Map([...rows.children].map((node) => [node.dataset.phase, node]));
  reconcileElementOrder(rows, steps.map((step) => {
    let row = heldRows.get(step.name);
    if (!row) {
      row = deskElement("tr", "");
      row.append(deskElement("td", ""), deskElement("td", "board-desk-release-step-time"));
    }
    writeAttribute(row, "data-phase", step.name);
    writeTextContent(row.firstElementChild, deskReleasePhaseWord(step.name));
    writeTextContent(row.lastElementChild, deskReleaseTime(step, now));
    return row;
  }));
  // The lane's own words for each step, as its status file wrote them — the name, its exit
  // code or skip, its seconds — for whoever opens the record.
  writeTextContent(log.querySelector(":scope > .board-raw-text"), phases.map((phase) => [
    phase.name, phase.skipped ? `skipped ${phase.skipped}` : `rc=${phase.rc}`,
    Number.isFinite(phase.secs) ? `${phase.secs}s` : "",
  ].filter(Boolean).join(" ")).join("\n"));
}

/* 빌드 줄의 발: 레인 말고 줄이 무엇을 하는가 — 기다리는 일감이 있으면 실행기의 차례대로 그 첫째
 * (`boardScheduleQueuePlan`), 도는 일감만 있으면 그것, 아무것도 없으면 「줄 비어 있음」. 일정의
 * 답이 아직 없으면 발은 서지 않는다 — 모르는 것을 비었다고 하지 않는다. */
/* 발의 글은 일정의 답 하나에 한 번 — 답이 바뀔 때만 다시 센다(줄의 첫째는 넣은 차례가 정한다). */
const deskLineFeet = new WeakMap();

function deskLineFoot(now) {
  const answer = boardScheduleAnswer;
  if (!answer || !Array.isArray(answer.jobs)) return "";
  const said = deskLineFeet.get(answer);
  if (said?.language === document.documentElement.lang && said.ledger === deskLedger) return said.words;
  const words = deskLineFootWords(answer, now);
  deskLineFeet.set(answer, { words, language: document.documentElement.lang, ledger: deskLedger });
  return words;
}

function deskLineFootWords(answer, now) {
  const waiting = answer.jobs.filter((job) => job.state === "waiting");
  if (waiting.length > 0) {
    const lanes = Array.isArray(answer.line?.runner_lanes) ? answer.line.runner_lanes : [];
    const plan = boardScheduleQueuePlan(answer.jobs, lanes, now);
    const next = waiting.reduce((first, job) =>
      ((plan.get(job.name)?.start ?? Infinity) < (plan.get(first.name)?.start ?? Infinity) ? job : first));
    return answer.line?.paused === true
      ? t("board.now.lineNextPaused", "다음 차례 — {{job}}(줄이 멈춘 동안 기다림)", { job: deskLineJobWords(next) })
      : t("board.now.lineNext", "다음 차례 — {{job}}", { job: deskLineJobWords(next) });
  }
  const running = answer.jobs.find((job) => job.state === "running");
  return running
    ? t("board.now.lineBusy", "줄에서 도는 일감 — {{job}}", { job: deskLineJobWords(running) })
    : t("board.now.lineEmpty", "줄 비어 있음");
}

/* 일감 하나의 이름: 결합 게이트의 일감은 그 게이트, 과업 id를 단 일감은 원장의 과업 이름, 그 밖에는
 * 일감 이름(차례와 넣은 시각을 뗀 것). */
function deskLineJobWords(job) {
  if (job.gate) return t("board.now.lineGate", "결합 게이트 {{gate}}", { gate: job.gate });
  const id = /(?:^|[^a-z0-9])t-?(\d{4,})/i.exec(`${job.who ?? ""} ${job.name}`);
  const task = id ? (deskLedger?.tasks ?? []).find((one) => one.id === `t-${id[1]}`) : null;
  return task?.title ?? (id ? `t-${id[1]}` : boardScheduleJobLabel(job.name));
}

function paintDeskRelease(block, now) {
  const status = releaseLane;
  if (!status || typeof status !== "object" || !status.sha) return false;
  const head = block.firstElementChild;
  if (!head.firstElementChild) {
    head.append(deskElement("span", "board-desk-head-word"), deskElement("span", "board-desk-head-note"));
  }
  writeTextContent(head.firstElementChild, t("board.now.line", "빌드 줄"));
  writeTextContent(head.lastElementChild, t("board.now.lineHint", "한 번에 하나씩"));
  const body = block.lastElementChild;
  let card = body.querySelector(":scope > .board-desk-release");
  if (!card) {
    card = deskElement("div", "board-desk-release");
    const line = deskElement("p", "board-desk-release-line");
    line.append(deskElement("span", "board-desk-release-title"), deskElement("strong", "board-desk-release-version"),
      deskElement("span", "board-desk-release-status"));
    const meta = deskElement("p", "board-desk-release-meta");
    meta.append(deskElement("code", "board-desk-release-sha"), deskElement("span", "board-desk-release-took"));
    card.append(line, meta, deskElement("ol", "board-desk-release-phases"),
      deskElement("p", "board-desk-release-reason"), deskElement("ul", "board-desk-release-skips"));
    const log = deskElement("details", "board-desk-release-log");
    const summary = deskElement("summary", "board-raw-head");
    summary.insertAdjacentHTML("afterbegin", icon("chevron"));
    log.append(summary);
    log.ontoggle = () => scheduleDeskPaint();
    body.replaceChildren(card, log, deskElement("p", "board-line-foot"));
  }
  const [, log, foot] = body.children;
  writeTextContent(card.querySelector(".board-desk-release-title"), t("board.now.lineRelease", "새 버전 발행"));
  const verdict = deskReleaseVerdict(status);
  const said = DESK_RELEASE_VERDICTS[verdict];
  const phases = Array.isArray(status.phases) ? status.phases.filter((one) => one && one.name) : [];
  const failed = phases.filter((phase) => deskReleasePhaseState(phase) === "failed");
  // A finished lane folds to its verdict and when it ended; a red one keeps the step that failed.
  const folded = verdict !== "running";
  writeClassName(card, `board-desk-release is-${verdict}${folded ? " is-folded" : ""}`);
  // A version is a name, and reads as one: `v1.1.84`.
  writeTextContent(card.querySelector(".board-desk-release-version"), status.version ? `v${status.version}` : "");
  const started = Date.parse(status.started_at);
  const updated = Date.parse(status.updated_at);
  // 「● 도는 중」, or a finished lane's 「● 초록 · 07:31 끝남」 — the dot is the status's own ::before.
  writeTextContent(card.querySelector(".board-desk-release-status"), folded && Number.isFinite(updated)
    ? t("board.now.lineEnded", "{{verdict}} · {{at}} 끝남", { verdict: t(said.key, said.word), at: boardClock(updated, now) })
    : t(said.key, said.word));
  writeTextContent(card.querySelector(".board-desk-release-sha"), String(status.sha).slice(0, SHORT_COMMIT_LENGTH));
  // The running step started when the steps before it had spent their seconds.
  const from = (Number.isFinite(started) ? started : now) + phases.reduce((sum, phase) => sum + (Number(phase.secs) || 0), 0) * 1000;
  const lane = folded ? null : deskReleaseAhead(status);
  const current = !folded && status.phase && !phases.some((one) => one.name === status.phase) ? String(status.phase) : "";
  const steps = phases.map((phase) => ({ ...phase, state: deskReleasePhaseState(phase) }));
  if (current) steps.push({ name: current, state: "running", from, expected: lane?.ahead[0]?.expected ?? null });
  for (const ahead of lane?.ahead.slice(current ? 1 : 0) ?? []) steps.push({ name: ahead.name, state: "ahead", expected: ahead.expected });
  const took = folded && Number.isFinite(started) && Number.isFinite(updated)
    ? t("board.desk.releaseTookShort", "· {{took}} 걸림", { took: deskSpanWords(Math.max(0, updated - started)) }) : "";
  const at = steps.findIndex((step) => step.state === "running");
  writeTextContent(card.querySelector(".board-desk-release-took"), folded ? took
    : lane && at >= 0 ? `· ${deskStepOfWords(at + 1, steps.length)}` : "");
  const list = card.querySelector(".board-desk-release-phases");
  const shown = folded ? steps.filter((step) => step.state === "failed") : steps;
  writeHidden(list, shown.length === 0);
  const held = new Map([...list.children].map((node) => [node.dataset.phase, node]));
  reconcileElementOrder(list, shown.map((step) => {
    let item = held.get(step.name);
    if (!item) {
      item = deskElement("li", "");
      item.append(deskElement("span", ""), deskElement("span", "board-desk-release-step"),
        deskElement("span", "board-desk-release-step-word"));
    }
    writeAttribute(item, "data-phase", step.name);
    writeClassName(item, `board-desk-release-phase is-${step.state}`);
    dressDeskStepMark(item, step.state);
    writeTextContent(item.children[1], deskReleasePhaseWord(step.name));
    writeTextContent(item.children[2], step.state === "running"
      ? runningWord(step.from, now) ?? t("board.desk.releaseJustStarted", "방금 시작")
      : t(DESK_RELEASE_STEPS[step.state].key, DESK_RELEASE_STEPS[step.state].word));
    return item;
  }));
  const reason = card.querySelector(".board-desk-release-reason");
  writeHidden(reason, verdict !== "red" || !status.reason);
  writeTextContent(reason, verdict === "red" ? String(status.reason || "") : "");
  const skips = card.querySelector(".board-desk-release-skips");
  const skipped = folded ? [] : phases.filter((phase) => phase.skipped);
  writeHidden(skips, skipped.length === 0);
  const heldSkips = new Map([...skips.children].map((node) => [node.dataset.phase, node]));
  reconcileElementOrder(skips, skipped.map((phase) => {
    const item = heldSkips.get(phase.name) ?? deskElement("li", "");
    writeAttribute(item, "data-phase", phase.name);
    writeTextContent(item, t("board.desk.skipReason", "{{name}}: {{reason}}",
      { name: deskReleasePhaseWord(phase.name), reason: String(phase.skipped) }));
    return item;
  }));
  writeTrailingText(log.firstElementChild, t("board.now.lineLog", "단계별 시간 · 원문 로그"));
  paintDeskReleaseTimes(log, steps, phases, now);
  writeHidden(log, steps.length === 0);
  const next = deskLineFoot(now);
  writeTextContent(foot, next);
  writeHidden(foot, next === "");
  return true;
}

/* 데스크가 보이는 동안의 1분 박자 하나 (t-67607 → t-70028). 느린 사실을 다시 묻고, 그 답을
 * 기다리지 않고 판을 한 번 다시 그린다 — 「N분째」·「N분 전」·단계의 경과는 원장이 움직이지 않아도
 * 1분마다 바뀌는 글자다. 그림은 바뀐 글자 노드만 쓰고(`writeTextContent`), 창이 숨었거나 작업
 * 보기가 서 있지 않으면 박자도 서지 않는다(`idlePoller`). 상태 바의 박자와 같은 문을 부른다. */
function deskMinute() {
  refreshDeskAmbient();
  scheduleDeskPaint();
}

const deskAmbient = idlePoller({
  wanted: () => deskViews().length > 0,
  every: DESK.ambientEveryMs,
  tick: deskMinute,
  onResume: deskMinute,
});
