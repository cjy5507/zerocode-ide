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

/* 사람이 고른 것: 목록을 펼친 단계, 우편을 다 펼쳤는가. 판이 다시 그려져도
 * 남는다 — 창의 기억이지 원장의 것이 아니다. */
const deskChoice = { stage: undefined, mailAll: false };

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
      const fresh = deskAgents.length === 0 && Array.isArray(agents) && agents.length > 0;
      deskLedgerSaid = said;
      deskLedger = answer && typeof answer === "object" ? answer : null;
      deskAgents = Array.isArray(agents) ? agents : [];
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
  void askReleaseStatus().then(scheduleDeskPaint);
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

/* 데스크 블록, 읽는 차례대로 — DOM 순서가 곧 좁은 판의 순서다. 그리는 손은
 * 블록에 쓸 것이 있으면 참을 돌려주고, 거짓이면 그 블록은 접힌다. */
const DESK_BLOCKS = Object.freeze([
  { id: "machine", paint: paintDeskMachine, quietHead: true },
  { id: "mail", paint: paintDeskMail },
  { id: "workers", paint: paintDeskWorkers },
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

/* ---- 답할 우편 --------------------------------------------------------------
 *
 * `check --peek`의 자리: 판 위의 런의 코디네이터가 빚진 편지(`desk_mail`) — 답을
 * 기다리는 질문, 그리고 워커가 멈췄다는 원장의 소식(한도 벽·끝남·조용해짐·서로
 * 기다림). 오래된 것이 먼저이고, 행마다 받은편지함의 상태(배달 전·받음·확인함)와
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
 * 실패의 표식이다. */
const DESK_MAIL = Object.freeze({
  question: { state: "needs-attention", key: "board.desk.mailQuestion", word: "질문" },
  quota_walled: { state: "needs-attention", key: "board.desk.mailWalled", word: "한도 벽" },
  worker_died: { state: "failed", key: "board.desk.mailDied", word: "워커 끝남" },
  went_quiet: { state: "needs-attention", key: "board.desk.mailQuiet", word: "조용해짐" },
  deadlocked: { state: "needs-attention", key: "board.desk.mailDeadlocked", word: "서로 기다림" },
});

/* 조용해진 까닭, 원장의 낱말마다. 표에 없는 낱말은 원장의 말 그대로 선다. */
const DESK_QUIET_REASONS = Object.freeze({
  stalled: { key: "board.desk.quietStalled", word: "보고 없이 턴이 멈춤" },
  quota_lifted: { key: "board.desk.quietQuotaLifted", word: "한도가 풀린 뒤에도 멈춰 있음" },
  pane_missing: { key: "board.desk.quietPaneMissing", word: "판이 보이지 않음" },
  never_spoke: { key: "board.desk.quietNeverSpoke", word: "한 번도 보고하지 않음" },
  judged: { key: "board.desk.quietJudged", word: "멈춘 까닭을 따로 판정함" },
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
    const reason = DESK_QUIET_REASONS[letter.reason];
    return reason ? t(reason.key, reason.word) : String(letter.reason ?? "");
  }
  if (letter.kind === "worker_died") return t("board.desk.mailDiedCopy", "보고 전에 판이 끝났어요");
  if (letter.kind === "deadlocked") return t("board.desk.mailDeadlockedCopy", "서로의 답을 기다리는 고리에 들었어요");
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
    await invoke("desk_ack", { run: letter.run, delivery: key, retryRequest: held.retry.id });
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

function paintDeskLetter(row, letter, seat, now, view) {
  row.__letter = letter;
  writeAttribute(row, "data-letter", deskLetterKey(letter));
  const kind = DESK_MAIL[letter.kind] ?? DESK_MAIL.went_quiet;
  writeClassName(row, `board-desk-letter is-${letter.kind} is-${letter.delivery}`);
  dressAgentGraphStateMark(row.querySelector(".agent-graph-node-state"), kind.state);
  writeTextContent(row.querySelector(".board-desk-letter-kind"), t(kind.key, kind.word));
  const who = [letter.worker, letter.task].filter(Boolean).join(" · ");
  writeTextContent(row.querySelector(".board-desk-letter-who"), who);
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

function paintDeskMail(block, now, view) {
  const letters = Array.isArray(deskLedger?.mail) ? deskLedger.mail : [];
  if (letters.length === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.mail", "답할 우편 · {{count}}", { count: letters.length }));
  const body = block.lastElementChild;
  let list = body.querySelector(":scope > .board-desk-letters");
  if (!list) {
    list = deskElement("ol", "board-desk-letters");
    const more = deskElement("button", "board-desk-letters-more");
    more.type = "button";
    more.onclick = () => {
      deskChoice.mailAll = !deskChoice.mailAll;
      paintCoordinatorDesk(view);
    };
    body.replaceChildren(list, more);
  }
  const seats = new Map((deskLedger.runs ?? []).map((run) => [run.run, run.seat === true]));
  const shown = deskChoice.mailAll ? letters : letters.slice(0, DESK.mailShown);
  let unseated = body.querySelector(":scope > .board-desk-unseated");
  if (!unseated) {
    unseated = deskElement("p", "board-desk-unseated");
    body.prepend(unseated);
  }
  const strangers = !isPopout && shown.some((letter) => !(seats.get(letter.run) ?? false));
  writeTextContent(unseated, strangers
    ? t("board.desk.noSeat", "이 창에 그 런의 코디네이터 자리가 없어 여기서는 답할 수 없어요") : "");
  writeHidden(unseated, !strangers);
  const held = new Map([...list.children].map((node) => [node.dataset.letter, node]));
  reconcileElementOrder(list, shown.map((letter) => {
    const row = held.get(deskLetterKey(letter)) ?? deskLetterRow(view);
    paintDeskLetter(row, letter, seats.get(letter.run) ?? false, now, view);
    return row;
  }));
  const more = body.querySelector(".board-desk-letters-more");
  const rest = letters.length - DESK.mailShown;
  writeHidden(more, rest <= 0);
  writeTextContent(more, deskChoice.mailAll ? t("board.desk.mailFewer", "접기")
    : t("board.desk.mailMore", "{{count}}통 더 보기", { count: Math.max(0, rest) }));
  writeAttribute(more, "aria-expanded", String(deskChoice.mailAll));
  // Letters that left the ledger's list take their drafts and presses with them.
  const standing = new Set(letters.map((letter) => letter.id));
  for (const id of deskDrafts.keys()) if (!standing.has(id)) deskDrafts.delete(id);
  const batches = new Set(letters.map((letter) => letter.delivery_id).filter(Boolean));
  for (const id of deskAcks.keys()) if (!batches.has(id)) deskAcks.delete(id);
  return true;
}

/* ---- 워커 ------------------------------------------------------------------
 *
 * `worker-list`의 자리: 원장이 아직 부르고 있는 워커마다 한 줄 — 건강 한 낱말,
 * 마지막 활동의 나이, 에이전트·모델·판·체크아웃, 그 체크아웃의 앞선 커밋과 바뀐
 * 파일. 건강의 사실은 원장의 것이다(판 없음은 조정자의 증명, 한도 벽은 원장이 다시
 * 읽은 벽, 답 기다림은 원장의 `awaiting_reply`, 잠듦은 원장의 낱말). 턴 중과
 * 유휴는 판의 훅이 말한 상태다. 이 표는 그 사실들에 붙는 낱말과 차례일 뿐이다 —
 * 위의 것이 먼저 맞으면 아래는 묻지 않는다. */
const DESK_HEALTH = Object.freeze([
  { id: "gone", state: "failed", key: "board.desk.healthGone", word: "판 없음",
    holds: (row) => Number.isFinite(row.pane_missing_since_ms) },
  { id: "walled", state: "needs-attention", key: "board.desk.healthWalled", word: "한도 벽",
    holds: (row, card, now) => Boolean(row.wall) && now < row.wall.stands_until_ms },
  { id: "asking", state: "needs-attention", key: "board.desk.healthAsking", word: "답 기다림",
    holds: (row, card) => row.asking === true || card?.state === "needs-attention" },
  { id: "asleep", state: "idle", key: "board.desk.healthAsleep", word: "잠듦",
    holds: (row) => row.ledger === "sleeping" },
  { id: "turn", state: "working", key: "board.desk.healthTurn", word: "턴 중",
    holds: (row, card) => card?.state === "working" },
  { id: "idle", state: "idle", key: "board.desk.healthIdle", word: "유휴", holds: () => true },
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

function deskWorkerRow(view) {
  const row = deskElement("li", "board-desk-worker");
  const button = deskElement("button", "board-desk-worker-main");
  button.type = "button";
  const line = deskElement("span", "board-desk-worker-line");
  line.append(agentGraphStateMark("idle"), deskElement("strong", "board-desk-worker-id"),
    deskElement("span", "board-desk-worker-health"), deskElement("span", "board-desk-worker-age"));
  const what = deskElement("span", "board-desk-worker-what");
  what.append(deskElement("span", "board-desk-worker-task"), deskElement("span", "board-desk-worker-facts"));
  button.append(line, what);
  button.onclick = () => {
    const term = row.__term;
    if (term != null) selectTaskBoardMember(view, `agent:term:${term}`);
  };
  row.append(button);
  return row;
}

function paintDeskWorkers(block, now, view) {
  const rows = deskAgents;
  if (rows.length === 0) return false;
  writeTextContent(block.firstElementChild, t("board.desk.workers", "워커 · {{count}}", { count: rows.length }));
  const body = block.lastElementChild;
  let list = body.firstElementChild;
  if (!list) {
    list = deskElement("ol", "board-desk-workers");
    body.replaceChildren(list);
  }
  const model = agentGraphModels.get(view);
  const cards = new Map((model?.agents ?? []).map((entry) => [entry.card.pane, entry.card]));
  const judged = rows.map((row) => {
    const card = row.term != null ? cards.get(`term:${row.term}`) : null;
    return { row, card, health: deskWorkerHealth(row, card, now) };
  }).sort((a, b) => DESK_HEALTH.indexOf(a.health) - DESK_HEALTH.indexOf(b.health) ||
    (Number(a.row.at) || 0) - (Number(b.row.at) || 0));
  const held = new Map([...list.children].map((node) => [node.dataset.worker, node]));
  reconcileElementOrder(list, judged.map(({ row, card, health }) => {
    const node = held.get(`${row.run}/${row.worker}`) ?? deskWorkerRow(view);
    node.__term = card ? row.term : null;
    writeAttribute(node, "data-worker", `${row.run}/${row.worker}`);
    writeClassName(node, `board-desk-worker is-${health.id}`);
    dressAgentGraphStateMark(node.querySelector(".agent-graph-node-state"), health.state);
    writeTextContent(node.querySelector(".board-desk-worker-id"), row.worker);
    const word = t(health.key, health.word);
    const reset = health.id === "walled" && Number.isFinite(row.wall?.resets_at_ms)
      ? usageCountdown(row.wall.resets_at_ms - now) : "";
    writeTextContent(node.querySelector(".board-desk-worker-health"), reset ? [word, reset].join(" · ") : word);
    const at = deskWorkerAt(row, card);
    writeTextContent(node.querySelector(".board-desk-worker-age"), at > 0
      ? t("board.desk.mailAge", "{{time}} 전", { time: agoWord(at, now) }) : "");
    const review = ledgerReviewWord({ reported: row.reported, review: row.review });
    writeTextContent(node.querySelector(".board-desk-worker-task"),
      [row.task || row.task_id, review].filter(Boolean).join(" · "));
    writeTextContent(node.querySelector(".board-desk-worker-facts"), deskWorkerFacts(row, now));
    const main = node.querySelector(".board-desk-worker-main");
    writeDisabled(main, node.__term == null);
    return node;
  }));
  return true;
}

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
    // The flow and the stuck stages are two runs of chips, so a narrow block
    // breaks between them rather than inside either.
    strip.append(deskElement("span", "board-desk-stage-run is-flow"), deskElement("span", "board-desk-stage-run"));
    body.replaceChildren(strip, deskElement("ol", "board-desk-tasks"), deskElement("p", "board-desk-more"));
  }
  writeAttribute(strip, "aria-label", t("board.desk.pipeline", "과업 흐름 · {{count}}", { count: total }));
  let open = deskChoice.stage === undefined ? deskDefaultStage(counts) : deskChoice.stage;
  if (open && (counts.get(open) ?? 0) === 0) open = deskDefaultStage(counts);
  const [flowRun, stuckRun] = strip.children;
  for (const [run, flow] of [[flowRun, true], [stuckRun, false]]) {
    reconcileElementOrder(run, DESK_STAGES.filter((stage) => stage.flow === flow).map((stage) => {
      const chip = deskStageChip(run, stage, counts, view);
      writeAttribute(chip, "aria-pressed", String(open === stage.id));
      return chip;
    }));
  }
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
