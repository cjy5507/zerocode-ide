/* ---- 작업 상황판 「지금」 (t-67607) ------------------------------------------
 *
 * 사람이 통과시킨 시안 B(Vercel Geist 언어, 아티팩트 p-90883818b247d08e v2)의
 * 정보 구조. 상황판은 네 탭이다: 「지금」은 맨 위 한 문장(답할 일 N개 · 다음에
 * 끝나는 일), 「당신 차례」 카드(질문·결정·검토, 행 안에 첫 행동), 「빌드 줄」
 * 카드(릴리즈 레인), 「지금 도는 일」 표(목표별로 접힘). 「일정」은 빌드 줄·결합
 * 게이트·발행 레인·워커 과업의 시간축이다(shell-board-schedule.js, t-67482).
 * 「워커」는 모델·판·비용·호출·바뀐 파일을 여기서만 보이고, 「기록」은 소식·과업
 * 흐름의 지난 수·에이전트 작업 목록이다. 수는 첫 화면에서 빠져 기록으로 갔다 — 지금
 * 일과 겨루지 않게.
 *
 * 그리는 사실은 데스크가 이미 읽은 한 벌뿐이다(`deskLedger`·`deskAgents`·
 * `releaseLaneHere()`, shell-board.js). 여기는 그 사실에 붙는 낱말과 차례를 정한다 —
 * 원장의 영어 문장은 행에 서지 않는다: 게이트는 종류(`gate.kind`)마다 우리말 한
 * 줄이고, 원문은 「원문 보기」 안에 있다.
 *
 * 사람 10-22(t-70028): 「화면 크기에 맞추고」·「시안대로 디자인이 안 나옴」 — 판은 창의
 * 폭을 다 쓰고, 행은 시안이 그린 그대로다: 상태는 낱말 곁의 채운 점, 한 줄의 메타는
 * 목표 · 과업과 id · 모델 · 나이, 워커 칸은 모델의 표시 이름, 가지 칸은 가지 이름.
 * 시간 글자(「N분째」·「N분 전」)는 데스크의 1분 박자가 고친다(`deskMinute`). */

/* 네 탭, 읽는 차례대로. 고른 탭은 창의 기억이다 — 판이 다시 그려져도 남는다. 낱말은
 * 마크업의 것이다(ui/index.html, 탭 단추의 `data-i18n`). 이름이 `boardNowTab`인 것은
 * shell.js의 `boardTab()`(보드를 든 창 탭)과 한 전역 이름을 나누면 늦게 실린 파일이
 * 통째로 서지 못하기 때문이다. */
const BOARD_TABS = Object.freeze([
  { id: "now" },
  { id: "schedule" },
  { id: "workers" },
  { id: "history" },
]);
let boardNowTab = "now";

/* 당신 차례에 처음부터 보이는 행 수. 나머지는 「N개 더 보기」 뒤에 선다 — 답할 일이
 * 스무 개여도 지금 도는 일이 화면 밖으로 밀려나지 않게. */
const BOARD_ASKS_SHOWN = 4;

/* 사람이 고른 것: 당신 차례를 다 펼쳤는가, 접은 목표, 펼친 원문·보고, 쓰는 중인 답.
 * 판이 다시 그려져도 남는다. */
const boardNowChoice = {
  asksAll: false, folded: new Set(), goalsAll: new Set(), raw: new Set(), reports: new Set(), answers: new Map(),
};

/* 누른 결정·통과(행 키마다): 보내는 중인가, 원장이 받았는가, 원장이 돌려준 답, 틀린 말.
 * 보낸 요청의 이름은 답이 불확실하면 그대로 남아 다시 누르면 같은 영수증을 되받는다.
 * 원장이 받은 행의 단추는 원장이 그 행을 목록에서 뺄 때까지 닫혀 있다. */
const boardPresses = new Map();

/* 탭의 단추를 그 탭에 맞게 — 표면의 `data-board-tab` 하나가 어느 블록이 보이는지
 * 정한다(shell.css 「작업 상황판 · 시안 B」). 블록의 `hidden`은 「쓸 것이 없음」의 뜻
 * 그대로 남는다. */
function paintBoardTabs(view) {
  const surface = view.querySelector(".task-board-surface");
  const strip = surface?.querySelector(".board-now-tabs");
  if (!strip) return;
  writeAttribute(surface, "data-board-tab", boardNowTab);
  // 원장의 런이 하나도 없는 창(코디네이터 없이 에이전트만 쓰는 사람)에는 탭이 나눌 것이
  // 없다: 탭과 맨 위 문장은 접히고, 에이전트 작업 목록이 예전처럼 판 전체에 선다.
  writeAttribute(surface, "data-board-ledger", (deskLedger?.runs?.length ?? 0) > 0 ? "on" : "none");
  if (!strip.__wired) {
    strip.__wired = true;
    strip.addEventListener("click", (event) => {
      const button = event.target.closest("[data-board-tab-button]");
      if (button) chooseBoardTab(view, button.dataset.boardTabButton, false);
    });
    // 탭 목록의 화살표 — 고른 탭만 Tab 차례에 서고(roving tabindex), 좌우가 옮긴다.
    strip.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
      const at = BOARD_TABS.findIndex((tab) => tab.id === boardNowTab);
      const step = event.key === "ArrowRight" ? 1 : BOARD_TABS.length - 1;
      event.preventDefault();
      chooseBoardTab(view, BOARD_TABS[(at + step) % BOARD_TABS.length].id, true);
    });
  }
  for (const button of strip.querySelectorAll("[data-board-tab-button]")) {
    const chosen = button.dataset.boardTabButton === boardNowTab;
    writeAttribute(button, "aria-selected", String(chosen));
    writeAttribute(button, "tabindex", chosen ? "0" : "-1");
  }
}

function chooseBoardTab(view, id, focus) {
  if (!BOARD_TABS.some((tab) => tab.id === id)) return;
  const moved = boardNowTab !== id;
  boardNowTab = id;
  paintBoardTabs(view);
  if (moved) paintCoordinatorDesk(view);
  if (focus) view.querySelector(`.board-now-tabs [data-board-tab-button="${id}"]`)?.focus();
}

/* ---- 끝나는 시각 ----------------------------------------------------------
 *
 * 과업 행의 `times`(데스크, t-67482)는 시작·넘김·검증·착지 시각을 싣는다. 끝날
 * 시각(`times.eta_ms`)이 실려 오면 그 값을 읽는 자리는 여기 하나다. 아직 오지 않은
 * 동안은 모른다고 하고 시각을 지어내지 않는다 — 도는 일은 「N분째」, 기다리는 일은
 * 「차례 기다림」으로 선다. */
function deskTaskEta(task, now) {
  const eta = Number(task?.times?.eta_ms);
  return Number.isFinite(eta) && eta > now ? eta : null;
}

/* 상황판의 시각 한 마디 — 시안의 「15:05쯤」·「07:31 끝남」: 하루 안이면 24시간 시:분, 그 밖이면
 * 날짜. 브라우저의 언어가 아니라 창이 고른 언어로. 같은 분, 같은 언어면 같은 글이라 일정의
 * 기억(`boardScheduleRemembered`)에 둔다 — 형식기는 부를 때마다 새로 지어진다. */
