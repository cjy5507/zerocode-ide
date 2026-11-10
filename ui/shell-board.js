/* ---- 코디네이터 데스크 (t-6588) ---------------------------------------------
 *
 * 작업 상황판 위의 다섯 답. 하루 동안 코디네이터가 원장 CLI(`worker-list`·
 * `check --peek`·`task-list`·`dispatch-show`)와 `df -g`·`uptime`·레인의
 * `status.sh`·`xcrun simctl list`를 수백 번 쳐서 물은 것을, 판이 먼저 답한다
 * (docs/design/agent-board-round4.md): 기계 띠 · 답할 우편 · 워커 · 과업
 * 흐름 · 빌드.
 *
 * 새 화면이 아니다. 작업 보드의 머리·레일·인스펙터는 3차 그대로이고, 데스크는
 * 작업 목록 위에 원장의 장(章)처럼 선다 — 위에서 아래로 읽고, 넓은 판에서는
 * 두 칸으로 폭을 나눈다(shell.css 「코디네이터 데스크」 절).
 *
 * 읽는 것은 창이 이미 들고 있는 한 벌뿐이다. 원장은 1초 박자가 게시한 것을
 * 읽고(`ledger_agents`·`board_desk`), 빌드는 이 프로젝트의 빌드 기록을 판
 * 번호와 함께 묻는다(`board_builds`). 셈(단계·상태·ack 가능 여부)은 백엔드의
 * 것이고 여기는 그 답을 그린다 — 같은 사실을 두 곳에서 세면 두 답이 된다. */

