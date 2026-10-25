/* ---- 업데이트 (t-3191) ------------------------------------------------------
 *
 * The settings page 「업데이트」 and the feed's two surfaces, in one module
 * (docs/design/versioned-auto-update.md §2.3, §2.5). Every judgement is the
 * backend's (`update_runtime`): this file knocks, follows the verdict it is
 * handed (`next`: announce or download), paints the phase, and speaks the
 * words. The clock is not this file's either — `update_check` decides whether
 * a knock is a check from the table; the window only knocks on the status
 * bar's own period (`USAGE_AMBIENT_MS`), once after boot from `update_spec`,
 * when the pane is opened, and when 「지금 확인」 is pressed.
 *
 * The surfaces are the t-3005 ones: the same sticky toast shape (once per
 * version, in window memory), the same General-page notice
 * (`paintUpdateNotice`, which reads `updateReadyVersion()`), and the same
 * one restart road — `restartToInstall` asks the census and goes by the way
 * out's notice, and the backend reaches `relaunch_window` when the notice
 * ends (t-67936), where the staged bundle is swapped in right before the
 * restart. Nothing here restarts anything by itself.
 *
 * Numbers are the table's: the megabyte the progress line counts in, and the
 * knock words the backend's `Knock` enum spells. */
const UPDATE_MB = 1024 * 1024;
const UPDATE_KNOCK = Object.freeze({ clock: "clock", paneOpened: "pane_opened", button: "button" });
const UPDATE_TOAST_MARK = "update-feed";

let updatePrefs = null;
let updateSpec = null;
let updateReport = null;
let updateHistory = null;
let updateKnockInFlight = null;
let updateBootTimer = null;
const updateNotesOpen = new Set();
const updateNoticed = new Set();
let updateFeedToastNode = null;
let updateFeedToastKey = null;

function updatePhase() {
  return updateReport?.phase ?? { phase: "idle" };
}

/* The version staged beside the running bundle, or null. Read by the General
 * page's notice (shell-settings.js), so the feed's 「준비됨」 and the lane's
 * 「새 빌드 준비됨」 share one surface and one button. */
function updateReadyVersion() {
  const held = updatePhase();
  return held.phase === "ready" ? held.version : null;
}

function applyUpdateSettingsSnapshot(snapshot, first) {
  if (hasSetting(snapshot, "update_spec")) updateSpec = snapshot.update_spec ?? updateSpec;
  if (hasSetting(snapshot, "update") && (first || !sameSetting(snapshot.update, updatePrefs))) {
    updatePrefs = { ...(snapshot.update ?? {}) };
  }
  paintUpdatePane();
  updateAmbient.sync();
  armUpdateBootCheck();
  if (first) void sayRestartBack();
}

/* One knock a minute after boot (the table's `check_after_boot_secs`), then
 * the ambient poller below. A timer, not a thread; armed once. */
function armUpdateBootCheck() {
  if (updateBootTimer !== null || !updateSpec) return;
  updateBootTimer = window.setTimeout(() => {
    void askUpdateCheck(UPDATE_KNOCK.clock);
  }, updateSpec.check_after_boot_secs * 1000);
}

const updateAmbient = idlePoller({
  wanted: () => (updatePrefs?.policy ?? "ask") !== "off",
  every: USAGE_AMBIENT_MS,
  tick: () => void askUpdateCheck(UPDATE_KNOCK.clock),
});

function absorbUpdateReport(report) {
  if (report === null || typeof report !== "object") return;
  updateReport = report;
  // The check writes the clock word and may release the skip; the row it
  // hands back is the stored one.
  if (report.prefs && typeof report.prefs === "object") updatePrefs = { ...report.prefs };
  paintUpdatePane();
  paintUpdateNotice();
  if (updateReadyVersion()) raiseUpdateReadyToast();
}

/* Knock. The backend decides whether this knock is a check (policy, clock,
 * a download already in flight) and answers the phase either way; a verdict
 * with `next` is followed here — the ask policy's announcement, the auto
 * policy's quiet download. A refused knock is silence unless a person
 * pressed the button. */