function boardClock(ms, now) {
  const tongue = document.documentElement.lang || undefined;
  const near = Math.abs(now - ms) < 86_400_000;
  return boardScheduleRemembered(`board-clock:${Math.floor(ms / 60_000)}:${near}:${tongue}`, () => (near
    ? new Date(ms).toLocaleTimeString(tongue, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" })
    : new Date(ms).toLocaleDateString(tongue)));
}

/* ---- 게이트의 우리말 한 줄 -------------------------------------------------
 *
 * 원장이 게이트에 적는 것은 영어 문장이다(`unexamined_question`·`examined_facts`,
 * zerocode-core). 데스크는 그 문장 옆에 종류와 사실을 실어 온다(`DeskGate.kind`·
 * `examined`, desk.rs) — 여기는 종류마다 무슨 일이고 무엇을 할지 한 줄을 고른다.
 * 새 종류를 이 표가 아직 모르면 「살펴보지 못함」의 줄로 읽는다: 원장의 문장을
 * 행에 그대로 세우지 않는다. `asked`는 누군가 제 말로 적은 결정이라 그 말 그대로다. */
const DESK_HOLD_SUMMARY = Object.freeze({
  unexamined: { key: "board.now.holdUnexamined", word: "{{worker}} 워커가 보고 전에 끝났습니다 · 창이 남긴 폴더를 곧 살펴봅니다" },
  unlanded: { key: "board.now.holdUnlanded", word: "{{worker}} 워커가 착지 안 된 커밋 {{commits}}개를 남겼습니다 · 착지한 뒤 거두거나 다시 띄우세요" },
  uncommitted: { key: "board.now.holdUncommitted", word: "{{worker}} 워커가 커밋 안 한 변경 {{changes}}개를 남겼습니다 · 그 폴더에서 정리한 뒤 거두세요" },
  unknown: { key: "board.now.holdUnknown", word: "{{worker}} 워커의 폴더를 살펴보지 못했습니다 · 직접 열어 보고 거두거나 다시 띄우세요" },
});

/* 결정 행의 단추 셋 — 열기는 그 워커의 폴더를 작업 공간 보기로, 둘은 원장의 두 답
 * (`HOLD_ACTS`, desk.rs)이다. */
const DESK_HOLD_ACTS = Object.freeze([
  { id: "open", key: "board.now.holdOpen", word: "열기" },
  { id: "harvest", key: "board.now.holdHarvest", word: "거두기" },
  { id: "dispatch", key: "board.now.holdDispatch", word: "다시 띄우기" },
]);

function deskHoldWords(gate) {
  if (gate.kind === "asked") return gate.question;
  const summary = DESK_HOLD_SUMMARY[gate.kind] ?? DESK_HOLD_SUMMARY.unknown;
  const examined = gate.examined ?? {};
  return t(summary.key, summary.word, {
    worker: gate.worker ?? "",
    commits: Number(examined.commits) || 0,
    changes: Number(examined.changes) || 0,
  });
}

/* ---- 지금 도는 일의 상태 ----------------------------------------------------
 *
 * 표의 네 상태와 그 낱말. 표식은 시안 B의 채운 점이다(`.board-dot`, t-70028) — 도는 중은
 * 호박, 차례 기다림은 회색, 멈춤은 빨강, 끝남은 청록. 색만으로 말하지 않는다: 점 곁에
 * 낱말이 같이 서고, 점은 돌지 않는다(돌던 고리는 멈추면 멈춘 그림으로 읽혔다). */
const BOARD_RUN_STATES = Object.freeze({
  run: { key: "board.now.stateRun", word: "도는 중" },
  queue: { key: "board.now.stateQueue", word: "차례 기다림" },
  halt: { key: "board.now.stateHalt", word: "멈춤" },
  done: { key: "board.now.stateDone", word: "끝남" },
});

/* 원장의 단계 → 표의 상태. 표에 없는 단계(닫힘·검토 자료 없음 같은 끝난 뒤의 것)는
 * 표에 서지 않는다 — 기록 탭의 과업 흐름이 센다. 보고가 와서 검토를 기다리는 과업도 표에
 * 없다: 그것은 당신 차례의 행이다(시안의 표에는 그 과업이 없다). */
const BOARD_RUN_OF_STAGE = Object.freeze({
  dispatched: "run",
  ready: "queue",
  pending: "queue",
  gate: "halt",
  blocked: "halt",
  failed: "halt",
  verified: "done",
  merged: "done",
  deployed: "done",
});

/* 끝난 행의 낱말 — 시안의 말: 병합된 과업은 「착지」, 발행된 과업은 「발행됨」. 그 밖의 끝난
 * 단계는 과업 흐름의 칩과 같은 말이다(`deskStageWord`). */
const BOARD_DONE_WORDS = Object.freeze({
  merged: { key: "board.now.doneLanded", word: "착지" },
  deployed: { key: "board.now.doneShipped", word: "발행됨" },
});

/* 글자 노드 하나를 제 자리에 — 아이콘 곁의 끝 글처럼 상자 없이 서는 글. 같은 글이면 쓰지
 * 않는다(`writeTextContent`의 짝). */
function writeTrailingText(node, value) {
  const text = node.lastChild?.nodeType === Node.TEXT_NODE ? node.lastChild : node.appendChild(document.createTextNode(""));
  if (text.data !== value) text.data = value;
}

/* ---- 한 줄의 메타 -----------------------------------------------------------
 *
 * 당신 차례의 행 밑 한 줄(시안): 목표 · 과업 이름과 id · 모델 · 나이. 묶음 사이는 「 · 」,
 * 과업 id 조각만 고정폭이다. 조각은 `{ id }`이거나 글이다. */
function boardMetaPieces(groups, tail = "") {
  const pieces = [];
  const push = (text, mono = "") => {
    const last = pieces.at(-1);
    if (!mono && last && !last.mono) last.text += text;
    else pieces.push({ text, mono });
  };
  groups.filter((group) => group.some(Boolean)).forEach((group, at) => {
    if (at > 0) push(" · ");
    group.filter(Boolean).forEach((part, index) => {
      if (index > 0) push(" ");
      if (typeof part !== "object") push(String(part));
      else if (part.id) push(part.id, "id");
      else pieces.push({ text: part.name, mono: "name" });
    });
  });
  if (tail) push(` — ${tail}`);
  return pieces;
}

/* 조각을 한 줄에 — 조각 수가 그대로면 바뀐 글자만 쓴다. 모델 이름 조각(`{ name }`)은 이름이라
 * 옮기지 않는다(`translate="no"`) — 「Opus 5.5」의 수는 시각이나 개수가 아니다. */
function paintBoardMeta(node, pieces) {
  if (node.firstChild?.nodeType === Node.TEXT_NODE) node.replaceChildren();
  while (node.children.length > pieces.length) node.lastElementChild.remove();
  pieces.forEach((piece, at) => {
    const span = node.children[at] ?? node.appendChild(document.createElement("span"));
    writeClassName(span, piece.mono === "id" ? "board-meta-id" : "");
    if (piece.mono === "name") writeAttribute(span, "translate", "no");
    else if (span.hasAttribute("translate")) span.removeAttribute("translate");
    writeTextContent(span, piece.text);
  });
}

/* 과업이 속한 목표의 이름 — 원장의 과업 행(`parent_title`). */
function boardGoalName(run, id) {
  return (deskLedger?.tasks ?? []).find((task) => task.run === run && task.id === id)?.parent_title ?? "";
}

/* 워커의 모델을 사람 말로: 창의 모델 카탈로그(`agent_models`)의 표시 이름 — 「Opus 5.5」.
 * 그 에이전트의 목록을 아직 읽지 않았으면 한 번 읽고(`agentModelsFor`) 그동안은 원장이 적은
 * 이름 그대로다. 모델을 모르면 에이전트의 이름. */
const boardModelsAsked = new Set();

function boardModelWords(row) {
  if (!row) return "";
  const id = (row.term != null ? paneModels.get(row.term) : null) ?? row.model ?? "";
  if (!id) return row.agent ? agentName(row.agent) : "";
  const rows = agentModelLists.get(row.agent)?.rows ?? null;
  if (!rows && row.agent && !boardModelsAsked.has(row.agent)) {
    boardModelsAsked.add(row.agent);
    void agentModelsFor(row.agent).then(scheduleDeskPaint);
  }
  return catalogModelWords(id, rows);
}

/* 한 줄의 모델 조각 — 이름이다(`boardMetaPieces`의 `{ name }`). */
function boardModelName(row) {
  const words = boardModelWords(row);
  return words ? { name: words } : "";
}

/* 멈춘 행의 한 줄 — 무슨 일이고 무엇을 할지. 워커의 건강(원장의 사실)이 먼저이고,
 * 그다음이 과업의 단계다. */
const BOARD_HALTS = Object.freeze({
  gone: { key: "board.now.haltGone", word: "워커 화면이 사라졌습니다 · 코디가 다시 띄웁니다" },
  walled: { key: "board.now.haltWalled", word: "사용 한도에 닿았습니다 · 한도가 풀리면 이어 갑니다" },
  gate: { key: "board.now.haltGate", word: "결정을 기다립니다 · 당신 차례에 있습니다" },
  blocked: { key: "board.now.haltBlocked", word: "선행 과업 {{tasks}}이(가) 실패해 멈췄습니다" },
  held: { key: "board.now.haltHeld", word: "코디가 세워 두었습니다" },
  failed: { key: "board.now.haltFailed", word: "실패했습니다 · 원인을 보고 다시 띄울지 정하세요" },
});

/* 도는 행의 「하는 일」: 그 워커의 판이 말한 지금의 일(작업 목록과 같은 말,
 * `taskBoardActivity`), 판이 없으면 단계의 낱말. */
function boardRunWork(task, state, carrier, health, entry) {
  if (state === "halt") {
    const halt = health && BOARD_HALTS[health.id] ? BOARD_HALTS[health.id]
      : task.stage === "blocked" && task.blocked_by?.length ? BOARD_HALTS.blocked
        : BOARD_HALTS[task.stage] ?? BOARD_HALTS.held;
    return t(halt.key, halt.word, { tasks: (task.blocked_by ?? []).join(", ") });
  }
  if (state === "run" && entry) return taskBoardActivity(entry);
  if (state === "run") return t("board.now.workRun", "워커가 일하는 중");
  if (state === "queue") {
    return task.stage === "pending"
      ? t("board.now.workPending", "앞선 과업이 끝나기를 기다립니다")
      : t("board.now.workReady", "워커를 기다립니다");
  }
  return "";
}

/* 행의 끝 칸(시안의 끝 시각 칸): 끝날 시각이 있으면 그 시각, 도는 일은 경과, 기다리는 일은
 * 「대기」, 멈춘 일은 「멈춤」. 「원문 보기」는 하는 일 칸의 한 줄 곁에 선다
 * (`paintBoardRunRow`). */
function boardRunEnd(task, state, carrier, now) {
  const eta = deskTaskEta(task, now);
  if (eta !== null) return t("board.now.endAt", "{{time}}쯤", { time: boardClock(eta, now) });
  if (state === "run") {
    const from = Number(carrier?.dispatch_started_ms) || 0;
    return (from > 0 ? runningWord(from, now) : null) ?? t("board.desk.releaseJustStarted", "방금 시작");
  }
  if (state === "queue") return t("board.now.stepAhead", "대기");
  if (state === "halt") return t("board.now.stateHalt", "멈춤");
  return "";
}

/* 멈춘 행의 원문 — 원장이 적은 사실 그대로, 펼친 사람에게만: 단계, 결정의 문장,
 * 실패한 선행, 워커와 그 판의 사실. 우리말 한 줄(`boardRunWork`)이 행에 서고, 이것은
 * 「원문 보기」 안에 있다. */
function boardRunRaw(row) {
  const { task, carrier } = row;
  if (row.state !== "halt") return "";
  return [
    `stage=${task.stage}`,
    task.gate ? `${task.gate.id}: ${task.gate.question}` : "",
    task.blocked_by?.length ? `blocked_by=${task.blocked_by.join(",")}` : "",
    carrier ? `worker=${carrier.worker} ledger=${carrier.ledger}` : "",
    Number.isFinite(carrier?.pane_missing_since_ms) ? `pane_missing_since_ms=${carrier.pane_missing_since_ms}` : "",
    carrier?.wall ? `wall_resets_at_ms=${carrier.wall.resets_at_ms ?? "?"}` : "",
  ].filter(Boolean).join("\n");
}

/* ---- 당신 차례 --------------------------------------------------------------
 *
 * 사람이 답해야 움직이는 것만: 워커의 질문(그 자리에서 답하기 — 답할 우편의 편지
 * 그대로, `paintDeskLetter`), 원장이 세운 결정(거두기·다시 띄우기·열기), 그리고
 * 보고가 와서 검증을 기다리는 과업(보고 읽기·통과). 셋 다 원장이 이미 고른 목록에서
 * 오고, 이 화면은 고르지도 세지도 않는다. */
const BOARD_ASK_ORDER = Object.freeze(["question", "hold", "review"]);

function boardAsks(ledger) {
  const asks = [];
  for (const letter of Array.isArray(ledger?.mail) ? ledger.mail : []) {
    asks.push({ kind: "question", key: `question:${deskLetterKey(letter)}`, letter, at: Number(letter.created_ms) || 0 });
  }
  for (const task of Array.isArray(ledger?.tasks) ? ledger.tasks : []) {
    // 아무도 돌아오지 않는 일은 누구의 차례도 아니다 — 남은 일의 무더기에 선다(`paintDeskLeft`).
    if (task.left) continue;
    if (task.stage === "gate" && task.gate) {
      asks.push({ kind: "hold", key: `hold:${task.run}/${task.gate.id}`, task, at: Number(task.created_ms) || 0 });
    } else if (task.stage === "reported") {
      asks.push({ kind: "review", key: `review:${task.run}/${task.id}`, task, at: Number(task.created_ms) || 0 });
    }
  }
  // 질문이 먼저 — 묻는 워커는 답을 받을 때까지 멈춰 있다. 그다음이 결정, 검토이고, 같은
  // 종류 안에서는 오래된 것이 먼저다.
  return asks.sort((a, b) => BOARD_ASK_ORDER.indexOf(a.kind) - BOARD_ASK_ORDER.indexOf(b.kind) || a.at - b.at);
}

/* 이 창이 그 런의 코디네이터 자리를 들고 있는가 — 답·결정·통과는 그 자리로만 간다. */
function boardSeatOf(run) {
  if (isPopout) return false;
  return (deskLedger?.runs ?? []).some((one) => one.run === run && one.seat === true);
}

function boardAskShell(kind) {
  const row = deskElement("li", `board-ask is-${kind}`);
  const line = deskElement("p", "board-ask-line");
  line.append(deskElement("strong", "board-ask-kind"), deskElement("span", "board-ask-title"));
  const acts = deskElement("div", "board-ask-acts");
  row.append(line, deskElement("p", "board-ask-meta"), acts, deskElement("p", "board-ask-note"));
  return row;
}

function boardAskButton(acts, id, primary) {
  let button = acts.querySelector(`:scope > [data-act="${id}"]`);
  if (!button) {
    button = deskElement("button", "");
    button.type = "button";
    button.dataset.act = id;
    acts.append(button);
  }
  writeClassName(button, `board-btn${primary ? " is-primary" : ""}`);
  return button;
}

/* 원문 보기 — 원장의 문장을 그대로, 펼친 사람에게만. */
function boardRawLine(row, key, raw) {
  let details = row.querySelector(":scope > .board-raw");
  if (!raw) {
    details?.remove();
    return;
  }
  if (!details) {
    details = deskElement("details", "board-raw");
    details.append(deskElement("summary", "board-raw-head"), deskElement("p", "board-raw-text"));
    details.ontoggle = () => {
      if (details.open) boardNowChoice.raw.add(details.__key);
      else boardNowChoice.raw.delete(details.__key);
    };
    row.insertBefore(details, row.querySelector(":scope > .board-ask-note"));
  }
  details.__key = key;
  const open = boardNowChoice.raw.has(key);
  if (details.open !== open) details.open = open;
  writeTextContent(details.firstElementChild, t("board.now.raw", "원문 보기"));
  writeTextContent(details.lastElementChild, raw);
}

function boardPressNote(row, key) {
  const press = boardPresses.get(key);
  const note = row.querySelector(":scope > .board-ask-note");
  const said = press?.error || press?.said || "";
  writeTextContent(note, said);
  writeAttribute(note, "data-tone", press?.error ? "halt" : "done");
  writeHidden(note, said === "");
  return press;
}

/* 누른 결정과 통과 — 사람이 누를 때만, 원장이 돌려준 답을 행에 바로. 행은 원장이
 * 다음 박자에 목록에서 뺄 때까지 그대로다: 화면은 답을 지어내지 않는다. 원장이 받은 뒤에는
 * 그 행의 단추가 닫힌다 — 행이 빠지기 전의 두 번째 누름은 새 이름의 두 번째 요청이었다
 * (검토 t-68962 P3 ⑥). */
async function pressBoardAsk(view, key, command, args, said) {
  const press = boardPresses.get(key) ?? { sending: false, done: false, said: "", error: "", retry: null };
  if (press.sending || press.done) return;
  const signature = JSON.stringify([command, args]);
  if (press.retry?.signature !== signature) {
    press.retry = { signature, id: `ui-board-${command}-${crypto.randomUUID()}` };
  }
  press.sending = true;
  press.error = "";
  boardPresses.set(key, press);
  paintCoordinatorDesk(view);
  try {
    const answer = await invoke(command, { ...args, retryRequest: press.retry.id });
    press.said = said(answer ?? {});
    press.retry = null;
    press.done = true;
    refreshDeskLedger();
  } catch (error) {
    press.error = String(error);
  } finally {
    press.sending = false;
    paintCoordinatorDesk(view);
  }
}

/* 원장의 답에서 영수증 이름 하나 — 무엇이든 원장이 준 것 그대로다. */
function boardReceipt(answer) {
  return String(answer.receipt ?? answer.messageId ?? answer.gateId ?? answer.taskId ?? answer.id ?? "");
}

/* 누른 행의 단추가 닫혀 있는가: 보내는 중이거나, 원장이 받아 행이 빠지기를 기다리는 중. */
function boardPressShut(press) {
  return Boolean(press?.sending || press?.done);
}

function paintBoardHold(row, ask, now, view) {
  const { task } = ask;
  const gate = task.gate;
  writeTextContent(row.querySelector(".board-ask-kind"), t("board.now.kindHold", "결정"));
  writeTextContent(row.querySelector(".board-ask-title"), deskHoldWords(gate));
  paintBoardMeta(row.querySelector(".board-ask-meta"), boardMetaPieces([
    [task.parent_title], [task.title, { id: task.id }], [gate.checkout ? basename(gate.checkout) : ""],
    [deskNextStepWords(task)],
  ]));
  const acts = row.querySelector(".board-ask-acts");
  const press = boardPressNote(row, ask.key);
  const seated = boardSeatOf(task.run);
  const held = gate.kind !== "asked";
  const sent = (answer) => t("board.now.holdSent", "{{gate}}에 답함 · 원장에 적히면 줄에서 빠져요 {{receipt}}",
    { gate: gate.id, receipt: boardReceipt(answer) }).trim();
  const resolve = (act, answer = null) => void pressBoardAsk(view, ask.key, "desk_hold_resolve",
    { run: task.run, gate: gate.id, act, answer }, sent);
  // A hold the ledger opened: open the folder, or one of the ledger's two answers.
  for (const act of DESK_HOLD_ACTS) {
    const button = boardAskButton(acts, act.id, act.id === "open");
    writeTextContent(button, t(act.key, act.word));
    const standing = act.id === "open" ? Boolean(gate.checkout) : seated;
    writeHidden(button, !held || !standing);
    writeDisabled(button, act.id !== "open" && boardPressShut(press));
    button.onclick = act.id === "open"
      ? () => openWorkbenchWorkspace(gate.checkout, basename(gate.checkout))
      : () => resolve(act.id);
  }
  // A decision somebody opened: the answers its asker offered, then the person's own words.
  const options = held ? [] : gate.options ?? [];
  for (const stale of acts.querySelectorAll(":scope > [data-act^=\"option-\"]")) {
    if (Number(stale.dataset.act.slice("option-".length)) >= options.length) stale.remove();
  }
  options.forEach((option, at) => {
    const button = boardAskButton(acts, `option-${at}`, at === 0);
    writeTextContent(button, option);
    writeHidden(button, !seated);
    writeDisabled(button, boardPressShut(press));
    button.onclick = () => resolve("answer", option);
  });
  const own = boardAskButton(acts, "own", !held && options.length === 0);
  const writing = boardNowChoice.answers.get(ask.key);
  writeTextContent(own, t("board.now.holdOwn", "직접 답하기"));
  writeHidden(own, held || !seated);
  writeDisabled(own, boardPressShut(press));
  writeAttribute(own, "aria-expanded", String(Boolean(writing?.open)));
  own.onclick = () => {
    const draft = boardNowChoice.answers.get(ask.key) ?? { open: false, text: "" };
    draft.open = !draft.open;
    boardNowChoice.answers.set(ask.key, draft);
    paintCoordinatorDesk(view);
    if (draft.open) row.querySelector(".board-desk-reply-field")?.focus();
  };
  boardHoldForm(row, ask.key, Boolean(writing?.open) && !held && seated, press, (text) => resolve("answer", text));
  // 원장의 영어 문장은 행에 서지 않는다 — 펼친 사람에게만.
  boardRawLine(row, ask.key, held ? gate.question : "");
}

/* 결정에 제 말로 답하는 칸 — 답할 우편의 답 칸과 같은 모양, 처음 펼 때 한 번 짓는다. */
function boardHoldForm(row, key, open, press, send) {
  let form = row.querySelector(":scope > .board-desk-reply");
  if (!open) {
    if (form) writeHidden(form, true);
    return;
  }
  if (!form) {
    form = deskElement("form", "board-desk-reply");
    const field = deskElement("textarea", "board-desk-reply-field");
    field.rows = 2;
    field.oninput = () => {
      const draft = boardNowChoice.answers.get(form.__key);
      if (draft) draft.text = field.value;
    };
    const cancel = deskElement("button", "board-desk-reply-cancel");
    cancel.type = "button";
    cancel.onclick = () => {
      const draft = boardNowChoice.answers.get(form.__key);
      if (draft) draft.open = false;
      writeHidden(form, true);
    };
    const submit = deskElement("button", "board-desk-reply-send");
    submit.type = "submit";
    form.onsubmit = (event) => {
      event.preventDefault();
      const text = field.value.trim();
      if (text !== "") form.__send(text);
    };
    const actions = deskElement("div", "board-desk-reply-actions");
    actions.append(deskElement("span", "board-desk-reply-error"), cancel, submit);
    form.append(field, actions);
    row.insertBefore(form, row.querySelector(":scope > .board-ask-note"));
  }
  form.__key = key;
  form.__send = send;
  writeHidden(form, false);
  const field = form.querySelector(".board-desk-reply-field");
  const draft = boardNowChoice.answers.get(key);
  if (draft && field.value !== draft.text && document.activeElement !== field) field.value = draft.text;
  writeAttribute(field, "aria-label", t("board.now.holdOwn", "직접 답하기"));
  writeAttribute(field, "placeholder", t("board.now.holdOwnPlaceholder", "코디네이터로서 이 결정에 답을 적어요 — 원장의 gate-resolve로 갑니다"));
  writeDisabled(field, boardPressShut(press));
  const submit = form.querySelector(".board-desk-reply-send");
  writeDisabled(submit, boardPressShut(press));
  writeTextContent(submit, press?.sending ? t("board.desk.replySending", "보내는 중…") : t("board.desk.replySend", "답 보내기"));
  writeTextContent(form.querySelector(".board-desk-reply-cancel"), t("board.desk.replyCancel", "취소"));
}

/* 코디네이터가 이 시도·출처의 검증을 거둔 넘김의 까닭(t-81913) — 원장이 읽은 말 그대로, 까닭 없이 거뒀으면
 * 빈 글, 거두지 않았으면 null. 원장은 그 위의 맨 통과를 거절하므로 화면도 통과를 내밀지 않고, 새 검수 결과나
 * 새 넘김만이 다시 통과시킨다. */
function boardReviewWithdrawn(task) {
  const why = task?.progress?.withdrawn;
  return typeof why === "string" ? why : null;
}

/* 검토 행의 제목(시안): 「… 보고가 왔습니다」, 넘김의 통과 조건이 모두 확인됐으면 그 말을 덧붙인다.
 * 검증을 거둔 넘김은 거둔 사실과 까닭이 제목이다. */
function boardReviewWords(task) {
  const withdrawn = boardReviewWithdrawn(task);
  if (withdrawn !== null) {
    return t("board.now.reviewWithdrawn", "{{task}} 검증이 철회됐습니다 — {{why}}", {
      task: task.title, why: withdrawn || t("board.now.reviewWithdrawnUnsaid", "까닭은 적지 않았습니다"),
    });
  }
  const conditions = Array.isArray(task.hand_in?.conditions) ? task.hand_in.conditions : [];
  const met = conditions.length > 0 && conditions.every((one) => one.state === "checked");
  const came = t("board.now.reviewTitle", "{{task}} 보고가 왔습니다", { task: task.title });
  return met ? t("board.now.reviewMet", "{{came}} — 통과 조건을 모두 채웠습니다", { came }) : came;
}

function paintBoardReview(row, ask, now, view) {
  const { task } = ask;
  const carrier = deskTaskCarrier(task);
  writeTextContent(row.querySelector(".board-ask-kind"), t("board.now.kindReview", "검토"));
  writeTextContent(row.querySelector(".board-ask-title"), boardReviewWords(task));
  // What it handed in (the commit, the conditions) is the report's, behind 보고 읽기. The line
  // under it is the draft's: the goal, the task and its id, the model, how long ago it handed in.
  const handed = Number(task.times?.handed_ms) || 0;
  // 거둔 검증에는 통과가 없다: 보고 읽기만 남고, 다시 통과하는 길(새 검수 결과나 새 넘김)은 줄 끝 말이 말한다.
  const withdrawn = boardReviewWithdrawn(task) !== null;
  paintBoardMeta(row.querySelector(".board-ask-meta"), boardMetaPieces([
    [task.parent_title], [task.title, { id: task.id }], [boardModelName(carrier)],
    [handed > 0 ? t("board.desk.mailAge", "{{time}} 전", { time: agoWord(handed, now) }) : ""],
    [deskNextStepWords(task)],
  ], withdrawn ? t("board.now.reviewAgain", "새 검수 결과를 적거나 새로 넘겨야 다시 통과됩니다")
    : t("board.now.reviewNext", "통과시키면 다음 발행에 같이 실립니다")));
  const acts = row.querySelector(".board-ask-acts");
  const press = boardPressNote(row, ask.key);
  const open = boardNowChoice.reports.has(ask.key);
  const read = boardAskButton(acts, "read", withdrawn);
  writeTextContent(read, open ? t("board.now.reportFold", "보고 접기") : t("board.now.reportRead", "보고 읽기"));
  writeAttribute(read, "aria-expanded", String(open));
  read.onclick = () => {
    if (boardNowChoice.reports.has(ask.key)) boardNowChoice.reports.delete(ask.key);
    else boardNowChoice.reports.add(ask.key);
    paintCoordinatorDesk(view);
  };
  const pass = boardAskButton(acts, "pass", true);
  const attempt = task.progress?.attempt ?? "";
  const source = task.progress?.source ?? "";
  writeTextContent(pass, t("board.now.reviewPass", "통과"));
  writeHidden(pass, withdrawn || !boardSeatOf(task.run) || attempt === "" || source === "");
  writeDisabled(pass, boardPressShut(press));
  pass.onclick = withdrawn ? null : () => void pressBoardAsk(view, ask.key, "desk_review_pass",
    { run: task.run, task: task.id, attempt, source },
    (answer) => t("board.now.reviewSent", "통과를 적음 · 원장에 적히면 줄에서 빠져요 {{receipt}}",
      { receipt: boardReceipt(answer) }).trim());
  // 보고의 몸 — 통과 조건과 넘김 메모, 남긴 보고서의 문(과업 흐름의 행과 같은 손). 펼친
  // 사람에게만 있다: 접힌 행은 그 몸을 들고 다니지 않는다.
  let body = row.querySelector(":scope > .board-ask-report");
  if (!open) {
    body?.remove();
    return;
  }
  if (!body) {
    body = deskElement("div", "board-ask-report");
    body.append(deskElement("p", "board-desk-task-work"), deskElement("p", "board-desk-task-handin"),
      deskElement("ol", "board-desk-task-conditions"), deskKeptLine("board-desk-task-kept", true));
    row.insertBefore(body, row.querySelector(":scope > .board-ask-note"));
  }
  const [workLine, handInLine, conditions, kept] = body.children;
  const work = deskTaskWorkWords(task.progress);
  writeTextContent(workLine, work);
  writeHidden(workLine, work === "");
  const handIn = deskHandInWords(task.hand_in);
  writeTextContent(handInLine, handIn || t("board.now.reportEmpty", "넘김 글에 남긴 메모가 없습니다"));
  paintDeskConditions(conditions, task.hand_in?.conditions ?? []);
  paintDeskKept(kept, task.kept, task);
}

/* 질문 행은 답할 우편의 편지 그대로이고, 첫 줄 대신 시안의 한 줄이 선다: 목표 · 과업과 id ·
 * 묻는 워커의 모델 · 물은 지 얼마. */
function paintBoardQuestion(row, ask, now, view) {
  const { letter } = ask;
  const seat = boardSeatOf(letter.run);
  const asker = deskAgents.find((one) => one.run === letter.run && one.worker === letter.worker) ?? null;
  paintDeskLetter(row, letter, seat, now, view, boardMetaPieces([
    [boardGoalName(letter.run, letter.task_id)], [letter.task, letter.task_id ? { id: letter.task_id } : ""],
    [asker ? boardModelName(asker) : letter.worker],
    [t("board.desk.mailAge", "{{time}} 전", { time: agoWord(letter.created_ms, now) })],
  ]));
}

function paintDeskAsks(block, now, view) {
  const ledger = deskLedger;
  if (!ledger) return false;
  const head = block.firstElementChild;
  if (!head.firstElementChild) {
    head.append(deskElement("span", "board-desk-head-word"), deskElement("span", "board-desk-head-note"));
  }
  writeTextContent(head.firstElementChild, t("board.now.asks", "당신 차례"));
  writeTextContent(head.lastElementChild, t("board.now.asksHint", "답하지 않으면 그 워커가 멈춰 있습니다"));
  const body = block.lastElementChild;
  let list = body.querySelector(":scope > .board-asks");
  if (!list) {
    list = deskElement("ol", "board-asks");
    const more = deskElement("button", "board-asks-more board-link");
    more.type = "button";
    more.onclick = () => {
      boardNowChoice.asksAll = !boardNowChoice.asksAll;
      paintCoordinatorDesk(view);
    };
    // The draft's foot: a grey check-circle — "nothing more", not the colour of finished.
    const foot = deskElement("p", "board-asks-foot");
    foot.insertAdjacentHTML("afterbegin", icon("circle-check"));
    foot.append(deskElement("span", ""));
    body.replaceChildren(deskElement("p", "board-desk-unseated"), list, more, foot);
  }
  const [unseated, , more, foot] = body.children;
  const asks = boardAsks(ledger);
  const owed = deskOwed(ledger);
  const shown = boardNowChoice.asksAll ? asks : asks.slice(0, BOARD_ASKS_SHOWN);
  const held = new Map([...list.children].map((node) => [node.dataset.ask, node]));
  reconcileElementOrder(list, shown.map((ask) => {
    let row = held.get(ask.key);
    if (!row) {
      row = ask.kind === "question" ? deskLetterRow(view) : boardAskShell(ask.kind);
      row.dataset.ask = ask.key;
    }
    if (ask.kind === "question") paintBoardQuestion(row, ask, now, view);
    else if (ask.kind === "hold") paintBoardHold(row, ask, now, view);
    else paintBoardReview(row, ask, now, view);
    return row;
  }));
  writeHidden(list, shown.length === 0);
  const rest = asks.length - shown.length;
  writeHidden(more, asks.length <= BOARD_ASKS_SHOWN);
  writeTextContent(more, boardNowChoice.asksAll ? t("board.desk.mailFewer", "접기")
    : t("board.now.asksMore", "{{count}}개 더 보기", { count: rest }));
  writeAttribute(more, "aria-expanded", String(boardNowChoice.asksAll));
  const strangers = shown.some((ask) => !boardSeatOf((ask.letter ?? ask.task).run));
  writeTextContent(unseated, strangers
    ? t("board.desk.noSeat", "이 창에 그 런의 코디네이터 자리가 없어 여기서는 답할 수 없어요") : "");
  writeHidden(unseated, !strangers || isPopout);
  writeTextContent(foot.lastElementChild, owed === 0
    ? t("board.now.asksNone", "지금 당신을 기다리는 결정은 없습니다.")
    : t("board.now.asksRest", "이 밖에 당신을 기다리는 결정은 없습니다."));
  // 원장의 목록을 떠난 행은 누른 기록과 펼친 것을 데리고 간다.
  const standing = new Set(asks.map((ask) => ask.key));
  for (const key of boardPresses.keys()) if (!standing.has(key) && !boardPresses.get(key).sending) boardPresses.delete(key);
  for (const key of boardNowChoice.reports) if (!standing.has(key)) boardNowChoice.reports.delete(key);
  for (const key of boardNowChoice.answers.keys()) if (!standing.has(key)) boardNowChoice.answers.delete(key);
  const letters = new Set(asks.filter((ask) => ask.letter).map((ask) => ask.letter.id));
  for (const id of deskDrafts.keys()) if (!letters.has(id)) deskDrafts.delete(id);
  return true;
}

/* ---- 남은 일: 정리 필요 · 검토 없이 끝남 (t-71220) ------------------------------
 *
 * 사람 10-22: 흐름의 「지금」에 몇 주 된 과업이 「차례 기다림」·「검토 대기」로 남아 끝나지
 * 않았다. 어느 과업이 아무도 돌아오지 않는 일인지는 원장이 한 번 고른다(core
 * `settle::left_behind`) — 데스크는 그 판정을 행마다(`task.left`)와 무더기마다(`left`)로, 흐름은
 * 무더기 둘(`left`·`unreviewed`)로 싣는다. 이 화면은 고르지도 세지도 않는다: 무더기의 이름과 수,
 * 확인 창의 말, 누름과 그 답을 그린다. 누름은 창의 사람 문(`board_settle`) 하나로 가고, 줄은
 * 원장이 적은 뒤에만 빠진다. 낱말과 누름은 여기 한 곳이다 — 상황판과 흐름이 같이 쓴다. */

/* 무더기 둘, 그리는 차례대로. `press`는 원장의 누름 낱말(`SettlePress`), `left`는 그 무더기에
 * 드는 판정(`Left`)이고 `names`는 누름이 이름을 대는 판정이다 — 정리 필요에는 닫힐 일과 열린 채
 * 남을 일이 같이 서고, 누름은 닫힐 일만 이름 댄다. `flow`는 흐름의 답에서 그 무더기의 열쇠다. */
const SETTLE_PILES = Object.freeze([
  {
    kind: "settle", press: "settle", left: ["settle", "look"], names: "settle", flow: "left", key: "board.settle.pileSettle", word: "정리 필요",
    note: { key: "board.settle.pileSettleNote", word: "코디 자리가 비었거나 {{days}}일 넘게 손대지 않은 일 · 「지금」에 세지 않음" },
  },
  {
    kind: "unreviewed", press: "waive", left: ["unreviewed"], names: "unreviewed", flow: "unreviewed", key: "board.settle.pileUnreviewed", word: "검토 없이 끝남",
    note: { key: "board.settle.pileUnreviewedNote", word: "넘겼지만 검토할 코디가 없거나 {{days}}일 넘게 손대지 않은 일" },
  },
]);
/* 줄 하나의 상태 말 — 원장의 판정(`Left`)마다. 열린 채 남는 줄은 사람이 볼 일이다. */
const SETTLE_LEFT_WORDS = Object.freeze({
  settle: { key: "board.settle.pileSettle", word: "정리 필요" },
  look: { key: "board.settle.look", word: "검토 필요" },
  unreviewed: { key: "board.settle.pileUnreviewed", word: "검토 없이 끝남" },
});
/* 확인 창에 이름으로 서는 줄 수 — 그 밖은 수로만 선다. */
const SETTLE_ASK_ROWS = 3;
/* 정리한 뒤 「되돌리기」를 내미는 시간, 그리고 답의 한 줄이 서 있는 시간. */
const SETTLE_UNDO_MS = 60_000;

/* 누른 것 — 표면과 무더기마다(`board:settle`, `flow:unreviewed`…): 보내는 중인가, 원장의 답, 틀린
 * 말, 답이 서 있는 끝. 판이 다시 그려져도 남는다. */
const settlePresses = new Map();

/* 줄 하나의 말 — 과업 · 런 · 손댄 지 얼마. 확인 창과 펼친 무더기가 같이 읽는다. `row`는
 * `{ task, run, words, touched }`다. */
function settleRowWords(row, now) {
  return t("board.settle.askRow", "{{task}} · {{run}} · {{age}} — {{title}}", {
    task: row.task, run: row.run, title: row.words,
    age: t("board.desk.mailAge", "{{time}} 전", { time: agoWord(row.touched, now) }),
  });
}

/* 무더기의 메모 — 무엇이 이 무더기에 드는지, 원장의 조용한 날수(`quiet_days`) 그대로. */
function settlePileNote(pile, quietDays) {
  return t(pile.note.key, pile.note.word, { days: quietDays });
}

/* 누름 단추의 말 — 몇 건에 무엇을 하는지. 한 번에 원장이 받는 수(`press_max`)를 넘으면 그만큼. */
function settlePressWords(pile, count, pressMax) {
  const taken = Math.min(count, pressMax);
  return pile.press === "waive"
    ? t("board.settle.pressWaive", "{{count}}건 검토 생략", { count: taken })
    : t("board.settle.pressSettle", "{{count}}건 닫기", { count: taken });
}

/* 확인 창 — 몇 건을, 처음 몇 줄의 과업 · 런 · 나이와 함께. 원장이 한 번에 받는 수를 넘으면
 * 오래된 것부터 그만큼이라고 말한다. */
function settlePileAsk(pile, rows, count, pressMax, now) {
  const taken = Math.min(count, pressMax);
  const shown = rows.slice(0, Math.min(SETTLE_ASK_ROWS, taken)).map((row) => settleRowWords(row, now));
  if (taken > shown.length) {
    shown.push(t("board.settle.askMoreRows", "… 그 밖에 {{count}}건", { count: taken - shown.length }));
  }
  const waive = pile.press === "waive";
  const title = count > taken
    ? waive
      ? t("board.settle.askWaiveMax", "{{count}}건 가운데 오래된 {{max}}건의 검토를 생략할까요?", { count, max: taken })
      : t("board.settle.askSettleMax", "{{count}}건 가운데 오래된 {{max}}건을 닫을까요?", { count, max: taken })
    : waive
      ? t("board.settle.askWaiveTitle", "{{count}}건의 검토를 생략할까요?", { count })
      : t("board.settle.askSettleTitle", "{{count}}건을 닫을까요?", { count });
  return {
    title,
    body: shown.join("\n"),
    detail: waive
      ? t("board.settle.askWaiveDetail", "검토 기록 대신 「검토 생략(사람)」을 원장에 남깁니다 — 검증이 아닙니다. 과업은 끝난 채 남고, 새로 넘기면 다시 검토 대기가 됩니다.")
      : t("board.settle.askSettleDetail", "원장이 과업마다 다시 읽어 아무것도 넘기지 않은 열린 과업만 「낡음」으로 닫습니다. 넘긴 것이 있거나 다른 과업이 기다리는 과업은 열린 채 남습니다. 닫은 뒤 {{seconds}}초 동안 되돌릴 수 있습니다.",
        { seconds: Math.round(SETTLE_UNDO_MS / 1000) }),
    confirm: settlePressWords(pile, count, pressMax),
    deny: t("board.batch.askKeep", "그대로 두기"),
    cancel: false,
  };
}

/* 누른다 — 사람이 확인 창에서 「예」라고 해야만 창의 사람 문을 부른다. 원장은 이름 하나하나를
 * 다시 읽어 그 판정이 그대로일 때만 쓰고, 나머지는 그대로 둔 까닭과 함께 돌려준다. 같은 무더기는
 * 보내는 동안 다시 눌리지 않는다. `names`는 원장이 고른 그 무더기의 과업 전부, 오래된 것부터다. */
async function pressSettlePile(key, pile, rows, names, pressMax, repaint) {
  if (settlePresses.get(key)?.sending || names.length === 0) return;
  const said = await askConfirm(settlePileAsk(pile, rows, names.length, pressMax, Date.now()));
  if (said !== true) return;
  await sendSettle(key, pile.press, names.slice(0, pressMax), repaint);
}

/* 정리가 닫았다고 답한 과업들을 그 답의 상태 그대로 되돌린다 — 정리한 뒤 잠시만. */
function undoSettle(key, repaint) {
  const held = settlePresses.get(key);
  const closed = held?.answer?.closed ?? [];
  if (!held || held.sending || held.press !== "settle" || closed.length === 0 || Date.now() >= held.until) return;
  void sendSettle(key, "undo", closed, repaint);
}

async function sendSettle(key, press, tasks, repaint) {
  const held = { sending: true, press, answer: null, error: "", until: 0 };
  settlePresses.set(key, held);
  repaint();
  try {
    held.answer = await invoke("board_settle", { press, tasks });
  } catch (error) {
    held.error = String(error);
  } finally {
    held.sending = false;
    held.until = Date.now() + SETTLE_UNDO_MS;
    // 답의 한 줄과 되돌리기는 그 시간이 지나면 접힌다 — 그때 한 번 다시 그린다.
    setTimeout(repaint, SETTLE_UNDO_MS);
    repaint();
  }
}

/* 무더기 곁의 한 줄 — 원장이 돌려준 답 그대로, 그리고 되돌릴 수 있는가. */
function settleSaid(key, now) {
  const held = settlePresses.get(key);
  if (!held) return { said: "", tone: "", undo: false };
  if (held.sending) return { said: t("board.batch.saidSending", "보내는 중…"), tone: "sending", undo: false };
  if (now >= held.until) return { said: "", tone: "", undo: false };
  if (held.error) return { said: t("board.batch.saidRefused", "안 됨 — {{why}}", { why: held.error }), tone: "halt", undo: false };
  const answer = held.answer ?? {};
  const count = (list) => (Array.isArray(list) ? list.length : 0);
  const kept = Array.isArray(answer.kept) ? answer.kept : [];
  const look = kept.filter((one) => one.why === "look").length;
  const said = [
    count(answer.closed) > 0 ? t("board.settle.saidClosed", "{{count}}건을 닫았습니다", { count: count(answer.closed) }) : "",
    count(answer.waived) > 0 ? t("board.settle.saidWaived", "{{count}}건의 검토를 생략했습니다", { count: count(answer.waived) }) : "",
    count(answer.reopened) > 0 ? t("board.settle.saidReopened", "{{count}}건을 되돌렸습니다", { count: count(answer.reopened) }) : "",
    look > 0 ? t("board.settle.saidKeptLook", "{{count}}건은 넘긴 것이 있어 열어 둠", { count: look }) : "",
    kept.length > look ? t("board.settle.saidKeptMoved", "{{count}}건은 그새 움직여 그대로 둠", { count: kept.length - look }) : "",
  ].filter(Boolean).join(" · ");
  return { said, tone: "done", undo: held.press === "settle" && count(answer.closed) > 0 };
}

/* 무더기가 서 있어야 하는가 — 든 일이 있거나, 방금 누른 답(되돌리기)이 아직 서 있다. */
function settlePileStands(key, count, now) {
  const held = settlePresses.get(key);
  return count > 0 || Boolean(held && (held.sending || now < held.until));
}

/* ---- 지금 도는 일 -----------------------------------------------------------
 *
 * 원장의 과업을 목표(`parent`)별로 묶은 표. 행 하나 = 이름과 id · 상태 점과 낱말 · 하는 일
 * 한 줄 · 워커(모델의 표시 이름) · 가지 · 끝 칸 — 시안 B의 배포 목록 그대로다. 끝난 과업은
 * 목표마다 접힌 줄 하나 뒤에 선다. 수는 세지 않는다 — 원장이 보낸 행(`STAGE_ROWS`)이 곧
 * 표다. */
function boardRunRows(ledger, now, view) {
  const model = agentGraphModels.get(view);
  const entries = new Map((model?.agents ?? []).map((entry) => [entry.card.pane, entry]));
  const rows = [];
  for (const task of Array.isArray(ledger?.tasks) ? ledger.tasks : []) {
    const state = BOARD_RUN_OF_STAGE[task.stage];
    if (!state || task.left) continue;
    const carrier = deskTaskCarrier(task);
    const health = carrier && (state === "run" || state === "halt") ? deskWorkerHealth(carrier, null, now) : null;
    const halted = state === "run" && health && (health.id === "gone" || health.id === "walled") ? "halt" : state;
    // The pane's own words for what it is doing, read the way the agent list reads them.
    const seen = carrier?.term != null ? entries.get(`term:${carrier.term}`) : null;
    const entry = seen ? { card: seen.card, facts: agentCardFacts(seen.card, seen.bucket, now) } : null;
    rows.push({ task, state: halted, carrier, health, entry });
  }
  return rows;
}

/* 한 목표 안의 차례 — 시안의 「끝나는 순서」: 도는 일이 먼저(끝날 시각이 있는 것은 그 시각
 * 차례, 없는 것은 먼저 시작한 것부터), 그다음 차례를 기다리는 일, 멈춘 일은 끝에. 멈춘 일은
 * 코디가 다시 띄우는 것이고, 사람이 답할 것은 당신 차례에 있다. */
const BOARD_RUN_ORDER = Object.freeze(["run", "queue", "halt", "done"]);

function boardRunStart(row) {
  return Number(row.carrier?.dispatch_started_ms) || Number(row.task.times?.started_ms) || Number(row.task.created_ms) || 0;
}

function boardGoals(rows, now) {
  const goals = new Map();
  for (const row of rows) {
    const key = `${row.task.run}/${row.task.parent ?? ""}`;
    let goal = goals.get(key);
    if (!goal) {
      goal = { key, name: row.task.parent_title ?? "", parent: row.task.parent ?? null, live: [], done: [], at: Infinity };
      goals.set(key, goal);
    }
    (row.state === "done" ? goal.done : goal.live).push(row);
    goal.at = Math.min(goal.at, Number(row.task.created_ms) || 0);
  }
  const eta = (row) => deskTaskEta(row.task, now) ?? Infinity;
  for (const goal of goals.values()) {
    goal.live.sort((a, b) => BOARD_RUN_ORDER.indexOf(a.state) - BOARD_RUN_ORDER.indexOf(b.state) ||
      eta(a) - eta(b) || boardRunStart(a) - boardRunStart(b));
  }
  // 지금 도는 일이 있는 목표만, 오래된 목표부터. 끝난 일만 남은 목표는 이 표의 것이 아니다 —
  // 기록 탭의 과업 흐름이 센다(오래 돈 원장에서 목표 머리가 수십 줄이 되지 않게).
  return [...goals.values()].filter((goal) => goal.live.length > 0).sort((a, b) => a.at - b.at);
}

/* 한 행의 칸 여섯 — 표가 서른 행이어도 판이 가볍게, 칸 안에 상자를 겹치지 않는다. 하는 일
 * 칸은 한 줄과(멈춘 행이면) 그 곁의 「원문 보기」, 가지 칸은 가지 아이콘과 이름이다. */
function boardRunRow(view) {
  const row = deskElement("div", "board-run");
  row.__view = view;
  row.setAttribute("role", "row");
  const title = deskElement("span", "board-run-title");
  title.append(deskElement("span", "board-run-name"), deskElement("code", "board-run-id"));
  const branch = deskElement("span", "board-run-branch");
  branch.insertAdjacentHTML("afterbegin", icon("branch"));
  const who = deskElement("span", "board-run-who");
  who.setAttribute("translate", "no");
  row.append(title, deskElement("span", "board-run-state"), deskElement("span", "board-run-work"), who, branch,
    deskElement("span", "board-run-end"));
  for (const cell of row.children) cell.setAttribute("role", "cell");
  return row;
}

/* 하는 일 칸: 한 줄의 글. 멈춘 행에만 그 곁에 「원문 보기」가 서고, 그때만 칸이 둘로 나뉜다. */
function paintBoardRunWork(node, row, words) {
  const cell = node.querySelector(".board-run-work");
  if (boardRunRaw(row) === "") {
    writeClassName(cell, "board-run-work");
    if (cell.firstElementChild) cell.replaceChildren();
    writeTextContent(cell, words);
    return;
  }
  writeClassName(cell, "board-run-work has-raw");
  if (!cell.firstElementChild) {
    const raw = deskElement("button", "board-link board-run-raw");
    raw.type = "button";
    raw.onclick = () => {
      const key = node.dataset.key;
      if (boardNowChoice.raw.has(key)) boardNowChoice.raw.delete(key);
      else boardNowChoice.raw.add(key);
      paintCoordinatorDesk(node.__view);
    };
    cell.replaceChildren(deskElement("span", "board-run-work-text"), raw);
  }
  writeTextContent(cell.firstElementChild, words);
  writeTextContent(cell.lastElementChild, t("board.now.raw", "원문 보기"));
  writeAttribute(cell.lastElementChild, "aria-expanded", String(boardNowChoice.raw.has(node.dataset.key)));
}

/* 행의 가지 칸: 워커 체크아웃의 가지(`desk_checkouts`), 아직 모르면 그 폴더의 이름. */
function boardRunBranch(carrier) {
  if (!carrier?.checkout) return "";
  return deskCheckouts.get(carrier.checkout)?.branch || basename(carrier.checkout);
}

function paintBoardRunRow(node, row, now) {
  const { task, state, carrier, health, entry } = row;
  const said = BOARD_RUN_STATES[state];
  writeClassName(node, `board-run is-${state}`);
  writeAttribute(node, "data-task", `${task.run}/${task.id}`);
  writeTextContent(node.querySelector(".board-run-name"), task.title);
  writeTextContent(node.querySelector(".board-run-id"), task.id);
  const done = BOARD_DONE_WORDS[task.stage];
  writeTextContent(node.querySelector(".board-run-state"), state !== "done" ? t(said.key, said.word)
    : done ? t(done.key, done.word) : deskStageWord(task.stage));
  // A stopped row's ledger record stands behind 「원문 보기」 beside its words (the draft's link).
  paintBoardRunWork(node, row, boardRunWork(task, state, carrier, health, entry));
  writeTextContent(node.querySelector(".board-run-who"), carrier ? boardModelWords(carrier) : "");
  const branch = boardRunBranch(carrier);
  const branchCell = node.querySelector(".board-run-branch");
  writeClassName(branchCell, branch === "" ? "board-run-branch is-empty" : "board-run-branch");
  writeTrailingText(branchCell, branch);
  writeTextContent(node.querySelector(".board-run-end"), state === "done" ? "" : boardRunEnd(task, state, carrier, now));
}

/* 끝난 과업의 단계 낱말 — 과업 흐름의 칩과 같은 말(`DESK_STAGES`). */
function deskStageWord(stage) {
  const held = DESK_STAGES.find((one) => one.id === stage);
  return held ? t(held.key, held.word) : String(stage);
}

function boardGoalHead(view) {
  const head = deskElement("div", "board-goal");
  head.setAttribute("role", "row");
  const fold = deskElement("button", "board-goal-fold");
  fold.type = "button";
  fold.innerHTML = icon("chevron", true);
  fold.append(deskElement("span", "board-goal-name"));
  fold.onclick = () => {
    const key = head.dataset.goal;
    if (boardNowChoice.folded.has(key)) boardNowChoice.folded.delete(key);
    else boardNowChoice.folded.add(key);
    paintCoordinatorDesk(view);
  };
  head.append(fold, deskElement("span", "board-goal-note"));
  return head;
}

/* 한 목표가 처음 보이는 행 수. 나머지는 「N개 더 보기」 뒤에 선다 — 한 목표가 스무 과업을
 * 돌려도 다른 목표가 화면 밖으로 밀리지 않게. 도는 행이 먼저라 늘 보인다. */
const BOARD_GOAL_ROWS_SHOWN = 5;

function boardGoalMore(view) {
  const line = deskElement("div", "board-goal-more");
  line.setAttribute("role", "row");
  const more = deskElement("button", "board-link");
  more.type = "button";
  more.setAttribute("role", "cell");
  more.onclick = () => {
    const key = line.dataset.goal;
    if (boardNowChoice.goalsAll.has(key)) boardNowChoice.goalsAll.delete(key);
    else boardNowChoice.goalsAll.add(key);
    paintCoordinatorDesk(view);
  };
  line.append(more);
  return line;
}

function boardDoneHead(view) {
  const head = deskElement("div", "board-goal is-done");
  head.setAttribute("role", "row");
  const fold = deskElement("button", "board-goal-fold");
  fold.type = "button";
  fold.innerHTML = icon("chevron", false);
  fold.append(deskElement("span", "board-goal-name"));
  fold.onclick = () => {
    const key = head.dataset.goal;
    if (boardNowChoice.folded.has(key)) boardNowChoice.folded.delete(key);
    else boardNowChoice.folded.add(key);
    paintCoordinatorDesk(view);
  };
  head.append(fold);
  return head;
}

/* 과업이 끝난 때 — 착지한 과업은 원장이 착지를 적은 때(`times.landed_ms`), 착지할 것 없이 끝난
 * 과업(조사·시안·검토, t-34501)은 코디가 검증을 적은 때(`times.verified_ms`). 그 밖은 모른다(`null`). */
function boardFinishedAt(task) {
  const times = task.times ?? {};
  if (Number.isFinite(times.landed_ms)) return times.landed_ms;
  return task.stage === "nothing_to_land" && Number.isFinite(times.verified_ms) ? times.verified_ms : null;
}

/* 오늘 끝난 일 — 끝난 때가 오늘인 과업, 늦게 끝난 것이 먼저. 이 표의 발이 그 수를 말하고 기록 탭의
 * 「오늘 끝난 일」 표(`paintDeskToday`, t-70264)가 그 행을 그린다 — 한 규칙이라 두 곳의 수가 갈리지
 * 않는다. */
function boardFinishedTodayTasks(ledger, now) {
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  return (ledger.tasks ?? []).filter((task) => (boardFinishedAt(task) ?? -Infinity) >= midnight.getTime())
    .sort((a, b) => boardFinishedAt(b) - boardFinishedAt(a));
}

function boardFinishedToday(ledger, now) {
  return boardFinishedTodayTasks(ledger, now).length;
}

function paintDeskRunning(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.tasks)) return false;
  const rows = boardRunRows(ledger, now, view);
  if (!rows.some((row) => row.state !== "done")) return false;
  const head = block.firstElementChild;
  if (!head.firstElementChild) {
    const legend = deskElement("ul", "board-legend");
    for (const id of Object.keys(BOARD_RUN_STATES)) {
      const item = deskElement("li", `is-${id}`);
      item.dataset.state = id;
      legend.append(item);
    }
    head.append(deskElement("span", "board-desk-head-word"), deskElement("span", "board-desk-head-note"), legend);
  }
  writeTextContent(head.children[0], t("board.now.running", "지금 도는 일"));
  // 「끝나는 순서」는 끝날 시각을 아는 행이 있을 때만의 말이다 — 모르면 차례를 그대로 말한다.
  const timed = rows.some((row) => row.state !== "done" && deskTaskEta(row.task, now) !== null);
  writeTextContent(head.children[1], timed ? t("board.now.runningHint", "목표별 · 끝나는 순서")
    : t("board.now.runningHintStarted", "목표별 · 먼저 시작한 일부터"));
  const legend = head.children[2];
  writeAttribute(legend, "aria-label", t("board.now.legend", "읽는 법"));
  for (const item of legend.children) {
    const state = BOARD_RUN_STATES[item.dataset.state];
    writeTextContent(item, t(state.key, state.word));
  }
  const body = block.lastElementChild;
  let table = body.querySelector(":scope > .board-runs");
  if (!table) {
    table = deskElement("div", "board-runs");
    table.setAttribute("role", "table");
    const foot = deskElement("p", "board-runs-foot");
    const toHistory = deskElement("button", "board-link");
    toHistory.type = "button";
    toHistory.onclick = () => chooseBoardTab(view, "history", false);
    foot.append(deskElement("span", ""), document.createTextNode(" · "), toHistory);
    body.replaceChildren(table, foot);
  }
  writeAttribute(table, "aria-label", t("board.now.running", "지금 도는 일"));
  const goals = boardGoals(rows, now);
  const held = new Map([...table.children].map((node) => [node.dataset.key, node]));
  const wanted = [];
  const take = (key, make) => {
    const node = held.get(key) ?? make();
    writeAttribute(node, "data-key", key);
    wanted.push(node);
    return node;
  };
  for (const goal of goals) {
    const folded = boardNowChoice.folded.has(goal.key);
    const goalHead = take(`goal:${goal.key}`, () => boardGoalHead(view));
    writeAttribute(goalHead, "data-goal", goal.key);
    const fold = goalHead.querySelector(".board-goal-fold");
    writeAttribute(fold, "aria-expanded", String(!folded));
    fold.querySelector(".icon")?.classList.toggle("is-open", !folded);
    writeTextContent(goalHead.querySelector(".board-goal-name"),
      goal.name || t("board.now.noGoal", "목표 없이 적은 일"));
    writeTextContent(goalHead.querySelector(".board-goal-note"), goal.parent ?? "");
    if (folded) continue;
    const all = boardNowChoice.goalsAll.has(goal.key);
    const live = all ? goal.live : goal.live.slice(0, BOARD_GOAL_ROWS_SHOWN);
    for (const row of live) {
      const key = `task:${row.task.run}/${row.task.id}`;
      paintBoardRunRow(take(key, () => boardRunRow(view)), row, now);
      const raw = boardRunRaw(row);
      if (raw === "" || !boardNowChoice.raw.has(key)) continue;
      const line = take(`raw:${key}`, () => {
        const node = deskElement("div", "board-run-rawline");
        node.setAttribute("role", "row");
        node.append(deskElement("pre", "board-raw-text"));
        return node;
      });
      writeTextContent(line.firstElementChild, raw);
    }
    if (goal.live.length > BOARD_GOAL_ROWS_SHOWN) {
      const more = take(`more:${goal.key}`, () => boardGoalMore(view));
      writeAttribute(more, "data-goal", goal.key);
      writeAttribute(more.firstElementChild, "aria-expanded", String(all));
      writeTextContent(more.firstElementChild, all ? t("board.desk.mailFewer", "접기")
        : t("board.now.asksMore", "{{count}}개 더 보기", { count: goal.live.length - live.length }));
    }
    if (goal.done.length === 0) continue;
    const doneKey = `done:${goal.key}`;
    const doneOpen = boardNowChoice.folded.has(doneKey);
    const doneHead = take(doneKey, () => boardDoneHead(view));
    writeAttribute(doneHead, "data-goal", doneKey);
    const doneFold = doneHead.querySelector(".board-goal-fold");
    writeAttribute(doneFold, "aria-expanded", String(doneOpen));
    doneFold.querySelector(".icon")?.classList.toggle("is-open", doneOpen);
    writeTextContent(doneHead.querySelector(".board-goal-name"),
      t("board.now.doneFold", "끝난 과업 {{count}}개", { count: goal.done.length }));
    if (!doneOpen) continue;
    for (const row of goal.done) {
      paintBoardRunRow(take(`task:${row.task.run}/${row.task.id}`, () => boardRunRow(view)), row, now);
    }
  }
  reconcileElementOrder(table, wanted);
  // The draft's foot: how many finished today, and the door to the record of them.
  const foot = body.querySelector(":scope > .board-runs-foot");
  const today = boardFinishedToday(ledger, now);
  writeTextContent(foot.firstElementChild, today > 0
    ? t("board.now.runsToday", "오늘 끝난 일 {{count}}개", { count: today })
    : t("board.now.runsTodayNone", "오늘 끝난 일은 아직 없습니다"));
  writeTextContent(foot.lastElementChild, t("board.now.toHistory", "기록에서 보기"));
  return true;
}

