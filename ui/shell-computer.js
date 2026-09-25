/* ---- Computer Use: the one hand and the last step ---------------------------
 *
 * (docs/design/computer-use-full-operator.md §1.3, §1.5.) Two small surfaces
 * over the whole window, both driven by the backend's events: a band that
 * says the operator is acting (and offers the stop — the same stop as the
 * desktop chord and `zerocode-computer stop`), and a question when a press
 * is about to land on a payment, transfer or delete control, with the two
 * buttons only a person presses. Every judgement is the backend's; this file
 * paints, counts down, and answers by id. */

/* How long the band stays after the last action before it slips away. */
const COMPUTER_BAND_IDLE_MS = 8000;
/* How often the question's clock is repainted. */
const COMPUTER_CONFIRM_TICK_MS = 1000;

let computerBandState = { active: false, stopped: null, actions: 0, verb: null };
let computerBandTimer = null;
let computerConfirmOpen = null;
let computerConfirmClock = null;

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

function paintComputerConfirmClock() {
  if (!computerConfirmOpen) return;
  const left = Math.max(0, Math.ceil((computerConfirmOpen.until - Date.now()) / 1000));
  el("computer-confirm-clock").textContent = t("computer.confirm.clock", "{{seconds}}초 뒤 거부", {
    seconds: left,
  });
}

function showComputerConfirm(ask) {
  computerConfirmOpen = { id: ask.id, until: Date.now() + (ask.timeoutMs ?? 0) };
  el("computer-confirm-text").textContent = computerConfirmWords(ask);
  el("computer-confirm").hidden = false;
  clearInterval(computerConfirmClock);
  paintComputerConfirmClock();
  computerConfirmClock = setInterval(paintComputerConfirmClock, COMPUTER_CONFIRM_TICK_MS);
  el("computer-confirm-allow").focus();
}

function hideComputerConfirm(id) {
  if (computerConfirmOpen && id && computerConfirmOpen.id !== id) return;
  computerConfirmOpen = null;
  clearInterval(computerConfirmClock);
  computerConfirmClock = null;
  el("computer-confirm").hidden = true;
}

function answerComputerConfirm(allow) {
  if (!computerConfirmOpen) return;
  const id = computerConfirmOpen.id;
  invoke("computer_confirm_answer", { id, allow }).catch(() => {});
  hideComputerConfirm(id);
}

listen("computer:confirm", (event) => {
  if (event.payload?.id) showComputerConfirm(event.payload);
});
listen("computer:confirm-closed", (event) => hideComputerConfirm(event.payload?.id));
el("computer-confirm-allow").addEventListener("click", () => answerComputerConfirm(true));
el("computer-confirm-deny").addEventListener("click", () => answerComputerConfirm(false));

/* The person's turn (§7.4): the operator hands the desk over — a 2FA code, a
 * CAPTCHA, a press it may not make — and waits for 「다 했어요」. Same channel
 * as the question: the answer goes back by id. */
let computerHandoffOpen = null;
let computerHandoffClock = null;

function paintComputerHandoffClock() {
  if (!computerHandoffOpen) return;
  const left = Math.max(0, Math.ceil((computerHandoffOpen.until - Date.now()) / 1000));
  el("computer-handoff-clock").textContent = t("computer.handoff.clock", "{{seconds}}초 남음", { seconds: left });
}

function showComputerHandoff(handoff) {
  computerHandoffOpen = { id: handoff.id, until: Date.now() + (handoff.timeoutMs ?? 0) };
  el("computer-handoff-text").textContent = handoff.reason ?? "";
  el("computer-handoff").hidden = false;
  clearInterval(computerHandoffClock);
  paintComputerHandoffClock();
  computerHandoffClock = setInterval(paintComputerHandoffClock, COMPUTER_CONFIRM_TICK_MS);
  el("computer-handoff-done").focus();
}

function hideComputerHandoff(id) {
  if (computerHandoffOpen && id && computerHandoffOpen.id !== id) return;
  computerHandoffOpen = null;
  clearInterval(computerHandoffClock);
  computerHandoffClock = null;
  el("computer-handoff").hidden = true;
}

function answerComputerHandoff(done) {
  if (!computerHandoffOpen) return;
  const id = computerHandoffOpen.id;
  invoke("computer_confirm_answer", { id, allow: done }).catch(() => {});
  hideComputerHandoff(id);
}

listen("computer:handoff", (event) => {
  if (event.payload?.id) showComputerHandoff(event.payload);
});
listen("computer:handoff-closed", (event) => hideComputerHandoff(event.payload?.id));
el("computer-handoff-done").addEventListener("click", () => answerComputerHandoff(true));
el("computer-handoff-cancel").addEventListener("click", () => answerComputerHandoff(false));

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