async function askUpdateCheck(knock) {
  if (updateKnockInFlight !== null) return updateReport;
  const started = handNow();
  updateKnockInFlight = knock;
  paintUpdatePane();
  try {
    const report = await invoke("update_check", { knock });
    absorbUpdateReport(report);
    // The clock's knock and the pane's opening are no hand's: their quiet
    // download is the program road (shell-boot.js `invoke`), the window's
    // only one. The button's is the hand that pressed it.
    if (report?.next === "download") {
      await runUpdateDownload(knock === UPDATE_KNOCK.button ? flowHand(started) : "program");
    }
    else if (report?.next === "announce") raiseUpdateAvailableToast();
    return report;
  } catch (error) {
    if (knock === UPDATE_KNOCK.button) showError(error);
    return null;
  } finally {
    updateKnockInFlight = null;
    paintUpdatePane();
  }
}

/* 「내려받기」 — download and verify, then stage beside the bundle: two
 * commands, one road, whichever policy asked for it. Progress arrives on
 * `update:progress` while the first one runs. `started` is the hand that
 * asked — the press's, or `program` for a quiet download (t-74927) — and
 * both commands carry it (t-75209). */
async function runUpdateDownload(started = handNow()) {
  try {
    if (updatePhase().phase === "available") {
      absorbUpdateReport(await invoke("update_download", undefined, { hand: flowHand(started) }));
    }
    if (updatePhase().phase === "downloaded") {
      absorbUpdateReport(await invoke("update_install", undefined, { hand: flowHand(started) }));
    }
  } catch (error) {
    showError(error);
  }
}

/* 「다시 시작하여 설치」: the one restart road, asked about first (t-6428).
 * The staged bundle is swapped in there, right before the restart, never
 * here. */
function restartToInstall() {
  void askBeforeRestart("update-install");
}

/* ---- 떠나기 전에 (t-6428) ----
 *
 * Every restart door asks the one census before it goes (`busy_census`, the
 * census `release_status` answers too). Nothing a restart would cut: the
 * door goes unasked, naming itself. Anything busy — or a census nobody
 * could read — asks: 「끝나면 다시 시작」 hands the wait to the backend's
 * beat, which goes at the first gap (nothing running under any worker's
 * pane, nothing unread), and 「지금 다시 시작」 goes now. The wait stands as
 * one line with its own 「취소」, said again whenever the beat says it moved;
 * a wait the table ran out of asks again. Every number — the counts, the
 * minutes, the patience — is the backend's; this file keeps no clock.
 *
 * And no door restarts the window by itself (t-67936): every one goes by the
 * way out's `leave_now`, whose restart stands its notice first — the line
 * below — and the beat takes the one restart road when the notice ends. Each
 * says whose hand pressed it (`handSaid`), read before the census's await
 * takes the event away, and carries that hand to the way out (`flowHand`,
 * t-75209). */
const EXIT_WAIT_MARK = "exit-wait";
let exitWaitToast = null;

async function askBeforeRestart(door) {
  const by = handSaid();
  const started = handNow();
  let census = null;
  try {
    census = await invoke("busy_census", { road: "restart", door });
  } catch {
    census = null;
  }
  if (census && !census.busy?.busy) {
    try {
      standExitNotice(await invoke("leave_now", { road: "restart", door, by, unasked: true }, { hand: flowHand(started) }));
    } catch (error) {
      showError(error);
    }
    return;
  }
  await askLeaving(census ?? { road: "restart", door, busy: null, waitMin: null }, "", started);
}

/* How each road asks: a restart and the window's close say the same
 * census and ask it differently — the close with the minute its question
 * stands before it goes anyway. */
