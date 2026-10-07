/* ---- Computer Use: the one hand and the last step ---------------------------
 *
 * (docs/design/computer-use-full-operator.md §1.3, §1.5.) A band over the whole
 * window, driven by the backend's events, that says the operator is acting (and
 * offers the stop — the same stop as the desktop chord and `zerocode-computer
 * stop`); and two questions the operator puts to the person — a press about to
 * land on a payment, transfer or delete control, and the hand-over of the desk —
 * which are asks like any other and stand in the ask popup in the middle of the
 * window (`registerAskKind`, shell-term.js), where the person is looking. The
 * band asks nothing, so it stays where it is. Every judgement is the backend's;
 * this file paints, counts down, and answers by id. */

/* How long the band stays after the last action before it slips away. */
const COMPUTER_BAND_IDLE_MS = 8000;

let computerBandState = { active: false, stopped: null, actions: 0, verb: null };
let computerBandTimer = null;

function paintComputerBand() {
  const band = el("computer-band");
  const stopped = computerBandState.stopped != null;
  const show = stopped || computerBandState.active;
  band.hidden = !show;
  band.classList.toggle("is-stopped", stopped);
  say(el("computer-band-word"), () => stopped
    ? t("computer.band.stopped", "Computer Use 정지됨 · {{reason}}", { reason: computerBandState.stopped })
    : t("computer.band.working", "Computer Use 진행 중"));
  say(el("computer-band-count"), () => t("computer.band.actions", "행동 {{count}}", {
    count: computerBandState.actions ?? 0,
  }));
  el("computer-band-stop").hidden = stopped;
  el("computer-band-resume").hidden = !stopped;
}

function computerBandSlipAway() {
  clearTimeout(computerBandTimer);
  computerBandTimer = setTimeout(() => {
    computerBandState = { ...computerBandState, active: false };
    paintComputerBand();
  }, COMPUTER_BAND_IDLE_MS);
}

listen("computer:activity", (event) => {
  const report = event.payload ?? {};
  computerBandState = {
    active: report.active === true,
    stopped: report.stopped ?? null,
    actions: report.actions ?? 0,
    verb: report.verb ?? null,
  };
  paintComputerBand();
  if (computerBandState.active) computerBandSlipAway();
});

el("computer-band-stop").addEventListener("click", () => {
  computerBandState = { ...computerBandState, stopped: "window" };
  paintComputerBand();
  invoke("computer_stop").catch(() => {});
});

el("computer-band-resume").addEventListener("click", () => {
  computerBandState = { ...computerBandState, stopped: null, active: false };
  paintComputerBand();
  invoke("computer_resume", { reset: false }).catch(() => {});
});

function computerConfirmTitle(kind) {
  if (kind === "transfer") return t("computer.confirm.transferTitle", "이체 단추를 누를까요?");
  if (kind === "delete") return t("computer.confirm.deleteTitle", "삭제 단추를 누를까요?");
  return t("computer.confirm.paymentTitle", "결제 단추를 누를까요?");
}

function computerConfirmWords(ask) {
  const label = ask.label ?? "";
  if (ask.kind === "transfer") {
    return t("computer.confirm.transfer", "이체 단추 「{{label}}」를 누르려 합니다", { label });
  }
  if (ask.kind === "delete") {
    return t("computer.confirm.delete", "삭제 단추 「{{label}}」를 누르려 합니다", { label });
  }
  return t("computer.confirm.payment", "결제 단추 「{{label}}」를 누르려 합니다", { label });
}

/* The window's own reasons for handing the desk over, in the source
 * language (a covered press, t-12979): the key the window sends picks one,
 * and `{{app}}` is the covering window's app — never its title. Each asks
 * the catalog by its own literal key, so no sentence reaches the window
 * untranslated. */
const COMPUTER_HANDOFF_WORDS = Object.freeze({
  "computer.cover.ask": (args) => t("computer.cover.ask", "누를 곳을 「{{app}}」의 창이 가리고 있어 멈췄습니다. 그 창은 읽거나 닫지 않았습니다. 직접 정리한 뒤 「다 했어요」를 눌러 주세요.", args),
  "computer.cover.stuck": (args) => t("computer.cover.stuck", "대상 창을 앞으로 가져오고 옮겨 봤지만 누를 곳이 아직 「{{app}}」의 창에 가려져 있습니다. 직접 정리한 뒤 「다 했어요」를 눌러 주세요.", args),
  "computer.cover.gone": (args) => t("computer.cover.gone", "누르려던 창이 화면에 없습니다(최소화했거나 다른 데스크톱에 있음). 창이 보이게 한 뒤 「다 했어요」를 눌러 주세요.", args),
});