/* ---- 남은 일의 무더기 (상황판) ----------------------------------------------
 *
 * 「지금 도는 일」 아래의 접힌 줄 둘 — 정리 필요 · 검토 없이 끝남. 수와 누를 이름은 데스크가
 * 무더기마다 실은 그대로이고(`left`), 펼친 행은 데스크가 단계마다 따로 실은 남은 일의
 * 행이다(`task.left`). 누름은 이 창의 사람 문이라 코디 자리를 묻지 않는다 — 자리가 빈 런이야말로
 * 이 무더기의 일이다. */
function boardLeftRows(ledger, pile) {
  return (Array.isArray(ledger?.tasks) ? ledger.tasks : [])
    .filter((task) => pile.left.includes(task.left))
    .sort((a, b) => (Number(a.touched_ms) || 0) - (Number(b.touched_ms) || 0));
}

/* 무더기 하나의 수 — 데스크가 판정마다 실은 이름의 수를 더한 것. */
function boardLeftCount(left, pile) {
  return pile.left.reduce((sum, kind) => sum + (Array.isArray(left?.[kind]) ? left[kind].length : 0), 0);
}

function boardLeftHead(view) {
  const head = deskElement("div", "board-goal is-done is-left");
  head.setAttribute("role", "row");
  const fold = deskElement("button", "board-goal-fold");
  fold.type = "button";
  fold.innerHTML = icon("chevron", false);
  fold.append(deskElement("span", "board-goal-name"));
  fold.onclick = () => {
    const key = head.dataset.goal;
    if (boardNowChoice.folded.has(key)) boardNowChoice.folded.delete(key);
    else boardNowChoice.folded.add(key);
    paintCoordinatorDesk(view);
  };
  const undo = deskElement("button", "board-link board-left-undo");
  undo.type = "button";
  const press = deskElement("button", "board-btn board-left-press");
  press.type = "button";
  head.append(fold, deskElement("span", "board-goal-note"), deskElement("span", "board-left-said"), undo, press);
  return head;
}