function leavingWords(census) {
  if (census.road === "close") {
    return {
      title: t("exit.closeTitle", "지금 닫으면 도는 일이 끊깁니다"),
      note: census.waitMin === null
        ? ""
        : t(
            "exit.closeNote",
            "「끝나면 종료」는 워커 판 아래 도는 명령이 없는 첫 틈에 종료합니다 · 최대 {{minutes}}분 · {{seconds}}초 안에 답이 없으면 지금 종료합니다",
            { minutes: census.waitMin, seconds: census.answerSec },
          ),
      confirm: t("exit.whenIdleClose", "끝나면 종료"),
      deny: t("exit.nowClose", "지금 종료"),
    };
  }
  return {
    title: t("exit.restartTitle", "다시 시작하면 도는 일이 끊깁니다"),
    note: census.waitMin === null
      ? ""
      : t(
          "exit.restartNote",
          "「끝나면 다시 시작」은 워커 판 아래 도는 명령이 없는 첫 틈에 다시 시작합니다 · 최대 {{minutes}}분 기다립니다",
          { minutes: census.waitMin },
        ),
    confirm: t("exit.whenIdleRestart", "끝나면 다시 시작"),
    deny: t("exit.nowRestart", "지금 다시 시작"),
  };
}

/* The question itself — asked by a restart door, by the window's close
 * (`exit:ask`), and again when a wait ran out: the census's words (or that
 * nobody could read it), the road's patience, and the two ways on. `lead`
 * is what is said first; `started` is the hand of the door that asked, none
 * for the backend's own question, and the answer's hand joins it. */
async function askLeaving(census, lead, started = "none") {
  const road = census.road;
  const door = census.door ?? null;
  const said = busyWords(census.busy) || t("exit.unread", "도는 일을 읽지 못했습니다");
  const words = leavingWords(census);
  const answer = await askConfirm({
    title: words.title,
    body: lead ? `${lead} ${said}` : said,
    note: words.note,
    confirm: words.confirm,
    deny: words.deny,
  });
  // The hand that answered, just after its own press (t-67936).
  const by = handSaid();
  if (answer === true) {
    try {
      standExitWait(await invoke("leave_when_idle", { road, door, by }, { hand: flowHand(started) }));
    } catch (error) {
      showError(error);
    }
  } else if (answer === false) {
    try {
      standExitNotice(await invoke("leave_now", { road, door, by }, { hand: flowHand(started) }));
    } catch (error) {
      showError(error);
    }
  } else if (road === "close") {
    // The close waits on this question; 「취소」 keeps the window.
    invoke("leave_cancel").catch(() => {});
  }
}

/* The wait's words: what still runs, whoever nobody could read, and the
 * whole minutes waited — the beat's own numbers. */
function exitWaitWords(line) {
  const parts = [t("exit.running", "도는 명령 {{n}}개", { n: line.running })];
  if (line.unknown > 0) {
    parts.push(t("exit.busyUnknown", "상태를 모르는 워커 {{n}}명", { n: line.unknown }));
  }
  parts.push(t("exit.waited", "{{minutes}}분째", { minutes: line.waitedMin }));
  const state = parts.join(" · ");
  return line.road === "close"
    ? t("exit.waitingClose", "끝나면 종료합니다 · {{state}}", { state })
    : t("exit.waitingRestart", "끝나면 다시 시작합니다 · {{state}}", { state });
}

/* One line stands for the wait: said again in place when the beat moves it,
 * never a second toast. */
function standExitWait(line) {
  if (!line || typeof line !== "object") return;
  const words = exitWaitWords(line);
  const node = exitWaitToast?.isConnected ? exitWaitToast.querySelector(".toast-text") : null;
  if (node) {
    node.textContent = words;
    return;
  }
  exitWaitToast = toast(words, "", {
    sticky: true,
    action: { label: t("app.cancel", "취소"), run: cancelExitWait },
  });
  if (exitWaitToast) exitWaitToast.dataset.notice = EXIT_WAIT_MARK;
}

function dropExitWait() {
  const note = exitWaitToast;
  exitWaitToast = null;
  if (note?.isConnected) closing(note, () => note.remove());
}

function cancelExitWait() {
  dropExitWait();
  invoke("leave_cancel").catch(() => {});
}