function computerHandoffText(handoff) {
  const say = COMPUTER_HANDOFF_WORDS[handoff.reasonKey];
  return say ? say(handoff.reasonArgs ?? {}) : (handoff.reason ?? "");
}

/* The seconds the backend will still wait. They count from the moment it
 * asked — the backend's own clock started then — so an ask that waits its turn
 * behind another in the popup is already spending them. Reaching nought
 * answers nothing here: the backend refuses when ITS time is up and says so with
 * a `-closed`, and the window is not the one that decides a press. */
function computerAskSeconds(ask) {
  return Math.max(0, Math.ceil((ask.until - Date.now()) / 1000));
}

/* Both answer the same road, by the id they were asked under: `allow` is the
 * press for the confirm and 「다 했어요」 for the hand-over. */
const computerAnswerRoad = {
  deliver: (ask, choice) => invoke("computer_confirm_answer", { id: ask.id, allow: choice.allow }),
};

/* The last step (§1.5): the person answers before a press lands on a payment,
 * transfer or delete control. Its default is refusing — the refusal holds the
 * focus so a stray Enter refuses, Escape refuses, and a question nobody
 * answers is refused by the backend when its time is up. */
registerAskKind("computer-confirm", {
  ...computerAnswerRoad,
  tone: "computer-confirm",
  view: (ask) => ({
    agent: t("computerUse.title", "컴퓨터 사용"),
    title: computerConfirmTitle(ask.payload.kind),
    why: computerConfirmWords(ask.payload),
    choices: [
      { label: t("computer.confirm.deny", "거부"), allow: false, tone: "primary" },
      { label: t("computer.confirm.allow", "허용"), allow: true, tone: "plain" },
    ],
    initial: 0,
    safe: 0,
  }),
  clock: (ask) => t("computer.confirm.clock", "{{seconds}}초 뒤 거부", {
    seconds: computerAskSeconds(ask),
  }),
});

/* The person's turn (§7.4): the operator hands the desk over — a 2FA code, a
 * CAPTCHA, a press it may not make — and waits for 「다 했어요」. Same channel
 * as the question: the answer goes back by id. Enter says nothing; Tab reaches
 * 「다 했어요」, and Escape hands the desk back unfinished.
 *
 * A card the window says has a line (`codeAsk`, t-40807) is the same turn with
 * one field to type a one-time code in. The window decides which cards have
 * it — the page never reads a reason for a secret — and says how much the
 * field takes; the page paints, sends what was typed by the one door that
 * takes it, and keeps nothing: the field is read once, emptied the moment
 * the code has gone, and the card holds the keyboard in it. */
const computerCodeInput = el("ask-code-input");
/* The ask the field now belongs to, and whether the window judged what was
 * typed no code (or nothing was typed). The field's text is the only copy of
 * a typed code the page has. */
let computerCodeOwner = null;
let computerCodeRefused = false;

/* What the window answers a typed code with — core's `CodeVerdict`, by name. */
const COMPUTER_CODE_VERDICT = Object.freeze({ delivered: "delivered", invalid: "invalid", gone: "gone" });

function paintComputerCodeError(limits) {
  const error = el("ask-code-error");
  error.hidden = !computerCodeRefused;
  if (computerCodeRefused) {
    say(error, () => t("computer.handoff.codeInvalid", "{{min}}~{{max}}자의 숫자·영문으로 입력해 주세요", limits));
  } else {
    say(error, null);
    error.textContent = "";
  }
  computerCodeInput.setAttribute("aria-invalid", computerCodeRefused ? "true" : "false");
}

/* The line, drawn for the card on screen — the same card repainted (a language
 * change) keeps what the hand has typed, a new card starts empty. */