/* 데스크의 박자와 상한, 한 표. */
const DESK = Object.freeze({
  /* 데스크가 보일 때만 도는 느린 박자: 원장의 행, 빌드, 기계 띠. */
  ambientEveryMs: 60_000,
  /* 빌드가 화면에 서 있고(「지금」의 카드나 쌓인 일) 그중 하나가 돌거나 차례를 기다리는 동안의 박자:
   * 단계 하나는 수십 초일 수 있어 1분 박자로는 도는 칸을 놓친다. 움직이지 않은 기록은 판 번호 한
   * 마디(`unchanged`)로 답하므로 박자가 잦아도 그리는 일은 없다. */
  buildsEveryMs: 5_000,
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
const deskChoice = {
  stage: undefined, mailAll: false, workersOpen: new Set(), todayOpen: new Set(),
  // The History tab's record (11-08 draft v3): the days a person opened or shut (today is open until shut), the result
  // shown, the words searched for.
  daysOpen: new Set(), daysShut: new Set(), historyResult: "all", historyQuery: "",
  // 「지금」의 빌드 카드(11-10): 지난 빌드의 접힌 줄을 폈는가.
  buildsOld: false,
};

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
  // The desk is the project's on screen (the build card's plan, §8): asked for its checkout, and an answer that comes
  // back after the person moved to another project is asked again rather than drawn.
  const asked = activeWorktreePath;
  Promise.all([invoke("board_desk", { worktree: asked }), readLedgerAgents()])
    .then(([answer, agents]) => {
      if (asked !== activeWorktreePath) {
        deskLedgerAgain = true;
        return;
      }
      const said = JSON.stringify([answer ?? null, agents ?? []]);
      if (said === deskLedgerSaid) return;
      // The roster is the workers still summoned (`worker-list`) for this project's runs: a row the ledger keeps only
      // for the finished work in its checkout (`settled`, t-10993) is on the board as that work, not here as a worker,
      // and another project's worker is on that project's board.
      const runs = new Set((Array.isArray(answer?.runs) ? answer.runs : []).map((run) => run.run));
      const summoned = Array.isArray(agents) ? agents.filter((row) => row?.settled !== true && runs.has(row.run)) : [];
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
  // Builds that moved repaint the desk themselves (`askBoardBuilds`).
  askBoardBuilds();
  void askMachineLoad();
  void askDeskCheckouts();
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
 * 「지금」은 당신 차례·지금 도는 일·빌드·통합 묶음(11-10: 좁은 판에서 지금 움직이는 것이 빌드 기록보다 먼저 읽힌다 —
 * 넓은 판의 자리는 격자가 정한다), 「워커」는 워커의 표·에이전트 대화·이 기계, 「기록」은
 * 오늘 끝난 일의 표·누적 수(시안 B, t-70264)와 소식·과업 흐름이다. 「일정」에는 데스크 블록이
 * 없다 — 그 탭의 블록은 일정의 그릇 하나다(`paintBoardSchedule`, t-67482). 그리는 손은 블록에
 * 쓸 것이 있으면 참을 돌려주고, 거짓이면 그 블록은 접힌다. 판에 선 탭의 블록만 그린다 — 다른
 * 탭은 고를 때 그린다(`chooseBoardTab`). */
const DESK_BLOCKS = Object.freeze([
  { id: "asks", tab: "now", paint: paintDeskAsks },
  { id: "running", tab: "now", paint: paintDeskRunning },
  { id: "builds", tab: "now", paint: paintDeskBuilds },
  { id: "batches", tab: "now", paint: paintDeskBatches },
  { id: "left", tab: "now", paint: paintDeskLeft, quietHead: true },
  { id: "workers", tab: "workers", paint: paintDeskWorkers },
  { id: "talks", tab: "workers", paint: paintDeskTalks },
  { id: "machine", tab: "workers", paint: paintDeskMachine },
  { id: "today", tab: "history", paint: paintDeskToday },
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
  boardBuildsPoller.sync();
  const now = Date.now();
  if (now - deskAmbientAt >= DESK.ambientEveryMs) refreshDeskAmbient();
  // 이번 그림이 읽을 에이전트 대화(11-10, shell-board-now.js): 탭 곁의 수와 「지금」의 블록들이 같은 한 벌을 읽는다.
  // 원장의 런이 없는 창에서는 읽지 않는다 — 그 창의 판은 에이전트 작업 목록 그 자체라 같은 대화가 두 번 선다.
  boardTalksNow = (deskLedger?.runs?.length ?? 0) > 0 ? boardTalks(view, now) : BOARD_TALKS_NONE;
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
  paintBoardHead(view, boardOwed(), now);
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
    segments.push({ id: "disk", tone: room.tone, text: free, note, tip: disk.at ?? "" });
  }
  const load = machine?.load;
  if (load && Number.isFinite(load.one_minute)) {
    const figures = { load: load.one_minute.toFixed(1), cores: load.cores };
    segments.push({
      id: "load", tone: load.loud ? "wait" : "",
      text: load.loud
        ? t("board.desk.loadBusy", "부하 {{load}} · 코어 {{cores}} · 붐빔", figures)
        : t("board.desk.load", "부하 {{load}} · 코어 {{cores}}", figures),
      // The meter is the load per core, full at one waiting process a core.
      meter: Number(load.cores) > 0 ? Math.min(1, load.one_minute / Number(load.cores)) : null,
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
  // A cell a fact (11-08 draft v3): its value, a meter where the fact is a share, and the note under it.
  reconcileElementOrder(line, segments.map((segment) => {
    let node = held.get(segment.id);
    if (!node) {
      node = deskElement("span", "");
      const meter = deskElement("span", "board-desk-machine-meter");
      meter.append(deskElement("i", ""));
      node.append(deskElement("strong", "board-desk-machine-value"), meter, deskElement("span", "board-desk-machine-note"));
    }
    const [value, meter, note] = node.children;
    writeAttribute(node, "data-segment", segment.id);
    writeClassName(node, `board-desk-machine-segment${segment.tone ? ` is-${segment.tone}` : ""}`);
    writeTextContent(value, segment.text);
    writeHidden(meter, !Number.isFinite(segment.meter));
    if (Number.isFinite(segment.meter)) writeCustomProperty(meter.firstElementChild, "--fill", `${Math.round(segment.meter * 100)}%`);
    writeTextContent(note, segment.note ?? "");
    writeHidden(note, !segment.note);
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
  quota_walled: { state: "needs-attention", key: "board.desk.mailWalled", word: "한도 도달" },
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
      if (draft.open) questionClosing(letter.run, letter.id).open = false;
      deskDrafts.set(letter.id, draft);
      paintCoordinatorDesk(view);
      if (draft.open) row.querySelector(".board-desk-reply-field")?.focus();
    } else void ackDeskBatch(view, letter);
  };
  // 답 없이 닫기 (t-88847): the same door as the inbox's, asked for the why first. A question's row only.
  const close = deskElement("button", "board-desk-letter-close");
  close.type = "button";
  close.hidden = true;
  close.onclick = () => {
    const letter = row.__letter;
    if (!letter) return;
    const closing = questionClosing(letter.run, letter.id);
    closing.open = !closing.open;
    if (closing.open) {
      const reply = deskDrafts.get(letter.id);
      if (reply) reply.open = false;
    }
    paintCoordinatorDesk(view);
    if (closing.open) row.querySelector(".board-desk-close-field")?.focus();
  };
  lead.append(deskElement("span", "board-desk-letter-body"), close, act);
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

/* 답 없이 닫는 까닭을 쓰는 칸 — 답하기의 칸처럼 처음 펼 때 한 번 짓고, 판이 다시 그려져도 같은 칸이다. */
function deskCloseForm(row, view) {
  let form = row.querySelector(".board-desk-close");
  if (form) return form;
  const closingOf = () => questionClosing(row.__letter.run, row.__letter.id);
  form = questionCloseForm("board-desk-close", "board-desk-close", {
    input: (text) => { closingOf().text = text; },
    cancel: () => {
      closingOf().open = false;
      paintCoordinatorDesk(view);
      // The door it came through: the focus goes back to 답 없이 닫기, not to the page.
      row.querySelector(".board-desk-letter-close")?.focus();
    },
    send: () => void closeDeskQuestion(view, row.__letter),
  });
  // Esc is the cancel from any part of the form, not only from its field.
  form.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    form.querySelector(".board-desk-close-cancel")?.click();
  });
  row.append(form);
  return form;
}

async function closeDeskQuestion(view, letter) {
  const closing = letter && questionClosing(letter.run, letter.id);
  if (!closing || closing.sending || closing.sent) return;
  await closeQuestionWithoutAnswer(closing, {
    run: letter.run,
    message: letter.id,
    repaint: () => paintCoordinatorDesk(view),
    closed: () => {
      closing.open = false;
      refreshDeskLedger();
    },
  });
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
  const closing = letter.kind === "question" ? questionClosing(letter.run, letter.id) : null;
  const note = row.querySelector(".board-desk-letter-note");
  const said = ack?.error || (draft?.sent
    ? t("board.desk.replySent", "답을 보냄 · 원장에 적히면 목록에서 빠져요")
    : closing?.sent ? (closing.answered
      ? t("inbox.note.answered", "그 사이 다른 쪽에서 답했어요 · 원장에 적히면 목록에서 빠져요")
      : t("inbox.note.closed", "답 없이 닫음 · 원장에 적히면 목록에서 빠져요"))
      : ack?.sent ? t("board.desk.ackSent", "묶음을 확인함 · 원장에 적히면 목록에서 빠져요") : "");
  writeTextContent(note, said);
  writeHidden(note, said === "");
  const act = row.querySelector(".board-desk-letter-act");
  const does = deskLetterAct(letter, seat);
  writeHidden(act, does === null || Boolean(draft?.sent) || Boolean(ack?.sent) || Boolean(closing?.sent));
  writeDisabled(act, Boolean(draft?.sending || ack?.sending));
  writeTextContent(act, does === "reply"
    ? t("board.desk.reply", "답하기")
    : ack?.sending
      ? t("board.desk.acking", "확인하는 중…")
      : t("board.desk.ack", "확인 · 이 묶음 {{count}}통", { count: letter.batch ?? 0 }));
  writeAttribute(act, "aria-expanded", String(does === "reply" && Boolean(draft?.open)));
  // The door out of a question nobody will answer: the person's one line of why, on the inbox's own door.
  const canClose = does === "reply" && !closing.sent;
  const closeOpener = row.querySelector(".board-desk-letter-close");
  writeHidden(closeOpener, !canClose);
  writeDisabled(closeOpener, Boolean(closing?.sending));
  writeTextContent(closeOpener, t("inbox.close.send", "답 없이 닫기"));
  writeAttribute(closeOpener, "aria-expanded", String(canClose && Boolean(closing.open)));
  const closeOpen = canClose && Boolean(closing.open);
  const closeForm = closeOpen ? deskCloseForm(row, view) : row.querySelector(".board-desk-close");
  if (closeForm) {
    writeHidden(closeForm, !closeOpen);
    if (closeOpen) paintQuestionCloseForm(closeForm, "board-desk-close", closing);
  }
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
  { id: "gone", state: "failed", dot: "halt", key: "board.desk.healthGone", word: "창이 닫힘",
    holds: (row) => Number.isFinite(row.pane_missing_since_ms) },
  { id: "walled", state: "needs-attention", dot: "halt", key: "board.desk.healthWalled", word: "한도 도달",
    holds: (row, card, now) => Boolean(row.wall) && now < row.wall.stands_until_ms },
  { id: "asking", state: "needs-attention", dot: "you", key: "board.desk.healthAsking", word: "답을 기다림",
    holds: (row, card) => row.asking === true || card?.state === "needs-attention" },
  // 일이 끝나 판이 회수를 기다린다(t-94842): 사람 손 없이 창이 닫는다. 「유휴」로 그리면 끝났는지 모른다.
  { id: "finished", state: "idle", dot: "done", key: "board.desk.healthFinished", word: "끝남",
    holds: (row) => row.ledger === "reclaimable" },
  { id: "asleep", state: "idle", dot: "queue", key: "board.desk.healthAsleep", word: "잠듦",
    holds: (row) => row.ledger === "sleeping" },
  { id: "turn", state: "working", dot: "run", key: "board.desk.healthTurn", word: "일하는 중",
    holds: (row, card) => card?.state === "working" },
  { id: "idle", state: "idle", dot: "queue", key: "board.desk.healthIdle", word: "쉬는 중", holds: () => true },
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
    row.gate ? t("board.workers.calls", "호출 {{count}}", { count: row.gate.metrics?.steps ?? 0 }) : "",
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
/* 칸 일곱(11-08 시안 v3): 「판」 칸(터미널 번호·%0)은 사람에게 뜻이 없어 「판 열기」 단추가 되었고, 호출 수는
 * 자세히로 갔다. 과업은 워커 칸의 둘째 줄이고, 그 자리에 워커가 지금 하는 일이 선다. */
const DESK_WORKER_COLUMNS = Object.freeze([
  { id: "who", key: "board.workers.colWho", word: "워커 · 과업" },
  { id: "state", key: "board.workers.colState", word: "상태" },
  { id: "work", key: "board.workers.colWork", word: "지금 하는 일", wide: true },
  { id: "took", key: "board.workers.colTook", word: "경과", number: true, wide: true },
  { id: "cost", key: "board.workers.colCost", word: "비용", number: true, wide: true },
  { id: "lines", key: "board.workers.colLines", word: "바뀐 줄", number: true, wide: true },
  { id: "act", key: "board.workers.colAct", word: "할 일" },
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
  const [who, state, work, took, cost, lines, act] = DESK_WORKER_COLUMNS;
  const cell = (column, className, ...parts) => {
    const one = deskTableCell("td", column, className);
    one.append(...parts);
    return one;
  };
  // 「판 열기」: the worker's own terminal, its worktree brought forward first (`openPaneFromBoard`).
  const openPane = deskElement("button", "board-btn board-desk-worker-open");
  openPane.type = "button";
  openPane.onclick = () => {
    if (group.__term != null) void openPaneFromBoard(group.__term);
  };
  row.append(cell(who, "board-desk-worker-who", fold, deskElement("span", "board-desk-worker-task")),
    cell(state, "board-desk-worker-state", deskElement("span", "board-desk-worker-health board-state"),
      deskElement("span", "board-desk-worker-verdict")),
    cell(work, "board-desk-worker-work"),
    cell(took, "board-desk-worker-took"), cell(cost, "board-desk-worker-cost"),
    cell(lines, "board-desk-worker-lines"), cell(act, "board-desk-worker-act", openPane));
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

/* 워커가 지금 하는 일 — 판이 있으면 그 판이 말한 일(작업 목록·지금 도는 일과 같은 말, `taskBoardActivity`), 없으면
 * 건강이 말하는 한 줄과 마지막 활동. */
const DESK_WORKER_WORK = Object.freeze({
  asking: { key: "board.workers.workAsking", word: "답을 기다립니다 · 당신 차례에 있습니다" },
  finished: { key: "board.workers.workFinished", word: "일을 마쳤습니다 · 창이 판을 거둡니다" },
});

function deskWorkerWork(row, entry, health, now) {
  if (entry && health.id === "turn") return taskBoardActivity(entry);
  // A gone or walled worker is said as the Now table says it (`BOARD_HALTS`, shell-board-now.js — read when painting,
  // since that file loads after this one).
  const said = health.id === "gone" || health.id === "walled" ? BOARD_HALTS[health.id] : DESK_WORKER_WORK[health.id];
  if (said) return t(said.key, said.word);
  const at = deskWorkerAt(row, entry?.card);
  return at > 0 ? t("board.workers.lastHeard", "마지막 활동 · {{time}} 전", { time: agoWord(at, now) }) : "";
}

/* 머리 줄: 건강마다의 수(0은 빼고)와 오늘 쓴 돈 — 계속 판정이 읽은 하루 합계(`day_spent_usd`). */
function deskWorkerSummary(judged) {
  const pieces = DESK_HEALTH.map((health) => [health, judged.filter((one) => one.health === health).length])
    .filter(([, count]) => count > 0)
    .map(([health, count]) => `${t(health.key, health.word)} ${count}`);
  const day = Math.max(0, ...judged.map(({ row }) => Number(row.gate?.day_spent_usd) || 0));
  if (day > 0) pieces.push(t("board.workers.daySpent", "오늘 {{spent}}", { spent: formatCost(day) }));
  return pieces.join(" · ");
}

/* 런 하나의 머리 행 — 그 아래 워커가 누구의 것인가(11-08 시안 v3: 코디마다 묶는다). */
function deskWorkerRunHead() {
  const group = deskElement("tbody", "board-desk-worker-run");
  const row = deskElement("tr", "");
  const head = deskElement("th", "board-desk-worker-run-name");
  head.scope = "rowgroup";
  head.colSpan = DESK_WORKER_COLUMNS.length;
  row.append(head);
  group.append(row);
  return group;
}

function paintDeskWorkers(block, now, view) {
  const rows = deskAgents;
  if (rows.length === 0) {
    // 워커가 없는 판(11-10): 빈 화면 대신 무엇이 여기에 서는지 말한다 — 원장의 런이 있는 창에서만. 사람의 화면은 이
    // 탭에 기계 띠 한 줄뿐이었고, 그것이 무슨 탭인지 알 수 없었다.
    if ((deskLedger?.runs?.length ?? 0) === 0) return false;
    writeTextContent(block.firstElementChild, t("board.workers.none", "지금 소환된 워커가 없습니다"));
    const body = block.lastElementChild;
    if (body.childElementCount !== 1 || !body.firstElementChild.classList.contains("board-desk-block-note")) {
      body.replaceChildren(deskElement("p", "board-desk-block-note"));
    }
    writeTextContent(body.firstElementChild,
      t("board.workers.noneNote", "코디네이터가 워커를 띄우면 모델 · 하는 일 · 비용 · 바뀐 줄과 함께 여기에 섭니다."));
    return true;
  }
  writeTextContent(block.firstElementChild, t("board.workers.title", "워커 {{count}}", { count: rows.length }));
  const table = deskTableBlock(block, "board-desk-workers", DESK_WORKER_COLUMNS);
  const model = agentGraphModels.get(view);
  const entries = new Map((model?.agents ?? []).map((entry) => [entry.card.pane, entry]));
  const runOrder = (deskLedger?.runs ?? []).map((one) => one.run);
  const judged = rows.map((row) => {
    const seen = row.term != null ? entries.get(`term:${row.term}`) : null;
    const entry = seen ? { card: seen.card, facts: agentCardFacts(seen.card, seen.bucket, now) } : null;
    return { row, card: seen?.card ?? null, entry, health: deskWorkerHealth(row, seen?.card ?? null, now) };
  }).sort((a, b) => (runOrder.indexOf(a.row.run) - runOrder.indexOf(b.row.run)) ||
    DESK_HEALTH.indexOf(a.health) - DESK_HEALTH.indexOf(b.health) || (Number(a.row.at) || 0) - (Number(b.row.at) || 0));
  writeTextContent(table.parentElement.previousElementSibling, deskWorkerSummary(judged));
  const held = new Map([...table.tBodies].map((node) => [node.dataset.worker ?? `run:${node.dataset.run}`, node]));
  const keys = new Set();
  const counted = new Map();
  for (const { row } of judged) counted.set(row.run, (counted.get(row.run) ?? 0) + 1);
  const ordered = [];
  let lastRun;
  for (const one of judged) {
    if (one.row.run !== lastRun) {
      lastRun = one.row.run;
      const head = held.get(`run:${lastRun}`) ?? deskWorkerRunHead();
      writeAttribute(head, "data-run", lastRun);
      writeTextContent(head.querySelector("th"), t("board.workers.runHead", "{{run}} · 워커 {{count}}", {
        run: boardRunName(lastRun) || lastRun, count: counted.get(lastRun) }));
      ordered.push(head);
    }
    ordered.push(one);
  }
  reconcileElementOrder(table, [table.tHead, ...ordered.map((one) => {
    if (one instanceof Element) return one;
    const { row, card, entry, health } = one;
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
    const review = ledgerReviewWord(row);
    writeTextContent(node.querySelector(".board-desk-worker-task"),
      [row.task || row.task_id, review, ledgerClosedReason(row.closed)].filter(Boolean).join(" · "));
    // The state is the health's word beside its dot; a gate that holds the worker says so beside it.
    const word = t(health.key, health.word);
    const reset = health.id === "walled" && Number.isFinite(row.wall?.resets_at_ms)
      ? usageCountdown(row.wall.resets_at_ms - now) : "";
    const healthNode = node.querySelector(".board-desk-worker-health");
    writeAttribute(healthNode, "data-dot", health.dot);
    // 회수가 거절돼 판이 남았으면 그 까닭을 한 줄로(t-94842): 낱말 옆에 「회수 보류」, 까닭은 풍선 도움말에.
    const releaseHeld = health.id === "finished" && typeof row.release_held === "string" ? row.release_held : "";
    const shown = releaseHeld ? [word, t("board.desk.releaseHeld", "회수 보류")].join(" · ") : word;
    writeTextContent(healthNode, reset ? [shown, reset].join(" · ") : shown);
    writeAttribute(healthNode, "data-tip", releaseHeld
      ? t("board.desk.releaseHeldTip", "창이 이 판을 닫지 못했습니다: {{reason}}", { reason: releaseHeld }) : "");
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
    writeTextContent(node.querySelector(".board-desk-worker-work"), deskWorkerWork(row, entry, health, now));
    const openPane = node.querySelector(".board-desk-worker-open");
    writeTextContent(openPane, t("board.workers.openPane", "판 열기"));
    writeHidden(openPane, card == null);
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
  no_wall: (said) => t("quotaWall.why.noWall", "원장에 워커 {{worker}}가 한도에 닿은 기록이 없습니다", { worker: said.worker }),
  wall_lifted: (said) => t("quotaWall.why.wallLifted", "워커 {{worker}}의 한도가 이미 풀렸습니다", { worker: said.worker }),
  account_moved: (said) => t("quotaWall.why.accountMoved", "워커 {{worker}}는 이 한도에 닿은 뒤 다른 계정으로 옮겨 갔습니다 — 그 한도는 떠난 계정의 것입니다", { worker: said.worker }),
  pane_not_here: (said) => t("quotaWall.why.paneNotHere", "이 창은 워커 {{worker}}의 판을 갖고 있지 않습니다", { worker: said.worker }),
  no_transient_order: (said) => t("quotaWall.why.noTransientOrder", "일시 오류 뒤에 이어서 하라는 지시가 이 실행에 없습니다"),
  unknown_run: (said) => t("quotaWall.why.unknownRun", "실행 {{run}}을 원장에서 찾을 수 없습니다", { run: said.run }),
  unknown_worker: (said) => t("quotaWall.why.unknownWorker", "워커 {{worker}}를 원장에서 찾을 수 없습니다", { worker: said.worker }),
  unknown_dispatch: (said) => t("quotaWall.why.unknownDispatch", "시도 {{dispatch}}를 원장에서 찾을 수 없습니다", { dispatch: said.dispatch }),
  unknown_task: (said) => t("quotaWall.why.unknownTask", "과업 {{task}}를 원장에서 찾을 수 없습니다", { task: said.task }),
  seat_lost: (said) => t("quotaWall.why.seatLost", "워커 {{worker}}의 판을 다른 워커가 차지했습니다", { worker: said.worker }),
  awaiting_answer: (said) => t("quotaWall.why.awaitingAnswer", "워커 {{worker}}가 자기가 물은 질문의 답을 기다리고 있습니다", { worker: said.worker }),
  at_wall: (said) => t("quotaWall.why.atWall", "시도 {{dispatch}}는 사용 한도에 닿아 멈춰 있습니다 — 넘기기가 그 답입니다", { dispatch: said.dispatch }),
  wall_not_now: (said) => t("quotaWall.why.wallNotNow", "이 누름은 지난 한도에 대한 것입니다 — 시도 {{dispatch}}가 지금 닿아 있는 한도가 아닙니다", { dispatch: said.dispatch }),
  turn_not_now: (said) => t("quotaWall.why.turnNotNow", "이 누름은 시도 {{dispatch}}가 지금 서 있지 않은 지난 실패에 대한 것입니다 — 워커가 그 뒤에 다시 말했습니다", { dispatch: said.dispatch }),
  resume_in_flight: (said) => t("quotaWall.why.resumeInFlight", "이어서 하라는 한 줄을 지금 넣는 중입니다"),
  resume_ceiling: (said) => t("quotaWall.why.resumeCeiling", "시도 {{dispatch}}에 이미 {{count}}번 이어 쓰기를 넣어 상한에 닿았습니다", { dispatch: said.dispatch, count: said.count }),
  already_typed: (said) => t("quotaWall.why.alreadyTyped", "이번 한도에 대해 시도 {{dispatch}}에 이미 한 줄을 넣었습니다 — 그 글이 입력 칸에 남아 있을 수 있습니다", { dispatch: said.dispatch }),
  too_soon: (said) => t("quotaWall.why.tooSoon", "시도 {{dispatch}}의 마지막 이어 쓰기가 {{seconds}}초 전이라 아직 간격이 지나지 않았습니다",
    { dispatch: said.dispatch, seconds: Math.ceil(Number(said.typed_ms_ago) / 1_000) }),
  coordinator_seat_elsewhere: (said) => t("quotaWall.why.coordinatorSeatElsewhere", "이 창은 실행 {{run}}의 코디네이터 자리를 갖고 있지 않습니다", { run: said.run }),
  no_live_coordinator: (said) => t("quotaWall.why.noLiveCoordinator", "실행 {{run}}의 코디네이터 자리가 비어 있습니다", { run: said.run }),
  handover_waits_for_typing: (said) => t("quotaWall.why.handoverWaitsForTyping", "시도 {{dispatch}}에 이어 쓰기를 넣는 중입니다 — 넘기기는 그것이 끝난 뒤에 됩니다", { dispatch: said.dispatch }),
  attempt_ended: (said) => t("quotaWall.why.attemptEnded", "시도 {{dispatch}}는 이미 끝났습니다 — 넘기기는 열린 시도에만 됩니다", { dispatch: said.dispatch }),
  no_checkout: (said) => t("quotaWall.why.noCheckout", "워커 {{worker}}가 체크아웃을 알리지 않아 물려줄 것이 없습니다", { worker: said.worker }),
  already_walked: (said) => t("quotaWall.why.alreadyWalked", "시도 {{dispatch}}는 이미 넘어갔습니다 — 한 시도는 한 번만 넘깁니다", { dispatch: said.dispatch }),
  handover_ceiling: (said) => t("quotaWall.why.handoverCeiling", "과업 {{task}}는 이미 {{count}}번 넘어가 상한에 닿았습니다", { task: said.task, count: said.count }),
  wall_row_gone: (said) => t("quotaWall.why.wallRowGone", "한도 {{wall}}의 기록이 원장에서 사라졌습니다", { wall: said.wall }),
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
  writeAttribute(retry, "data-tip", t("quotaWall.retryTip", "입력 칸에 이어서 하라는 한 줄을 넣습니다 — 한도마다 한 번만"));
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
      body: t("quotaWall.retryBody", "이 워커({{worker}})의 입력 칸에 이어서 하라는 한 줄을 넣습니다. 한도가 아직 풀리지 않았으면 다시 멈출 수 있고, 한도마다 한 번만 넣습니다.", { worker }),
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
  partial_usage: { key: "board.desk.cost.reasonPartialUsage", word: "세션 사용량 기록의 일부만 읽을 수 있었음" },
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
    wall ? t("board.desk.cost.wall", "{{time}}(대기 포함)", { time: wall }) : t("board.desk.cost.wallOpen", "걸린 시간 —"),
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
  // A gate is said in the person's words, as the Now tab says it (`deskHoldWords`); the ledger's English stays behind it.
  const note = task.gate
    ? deskHoldWords(task.gate)
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

/* 흐름 막대와 칩의 색 — 판의 상태 점과 같은 말: 도는 일은 호박, 사람 차례는 파랑, 막힘·실패는 빨강, 착지·검증은 초록,
 * 발행은 청록, 나머지는 회색. 색만이 아니라 칩의 낱말과 수가 같이 선다. */
const DESK_FLOW_TONES = Object.freeze({
  dispatched: "run", reported: "you", gate: "you", verified: "ok", merged: "ok", deployed: "done",
  blocked: "halt", failed: "halt",
});

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
  // 머리 곁의 한 줄(11-10): 칩이 문이라는 것 — 눌러야 그 단계의 과업이 펼쳐진다는 것을 처음 보는 사람이 알게. 글은
  // 속성으로 서서, 카드의 이름은 예전 그대로 읽힌다.
  writeAttribute(block.firstElementChild, "data-note", t("board.desk.pipelineNote", "단계를 누르면 그 과업이 펼쳐집니다"));
  const body = block.lastElementChild;
  let strip = body.querySelector(":scope > .board-desk-stages");
  let bar = body.querySelector(":scope > .board-desk-flowbar");
  if (!strip) {
    strip = deskElement("div", "board-desk-stages");
    strip.setAttribute("role", "group");
    // The flow and the stuck stages are two runs of chips that wrap as one
    // line: the stuck run starts beside the flow's last chip, one space on.
    strip.append(deskElement("span", "board-desk-stage-run is-flow"), deskElement("span", "board-desk-stage-run"));
    // One bar over the chips (11-08 draft v3): where the tasks stand, by share — the chips stay its legend and its doors.
    bar = deskElement("p", "board-desk-flowbar");
    bar.setAttribute("aria-hidden", "true");
    body.replaceChildren(bar, strip, deskElement("ol", "board-desk-tasks"), deskElement("p", "board-desk-more"));
  }
  // Closed tasks were folded, handed over or outdated — bookkeeping, not work — so their chip counts them but the bar
  // leaves them out: two hundred of them would paint the whole bar one grey.
  const standing = DESK_STAGES.filter((stage) => stage.id !== "closed" && (counts.get(stage.id) ?? 0) > 0);
  // A segment names its stage as `data-part`, never `data-stage`: that is the chips' (a press finds them by it).
  const heldSegments = new Map([...bar.children].map((node) => [node.dataset.part, node]));
  reconcileElementOrder(bar, standing.map((stage) => {
    const segment = heldSegments.get(stage.id) ?? deskElement("i", "board-desk-flowbar-part");
    writeAttribute(segment, "data-part", stage.id);
    writeAttribute(segment, "data-tone", DESK_FLOW_TONES[stage.id] ?? "queue");
    // Its share as a custom property, written only when it moved: a paint over the same counts writes nothing.
    writeCustomProperty(segment, "--share", String(counts.get(stage.id)));
    return segment;
  }));
  writeAttribute(strip, "aria-label", t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  let open = deskChoice.stage === undefined ? deskDefaultStage(counts) : deskChoice.stage;
  if (open && (counts.get(open) ?? 0) === 0) open = deskDefaultStage(counts);
  const [flowRun, stuckRun] = strip.children;
  for (const [run, flow] of [[flowRun, true], [stuckRun, false]]) {
    // A stage with no task draws no chip (11-08: nine of fifteen chips said 0). The evidence stage keeps its rule: only an
    // exact zero hides it, and a count the ledger did not give stands as 「—」, never as a quiet nothing.
    const chips = DESK_STAGES.filter((stage) => stage.flow === flow &&
      (stage.evidence ? counts.get(stage.id) !== 0 : (counts.get(stage.id) ?? 0) > 0))
      .map((stage) => {
        const chip = deskStageChip(run, stage, counts, view);
        writeAttribute(chip, "aria-pressed", String(open === stage.id));
        writeAttribute(chip, "data-tone", DESK_FLOW_TONES[stage.id] ?? "queue");
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

/* 기록의 거르개(11-08 시안 v3): 끝난 결과마다 하나, 0인 것은 서지 않는다. */
const DESK_HISTORY_RESULTS = Object.freeze([
  { id: "all", key: "board.history.resultAll", word: "전체", holds: () => true },
  { id: "deployed", key: "board.now.doneShipped", word: "발행됨", holds: (task) => task.stage === "deployed" },
  { id: "merged", key: "board.history.resultLanded", word: "착지", holds: (task) => task.stage === "merged" },
  { id: "other", key: "board.history.resultOther", word: "착지할 것 없이 끝남", holds: (task) => task.stage !== "deployed" && task.stage !== "merged" },
]);

/* 끝난 과업의 날 — 그 끝난 때의 지역 날짜. */
function deskDayKey(ms) {
  const day = new Date(ms);
  return `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`;
}

function deskDayWords(key, now) {
  if (key === deskDayKey(now)) return t("board.history.dayToday", "오늘");
  if (key === deskDayKey(now - 86_400_000)) return t("board.history.dayYesterday", "어제");
  const [year, month, date] = key.split("-").map(Number);
  return new Date(year, month - 1, date).toLocaleDateString(document.documentElement.lang || undefined,
    { month: "long", day: "numeric", weekday: "short" });
}

function deskDayOpen(key, now) {
  return key === deskDayKey(now) ? !deskChoice.daysShut.has(key) : deskChoice.daysOpen.has(key);
}

function deskDayHead(view) {
  const group = deskElement("tbody", "board-today-day");
  const row = deskElement("tr", "");
  const head = deskElement("th", "board-today-day-head");
  head.scope = "rowgroup";
  head.colSpan = DESK_TODAY_COLUMNS.length;
  const fold = deskElement("button", "board-today-day-fold");
  fold.type = "button";
  fold.insertAdjacentHTML("afterbegin", icon("chevron", false));
  fold.append(deskElement("strong", "board-today-day-name"), deskElement("span", "board-today-day-count"));
  fold.onclick = () => {
    const key = group.dataset.day;
    const today = key === deskDayKey(Date.now());
    const shut = today ? deskChoice.daysShut : deskChoice.daysOpen;
    if (shut.has(key)) shut.delete(key);
    else shut.add(key);
    paintCoordinatorDesk(view);
  };
  head.append(fold);
  row.append(head);
  group.append(row);
  return group;
}

/* 기록의 거르개와 찾기 칸 — 블록의 메모 자리에 한 번 짓는다. */
function deskHistoryControls(note, view) {
  if (note.querySelector(".board-history-filter")) return note;
  const filter = deskElement("span", "board-history-filter");
  filter.setAttribute("role", "group");
  filter.onclick = (event) => {
    const button = event.target.closest("button[data-result]");
    if (!button) return;
    deskChoice.historyResult = button.dataset.result;
    paintCoordinatorDesk(view);
  };
  const search = deskElement("input", "board-history-search");
  search.type = "search";
  search.oninput = () => {
    deskChoice.historyQuery = search.value;
    paintCoordinatorDesk(view);
  };
  note.replaceChildren(filter, search);
  return note;
}

function paintDeskToday(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.tasks)) return false;
  writeTextContent(block.firstElementChild, t("board.history.today", "끝난 일"));
  const table = deskTableBlock(block, "board-today", DESK_TODAY_COLUMNS);
  const card = table.parentElement;
  const note = deskHistoryControls(card.previousElementSibling, view);
  const empty = card.querySelector(":scope > .board-today-empty") ?? card.appendChild(deskElement("p", "board-today-empty"));
  // Every finished task the desk carries, the latest first; the tab's one record (11-08: 「오늘 것만」 was the complaint).
  const finished = (ledger.tasks ?? []).filter((task) => boardFinishedAt(task) !== null)
    .sort((a, b) => boardFinishedAt(b) - boardFinishedAt(a));
  const [filter, search] = note.children;
  const counts = new Map(DESK_HISTORY_RESULTS.map((result) => [result.id, finished.filter(result.holds).length]));
  if (!counts.get(deskChoice.historyResult)) deskChoice.historyResult = "all";
  const heldButtons = new Map([...filter.children].map((node) => [node.dataset.result, node]));
  reconcileElementOrder(filter, DESK_HISTORY_RESULTS.filter((result) => result.id === "all" || counts.get(result.id) > 0)
    .map((result) => {
      let button = heldButtons.get(result.id);
      if (!button) {
        button = deskElement("button", "board-history-result");
        button.type = "button";
      }
      writeAttribute(button, "data-result", result.id);
      writeAttribute(button, "aria-pressed", String(deskChoice.historyResult === result.id));
      writeTextContent(button, `${t(result.key, result.word)} ${counts.get(result.id)}`);
      return button;
    }));
  writeAttribute(filter, "aria-label", t("board.history.filter", "결과로 거르기"));
  writeAttribute(search, "placeholder", t("board.history.search", "과업·id 찾기"));
  writeAttribute(search, "aria-label", t("board.history.search", "과업·id 찾기"));
  if (document.activeElement !== search && search.value !== deskChoice.historyQuery) search.value = deskChoice.historyQuery;
  const shown = DESK_HISTORY_RESULTS.find((result) => result.id === deskChoice.historyResult) ?? DESK_HISTORY_RESULTS[0];
  const query = deskChoice.historyQuery.trim().toLowerCase();
  const tasks = finished.filter((task) => shown.holds(task)
    && (!query || `${task.title} ${task.id}`.toLowerCase().includes(query)));
  const days = new Map();
  for (const task of tasks) {
    const key = deskDayKey(boardFinishedAt(task));
    if (!days.has(key)) days.set(key, []);
    days.get(key).push(task);
  }
  const runs = ledger.runs?.length ?? 1;
  const held = new Map([...table.tBodies].map((node) => [node.dataset.task ?? `day:${node.dataset.day}`, node]));
  const keys = new Set();
  const wanted = [table.tHead];
  for (const [day, dayTasks] of days) {
    const head = held.get(`day:${day}`) ?? deskDayHead(view);
    writeAttribute(head, "data-day", day);
    const open = deskDayOpen(day, now) || query !== "";
    writeAttribute(head.querySelector(".board-today-day-fold"), "aria-expanded", String(open));
    head.querySelector(".board-today-day-fold .icon")?.classList.toggle("is-open", open);
    writeTextContent(head.querySelector(".board-today-day-name"), deskDayWords(day, now));
    writeTextContent(head.querySelector(".board-today-day-count"), t("board.history.dayCount", "{{count}}개", { count: dayTasks.length }));
    wanted.push(head);
    if (open) wanted.push(...dayTasks.map((task) => deskTodayTaskRow(held, keys, task, runs, now, view)));
  }
  reconcileElementOrder(table, wanted);
  // A task that left the record takes its opened lines with it.
  for (const key of deskChoice.todayOpen) if (!keys.has(key)) deskChoice.todayOpen.delete(key);
  writeHidden(table, tasks.length === 0);
  writeTextContent(empty, finished.length === 0 ? t("board.history.none", "끝난 일이 아직 없습니다")
    : t("board.history.noMatch", "거르개와 찾는 말에 맞는 일이 없습니다"));
  writeHidden(empty, tasks.length > 0);
  return true;
}

function deskTodayTaskRow(held, keys, task, runs, now, view) {
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
}


/* ---- 빌드 --------------------------------------------------------------------
 *
 * 이 프로젝트의 빌드 — 어떤 빌드든 제 기록을 남긴 것(`board_builds`, zerocode_core::builds: `zo build
 * report`나 그 모양을 지키는 아무 스크립트). 빌드마다 한 줄이 프로젝트가 정한 차례대로 선다
 * (`.zo/build.toml`): 이름과 표지(버전 같은 것)·도구·상태, 커밋과 몇째 단계인지, 그리고 단계 띠 — 칸
 * 길이는 그 단계가 평소 걸린 시간이고, 도는 칸은 평소 대비 경과만큼 찬다. 끝난 빌드는 띠가 얇아지고
 * 시간 글자를 접는다. 단계마다의 시간은 펼친 사람에게만 짓는다. 기록이 하나도 없는 프로젝트는 블록을
 * 접고 당신 차례가 행을 다 쓴다. 창은 어느 도구도, 어느 프로젝트의 단계 이름도 모른다 — 이름은
 * 기록이 말한다. */

/* 빌드 상태의 낱말과 상태 점(`.board-state[data-dot]`). 일정의 지난 빌드 줄도 같은 낱말을 쓴다. */
const BUILD_STATES = Object.freeze({
  waiting: { dot: "queue", key: "board.now.stateQueue", word: "차례 기다림" },
  running: { dot: "run", key: "board.now.stateRun", word: "도는 중" },
  silent: { dot: "queue", key: "board.build.silent", word: "소식 없음" },
  passed: { dot: "done", key: "board.build.passed", word: "통과" },
  failed: { dot: "halt", key: "board.build.failed", word: "실패" },
  cancelled: { dot: "queue", key: "board.build.cancelled", word: "취소됨" },
  ended: { dot: "queue", key: "board.build.ended", word: "끝남" },
});
const BUILD_ENDED = new Set(["passed", "failed", "cancelled", "ended"]);

/* 사람이 누를 단계에서 기다리는 빌드(기록의 `waiting`이 그 단계): 차례는 사람에게 있다. */
const BUILD_WAITS_FOR_PERSON = Object.freeze({ dot: "you", key: "board.build.stateManual", word: "누를 차례" });

/* 아직 끝나지 않은 빌드 — 쓰는 쪽이 아직 기록을 고치고 있다. */
const BUILD_LIVE = new Set(["waiting", "running"]);

/* 펼친 단계 목록의 표식: 묶음 아이콘, 도는 단계는 호박 점. */
const BUILD_STEP_MARKS = Object.freeze({
  done: "circle-check", skipped: "circle-dashed", failed: "circle-x", ahead: "circle", running: null,
});

/* 띠의 칸이 이보다 많으면 이어진 끝난 단계끼리, 남은 단계끼리, 건너뛴 단계끼리 한 칸으로 묶는다 —
 * 카드 폭에서 칸 이름이 읽히는 수. */
const BUILD_BAND_MOST = 6;
const BUILD_GROUP_WORDS = Object.freeze({
  done: { key: "board.build.doneSteps", word: "끝난 {{count}}단계" },
  ahead: { key: "board.build.aheadSteps", word: "남은 {{count}}단계" },
  skipped: { key: "board.build.skippedSteps", word: "건너뛴 {{count}}단계" },
});

/* 기록의 답(`{ revision, builds }`)과 그것을 물은 체크아웃, 그리고 가는 중인 물음: 다른 프로젝트로 옮기기
 * 전에 읽은 답은 이 프로젝트의 것이 아니다. */
let boardBuildsAnswer = null;
let boardBuildsFor;
let boardBuildsAsking = null;

/* 판이 쥔 판 번호를 쥐고 묻는다: 기록이 하나도 움직이지 않았으면 답은 `unchanged` 한 마디다. 물음은 한
 * 번에 하나다 — 가는 중이면 그 물음을 돌려준다. */
function askBoardBuilds() {
  if (boardBuildsAsking) return boardBuildsAsking;
  const asked = activeWorktreePath;
  const known = boardBuildsFor === asked ? boardBuildsAnswer?.revision ?? null : null;
  boardBuildsAsking = invoke("board_builds", { worktree: asked, known })
    .then((answer) => {
      if (!answer || typeof answer !== "object" || answer.unchanged) return;
      boardBuildsAnswer = {
        revision: answer.revision,
        builds: Array.isArray(answer.builds) ? answer.builds : [],
        file: typeof answer.file?.state === "string" ? answer.file : BUILD_FILE_NONE,
        suggest: Array.isArray(answer.suggest) ? answer.suggest : [],
        runnable: Array.isArray(answer.runnable) ? answer.runnable : [],
        report_acts: answer.report_acts && typeof answer.report_acts === "object" ? answer.report_acts : {},
        // The machine's build line (`zo line`, 11-08): every project's heavy work here, and its pause.
        line: answer.line && Array.isArray(answer.line.jobs) ? answer.line : null,
      };
      boardBuildsFor = asked;
      scheduleDeskPaint();
      // 쌓인 일도 빌드를 말한다 — 탭의 수는 언제나, 패널은 서 있는 동안(shell-scm.js).
      paintStackedWorkCount();
      paintStackedWorkIfUp();
    })
    .catch(() => {})
    .finally(() => {
      boardBuildsAsking = null;
      boardBuildsPoller.sync();
    });
  return boardBuildsAsking;
}

/* 순서 파일이 없다는 말(`OrderFile::None`): 답이 그것을 싣지 않았을 때의 자리이기도 하다. */
const BUILD_FILE_NONE = Object.freeze({ state: "none" });

/* 화면의 프로젝트가 보일 빌드 답 전체 — 기록, 순서 파일의 말, 뿌리에서 찾은 빌드. 다른 체크아웃의 답이면 `null`. */
function buildsAnswerHere() {
  return boardBuildsFor === activeWorktreePath ? boardBuildsAnswer : null;
}

/* 화면의 프로젝트가 보일 빌드: 그 체크아웃으로 물은 답, 아직 없으면 `null`(모른다)이고 묻는다. */
function buildsHere() {
  if (boardBuildsFor === activeWorktreePath) return boardBuildsAnswer?.builds ?? [];
  askBoardBuilds();
  return null;
}

/* 기록이 말한 이름 — 한 낱말이거나 말마다 한 낱말(`{"ko": …, "en": …}`): 창의 말, 원문 말(한국어),
 * 아무 말 순이고, 이름이 없으면 기록의 id 그대로다. */
function buildWords(text, id) {
  if (typeof text === "string" && text !== "") return text;
  const words = text && typeof text === "object" ? text : {};
  const tongue = locale === "system" ? systemLocale : locale;
  const said = [words[tongue], words.ko, ...Object.values(words)].find((word) => typeof word === "string" && word !== "");
  return said ?? String(id);
}

/* 빌드의 이름과 표지: 「앱 빌드 1.1.20」, 「도커 이미지」. */
function buildTitleWords(build) {
  return [buildWords(build.name, build.id), build.label].filter(Boolean).join(" ");
}

/* 빌드가 지금 도는 단계, 없으면 `null`. */
function buildRunningStep(build) {
  return (Array.isArray(build.steps) ? build.steps : []).find((step) => step.state === "running") ?? null;
}

/* 「멈춘 듯」 — 기록이 적은 그대로(`stalled`): 도는 단계의 명령이 평소보다 오래 아무것도 쓰지 않았다.
 * 얼마나 조용했는지는 기록이 적은 그 시각부터 지금까지이고, 쓰는 쪽이 그 단계를 실패로 끝낼 상한이
 * 있으면 그것도 말한다. */
function buildStalledVars(stalled, now) {
  const since = Number(stalled.quiet_since_ms);
  return {
    what: String(stalled.what ?? ""),
    time: deskSpanWords(Number.isFinite(since) ? Math.max(0, now - since) : 0),
    cap: Number(stalled.cap_secs) > 0 ? deskSpanWords(Number(stalled.cap_secs) * 1000) : "",
  };
}

function buildStalledWords(stalled, now) {
  const vars = buildStalledVars(stalled, now);
  return vars.cap
    ? t("board.build.stalledCap", "멈춘 듯: {{what}} {{time}} 출력 없음 · {{cap}} 넘으면 실패로 끝냄", vars)
    : t("board.build.stalled", "멈춘 듯: {{what}} {{time}} 출력 없음", vars);
}

/* 「네 단계 중 셋째」 — 시안은 차례를 숫자 대신 우리말 셈으로 적는다. 셈말은 말마다 번역 목록이
 * 가진다(`board.orbit.particle`처럼 `|`로 가른 목록): 한국어만 낱말이 있고, 다른 말과 목록을 넘는
 * 수는 숫자다. */
function buildStepOfWords(at, total) {
  const tally = t("board.build.tally",
    "|한|두|세|네|다섯|여섯|일곱|여덟|아홉|열|열한|열두|열세|열네|열다섯|열여섯|열일곱|열여덟|열아홉|스무").split("|");
  const ordinal = t("board.build.ordinal",
    "|첫째|둘째|셋째|넷째|다섯째|여섯째|일곱째|여덟째|아홉째|열째|열한째|열두째|열셋째|열넷째|열다섯째|열여섯째"
    + "|열일곱째|열여덟째|열아홉째|스무째").split("|");
  const said = tally[total] && ordinal[at] ? { at: ordinal[at], total: tally[total] } : { at, total };
  return t("board.build.stepOf", "{{total}} 단계 중 {{at}}", said);
}

/* 길이 한 마디: 1분이 안 되면 그렇다고. */
function deskSpanWords(ms) {
  return ms >= 60_000 ? usageDuration(ms) : t("board.desk.underMinute", "1분 미만");
}

/* 한 단계가 평소 걸리는 밀리초 — 지난 통과에서 잰 것이 있을 때만. */
function buildStepUsualMs(step) {
  return Number(step.expected?.samples) > 0 ? Number(step.expected.secs) * 1000 : null;
}

/* 단계 하나의 시간: 끝난 단계는 걸린 시간, 도는 단계는 경과와 평소, 남은 단계는 평소. */
/* 단계 안 진행 — 도는 단계의 출력이 말한 단위와 그 전체(`progress`). 전체를 모르거나 이미 넘어섰으면 없다: 그때는
 * 시간이 말한다. */
function buildStepProgress(step) {
  const done = Number(step?.progress?.done);
  const of = Number(step?.progress?.of);
  return Number.isInteger(done) && Number.isInteger(of) && of > 0 && done <= of ? { done, of } : null;
}

function buildStepTime(step, build, now) {
  const usualMs = buildStepUsualMs(step);
  const usual = usualMs === null ? "" : deskSpanWords(usualMs);
  if (step.state === "running") {
    const ran = runningWord(Number(build.running_since_ms) || now, now) ?? t("board.now.justStarted", "방금 시작");
    const progress = buildStepProgress(step);
    if (progress) return t("board.build.progressRan", "{{done}}/{{of}} · {{ran}}", { ...progress, ran });
    return usual ? t("board.build.ranUsual", "{{ran}} · 평소 {{usual}}", { ran, usual }) : ran;
  }
  if (step.state === "ahead") return usual ? t("board.build.about", "약 {{time}}", { time: usual }) : "—";
  if (step.state === "skipped") return t("board.build.skipped", "건너뜀");
  const took = deskSpanWords((Number(step.secs) || 0) * 1000);
  return step.state === "failed" ? t("board.build.failedTook", "실패 · {{time}}", { time: took }) : took;
}

/* 남은 단계들이 평소 걸리는 시간의 합(`ms`)과, 그 단계들이 모두 지난 통과를 가지는지(`known`). */
function buildAheadUsual(steps) {
  const usual = steps.filter((step) => step.state === "ahead").map(buildStepUsualMs);
  return { ms: usual.reduce((sum, ms) => sum + (ms ?? 0), 0), known: usual.every((ms) => ms !== null) };
}

/* 도는 빌드가 끝날 때: 도는 단계의 평소 끝에 남은 단계의 평소를 더한 것 — 도는 단계와 남은 단계가
 * 모두 지난 통과를 가질 때만 안다(`known`). 도는 단계가 평소를 넘었으면 늦다(`lateMs` > 0). */
function buildEnd(build, now) {
  const steps = Array.isArray(build.steps) ? build.steps : [];
  const at = steps.findIndex((step) => step.state === "running");
  if (at < 0) return null;
  const since = Number(build.running_since_ms) || now;
  const usual = buildStepUsualMs(steps[at]);
  const ahead = buildAheadUsual(steps.slice(at + 1));
  return {
    step: steps[at],
    since,
    known: usual !== null && ahead.known,
    lateMs: usual === null ? 0 : now - (since + usual),
    end: Math.max(now, since + (usual ?? 0)) + ahead.ms,
  };
}

/* 띠의 칸들: 단계가 많지 않으면 단계마다 한 칸, 많으면 이어진 같은 상태의 끝난·남은·건너뛴 단계를 한
 * 칸으로 묶는다. 도는 단계와 실패한 단계는 언제나 제 칸이다. */
function buildBand(steps) {
  if (steps.length <= BUILD_BAND_MOST) return steps.map((step) => ({ key: step.name, state: step.state, steps: [step] }));
  const band = [];
  for (const step of steps) {
    const last = band.at(-1);
    if (last && last.state === step.state && BUILD_GROUP_WORDS[step.state]) last.steps.push(step);
    else band.push({ key: step.name, state: step.state, steps: [step] });
  }
  return band;
}

/* 한 칸의 이름과 시간: 단계 하나면 그 단계의 것, 묶음이면 「남은 10단계 · 약 25분」. */
function buildSegmentWords(segment, build, now) {
  const [first] = segment.steps;
  if (segment.steps.length === 1) return { name: buildWords(first.words, first.name), time: buildStepTime(first, build, now) };
  const group = BUILD_GROUP_WORDS[segment.state];
  const name = t(group.key, group.word, { count: segment.steps.length });
  if (segment.state === "skipped") return { name, time: t("board.build.skipped", "건너뜀") };
  if (segment.state === "done") {
    return { name, time: deskSpanWords(segment.steps.reduce((sum, step) => sum + (Number(step.secs) || 0), 0) * 1000) };
  }
  const usual = segment.steps.map(buildStepUsualMs);
  return {
    name,
    time: usual.every((ms) => ms !== null)
      ? t("board.build.about", "약 {{time}}", { time: deskSpanWords(usual.reduce((sum, ms) => sum + ms, 0)) })
      : "—",
  };
}

/* 칸의 길이는 그 단계가 평소 걸린 시간(없으면 이번에 걸린 시간)이고, 아무것도 모르는 단계는 이 빌드에서
 * 아는 단계들의 평균 — 하나도 모르면 모두 같은 길이다. */
function buildSegmentWeights(segments) {
  const weightOf = (step) => (buildStepUsualMs(step) ?? 0) / 1000 || Number(step.secs) || 0;
  const known = segments.flatMap((segment) => segment.steps.map(weightOf)).filter((weight) => weight > 0);
  const fallback = known.length > 0 ? known.reduce((sum, weight) => sum + weight, 0) / known.length : 1;
  return segments.map((segment) => segment.steps.reduce((sum, step) => sum + (weightOf(step) || fallback), 0));
}

/* 단계 띠 — 칸마다 막대·이름·시간. 도는 칸은 평소 대비 경과만큼 차고(`--fill`), 평소를 모르면 빗금이다.
 * 찬 만큼은 분으로 센다: 시간 글자와 같이 1분 박자에만 움직이고, 같은 분에 다시 그린 띠는 아무것도 쓰지
 * 않는다. 칸의 풍선 글은 잘린 이름을 읽게 이름뿐이다. */
function paintBuildBand(band, build, steps, now) {
  const segments = buildBand(steps);
  const weights = buildSegmentWeights(segments);
  const held = new Map([...band.children].map((node) => [node.dataset.key, node]));
  reconcileElementOrder(band, segments.map((segment, index) => {
    let node = held.get(segment.key);
    if (!node) {
      node = deskElement("li", "");
      const bar = deskElement("span", "board-build-bar");
      bar.append(document.createElement("i"));
      node.append(bar, deskElement("span", "board-build-seg-name"), deskElement("span", "board-build-seg-time"));
    }
    const { name, time } = buildSegmentWords(segment, build, now);
    const running = segment.state === "running";
    const usual = running ? buildStepUsualMs(segment.steps[0]) : null;
    const progress = running ? buildStepProgress(segment.steps[0]) : null;
    writeAttribute(node, "data-key", segment.key);
    writeClassName(node, `board-build-seg is-${segment.state}${running && usual === null && progress === null ? " is-unknown" : ""}`);
    writeStyleValue(node, "flexGrow", String(Math.round(weights[index] * 100) / 100));
    if (progress) {
      writeCustomProperty(node, "--fill", `${Math.round((progress.done / progress.of) * 100)}%`);
    } else if (usual !== null) {
      const ran = Math.floor(Math.max(0, now - (Number(build.running_since_ms) || now)) / 60_000) * 60_000;
      writeCustomProperty(node, "--fill", `${Math.min(100, Math.round((ran / usual) * 100))}%`);
    }
    writeAttribute(node, "title", name);
    writeTextContent(node.children[1], name);
    writeTextContent(node.children[2], time);
    return node;
  }));
}

/* 펼친 단계 목록의 표식 — 행의 첫 칸, 상태가 바뀔 때만 다시 짓는다: 묶음 아이콘이거나 도는 단계의 점. */
function dressBuildStepMark(item, state) {
  if (item.dataset.mark === state) return;
  item.dataset.mark = state;
  const glyph = BUILD_STEP_MARKS[state];
  const holder = document.createElement("template");
  holder.innerHTML = glyph ? icon(glyph) : '<span class="board-dot is-run" aria-hidden="true"></span>';
  item.firstElementChild.replaceWith(holder.content.firstElementChild);
}

/* 건너뛴 까닭 — 「…부터 다시」가 앞 단계를 건너뛸 때 러너가 적는 말(`SKIPPED_BEFORE_FROM`, zerocode-core builds.rs)은
 * 사람의 말로, 다른 까닭은 쓴 쪽의 말 그대로. */
const BUILD_SKIPPED_BEFORE_FROM = "before the step this run went on from";

function buildSkippedWords(why) {
  return why === BUILD_SKIPPED_BEFORE_FROM ? t("board.build.skippedBefore", "다시 돈 단계 앞") : String(why);
}

/* 단계마다의 시간 — 펼친 사람에게만 짓는다: 접힌 목록은 행을 들고 다니지 않는다. */
function paintBuildLog(log, build, steps, now) {
  const list = log.lastElementChild;
  if (!log.open) {
    if (list.childElementCount > 0) list.replaceChildren();
    return;
  }
  const held = new Map([...list.children].map((node) => [node.dataset.step, node]));
  reconcileElementOrder(list, steps.map((step) => {
    let row = held.get(step.name);
    if (!row) {
      row = deskElement("li", "");
      row.append(deskElement("span", ""), deskElement("span", "board-build-log-name"), deskElement("span", "board-build-log-time"));
    }
    writeAttribute(row, "data-step", step.name);
    writeClassName(row, `board-build-log-step is-${step.state}`);
    dressBuildStepMark(row, step.state);
    writeTextContent(row.children[1], buildWords(step.words, step.name));
    writeTextContent(row.children[2], step.skipped
      ? t("board.build.skippedWhy", "건너뜀 · {{why}}", { why: buildSkippedWords(step.skipped) })
      : buildStepTime(step, build, now));
    return row;
  }));
}

/* 둘째 줄의 시간: 끝난 빌드는 걸린 시간, 소식 없는 빌드는 마지막 기록의 시각, 아직 도는 단계가 없는 빌드는
 * 시작한 뒤의 시간. 단계가 도는 동안은 그 칸이 경과를 말하고 맨 위 문장이 끝날 시각을 말하므로, 둘째 줄은
 * 시계를 되풀이하지 않는다(빈 글). */
function buildTookWords(build, now) {
  const stopped = buildHaltedStep(build);
  if (build.state === "cancelled" && stopped && Number.isFinite(Number(stopped.secs))) {
    return t("board.build.stoppedAt", "{{step}} {{took}}에서 멈춤", {
      step: buildWords(stopped.words, stopped.name), took: buildSecsWords(Number(stopped.secs)) });
  }
  if (build.state === "silent") {
    return t("board.build.lastHeard", "마지막 기록 {{at}}", { at: boardClock(Number(build.updated_ms), now) });
  }
  if (BUILD_ENDED.has(build.state)) {
    return t("board.build.took", "{{took}} 걸림",
      { took: deskSpanWords(Math.max(0, Number(build.updated_ms) - Number(build.started_ms))) });
  }
  if (buildRunningStep(build)) return "";
  return runningWord(Number(build.started_ms) || now, now) ?? t("board.now.justStarted", "방금 시작");
}

/* 실패하거나 멈춘 단계 — 끝난 실행에서 다시 돌 자리(「…부터 다시」)이고, 멈춘 자리를 말하는 단계다. */
function buildHaltedStep(build) {
  if (build.state !== "failed" && build.state !== "cancelled") return null;
  return (Array.isArray(build.steps) ? build.steps : []).find((step) => step.state === "failed") ?? null;
}

/* 초 단위 걸린 시간: 1분 안이면 초로(「4초」), 넘으면 판의 시간 글로. */
function buildSecsWords(secs) {
  return secs < 60 ? t("board.build.seconds", "{{count}}초", { count: Math.max(1, Math.round(secs)) }) : deskSpanWords(secs * 1000);
}

/* 사람이 누를 단계의 이름 — 기록의 단계 낱말, 없으면 단계 id. */
function buildWaitingStepWords(build) {
  const step = (Array.isArray(build.steps) ? build.steps : []).find((one) => one.name === build.waiting);
  return buildWords(step?.words, String(build.waiting ?? ""));
}

/* 카드 아래의 한 줄: 도는 빌드가 멈춘 듯하면 그 말, 사람이 누를 단계에서 기다리면 그 말, 실패하거나 취소된
 * 빌드는 그 까닭. */
function buildNote(build, now) {
  if (build.state === "running" && build.stalled && typeof build.stalled === "object") {
    return { tone: "stalled", words: buildStalledWords(build.stalled, now) };
  }
  if (build.state === "waiting" && build.waiting) {
    return { tone: "", words: t("board.build.manualWaits", "{{step}} 단계는 누를 때 돌아요", { step: buildWaitingStepWords(build) }) };
  }
  if ((build.state === "failed" || build.state === "cancelled") && build.reason) return { tone: build.state, words: String(build.reason) };
  return { tone: "", words: "" };
}

function buildCardNode() {
  const card = deskElement("article", "board-build");
  const line = deskElement("p", "board-build-line");
  // 상태는 이름 앞의 점 하나다(시안 v3): 차례 번호 원은 단계로 읽혀서 걷었다 — 차례는 카드가 선 차례다.
  line.append(deskElement("span", "board-build-dot"), deskElement("span", "board-build-name"),
    deskElement("strong", "board-build-label"), deskElement("span", "board-build-tool"),
    deskElement("span", "board-state board-build-state"));
  const meta = deskElement("p", "board-build-meta");
  meta.append(deskElement("code", "board-build-sha"), deskElement("span", "board-build-at"), deskElement("span", "board-build-took"));
  // 단추 줄: 돌리기(다시·멈추기·누를 단계)는 `.board-build-act`, 그 곁의 로그와 ⋯는 `.board-build-aux`.
  const acts = deskElement("p", "board-build-acts");
  for (const act of BUILD_CARD_ACTS) {
    const button = deskElement("button", "board-build-act");
    button.type = "button";
    button.dataset.act = act;
    acts.append(button);
    if (act === "again") {
      const caret = deskElement("button", "board-build-caret");
      caret.type = "button";
      caret.dataset.menu = "again";
      caret.setAttribute("aria-haspopup", "menu");
      caret.insertAdjacentHTML("afterbegin", icon("chevron"));
      acts.append(caret);
    }
  }
  for (const aux of BUILD_CARD_AUX) {
    const button = deskElement("button", "board-build-aux");
    button.type = "button";
    button.dataset.act = aux;
    if (aux === "more") {
      button.setAttribute("aria-haspopup", "menu");
      button.insertAdjacentHTML("afterbegin", icon("more"));
    }
    acts.append(button);
  }
  acts.append(deskElement("span", "board-build-menu"));
  acts.onclick = (event) => pressBuildCardButton(card, event.target.closest("button"));
  const log = deskElement("details", "board-build-log");
  const summary = deskElement("summary", "board-build-log-head");
  summary.insertAdjacentHTML("afterbegin", icon("chevron"));
  log.append(summary, deskElement("ol", "board-build-log-steps"));
  log.ontoggle = () => scheduleDeskPaint();
  // 실패 까닭(11-08 시안 v3): 실패한 단계가 제 출력과 시험 보고서로 말한 실패 — 파일과 줄이 있으면 누르면 편집기가 거기를 연다.
  const failures = deskElement("ul", "board-build-failures");
  failures.onclick = (event) => {
    const button = event.target.closest("button[data-file]");
    if (button) openPath(button.dataset.file, button.dataset.line ? { line: Number(button.dataset.line) } : {});
  };
  card.append(line, meta, deskElement("ol", "board-build-band"), deskElement("p", "board-build-note"), failures, acts,
    deskElement("pre", "board-build-tail"), log);
  return card;
}

/* 창이 그대로 열 수 있는 경로: 맥·리눅스의 「/…」, 윈도의 「C:\…」·「C:/…」·「\\서버\…」. 백엔드는 실패가 폴더 안에 선
 * 파일을 말할 때만 이렇게 준다. */
function buildPathIsWhole(path) {
  return /^(?:\/|[A-Za-z]:[\\/]|\\\\)/.test(path);
}

/* 한 실패의 자리 글: 파일 이름과 줄(「order_test.go:31」) — 경로는 마지막 토막만, 전체는 풍선 도움말이 말한다. */
function buildFailurePlace(failure) {
  if (!failure.file) return "";
  const name = basename(failure.file);
  return Number.isInteger(failure.line) ? `${name}:${failure.line}` : name;
}

/* 실패 까닭의 줄들: 실패한 빌드만, 기록이 실은 만큼(백엔드가 스물로 자른다). 절대 경로가 있는 실패는 단추라 누르면 그 파일의
 * 그 줄이 열리고, 자리를 모르거나 폴더 밖의 이름이면 글만 선다. */
function paintBuildFailures(list, build) {
  const failures = build.state === "failed" && Array.isArray(build.failures) ? build.failures : [];
  const items = [...list.children];
  reconcileElementOrder(list, failures.map((failure, at) => {
    const item = items[at] ?? deskElement("li", "board-build-failure");
    const opens = typeof failure.file === "string" && buildPathIsWhole(failure.file);
    let part = item.firstElementChild;
    if (!part || (part.tagName === "BUTTON") !== opens) {
      part = deskElement(opens ? "button" : "span", "board-build-failure-part");
      if (opens) part.type = "button";
      item.replaceChildren(part);
    }
    const place = buildFailurePlace(failure);
    writeTextContent(part, place ? `${failure.what} · ${place}` : failure.what);
    writeAttribute(part, "data-file", opens ? failure.file : "");
    writeAttribute(part, "data-line", Number.isInteger(failure.line) ? String(failure.line) : "");
    writeAttribute(part, "data-tip", opens ? t("board.build.failureOpen", "{{place}} 열기", { place: failure.file }) : "");
    return item;
  }));
  writeAttribute(list, "aria-label", t("board.build.failures", "실패 까닭 {{count}}", { count: failures.length }));
  writeHidden(list, failures.length === 0);
}

function paintBuildCard(card, build, order, now, hands) {
  const said = build.state === "waiting" && build.waiting
    ? BUILD_WAITS_FOR_PERSON
    : BUILD_STATES[build.state] ?? BUILD_STATES.ended;
  const ended = BUILD_ENDED.has(build.state);
  writeClassName(card, `board-build is-${build.state}${ended ? " is-ended" : ""}`);
  writeAttribute(card, "data-started", String(build.started_ms ?? ""));
  writeAttribute(card, "data-order", String(order));
  const [line, meta, band, note, failures, acts, tail, log] = card.children;
  const [dot, name, label, tool, state] = line.children;
  writeAttribute(dot, "data-dot", said.dot);
  writeTextContent(name, buildWords(build.name, build.id));
  writeTextContent(label, build.label ?? "");
  writeHidden(label, !build.label);
  writeTextContent(tool, build.tool ?? "");
  writeHidden(tool, !build.tool);
  // 「● 도는 중」, 끝난 빌드는 「● 통과 · 14:02 끝남」 — 점은 `.board-state`의 ::before다.
  writeAttribute(state, "data-dot", said.dot);
  const verdict = t(said.key, said.word);
  writeTextContent(state, ended
    ? t("board.build.endedAt", "{{verdict}} · {{at}} 끝남", { verdict, at: boardClock(Number(build.updated_ms), now) })
    : verdict);
  const steps = Array.isArray(build.steps) ? build.steps : [];
  const at = steps.findIndex((step) => step.state === "running");
  const [sha, position, took] = meta.children;
  writeTextContent(sha, build.sha ? String(build.sha).slice(0, SHORT_COMMIT_LENGTH) : "");
  writeHidden(sha, !build.sha);
  writeTextContent(position, at >= 0 ? buildStepOfWords(at + 1, steps.length) : "");
  writeHidden(position, at < 0);
  const tookWords = buildTookWords(build, now);
  writeTextContent(took, tookWords);
  writeHidden(took, tookWords === "");
  paintBuildActs(acts, build, hands);
  paintBuildTail(tail, build);
  paintBuildBand(band, build, steps, now);
  writeHidden(band, steps.length === 0);
  const { tone, words } = buildNote(build, now);
  writeClassName(note, tone ? `board-build-note is-${tone}` : "board-build-note");
  writeTextContent(note, words);
  writeHidden(note, words === "");
  paintBuildFailures(failures, build);
  writeTrailingText(log.firstElementChild, t("board.build.steps", "단계 {{count}}개", { count: steps.length }));
  writeHidden(log, steps.length === 0);
  paintBuildLog(log, build, steps, now);
}

/* 기록이 있는 빌드가 하나도 없으면(또는 아직 모르면) 블록은 접히고, 접힌 블록은 앞 프로젝트의 카드를
 * 들고 있지 않는다. */
function paintDeskBuilds(block, now) {
  const builds = buildsHere();
  const body = block.lastElementChild;
  const answer = buildsAnswerHere();
  const foot = builds === null ? null : buildFoot(answer, builds);
  const line = answer?.line ?? null;
  const lineStands = Boolean(line) && (line.jobs.length > 0 || Boolean(line.pause));
  if (!foot?.stands && !lineStands) {
    if (body.childElementCount > 0) body.replaceChildren();
    return false;
  }
  writeTextContent(block.firstElementChild, t("board.build.head", "빌드"));
  const held = new Map([...body.children].map((node) => [node.dataset.build, node]));
  const runnable = answer?.runnable ?? [];
  const runnerHolds = buildRunnerHolds(builds, runnable);
  // 지난 빌드(11-10): 끝난 지 하루가 넘은 실행은 접힌 한 줄 뒤에 선다 — 이틀 전에 취소된 빌드 둘이 첫 화면의 맨 위를
  // 쓰고 지금 도는 일감은 화면 밖에 있었다. 사람이 그 줄을 누르면 카드가 그대로 선다.
  const old = builds.filter((build) => buildIsOld(build, now));
  const folded = old.length > 0 && !deskChoice.buildsOld;
  const fresh = [];
  const aged = [];
  builds.forEach((build, index) => {
    const isOld = old.includes(build);
    if (isOld && folded) return;
    const card = held.get(build.id) ?? buildCardNode();
    writeAttribute(card, "data-build", build.id);
    paintBuildCard(card, build, index + 1, now, {
      runnable: runnable.includes(build.id), runnerHolds, report: answer?.report_acts?.[build.id] ?? null });
    (isOld ? aged : fresh).push(card);
  });
  const oldNode = old.length > 0 ? (block.__old ??= buildOldNode()) : null;
  if (oldNode) paintBuildOld(oldNode, old, now);
  const footNode = body.querySelector(":scope > .board-build-foot") ?? buildFootNode();
  if (foot?.stands) paintBuildFoot(footNode, foot);
  writeHidden(footNode, !foot?.stands);
  // Held by the block, so a paint while the line has nothing to say builds no node to throw away.
  const lineNode = block.__line ??= buildLineNode();
  paintBuildLine(lineNode, line, now);
  writeHidden(lineNode, !lineStands);
  // 줄에 도는 일감이나 기다리는 일감이 있으면 그 줄이 카드 위에 선다(11-10) — 지금 움직이는 것이 먼저 읽힌다. 끝난
  // 일감만 남은 줄은 예전처럼 발 밑에 서고, 말할 것이 없으면 서지 않는다.
  const lineLive = lineStands && line.jobs.some((job) => job.state === "running" || job.state === "waiting");
  writeClass(lineNode, "is-live", lineLive);
  reconcileElementOrder(body, [...(lineLive ? [lineNode] : []), ...fresh, ...(oldNode ? [oldNode] : []), ...aged, footNode,
    ...(lineStands && !lineLive ? [lineNode] : [])]);
  return true;
}

/* ---- 지난 빌드 (11-10) ----
 * 끝난 지 `BUILD_RECENT_MS`가 넘은 실행: 통과든 실패든 취소든 「지금」의 일이 아니다. 접힌 줄 하나가 몇 개인지와
 * 이름을 말하고, 그 가운데 실패로 끝난 것이 있으면 그 수를 말한다 — 색만으로 말하지 않는다. */
const BUILD_RECENT_MS = 24 * 3_600_000;

function buildIsOld(build, now) {
  return BUILD_ENDED.has(build.state) && now - Number(build.updated_ms) >= BUILD_RECENT_MS;
}

function buildOldNode() {
  const fold = deskElement("button", "board-build-old");
  fold.type = "button";
  fold.insertAdjacentHTML("afterbegin", icon("chevron", false));
  fold.append(deskElement("span", "board-build-old-word"), deskElement("span", "board-build-old-note"));
  fold.onclick = () => {
    deskChoice.buildsOld = !deskChoice.buildsOld;
    scheduleDeskPaint();
  };
  return fold;
}

function paintBuildOld(fold, old, now) {
  const [, word, note] = fold.children;
  writeTextContent(word, t("board.build.old", "지난 빌드 {{count}}개", { count: old.length }));
  const failed = old.filter((build) => build.state === "failed").length;
  const latest = Math.max(...old.map((build) => Number(build.updated_ms) || 0));
  writeTextContent(note, [
    old.map((build) => buildWords(build.name, build.id)).join(" · "),
    failed > 0 ? t("board.build.oldFailed", "실패 {{count}}", { count: failed }) : "",
    latest > 0 ? t("board.build.oldLast", "마지막 {{at}}", { at: boardClock(latest, now) }) : "",
  ].filter(Boolean).join(" — "));
  writeAttribute(fold, "aria-expanded", String(deskChoice.buildsOld));
  fold.querySelector(".icon")?.classList.toggle("is-open", deskChoice.buildsOld);
}

/* ---- 빌드 줄 (11-08 시안 v3): 기계 하나에 줄 하나 — `zo line` ----
 * 어느 프로젝트의 무거운 일이든 이 기계의 빌드 칸에서 차례로 돈다. 카드 밑의 이 칸은 지금 도는 일, 기다리는 일(차례와
 * 함께), 방금 끝난 일을 한 줄씩 세우고, 그 자리의 단추(맨 앞으로 · 빼기 · 멈추기 · 다시 · 지우기)와 줄 멈추기를 둔다.
 * 창은 기록을 쓰지 않는다 — 단추는 `zo line`을 부른다(`line_act`·`line_pause`). */
const LINE_STATES = Object.freeze({
  waiting: { dot: "queue", key: "board.line.waiting", word: "차례 기다림", acts: ["front", "withdraw"] },
  running: { dot: "run", key: "board.line.running", word: "도는 중", acts: ["stop"] },
  passed: { dot: "done", key: "board.line.passed", word: "통과", acts: ["forget"] },
  failed: { dot: "halt", key: "board.line.failed", word: "실패", acts: ["retry", "forget"] },
  capped: { dot: "halt", key: "board.line.capped", word: "상한 넘어 끝남", acts: ["retry", "forget"] },
  stopped: { dot: "halt", key: "board.line.stopped", word: "멈춤", acts: ["retry", "forget"] },
  withdrawn: { dot: "queue", key: "board.line.withdrawn", word: "뺌", acts: ["forget"] },
  lost: { dot: "halt", key: "board.line.lost", word: "끝 모름", acts: ["retry", "forget"] },
});
const LINE_ACT_WORDS = Object.freeze({
  front: { key: "board.line.front", word: "맨 앞으로" },
  withdraw: { key: "board.line.withdraw", word: "빼기" },
  stop: { key: "board.line.stop", word: "멈추기" },
  retry: { key: "board.line.retry", word: "다시" },
  forget: { key: "board.line.forget", word: "지우기" },
});
let linePressing = false;

function buildLineNode() {
  const line = deskElement("section", "board-line");
  const head = deskElement("header", "board-line-head");
  const pause = deskElement("button", "board-build-aux board-line-pause");
  pause.type = "button";
  pause.onclick = () => void pressLinePause(!buildsAnswerHere()?.line?.pause);
  head.append(deskElement("strong", "board-line-title"), deskElement("span", "board-line-sum"), pause);
  const jobs = deskElement("ol", "board-line-jobs");
  jobs.onclick = (event) => {
    const button = event.target.closest("button[data-verb]");
    const job = buildsAnswerHere()?.line?.jobs?.find((one) => one.id === button?.closest("[data-job]")?.dataset.job);
    if (button && job) void pressLineAct(job, button.dataset.verb);
  };
  line.append(head, deskElement("p", "board-line-paused"), jobs);
  return line;
}

/* 한 일감의 「언제」: 도는 것은 걸린 시간, 기다리는 것은 차례, 끝난 것은 상태와 끝난 시각. */
function lineJobWhen(job, places, now) {
  if (job.state === "running") {
    return Number(job.started_ms) > 0 ? t("board.line.runningFor", "{{time}}째", { time: deskSpanWords(now - Number(job.started_ms)) }) : "";
  }
  if (job.state === "waiting") {
    const place = places?.[job.id];
    return Number.isInteger(place) ? t("board.line.place", "{{place}}번째", { place }) : "";
  }
  return Number(job.ended_ms) > 0 ? boardClock(Number(job.ended_ms), now) : "";
}

function paintBuildLine(node, line, now) {
  const [head, paused, list] = node.children;
  const [title, sum, pause] = head.children;
  const jobs = line?.jobs ?? [];
  writeTextContent(title, t("board.line.title", "빌드 줄 · 이 기계"));
  const running = jobs.filter((job) => job.state === "running").length;
  const waiting = jobs.filter((job) => job.state === "waiting").length;
  writeTextContent(sum, t("board.line.sum", "도는 {{running}} · 기다림 {{waiting}}", { running, waiting }));
  writeTextContent(pause, line?.pause ? t("board.line.resume", "줄 다시 열기") : t("board.line.pause", "줄 멈추기"));
  writeDisabled(pause, linePressing);
  const why = line?.pause?.why || line?.pause?.by || "";
  writeTextContent(paused, !line?.pause ? ""
    : why ? t("board.line.paused", "줄 멈춤 · {{why}}", { why }) : t("board.line.pausedPlain", "줄 멈춤"));
  writeHidden(paused, !line?.pause);
  const held = new Map([...list.children].map((item) => [item.dataset.job, item]));
  reconcileElementOrder(list, jobs.map((job) => {
    const state = LINE_STATES[job.state] ?? LINE_STATES.lost;
    let item = held.get(job.id);
    if (!item) {
      item = deskElement("li", "board-line-job");
      item.append(deskElement("span", "board-state board-line-state"), deskElement("span", "board-line-what"),
        deskElement("span", "board-line-where"), deskElement("span", "board-line-when"), deskElement("span", "board-line-acts"));
    }
    writeAttribute(item, "data-job", job.id);
    writeAttribute(item, "data-state", job.state);
    const [dot, what, where, when, acts] = item.children;
    writeAttribute(dot, "data-dot", state.dot);
    writeTextContent(dot, t(state.key, state.word));
    const command = Array.isArray(job.command) ? job.command.join(" ") : "";
    writeTextContent(what, job.what || command);
    writeAttribute(what, "data-tip", command);
    writeTextContent(where, [basename(String(job.checkout ?? "")), job.owner].filter(Boolean).join(" · "));
    writeTextContent(when, lineJobWhen(job, line?.places, now));
    const buttons = new Map([...acts.children].map((button) => [button.dataset.verb, button]));
    reconcileElementOrder(acts, state.acts.map((verb) => {
      let button = buttons.get(verb);
      if (!button) {
        button = deskElement("button", "board-build-aux board-line-act");
        button.type = "button";
      }
      writeAttribute(button, "data-verb", verb);
      writeTextContent(button, t(LINE_ACT_WORDS[verb].key, LINE_ACT_WORDS[verb].word));
      writeDisabled(button, linePressing);
      return button;
    }));
    return item;
  }));
}

/* 한 일감에 단추 하나: `zo line <verb> <id>`. 지우기는 토스트에 되돌리기가 선다(`restore`). */
async function pressLineAct(job, verb) {
  if (linePressing) return;
  linePressing = true;
  scheduleDeskPaint();
  try {
    await invoke("line_act", { job: job.id, verb });
    if (verb === "forget") {
      toast(t("board.line.forgot", "{{what}} 기록을 지웠습니다", { what: job.what || job.id }), "", {
        action: { label: t("board.build.forgetUndo", "되돌리기"), run: () => void pressLineAct(job, "restore") },
      });
    }
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    linePressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

async function pressLinePause(pause) {
  if (linePressing) return;
  linePressing = true;
  scheduleDeskPaint();
  try {
    await invoke("line_pause", { pause, why: pause ? t("board.line.pausedFromBoard", "상황판에서 멈춤") : null });
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    linePressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

/* 카드 밑의 한 줄(빌드 카드 계획 5단계). 순서 파일이 없으면 이 프로젝트의 뿌리에서 찾은 빌드를 말하고 — 감지는
 * 제안일 뿐이고, 파일은 「순서 정하기」를 눌러야 생긴다 —, 순서 파일을 그대로 돌릴 수 없으면 그 까닭을 말하고 그
 * 파일을 연다. 기록도 찾은 빌드도 돌릴 수 없는 파일도 없으면 줄도 카드도 서지 않는다(`stands`). */
function buildFoot(answer, builds) {
  const file = answer?.file ?? BUILD_FILE_NONE;
  const suggest = answer?.suggest ?? [];
  const runnable = answer?.runnable ?? [];
  if (file.state === "refused") {
    return {
      stands: true, act: "file", actWords: t("board.build.fileOpen", "파일 열기"), run: false,
      words: t("board.build.fileRefused", "순서 파일을 그대로 돌릴 수 없어요 · {{reason}}", { reason: file.reason ?? "" }),
    };
  }
  return {
    stands: builds.length > 0 || suggest.length > 0 || runnable.length > 0,
    act: "order", actWords: t("board.build.orderEdit", "순서 정하기"),
    run: runnable.length > 0 && !buildRunnerHolds(builds, runnable),
    words: suggest.length === 0 ? "" : t("board.build.suggest", "이 프로젝트에서 찾은 빌드 {{count}}개 · {{names}}", {
      count: suggest.length, names: suggest.map((one) => buildWords(one.name, one.id)).join(" · "),
    }),
  };
}

function buildFootNode() {
  const foot = deskElement("p", "board-build-foot");
  const run = deskElement("button", "board-build-foot-act is-primary");
  run.type = "button";
  run.dataset.act = "build";
  run.onclick = () => void pressBuild(buildsAnswerHere()?.runnable ?? []);
  const act = deskElement("button", "board-build-foot-act");
  act.type = "button";
  act.onclick = () => void (act.dataset.act === "file" ? openBuildOrderFile() : openBuildOrder());
  foot.append(deskElement("span", "board-build-foot-words"), run, act);
  return foot;
}

function paintBuildFoot(foot, said) {
  const [words, run, act] = foot.children;
  writeClass(foot, "is-refused", said.act === "file");
  writeTextContent(words, said.words);
  writeHidden(words, said.words === "");
  writeTextContent(run, t("board.build.run", "빌드 실행"));
  writeHidden(run, !said.run);
  writeDisabled(run, buildPressing);
  writeAttribute(act, "data-act", said.act);
  writeTextContent(act, said.actWords);
}

/* ---- 빌드 단추 (빌드 카드 계획 6단계) ----
 * 「빌드」·「다시」·누를 단계는 `zo build`를 창 밖에서 띄우고, 「멈추기」는 그것에 멈추라고 한다 — 창은 기록을 쓰지
 * 않고, 창이 닫혀도 빌드는 돈다. 아무도 허락하지 않은 명령이 있으면 아무것도 돌지 않고 그 명령을 보여 준다: 사람이
 * 「허락하고 빌드」를 눌러야 돈다(`build_start`). 단추는 `zo build`가 돌리는 빌드에만 선다(`runnable`) — 다른
 * 스크립트가 알리는 빌드는 그 스크립트가 돌린다. */
const BUILD_CARD_ACTS = Object.freeze(["step", "again", "stop"]);
const BUILD_CARD_ACT_WORDS = Object.freeze({
  again: { key: "board.build.again", word: "다시" },
  stop: { key: "board.build.stop", word: "멈추기" },
});
/* 곁의 둘: 실행의 로그 끝을 카드 안에 펴고, ⋯는 드문 일(로그 파일 열기 · 기록 지우기)을 든다. */
const BUILD_CARD_AUX = Object.freeze(["log", "more"]);

/* 누른 단추의 물음이 가는 동안은 단추가 다시 눌리지 않는다. */
let buildPressing = false;
/* 허락을 기다리는 누름: `{ builds, step, commands }`. */
let buildApproval = null;

/* `zo build`의 러너가 이 프로젝트를 쥐고 있는가 — 그것이 돌리는 빌드가 돌거나 차례를 기다린다. 누를 단계에서
 * 기다리는 빌드는 러너가 쥐지 않는다. */
function buildRunnerHolds(builds, runnable) {
  return builds.some((build) => runnable.includes(build.id)
    && (build.state === "running" || (build.state === "waiting" && !build.waiting)));
}

/* 카드의 단추: 누를 단계에서 기다리면 그 단계와 「멈추기」, 돌거나 차례를 기다리면 「멈추기」, 끝났으면 「다시」 —
 * 러너가 이 프로젝트를 쥔 동안 「다시」는 서지 않는다. 다른 스크립트가 알리는 빌드는 프로젝트가 순서 파일에
 * 그 다시·멈추기 명령을 적었을 때만 같은 단추를 든다(`report`). */
function buildCardActs(build, hands) {
  if (!hands.runnable) {
    const report = hands.report ?? {};
    if (BUILD_LIVE.has(build.state)) return report.stop ? ["stop"] : [];
    return report.retry ? ["again"] : [];
  }
  if (build.state === "waiting" && build.waiting) return ["step", "stop"];
  if (BUILD_LIVE.has(build.state)) return ["stop"];
  return hands.runnerHolds ? [] : ["again"];
}

/* 「다시」가 어디서 다시 도는가: 실패하거나 멈춘 단계가 첫 단계가 아니면 그 단계부터(GitHub Actions의 「실패한 것만
 * 다시」, CircleCI의 「실패한 곳부터」), 아니면 처음부터. 단계부터는 `zo build`가 도는 빌드만 — 다른 스크립트는 제
 * 명령 하나로 다시 돈다. */
function buildAgainFrom(build, hands) {
  if (!hands.runnable) return null;
  const halted = buildHaltedStep(build);
  const steps = Array.isArray(build.steps) ? build.steps : [];
  return halted && steps.indexOf(halted) > 0 ? halted : null;
}

function paintBuildActs(acts, build, hands) {
  const shown = buildCardActs(build, hands);
  const from = shown.includes("again") ? buildAgainFrom(build, hands) : null;
  const ended = BUILD_ENDED.has(build.state);
  for (const button of acts.querySelectorAll(":scope > .board-build-act")) {
    const act = button.dataset.act;
    writeHidden(button, !shown.includes(act));
    writeDisabled(button, buildPressing);
    const said = BUILD_CARD_ACT_WORDS[act];
    writeTextContent(button, act === "again" && from
      ? t("board.build.againFrom", "{{step}}부터 다시", { step: buildWords(from.words, from.name) })
      : said
        ? t(said.key, said.word)
        : t("board.build.stepRun", "{{step}} 실행", { step: buildWaitingStepWords(build) }));
  }
  const caret = acts.querySelector(":scope > .board-build-caret");
  writeHidden(caret, !from);
  writeDisabled(caret, buildPressing);
  writeAttribute(caret, "aria-label", t("board.build.againMenu", "다시 돌릴 자리 고르기"));
  writeAttribute(caret, "data-tip", t("board.build.againMenu", "다시 돌릴 자리 고르기"));
  const hasLog = Boolean(build.log);
  const [log, more] = acts.querySelectorAll(":scope > .board-build-aux");
  writeHidden(log, !hasLog);
  writeTextContent(log, t("board.build.log", "로그"));
  const open = buildTailsOpen.has(buildRunKey(build));
  writeAttribute(log, "aria-expanded", String(open));
  const rare = ended || Boolean(build.log_path);
  writeHidden(more, !rare);
  writeAttribute(more, "aria-label", t("board.build.more", "더 보기"));
  writeAttribute(more, "data-tip", t("board.build.more", "더 보기"));
  writeAttribute(more, "aria-expanded", String(buildMenuOpen?.card === acts.parentElement.dataset.build && buildMenuOpen.menu === "more"));
  writeAttribute(caret, "aria-expanded", String(buildMenuOpen?.card === acts.parentElement.dataset.build && buildMenuOpen.menu === "again"));
  const menu = acts.querySelector(":scope > .board-build-menu");
  const menuOf = buildMenuOpen?.card === acts.parentElement.dataset.build ? buildMenuOpen.menu : null;
  paintBuildMenu(menu, menuOf, build, from, ended, hands);
  writeHidden(acts, shown.length === 0 && !hasLog && !rare);
}

/* 열린 메뉴 하나: `{ card, menu }` — 「다시 ▾」의 자리 고르기, 「⋯」의 드문 일. */
let buildMenuOpen = null;

function buildMenuItems(menu, build, from, ended, hands) {
  if (menu === "again") {
    return from ? [{ id: "again-start", words: t("board.build.againStart", "처음부터 다시") },
      { id: "again-from", words: t("board.build.againFrom", "{{step}}부터 다시", { step: buildWords(from.words, from.name) }) }] : [];
  }
  if (menu === "more") {
    const items = [];
    if (build.log_path) items.push({ id: "log-file", words: t("board.build.logFile", "로그 파일 열기") });
    if (ended) items.push({ id: "forget", words: t("board.build.forget", "기록 지우기"), halt: true });
    return items;
  }
  return [];
}

function paintBuildMenu(menu, which, build, from, ended, hands) {
  const items = which ? buildMenuItems(which, build, from, ended, hands) : [];
  writeHidden(menu, items.length === 0);
  writeAttribute(menu, "role", "menu");
  writeAttribute(menu, "data-menu", which ?? "");
  const held = new Map([...menu.children].map((node) => [node.dataset.item, node]));
  reconcileElementOrder(menu, items.map((item) => {
    let node = held.get(item.id);
    if (!node) {
      node = deskElement("button", "board-build-menu-item");
      node.type = "button";
    }
    writeAttribute(node, "role", "menuitem");
    writeAttribute(node, "data-item", item.id);
    writeClassName(node, `board-build-menu-item${item.halt ? " is-halt" : ""}`);
    writeTextContent(node, item.words);
    return node;
  }));
}

function closeBuildMenu() {
  if (!buildMenuOpen) return;
  buildMenuOpen = null;
  scheduleDeskPaint();
}

document.addEventListener("pointerdown", (event) => {
  if (buildMenuOpen && !event.target.closest?.(".board-build-acts")) closeBuildMenu();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && buildMenuOpen) closeBuildMenu();
});

function pressBuildCardButton(card, button) {
  if (!button || button.disabled) return;
  const id = card.dataset.build;
  const build = (buildsHere() ?? []).find((one) => one.id === id);
  if (!build) return;
  const hands = buildHandsOf(id);
  if (button.classList.contains("board-build-act")) {
    closeBuildMenu();
    pressBuildCardAct(build, hands, button.dataset.act);
    return;
  }
  if (button.classList.contains("board-build-caret") || button.dataset.act === "more") {
    const menu = button.dataset.menu ?? button.dataset.act;
    buildMenuOpen = buildMenuOpen?.card === id && buildMenuOpen.menu === menu ? null : { card: id, menu };
    scheduleDeskPaint();
    return;
  }
  if (button.dataset.act === "log") {
    toggleBuildTail(build);
    return;
  }
  if (button.classList.contains("board-build-menu-item")) {
    closeBuildMenu();
    const item = button.dataset.item;
    if (item === "again-start") void pressBuild([id]);
    else if (item === "again-from") void pressBuild([id], null, [], buildAgainFrom(build, hands)?.name ?? null);
    else if (item === "log-file" && build.log_path) void openFile(String(build.log_path));
    else if (item === "forget") void forgetBuildRun(build);
  }
}

function pressBuildCardAct(build, hands, act) {
  const id = build.id;
  if (!hands.runnable) {
    if (act === "again" || act === "stop") void pressBuildReport(id, act === "again" ? "retry" : "stop");
    return;
  }
  if (act === "stop") {
    void stopBuilds([id]);
    return;
  }
  if (act === "step") {
    void pressBuild([id], build.waiting ?? null);
    return;
  }
  void pressBuild([id], null, [], buildAgainFrom(build, hands)?.name ?? null);
}

/* 이 카드의 손: `zo build`가 돌리는가, 그 러너가 이 프로젝트를 쥐었는가, 다른 스크립트의 다시·멈추기 명령. */
function buildHandsOf(id) {
  const answer = buildsAnswerHere();
  const runnable = answer?.runnable ?? [];
  return {
    runnable: runnable.includes(id), runnerHolds: buildRunnerHolds(buildsHere() ?? [], runnable),
    report: answer?.report_acts?.[id] ?? null,
  };
}

/* ---- 로그 끝 (카드 안) ----
 * 「로그」는 그 실행의 로그 끝을 카드 안에 편다 — 일정 탭과 같은 물음(`schedule_timeline`의 `log`)이라 창은 기록이 연 실행의
 * 로그만 읽는다. 도는 실행은 기록이 바뀔 때마다 다시 읽는다. */
const buildTailsOpen = new Set();
const buildTails = new Map();

function buildRunKey(build) {
  return `${build.id}:${build.started_ms ?? ""}`;
}

function toggleBuildTail(build) {
  const key = buildRunKey(build);
  if (buildTailsOpen.has(key)) buildTailsOpen.delete(key);
  else {
    buildTailsOpen.add(key);
    void readBuildTail(build);
  }
  scheduleDeskPaint();
}

async function readBuildTail(build) {
  const key = buildRunKey(build);
  const stamp = Number(build.updated_ms) || 0;
  const held = buildTails.get(key);
  if (held && (held.stamp === stamp || held.asking)) return;
  buildTails.set(key, { lines: held?.lines ?? [], cut: held?.cut ?? false, missing: false, stamp, asking: true });
  try {
    const answer = await invoke("schedule_timeline", {
      worktree: activeWorktreePath, known: null, fromMs: null,
      log: { build: { id: build.id, started_ms: Number(build.started_ms) } },
    });
    const log = answer?.log ?? null;
    buildTails.set(key, { lines: Array.isArray(log?.lines) ? log.lines : [], cut: Boolean(log?.cut), missing: Boolean(log?.missing), stamp, asking: false });
  } catch (error) {
    buildTails.set(key, { lines: [], cut: false, missing: true, error: String(error?.message ?? error), stamp, asking: false });
  }
  scheduleDeskPaint();
}

function paintBuildTail(tail, build) {
  const key = buildRunKey(build);
  const open = buildTailsOpen.has(key);
  writeHidden(tail, !open);
  if (!open) return;
  const held = buildTails.get(key);
  if (!held || (held.stamp !== (Number(build.updated_ms) || 0) && BUILD_LIVE.has(build.state))) void readBuildTail(build);
  const lines = held?.lines ?? [];
  writeTextContent(tail, held?.asking && lines.length === 0 ? t("board.build.logReading", "로그를 읽는 중…")
    : held?.missing ? t("board.build.logMissing", "이 실행의 로그 파일이 없습니다")
      : lines.slice(-BUILD_TAIL_LINES).join("\n"));
}

/* 카드가 펴는 로그 끝의 줄 수 — 나머지는 「⋯ → 로그 파일 열기」. */
const BUILD_TAIL_LINES = 40;

/* ---- 기록 지우기 ----
 * 지우기는 `zo build forget`이 기록과 로그를 휴지통으로 옮기는 일이다(창은 기록을 쓰지 않는다). 확인 창 대신 5초 동안
 * 「되돌리기」가 서고(NN/g: 자주 묻는 확인은 습관으로 눌린다), 휴지통은 하루 뒤 비워진다. */
async function forgetBuildRun(build) {
  if (buildPressing) return;
  buildPressing = true;
  scheduleDeskPaint();
  const asked = { worktree: activeWorktreePath, build: build.id, startedMs: Number(build.started_ms) };
  try {
    await invoke("build_forget", asked);
    toast(t("board.build.forgot", "{{build}} {{at}} 기록을 지웠습니다", {
      build: buildWords(build.name, build.id), at: boardClock(Number(build.started_ms), Date.now()) }), "", {
      action: { label: t("board.build.forgetUndo", "되돌리기"), run: () => void restoreBuildRun(asked) },
    });
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    buildPressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

async function restoreBuildRun(asked) {
  try {
    await invoke("build_restore", asked);
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

/* 다른 스크립트가 알리는 빌드의 다시·멈추기: 프로젝트가 순서 파일에 적은 명령을 `zo build act`가 돌린다 — 아무도
 * 허락하지 않은 명령이면 빌드처럼 먼저 묻는다. */
async function pressBuildReport(id, act, allowed = []) {
  if (buildPressing) return;
  buildPressing = true;
  scheduleDeskPaint();
  try {
    const pressed = await invoke("build_act", { worktree: activeWorktreePath, build: id, act, allowed });
    if (pressed?.state === "approve") openBuildApproval({ builds: [id], step: null, from: null, act, commands: pressed.commands ?? [] });
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    buildPressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

/* 빌드를 띄우고, 허락할 명령이 남았으면 그 목록을 연다. 띄운 뒤에는 기록을 다시 묻는다 — `build_start`는 러너가
 * 이 프로젝트를 쥘 때까지 기다렸다 답하므로 다음 답에 그 빌드가 선다. */
async function pressBuild(builds, step = null, allowed = [], from = null) {
  if (buildPressing || builds.length === 0) return;
  buildPressing = true;
  scheduleDeskPaint();
  try {
    const pressed = await invoke("build_start", { worktree: activeWorktreePath, builds, step, from, allowed });
    if (pressed?.state === "approve") openBuildApproval({ builds, step, from, commands: pressed.commands ?? [] });
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    buildPressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

async function stopBuilds(builds) {
  if (buildPressing) return;
  buildPressing = true;
  scheduleDeskPaint();
  try {
    await invoke("build_stop", { worktree: activeWorktreePath, builds });
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
  } finally {
    buildPressing = false;
    await Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
    scheduleDeskPaint();
  }
}

function openBuildApproval(asked) {
  buildApproval = asked;
  el("build-approve-list").replaceChildren(...asked.commands.map((command) => {
    const item = document.createElement("li");
    const code = document.createElement("code");
    code.textContent = command;
    item.append(code);
    return item;
  }));
  showModal(el("build-approve-scrim"));
}

function closeBuildApproval() {
  buildApproval = null;
  hideModal(el("build-approve-scrim"));
}

/* 「허락하고 빌드」: 사람이 본 명령 그대로를 허락으로 보낸다 — 그 사이 파일이 바뀌었으면 새 명령을 다시 묻는다. */
function approveBuild() {
  const asked = buildApproval;
  closeBuildApproval();
  if (!asked) return;
  if (asked.act) void pressBuildReport(asked.builds[0], asked.act, asked.commands);
  else void pressBuild(asked.builds, asked.step, asked.commands, asked.from ?? null);
}

el("build-approve-yes").addEventListener("click", approveBuild);
el("build-approve-cancel").addEventListener("click", closeBuildApproval);

/* ---- 빌드 순서 (「순서 정하기」, 빌드 카드 계획 5단계) ----
 * 편집기는 이 프로젝트의 순서 파일로, 없으면 뿌리에서 찾은 빌드로 연다 — 기록이 있는데 파일에 없는 빌드는 다른
 * 스크립트가 알리는 자리로 끝에 붙는다(`build_order_draft`). 사람은 빌드의 차례를 바꾸고 뺀다. 쓰일 파일의 글은 늘
 * Rust가 짓고(`build_order_preview`) 창은 TOML을 짓지 않는다 — 그대로 돌릴 수 없는 순서면 까닭을 말하고 쓰지 않는다.
 * 파일은 미리 본 뒤 「이대로 쓰기」를 눌러야 생기고, 연 뒤에 바뀐 파일 위에는 쓰지 않는다(`build_order_write`). */
let buildOrder = null;

/* 행의 단추 셋. */
const BUILD_ORDER_ACTS = Object.freeze([
  { act: "up", glyph: "arrow-up", key: "buildOrder.up", word: "위로" },
  { act: "down", glyph: "arrow-down", key: "buildOrder.down", word: "아래로" },
  { act: "remove", glyph: "x", key: "buildOrder.remove", word: "빼기" },
]);

/* 편집기의 초안 — 이 체크아웃의 것이다. 묻는 사이 다른 프로젝트로 옮겼으면 버린다. */
async function askBuildOrderDraft() {
  const asked = activeWorktreePath;
  try {
    const draft = await invoke("build_order_draft", { worktree: asked });
    return asked === activeWorktreePath && draft ? { asked, draft } : null;
  } catch (error) {
    toast(String(error?.message ?? error), "halt");
    return null;
  }
}

async function openBuildOrder() {
  const opened = await askBuildOrderDraft();
  if (!opened) return;
  const { asked, draft } = opened;
  buildOrder = {
    asked, draft, builds: Array.isArray(draft.file?.build) ? [...draft.file.build] : [],
    preview: "", problem: "", asking: 0, pending: false, saving: false,
  };
  paintBuildOrder();
  showModal(el("build-order-scrim"));
  void previewBuildOrder();
}

/* 순서 파일을 편집기 탭에 연다 — 그대로 돌릴 수 없는 파일을 고치는 것은 사람의 손이다. */
async function openBuildOrderFile() {
  const path = buildOrder?.draft.path ?? (await askBuildOrderDraft())?.draft.path;
  if (!path) return;
  closeBuildOrder();
  await openFile(path);
}

function closeBuildOrder() {
  if (!buildOrder) return;
  buildOrder = null;
  hideModal(el("build-order-scrim"));
}

/* 지금 순서가 쓰일 파일의 글을 Rust에 묻는다 — 바뀔 때마다 한 번이고, 늦게 온 옛 답은 버린다. */
async function previewBuildOrder() {
  const order = buildOrder;
  if (!order) return;
  const asking = (order.asking += 1);
  if (order.builds.length === 0) {
    Object.assign(order, { preview: "", problem: t("buildOrder.empty", "빌드가 하나도 없으면 쓸 것이 없어요."), pending: false });
    paintBuildOrder();
    return;
  }
  order.pending = true;
  paintBuildOrder();
  let preview = "";
  let problem = "";
  try {
    preview = await invoke("build_order_preview", { file: { build: order.builds } });
  } catch (error) {
    problem = String(error?.message ?? error);
  }
  if (buildOrder !== order || order.asking !== asking) return;
  Object.assign(order, { preview: typeof preview === "string" ? preview : "", problem, pending: false });
  paintBuildOrder();
}

/* 행의 단추. 움직인 뒤에도 초점은 같은 빌드의 같은 단추에 남는다 — 끝에 닿아 막혔으면 그 행의 다른 단추에. */
function moveBuildOrder(id, act) {
  const order = buildOrder;
  const at = order ? order.builds.findIndex((build) => build.id === id) : -1;
  if (at < 0) return;
  if (act === "remove") order.builds.splice(at, 1);
  else {
    const to = act === "up" ? at - 1 : at + 1;
    if (to < 0 || to >= order.builds.length) return;
    [order.builds[at], order.builds[to]] = [order.builds[to], order.builds[at]];
  }
  paintBuildOrder();
  const list = el("build-order-list");
  const row = [...list.children].find((one) => one.dataset.build === id);
  const open = "[data-order-act]:not(:disabled)";
  (row?.querySelector(`[data-order-act="${act}"]:not(:disabled)`) ?? row?.querySelector(open) ?? list.querySelector(open)
    ?? el("build-order-cancel")).focus();
  void previewBuildOrder();
}

async function saveBuildOrder() {
  const order = buildOrder;
  if (!order || order.saving || order.pending || order.problem !== "" || order.preview === "") return;
  order.saving = true;
  paintBuildOrder();
  try {
    await invoke("build_order_write", { worktree: order.asked, file: { build: order.builds }, version: order.draft.version ?? null });
  } catch (error) {
    if (buildOrder !== order) return;
    Object.assign(order, { saving: false, problem: String(error?.message ?? error) });
    paintBuildOrder();
    return;
  }
  closeBuildOrder();
  toast(t("buildOrder.saved", "빌드 순서를 썼어요 · {{name}}", { name: order.draft.name }));
  // The card reads the new order — asked again once an ask that began before the write is in.
  void Promise.resolve(boardBuildsAsking).then(() => askBoardBuilds());
}

function paintBuildOrder() {
  const order = buildOrder;
  if (!order) return;
  const { draft } = order;
  const refused = draft.from === "refused";
  const from = el("build-order-from");
  writeTextContent(from, refused
    ? t("board.build.fileRefused", "순서 파일을 그대로 돌릴 수 없어요 · {{reason}}", { reason: draft.reason ?? "" })
    : draft.from === "file"
      ? t("buildOrder.fromFile", "{{name}}의 순서예요. 빌드는 위에서부터 차례로 돌아요.", { name: draft.name })
      : t("buildOrder.fromSuggestion", "이 프로젝트에서 찾은 빌드예요. 아무것도 돌지 않고, 「이대로 쓰기」를 눌러야 {{name}}이 생겨요.", {
        name: draft.name,
      }));
  writeClass(from, "is-refused", refused);
  const list = el("build-order-list");
  writeHidden(list, refused);
  const names = new Map(order.builds.map((build) => [build.id, buildWords(build.name, build.id)]));
  const held = new Map([...list.children].map((row) => [row.dataset.build, row]));
  reconcileElementOrder(list, order.builds.map((build, index) => {
    const row = held.get(build.id) ?? buildOrderRow();
    paintBuildOrderRow(row, build, index, order.builds.length, names);
    return row;
  }));
  const note = el("build-order-note");
  const words = refused ? "" : order.problem || (draft.comments ? t("buildOrder.comments", "쓰면 파일의 주석은 남지 않아요.") : "");
  writeTextContent(note, words);
  writeHidden(note, words === "");
  writeClass(note, "is-problem", order.problem !== "");
  writeTextContent(el("build-order-preview"), order.preview);
  writeHidden(el("build-order-preview-box"), refused || order.preview === "");
  const save = el("build-order-save");
  writeHidden(save, refused);
  writeDisabled(save, order.pending || order.saving || order.problem !== "" || order.preview === "");
  writeHidden(el("build-order-open"), draft.from === "suggestion");
}

function buildOrderRow() {
  const row = deskElement("li", "build-order-row");
  const head = deskElement("p", "build-order-head");
  head.append(deskElement("strong", "build-order-name"), deskElement("span", "board-build-tool"));
  const what = deskElement("div", "build-order-what");
  what.append(head, deskElement("p", "build-order-steps"));
  const moves = deskElement("div", "build-order-moves");
  for (const { act, glyph } of BUILD_ORDER_ACTS) {
    const button = deskElement("button", "icon-btn");
    button.type = "button";
    button.dataset.orderAct = act;
    button.insertAdjacentHTML("afterbegin", icon(glyph));
    moves.append(button);
  }
  row.append(deskElement("span", "build-order-at"), what, moves);
  return row;
}

function paintBuildOrderRow(row, build, index, count, names) {
  const [at, what, moves] = row.children;
  const [head, steps] = what.children;
  const [name, tool] = head.children;
  const called = names.get(build.id);
  writeAttribute(row, "data-build", build.id);
  writeTextContent(at, String(index + 1));
  writeTextContent(name, called);
  writeTextContent(tool, build.tool ?? "");
  writeHidden(tool, !build.tool);
  writeTextContent(steps, buildOrderStepsWords(build, names));
  for (const button of moves.children) {
    const said = BUILD_ORDER_ACTS.find((one) => one.act === button.dataset.orderAct);
    const word = t(said.key, said.word);
    writeAttribute(button, "data-tip", word);
    writeAttribute(button, "aria-label", `${word} · ${called}`);
    writeDisabled(button, (said.act === "up" && index === 0) || (said.act === "down" && index === count - 1));
  }
}

/* 행의 둘째 줄: 단계 이름들(손으로 누르는 단계는 그렇다고), 다른 스크립트가 알리는 빌드는 그렇다고, 기다리는 빌드가
 * 있으면 그 이름. */
function buildOrderStepsWords(build, names) {
  const steps = build.source === "report"
    ? [t("buildOrder.reported", "다른 스크립트가 알려요")]
    : (Array.isArray(build.step) ? build.step : []).map((step) => {
      const name = buildWords(step.name, step.id);
      return step.when === "manual" ? t("buildOrder.manual", "{{name}} (손으로)", { name }) : name;
    });
  const after = build.after ? [t("buildOrder.after", "{{name}} 뒤에 돌아요", { name: names.get(build.after) ?? build.after })] : [];
  return [...steps, ...after].join(" · ");
}

el("build-order-list").addEventListener("click", (event) => {
  const button = event.target.closest("[data-order-act]");
  const row = button?.closest(".build-order-row");
  if (row && !button.disabled) moveBuildOrder(row.dataset.build, button.dataset.orderAct);
});
el("build-order-save").addEventListener("click", () => void saveBuildOrder());
el("build-order-open").addEventListener("click", () => void openBuildOrderFile());
el("build-order-cancel").addEventListener("click", closeBuildOrder);

/* 데스크가 보이는 동안의 1분 박자 하나 (t-67607 → t-70028). 느린 사실을 다시 묻고, 그 답을
 * 기다리지 않고 판을 한 번 다시 그린다 — 「N분째」·「N분 전」·단계의 경과는 원장이 움직이지 않아도
 * 1분마다 바뀌는 글자다. 그림은 바뀐 글자 노드만 쓰고(`writeTextContent`), 창이 숨었거나 작업
 * 보기가 서 있지 않으면 박자도 서지 않는다(`idlePoller`). */
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

/* 도는 빌드를 따라가는 박자(`DESK.buildsEveryMs`): 빌드가 화면에 서 있고 그중 하나가 아직 끝나지 않은
 * 동안만 돈다. 답이 올 때마다 다시 잰다(`askBoardBuilds`). */
const boardBuildsPoller = idlePoller({
  wanted: () => ((boardNowTab === "now" && deskViews().length > 0) || stackedWorkIsUp())
    && (boardBuildsAnswer?.builds ?? []).some((build) => BUILD_LIVE.has(build.state)),
  every: DESK.buildsEveryMs,
  tick: askBoardBuilds,
  onResume: askBoardBuilds,
});

/* 판도 쌓인 일도 서 있지 않을 때: 쌓인 일 탭의 「빌드 실패」(shell-scm.js)는 늘 서 있으므로 상태 바의
 * 느린 박자(`USAGE_AMBIENT_MS`)로 묻는다. */
const boardBuildsAmbient = idlePoller({
  wanted: () => true,
  every: USAGE_AMBIENT_MS,
  tick: askBoardBuilds,
  onResume: askBoardBuilds,
});
boardBuildsAmbient.sync();