listen("exit:waiting", (event) => standExitWait(event?.payload));

/* ---- 다시 시작 예고 (t-67936) ----
 *
 * A restart decided — a door pressed, 「지금 다시 시작」, or a wait that found
 * its gap — stands one line with its own 「취소」 before it goes:
 * 「10초 뒤 1.1.86(으)로 다시 시작합니다」, said again in place each whole
 * second the beat counts, led by 「입력 중이라 미룹니다」 while the person's
 * typing holds it. On 2026-10-21 the window went twice by this road with no
 * word at all, and the person read it as 「갑자기 꺼짐」. The seconds and the
 * hold are the backend's (`exit_runtime`'s table); this file reports the
 * person's keys while the line stands (`leave_hold`) and keeps no clock. */
const EXIT_NOTICE_MARK = "exit-notice";
let exitNoticeToast = null;
/* Whether a key has been reported since the beat last said the line — one
 * report a line is all the backend's hold needs. */
let exitNoticeKeyed = false;

/* The build a restart brings, as the lane's notice or the feed's 「준비됨」
 * names it — the version, else the lane's sha — or nothing when neither
 * stands: the window material's relaunch brings the build it runs. */
function restartBrings() {
  const app = releaseNotice?.app ?? null;
  if (app) return (app.change === "version" ? app.installed_version : shortSha(app.installed)) || "";
  return updateReadyVersion() ?? "";
}

function exitNoticeWords(line) {
  const brings = restartBrings();
  const when = brings
    ? t("exit.noticeTo", "{{seconds}}초 뒤 {{version}}(으)로 다시 시작합니다", {
        seconds: line.seconds,
        version: brings,
      })
    : t("exit.notice", "{{seconds}}초 뒤 다시 시작합니다", { seconds: line.seconds });
  return line.held ? `${t("exit.noticeHeld", "입력 중이라 미룹니다")} · ${when}` : when;
}

/* One line stands for the notice, said again in place — and the person's
 * last key reported with each saying, so a word typed just before the
 * notice holds it too. A road that went at once (`null`) stands nothing. */
function standExitNotice(line) {
  if (!line || typeof line !== "object") return;
  dropExitWait();
  const words = exitNoticeWords(line);
  const node = exitNoticeToast?.isConnected ? exitNoticeToast.querySelector(".toast-text") : null;
  if (node) {
    node.textContent = words;
  } else {
    exitNoticeToast = toast(words, "", {
      sticky: true,
      action: { label: t("app.cancel", "취소"), run: cancelExitNotice },
    });
    if (exitNoticeToast) exitNoticeToast.dataset.notice = EXIT_NOTICE_MARK;
  }
  exitNoticeKeyed = false;
  holdExitNotice(performance.now() - handKeyedAt);
}

/* The person's hands moved `agoMs` ago: the backend holds the notice until
 * they have rested. A key that never went down says nothing. */
function holdExitNotice(agoMs) {
  if (!Number.isFinite(agoMs)) return;
  invoke("leave_hold", { agoMs: Math.max(0, Math.round(agoMs)) }).catch(() => {});
}

function dropExitNotice() {
  const note = exitNoticeToast;
  exitNoticeToast = null;
  if (note?.isConnected) closing(note, () => note.remove());
}

function cancelExitNotice() {
  dropExitNotice();
  invoke("leave_cancel").catch(() => {});
}

listen("exit:notice", (event) => standExitNotice(event?.payload));

/* A key the person presses while the notice stands holds it — anywhere in
 * the window: a pane, a field, a conversation's composer. */
window.addEventListener("keydown", (event) => {
  if (!event.isTrusted || exitNoticeKeyed || !exitNoticeToast?.isConnected) return;
  exitNoticeKeyed = true;
  holdExitNotice(0);
}, { capture: true });

/* ---- 다시 시작한 뒤 (t-67936) ----
 *
 * The window a restart started says so once, as it comes up: which build it
 * came back as, and — when the build moved — that an update was why. The
 * note is the backend's, left on the restart road and taken once by the main
 * window (`boot_after_restart`); a start that was no restart says nothing. */