function paintComputerCode(ask) {
  const limits = ask.payload.codeAsk;
  el("ask-code").hidden = !limits;
  if (!limits) return;
  if (computerCodeOwner !== ask) {
    computerCodeInput.value = "";
    computerCodeRefused = false;
    computerCodeOwner = ask;
  }
  computerCodeInput.maxLength = limits.typedMax;
  computerCodeInput.readOnly = ask.sending === true;
  say(el("ask-code-label"), () => t("computer.handoff.codeLabel", "인증번호"));
  say(el("ask-code-hint"), () => t("computer.handoff.codeHint", "{{min}}~{{max}}자의 숫자·영문 · 창이 에이전트가 정한 칸에 한 번 입력합니다. 에이전트는 번호를 받지 않습니다", limits));
  paintComputerCodeError(limits);
}

/* The card is over — answered, cancelled, or closed by the window — and what
 * was typed goes with it. */
function wipeComputerCode(ask) {
  if (computerCodeOwner !== ask) return;
  computerCodeInput.value = "";
  computerCodeRefused = false;
  computerCodeOwner = null;
}

/* The answer the card could not take, said in the card's words: the ask stays,
 * the keyboard stays in the field, and the popup is told it is held (not
 * failed) by the mark on what is thrown. */
function holdComputerCode(ask) {
  computerCodeRefused = true;
  paintComputerCodeError(ask.payload.codeAsk);
  computerCodeInput.focus({ preventScroll: true });
  throw Object.assign(new Error("held"), { askHeld: true });
}

/* Send what is typed. Nothing typed is no round trip; the window's verdict is
 * the only judge of the rest: `invalid` holds the card, `delivered` and `gone`
 * both end it (a code the window no longer waits for is not one to retry). */
async function sendComputerCode(ask) {
  const typed = computerCodeInput.value;
  if (typed.trim() === "") return holdComputerCode(ask);
  computerCodeInput.readOnly = true;
  let verdict;
  try {
    verdict = await invoke("computer_handoff_code", { id: ask.id, code: typed });
  } finally {
    computerCodeInput.readOnly = false;
  }
  if (verdict === COMPUTER_CODE_VERDICT.invalid) return holdComputerCode(ask);
  if (verdict !== COMPUTER_CODE_VERDICT.delivered && verdict !== COMPUTER_CODE_VERDICT.gone) {
    throw new Error("the window answered a code with a word this page does not know");
  }
  computerCodeInput.value = "";
  return undefined;
}

computerCodeInput.addEventListener("input", () => {
  if (!computerCodeRefused || computerCodeOwner === null) return;
  computerCodeRefused = false;
  paintComputerCodeError(computerCodeOwner.payload.codeAsk);
});

/* Enter in the field is Send, the way a button press is — a held key repeats
 * and is the popup's to ignore, and a key that is part of composing text is
 * the input method's. */
computerCodeInput.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.isComposing || event.repeat) return;
  event.preventDefault();
  el("ask-choices").querySelector("button.btn--primary")?.click();
});

registerAskKind("computer-handoff", {
  ...computerAnswerRoad,
  deliver: (ask, choice) =>
    choice.allow && ask.payload.codeAsk ? sendComputerCode(ask) : computerAnswerRoad.deliver(ask, choice),
  tone: "computer-handoff",
  view: (ask) => {
    const line = ask.payload.codeAsk;
    return {
      agent: t("computerUse.title", "컴퓨터 사용"),
      title: line ? t("computer.handoff.codeTitle", "인증번호를 입력해 주세요") : t("computer.handoff.title", "사람이 할 차례"),
      why: computerHandoffText(ask.payload),
      // Neither answer is the primary of the plain card: the primary is the
      // button Enter presses, and Enter answers nothing here. 「다 했어요」 says
      // the person did a thing on their desktop, and a stray Enter must not say
      // it for them. The keyboard starts on the frame; Tab reaches either
      // answer, and Escape hands the desk back. The card with a line starts the
      // keyboard in its field, where Enter sends what the person typed — never
      // an empty field — and Send is its primary.
      choices: line
        ? [
            { label: t("computer.handoff.cancel", "취소"), allow: false, tone: "plain" },
            { label: t("computer.handoff.codeSend", "보내기"), allow: true, tone: "primary" },
          ]
        : [
            { label: t("computer.handoff.cancel", "취소"), allow: false, tone: "plain" },
            { label: t("computer.handoff.done", "다 했어요"), allow: true, tone: "plain" },
          ],
      initial: null,
      focus: line ? "ask-code-input" : null,
      safe: 0,
    };
  },
  paint: (ask) => paintComputerCode(ask),
  clear: () => {
    el("ask-code").hidden = true;
  },
  delivered: (ask) => wipeComputerCode(ask),
  withdrawn: (ask) => wipeComputerCode(ask),
  clock: (ask) => t("computer.handoff.clock", "{{seconds}}초 남음", {
    seconds: computerAskSeconds(ask),
  }),
});

