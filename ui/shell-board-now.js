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
 * `releaseLane`, shell-board.js). 여기는 그 사실에 붙는 낱말과 차례를 정한다 —
 * 원장의 영어 문장은 행에 서지 않는다: 게이트는 종류(`gate.kind`)마다 우리말 한
 * 줄이고, 원문은 「원문 보기」 안에 있다. */

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

/* 누른 결정·통과(행 키마다): 보내는 중인가, 원장이 돌려준 답, 틀린 말. 보낸 요청의
 * 이름은 답이 불확실하면 그대로 남아 다시 누르면 같은 영수증을 되받는다. */
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
 * 표의 네 상태와 그 낱말·표식. 표식은 보드의 다섯 상태 표식을 빌린다 — 도는 중은
 * 고리, 차례 기다림은 빈 점, 멈춤은 ✕, 끝남은 체크(`dressAgentGraphStateMark`). 색만
 * 으로 말하지 않는다: 표식의 모양과 낱말이 같이 선다. */
const BOARD_RUN_STATES = Object.freeze({
  run: { mark: "working", key: "board.now.stateRun", word: "도는 중" },
  queue: { mark: "idle", key: "board.now.stateQueue", word: "차례 기다림" },
  halt: { mark: "failed", key: "board.now.stateHalt", word: "멈춤" },
  done: { mark: "done", key: "board.now.stateDone", word: "끝남" },
});

/* 원장의 단계 → 표의 상태. 표에 없는 단계(닫힘·검토 자료 없음 같은 끝난 뒤의 것)는
 * 표에 서지 않는다 — 기록 탭의 과업 흐름이 센다. */
const BOARD_RUN_OF_STAGE = Object.freeze({
  dispatched: "run",
  ready: "queue",
  pending: "queue",
  gate: "halt",
  blocked: "halt",
  failed: "halt",
  reported: "done",
  verified: "done",
  merged: "done",
  deployed: "done",
});

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

/* 행의 끝 칸: 끝날 시각이 있으면 그 시각, 도는 일은 경과, 기다리는 일은 차례. 멈춘
 * 행의 끝 칸은 「원문 보기」 단추다(`paintBoardRunEnd`) — 멈춤은 상태 칸이 이미 말한다. */
function boardRunEnd(task, state, carrier, now) {
  const eta = deskTaskEta(task, now);
  if (eta !== null) return t("board.now.endAt", "{{time}}쯤", { time: knowledgeClock(eta, now) });
  if (state === "run") {
    const from = Number(carrier?.dispatch_started_ms) || 0;
    return (from > 0 ? runningWord(from, now) : null) ?? t("board.desk.releaseJustStarted", "방금 시작");
  }
  if (state === "queue") return t("board.now.stateQueue", "차례 기다림");
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
 * 다음 박자에 목록에서 뺄 때까지 그대로다: 화면은 답을 지어내지 않는다. */
async function pressBoardAsk(view, key, command, args, said) {
  const press = boardPresses.get(key) ?? { sending: false, said: "", error: "", retry: null };
  if (press.sending) return;
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

function paintBoardHold(row, ask, now, view) {
  const { task } = ask;
  const gate = task.gate;
  writeTextContent(row.querySelector(".board-ask-kind"), t("board.now.kindHold", "결정"));
  writeTextContent(row.querySelector(".board-ask-title"), deskHoldWords(gate));
  writeTextContent(row.querySelector(".board-ask-meta"), [
    task.title, task.id, gate.checkout ? basename(gate.checkout) : "",
  ].filter(Boolean).join(" · "));
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
    writeDisabled(button, Boolean(press?.sending));
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
    writeDisabled(button, Boolean(press?.sending));
    button.onclick = () => resolve("answer", option);
  });
  const own = boardAskButton(acts, "own", !held && options.length === 0);
  const writing = boardNowChoice.answers.get(ask.key);
  writeTextContent(own, t("board.now.holdOwn", "직접 답하기"));
  writeHidden(own, held || !seated);
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
  writeDisabled(field, Boolean(press?.sending));
  const submit = form.querySelector(".board-desk-reply-send");
  writeDisabled(submit, Boolean(press?.sending));
  writeTextContent(submit, press?.sending ? t("board.desk.replySending", "보내는 중…") : t("board.desk.replySend", "답 보내기"));
  writeTextContent(form.querySelector(".board-desk-reply-cancel"), t("board.desk.replyCancel", "취소"));
}