const RESTART_BACK_MARK = "restart-back";

async function sayRestartBack() {
  if (isPopout) return;
  let back = null;
  try {
    back = await invoke("boot_after_restart");
  } catch {
    back = null;
  }
  if (!back || typeof back !== "object") return;
  const note = toast(restartBackWords(back), "", {
    sticky: true,
    action: { label: t("app.close", "닫기"), run: () => closing(note, () => note.remove()) },
  });
  if (note) note.dataset.notice = RESTART_BACK_MARK;
}

function restartBackWords(back) {
  if (back.version !== back.fromVersion) {
    return t("exit.backTo", "{{version}}(으)로 다시 시작했습니다 (업데이트)", { version: back.version });
  }
  if (back.commit !== back.fromCommit) {
    return t("exit.backToBuild", "새 빌드({{sha}})로 다시 시작했습니다 (업데이트)", { sha: shortSha(back.commit) });
  }
  return t("exit.back", "다시 시작했습니다");
}

/* The window's close held by the backend because it would cut work: the
 * same question, in the close's words. A restart's wait or notice standing
 * before it is gone — the backend put the close's question in its place. */
listen("exit:ask", (event) => {
  const census = event?.payload;
  if (!census || typeof census !== "object") return;
  dropExitWait();
  dropExitNotice();
  void askLeaving(census, "");
});

listen("exit:overdue", (event) => {
  const census = event?.payload;
  dropExitWait();
  if (!census || typeof census !== "object") return;
  void askLeaving(
    census,
    t("exit.overdue", "{{minutes}}분을 기다렸지만 아직 도는 명령이 있습니다.", { minutes: census.waitMin }),
  );
});

listen("update:progress", (event) => {
  const moved = event?.payload;
  if (!moved || typeof moved !== "object") return;
  updateReport = {
    ...(updateReport ?? {}),
    phase: {
      phase: "downloading",
      version: moved.version,
      received_bytes: moved.received_bytes,
      total_bytes: moved.total_bytes ?? null,
      percent: moved.percent ?? null,
    },
  };
  paintUpdatePane();
});

/* ---- the toasts: the t-3005 shape, once per version ---- */

function dismissUpdateFeedToast() {
  const note = updateFeedToastNode;
  updateFeedToastNode = null;
  updateFeedToastKey = null;
  if (note?.isConnected) closing(note, () => note.remove());
}

function raiseUpdateFeedToast(key, words, action) {
  if (updateFeedToastNode && updateFeedToastKey !== key) dismissUpdateFeedToast();
  if (updateNoticed.has(key)) return;
  updateNoticed.add(key);
  updateFeedToastKey = key;
  updateFeedToastNode = toast(words, "", { sticky: true, action });
  if (updateFeedToastNode) updateFeedToastNode.dataset.notice = UPDATE_TOAST_MARK;
}

function raiseUpdateAvailableToast() {
  const held = updatePhase();
  if (held.phase !== "available") return;
  const version = held.announced?.version ?? "";
  raiseUpdateFeedToast(
    `available:${version}`,
    t("update.available", "새 버전 {{version}}이 있습니다.", { version }),
    { label: t("update.download", "내려받기"), run: () => void runUpdateDownload() },
  );
}

function raiseUpdateReadyToast() {
  const version = updateReadyVersion();
  if (!version) return;
  raiseUpdateFeedToast(
    `ready:${version}`,
    t("update.readyVersion", "버전 {{version}}이 준비됐습니다. 다시 시작하면 적용됩니다.", { version }),
    { label: t("update.restart", "다시 시작"), run: restartToInstall },
  );
}

/* ---- the pane ---- */

