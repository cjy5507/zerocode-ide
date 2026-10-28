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

/* What the backend last granted, and what the person has pressed that it has
 * not answered yet (t-59372). `revision` numbers the backend's stop: a word
 * numbered below the one shown changes nothing, one numbered the same moves
 * only the counts, and once numbers came a word without one never moves the
 * stop. `stopping` and `resuming` are the person's presses still out — neither
 * paints itself as granted — and `resumeFailed` says the last resume left the
 * stop standing. `othersStop` says the backend named the standing stop not
 * the person's — an agent's, a budget's — which theirs still goes over. */
let computerBandState = {
  active: false,
  stopped: null,
  othersStop: false,
  actions: 0,
  verb: null,
  revision: null,
  stopping: false,
  resuming: false,
  resumeFailed: false,
};
let computerBandTimer = null;
/* The window shared with an agent, as the backend last said it (t-73906):
 * its key, the words the person knows it by, and whether the person took
 * control. The band stands while a window is shared, whatever the agent last
 * did, so 「제어권 가져오기」 is always one press away. With sharing required,
 * the app whose card went unanswered waits (t-74826): the band names it and
 * offers 「맡기기」 for as long as it waits. */
let computerShareState = { target: null, label: null, takenBack: false, waiting: null, waitingLabel: null };
let computerShareRevision = null;
/* The pane and assignment of the newest look the backend announced: what the
 * band's view opens. */
let computerBandLook = null;

function computerBandNote(state) {
  if (state.stopping) return () => t("computer.band.stopping", "정지하는 중…");
  if (state.resuming) return () => t("computer.band.resuming", "다시 시작하는 중…");
  if (state.resumeFailed && state.stopped != null) {
    return () => t("computer.band.resumeFailed", "다시 시작하지 못했습니다 · 정지는 그대로입니다");
  }
  return null;
}

/* How far the person's stop reached the work browsers (t-62872), said on the
 * band and in the panel alike: still pausing, some it could not pause, or how
 * many it paused — and, once the band's resume ended it, how many the resume
 * had to leave paused; nothing when it reached none. */
function computerStopReachWords(reach) {
  if (!reach) return null;
  if (reach.pausing > 0) {
    return () => t("computer.band.browsersPausing", "작업 브라우저 {{count}}개를 멈추는 중…", { count: reach.pausing });
  }
  if (reach.failed > 0) {
    return () => t("computer.band.browsersFailed", "작업 브라우저 {{failed}}개를 멈추지 못했습니다 · 멈춘 것 {{stopped}}개 · 그 탭의 막대에서 멈춰 주세요", {
      failed: reach.failed, stopped: reach.stopped,
    });
  }
  if (reach.stopped > 0) {
    return () => t("computer.band.browsersStopped", "작업 브라우저 {{count}}개도 멈췄습니다", { count: reach.stopped });
  }
  if (reach.kept > 0) {
    return () => t("computer.band.browsersKept", "작업 브라우저 {{count}}개는 멈춘 채 둡니다 · 그 탭의 막대에서 다시 맡길 수 있습니다", { count: reach.kept });
  }
  return null;
}

function sayComputerStopReach(node, reach) {
  const words = computerStopReachWords(reach);
  say(node, words);
  if (!words) node.textContent = "";
  node.hidden = !words;
  node.classList.toggle("is-failed", (reach?.failed ?? 0) > 0 && (reach?.pausing ?? 0) === 0);
}

/* The reach the band last painted, as one word: the work browsers' chrome
 * repaints often, and the band repaints from it only when that word moved.
 * `computerBandKept` is how many the band last said the resume left paused. */
let computerBandReach = null;
let computerBandKept = 0;

function computerStopReachKey(reach) {
  return reach ? `${reach.stopped}/${reach.failed}/${reach.pausing}/${reach.kept}` : null;
}

/* What the work browsers' chrome asks of this file when it repaints: the
 * band, when the person's stop reached further, and the panel's rows. */
function paintComputerWorkBrowsers() {
  if (computerStopReachKey(browserWorkspaceStopReach()) !== computerBandReach) paintComputerBand();
  else paintComputerProgressControl();
}