function raiseComputerAsk(kind, ask) {
  if (!ask?.id) return;
  raiseAsk({
    kind,
    key: `computer:${ask.id}`,
    id: ask.id,
    payload: ask,
    until: Date.now() + (ask.timeoutMs ?? 0),
  });
}

/* The backend closing an ask — answered from another door, or its time run out
 * — takes it away wherever it stands: on screen, or waiting its turn. */
function closeComputerAsk(id) {
  if (!id) return;
  withdrawAsks((ask) => ask.key === `computer:${id}`);
}

listen("computer:confirm", (event) => raiseComputerAsk("computer-confirm", event.payload));
listen("computer:confirm-closed", (event) => closeComputerAsk(event.payload?.id));
listen("computer:handoff", (event) => raiseComputerAsk("computer-handoff", event.payload));
listen("computer:handoff-closed", (event) => closeComputerAsk(event.payload?.id));

/* Both Settings and the refusal card name the row the list judges: the helper
 * for Accessibility, the app itself for Screen Recording (the report carries
 * that row per permission). A new probe ends the old helper session, so the
 * next action inherits the fresh grant. */
const COMPUTER_PERMISSION_IDS = ["accessibility", "screenshots"];
/* A denied action re-checks at most this often: an agent retrying every few
 * seconds must not spawn a helper probe per retry. */
const COMPUTER_PERMISSION_RECHECK_MIN_MS = 3000;
const COMPUTER_PERMISSION_HELPER_ROW = "ZeroCode Computer Use";
const COMPUTER_PERMISSION_HELPER_NAME = "ZeroCode Computer Use.app";
/* What a TCC row reads as (t-6058), in the catalog's words. The backend reads
 * the row and says its grant, why it could not be read, and which buttons it
 * offers — all three from the one table in zerocode-core
 * (`COMPUTER_PERMISSION_GRANTS`). This object only gives each of those words
 * its sentence, one entry per word (a source contract holds the two
 * together); nothing here decides a grant or a button. */
const COMPUTER_TCC_WORDS = Object.freeze({
  grant: Object.freeze({
    granted: { key: "computerUse.tccGranted", word: "현재 서명에 묶인 허용" },
    stale: { key: "computerUse.tccStale", word: "옛 빌드에 묶인 허용 — 시스템 설정에서 제거 후 다시 추가" },
    denied: { key: "computerUse.tccDenied", word: "허용 안 됨 — 행 없음 또는 거부" },
    unreadable: { key: "computerUse.tccUnreadable", word: "읽을 수 없음 — {{why}}" },
  }),
  unreadable: Object.freeze({
    "no-full-disk-access": { key: "computerUse.tccNoFullDiskAccess", word: "전체 디스크 접근 없음" },
    database: { key: "computerUse.tccDatabase", word: "TCC 데이터베이스를 열지 못함" },
    requirement: { key: "computerUse.tccRequirement", word: "기록된 서명 요구사항을 읽지 못함" },
    signature: { key: "computerUse.tccSignature", word: "번들의 현재 서명을 읽지 못함" },
  }),
  action: Object.freeze({
    reset: { key: "computerUse.tccReset", word: "초기화" },
    "open-settings": { key: "computerUse.tccOpenSettings", word: "시스템 설정 열기" },
  }),
});

/* A TCC row's sentence: its grant's words, and for a row that could not be
 * read, why. A word the table does not know is said as the backend wrote it. */
function computerTccGrantWords(row) {
  const grant = COMPUTER_TCC_WORDS.grant[row.grant];
  if (!grant) return String(row.grant ?? "");
  const why = COMPUTER_TCC_WORDS.unreadable[row.unreadable];
  return t(grant.key, grant.word, {
    why: why ? t(why.key, why.word) : String(row.unreadable ?? ""),
  });
}