function paintBoardReview(row, ask, now, view) {
  const { task } = ask;
  const carrier = deskTaskCarrier(task);
  writeTextContent(row.querySelector(".board-ask-kind"), t("board.now.kindReview", "검토"));
  writeTextContent(row.querySelector(".board-ask-title"),
    t("board.now.reviewTitle", "{{task}} — 보고가 왔습니다", { task: task.title }));
  // What it handed in (the commit, the conditions) is the report's, behind 보고 읽기.
  writeTextContent(row.querySelector(".board-ask-meta"), [
    task.id, carrier ? agentName(carrier.agent) : "",
  ].filter(Boolean).join(" · "));
  const acts = row.querySelector(".board-ask-acts");
  const press = boardPressNote(row, ask.key);
  const open = boardNowChoice.reports.has(ask.key);
  const read = boardAskButton(acts, "read", false);
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
  writeHidden(pass, !boardSeatOf(task.run) || attempt === "" || source === "");
  writeDisabled(pass, Boolean(press?.sending));
  pass.onclick = () => void pressBoardAsk(view, ask.key, "desk_review_pass",
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

function paintBoardQuestion(row, ask, now, view) {
  const seat = boardSeatOf(ask.letter.run);
  paintDeskLetter(row, ask.letter, seat, now, view);
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
    const foot = deskElement("p", "board-asks-foot");
    foot.append(agentGraphStateMark("done"), deskElement("span", ""));
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
    ? t("board.now.asksNone", "지금 당신을 기다리는 결정은 없습니다")
    : t("board.now.asksRest", "이 밖에 당신을 기다리는 결정은 없습니다"));
  // 원장의 목록을 떠난 행은 누른 기록과 펼친 것을 데리고 간다.
  const standing = new Set(asks.map((ask) => ask.key));
  for (const key of boardPresses.keys()) if (!standing.has(key) && !boardPresses.get(key).sending) boardPresses.delete(key);
  for (const key of boardNowChoice.reports) if (!standing.has(key)) boardNowChoice.reports.delete(key);
  for (const key of boardNowChoice.answers.keys()) if (!standing.has(key)) boardNowChoice.answers.delete(key);
  const letters = new Set(asks.filter((ask) => ask.letter).map((ask) => ask.letter.id));
  for (const id of deskDrafts.keys()) if (!letters.has(id)) deskDrafts.delete(id);
  return true;
}

/* ---- 지금 도는 일 -----------------------------------------------------------
 *
 * 원장의 과업을 목표(`parent`)별로 묶은 표. 행 하나 = 상태 표식과 낱말 · 하는 일
 * 한 줄 · 워커(모델) · 작업 폴더 · 끝 칸. 끝난 과업은 목표마다 접힌 줄 하나 뒤에
 * 선다. 수는 세지 않는다 — 원장이 보낸 행(`STAGE_ROWS`)이 곧 표다. */
function boardRunRows(ledger, now, view) {
  const model = agentGraphModels.get(view);
  const entries = new Map((model?.agents ?? []).map((entry) => [entry.card.pane, entry]));
  const rows = [];
  for (const task of Array.isArray(ledger?.tasks) ? ledger.tasks : []) {
    const state = BOARD_RUN_OF_STAGE[task.stage];
    if (!state) continue;
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

const BOARD_RUN_ORDER = Object.freeze(["halt", "run", "queue", "done"]);

function boardGoals(rows) {
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
  for (const goal of goals.values()) {
    goal.live.sort((a, b) => BOARD_RUN_ORDER.indexOf(a.state) - BOARD_RUN_ORDER.indexOf(b.state) ||
      (Number(a.task.created_ms) || 0) - (Number(b.task.created_ms) || 0));
  }
  // 지금 도는 일이 있는 목표만, 오래된 목표부터. 끝난 일만 남은 목표는 이 표의 것이 아니다 —
  // 기록 탭의 과업 흐름이 센다(오래 돈 원장에서 목표 머리가 수십 줄이 되지 않게).
  return [...goals.values()].filter((goal) => goal.live.length > 0).sort((a, b) => a.at - b.at);
}

/* 한 행의 칸 여섯 — 표가 서른 행이어도 판이 가볍게, 칸 안에 상자를 겹치지 않는다. */
function boardRunRow() {
  const row = deskElement("div", "board-run");
  row.setAttribute("role", "row");
  const title = deskElement("span", "board-run-title");
  title.append(deskElement("span", "board-run-name"), deskElement("code", "board-run-id"));
  const status = deskElement("span", "board-run-state");
  status.append(agentGraphStateMark("idle"), deskElement("span", "board-run-word"));
  row.append(title, status, deskElement("span", "board-run-work"), deskElement("span", "board-run-who"),
    deskElement("code", "board-run-branch"), deskElement("span", "board-run-end"));
  for (const cell of row.children) cell.setAttribute("role", "cell");
  return row;
}

/* 끝 칸: 멈춘 행은 「원문 보기」 단추 하나, 그 밖의 행은 끝 낱말. 단추는 멈춘 행에만
 * 있다. */
function paintBoardRunEnd(node, row, now) {
  const end = node.querySelector(".board-run-end");
  if (boardRunRaw(row) === "") {
    if (end.firstElementChild) end.replaceChildren();
    writeTextContent(end, boardRunEnd(row.task, row.state, row.carrier, now));
    return;
  }
  let raw = end.firstElementChild;
  if (!raw) {
    raw = deskElement("button", "board-link board-run-raw");
    raw.type = "button";
    raw.onclick = () => {
      const key = node.dataset.key;
      if (boardNowChoice.raw.has(key)) boardNowChoice.raw.delete(key);
      else boardNowChoice.raw.add(key);
      if (node.__view) paintCoordinatorDesk(node.__view);
    };
    end.replaceChildren(raw);
  }
  writeTextContent(raw, t("board.now.raw", "원문 보기"));
  writeAttribute(raw, "aria-expanded", String(boardNowChoice.raw.has(node.dataset.key)));
}

function paintBoardRunRow(node, row, now, view) {
  const { task, state, carrier, health, entry } = row;
  node.__view = view;
  const said = BOARD_RUN_STATES[state];
  writeClassName(node, `board-run is-${state}`);
  writeAttribute(node, "data-task", `${task.run}/${task.id}`);
  writeTextContent(node.querySelector(".board-run-name"), task.title);
  writeTextContent(node.querySelector(".board-run-id"), task.id);
  dressAgentGraphStateMark(node.querySelector(".agent-graph-node-state"), said.mark);
  writeTextContent(node.querySelector(".board-run-word"),
    state === "done" ? deskStageWord(task.stage) : t(said.key, said.word));
  writeTextContent(node.querySelector(".board-run-work"), boardRunWork(task, state, carrier, health, entry));
  const model = carrier ? ((carrier.term != null ? paneModels.get(carrier.term) : null) ?? carrier.model ?? "") : "";
  writeTextContent(node.querySelector(".board-run-who"), model || (carrier ? agentName(carrier.agent) : ""));
  writeTextContent(node.querySelector(".board-run-branch"), carrier?.checkout ? basename(carrier.checkout) : "");
  paintBoardRunEnd(node, row, now);
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
 * 돌려도 다른 목표가 화면 밖으로 밀리지 않게. 멈춘 행이 먼저라 늘 보인다. */
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

function paintDeskRunning(block, now, view) {
  const ledger = deskLedger;
  if (!ledger || !Array.isArray(ledger.tasks)) return false;
  const rows = boardRunRows(ledger, now, view);
  if (!rows.some((row) => row.state !== "done")) return false;
  const head = block.firstElementChild;
  if (!head.firstElementChild) {
    const legend = deskElement("ul", "board-legend");
    for (const [id, state] of Object.entries(BOARD_RUN_STATES)) {
      const item = deskElement("li", "");
      item.dataset.state = id;
      item.append(agentGraphStateMark(state.mark), deskElement("span", ""));
      legend.append(item);
    }
    head.append(deskElement("span", "board-desk-head-word"), deskElement("span", "board-desk-head-note"), legend);
  }
  writeTextContent(head.children[0], t("board.now.running", "지금 도는 일"));
  writeTextContent(head.children[1], t("board.now.runningHint", "목표별 · 멈춘 것이 먼저"));
  const legend = head.children[2];
  writeAttribute(legend, "aria-label", t("board.now.legend", "읽는 법"));
  for (const item of legend.children) {
    const state = BOARD_RUN_STATES[item.dataset.state];
    writeTextContent(item.lastElementChild, t(state.key, state.word));
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
    foot.append(deskElement("span", ""), toHistory);
    body.replaceChildren(table, foot);
  }
  writeAttribute(table, "aria-label", t("board.now.running", "지금 도는 일"));
  const goals = boardGoals(rows);
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
      paintBoardRunRow(take(key, boardRunRow), row, now, view);
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
      paintBoardRunRow(take(`task:${row.task.run}/${row.task.id}`, boardRunRow), row, now, view);
    }
  }
  reconcileElementOrder(table, wanted);
  const foot = body.querySelector(":scope > .board-runs-foot");
  writeTextContent(foot.firstElementChild, t("board.now.runsFoot", "끝난 일과 지난 수는 기록에 있습니다"));
  writeTextContent(foot.lastElementChild, t("board.now.toHistory", "기록에서 보기"));
  return true;
}

/* ---- 맨 위 한 문장 ----------------------------------------------------------
 *
 * 「답할 일 N개 — 나머지는 저절로 돕니다」와, 그 아래 다음에 끝날 일. N은 당신 차례
 * 카드의 발이 말하는 수와 같은 하나(`deskOwed`, 백엔드의 수)다 — 두 곳에서 세면 두
 * 답이 된다. */
function boardNextWords(rows, now) {
  let next = null;
  for (const row of rows) {
    const eta = deskTaskEta(row.task, now);
    if (eta !== null && (!next || eta < next.eta)) next = { eta, title: row.task.title };
  }
  if (next) {
    return t("board.now.nextAt", "다음으로 끝나는 일은 {{title}}, {{time}}쯤입니다",
      { title: next.title, time: knowledgeClock(next.eta, now) });
  }
  const lane = releaseLane;
  if (lane && typeof lane === "object" && lane.sha && deskReleaseVerdict(lane) === "running") {
    return t("board.now.nextLane", "빌드 줄에서 {{version}}이(가) {{phase}} 단계를 돌고 있습니다",
      { version: lane.version ? `v${lane.version}` : String(lane.sha).slice(0, 8), phase: String(lane.phase || "") });
  }
  const running = rows.filter((row) => row.state === "run").length;
  return running > 0
    ? t("board.now.nextRunning", "지금 {{count}}개가 돌고 있습니다", { count: running })
    : t("board.now.nextIdle", "지금 도는 일이 없습니다");
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