function paintComputerBand() {
  const band = el("computer-band");
  const state = computerBandState;
  const stopped = state.stopped != null;
  const reach = browserWorkspaceStopReach();
  computerBandReach = computerStopReachKey(reach);
  // What the band's resume left paused is said until the band slips away.
  const kept = reach?.kept ?? 0;
  if (kept > computerBandKept) computerBandSlipAway();
  computerBandKept = kept;
  const shared = computerShareState.target != null || computerShareState.takenBack ||
    computerShareState.waiting != null;
  const show = stopped || state.active || state.stopping || state.resuming || kept > 0 || shared;
  band.hidden = !show;
  band.classList.toggle("is-stopped", stopped);
  say(el("computer-band-word"), () => stopped
    ? t("computer.band.stopped", "Computer Use 정지됨 · {{reason}}", { reason: state.stopped })
    : state.active
      ? t("computer.band.working", "Computer Use 진행 중")
      : t("computer.band.idle", "Computer Use 대기 중"));
  say(el("computer-band-count"), () => t("computer.band.actions", "행동 {{count}}", {
    count: state.actions ?? 0,
  }));
  const note = el("computer-band-note");
  const words = computerBandNote(state);
  say(note, words);
  if (!words) note.textContent = "";
  note.hidden = !words;
  sayComputerStopReach(el("computer-band-browsers"), reach);
  // The stop stays in reach while a resume is out: the person's next stop
  // does not wait behind their last resume. Over a stop that is not the
  // person's it stays in reach too: theirs goes over it (t-62872).
  el("computer-band-stop").hidden = state.stopping || (stopped && !state.resuming && !state.othersStop);
  // A resume names the stop it lifts; with no number from the backend there is
  // nothing to name. While the stop is still reaching a work browser there is
  // nothing whole to lift yet (t-62872).
  el("computer-band-resume").hidden = !stopped || state.revision == null || state.resuming || state.stopping ||
    (reach?.pausing ?? 0) > 0;
  el("computer-band-view").hidden = computerBandLook == null;
  paintComputerShare();
  paintComputerProgressControl();
}

/* The share's part of the band: which window an agent works in, the press
 * that takes it back, and — once taken — the press that hands it back; and
 * the app an agent waits for, with the press that hands it over. */
function paintComputerShare() {
  const share = computerShareState;
  const app = share.label ?? share.target;
  const line = el("computer-band-share");
  const words = app == null
    ? null
    : share.takenBack
      ? () => t("computer.share.taken", "「{{app}}」 제어권을 가져왔습니다", { app })
      : computerBandState.active && computerBandState.stopped == null
        ? () => t("computer.share.working", "에이전트가 「{{app}}」 창을 다루는 중", { app })
        : () => t("computer.share.shared", "「{{app}}」 창을 에이전트와 공유 중", { app });
  say(line, words);
  if (!words) line.textContent = "";
  line.hidden = !words;
  el("computer-band-take").hidden = share.target == null || share.takenBack;
  el("computer-band-give").hidden = !share.takenBack;
  el("computer-band-end").hidden = share.target == null;
  const waiting = share.waitingLabel ?? share.waiting;
  const wait = el("computer-band-wait");
  const waitWords = waiting == null
    ? null
    : () => t("computer.share.waiting", "에이전트가 「{{app}}」 창을 맡겨 주기를 기다립니다", { app: waiting });
  say(wait, waitWords);
  if (!waitWords) wait.textContent = "";
  wait.hidden = !waitWords;
  el("computer-band-hand").hidden = share.waiting == null;
}

function takeComputerShare(said) {
  const revision = Number.isSafeInteger(said?.revision) ? said.revision : null;
  if (revision != null && computerShareRevision != null && revision < computerShareRevision) return;
  if (revision != null) computerShareRevision = revision;
  computerShareState = {
    target: said?.target ?? null,
    label: said?.label ?? null,
    takenBack: said?.takenBack === true,
    generation: said?.generation ?? null,
    waiting: said?.waiting ?? null,
    waitingLabel: said?.waitingLabel ?? null,
  };
  paintComputerBand();
}