/* 남은 일의 행 하나 — 「지금 도는 일」의 행과 같은 여섯 칸: 이름과 id · 판정의 말 · 런 · 워커 ·
 * 가지 · 손댄 지 얼마. */
function paintBoardLeftRow(node, task, now, runs) {
  writeClassName(node, `board-run is-left is-${task.left}`);
  writeAttribute(node, "data-task", `${task.run}/${task.id}`);
  writeTextContent(node.querySelector(".board-run-name"), task.title);
  writeTextContent(node.querySelector(".board-run-id"), task.id);
  const said = SETTLE_LEFT_WORDS[task.left] ?? SETTLE_LEFT_WORDS.settle;
  writeTextContent(node.querySelector(".board-run-state"), t(said.key, said.word));
  const run = runs.get(task.run);
  paintBoardRunWork(node, { task, state: "left" }, run?.name ? `${run.name} · ${task.run}` : task.run);
  writeTextContent(node.querySelector(".board-run-who"), "");
  const branchCell = node.querySelector(".board-run-branch");
  writeClassName(branchCell, "board-run-branch is-empty");
  writeTrailingText(branchCell, "");
  writeTextContent(node.querySelector(".board-run-end"),
    t("board.desk.mailAge", "{{time}} 전", { time: agoWord(Number(task.touched_ms) || 0, now) }));
}