function computerPermissionGuidance(id, report) {
  const row = report?.judged_rows?.find((row) => row.id === id);
  const values = { row: row?.name ?? COMPUTER_PERMISSION_HELPER_ROW, path: row?.path ?? report?.helper_app_path ?? COMPUTER_PERMISSION_HELPER_NAME };
  return id === "accessibility"
    ? t("computerUse.accessibilityHint", "시스템 설정 → 개인정보 보호 및 보안 → 손쉬운 사용에서 {{row}}를 켜세요. 목록에 없으면 +로 {{path}}를 추가하세요. 켰는데도 허용 안 됨이면 권한 재설정 후 다시 켜고 다시 확인하세요.", values)
    : t("computerUse.screenshotsHint", "시스템 설정 → 개인정보 보호 및 보안 → 화면 및 시스템 오디오 녹음에서 {{row}}를 켜세요. 목록에 없으면 +로 {{path}}를 추가하세요. 켰는데도 허용 안 됨이면 권한 재설정 후 다시 켜고 다시 확인하세요.", values);
}

let computerPermissionRecovery = null;
let computerPermissionRecoveryBusy = false;
let computerPermissionRecoveryOperation = 0;
/* The row the person put the card down for. It stays down while that same
 * row is the one missing — a retrying agent must not raise it again — and the
 * card returns when a different permission goes missing or the person acts. */
let computerPermissionDismissedFor = null;
let computerPermissionLastCheckAt = 0;

function missingComputerPermission(report) {
  return COMPUTER_PERMISSION_IDS.find((id) => report?.permissions?.some((row) => row.id === id && row.status === "not-granted")) ?? null;
}

function paintComputerPermissionRecovery() {
  const report = computerPermissionRecovery;
  const id = COMPUTER_PERMISSION_IDS.find((id) => report?.permissions?.some((row) => row.id === id && row.status === "not-granted"));
  el("computer-permission-recovery").hidden = report === null;
  say(el("computer-permission-recovery-text"), () => id
    ? computerPermissionGuidance(id, report)
    : t("computerUse.permissionRetry", "권한을 다시 확인한 뒤 요청한 동작을 다시 실행하세요."));
  for (const action of ["open", "reset", "check"]) {
    el(`computer-permission-recovery-${action}`).disabled = computerPermissionRecoveryBusy || (action !== "check" && (!id || report?.platform !== "darwin"));
  }
}

async function recoverComputerPermission(action) {
  if (computerPermissionRecoveryBusy) return;
  const operation = ++computerPermissionRecoveryOperation;
  const id = missingComputerPermission(computerPermissionRecovery);
  if (action !== "denied") computerPermissionDismissedFor = null;
  computerPermissionLastCheckAt = Date.now();
  computerPermissionRecoveryBusy = true;
  el("computer-permission-recovery-error").hidden = true;
  paintComputerPermissionRecovery();
  try {
    if (action !== "check" && id) {
      await invoke("open_computer_use_permission", { id, reset: action === "reset" });
    }
    const report = await invoke("computer_use_permission_status");
    if (operation !== computerPermissionRecoveryOperation) return;
    const ready = COMPUTER_PERMISSION_IDS.every((id) => report?.permissions?.some((row) => row.id === id && row.status === "granted"));
    const stillDismissed = action === "denied" && missingComputerPermission(report) === computerPermissionDismissedFor;
    computerPermissionRecovery = ready || stillDismissed ? null : report;
  } catch (error) {
    if (operation !== computerPermissionRecoveryOperation) return;
    el("computer-permission-recovery-error").textContent = String(error);
    el("computer-permission-recovery-error").hidden = false;
  } finally {
    if (operation === computerPermissionRecoveryOperation) {
      computerPermissionRecoveryBusy = false;
      paintComputerPermissionRecovery();
    }
  }
}

listen("computer:permission-denied", () => {
  if (Date.now() - computerPermissionLastCheckAt < COMPUTER_PERMISSION_RECHECK_MIN_MS) return;
  computerPermissionRecovery ??= {};
  void recoverComputerPermission("denied");
});
for (const action of ["open", "reset", "check"]) {
  el(`computer-permission-recovery-${action}`).addEventListener("click", () => void recoverComputerPermission(action));
}
el("computer-permission-recovery-close").addEventListener("click", () => {
  computerPermissionDismissedFor = missingComputerPermission(computerPermissionRecovery);
  computerPermissionRecovery = null;
  paintComputerPermissionRecovery();
});