listen("computer:share-state", (event) => takeComputerShare(event.payload));
function syncComputerSharePanelWords() {
  void invoke("computer_share_state", {
    panelLabel: t("computer.band.take", "제어권 가져오기"),
  }).then(takeComputerShare).catch(() => {});
}
syncComputerSharePanelWords();

el("computer-band-take").addEventListener("click", () => {
  void invoke("computer_take_control", undefined, { hand: handNow() }).catch(() => {});
});

el("computer-band-hand").addEventListener("click", () => {
  const app = computerShareState.waiting;
  if (app == null) return;
  void invoke("computer_share_hand_over", { app }).catch(() => {});
});

el("computer-band-give").addEventListener("click", () => {
  void invoke("computer_give_back", undefined, { hand: handNow() }).catch(() => {});
});

el("computer-band-end").addEventListener("click", () => {
  const { target: app, generation } = computerShareState;
  if (app == null || !Number.isSafeInteger(generation)) return;
  void invoke("computer_share_end", { app, generation }, { hand: handNow() })
    .then(takeComputerShare).catch(() => {});
});

browserWorkspaceChromeReaders.add(paintComputerWorkBrowsers);

function computerBandSlipAway() {
  clearTimeout(computerBandTimer);
  computerBandTimer = setTimeout(() => {
    computerBandState = { ...computerBandState, active: false };
    forgetAgentBrowserWorkspacesKept();
    paintComputerBand();
  }, COMPUTER_BAND_IDLE_MS);
}

/* Take in the stop the backend says stands at `revision` (`null`: a word with
 * no number). While the person's own stop is out, a newer word may show a stop
 * but not take one away: their last press was a stop. A stop the backend says
 * is the person's (`persons`) reaches the work browsers too (t-62872); an
 * agent's or a budget's does not. A word of no stop at a revision the band has
 * reached lets the person's stop's record go, unless the band's own resume is
 * out to give back what it paused: a lift the band did not make gives nothing
 * back. */
function takeComputerBandStop(revision, stopped, persons) {
  const state = computerBandState;
  if (revision == null) {
    if (state.revision == null) computerBandState = { ...state, stopped };
    return;
  }
  if (stopped == null && !state.resuming && (state.revision == null || revision >= state.revision)) {
    forgetAgentBrowserWorkspaceStop();
  }
  if (state.revision != null && revision <= state.revision) return;
  const kept = state.stopping && stopped == null ? state.stopped : stopped;
  const othersStop = kept === stopped ? stopped != null && persons === false : state.othersStop;
  computerBandState = { ...state, revision, stopped: kept, othersStop, resumeFailed: false };
  if (stopped != null && persons === true) stopAgentBrowserWorkspaces(revision);
}

/* Take in the backend's report: an activity event, or the one read as the
 * page loads. */
function takeComputerActivity(report) {
  const revision = typeof report.revision === "number" ? report.revision : null;
  if (revision != null && computerBandState.revision != null && revision < computerBandState.revision) return;
  takeComputerBandStop(revision, report.stopped ?? null, report.personsStop);
  computerBandState = {
    ...computerBandState,
    active: report.active === true,
    actions: report.actions ?? 0,
    verb: report.verb ?? null,
  };
  paintComputerBand();
  if (computerBandState.active) computerBandSlipAway();
}

/* Whether an activity event reached this page yet: the read as the page loads
 * speaks only to a page that heard none. */
let computerBandHeard = false;

listen("computer:activity", (event) => {
  computerBandHeard = true;
  takeComputerActivity(event.payload ?? {});
});

/* The band reads the backend's report once as the page loads (t-62872): a stop
 * that stood across a reload shows at once and walks the work browsers,
 * without a clock and without waiting for the next action. */
invoke("computer_guard_status")
  .then((report) => {
    if (!computerBandHeard && report && typeof report === "object") takeComputerActivity(report);
  })
  .catch(() => {});