function paintDeskLeft(block, now, view) {
  const ledger = deskLedger;
  const left = ledger?.left;
  if (!left) return false;
  const piles = SETTLE_PILES
    .map((pile) => ({ pile, key: `board:${pile.kind}`, count: boardLeftCount(left, pile) }))
    .filter(({ key, count }) => settlePileStands(key, count, now));
  if (piles.length === 0) return false;
  const head = block.firstElementChild;
  writeTextContent(head, t("board.settle.head", "남은 일"));
  const body = block.lastElementChild;
  let table = body.querySelector(":scope > .board-runs");
  if (!table) {
    table = deskElement("div", "board-runs");
    table.setAttribute("role", "table");
    body.replaceChildren(table);
  }
  writeAttribute(table, "aria-label", t("board.settle.head", "남은 일"));
  const runs = new Map((ledger.runs ?? []).map((run) => [run.run, run]));
  const quietDays = Number(left.quiet_days) || 0;
  const pressMax = Number(left.press_max) || 0;
  const repaint = () => paintCoordinatorDesk(view);
  const held = new Map([...table.children].map((node) => [node.dataset.key, node]));
  const wanted = [];
  const take = (key, make) => {
    const node = held.get(key) ?? make();
    writeAttribute(node, "data-key", key);
    wanted.push(node);
    return node;
  };
  for (const { pile, key, count } of piles) {
    const open = boardNowChoice.folded.has(key);
    const pileHead = take(`left:${pile.kind}`, () => boardLeftHead(view));
    writeAttribute(pileHead, "data-goal", key);
    const fold = pileHead.querySelector(".board-goal-fold");
    writeAttribute(fold, "aria-expanded", String(open));
    fold.querySelector(".icon")?.classList.toggle("is-open", open);
    writeTextContent(fold.querySelector(".board-goal-name"), `${t(pile.key, pile.word)} ${count}`);
    writeTextContent(pileHead.querySelector(".board-goal-note"), settlePileNote(pile, quietDays));
    const names = Array.isArray(left[pile.names]) ? left[pile.names] : [];
    const rows = boardLeftRows(ledger, pile);
    const press = pileHead.querySelector(".board-left-press");
    const answer = settleSaid(key, now);
    writeTextContent(press, settlePressWords(pile, names.length, pressMax));
    writeHidden(press, names.length === 0 || isPopout);
    writeDisabled(press, settlePresses.get(key)?.sending === true);
    press.onclick = () => void pressSettlePile(key, pile,
      rows.filter((task) => task.left === pile.names)
        .map((task) => ({ task: task.id, run: task.run, words: task.title, touched: Number(task.touched_ms) || 0 })),
      names, pressMax, repaint);
    const said = pileHead.querySelector(".board-left-said");
    writeTextContent(said, answer.said);
    writeAttribute(said, "data-tone", answer.tone);
    writeHidden(said, answer.said === "");
    const undo = pileHead.querySelector(".board-left-undo");
    writeTextContent(undo, t("board.settle.undo", "되돌리기"));
    writeHidden(undo, !answer.undo || isPopout);
    undo.onclick = () => undoSettle(key, repaint);
    if (!open) continue;
    for (const task of rows) paintBoardLeftRow(take(`leftrow:${task.run}/${task.id}`, () => boardRunRow(view)), task, now, runs);
    const unseen = count - rows.length;
    if (unseen > 0) {
      const more = take(`leftmore:${pile.kind}`, () => {
        const line = deskElement("div", "board-goal-more");
        line.setAttribute("role", "row");
        line.append(deskElement("span", "board-goal-note"));
        return line;
      });
      writeTextContent(more.firstElementChild, t("board.settle.more", "그 밖에 {{count}}건", { count: unseen }));
    }
  }
  reconcileElementOrder(table, wanted);
  return true;
}