function updateFailureWord(word) {
  const words = {
    network: () => t("settings.update.fail.network", "네트워크"),
    forbidden: () => t("settings.update.fail.forbidden", "접근 거부 (403)"),
    signature: () => t("settings.update.fail.signature", "서명 검증 실패"),
    signing_key_unset: () => t("settings.update.fail.signingKeyUnset", "서명 키 미설정"),
    disk: () => t("settings.update.fail.disk", "디스크"),
    malformed: () => t("settings.update.fail.malformed", "형식 오류"),
    no_asset_for_platform: () =>
      t("settings.update.fail.noAssetForPlatform", "이 플랫폼의 자산이 아직 없음"),
    nothing_published: () => t("settings.update.fail.nothingPublished", "아직 공개된 버전 없음"),
    not_a_bundle: () => t("settings.update.fail.notABundle", "앱 번들이 아님"),
    unsupported: () => t("settings.update.fail.unsupported", "지원되지 않음"),
  };
  return (words[word] ?? (() => String(word ?? "")))();
}

function updateMegabytes(bytes) {
  return (Number(bytes ?? 0) / UPDATE_MB).toFixed(1);
}

function updateWhen(iso) {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return String(iso);
  return at.toLocaleString(document.documentElement.lang || undefined);
}

function updateDate(iso) {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return String(iso);
  return at.toLocaleDateString(document.documentElement.lang || undefined);
}

/* The status line, one sentence per phase, and its tone. */
function updateStatusWords() {
  const held = updatePhase();
  if (updateKnockInFlight !== null && updateKnockInFlight !== UPDATE_KNOCK.clock) {
    return { words: t("settings.update.status.checking", "확인 중…"), tone: "" };
  }
  switch (held.phase) {
    case "dev_build":
      return {
        words: held.reason === "lane_installed"
          ? t("settings.update.status.laneInstalled", "이 빌드는 로컬 레인이 설치했습니다. 새 빌드는 「새 빌드 준비됨」이 알립니다.")
          : t("settings.update.status.devBuild", "개발 빌드는 피드를 확인하지 않습니다."),
        tone: "",
      };
    case "checking":
      return { words: t("settings.update.status.checking", "확인 중…"), tone: "" };
    case "up_to_date":
      return { words: t("settings.update.status.upToDate", "최신 버전입니다."), tone: "" };
    case "available":
      return {
        words: t("settings.update.status.available", "새 버전 {{version}}", {
          version: held.announced?.version ?? "",
        }),
        tone: "ready",
      };
    case "skipped":
      return {
        words: t("settings.update.status.skipped", "{{version}} 건너뜀", {
          version: held.announced?.version ?? "",
        }),
        tone: "",
      };
    case "downloading":
      return {
        words: held.percent === null || held.percent === undefined
          ? t("settings.update.status.downloadingBytes", "내려받는 중 {{received}} MB", {
              received: updateMegabytes(held.received_bytes),
            })
          : t("settings.update.status.downloading", "내려받는 중 {{percent}}%", {
              percent: held.percent,
            }),
        tone: "",
      };
    case "downloaded":
      return { words: t("settings.update.status.downloaded", "내려받았습니다 · 준비 중"), tone: "" };
    case "ready":
      return {
        words: t("settings.update.status.ready", "{{version}} 준비됨 · 다시 시작하면 적용됩니다", {
          version: held.version ?? "",
        }),
        tone: "ready",
      };
    case "nothing_published":
      return { words: t("settings.update.status.nothingPublished", "아직 공개된 버전이 없습니다."), tone: "" };
    case "no_asset_for_platform":
      return {
        words: t("settings.update.status.noAssetForPlatform", "이 플랫폼의 자산이 아직 없습니다."),
        tone: "",
      };
    case "failed":
      return {
        words: t("settings.update.status.failed", "실패: {{word}}", {
          word: updateFailureWord(held.word),
        }),
        tone: "failed",
      };
    default:
      return { words: t("settings.update.status.idle", "아직 확인하지 않았습니다."), tone: "" };
  }
}

/* The one sentence for the build this process runs — 「ZeroCode {{version}}
 * · 빌드 {{sha}} · {{channel}}」 — spoken by this pane's head and, since
 * t-3237, by the General page's notice under 「새 버전 {{version}}」. One
 * key, two readers; `running` is a BuildStamp (`{version, commit}`). */