el("computer-band-stop").addEventListener("click", () => {
  if (computerBandState.stopping) return;
  computerBandState = { ...computerBandState, stopping: true };
  paintComputerBand();
  invoke("computer_stop")
    .then((answer) => {
      if (answer?.stopped !== true) return;
      const revision = typeof answer.revision === "number" ? answer.revision : null;
      takeComputerBandStop(revision, answer.reason ?? "window", answer.personsStop);
    })
    .catch(() => {})
    .finally(() => {
      computerBandState = { ...computerBandState, stopping: false };
      paintComputerBand();
    });
});

el("computer-band-resume").addEventListener("click", () => {
  const state = computerBandState;
  if (state.stopped == null || state.revision == null || state.resuming || state.stopping) return;
  computerBandState = { ...state, resuming: true, resumeFailed: false };
  paintComputerBand();
  invoke("computer_resume", { reset: false, revision: state.revision }, { hand: handNow() })
    .then((answer) => {
      if (typeof answer?.revision === "number") {
        takeComputerBandStop(answer.revision, answer.stopped ?? null, answer.personsStop);
      }
      // The resume lifted the stop it named: what that stop paused goes back.
      if (answer?.resumed === true) resumeAgentBrowserWorkspaces(state.revision);
      if (answer?.resumed === false) {
        computerBandState = { ...computerBandState, resumeFailed: computerBandState.stopped != null };
      }
    })
    .catch(() => {
      computerBandState = { ...computerBandState, resumeFailed: computerBandState.stopped != null };
    })
    .finally(() => {
      computerBandState = { ...computerBandState, resuming: false };
      // A resume whose answer failed after the backend's report already
      // lifted the stop: the stop's record goes, and nothing is given back
      // (t-62872).
      if (computerBandState.stopped == null) forgetAgentBrowserWorkspaceStop();
      paintComputerBand();
    });
});

el("computer-band-view").addEventListener("click", () => {
  if (computerBandLook) openComputerProgress(computerBandLook);
});

/* The work screen panel (t-59372): the newest look one pane took under one
 * assignment, below the band, opened only by the person. The backend tells the
 * page a look was taken by its number (`computer:frame`), never by picture; the
 * panel asks the window's own door for it — one ask out at a time, the newest
 * number held while one is out — and keeps one picture, let go when it closes or
 * turns to another assignment. Nothing here runs on a clock. The desktop's
 * pictures only: the dedicated browser's tabs are watched in their own tab, and
 * the panel says only who holds them (`paintComputerProgressControl`). */
let computerProgress = null;
let computerProgressSessions = 0;
let computerProgressAsking = false;

function computerProgressOwns(word) {
  return computerProgress != null && word?.term === computerProgress.term &&
    word?.scope === computerProgress.scope;
}

function computerProgressName(term) {
  const tab = tabOfTerm(term);
  return tab ? tabLabel(tab) : t("computer.progress.pane", "판 {{term}}", { term });
}