/* ---- 맨 위 한 문장 ----------------------------------------------------------
 *
 * 「답할 일 N개 — 나머지는 저절로 돕니다」와, 그 아래 다음에 끝날 일. N은 당신 차례
 * 카드의 발이 말하는 수와 같은 하나(`deskOwed`, 백엔드의 수)다 — 두 곳에서 세면 두
 * 답이 된다. 다음에 끝날 일은 끝날 시각을 아는 것 가운데 가장 이른 것이다: 과업의 시각,
 * 그리고 도는 발행 레인의 예상 끝(일정의 셈, `boardScheduleLaneEnd` — 남은 단계가 다 지난
 * 소요를 가질 때만).
 *
 * 도는 단계가 평소(지난 초록 레인들의 가운데 값)를 넘었으면 레인의 끝은 아무도 모른다(t-83513):
 * 지난 시각이나 지금에 밀려 가는 시각 대신 「평소보다 늦음 · N분째」를 말하고, 레인이 그 단계의
 * 레시피가 오래 아무것도 쓰지 않았다고 적었으면(`stalled`) 그 말을 한다 — 사람이 지켜보는 것은
 * 그 레인이라 과업의 시각보다 앞에 선다. */
function boardNextWords(rows, now) {
  let next = null;
  for (const row of rows) {
    const eta = deskTaskEta(row.task, now);
    if (eta !== null && (!next || eta < next.eta)) next = { eta, title: row.task.title };
  }
  const lane = releaseLaneHere();
  const running = lane && typeof lane === "object" && lane.sha && deskReleaseVerdict(lane) === "running";
  if (running && lane.stalled && typeof lane.stalled === "object") {
    return t("board.now.nextStalled", "새 버전 발행이 멈춘 듯합니다 — {{recipe}} {{time}} 출력 없음",
      deskStalledVars(lane.stalled, now));
  }
  const ahead = running ? deskReleaseAhead(lane) : null;
  if (ahead) {
    const { end, lateMs, known } = boardScheduleLaneEnd(ahead, now);
    const stage = ahead.stages?.[ahead.stages.length - 1];
    // Late only against a usual time the past gave: a step with no green run before has no usual.
    if (lateMs > 0 && Number(ahead.ahead[0]?.expected?.samples) > 0) {
      return t("board.now.nextLate", "새 버전 발행이 평소보다 늦음 · {{phase}} {{time}}", {
        phase: deskReleasePhaseWord(String(stage?.name ?? lane.phase ?? ""), lane),
        time: runningWord(Number(stage?.start_ms), now) ?? t("board.desk.releaseJustStarted", "방금 시작"),
      });
    }
    if (known && end > now && (!next || end < next.eta)) next = { eta: end, title: t("board.now.lineRelease", "새 버전 발행") };
  }
  if (next) {
    return t("board.now.nextAt", "다음으로 끝나는 일은 {{title}}, {{time}}쯤입니다.",
      { title: next.title, time: boardClock(next.eta, now) });
  }
  if (running) {
    return t("board.now.nextLane", "빌드 줄에서 {{version}} 발행이 {{phase}} 단계를 돌고 있습니다.",
      { version: deskReleaseVersionWord(lane),
        phase: deskReleasePhaseWord(String(lane.phase || ""), lane) });
  }
  const live = rows.filter((row) => row.state === "run").length;
  return live > 0
    ? t("board.now.nextRunning", "지금 {{count}}개가 돌고 있습니다.", { count: live })
    : t("board.now.nextIdle", "지금 도는 일이 없습니다.");
}

function paintBoardHead(view, owed, now) {
  const intro = view.querySelector(".task-board-intro");
  if (!intro) return;
  writeTextContent(intro.querySelector(".board-now-title"), owed > 0
    ? t("board.now.headOwed", "답할 일 {{count}}개 — 나머지는 저절로 돕니다", { count: owed })
    : t("board.now.headClear", "지금 답할 일이 없습니다"));
  writeTextContent(intro.querySelector(".task-board-summary"),
    boardNextWords(deskLedger ? boardRunRows(deskLedger, now, view) : [], now));
}