function updateRunningWords(running) {
  const channel = updatePrefs?.channel ?? "stable";
  return t("settings.update.running", "ZeroCode {{version}} · 빌드 {{sha}} · {{channel}}", {
    version: running.version,
    sha: shortSha(running.commit),
    channel: channel === "beta"
      ? t("settings.update.channelBeta", "베타")
      : t("settings.update.channelStable", "안정"),
  });
}

function paintUpdatePane() {
  const running = updateReport?.running ?? null;
  const channel = updatePrefs?.channel ?? "stable";
  el("update-running").textContent = running
    ? updateRunningWords(running)
    : t("settings.update.runningUnknown", "ZeroCode");
  // 「일반」 끝의 버전 줄은 같은 낱말이다 — 누르면 숨은 페이지로 가는 길(shell-design.js).
  el("general-version").textContent = el("update-running").textContent;
  const { words, tone } = updateStatusWords();
  const status = el("update-status");
  status.textContent = words;
  status.dataset.tone = tone;
  const lastChecked = el("update-last-checked");
  lastChecked.hidden = !updatePrefs?.last_checked;
  lastChecked.textContent = updatePrefs?.last_checked
    ? t("settings.update.lastChecked", "마지막 확인 {{when}}", {
        when: updateWhen(updatePrefs.last_checked),
      })
    : "";
  const held = updatePhase();
  el("update-check-now").disabled =
    updateKnockInFlight !== null || held.phase === "downloading" || held.phase === "checking";
  el("update-download").hidden = held.phase !== "available";
  el("update-restart").hidden = held.phase !== "ready";
  el("update-skip").hidden = held.phase !== "available";
  const progress = el("update-progress");
  progress.hidden = held.phase !== "downloading";
  progress.dataset.indeterminate = String(held.phase === "downloading" && held.percent === null);
  el("update-progress-bar").style.width = held.phase === "downloading" && held.percent !== null
    ? `${held.percent}%`
    : "0%";
  for (const radio of document.querySelectorAll('input[name="update-policy"]')) {
    radio.checked = radio.value === (updatePrefs?.policy ?? "ask");
  }
  el("update-channel").value = channel;
  paintUpdateZoNote();
  paintUpdateHistory();
}

/* zo rides along (§2.4): what the boot did about `~/.local/bin/zo`, as the
 * backend reports it on every check — installed from the bundle, left alone
 * because the installed one is newer, or not installed and why. Same or no
 * bundle is nothing to say. */
function paintUpdateZoNote() {
  const note = el("update-zo-note");
  const zo = updateReport?.zo ?? null;
  const words = zo === null ? "" : {
    installed: () => t("settings.update.zoInstalled", "zo {{version}}을 ~/.local/bin 에 설치했습니다.", {
      version: zo.bundled ?? "",
    }),
    left_newer: () => t(
      "settings.update.zoNewer",
      "설치된 zo {{installed}}가 앱의 zo {{bundled}}보다 새 버전이라 그대로 두었습니다.",
      { installed: zo.installed ?? "", bundled: zo.bundled ?? "" },
    ),
    failed: () => t("settings.update.zoFailed", "zo를 설치하지 못했습니다 ({{detail}}).", {
      detail: zo.detail ?? "",
    }),
  }[zo.outcome]?.() ?? "";
  note.hidden = words === "";
  note.textContent = words;
}

/* ---- the history ---- */

async function refreshUpdateHistory(force) {
  try {
    updateHistory = await invoke("update_history", { force: Boolean(force) });
  } catch (error) {
    updateHistory = { releases: [], fetched_at: null, source: "none", failure: "network" };
    if (force) showError(error);
  }
  paintUpdateHistory();
}