function paintComputerProgressSubject(look) {
  const panel = computerProgress;
  if (!panel) return;
  say(el("computer-progress-subject"), () => {
    const name = computerProgressName(panel.term);
    if (!look) return t("computer.progress.empty", "{{name}} · 아직 본 화면이 없습니다", { name });
    const time = new Date(look.at).toLocaleTimeString(document.documentElement.lang || undefined, {
      hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
    return t("computer.progress.seen", "{{name}} · {{time}}에 마지막으로 본 화면", { name, time });
  });
}

/* Who holds control, a line each (t-62464): the desktop as the backend granted
 * its stop — the band's state, in its revision order — and each work browser
 * the panel's pane owns as the dedicated browser's own state says it
 * (`tab.workspace`, which shell-browser.js takes in revision order), in that
 * browser's own words. The panel moves no control: a work browser's row leads
 * to that browser's own bar, where 「내가 조작」 and 「다시 맡기기」 change it.
 * Painted when the panel opens, when the band's state moves and with every
 * repaint of the work browsers' chrome. */
function computerProgressBrowsers(term) {
  const owner = String(term);
  return tabs.filter((tab) => tab.kind === "browser" && tab.agentWorkspace === true
    && tab.workspace?.ownerTerm === owner && browserWorkspaceLive(tab));
}

function computerProgressBrowserWords(id) {
  const tab = tabs.find((one) => one.id === id);
  if (!tab) return "";
  return t("computer.progress.browser", "작업 브라우저 {{name}} · {{state}}", {
    name: tabLabel(tab),
    state: browserWorkspaceWords(tab),
  });
}

function forgetComputerProgressRows(list) {
  for (const row of list.children) {
    for (const node of row.children) say(node, null);
  }
  list.replaceChildren();
  delete list.dataset.tabs;
}

function paintComputerProgressControl() {
  const panel = computerProgress;
  if (!panel) return;
  const { stopped, active } = computerBandState;
  // The agent holds the desktop only while it acts (the band's own word for
  // it); with no action going on nobody is said to (t-63505).
  say(el("computer-progress-desk"), () => stopped != null
    ? t("computer.progress.deskStopped", "데스크톱 · 정지됨({{reason}}) · 에이전트는 입력할 수 없습니다", { reason: stopped })
    : active
      ? t("computer.progress.deskAgent", "데스크톱 · 에이전트에게 제어권이 있습니다")
      : t("computer.progress.deskIdle", "데스크톱 · 지금 조작 중인 에이전트가 없습니다"));
  sayComputerStopReach(el("computer-progress-stop-browsers"), browserWorkspaceStopReach());
  const held = computerProgressBrowsers(panel.term);
  const list = el("computer-progress-browsers");
  const signature = held.map((tab) => tab.id).join("\n");
  if ((list.dataset.tabs ?? "") !== signature) {
    forgetComputerProgressRows(list);
    list.append(...held.map((tab) => {
      const row = document.createElement("li");
      row.dataset.tab = tab.id;
      const go = document.createElement("button");
      go.type = "button";
      go.className = "btn computer-progress-browser-go";
      row.append(document.createElement("span"), go);
      return row;
    }));
    list.dataset.tabs = signature;
  }
  for (const row of list.children) {
    const id = row.dataset.tab;
    say(row.firstElementChild, () => computerProgressBrowserWords(id));
    const go = row.lastElementChild;
    say(go, () => t("computer.progress.browserGo", "그 막대 열기"));
    // C8's own launcher leads only to the browsers of the workspace on screen.
    go.hidden = held.find((tab) => tab.id === id)?.worktree !== activeWorktreePath;
  }
  list.hidden = held.length === 0;
  const hint = el("computer-progress-browser-hint");
  say(hint, held.length === 0 ? null : () => t("computer.progress.browserHint",
    "작업 브라우저의 제어권은 그 막대의 「{{human}}」·「{{agent}}」로 바꿉니다 · 띠의 정지는 작업 브라우저도 함께 멈춥니다",
    { human: BROWSER_WORKSPACE_ACTION_WORDS.human(), agent: BROWSER_WORKSPACE_ACTION_WORDS.agent() }));
  if (held.length === 0) hint.textContent = "";
  hint.hidden = held.length === 0;
}

/* Paint one look: its number, its picture when one was kept, and why there is
 * none when there is not — never an older picture in its place. */
function paintComputerProgressLook(look) {
  const frame = el("computer-progress-frame");
  const note = el("computer-progress-note");
  frame.dataset.seq = String(look.seq);
  const picture = typeof look.png === "string" && look.png !== "";
  if (picture) frame.setAttribute("src", `data:image/png;base64,${look.png}`);
  else frame.removeAttribute("src");
  frame.hidden = !picture;
  const words = picture ? null : look.perception === "accessibility"
    ? () => t("computer.progress.accessibility", "이 관측은 화면의 구조만 읽어 그림이 없습니다")
    : look.perception === "oversize"
      ? () => t("computer.progress.oversize", "이 관측의 그림은 너무 커서 보관하지 않았습니다")
      : computerProgressGoneWords;
  say(note, words);
  if (!words) note.textContent = "";
  note.hidden = !words;
  paintComputerProgressSubject(look);
}

function computerProgressGoneWords() {
  return t("computer.progress.gone", "이 관측의 그림은 더는 보관하지 않습니다");
}

function takeComputerProgressLook(session, look) {
  const panel = computerProgress;
  if (!panel || panel.session !== session || !computerProgressOwns(look)) return;
  if (typeof look.seq !== "number" || (panel.shown != null && look.seq <= panel.shown)) return;
  panel.shown = look.seq;
  paintComputerProgressLook(look);
}

/* The backend no longer has the look the panel shows (t-62872): its door
 * answered nothing for a newer number, or can no longer tell the pane or its
 * work. The picture goes, and the panel says it is no longer kept; the line
 * of when the look was taken stays. */
function dropComputerProgressLook(session) {
  const panel = computerProgress;
  const frame = el("computer-progress-frame");
  if (!panel || panel.session !== session || panel.shown == null || !frame.hasAttribute("src")) return;
  frame.removeAttribute("src");
  frame.hidden = true;
  const note = el("computer-progress-note");
  say(note, computerProgressGoneWords);
  note.hidden = false;
}

function askComputerProgress() {
  const panel = computerProgress;
  if (!panel || !panel.wanted || computerProgressAsking) return;
  const { session, term, scope } = panel;
  const seq = panel.wantedSeq;
  panel.wanted = false;
  panel.asked = Math.max(panel.asked ?? 0, seq ?? 0);
  computerProgressAsking = true;
  invoke("computer_progress_frame", { term, scope, seq })
    .then((look) => (look == null ? dropComputerProgressLook(session) : takeComputerProgressLook(session, look)))
    .catch((error) => {
      // A look that failed to arrive leaves the last good one; a door that no
      // longer knows the pane or its work shows none, and neither does this.
      if (/^computer_progress_(pane|scope)_unknown:/u.test(String(error?.message ?? error))) {
        dropComputerProgressLook(session);
      }
    })
    .finally(() => {
      computerProgressAsking = false;
      askComputerProgress();
    });
}

/* Where the keyboard was when the person opened the panel: closing it gives
 * the keyboard back there, or to the band's view when that place is gone
 * (t-63505). */
let computerProgressReturn = null;

function forgetComputerProgress() {
  computerProgress = null;
  el("computer-progress").hidden = true;
  const frame = el("computer-progress-frame");
  frame.removeAttribute("src");
  delete frame.dataset.seq;
  frame.hidden = true;
  const note = el("computer-progress-note");
  say(note, null);
  note.textContent = "";
  note.hidden = true;
  say(el("computer-progress-subject"), null);
  say(el("computer-progress-desk"), null);
  el("computer-progress-desk").textContent = "";
  const reach = el("computer-progress-stop-browsers");
  say(reach, null);
  reach.textContent = "";
  reach.hidden = true;
  forgetComputerProgressRows(el("computer-progress-browsers"));
  el("computer-progress-browsers").hidden = true;
  const hint = el("computer-progress-browser-hint");
  say(hint, null);
  hint.textContent = "";
  hint.hidden = true;
}

function closeComputerProgress() {
  const giveBack = el("computer-progress").contains(document.activeElement)
    ? [computerProgressReturn, el("computer-band-view")].find((node) => node?.isConnected && !node.closest("[hidden]"))
    : null;
  computerProgressReturn = null;
  forgetComputerProgress();
  giveBack?.focus({ preventScroll: true });
}

function openComputerProgress(subject) {
  if (computerProgressOwns(subject)) return;
  const from = document.activeElement;
  const returnTo = computerProgress ? computerProgressReturn
    : from instanceof HTMLElement && from !== document.body && !el("computer-progress").contains(from) ? from : null;
  forgetComputerProgress();
  computerProgressReturn = returnTo;
  computerProgressSessions += 1;
  computerProgress = {
    session: computerProgressSessions,
    term: subject.term,
    scope: subject.scope,
    shown: null,
    asked: null,
    wanted: true,
    wantedSeq: null,
  };
  el("computer-progress").hidden = false;
  paintComputerProgressSubject(null);
  paintComputerProgressControl();
  askComputerProgress();
}

el("computer-progress-close").addEventListener("click", closeComputerProgress);

/* A work browser's row leads to that browser's own bar, beside the
 * conversation, as C8's own launcher does — the browser still held by this
 * pane, and only in the workspace on screen. */
el("computer-progress-browsers").addEventListener("click", (event) => {
  const row = event.target.closest("[data-tab]");
  if (!row || !event.target.closest("button") || !computerProgress) return;
  const tab = computerProgressBrowsers(computerProgress.term).find((one) => one.id === row.dataset.tab);
  if (tab && tab.worktree === activeWorktreePath) watchBrowserWorkspace(tab, { beside: true });
  else paintComputerProgressControl();
});

listen("computer:frame", (event) => {
  const word = event.payload ?? {};
  if (typeof word.term !== "number" || typeof word.scope !== "string" || typeof word.seq !== "number") return;
  const first = computerBandLook == null;
  computerBandLook = { term: word.term, scope: word.scope };
  if (first) paintComputerBand();
  const panel = computerProgress;
  if (!computerProgressOwns(word)) return;
  if (word.seq <= Math.max(panel.shown ?? 0, panel.asked ?? 0, panel.wantedSeq ?? 0)) return;
  panel.wanted = true;
  panel.wantedSeq = word.seq;
  askComputerProgress();
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
  deliver: (ask, choice, hand) => invoke("computer_confirm_answer", { id: ask.id, allow: choice.allow }, { hand }),
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
 * 「다 했어요」. The plain card is docked (see `view().docked`): the person is
 * doing something while it stands, so it takes no backdrop, no focus and no
 * Escape — 「취소」 hands the desk back unfinished. The card with a field keeps
 * the modal keyboard: Escape hands the desk back.
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
async function sendComputerCode(ask, hand) {
  const typed = computerCodeInput.value;
  if (typed.trim() === "") return holdComputerCode(ask);
  computerCodeInput.readOnly = true;
  let verdict;
  try {
    verdict = await invoke("computer_handoff_code", { id: ask.id, code: typed }, { hand });
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
  deliver: (ask, choice, hand) =>
    choice.allow && ask.payload.codeAsk ? sendComputerCode(ask, hand) : computerAnswerRoad.deliver(ask, choice, hand),
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
      // The plain card asks for something done elsewhere — a Settings row, a
      // window on the desktop — so it stands in the corner and covers nothing
      // (t-79119). The card with a field is the one where the person types IN
      // the card, and stays the modal it was.
      docked: !line,
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

/* One window shared with an agent (t-73906): the first time an agent acts
 * in an app the person is asked once whether to hand that window over. The
 * refusal holds the focus; silence lets an unattended run go on unshared —
 * or, with sharing required, keeps the agent waiting until the person hands
 * the app over from the band (t-74826) — as the clock says. Same road as the
 * confirm: the answer goes back by id. */
registerAskKind("computer-share", {
  ...computerAnswerRoad,
  // The default edge, the one blue for what waits for the person — not the
  // confirm's red, which a payment or a delete wears.
  tone: "computer-share",
  view: (ask) => ({
    agent: t("computerUse.title", "컴퓨터 사용"),
    title: t("computer.share.title", "이 창을 맡길까요?"),
    why: t("computer.share.why", "에이전트가 「{{app}}」 창 하나만 다룹니다. 포인터·포커스·키보드는 그대로 둡니다.", {
      app: ask.payload.app ?? "",
    }),
    choices: [
      { label: t("computer.share.deny", "거절"), allow: false, tone: "primary" },
      { label: t("computer.share.allow", "맡기기"), allow: true, tone: "plain" },
    ],
    initial: 0,
    safe: 0,
  }),
  clock: (ask) => ask.payload.required === true
    ? t("computer.share.clockRequired", "{{seconds}}초 안에 답이 없으면 에이전트가 기다립니다 · 나중에 띠에서 맡길 수 있습니다", {
      seconds: computerAskSeconds(ask),
    })
    : t("computer.share.clock", "{{seconds}}초 동안 답이 없으면 공유 없이 진행합니다", {
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
listen("computer:share", (event) => raiseComputerAsk("computer-share", event.payload));
listen("computer:share-closed", (event) => closeComputerAsk(event.payload?.id));

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