function paintUpdateHistory() {
  const list = el("update-history");
  const releases = Array.isArray(updateHistory?.releases) ? updateHistory.releases : [];
  list.replaceChildren();
  for (const release of releases) {
    const row = document.createElement("li");
    row.className = "update-history-row";
    row.dataset.tag = release.tag;
    const open = updateNotesOpen.has(release.tag);
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "update-history-toggle";
    toggle.setAttribute("aria-expanded", String(open));
    const tag = document.createElement("span");
    tag.className = "update-history-tag";
    tag.textContent = release.tag;
    toggle.append(tag);
    if (release.prerelease) {
      const badge = document.createElement("span");
      badge.className = "update-history-badge";
      badge.textContent = t("settings.update.prerelease", "프리릴리즈");
      toggle.append(badge);
    }
    const name = document.createElement("span");
    name.className = "update-history-name";
    name.textContent = release.name && release.name !== release.tag ? release.name : "";
    toggle.append(name);
    const date = document.createElement("span");
    date.className = "update-history-date";
    date.textContent = updateDate(release.published_at);
    toggle.append(date);
    toggle.addEventListener("click", () => {
      if (updateNotesOpen.has(release.tag)) updateNotesOpen.delete(release.tag);
      else updateNotesOpen.add(release.tag);
      paintUpdateHistory();
    });
    row.append(toggle);
    const notes = document.createElement("pre");
    notes.className = "update-history-notes";
    notes.hidden = !open;
    notes.textContent = release.body?.trim()
      ? release.body
      : t("settings.update.noNotes", "릴리즈 노트가 없습니다.");
    row.append(notes);
    list.append(row);
  }
  el("update-history-empty").hidden = releases.length > 0;
  const note = el("update-history-note");
  const failure = updateHistory?.failure ?? null;
  const when = updateWhen(updateHistory?.fetched_at);
  if (failure) {
    note.textContent = t("settings.update.historyFailed", "이력을 새로 읽지 못했습니다 ({{word}})", {
      word: updateFailureWord(failure),
    });
  } else if (updateHistory?.source === "cache") {
    note.textContent = t("settings.update.historyCached", "캐시 · {{when}}", { when });
  } else if (updateHistory?.source === "live") {
    note.textContent = t("settings.update.historyLive", "새로 읽음 · {{when}}", { when });
  } else {
    note.textContent = "";
  }
}

/* ---- the controls ---- */

el("update-check-now").addEventListener("click", () => {
  void askUpdateCheck(UPDATE_KNOCK.button);
});

el("update-download").addEventListener("click", () => {
  void runUpdateDownload();
});

el("update-restart").addEventListener("click", restartToInstall);

el("update-skip").addEventListener("click", () => {
  const held = updatePhase();
  if (held.phase !== "available") return;
  const version = held.announced?.version ?? null;
  if (!version) return;
  // Painted as skipped at once; the next check judges it again from the row.
  updateReport = { ...updateReport, phase: { phase: "skipped", announced: held.announced } };
  dismissUpdateFeedToast();
  paintUpdatePane();
  void commitSetting("update", "patch_update_prefs", {
    patch: { kind: "skipped_version", value: version },
  });
});

for (const radio of document.querySelectorAll('input[name="update-policy"]')) {
  radio.addEventListener("change", () => {
    if (!radio.checked) return;
    updatePrefs = { ...(updatePrefs ?? {}), policy: radio.value };
    updateAmbient.sync();
    void commitSetting("update", "patch_update_prefs", {
      patch: { kind: "policy", value: radio.value },
    });
  });
}

el("update-channel").addEventListener("change", () => {
  const channel = el("update-channel").value;
  updatePrefs = { ...(updatePrefs ?? {}), channel };
  paintUpdatePane();
  void commitSetting("update", "patch_update_prefs", {
    patch: { kind: "channel", value: channel },
  }).then(() => askUpdateCheck(UPDATE_KNOCK.paneOpened));
});

el("update-history-refresh").addEventListener("click", () => {
  void refreshUpdateHistory(true);
});

/* The pane's arrival: a check (unless the policy is off — the backend
 * judges) and the history, both once per arrival. */
function enterUpdatePane() {
  void askUpdateCheck(UPDATE_KNOCK.paneOpened);
  void refreshUpdateHistory(false);
}
