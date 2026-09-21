/* ---- events from the backend ---- */

listen("lane:updated", (event) => {
  upsertLaneWithSubagents(event.payload);
});

/* Self-healing geometry: if a frame arrives sized for a different grid than
 * the stage can show — a stale size from before a resize landed, or metrics
 * that changed when the real font loaded — ask for the right size again.
 * Throttled so a disagreement converges instead of oscillating. */
let lastHeal = 0;

listen("lane:screen", (event) => {
  const { lane, delta } = event.payload;
  // Screens stream only for the registry's focused lane; a frame for anyone
  // else is a switch that has not reached the view yet — skip it.
  if (lane !== focusedId) return;
  noteMouseModes(`lane:${lane}`, delta);
  stageView.apply(delta);
  // The clock is asked BEFORE the measurement, not after. `gridSize` reads
  // layout back out of the DOM that `apply` just wrote, which forces a
  // synchronous reflow — doing that on every frame is the classic write-then-
  // read thrash, and the heal it feeds only ever acts once a second anyway.
  if (Date.now() - lastHeal <= 1000) return;
  // The clock records the CHECK, not the heal. Writing it only when a
  // mismatch was found left it at zero for a window whose size already
  // agrees — which is every window, almost always — so the guard never
  // closed and the reflow ran on every frame. That is a synchronous layout
  // read against the DOM `apply` just wrote, sixty times a second, on the
  // path a keystroke echoes back through: it is felt as typing lag.
  lastHeal = Date.now();
  const want = stageView.gridSize();
  const [gotRows, gotCols] = delta.size;
  if (Math.abs(gotRows - want.rows) > 2 || Math.abs(gotCols - want.cols) > 2) {
    resizeStageLane();
  }
});

listen("lane:closed", (event) => {
  zoIntegrationRecords.delete(event.payload.lane);
  const session = lanes.get(event.payload.lane)?.lane.session_id;
  if (session) clearLaneSubagents(session);
  removeLane(event.payload.lane);
  // The worktree rows carry a count of the sessions inside them, so a lane
  // ending changes what they say. Without this the number stays at what it
  // was until something else happened to redraw the list.
  refreshWorktrees();
});

/* ---- what an agent reports about itself ------------------------------------
 *
 * The point of the whole hook bridge, in one line of state: which shell has an
 * agent in it, and what that agent last said it was doing. Before this, the
 * only way to know an agent had stopped to ask permission was to be looking at
 * its pane — and a question in a pane behind another tab is work that silently
 * never happens.
 *
 * Keyed by terminal id, not by tab: a split tab has several shells and each can
 * hold its own agent. The worst state among a tab's shells is what the tab
 * wears, for the same reason a worktree row shows its worst lane. */
function paneHookStates() {
  const states = new Map();
  const observed = new Map();
  const fromSnapshot = new Set();
  const deleteState = states.delete.bind(states);
  const clearStates = states.clear.bind(states);
  states.observedAt = (term) => observed.get(term) ?? 0;
  states.observe = (term, at) => observed.set(term, at);
  states.noteSnapshot = (term) => fromSnapshot.add(term);
  states.consumeSnapshot = (term) => fromSnapshot.delete(term);
  states.delete = (term) => {
    observed.delete(term);
    fromSnapshot.delete(term);
    return deleteState(term);
  };
  states.clear = () => {
    observed.clear();
    fromSnapshot.clear();
    clearStates();
  };
  return states;
}
const hookStates = paneHookStates();
/* The model a pane's agent is on, sticky the way Orca's lead state keeps it
 * (index.js:10385): the payload's word when it says one, the last word kept
 * when it does not. And WHEN the state last moved, for the row's "now"/"1m"
 * — both owned by the shell and dropped with it in `dropTermView`. */
const paneModels = new Map();
/* The permission mode a pane last reported, as the CLI spelled it
 * (`bypassPermissions`, `workspace-write`) — the composer's mode chip reads it. */
const panePermissionModes = new Map();
/* The words a pane's agent is saying right now, by pane: what its own channel
 * streams (zo's `text_delta` and `reasoning` frames, `session:frame`) before
 * its transcript carries the turn — the pane's counterpart of a wire's
 * `wire_log.live`, drawn by the same streaming rows (`syncStreamingTurns`). A
 * piece the voice finished (`done`) stands until the turn that says those
 * words arrives (`settlePaneLive`), so the close is one paint, never a blink. */
const paneLive = new Map();
/* The question a pane's program is asking, when the hook DESCRIBED it — a
 * permission request (tool, summary, the edit as rows) or an AskUserQuestion
 * with its options — keyed by term, cleared by the next report that is not a
 * question. The conversation view draws it as the extension's card; a
 * question the hook only signalled is on the program's screen alone. */
const paneAsks = new Map();
const hookStamps = new Map();

/* The live turn's prompt and the last assistant words, sticky the same way —
 * Orca's `entry.prompt` and `entry.lastAssistantMessage`, the two rungs of the
 * compact row's word ladder that outlive the event that said them. The backend
 * folds both to one short line before they ride the wire, so what sits here is
 * already row-sized. Dropped with the shell in `dropTermView`. */
const panePrompts = new Map();
const paneSaid = new Map();

/* And WHICH CONVERSATION each shell is on, when its agent said so.
 *
 * The id belongs to the vendor and outlives our process, which is the whole
 * point: a tab can be closed and the same conversation reopened. Held here
 * because the tab menu asks about the tab in front of it, and a round trip to
 * the backend to decide whether to draw one row would make the menu wait on a
 * command. */
const paneSessions = new Map();

/* zo's IDE channel reports the helpers inside one lane as a complete
 * session-scoped snapshot. The nodes live with that snapshot rather than in
 * the worktree-agent ledger: these helpers share the lane's one TUI and have
 * no terminal pane of their own. */
const laneSubagents = new Map();

/* And WHEN each shell began. The ledgers above know only the last state change,
 * which is not when the work started — a row's clock asks for the beginning,
 * and this window's first paint of the shell is the honest stand-in for it. */
const paneBorn = new Map();

/* And WHICH AGENT runs in each shell, for the surfaces that must answer
 * synchronously — the pane header's continue button renders on every
 * `renderPanes` and cannot wait on a command. Orca's predicate is the same
 * two-step (`resolveSourceAgent`: the pane's reported agent, else the tab's
 * launch agent); the backend's `agent_terms` stays the authority and the
 * continuation dialog re-asks it, so a stale entry here can show a button a
 * beat early but can never hand the wrong agent a transcript. Seeded at boot
 * from the backend's ledger, refreshed by every `hook:agent` event. */
const paneAgents = new Map();

/* Restart restoration facts, owned only for this window's lifetime.
 *
 * `restoredWorkers` is how a live pane was born and disappears on its first
 * real hook. `restoringWorkers` is the checkout whose sleeping worker is still
 * waiting for its coordinator team; the `term:worker` event replaces it with
 * the live row. Neither belongs in pane-layout persistence — the ledger owns
 * both lifetimes. */
const restoredWorkers = new Map();
const restoringWorkers = new Map();

/* What the ORCHESTRATION LEDGER says about the panes this window holds, by
 * shell id: which task the seat carries (its title is the row's first words —
 * a person reads "navigator cleanup", not "Claude"), whether the worker has
 * reported, and what a coordinator wrote about the outcome (verified, merged,
 * deployed — `ReviewFacts`). None of it is inferred from a hook: a provider's
 * turn ending is a turn ending, a `worker_done` is the worker's claim, and
 * only the coordinator's own keys say verified or merged.
 *
 * Kept true by the ledger, not by boot: seeded once, then re-read on every
 * `ledger:changed` beat (the window's own beat says so whenever the ledger's
 * revision moved) and on `agents:changed` — so a task verified an hour after
 * its worker finished changes its row then, not at the next restart. NOT on
 * the agent clock: the backend publishes the board snapshot before emitting
 * the beat event, and that event is the poll. Restore asks its existing
 * asynchronous ownership command for a current seat. */
// Navigator events and board paints share the answer already in flight.
// Clear on rejection too, so a failed poll cannot park future readers forever.
let ledgerAgentsPending = null;
function readLedgerAgents() {
  if (!ledgerAgentsPending) {
    ledgerAgentsPending = invoke("ledger_agents").finally(() => { ledgerAgentsPending = null; });
  }
  return ledgerAgentsPending;
}

const paneLedger = new Map();
let paneLedgerAsking = false;
async function refreshPaneLedger() {
  if (paneLedgerAsking) return;
  paneLedgerAsking = true;
  try {
    const rows = await readLedgerAgents();
    const next = new Map();
    for (const row of rows ?? []) {
      if (typeof row?.term !== "number") continue;
      next.set(row.term, {
        run: row.run ?? "",
        worker: row.worker ?? "",
        task: row.task ?? "",
        taskId: row.task_id ?? "",
        ledger: row.ledger ?? "",
        reported: row.reported === true,
        review: row.review ?? null,
      });
    }
    // Replaced whole, so a seat the ledger released stops wearing its old
    // task — and only a real change costs a repaint.
    if (paneLedgerSaid(next) !== paneLedgerSaid(paneLedger)) {
      paneLedger.clear();
      for (const [term, facts] of next) paneLedger.set(term, facts);
      scheduleAgentPaint(["cards", "board"]);
    }
  } catch {
    // The ledger may be unavailable in this window; the rows then say what
    // the hooks say, which is what they said before this map existed.
  } finally {
    paneLedgerAsking = false;
  }
}
function paneLedgerSaid(map) {
  return [...map.entries()]
    .sort(([a], [b]) => a - b)
    .map(([term, f]) =>
      `${term}:${f.taskId}:${f.task}:${f.ledger}:${f.reported ? 1 : 0}:${JSON.stringify(f.review)}`)
    .join(",");
}
listen("ledger:changed", () => {
  void refreshPaneLedger();
  // A worker that finished may have left a report; the chips count again.
  void refreshArtifactCounts();
});

/* The ledger's word for where a seat's WORK stands — the row's secondary
 * words when the provider's own turn has nothing better to say. Only the
 * facts the ledger holds: reported is the worker's claim; verified, merged
 * and deployed are the coordinator's. A turn that ended without a report is
 * not "awaiting review" — nothing was handed in. */
function paneLedgerWord(term) {
  const facts = paneLedger.get(term);
  if (!facts) return "";
  const review = facts.review ?? {};
  if (review.deployed) return t("board.deployed", "배포됨");
  if (review.merged) return t("board.merged", "병합됨");
  if (review.verified) return t("board.verified", "검증됨");
  if (facts.reported) return t("board.awaitingReview", "검증 대기");
  return "";
}

function seedPaneAgents() {
  void refreshPaneLedger();
  return invoke("agent_terms")
    .then((rows) => {
      for (const [term, agent] of rows ?? []) paneAgents.set(term, agent);
      // The surfaces that wear these answers have usually painted already —
      // boot does not wait for this call. Repainted here or a restarted
      // window shows bare tabs and faceless rows until the next hook
      // happens to land.
      if ((rows ?? []).length > 0) {
        renderTabs();
        paintWorktreeAgents();
        agentClock.sync();
      }
    })
    .catch(() => {});
}

/* The one door every agent launch goes through, and the ledger entry it
 * writes on the way. The backend records `agent_terms` at the spawn; this is
 * the window's half of that same fact, written HERE because ten call sites
 * each writing it is how one forgets. Without it, an agent whose vendor has
 * no hook wired — zo has none, codex's slot on this machine is held by the
 * real Orca — never spoke and so never stood a sidebar row ("지금 프로젝트에
 * 모델들이 안 떠, claude만 뜨는데"), while the board's own rule already says
 * a live pty we launched as an agent is doing SOMETHING (`pane_agents`,
 * main.rs). The slug is the args' own `agent` word, known at the spawn. */
async function launchAgentTab(args) {
  const term = await invoke("launch_agent_tab", args);
  paneAgents.set(term, args.agent);
  agentClock.sync();
  return term;
}

/* Whether this pane holds a conversation worth continuing — Orca's
 * `canContinueAgentSessionInNewSession(status.agentType ?? tab.launchAgent)`.
 * The tab fallback answers for an agent launched a moment ago whose first
 * hook has not landed, and only when the tab has a single pane: on a split
 * tab the tab-level word cannot say WHICH pane the agent is in. */
function paneHoldsAgent(tab, term) {
  if (paneAgents.has(term)) return true;
  return Boolean(tab.agent) && paneLeaves(tab.layout).length === 1;
}

/* When each pane was last LOOKED AT — Orca's `acknowledgedAgentsByPaneKey`
 * (index-ftls8Hg_.js:26399): a card is unseen while the pane's state started
 * after the person last had it in front of them. Keyed the wire's way
 * (`term:N`) because that is the name the card already carries. */
const acknowledgedPanes = new Map();

function acknowledgeTab(tab) {
  if (tab?.kind !== "term") return;
  for (const term of paneLeaves(tab.layout)) {
    acknowledgedPanes.set(`term:${term}`, Date.now());
    // Looking at the tab is the answer to its bell. Cleared here rather than
    // in `setActiveTab` so every road that counts as "seen" clears it — this
    // is already the one door acknowledgement walks through.
    bellRang.delete(term);
  }
}

/* ---- the bell ----
 *
 * Shells that rang while nobody was looking at them.
 *
 * The grid used to swallow BEL outright, which meant a background tab had no
 * way to say anything at all: a build that finished, a prompt waiting on an
 * answer, an agent that wants a decision — all silent, and the only way to
 * find out was to click through every tab. Orca marks the tab and debounces an
 * OS notification off the same signal (`onBell`, :20591).
 *
 * The mark, not the notification. This window already has an OS-notification
 * road with its own rules about when it is allowed to interrupt, and a bell is
 * far too cheap a signal to be given that — a program can ring it in a loop.
 * What it earns is the badge the strip already uses to say "this one wants
 * you", which is the same sentence. */
const bellRang = new Set();

/* Orca's own debounce (`onBell`, :20591). A shell can ring the bell on every
 * line it prints — a `find` with a broken pipe will — and each ring would
 * otherwise cost a full strip render. One render per quarter second is well
 * under what an eye resolves and bounded no matter how hard a program leans on
 * it. */
const BELL_DEBOUNCE_MS = 250;
let bellRender = null;

function noteBell(term) {
  const tab = tabOfTerm(term);
  if (tab === null) return;
  // A bell from the tab in front of a person who is here to hear it has
  // already done its job. Badging THAT is badging the thing being read — and
  // it would never clear, because acknowledgement is "look at the tab" and
  // they are looking at it.
  if (tab.id === activeTabId && document.hasFocus()) return;
  bellRang.add(term);
  if (bellRender !== null) return;
  bellRender = window.setTimeout(() => {
    bellRender = null;
    renderTabs();
  }, BELL_DEBOUNCE_MS);
}

/* The session a tab could be resumed onto, or `null`.
 *
 * `null` for three different reasons and the caller does not care which: the tab
 * holds no shell that reported one, the agent publishes no resumable id, or the
 * agent publishes one and offers no way back. All three mean the same thing to a
 * menu — do not offer it. */
function resumableSessionOf(tab) {
  if (tab.kind !== "term") return null;
  for (const term of paneLeaves(tab.layout)) {
    const known = paneSessions.get(term);
    if (known?.resumable) return known;
  }
  return null;
}

/* Open a conversation — or go to the pane already holding it.
 *
 * The command line is built by the BACKEND from its measured table — this hands
 * over the agent and the session and nothing else. A resume argv assembled here
 * would be this window guessing at a vendor's flags. Whether the conversation is
 * already open is the backend's answer too (`wakeConversation`): one transcript,
 * one process, judged once for every door. The tab menu's 「이 대화 다시 열기」
 * comes through here as well, and on a pane whose agent is still running it
 * goes to that pane rather than starting a second writer on its transcript; a
 * pane whose agent was quit back to its shell holds nothing, and reopening that
 * conversation is what the row is for. */
async function resumeSession(known) {
  try {
    const woke = await wakeConversation(known.agent, known.session, spawnGrid({ placement: "tab" }));
    if (woke.standing) {
      // Held in a tab: that pane is the answer. In none yet: the door that is
      // opening it puts it on the stage itself.
      const tab = tabOfTerm(woke.term);
      if (tab) await focusAgentPane(tab.worktree, tab.id, woke.term, known.agent);
      return;
    }
    // Named for the conversation rather than numbered: the whole point of
    // reopening one is that it is the SAME conversation, and `Terminal 12` says
    // nothing about which.
    //
    // Its own tab, never a division: going back into a conversation is a
    // destination, and the pane it would have cut is the one somebody was
    // reading when they asked to go back.
    mountTermTab(
      woke.term,
      { agent: agentName(known.agent), resumed: true },
      { placement: "tab" },
    );
  } catch (error) {
    showError(String(error));
  }
}

/* ---- the agent ontology graph ------------------------------------------- */
let boardQuery = "";
let agentBoardMode = "tasks";
let taskBoardFilter = "all";
const taskBoardModels = new WeakMap();
const taskBoardHistoryOpen = new WeakSet();
/* The graph is an inventory-backed control surface. Rust still owns state
 * classification, but it must classify the complete card set: idle folding
 * and search dimming are presentation decisions made after that snapshot. */
const AGENT_GRAPH_INCLUDE_IDLE = true;

function agentGraphSnapshotQuery() {
  return { query: "", projects: [], show_idle: AGENT_GRAPH_INCLUDE_IDLE };
}
/* What the badge on the door last said, so a repaint that does not change it
 * does not touch the DOM — this is recomputed on every hook event. */
let boardStuck = 0;

/* One selected ontology entity across repaints and between the docked and
 * pop-out documents. Each document keeps its own last model in a WeakMap;
 * selection is a user fact, while the model belongs to the DOM that drew it. */
let agentGraphSelectedKey = null;
let agentGraphSelectedEdgeKey = null;
let agentGraphScopeKey = "";
let agentGraphScopeLabel = "";
let agentGraphInspectorTab = "relations";
let agentGraphCardDetails = false;
const agentGraphScopeSources = new WeakMap();
const agentGraphScopeFolded = new Set();
const agentGraphScopeIdleCollapsed = new Set();
const agentGraphTaskScopes = new WeakMap();
const agentGraphInspectorViews = new WeakMap();
const agentGraphEdgeMeasurements = new WeakMap();
let agentGraphOverlayMode = "none";
let agentGraphFollowing = false;
let agentGraphHotKey = null;
let agentGraphSpaceHeld = false;
const AGENT_GRAPH_OVERLAYS = new Set(["none", "mail", "dependency", "merge"]);
const agentGraphModels = new WeakMap();
let agentGraphLayoutRuns = 0;
let agentGraphNodeCreations = 0;
let agentGraphEdgeMeasureRuns = 0;
const agentGraphEdgeFrames = new WeakMap();
/* The lanes that are not agents. The project is NOT one of them — it is the
 * band a lane strip runs across, because the number that grows without bound
 * on this surface is projects, and a project in a column is a heading that
 * scrolls off the left edge the moment a lineage gets deep. */
const AGENT_GRAPH_CONTEXT_LAYERS = ["workspace"];
const AGENT_GRAPH_LANE_CLASSES = ["lane-1", "lane-2", "lane-3", "lane-4", "lane-5"];
const AGENT_RELATIVE_TIME_TICK_MS = 30_000;
/* 카드가 그리는 활동의 양 (t-2374).
 *
 * 세 줄의 와이어였다 — 도구 호출 셋을 각각 한 줄로. 그 셋은 "무엇을 하는
 * 중인가"에는 답했지만 "얼마나 오래 이러고 있는가"에는 답하지 못했고, 셋을
 * 넘기려면 카드가 로그가 되어야 했다. 지금은 답이 둘로 갈린다: 활동 **줄**
 * 하나가 마지막 호출을 문장으로 말하고, 활동 **띠**가 그 뒤 여덟을 종류별
 * 글리프로 한 줄에 눕힌다 — 로그가 아니라 맥박이다.
 *
 * 토큰이 아니라 상수인 것은 이것이 시각 수치가 아니라 **데이터 양**이기
 * 때문이다. 여덟은 CSS가 골라도 되는 수가 아니다. */
const AGENT_GRAPH_TICKER_KEEP = 8;
const AGENT_GRAPH_ACTIVITY_KEEP = 3;
/* 도구 하나가 어느 종류인가 — 한 표.
 *
 * 활동 줄의 낱말(`activityWord`)은 벤더의 동사를 그대로 옮기지만, 띠는 여덟
 * 개를 한 줄에 눕히므로 그만큼 굵은 어휘가 필요하다: 읽기·고치기·실행·보고·
 * 질문 다섯. 표가 하나인 것은 §6의 규칙 그대로다 — 그래프 노드와 인스펙터
 * 카드가 같은 글리프를 두 곳에서 따로 고르면 한 화면의 카드 둘이 같은 일을
 * 다른 모양으로 말한다. 이름 없는 도구(이 창이 모르는 MCP)는 「읽기」가 아니라
 * 제 몫의 「그 밖」이다 — 모르는 것을 아는 것으로 적지 않는다. */
const AGENT_ACTIVITY_KINDS = new Map([
  ["read", "read"], ["grep", "read"], ["web", "read"],
  ["edit", "edit"], ["write", "edit"],
  ["bash", "run"],
  ["task", "report"], ["stop", "report"],
  ["prompt", "ask"],
]);
/* 글리프는 이 창이 **이미 그리고 있는** 글자들에서 고른다. 첫 판의 `\u25b8`와
 * `\u25aa`는 이 폰트에 없어 둘 다 점으로 떨어졌다(찍은 그림에서 실행과 보고가
 * 구별되지 않았다) — 아래 여섯은 이 창이 다른 자리에서 이미 쓰는 글자들이라
 * 대체 글리프로 떨어지지 않는다. */
const AGENT_ACTIVITY_GLYPHS = new Map([
  ["read", "\u25cb"], ["edit", "\u25c6"], ["run", "\u2192"],
  ["report", "\u2191"], ["ask", "?"], ["other", "\u00b7"],
]);
/* 컨텍스트 미터가 경고의 색으로 넘어가는 자리 (설계 §4). 데이터의 문턱이지
 * 시각 수치가 아니므로 상수로 남는다 — 이 수를 CSS가 골라도 되는 값으로
 * 내리면 「80%가 되면 노랗다」는 계약이 두 곳에 적힌다. */
const AGENT_GRAPH_CONTEXT_FULL = 0.8;
const AGENT_GRAPH_QUIET_AFTER_MS = 5 * 60_000;
/* Which project bands a person has closed, how far the canvas is zoomed, and
 * whether the legend stands. All three are user facts rather than model facts
 * and live beside the selection: a hook event repaints the graph several times
 * a second, and a repaint that reopened a band somebody just closed would make
 * the fold useless on exactly the busy surface it exists for. */
const agentGraphFolded = new Set();
/* Projects without a single agent start as one summary row. Unlike an active
 * project's ordinary fold, opening dormant inventory is the exceptional user
 * choice, so remember the OPEN set: newly discovered quiet projects then keep
 * the useful default without a paint mutating presentation state. */
const agentGraphDormantExpanded = new Set();
/* Idle inventory is never removed from the model. This set records the
 * workspaces whose presentation chip a person explicitly opened. */
const agentGraphIdleExpanded = new Set();
const agentGraphExpandedTimelines = new Set();
let agentGraphZoom = 1;
let agentGraphLegendOpen = true;
let agentGraphClock = null;

function agentGraphClockBeat(columns) {
  if (agentGraphClock !== null) clearTimeout(agentGraphClock);
  agentGraphClock = null;
  if (!columns.some((column) => column.cards.some((card) => card.at > 0))) return;
  agentGraphClock = window.setTimeout(() => {
    agentGraphClock = null;
    if (!document.hidden) scheduleAgentPaint(["board"]);
  }, AGENT_RELATIVE_TIME_TICK_MS);
}

/* What a person has picked on each card's question, by the card's pane.
 *
 * Module state because the board repaints on every hook event, and a repaint
 * that dropped half-made picks would make the panel unusable while other
 * agents stream. Each draft remembers WHICH question it answers — Orca's
 * dismiss key (`nativeChatCardDismissKey`, index-ftls8Hg_.js:69233), the
 * question count and the first question's text — so a new question on the
 * same pane starts clean instead of inheriting picks that answered something
 * else. */
const askDrafts = new Map();

function askSignature(card) {
  const prompt = card.ask_prompt;
  return JSON.stringify(prompt ? ["question", prompt] : ["approval", card.approval]);
}

function askDraftFor(card) {
  const sig = askSignature(card);
  const held = askDrafts.get(card.pane);
  if (held && held.sig === sig) return held;
  const draft = askDraftOf(card, sig);
  askDrafts.set(card.pane, draft);
  return draft;
}

/* A fresh draft for one question. The board keeps its drafts by card; the
 * conversation view keeps one with its page — the same shape, so the panels
 * read either. `repaint` is the surface's own repaint (the board's when
 * unset); `instead` is the conversation's third road on a permission card. */
function askDraftOf(card, sig, { repaint = null, instead = null, answer = null, base = "" } = {}) {
  return {
    sig,
    index: 0,
    open: false,
    sending: false,
    focusOther: false,
    selections: (card.ask_prompt?.questions ?? []).map(() => []),
    other: (card.ask_prompt?.questions ?? []).map(() => ""),
    repaint,
    instead,
    // The road an answer takes when it is not the board's keystroke road:
    // a wire's card answers the agent's request itself.
    answer,
    // Where the card's prose measures its relative paths from — a plan names
    // the files it will touch, and those names are links off this checkout.
    // The board's own cards have none; their prose resolves to nothing.
    base,
  };
}

function repaintAsk(draft) {
  void (draft.repaint ?? paintBoardView)();
}

/* One question's answer as text — picked labels, then what was typed
 * (`answerFor`, :69265). Its emptiness is what the send button reads. */
function askAnswerFor(prompt, draft, qi) {
  const question = prompt.questions[qi];
  const picked = (draft.selections[qi] ?? [])
    .map((i) => question?.options[i]?.label ?? "")
    .filter((label) => label.length > 0);
  const other = (draft.other[qi] ?? "").trim();
  return [...picked, ...(other ? [other] : [])].join(", ");
}

/* Toggle one option (`pickOption`, :69289): multi-select keeps a sorted set,
 * single-select swaps — and picking the picked row unpicks it. */
function pickAskOption(draft, question, optionIndex) {
  const current = draft.selections[draft.index] ?? [];
  if (question.multi_select) {
    draft.selections[draft.index] = current.includes(optionIndex)
      ? current.filter((picked) => picked !== optionIndex)
      : [...current, optionIndex].sort((a, b) => a - b);
  } else {
    draft.selections[draft.index] = current.includes(optionIndex) ? [] : [optionIndex];
  }
}

/* The one button's three meanings (`confirm`, :69302): not on the last
 * question it advances; on the last it submits what was answered; and a
 * pointer pressing "skip" with nothing answered anywhere closes the panel —
 * but Enter does not, because a slip of the key would dismiss the question. */
function confirmAsk(card, draft, fromKeyboard = false) {
  const prompt = card.ask_prompt;
  if (draft.index < prompt.questions.length - 1) {
    draft.index += 1;
    repaintAsk(draft);
    return;
  }
  const answered = prompt.questions.some((unused, i) => askAnswerFor(prompt, draft, i).length > 0);
  if (answered) {
    submitAsk(card, draft);
  } else if (!fromKeyboard) {
    draft.open = false;
    repaintAsk(draft);
  }
}

/* Hand the picks to the backend, which walks the agent's own TUI with them
 * (`answer_ask` — the keys and their pacing live over there). The panel goes
 * quiet while the keys are being typed; the hook event that follows the
 * agent moving on clears the ask and takes the card out of the attention
 * column, draft and all. */
function submitAsk(card, draft) {
  const selections = card.ask_prompt.questions.map((unused, i) => ({
    indices: [...(draft.selections[i] ?? [])],
    other: (draft.other[i] ?? "").trim(),
  }));
  draft.sending = true;
  repaintAsk(draft);
  const term = Number(card.pane.slice(card.pane.indexOf(":") + 1));
  const road = draft.answer
    ? draft.answer(selections)
    : invoke("answer_ask", { term, agent: card.agent, prompt: card.ask_prompt, selections });
  road.catch((error) => {
    draft.sending = false;
    showError(String(error));
    repaintAsk(draft);
  });
}

/* The answer panel, unfolded from the amber box — Orca's ask card
 * (`NativeChatQuestionCard`, :69245), drawn at card scale: step chips when
 * there are several questions, the numbered option rows, and the free-text
 * row whose button is Send, Next or Skip depending on where the walk stands. */
function askPanelNode(card, draft) {
  const prompt = card.ask_prompt;
  const total = prompt.questions.length;
  const question = prompt.questions[draft.index];
  const panel = document.createElement("span");
  panel.className = "board-ask";

  if (total > 1) {
    const steps = document.createElement("span");
    steps.className = "board-ask-steps";
    prompt.questions.forEach((one, i) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "board-ask-step";
      chip.disabled = draft.sending;
      if (i === draft.index) chip.classList.add("is-here");
      chip.textContent = one.header || t("board.ask.step", "단계 {{num}}", { num: i + 1 });
      if (askAnswerFor(prompt, draft, i).length > 0) chip.classList.add("is-answered");
      chip.onclick = (event) => {
        event.stopPropagation();
        draft.index = i;
        repaintAsk(draft);
      };
      steps.appendChild(chip);
    });
    panel.appendChild(steps);
  }

  const title = document.createElement("span");
  title.className = "board-ask-question";
  title.textContent = question.question;
  title.dataset.tip = question.question;
  panel.appendChild(title);

  const rows = document.createElement("span");
  rows.className = "board-ask-rows";
  question.options.forEach((option, i) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "board-ask-option";
    row.disabled = draft.sending;
    const picked = (draft.selections[draft.index] ?? []).includes(i);
    if (picked) row.classList.add("is-picked");
    row.setAttribute("aria-pressed", picked ? "true" : "false");
    const badge = document.createElement("span");
    badge.className = "board-ask-badge";
    badge.textContent = picked ? "✓" : String(i + 1);
    const words = document.createElement("span");
    words.className = "board-ask-option-words";
    const label = document.createElement("span");
    label.className = "board-ask-option-label";
    label.textContent = option.label;
    label.dataset.tip = option.label;
    words.appendChild(label);
    if (option.description) {
      const description = document.createElement("span");
      description.className = "board-ask-option-description";
      description.textContent = option.description;
      description.dataset.tip = option.description;
      words.appendChild(description);
    }
    row.append(badge, words);
    row.onclick = (event) => {
      event.stopPropagation();
      pickAskOption(draft, question, i);
      repaintAsk(draft);
    };
    rows.appendChild(row);
  });

  const otherRow = document.createElement("span");
  otherRow.className = "board-ask-other";
  const field = document.createElement("input");
  field.type = "text";
  field.className = "board-ask-field";
  field.placeholder = t("board.ask.typeAnswer", "답을 입력");
  field.value = draft.other[draft.index] ?? "";
  field.disabled = draft.sending;
  field.oninput = () => {
    draft.other[draft.index] = field.value;
    // The button's word follows what is answered, without a repaint that
    // would knock the caret out of this very field.
    send.textContent = askSendWord(card, draft);
  };
  field.onfocus = () => {
    draft.focusOther = true;
  };
  field.onblur = () => {
    draft.focusOther = false;
  };
  field.onkeydown = (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      confirmAsk(card, draft, true);
    }
    event.stopPropagation();
  };
  field.onclick = (event) => event.stopPropagation();
  const send = document.createElement("button");
  send.type = "button";
  send.className = "board-ask-send";
  send.disabled = draft.sending;
  send.textContent = askSendWord(card, draft);
  if (askAnswerFor(prompt, draft, draft.index).length > 0) send.classList.add("is-live");
  send.onclick = (event) => {
    event.stopPropagation();
    confirmAsk(card, draft);
  };
  otherRow.append(field, send);
  rows.appendChild(otherRow);
  panel.appendChild(rows);

  if (total > 1) {
    const where = document.createElement("span");
    where.className = "board-ask-count";
    where.textContent = `${draft.index + 1}/${total}`;
    panel.appendChild(where);
  }

  // The repaint rebuilt the input; a person mid-word gets their caret back.
  if (draft.focusOther && !draft.sending) {
    queueMicrotask(() => {
      field.focus();
      field.setSelectionRange(field.value.length, field.value.length);
    });
  }
  return panel;
}

/* Send / Next / Skip / Sending…, exactly Orca's precedence (:69404). */
function askSendWord(card, draft) {
  if (draft.sending) return t("board.ask.sending", "전송 중…");
  const prompt = card.ask_prompt;
  if (askAnswerFor(prompt, draft, draft.index).length === 0) return t("board.ask.skip", "건너뛰기");
  const isLast = draft.index === prompt.questions.length - 1;
  return isLast ? t("board.ask.send", "답 보내기") : t("board.ask.next", "다음");
}

/* The approval panel — Orca's `NativeChatApprovalCard` (:69456) at card
 * scale: "Allow {tool}?" in a strong weight, the summary in mono under it,
 * and the two buttons — Allow filled, Deny bordered. One key each, sent at
 * once, and the panel dismisses in the same breath (`onChoose` →
 * `sendRaw`, :69583); the hook's next event takes the card out of the
 * column. Which byte means which is the backend's (`answer_approval`). */
function approvalPanelNode(card, draft) {
  const panel = document.createElement("span");
  panel.className = "board-approve";
  // The plan this permission asks approval FOR, when the CLI's plan tool is
  // what asked (`ApprovalPrompt.plan`, filled off the catalog in Rust): the
  // extension's plan review card — the plan itself instead of a summary
  // line, and a reason field on the refusal.
  const plan = typeof card.approval.plan === "string" ? card.approval.plan : "";
  const title = document.createElement("span");
  title.className = "board-approve-title";
  const glyph = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  glyph.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", "#i-shield");
  glyph.appendChild(use);
  const words = document.createElement("span");
  words.textContent = plan
    ? t("board.approve.planTitle", "계획을 승인할까요?")
    : t("board.approve.title", "{{tool}} 허용할까요?", { tool: card.approval.tool });
  title.append(glyph, words);
  panel.appendChild(title);
  if (plan) {
    // The plan reads as the answer's own prose — one painter, so a heading
    // and a numbered step look the same here as in the transcript.
    const well = document.createElement("div");
    well.className = "board-approve-plan helper-said";
    paintHelperProse(well, plan, draft.base ?? "");
    panel.appendChild(well);
  } else if (card.approval.summary) {
    const detail = document.createElement("span");
    detail.className = "board-approve-detail";
    detail.textContent = card.approval.summary;
    detail.dataset.tip = card.approval.summary;
    panel.appendChild(detail);
  }
  // The edit the tool asks to make, as the extension's card shows it —
  // the diff before the answer, cut by the backend from the request's own
  // input (`ApprovalPrompt.edits`).
  if (card.approval.edits?.length > 0) {
    panel.appendChild(toolDiffNode(card.approval.edits, card.approval.summary ?? ""));
  }
  // The reason a refusal carries, on a plan the wire asked about: the
  // extension's field under its plan review. Only where a refusal has
  // somewhere to put words — the pane's road says them in the composer
  // instead (`draft.instead`).
  const feedback = plan && draft.answer ? planFeedbackNode() : null;
  if (feedback) panel.appendChild(feedback);
  const acts = document.createElement("span");
  acts.className = "board-approve-acts";
  const term = Number(card.pane.slice(card.pane.indexOf(":") + 1));
  // The board's card answers the TUI by keystroke; a wire's card answers the
  // request itself (`draft.answer`, the option's id).
  const decide = draft.answer ?? ((allow) => invoke("answer_approval", { term, allow }));
  const act = (label, choice, kind, { then = null, says = null } = {}) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `board-approve-act ${kind}`;
    button.disabled = draft.sending;
    button.textContent = label;
    button.onclick = (event) => {
      event.stopPropagation();
      draft.sending = true;
      draft.open = false;
      decide(choice, says?.() || null).catch((error) => showError(String(error)));
      repaintAsk(draft);
      then?.();
    };
    return button;
  };
  const options = card.approval.options ?? [];
  if (plan) {
    // A plan is answered in its own two words, whatever the wire calls its
    // options: approve it, or send back what to change. The ids are the
    // wire's own — read by the kind it gave them, never spelled here.
    const allowing = options.find((one) => one.kind === "allow_once") ??
      options.find((one) => one.kind.startsWith("allow"));
    const denying = options.find((one) => one.kind.startsWith("reject"));
    acts.appendChild(act(t("board.approve.planAllow", "승인하고 진행"), allowing ? allowing.id : true, "is-allow"));
    if (feedback) {
      acts.appendChild(act(t("board.approve.planFeedback", "피드백 보내기"), denying ? denying.id : false, "is-deny",
        { says: () => feedback.value.trim() }));
    } else if (!draft.instead) {
      // A plan card with neither field nor composer still refuses.
      acts.appendChild(act(t("board.approve.deny", "거부"), denying ? denying.id : false, "is-deny"));
    }
  } else if (options.length > 0) {
    // The wire's own choices, in its order: the agent's words when it gave
    // them (ACP names its options), the window's word for a decision's kind.
    for (const option of options) {
      acts.appendChild(act(option.label || wireOptionWords(option.kind), option.id,
        option.kind.startsWith("allow") ? "is-allow" : "is-deny"));
    }
  } else {
    acts.append(
      act(t("board.approve.allow", "허용"), true, "is-allow"),
      act(t("board.approve.deny", "거부"), false, "is-deny"),
    );
  }
  // The extension's third row — "No, and tell Claude what to do instead":
  // the refusing key, then the composer takes the words. Only where a
  // composer stands (the conversation view hands the focus road in).
  if (draft.instead) {
    acts.appendChild(act(t("composer.instead", "대신 지시하기"), false, "is-instead", { then: draft.instead }));
  }
  panel.appendChild(acts);
  return panel;
}

/* The reason field under a plan the wire asked about — the extension's plan
 * review card writes its feedback into the refusal, which Claude Code shows
 * the model as the sentence it reads (`wire_answer`'s `message`). */
function planFeedbackNode() {
  const field = document.createElement("textarea");
  field.className = "board-approve-feedback";
  field.rows = 2;
  field.placeholder = t("board.approve.feedbackHint", "계획에서 고칠 점을 적어 주세요");
  field.setAttribute("aria-label", field.placeholder);
  return field;
}

/* The window's word for a wire decision's kind — the four ACP names a
 * permission option by, which Codex's three decisions map onto. */
function wireOptionWords(kind) {
  const words = {
    allow_once: t("wire.allowOnce", "허용"),
    allow_always: t("wire.allowAlways", "이 세션에서 항상 허용"),
    reject_once: t("wire.rejectOnce", "거부"),
    reject_always: t("wire.rejectAlways", "항상 거부"),
  };
  return words[kind] ?? kind;
}

/* Which tab holds a given shell, or `null`. The board's cards are per agent and
 * an agent lives in a leaf, but everything a card SAYS about it — the workspace,
 * the project, the name — hangs off the tab. */
function tabOfTerm(term) {
  return (
    tabs.find((tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(term)) ?? null
  );
}

function projectOfWorktree(path) {
  if (!path) return null;
  return projects.find((project) =>
    project.worktrees.some((worktree) => worktree.path === path)) ?? null;
}

function worktreeAt(path) {
  if (!path) return null;
  for (const project of projects) {
    const found = project.worktrees.find((worktree) => worktree.path === path);
    if (found) return found;
  }
  return null;
}

/* The project a checkout belongs to, listed or not.
 *
 * A worker whose worktree was removed after its work landed is history the
 * board still shows, and the catalog no longer lists that path — so filing by
 * `projectOfWorktree` alone put five finished second-wave workers under
 * 범위 밖 while the sidebar's project tree, drawn from live rows, showed none of
 * them (2026-09-15). The checkout's own location is the lineage: the window
 * keeps a project's worktrees side by side in one folder, so a removed
 * checkout beside a listed worktree of the project is that project's, and one
 * under the project's own path (an in-repo `.worktrees/`) is too. Listed
 * worktrees still answer first; nothing here invents a project for a path
 * no listed project touches. */
function projectHoldingCheckout(checkout) {
  const listed = projectOfWorktree(checkout);
  if (listed || !checkout) return listed;
  const folder = dirname(checkout);
  return projects.find((project) =>
    project.worktrees.some((worktree) => !worktree.is_main && dirname(worktree.path) === folder))
    ?? projects.find((project) => project.path && checkout.startsWith(`${project.path}/`))
    ?? null;
}

/* What a project path is CALLED. A card carries the path, because that is what
 * the backend filters on; a filter row and a chip carry the name, because that
 * is what somebody named it. The path is the honest fallback for a project the
 * catalog no longer lists — a chip that says nothing cannot be taken off. */
function projectNameAt(path) {
  return projects.find((project) => project.path === path)?.name ?? path;
}

/* One card per agent in a terminal.
 *
 * The state is the BACKEND's, not this window's `hookStates` map: the map only
 * holds what arrived while the window was listening, and a board opened an hour
 * into a session has to be right immediately. */
function cardsFromPanes(panes) {
  const drawn = panes.map((pane) => {
    const tab = tabOfTerm(pane.term);
    const worktree = worktreeAt(tab?.worktree);
    const project = projectOfWorktree(tab?.worktree);
    return {
      pane: `term:${pane.term}`,
      agent: pane.agent,
      state: pane.state,
      // And what the ORCHESTRATION LEDGER last said about this pane's worker.
      //
      // The state above is the agent SPEAKING, and it is only ever as good as
      // its last chance to speak: a process that stops has no last word, so a
      // `working` from before it stopped stands forever. The ledger is the one
      // authority that knows whether anybody is still waiting on this terminal
      // — it is what recorded the release nobody answered — and a board that
      // reads only the first of the two tells a person their work is
      // progressing when it ended (t-612).
      //
      // A passthrough and never a judgement: WHICH ledger words mean "over" is
      // `zerocode_core::board`'s rule, tested there beside the state words it
      // outranks. Empty is "no ledger row names this pane", which is every
      // agent a person started by hand.
      ledger: pane.ledger ?? "",
      // Orca's `conversationName ?? worktreeName`, and its rule that the
      // workspace moves to the footer once the heading is a conversation.
      //
      // The PANE's name first. A card is one pane, and Orca resolves a name
      // per leaf (`paneTitle ?? tabTitle`); now that a launch can land as a
      // pane of a tab that already had a name, reading the tab's would head
      // an agent's card with whatever the shell beside it is called.
      heading: tab ? paneTitleOf(tab, pane.term) || tabLabel(tab) : pane.agent,
      worktree: worktree ? worktreeDisplayName(worktree) : "",
      // The checkout's own path, which is what the review states are keyed by
      // (`boardReviews`). Not `worktree` above: that is the LABEL, which a
      // person renames; and not `project`, which every checkout in one
      // repository shares. Rust's `BoardCard` has no field for it, so it
      // travels beside the cards rather than through the grouping.
      checkout: tab?.worktree ?? "",
      project: project?.path ?? "",
      // Which MACHINE this agent runs on, for the map's host axis (1-g78c).
      // The one field this window already writes when a shell is opened over
      // SSH — `remoteHost`, which both roads set (an SSH terminal and a remote
      // workspace's terminal). A pane whose tab is not on the strip is local:
      // an unknown tab cannot have been opened over SSH by this window.
      // Read here rather than in the map, because this is where the tab is
      // already in hand — and a second `tabOfTerm` walk is a second answer.
      host: tab?.remoteHost ? "ssh" : "local",
      // And the machine's NAME, for the card's host badge — Orca's tooltip
      // says which one ("SSH host · {{host}}", DashboardHostBadge.tsx). Read
      // in the same walk as `host` for the same reason: the grouping
      // round-trip carries neither, and `places` ferries both past it.
      host_name: tab?.remoteHost ? String(tab.remoteHost) : "",
      task: "",
      // The conversation's two lines, already clamped by the backend — what
      // was asked, and what came back (Orca's lastUserMessage/
      // lastAgentMessage). The board is how somebody supervises five agents,
      // and a card that only says "working" makes them open every tab to
      // learn which one is doing what.
      you: pane.you ?? "",
      said: pane.said ?? "",
      ask: pane.ask ?? "",
      // The question's choices and the permission request, when the backend
      // parsed them. Spelled the wire's way because this card round-trips
      // through `board_columns`, whose `BoardCard` carries both as fields.
      ask_prompt: pane.ask_prompt ?? null,
      approval: pane.approval ?? null,
      // zo의 session_status가 말한 목표/루프 전문. `board_columns`의
      // BoardCard가 이 필드를 알아야 왕복 뒤에도 남는다.
      autonomy: paneAutonomyValue(pane.term, pane.autonomy ?? null),
      // Which pane started this one, in the ONE spelling a card id has: the
      // backend counts terminals, and a card looks for a parent by looking
      // for a card that calls itself this. `zerocode_core::agent_lineage`
      // turns the answers into the tree, and promotes a card whose parent is
      // not on the board rather than dropping it.
      parent: pane.parent == null ? "" : `term:${pane.parent}`,
      // Orca's rule (:26399): unseen while the state BEGAN after the pane
      // was last in front of the person — judged against when the state
      // started, never the last event, or a busy turn's every tool call
      // re-bolds a card the person already read (`acknowledgedAt <
      // stateStartedAt`, build-dashboard-snapshot.ts:237-239; P0-5). A pane
      // that never reported has a zero clock and can never be unseen —
      // there is nothing to have missed.
      unseen: (acknowledgedPanes.get(`term:${pane.term}`) ?? 0) < pane.state_started_at,
      // The sort key is the state's own clock too (Orca sorts on
      // `stateChangedAt`, AgentKanbanBoard.tsx:88) — sorting on the last
      // event reshuffled the working column under the cursor every time any
      // agent touched a tool. The footer's relative time keeps `at`.
      changed_at: pane.state_started_at,
      at: pane.at,
    };
  });
  // And a card per helper running inside one of them, hung off its parent by
  // the ONE thing that makes a card a child here: a `parent` that spells an id
  // some other card calls itself. `zerocode_core::agent_lineage` does the rest,
  // so the board's tree, its fold and its child counts all include helpers
  // without a line of tree code being written twice.
  //
  // The id carries the pane it belongs to because a helper has no pane of its
  // own: clicking the card opens its PARENT's screen, which is where the work
  // it is doing is actually being written.
  for (const pane of panes) {
    const rows = paneSubagents.get(pane.term);
    if (!rows) continue;
    const parent = drawn.find((card) => card.pane === `term:${pane.term}`);
    for (const sub of rows) {
      drawn.push({
        ...parent,
        pane: `sub:${pane.term}:${sub.id}`,
        parent: `term:${pane.term}`,
        // The helper's own state: the backend keeps a finished helper's row
        // (greyed) rather than dropping it. The parent's `ledger` rides
        // along in the spread above and is the one thing that may contradict
        // a running one: a helper cannot outlive the terminal it runs inside,
        // so a pane whose worker the ledger retired has no working helpers.
        state: sub.state === "done" ? "done" : "working",
        heading: sub.name,
        task: "",
        // The parent's conversation is the parent's. A helper repeating the
        // coordinator's last two lines under it would read as five agents all
        // having said the same thing.
        you: "",
        said: "",
        ask: "",
        ask_prompt: null,
        approval: null,
        // 도우미는 부모의 판에서 돌지만 부모의 자율 정책을 수행하는 주체는
        // 아니다. spread로 빌린 부모 사실을 여기서 명시적으로 걷는다.
        autonomy: null,
        unseen: false,
        // No stamp: a helper's card is a fact about right now, and borrowing
        // the parent's would age it by the parent's clock.
        changed_at: 0,
        at: 0,
      });
    }
  }
  return drawn;
}

/* And one per supervised lane.
 *
 * Both sources are on the same board because they are the same thing to the
 * person looking — an agent doing work. They spell their states differently
 * (`streaming` against `working`), which is exactly why the mapping that decides
 * columns reads both vocabularies rather than one. */
function cardsFromLanes() {
  return [...lanes.values()].map(({ lane, changedAt }) => {
    const worktree = worktreeAt(lane.worktree_id);
    const project = projectOfWorktree(lane.worktree_id);
    return {
      pane: `lane:${lane.id}`,
      agent: lane.agent,
      state: lane.state,
      heading: lane.title?.trim() || (worktree ? worktreeDisplayName(worktree) : lane.agent),
      worktree: worktree ? worktreeDisplayName(worktree) : "",
      checkout: lane.worktree_id ?? "",
      project: project?.path ?? "",
      // A supervised lane runs in a checkout the orchestrator manages, and
      // this window opens none of those over SSH — there is no remote lane to
      // be wrong about (1-g78c).
      host: "local",
      host_name: "",
      task: lane.title ?? "",
      // A lane is opened by a person or by a workspace becoming active, and
      // nothing this window can see makes one lane the parent of another. An
      // empty string is the honest answer, not a placeholder: it is what a
      // root says.
      parent: "",
      unseen: false,
      // The ingestion stamp, never the render clock: a card stamped at render
      // says "방금" about everything forever. `?? 0` keeps a lane the stamp
      // never reached honest — zero paints no age at all.
      changed_at: changedAt ?? 0,
      at: changedAt ?? 0,
    };
  });
}

/* And one per worker the ORCHESTRATION LEDGER still holds.
 *
 * The third source, and the only one that is not a question about this window.
 * The two above both ask "what am I holding" — a pane needs a terminal this
 * window opened, a lane needs a lane this window supervises — so a worker
 * whose seat this window does not have was on no surface at all: a run whose
 * coordinator restarted, a pane that closed under a worker still carrying a
 * dispatch, a summons whose terminal never came up. `worker-list` printed
 * every one of them the whole time. The rule the person set is that a
 * coordinator must never lose an agent it summoned, and one source that is not
 * about this window's furniture is what that costs.
 *
 * It hangs off the SAME ontology as the other two rather than adding a layer:
 * a worker row carries the checkout its pane sat in, and `worktreeAt` climbs
 * from a checkout to the workspace and its project exactly as a tab's does.
 *
 * A checkout no project in the catalog lists gets no box of its own. The
 * board's project filter is opt-in — empty means every project, never none —
 * so such a card is already kept, and its own path names it better than a
 * bucket called "elsewhere" would. What it loses is the repo chip, which is
 * honest: this window does not know that repository.
 */
function boardCardPlace(checkout) {
  const worktree = worktreeAt(checkout);
  const project = projectHoldingCheckout(checkout);
  return {
    worktree: worktree
      ? worktreeDisplayName(worktree)
      : checkout ? basename(checkout) : "",
    checkout,
    project: project?.path ?? "",
  };
}

function boardLedgerIdentity(row) {
  return {
    task_id: row.task_id ?? "",
    worker_id: row.worker ?? "",
    dispatch_id: row.dispatch_id ?? "",
    dispatch_started_ms: Number(row.dispatch_started_ms) || 0,
    reported: typeof row.reported === "boolean" ? row.reported : null,
    retry_of: row.retry_of ?? null,
  };
}

function cardsFromLedger(rows, drawn) {
  const cards = [];
  for (const row of rows) {
    /* A worker this window already drew as a pane is one agent, not two. The
     * merge lives here because this is the only half that knows which panes
     * actually became cards — the backend can report a seat it holds while
     * the pane behind it has already left `agent_terms`.
     *
     * "Drawn" does not mean "complete", though. A live pane whose mounting
     * event was missed has no tab, so `cardsFromPanes` cannot name its
     * checkout. The ledger row is the other half of that SAME card and owns
     * the missing location. Complete it before deduplicating; otherwise the
     * graph files a worker under 범위 밖 while the sidebar files it under its
     * real workspace. */
    const held = row.term == null ? null : drawn.get(`term:${row.term}`);
    if (held) {
      if (!held.checkout && row.checkout) Object.assign(held, boardCardPlace(row.checkout));
      if (!held.ledger && row.ledger) held.ledger = row.ledger;
      if (!held.run && row.run) held.run = row.run;
      if (!held.task && row.task) held.task = row.task;
      // These are ledger identities, not the human-readable task sentence.
      // Keep them beside the card through the Rust grouping round trip.
      Object.assign(held, boardLedgerIdentity(row));
      continue;
    }
    // Its own id, spelled the way every other card's is: the kind, then the
    // thing. `worker:` is also the fact that this card has no screen in this
    // window, which is what `openBoardCard` reads it for.
    const pane = `worker:${row.worker}`;
    cards.push({
      pane,
      agent: row.agent,
      // The coordinator's side of the story, which for a seatless worker is
      // the only side there is: no hook ever fired for a pane this window
      // does not have. Chosen in Rust beside the states it reads.
      state: row.state,
      // And the ledger's verdict on the terminal, in the field a pane's card
      // already carries it in. `zerocode_core::board` lets it take a claim of
      // work away and never add one, so a row that says it is running over a
      // terminal nobody can find lands in the column endings are read in.
      ledger: row.ledger,
      // A run is a control scope, not a sixth agent state. It travels beside
      // the grouped card and is counted by the graph model, never classified
      // by `zerocode_core::board`.
      run: row.run ?? "",
      ...boardLedgerIdentity(row),
      // What it was told to do, in the coordinator's words — the one line
      // that makes a lost worker identifiable. The id stands in when the
      // dispatch is gone, because a card with no name cannot be looked for.
      heading: row.task || row.worker,
      ...boardCardPlace(row.checkout),
      // Nothing here runs over SSH: a federated worker's pane belongs to the
      // window on the far side, and this row is the ledger's, not a tab's.
      host: "local",
      host_name: "",
      // The heading already says it; a second copy would print it twice.
      task: "",
      // A worker is summoned BY a coordinator, and the ledger knows which —
      // but the coordinator's own card is a pane and this is not its child in
      // the sense the tree means (`parent` spells a card id, and a run is not
      // a card). An empty string is what a root says.
      parent: "",
      // Never seen, until somebody looks. This is the whole point: a lost
      // worker's card lands in Done, and `bucket_at` settles an ACKNOWLEDGED
      // done into the hidden idle column — so without this the board would
      // draw the card and hide it in the same beat.
      unseen: (acknowledgedPanes.get(pane) ?? 0) < row.at,
      changed_at: row.at,
      at: row.at,
    });
  }
  // An internal helper has no separate checkout report. Its parent's ledger
  // may have supplied the location after cardsFromPanes copied the helper.
  for (const card of drawn.values()) {
    if (!card.pane.startsWith("sub:") || card.checkout) continue;
    const parent = drawn.get(card.parent);
    if (parent?.checkout) Object.assign(card, boardCardPlace(parent.checkout));
  }
  return cards;
}

async function boardCards() {
  let panes = [];
  let ledger = [];
  try {
    // Asked together: they are two halves of one paint, and asking them in
    // series puts a whole round trip between the cards that describe the same
    // agent.
    [panes, ledger] = await Promise.all([invoke("pane_agents"), readLedgerAgents()]);
  } catch (error) {
    showError(String(error));
  }
  const drawn = cardsFromPanes(panes);
  return [
    ...drawn,
    ...cardsFromLedger(ledger, new Map(drawn.map((card) => [card.pane, card]))),
    ...cardsFromLanes(),
  ];
}

/* ---- the review a card's checkout is on (1-g78a) ---------------------------
 *
 * Orca's card wears a ReviewPill, filled from a per-worktree GitHub PR cache
 * its snapshot builder keeps (`hostedReviewInfoFromGitHubPRInfo`). Here the
 * cache is Rust's — `github_review_states` answers a list keyed by checkout
 * path and re-asks `gh` at most once a minute per checkout — so this side
 * holds only the last answer and the fact that one is on its way. */
let boardReviews = new Map();

/* Whether an answer is already coming. One question at a time is the whole
 * generation guard this needs: the board repaints on every hook event, and
 * without it a busy minute puts a dozen identical questions in flight and the
 * one that lands LAST decides what the cards wear. */
let boardReviewsAsking = false;

/* Do these two answers say the same thing?
 *
 * The repaint at the end of an answer is what makes the pill appear, so an
 * answer that changed nothing must not cause one — a repaint asks again, and
 * asking again would repaint again. */
function sameBoardReviews(held, next) {
  if (held.size !== next.size) return false;
  for (const [where, review] of next) {
    const had = held.get(where);
    if (!had || had.number !== review.number || had.state !== review.state) return false;
  }
  return true;
}

async function askBoardReviews(checkouts) {
  if (boardReviewsAsking || checkouts.length === 0) return;
  boardReviewsAsking = true;
  let rows = [];
  try {
    rows = await invoke("github_review_states", { worktrees: checkouts });
  } catch {
    // `gh`가 없거나 로그인하지 않은 기계에서 카드가 리뷰를 두르지 않을 뿐이다.
    // 오류 상자는 사람이 청한 적 없는 그림 뒤의 물음에 붙일 무게가 아니다.
  } finally {
    boardReviewsAsking = false;
  }
  const next = new Map(rows.map((row) => [row.worktree, row]));
  if (sameBoardReviews(boardReviews, next)) return;
  boardReviews = next;
  scheduleWorkspaceBoardPaint();
  if (!workspaceBoardOpen) void paintBoardView();
}

/* The badge on the door: how many agents are waiting on a person.
 *
 * Counted from the same rules as the board's own Needs You column, by asking for
 * that column — so the number on the closed door and the length of the list
 * behind it cannot disagree. */
function applyBoardBadge(answer) {
  // 팝아웃에는 닫힌 문이 없다 — 그 창은 통째로 보드이고, 독 배지는 메인
  // 창의 것이다. 두 창이 같은 숫자를 각자 계산해 각자 쓰면 셋 중 하나는
  // 늦게 도착한 답이 되고, 그것이 곧 보드와 배지가 어긋나는 방식이다.
  if (isPopout) return;
  const badge = el("board-badge");
  if (!badge) return;
  const stuck = answer.attention_count ?? 0;
  updateActionHub(answer);
  if (stuck === boardStuck) return;
  boardStuck = stuck;
  badge.hidden = stuck === 0;
  badge.textContent = String(stuck);
  // The dock wears the SAME number — this one, from the attention column
  // Rust just grouped. Never its own count: two counts is how the dock says
  // 3 while the board says 2.
  void invoke("set_dock_badge", { count: stuck }).catch(() => {});
}

let attentionCards = [];

function updateActionHub(answer) {
  if (isPopout) return;
  const attCol = answer.columns?.find((c) => c.bucket === "attention");
  attentionCards = attCol?.cards ?? [];
  const stuck = answer.attention_count ?? attentionCards.length;

  const navAttention = el("nav-attention");
  const attBadge = el("attention-badge");
  const hubBadge = el("action-hub-badge");

  if (navAttention) navAttention.hidden = (stuck === 0);
  if (attBadge) attBadge.textContent = String(stuck);
  if (hubBadge) hubBadge.textContent = String(stuck);

  const scrim = el("action-hub-scrim");
  if (scrim && !scrim.hidden) {
    paintActionHubModal();
  }
}

function openActionHub() {
  const scrim = el("action-hub-scrim");
  if (!scrim) return;
  paintActionHubModal();
  showModal(scrim);
}

function closeActionHub() {
  const scrim = el("action-hub-scrim");
  if (scrim) hideModal(scrim);
}

function paintActionHubModal() {
  const list = el("action-hub-list");
  const empty = el("action-hub-empty");
  const badge = el("action-hub-badge");
  if (!list || !empty) return;

  const count = attentionCards.length;
  if (badge) badge.textContent = String(count);
  empty.hidden = count > 0;
  list.replaceChildren();

  for (const card of attentionCards) {
    const item = document.createElement("div");
    item.className = "action-hub-item";

    const isQuestion = Boolean(card.ask);
    const stateLabel = isQuestion ? t("actionHub.stateAsk", "질문 대기") : t("actionHub.stateGated", "권한 승인 대기");
    const preview = card.ask || card.said || card.heading || card.task || t("actionHub.noContent", "대기 중인 내용 없음");

    const head = document.createElement("div");
    head.className = "action-hub-item-head";
    const left = document.createElement("div");
    left.className = "action-hub-item-left";

    const agentTag = document.createElement("span");
    agentTag.className = "action-hub-agent-tag";
    agentTag.textContent = card.agent || "Agent";

    const stateTag = document.createElement("span");
    stateTag.className = "action-hub-state-tag";
    stateTag.textContent = stateLabel;

    const titleSpan = document.createElement("span");
    titleSpan.className = "action-hub-item-title";
    titleSpan.textContent = card.worktree || card.project || "";

    left.append(agentTag, stateTag, titleSpan);
    head.appendChild(left);

    const previewP = document.createElement("p");
    previewP.className = "action-hub-preview";
    previewP.textContent = preview;

    const acts = document.createElement("div");
    acts.className = "action-hub-actions";
    const jumpBtn = document.createElement("button");
    jumpBtn.className = "btn btn--primary btn--sm action-hub-jump-btn";
    jumpBtn.type = "button";
    jumpBtn.textContent = t("actionHub.jump", "즉시 이동");
    jumpBtn.addEventListener("click", () => {
      closeActionHub();
      openBoardCard(card, "attention");
    });
    acts.appendChild(jumpBtn);

    item.append(head, previewP, acts);
    list.appendChild(item);
  }
}

window.openActionHub = openActionHub;
window.closeActionHub = closeActionHub;

el("nav-attention")?.addEventListener("click", openActionHub);
el("action-hub-close")?.addEventListener("click", closeActionHub);
el("action-hub-dismiss")?.addEventListener("click", closeActionHub);

window.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.shiftKey && (event.key === "a" || event.key === "A")) {
    event.preventDefault();
    openActionHub();
  }
});

async function refreshBoardBadge() {
  try {
    const cards = await boardCards();
    const answer = await invoke("board_snapshot", {
      cards,
      query: agentGraphSnapshotQuery(),
    });
    applyBoardBadge(answer);
  } catch {
    // A badge is a best-effort interruption hint. The graph itself owns the
    // recoverable error surface when the same snapshot cannot be built.
  }
}

/* The OS said no to notifications. Once per session, because a denied
 * permission is one fact — and said in words, because a ring that silently
 * never rings is an agent waiting on a person who was never told. */
listen("notify:blocked", () => {
  showError(t("notify.blocked", "시스템 알림이 차단되어 있습니다 — 에이전트가 기다려도 울리지 않습니다. 시스템 설정 > 알림에서 허용하세요."));
});

function openBoard() {
  // Opening is a fresh supervision decision: attention first, then the newest
  // live agent. A project/workspace inspected on the previous visit must not
  // suppress that default when the control room is reopened later.
  agentGraphSelectedKey = null;
  workbenchScopes.tasks = null;
  openTab({ id: "board", kind: "board" });
  void askForTour("board");
}

/* 다른 표면(아티팩트 서랍·지식 그래프)에서 한 판의 카드로 — 보드를 열고, 그리고,
 * 그 카드를 고른다. 보드는 열리면서 제 기본 선택을 고르므로(`openBoard`) 고르는
 * 손은 첫 그림 **뒤에** 온다. 판이 카드를 들고 있지 않으면(끝난 워커) 보드만
 * 선다 — 답은 골랐는가다. 카드의 열쇠는 `agent:term:<n>`(`cardsFromPanes`). */
async function revealTaskBoardPane(term) {
  leavePagesForStage();
  openBoard();
  const tab = boardTab();
  if (!tab) return false;
  await paintBoardView(tab, { force: true });
  const view = docHost(tab.pane, "board");
  const key = `agent:term:${term}`;
  selectAgentGraphEntity(view, key, { focus: true });
  return agentGraphSelectedKey === key;
}

/* ⌘K(맥) / Ctrl+K가 보드 검색으로 손을 옮긴다 — 실측 AgentKanbanBoard:348.
 * 글자를 받는 자리(입력·편집 영역·터미널) 위에서는 비켜선다: 터미널의 ⌘K는
 * 화면 지우기라는 선약이 있다. 보드가 보이지 않으면 아무 일도 없다 — 이
 * 단축키는 대시보드 창의 것이지 창 전체의 것이 아니다. */
document.addEventListener("keydown", (event) => {
  const mac = navigator.userAgent.includes("Mac");
  if (!(mac ? event.metaKey : event.ctrlKey) || event.key.toLowerCase() !== "k") return;
  if (event.target instanceof Element
    && event.target.closest('input, textarea, [contenteditable="true"], .term')) return;
  const query = document.querySelector("#board-view:not([hidden]) .board-query, "
    + '[data-surface="popout"] .board-query');
  if (!query) return;
  event.preventDefault();
  query.focus();
});

function visibleAgentGraphView() {
  return [...document.querySelectorAll("#board-view, .file-view")]
    .find((view) => !view.hidden && view.querySelector(".agent-graph-layout")?.hidden === false)
    ?? null;
}

document.addEventListener("keydown", (event) => {
  const view = visibleAgentGraphView();
  if (!view || agentBoardMode === "tasks") return;
  if (event.key === "Escape" && agentGraphFollowing) {
    agentGraphPauseFollow(view);
    return;
  }
  if (event.code === "Space" && !(event.target instanceof Element &&
      event.target.closest("input, textarea, [contenteditable='true']"))) {
    agentGraphSpaceHeld = true;
    event.preventDefault();
    return;
  }
  if (!event.shiftKey || !["1", "2"].includes(event.key)) return;
  event.preventDefault();
  agentGraphShortcut(view, event.key);
});

document.addEventListener("keyup", (event) => {
  if (event.code === "Space") agentGraphSpaceHeld = false;
});
window.addEventListener("blur", () => { agentGraphSpaceHeld = false; });

function bucketWord(bucket) {
  if (bucket === "attention") return t("board.attention", "확인 필요");
  if (bucket === "working") return t("board.working", "작업 중");
  if (bucket === "done") return t("board.done", "완료");
  if (bucket === "paused") return t("board.autonomy.paused", "일시 정지");
  return t("board.idle", "대기 중");
}

/* One answer to "is a turn in flight?" across the two state vocabularies.
 * Hook-backed panes say `working`; supervised lanes say `streaming`. The
 * board mark and the durable pane snapshot both read this function so adding
 * a third live word cannot make the picture and restart record disagree. */
function isMidTurn(state) {
  return state === "working" || state === "streaming";
}

/* The word a review wears, from the one Rust sends (`gh::review_word`).
 *
 * Shaped like `bucketWord` above because it is the same kind of thing: the
 * backend decides which of four states this is, and this table is the only
 * place that turns that word into a person's. An unknown word reads as open,
 * exactly as Rust's own table does. */
function reviewWord(state) {
  if (state === "draft") return t("board.review.draft", "초안 리뷰");
  if (state === "merged") return t("board.review.merged", "병합된 리뷰");
  if (state === "closed") return t("board.review.closed", "닫힌 리뷰");
  return t("board.review.open", "열린 리뷰");
}

/* The pill a card wears when its checkout is on a review — Orca's ReviewPill.
 *
 * One glyph for all four states, because this window already decided that a
 * pull request is `#i-pr` and its STATE is a colour (`.wt-gh-mark`): open is
 * the living tone, a draft is muted, merged is the tone `.gl-item-state`
 * chose for it, and closed is the halt. A second glyph vocabulary here would
 * be a second answer to a question the composer's row already answered. */
function reviewPillNode(review) {
  const pill = document.createElement("span");
  pill.className = `wt-gh-pill board-card-review is-${review.state}`;
  const mark = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  mark.setAttribute("class", "icon");
  mark.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", "#i-pr");
  mark.appendChild(use);
  const words = document.createElement("span");
  words.className = "board-card-review-words";
  words.textContent = reviewWord(review.state);
  pill.append(mark, words);
  return pill;
}

/* A card's own relative time, in Orca's steps: `just now` under a minute, then
 * minutes, hours, days (`formatStartedAgo`, :86-94). */
function agoWord(at, now) {
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  if (seconds < 60) return t("board.justNow", "방금");
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t("board.minutes", "{{count}}분", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("board.hours", "{{count}}시간", { count: hours });
  return t("board.days", "{{count}}일", { count: Math.floor(hours / 24) });
}

/* 그리고 같은 걸음의 두 번째 낱말 — **얼마나 오래 돌고 있는가** (1-t1153).
 *
 * `agoWord`와 계단이 같은 이유는 두 낱말이 같은 자리에 서기 때문이다: 한
 * 카드에 한 시계이므로, 두 답의 눈금이 다르면 카드가 바뀔 때마다 사람이
 * 단위를 다시 읽어야 한다. 다른 것은 낱말뿐이고, 그 다름이 여기서 하는 일의
 * 전부다 — 같은 숫자를 다른 뜻으로 적으면서 아무 표시도 하지 않는 것이 지금의
 * 거짓말이다.
 *
 * 1분 아래는 낱말이 없다(`null`). `at`은 이 상태 안에서 마지막으로 온 소식이라
 * 언제나 `changed_at` 뒤에 있으므로, 시작한 지 1분이 안 된 카드는 두 질문의 답이
 * **둘 다** 「방금」이다. 거기서 「막 시작」이라 적는 것은 아무도 재지 않은 구별을
 * 지어내는 것이고, 그 자리에서는 옛 낱말이 이미 정확하다. */
function runningWord(from, now) {
  const minutes = Math.floor(Math.max(0, now - from) / 60_000);
  if (minutes < 1) return null;
  if (minutes < 60) return t("board.running.minutes", "{{count}}분째", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("board.running.hours", "{{count}}시간째", { count: hours });
  return t("board.running.days", "{{count}}일째", { count: Math.floor(hours / 24) });
}

/* 카드의 시계가 답하는 질문 (1-t1153).
 *
 * `at`은 이 판이 **마지막으로 말한** 때다. 도는 판에게 그것은 방금이다 — 도구
 * 이벤트마다 밀리므로(백엔드 `PaneState.at`: "a busy turn's dozens of tool
 * events push `at`"), 한 시간을 갈아 온 판도 한 마디 하고 조용해진 판도 똑같이
 * 「방금」이라 적혔다. 원장만 아는 워커의 카드에서는 같은 결함이 반대로 나온다:
 * 거기서 `at`은 소환 시각(`Worker::started_ms`)이라 아예 움직이지 않으므로,
 * 50분째 갈고 있는 워커와 뜨자마자 죽은 판이 같은 「50분」을 적었다.
 *
 * `changed_at`은 이 상태에 들어선 때다 — 백엔드의 `state_started_at`이고, 그
 * 문서가 스스로 "answers a different question than `at`"이라 적어 둔 그 필드다.
 * 도는 카드에게 그 답은 곧 **이 턴이 시작된 때**이므로, 도는 카드만 그쪽을 읽고
 * 읽었다는 사실을 낱말로 말한다.
 *
 * 판정은 상태 낱말이 아니라 BUCKET이다. 멈춘 프로세스는 마지막 `working`을
 * 영원히 들고 있고 그것을 걷어내는 규칙은 원장을 읽는 백엔드의 것이므로
 * (`zerocode_core::board`, t-612), 여기서 다시 판정하면 그 규칙의 두 번째 사본이
 * 된다 — 그리고 먼저 도는 사본이 된다. 이 창의 다른 자리도 같은 이유로 같은
 * 문을 쓴다(그래프 노드의 `live`).
 *
 * 데이터가 얇으면 아무 말도 하지 않는다: 소식이 없는 카드(`at <= 0`)는 지금처럼
 * 시계가 없고, `state_started_at`이 0인 카드는 셀 시작이 없으므로 옛 낱말로
 * 남는다 — 없는 시작을 render 시각으로 메우면 모든 카드가 영원히 「방금」이다. */
function cardClock(card, bucket, now) {
  if (!(card.at > 0)) return null;
  // A state cannot have started after its latest report. Rejecting that shape
  // also keeps corrupt or future start data from drawing two contradictory
  // facts on one card.
  const running = bucket === "working" && card.changed_at > 0 &&
      card.changed_at <= now && card.changed_at <= card.at
    ? runningWord(card.changed_at, now)
    : null;
  return { word: agoWord(card.at, now), running };
}

/* The mark a state wears on a card — Orca's AgentStateDot vocabulary, not one
 * dot for every state: 'working' renders a spinner, 'done' a check icon "so
 * completion is visually distinct from 'idle' (grey dot)", the attention
 * family the question glyph in the wait colour, and everything quiet the dot
 * (AgentStateDot.tsx — its own why, quoted). The spinner is the sidebar's
 * measured ring (`animate-spin` at `1s steps(12)`), because the two surfaces
 * already agreed to share one state vocabulary. A settled card — done and
 * seen — has already dropped to idle's dot by the display rule below. */
function boardStateMark(state, settled) {
  const mark = document.createElement("span");
  mark.className = "board-card-state";
  mark.setAttribute("aria-hidden", "true");
  if (isMidTurn(state)) {
    mark.classList.add("is-spin");
  } else if ((state === "done" || state === "exited") && !settled) {
    mark.classList.add("is-check");
    mark.innerHTML = icon("circle-check");
  } else if (
    state === "needs-attention" || state === "waiting" || state === "blocked" ||
    state === "awaiting_permission"
  ) {
    mark.classList.add("is-ask");
    mark.innerHTML = icon("help");
  } else {
    mark.classList.add("is-dot");
  }
  return mark;
}

function boardCardNode(card, now, bucket, review, place, { preview = true } = {}) {
  // The question box exists only in the attention column — Orca computes
  // `askSummary` from the bucket, not the card alone (:26400). The round-two
  // hierarchy keeps the state mark in its dedicated status row as well: the
  // box says what needs answering, while the mark says which state owns it.
  const asking = bucket === "attention" && Boolean(card.ask || card.ask_prompt || card.approval);
  // A card whose question came with choices — or whose stop is a permission
  // request — can answer it right here. A card holding buttons cannot itself
  // BE a button, so it becomes a plain node that still opens the tab when
  // the click lands outside the panel.
  const answerable = asking && (card.ask_prompt != null || card.approval != null);
  const boxed = answerable;
  const node = document.createElement(boxed ? "div" : "button");
  if (boxed) {
    node.setAttribute("role", "button");
    node.tabIndex = 0;
  } else {
    node.type = "button";
  }
  node.className = `board-card is-${card.state}`;
  if (card.lineage) {
    node.style.setProperty("--card-depth", String(card.lineage.depth));
  }
  // A finished card settles once somebody has seen it — Orca's
  // `dashboardCardDisplayState`: "Completed agents stay green until
  // acknowledged, then settle into gray idle" (dashboard-snapshot.ts:39-43),
  // and the emerald dress is keyed to that display state, not the raw one
  // (`isDone = displayState === 'done'`, AgentKanbanCard.tsx). The COLUMN
  // moves with it: the backend's bucket reads the same display state
  // (`bucket_at`), so a settled card re-buckets as idle and sails when idle
  // is hidden — Orca's bucket takes the demoted reading too
  // (build-dashboard-snapshot.ts:236-240).
  const settled = (card.state === "done" || card.state === "exited") && !card.unseen;
  if (settled) node.classList.add("is-settled");
  // The measured unseen dress (:96425): the heading is muted and ordinary
  // until something happened that nobody has looked at — then it turns
  // semibold and full-ink. A dress, not a badge: the card already has a dot
  // for its state, and this is about the PERSON, not the agent.
  if (card.unseen) node.classList.add("is-unseen");
  node.dataset.pane = card.pane;
  /* 그리고 이 카드가 말하는 일곱 — 그래프 노드가 그리는 바로 그 묶음 (t-2374).
   *
   * 같은 조립기에서 나오므로 인스펙터의 카드와 그래프의 노드가 같은 낱말과
   * 같은 글리프를 쓴다. 두 함수가 각자 지으면 하나만 고쳐지는 날이 오고, 그날
   * 한 화면의 두 그림이 같은 에이전트를 다르게 말한다(설계 §6). */
  const facts = agentCardFacts(card, bucket, now);
  const head = document.createElement("span");
  head.className = "board-card-head";
  // The agent's real mark, through the one path that draws them — a letter tile
  // now, the vendor's mark when the backend has it. A card built with its own
  // `<img src>` would be the remote-image mistake this window already made once.
  const mark = agentIcon(agentRows.find((row) => row.id === card.agent) ?? { id: card.agent });
  mark.classList.add("board-card-mark");
  const heading = document.createElement("span");
  heading.className = "board-card-heading";
  heading.textContent = facts.identity;
  head.append(mark, heading);
  node.appendChild(head);

  /* 상태 마크 · 낱말 · 단계. 모델과 시계는 활동 아래 발치로 내려간다. */
  const status = document.createElement("span");
  status.className = "board-card-status";
  const stateWord = document.createElement("span");
  stateWord.className = "agent-graph-state-word";
  stateWord.textContent = facts.stateWord;
  const phase = document.createElement("span");
  phase.className = `agent-card-phase is-${facts.phase || "none"}`;
  phase.textContent = facts.phaseWord;
  phase.dataset.tip = facts.phaseTip;
  // The state's own shape stays in its row even while the ask box stands
  // (round two, t-2466): the row says which state owns the card, the box
  // below says what needs answering, and a row that loses its mark on one
  // state misaligns against every other card's.
  status.append(boardStateMark(card.state, settled), stateWord, phase);
  node.appendChild(status);

  const doing = agentCardDoingNode();
  // 이 카드는 질문을 제 호박색 상자로 이미 말한다 — 활동 줄까지 그것을 적으면
  // 한 카드가 같은 문장을 두 번 쓴다(실측: 인스펙터 카드에 질문 두 줄).
  dressAgentCardDoing(doing, facts, { askInline: false });
  node.appendChild(doing);

  // One recent utterance only. A card is a glance, not a transcript, and an
  // old tool failure may headline only the states that currently mean trouble.
  const recent = agentGraphMessage(card, agentGraphState({ card, bucket }));
  const fromAgent = recent !== "" && recent === agentGraphCleanText(card.said);
  if (recent) {
    if (!fromAgent) {
      const you = document.createElement("span");
      you.className = "board-card-you";
      const who = document.createElement("span");
      who.className = "board-card-who";
      who.textContent = t("board.you", "나");
      you.append(who, ` ${recent}`);
      node.appendChild(you);
    }
    if (fromAgent) {
      const said = document.createElement("span");
      said.className = "board-card-said";
      const who = document.createElement("span");
      who.className = "board-card-who";
      who.textContent = agentRows.find((row) => row.id === card.agent)?.name ?? card.agent;
      said.append(who, ` ${recent}`);
      node.appendChild(said);
    }
  } else if (card.task) {
    const task = document.createElement("span");
    task.className = "board-card-task";
    task.textContent = card.task;
    node.appendChild(task);
  }
  if (card.lineage && card.lineage.child_count > 0) {
    const kids = document.createElement("span");
    kids.className = "board-card-kids";
    const words = document.createElement("span");
    words.className = "board-card-kids-words";
    words.textContent = agentGraphCountWord("agent", card.lineage.child_count);
    kids.appendChild(words);
    node.appendChild(kids);
  }

  // The amber question box, after the conversation lines and before the
  // footer — Orca's own order (:96444, drawn after the message block). When
  // the question came with its choices, the box is a door: it unfolds into
  // the answer panel, and folds back when the person changes their mind.
  if (asking) {
    const ask = document.createElement(answerable ? "button" : "span");
    ask.className = "board-card-ask";
    if (answerable) ask.type = "button";
    const glyph = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    glyph.setAttribute("aria-hidden", "true");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-help");
    glyph.appendChild(use);
    const words = document.createElement("span");
    words.className = "board-card-ask-words";
    words.textContent = card.ask || card.ask_prompt?.questions?.[0]?.question || card.approval?.summary || "";
    ask.append(glyph, words);
    node.appendChild(ask);
    if (answerable) {
      ask.classList.add("is-openable");
      const draft = askDraftFor(card);
      ask.setAttribute("aria-expanded", draft.open ? "true" : "false");
      ask.onclick = (event) => {
        event.stopPropagation();
        draft.open = !draft.open;
        void paintBoardView();
      };
      // The question first, the approval second — `parseInteractivePrompt`'s
      // own order (:69232): a payload that parses as questions IS a question.
      if (draft.open) {
        node.appendChild(
          card.ask_prompt ? askPanelNode(card, draft) : approvalPanelNode(card, draft),
        );
      }
    }
  }

  // 그리고 이 에이전트가 지금 찍고 있는 것 — 화면 꼬리 몇 줄.
  //
  // 이 창의 실인 하나가 여기서 닫힌다: 무대는 한 번에 한 탭의 판만 드러내므로
  // "읽고 있는 셸"은 에이전트가 여덟이어도 언제나 하나였고, 나머지의 프레임은
  // 전부 펌프에서 멈춰 섰다("지금 에이전트가 떠도 화면을 볼수없음"). 카드가
  // 두 번째 티어를 들면 보드는 tui처럼 전부를 한꺼번에 보여 준다.
  //
  // 판을 가진 카드에만 — 헬퍼(`sub:`)는 자기 화면이 없고 그 출력은 부모의
  // 것이며, 레인은 무대에만 산다. 노드를 표에 적는 것이 곧 선언의 근거이므로
  // (`previewingTerms`가 서 있는 노드에서 유도한다) 두 줄은 붙어 있어야 한다.
  const screenTerm = card.pane.startsWith("term:") ? Number(card.pane.slice(5)) : null;
  if (preview && screenTerm !== null && Number.isInteger(screenTerm)) {
    const screen = document.createElement("span");
    screen.className = "board-card-screen";
    node.appendChild(screen);
    previewScreens.set(screenTerm, screen);
    // 아는 것이 있으면 지금 곧바로 — 다음 박자를 기다리면 다시 그릴 때마다
    // 카드가 한 번씩 깜빡인다.
    paintPreviewScreen(screenTerm);
  }

  const foot = document.createElement("span");
  foot.className = "board-card-foot";
  const model = document.createElement("span");
  model.className = "agent-card-model";
  model.textContent = facts.model;
  foot.appendChild(model);
  // The repo's own chip leads the footer — Orca's 18px glyph tile whose
  // tooltip says the repo's name (AgentKanbanCard.tsx footer). The name is
  // resolved from the card's project path by the hand every list here uses.
  const repoName = card.project ? projectNameAt(card.project) : "";
  if (repoName) {
    const repo = document.createElement("span");
    repo.className = "board-card-repo";
    repo.innerHTML = icon("folder");
    repo.dataset.tip = repoName;
    repo.setAttribute("role", "img");
    repo.setAttribute("aria-label", repoName);
    foot.appendChild(repo);
  }
  // And the machine, only when it is not this one — Orca's DashboardHostBadge
  // renders nothing for a local card and a server glyph whose tooltip names
  // the SSH host for a remote one (DashboardHostBadge.tsx:28-45).
  if (place?.host === "ssh") {
    const host = document.createElement("span");
    host.className = "board-card-host";
    host.innerHTML = icon("server");
    const said = place.hostName
      ? t("board.host.sshNamed", "SSH 호스트 · {{host}}", { host: place.hostName })
      : t("board.host.ssh", "SSH 호스트");
    host.dataset.tip = said;
    host.setAttribute("role", "img");
    host.setAttribute("aria-label", said);
    foot.appendChild(host);
  }
  // The workspace stands here only while something else heads the card —
  // Orca drops it "rather than say it twice" when the worktree already IS
  // the heading (`worktreeInFooter`, AgentKanbanCard.tsx:213-216).
  if (card.worktree && card.worktree !== card.heading) {
    const where = document.createElement("span");
    where.className = "board-card-where";
    where.textContent = card.worktree;
    foot.appendChild(where);
  }
  /* 「아티팩트 N」 — 이 자리의 워커가 남긴 것이 있을 때만 (t-2720 §3). 워커는
   * 원장 명부가 이 판에 붙여 둔 것이고, 수는 카탈로그의 것이다: 둘 다 이미 손에
   * 있으므로 카드는 아무것도 묻지 않는다. */
  const seatTerm = card.pane.startsWith("term:") ? Number(card.pane.slice(5)) : NaN;
  const seatWorker = paneLedger.get(seatTerm)?.worker;
  const artifactChip = seatWorker
    ? artifactChipNode("board-card-artifacts", "worker", seatWorker, facts.identity)
    : null;
  if (artifactChip) foot.appendChild(artifactChip);
  // A card the ledger alone knows says so, in words. This window has no
  // terminal for it, which is the one thing about it a person cannot act on
  // without being told: the card opens no screen, and a card that silently
  // does nothing when clicked reads as broken. Words rather than a tint,
  // because a colour is not a sentence and a person who cannot see the tint
  // gets no sentence at all.
  if (card.pane.startsWith("worker:")) {
    const seatless = document.createElement("span");
    seatless.className = "board-card-seatless";
    seatless.textContent = t("board.card.noSeat", "이 창에 터미널 없음");
    foot.appendChild(seatless);
  }
  // The review its checkout is on, when there is one. In the meta row rather
  // than the head: the head already carries the state dot, and a second mark
  // beside it makes two things claiming to say how the agent is going.
  if (review) foot.appendChild(reviewPillNode(review));
  // Only when the agent has actually reported. A lane that has never said
  // anything has no moment to count from, and `방금` on it would be a claim.
  //
  // The old clock still answers when this card last spoke. A running card adds
  // the state-start clock beside it, so the two different questions keep two
  // visibly different answers.
  const clock = cardClock(card, bucket, now);
  if (clock) {
    const when = document.createElement("span");
    when.className = "board-card-when";
    // 도는 시계는 **움직이는 숫자**다. 발치에서 유일하게 스스로 자라는 값이므로
    // 잉크를 한 칸 올려 둔다 — 낱말이 이미 뜻을 말하고 있으니 색은 눈이 그
    // 낱말을 찾는 데만 쓰인다(색이 빠져도 문장은 그대로다).
    when.textContent = clock.word;
    foot.appendChild(when);
    if (clock.running) {
      const elapsed = document.createElement("span");
      elapsed.className = "board-card-when is-running";
      elapsed.textContent = clock.running;
      foot.appendChild(elapsed);
    }
  }
  if (foot.childElementCount > 0) node.appendChild(foot);

  // A card click LOOKS, without leaving — Orca acks the card and opens the
  // pane's own live screen in a dialog (`AgentTerminalDialog`,
  // I18nProvider:96907, opened :97066), with the worktree door in the footer
  // (:96957). A lane's screen only exists on the stage, so a lane card stays a
  // door. On an answerable card either road only answers clicks that landed
  // outside the question's own controls (Orca's interactive-target rule,
  // :69880). The bucket travels with the card: the dialog's header says how it
  // is going, and only the column knows that.
  node.onclick = (event) => {
    if (boxed && event.target.closest(".board-ask, .board-approve, .board-card-ask, .board-card-kids"))
      return;
    openBoardCard(card, bucket);
  };
  return node;
}

/* What a card opening MEANS, in one place.
 *
 * The kanban's card and the orchestration tile are two drawings of one card,
 * so opening one has to be one function: a second copy is how the two roads
 * end up with only one of them activating the foreign workspace first, and
 * that hole was already fixed once here (`openPaneFromBoard`). */
function boardCardDestination(card) {
  const pane = String(card?.pane ?? "");
  const at = pane.indexOf(":");
  const kind = at < 0 ? "" : pane.slice(0, at);
  const id = at < 0 ? "" : pane.slice(at + 1);
  const split = id.indexOf(":");
  const number = kind === "term" ? Number(id)
    : kind === "sub" && split > 0 ? Number(id.slice(0, split)) : NaN;
  const term = Number.isSafeInteger(number) && number >= 0 && id !== "" ? number : null;
  const valid = kind === "term" || kind === "sub" ? term !== null
    : (kind === "worker" || kind === "lane") && id !== "";
  return { kind, id, term, valid };
}

function openBoardCard(card, bucket) {
  const { kind, id, term, valid } = boardCardDestination(card);
  if (!valid) return;
  if (kind === "term") {
    void openBoardPeek(card, term, bucket);
    return;
  }
  // A helper's card opens the pane it runs inside — `sub:<term>:<id>`, so
  // the shell is the first field. It has no screen of its own and this is
  // not a shortcoming to route around: its output IS the parent's output.
  if (kind === "sub") {
    void openBoardPeek(card, term, bucket);
    return;
  }
  /* A worker the ledger alone knows has no screen in this window — that IS
   * what the card reports. So the click does the one thing that is true here:
   * it acknowledges. Without it the card would be unacknowledgeable, and an
   * unseen ending stands in Done forever by design (`bucket_at`) — the rule
   * that keeps a lost worker from being hidden would turn into a pile nobody
   * can put down. */
  if (kind === "worker") {
    acknowledgedPanes.set(card.pane, Date.now());
    if (isPopout) void invoke("ack_board_agent", { pane: card.pane }).catch(() => {});
    void paintBoardView();
    return;
  }
  focusLane(id);
}

/* ---- the look without leaving ----
 *
 * The pane's own screen, BORROWED. Orca's `AgentTerminalDialog` holds the
 * card's live terminal — the same instance the tab holds — and resizes the
 * real pty to the dialog's box, so what is read there is genuinely rewrapped
 * to the width it is being read at.
 *
 * This used to copy the view's paint into a `<pre>` of its own, and the copy
 * is what made it wrong: the pane is wider than the dialog, so every line ran
 * off the right edge with nothing to reflow it. A copy cannot be reflowed —
 * only the pty can do that, and only if it is told the new size.
 *
 * So the host is moved rather than mirrored: out of its slot into the dialog
 * on the way in, back into the exact place it came from on the way out, with
 * its `hidden` state restored so the stage's own bookkeeping still holds. The
 * keyboard follows it, because a screen you can watch changing under someone
 * else's typing but not answer is a worse lie than no screen at all. */
let peekTerm = null;
/* Where the borrowed host has to go back to: its parent, the sibling it sat
 * before, and whether it was drawn at all. Captured rather than recomputed —
 * the stage decides both from a layout this dialog has no view of, and a host
 * returned to the wrong slot is a terminal in someone else's pane. */
let peekHome = null;
/* 어느 카드였는가. 팝아웃의 "워크스페이스 열기"는 term 번호가 아니라 이
 * 이름을 메인 창으로 건넨다 — 그쪽에서 탭을 찾는 것은 그쪽의 일이다. */
let peekPane = null;

async function openBoardPeek(card, term, bucket) {
  // Looking IS acknowledging — the same meaning a tab gives it, written
  // before the dialog is even up (Orca acks on click, :96574).
  acknowledgedPanes.set(card.pane, Date.now());
  // 팝아웃에서 본 것도 본 것이다. 확인은 창 하나의 기억이 아니라 이
  // 에이전트에 대한 사실이므로, 그 사실만 메인 창으로 건넌다.
  if (isPopout) void invoke("ack_board_agent", { pane: card.pane }).catch(() => {});
  peekTerm = term;
  peekPane = card.pane;
  el("peek-title").textContent = card.heading;
  // Who is working and how it is going, in one muted line — the header says
  // what the card said, so the dialog does not have to be closed to re-read
  // it. A caller with no bucket to offer says only who.
  el("peek-agent").textContent =
    bucket === undefined ? card.agent : `${card.agent} · ${bucketWord(bucket)}`;
  // 이 창이 이 셸의 화면을 이미 들고 있었는가. 들고 있지 않았다면 방금
  // 만들어진 뷰이고 — 팝아웃의 미리보기는 언제나 이쪽이다 — 그 뷰의 바닥은
  // 아래의 선언이 새로 읽기 시작한 셸의 스냅샷 빚으로 받는다. 그 답이 이 셸을
  // `missing`으로 부르면 뒤에 셸이 없다는 뜻이다.
  const newborn = !termViews.has(term);
  const view = termView(term);
  peekHome = { parent: view.host.parentNode, next: view.host.nextSibling, hidden: view.host.hidden };
  el("peek-body").appendChild(view.host);
  // Unhidden BEFORE the measure, and before the resize below: a hidden host
  // has no geometry, the metrics probe reads zero from it, and `resizeTermTab`
  // refuses outright to size a screen nobody is looking at.
  view.host.hidden = false;
  // 지난 카드의 대답이 이 카드의 화면 위에 남아 있으면 안 된다.
  el("peek-closed").hidden = true;
  showModal(el("peek-scrim"));
  // 키보드는 이미 이 화면을 따른다(`keyboardTarget`의 peek 분기) — 하지만
  // 씽크 자체가 보드 버튼에 포커스를 뺏긴 채면 한 글자도 닿지 않고 커서는
  // 빈 고리로 남는다("잇풋창에 재대로 포커스 안됨", Image #28). 여는 손이
  // 곧 포커스다.
  // Measured in the box it now sits in, not the one it left. Then painted —
  // a view whose tab was off the stage tracked its rows without touching the
  // DOM, and this is where that screen comes back — and only then is the pty
  // told the shape of what the person is actually reading.
  view.measure();
  view.repaint();
  resizeTermTab(term);
  // A preview acknowledgement repaints card dress, but opening a preview is
  // not a graph-selection gesture (the pop-out can call this with no selected
  // graph node at all). Keep an empty selection empty on that repaint.
  void paintBoardView(boardTab(), { force: true, selectDefault: false });
  // 미리보기도 감시자다 — 이 화면은 무대 밖에서 열리므로 `updateStage`를 지나지
  // 않는다. 선언이 새 셸의 바닥 빚을 세우고, 그 뒤의 당김이 그 빚을 갚는다.
  const floor = await syncWatchedTerms();
  // 들여다보는 판은 전속력 티어로 올라갔으므로 미리보기 집합에서는 빠진다 —
  // 한 화면이 델타와 스냅샷을 번갈아 받으면 그 둘 사이에서 찢어진다. 뒤에 서있는
  // 카드는 그동안 마지막 꼬리를 든 채로 멈춰 있고, 닫힐 때 다시 내려간다.
  void syncPreviewedTerms();
  if (peekTerm !== term) return;
  if (!newborn) return;
  // 화면의 바닥은 선언 뒤의 당김이 이미 발랐다. 백엔드가 이 셸을 들고 있지
  // 않으면 그 답이 `missing`으로 부르고, 그것이 "라이브 터미널 없음"의
  // 근거다 — 빈 상자는 왜 비었는지 말하지 않는다. 선언이 새로 든 것 없이
  // 끝났다면(같은 셸을 이미 읽고 있었다) 한 번 더 물어 같은 답을 얻는다.
  const answer = floor ?? await awaitTermPull();
  // 그 사이에 닫혔거나 다른 카드가 열렸다면 이 답은 남의 화면이다. 여기서
  // 남은 것은 이 대화상자만 아는 사실, 즉 셸이 없다는 것을 말로 하는 일이다.
  if (peekTerm !== term) return;
  el("peek-closed").hidden = !(answer?.missing ?? []).includes(term);
}

// 미리보기 안의 클릭이 포커스를 도로 가져온다 — 무대의 판 host들이 쓰는
// 그 mousedown 관용구 그대로.
el("peek-body").addEventListener("mousedown", () => keySink.focus());

function closeBoardPeek() {
  hideModal(el("peek-scrim"));
  // Escape and the X can both arrive, and the second one must find nothing
  // left to give back.
  if (peekTerm === null || peekHome === null) {
    return;
  }
  const term = peekTerm;
  const home = peekHome;
  peekTerm = null;
  peekHome = null;
  peekPane = null;
  // 읽는 도중에 셸이 끝났을 수 있다 — `term:exited`가 뷰를 이미 버렸다면
  // 돌려줄 것이 없고, `termView`를 부르면 아무도 원하지 않는 빈 화면을
  // 하나 만들어 `termViews`에 남긴다.
  const view = termViews.get(term);
  if (!view) return;
  // Only if the host is still the dialog's to give back. A pane rebuild while
  // this was open re-appends the host into its own fresh slot, and that slot
  // is a better home than the one recorded here.
  if (view.host.parentNode === el("peek-body")) {
    // The recorded home can have been torn out from under it meanwhile — a
    // pane closed, a tab pruned. `stage` is where a host with no slot lives,
    // the same place `termView` puts a newborn one, so nothing is left
    // stranded inside a hidden dialog; and a sibling that is no longer the
    // parent's child would make `insertBefore` throw, which would strand it
    // exactly there.
    const parent = home.parent?.isConnected ? home.parent : stage;
    const next = home.next?.parentNode === parent ? home.next : null;
    parent.insertBefore(view.host, next);
    view.host.hidden = home.hidden;
  }
  // 이 창에 이 셸의 자리가 없다면, 그 화면은 미리보기가 보려고 만든 것이다.
  //
  // `openBoardPeek`은 뷰가 없으면 만든다(`termView`). 팝아웃에는 무대도 탭도
  // 없으므로 거기서 보는 카드는 **언제나** 이쪽이고, 메인 창에서도 남의
  // 워크스페이스 카드를 들여다보면 그렇다. 닫으면서 돌려줄 자리가 없으니 그
  // 화면은 `stage` 아래 숨은 채 `termViews`에 남았다 — 카드를 열두 번 들여다본
  // 팝아웃은 열두 장의 죽은 화면과 그만큼의 다시그리기 클로저를 들고 있게 된다.
  //
  // 화면만 버린다. 이 에이전트는 살아 있고, 훅 상태도 대화 번호도 확인 시각도
  // 여전히 참이다 — 그것들까지 지우는 것은 셸이 끝났을 때의 일이다.
  if (!tabOfTerm(term)) {
    dropTermScreen(term);
    return;
  }
  view.measure();
  // The pty was reflowed to the dialog. If the tile is on screen it needs its
  // own shape back now; if it went back hidden, `updateStage` measures and
  // resizes it on the way to visible, which is the same path a background tab
  // has always come back through.
  if (!view.host.hidden) {
    view.repaint();
    resizeTermTab(term);
  }
  // 그리고 이 화면을 다시 읽지 않게 되었을 수 있다 — 빌려 온 판이 원래 숨어
  // 있었다면 그렇다. 열 때와 같은 문으로 말한다.
  void syncWatchedTerms();
  // 전속력을 놓았으니 그 카드는 다시 두 번째 티어로 내려간다 — 아무도 안
  // 보는 것과 느리게 보는 것 사이에는 카드 한 장만큼의 차이가 있다.
  void syncPreviewedTerms();
}

/* The footer's door: the pane's own tab, with its worktree activated FIRST
 * when it is not the one on stage — `paneTabs` filters by the active
 * worktree, so a foreign tab set active without the switch is a tab the
 * stage is not showing. (The old card click had exactly that hole.) */
async function openPaneFromBoard(term) {
  const tab = tabOfTerm(term);
  if (!tab) return;
  if (tab.worktree !== activeWorktreePath && !(await activateWorktree(tab.worktree))) return;
  setActiveTab(tab.id);
}

el("peek-open").addEventListener("click", () => {
  const term = peekTerm;
  const pane = peekPane;
  closeBoardPeek();
  // 팝아웃에는 무대가 없다. 문은 메인 창에 있으므로 그 창을 올려 달라고
  // 하고 어느 카드였는지만 건넨다 — 탭을 찾는 일은 무대를 가진 쪽의 것이다
  // (Orca의 `dashboardPopout:revealAgent`, :54140).
  if (isPopout) {
    if (pane !== null) void invoke("reveal_board_agent", { pane }).catch(showError);
    return;
  }
  if (term !== null) void openPaneFromBoard(term);
});

/* ---- 팝아웃이 건넨 두 가지 사실 ----
 *
 * 보드의 내용은 두 창이 각자 백엔드에서 읽으므로 릴레이할 것이 없다. 창을
 * 건너는 것은 사람이 한 일 둘뿐이다: 카드를 열어 봤다는 것과, 그 워크스페이스로
 * 데려가 달라는 것. 둘 다 메인 창에만 배달된다(`emit_to("main", …)`). */
listen("board:ack", (event) => {
  const pane = event.payload?.pane;
  if (!pane) return;
  acknowledgedPanes.set(pane, Date.now());
  void refreshBoardBadge();
  if (boardIsOpen()) void paintBoardView();
});

listen("board:reveal", (event) => {
  // 창은 Rust가 이미 올렸다. 여기서 남은 것은 그 카드의 자리로 가는 것뿐이고,
  // 그 길은 카드 클릭이 쓰는 바로 그 두 함수다.
  const pane = event.payload?.pane;
  if (!pane) return;
  revealAgentGraphCard({ pane });
});

el("peek-close").addEventListener("click", closeBoardPeek);

/* The board's two controls, wired on the view that holds them.
 *
 * Assigned rather than added, and per paint rather than once: this surface is
 * cloned for a second leaf and a clone carries no listeners — the same reason
 * the file view re-assigns its mode button every time. The values come from the
 * module's own two variables so a repaint does not lose what was typed. */
function wireBoardHead(view) {
  paintWorkbenchNavigation(view, "tasks");
  for (const button of view.querySelectorAll("[data-board-mode]")) {
    writeAttribute(button, "aria-pressed", String(button.dataset.boardMode === agentBoardMode));
    button.onclick = () => {
      agentBoardMode = button.dataset.boardMode;
      setAgentGraphInspectorOpen(view, false);
      const model = agentGraphFullModel(view);
      if (model && agentBoardMode === "graph" && agentGraphScopeKey && agentGraphSelectedKey) {
        const group = agentGraphTasksFor(view, model).groups.find((one) =>
          one.members.some((entry) => entry.key === agentGraphSelectedKey));
        if (group?.key !== agentGraphScopeKey) setAgentGraphScope(view, group?.key ?? "");
        else paintAgentGraph(view, model);
      } else if (model) paintAgentGraph(view, model);
      wireBoardHead(view);
      void syncPreviewedTerms();
    };
  }
  const query = view.querySelector(".board-query");
  query.value = boardQuery;
  // The shortcut sits in the empty field — Orca's ⌘K keycap, standing only
  // while nothing is typed: once something is, the field's own clear takes
  // that corner (AgentDashboardToolbar.tsx:100-117). Spelled by the window's
  // one chord speller, so Linux reads Ctrl+K.
  const keys = view.querySelector(".board-search-keys");
  keys.textContent = chordLabel("mod+k");
  keys.hidden = boardQuery !== "";
  query.oninput = (event) => {
    boardQuery = event.target.value;
    keys.hidden = boardQuery !== "";
    void paintBoardView();
  };
  view.querySelector(".agent-graph-focus").onclick = () => focusAgentGraphSelection(view);
  const popout = view.querySelector(".board-popout");
  popout.onclick = () => {
    const title = agentBoardMode === "tasks"
      ? t("board.tasks.heading", "작업 상황판") : t("board.graph.heading", "에이전트 그래프");
    void invoke("open_board_popout", { title })
      .catch(showError);
  };
}

/* ---- ZeroCode agent ontology graph ---------------------------------------
 *
 * One picture, three questions. WHERE work lives is containment: a project
 * band holds workspaces, a workspace holds agents. WHO started whom is
 * lineage: the dashed edge from an agent to the agent it spawned. WHAT is
 * happening right now is the live wire inside an agent's card — the last few
 * tool calls, each still carrying its own three fields (verb, target, phase),
 * which is what makes this an ontology of running work rather than a diagram
 * of a directory tree.
 *
 * Projects are BANDS and not a fourth column because projects are the count
 * that grows without bound. A column puts every band's identity in a 116px
 * cell that scrolls away sideways the moment the lineage gets deep; a band's
 * rail stays pinned to the left edge, carries the project's own rollup, and
 * folds shut when a person is done with it.
 *
 * State is an attribute, never a layout axis, so a hook event repaints a
 * node's dress and never moves it out from under the pointer. */

/* The last few things one agent did, newest first.
 *
 * The ring the backend keeps is already the right shape — `{verb, target,
 * phase}` per call — so this slices it and never re-derives anything. Three
 * is the number a card can draw without becoming a log: the newest line is
 * the answer to "what is it doing", and the two behind it are the answer to
 * "and what was it doing a second ago", which is the question a person
 * watching five agents actually asks next. */
function agentGraphTicker(pane) {
  return agentGraphFoldedActivities(pane).slice(-AGENT_GRAPH_TICKER_KEEP);
}

/* 그리고 그 중 마지막 하나 — 활동 줄이 문장으로 말하는 그것. 접힌 다음의
 * 마지막이라 「같은 명령 셋」은 한 줄이고 그 줄이 셋을 세고 있다. */
function agentGraphNewestBeat(pane) {
  return agentGraphFoldedActivities(pane).at(-1) ?? null;
}

/* Adjacent repetitions are one activity with a count, not three lines that
 * make the card look busier than it is. The source ring is chronological and
 * stays untouched; cards take its newest three folded rows and the inspector
 * takes the newest twenty in chronological order. */
const agentGraphActivitySeenAt = new WeakMap();

function agentGraphFoldedActivities(pane) {
  const folded = [];
  for (const stamped of paneActivities.get(pane) ?? []) {
    const activity = stamped?.activity;
    if (!activity) continue;
    let at = Number(stamped.at) || agentGraphActivitySeenAt.get(stamped) || 0;
    if (at === 0 && typeof stamped === "object") {
      at = Date.now();
      agentGraphActivitySeenAt.set(stamped, at);
    }
    const one = {
      verb: activity.verb ?? "",
      target: agentGraphCleanText(activity.target ?? ""),
      phase: activity.phase ?? "",
      at,
      repeat: 1,
    };
    const previous = folded.at(-1);
    if (
      previous && previous.verb === one.verb && previous.target === one.target &&
      previous.phase === one.phase
    ) {
      previous.repeat += 1;
      previous.at = Math.max(previous.at, one.at);
    } else {
      folded.push(one);
    }
  }
  return folded;
}

function agentGraphTickerSignature(pane) {
  return agentGraphTicker(pane)
    .map((beat) => [beat.verb, beat.target ?? "", beat.phase, beat.repeat, beat.at].join(""))
    .join("");
}

/* ---- 「지금 무엇을 하는가」를 읽는 한 손 (t-2374) ------------------------
 *
 * 카드가 말하는 것은 일곱이고(설계 §2), 그 일곱을 그리는 그림은 둘이다 —
 * 그래프의 노드와 인스펙터의 카드. 둘이 같은 사실을 **따로** 지으면 한 화면의
 * 두 그림이 같은 에이전트를 다르게 말하고, 그 어긋남은 고쳐도 다시 자란다
 * (실측된 전례: 상태 낱말이 눈썹줄과 대화 줄 양쪽에서 따로 서던 1-t353).
 * 그래서 사실은 여기서 한 번 조립되고 두 그림은 그것을 **그리기만** 한다.
 *
 * 아무것도 재지 않는다: 여기 있는 것은 전부 이 창이 이미 들고 있는 값이고,
 * 그래서 이 함수는 판마다 카드마다 돌아도 레이아웃을 건드리지 않는다. */

/* 이 세션이 마지막으로 말한 컨텍스트 사용량. 상태바의 낱말과 카드의 미터가
 * 여기 하나에서 나온다. */
const sessionUsage = new Map();
const paneAutonomy = new Map();

function paneAutonomyValue(term, fallback = null) {
  const held = paneAutonomy.get(term);
  if (!held) return fallback;
  const session = paneSessions.get(term)?.session?.id;
  return session && held.session && session !== held.session ? null : held.value;
}

function rememberPaneAutonomy(term, value, session = null) {
  const current = paneSessions.get(term)?.session?.id;
  if (current && session && current !== session) return;
  const clean = value && typeof value === "object" ? value : null;
  const held = paneAutonomy.get(term);
  if (held?.session === session && JSON.stringify(held.value) === JSON.stringify(clean)) return;
  paneAutonomy.set(term, { session, value: clean });
  scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
}

function autonomyRunning(autonomy) {
  return autonomy?.goal?.phase === "running" ||
    (Array.isArray(autonomy?.loops) && autonomy.loops.some((one) => one?.phase === "running"));
}

function autonomousAgentState(state, autonomy, ledger) {
  if (["release_pending", "release_unknown", "released", "sleeping"].includes(ledger)) {
    return state === "working" || state === "needs-attention" ? "done" : state;
  }
  if (state !== "done") return state;
  if (autonomyRunning(autonomy)) return "working";
  return autonomy?.goal?.phase === "paused" || autonomy?.loops?.some((one) => one?.phase === "paused")
    ? "paused" : state;
}

function autonomousPaneState(term, state) {
  return autonomousAgentState(state, paneAutonomyValue(term), paneLedger.get(term)?.ledger);
}

listen("pane:autonomy", (event) => {
  const { term, session, autonomy, model } = event.payload ?? {};
  if (typeof term !== "number") return;
  const current = paneSessions.get(term)?.session?.id;
  if (current && session && current !== session) return;
  if (model) paneModels.set(term, model);
  rememberPaneAutonomy(term, autonomy, session);
});

function rememberSessionUsage(session, frame) {
  const tokens = Number(frame?.ctx_tokens ?? 0);
  const window = Number(frame?.context_window ?? 0);
  const reading = {
    tokens: Number.isFinite(tokens) && tokens > 0 ? tokens : 0,
    window: Number.isFinite(window) && window > 0 ? window : 0,
  };
  if (session) sessionUsage.set(session, reading);
  return reading;
}

function usageCtxWord(reading) {
  return `ctx ${reading.tokens.toLocaleString()}`;
}

/* 그리고 판의 전사가 말한 같은 두 수 — 훅은 토큰을 나르지 않고 채널은
 * zo의 것이라, 그 둘이 없는 판에게는 제 전사가 유일한 출처다(`pane_log`의
 * `usage`). 판 번호로 잡히므로 판이 죽을 때(`term:exited`)와 대화를 잊을
 * 때(`forgetPaneChat`) 같이 지운다. */
const paneUsage = new Map();

/* 담고, 값이 움직였는지 말한다 — 움직였을 때만 칩을 다시 그리므로, 같은
 * 수를 다시 실어 온 폴은 아무것도 만지지 않는다. */
function rememberPaneUsage(term, usage) {
  const tokens = Number(usage?.tokens ?? 0);
  const window = Number(usage?.window ?? 0);
  if (!Number.isFinite(tokens) || tokens <= 0) return false;
  const reading = { tokens, window: Number.isFinite(window) && window > 0 ? window : 0 };
  const held = paneUsage.get(term);
  if (held && held.tokens === reading.tokens && held.window === reading.window) return false;
  paneUsage.set(term, reading);
  return true;
}

/* 그리고 그 판의 카드가 읽는 같은 값 — 비율까지.
 *
 * 둘 다 있어야 그린다. 토큰만 아는 카드에 막대를 그리려면 창 크기를 지어내야
 * 하고, 지어낸 분모 위의 막대는 「거의 찼다」와 「이제 시작」을 구별하지 못하는
 * 그림이다. 모르면 그리지 않고, DOM에는 그대로 둔다(§2.6). */
/* 이 카드가 어느 판을 말하는가 — 그 판이 있을 때만.
 *
 * 헬퍼(`sub:`)·레인·원장만 아는 워커는 판 번호가 없고, 그것이 모델도
 * 컨텍스트도 없는 이유다. 없는 것을 부모의 것으로 메우면 재지 않은 사실을
 * 카드에 적는 일이 된다(사이드바의 같은 규칙, `fit.model = row.sub ? null : …`). */
function agentCardTerm(card) {
  // A workbench entry's card may carry no pane at all (a knowledge or
  // workspace entry, a ledger-only worker): no pane, no term — the same
  // answer a `sub:` pane gets, not a crash on the way to it.
  const pane = typeof card?.pane === "string" ? card.pane : "";
  const at = pane.startsWith("term:") ? Number(pane.slice(5)) : NaN;
  return Number.isInteger(at) ? at : null;
}

/* 그리고 그 판이 지금 어느 **모델**을 쓰는가 (t-2374).
 *
 * 이 창은 진작부터 알고 있었다 — `hook:agent`가 이벤트마다 싣고 `paneModels`가
 * 붙들어 둔다 — 그런데 그 값을 읽는 것은 사이드바 행 하나뿐이었고, 보드의 칩은
 * 그동안 벤더 이름을 모델이라 적었다. 카드에 얹어 보낼 수는 없다: 카드는
 * `board_columns`를 지나 Rust `BoardCard`로 왕복하고, 그 구조체가 이름을
 * 모르는 필드는 돌아오는 길에 사라진다(`checkout`이 `places`를 타고 따로
 * 오는 이유와 같다). 그래서 카드가 아니라 **판**에 묻는다 — Rust 변경 0. */
function agentCardModel(card) {
  const at = agentCardTerm(card);
  return at === null ? "" : agentGraphCleanText(paneModels.get(at) ?? "");
}

function agentCardContext(card) {
  const at = agentCardTerm(card);
  if (at === null) return null;
  const id = paneSessions.get(at)?.session?.id;
  const held = id ? sessionUsage.get(id) : null;
  if (!held || held.tokens <= 0 || held.window <= 0) return null;
  return { ...held, ratio: Math.min(1, held.tokens / held.window) };
}

/* 이 턴이 지금 어느 걸음에 서 있는가 (설계 §2.2).
 *
 * 새 상태가 아니라 `working` 위에 입는 옷이다 — `went_quiet`과 같은 층이고,
 * 상태 낱말은 넷 다 「작업 중」 그대로다. 넷은 이 창이 이미 들고 있는 사실에서
 * 나온다: 도구 링의 마지막 박자가 열려 있으면 도구를 돌리는 중이고, 닫혀
 * 있으면 그 결과 위에 답을 쓰는 중이며, 아무 박자도 없는 턴은 아직 모델을
 * 기다리는 중이다. 판정이 `bucket`인 것은 카드의 시계와 같은 이유다: 멈춘
 * 프로세스는 마지막 `working`을 영원히 들고 있고 그것을 걷어내는 규칙은
 * 원장을 읽는 백엔드의 것이다(`zerocode_core::board`, t-612).
 *
 * `stop`을 「답 쓰는 중」이 아니라 「모델 대기」로 읽는 것은 그 동사가 턴의
 * 끝을 뜻하기 때문이다 — 그 뒤에 오는 것은 다음 사람의 말이다. */
function agentTurnPhase(card, bucket, now) {
  if (bucket !== "working") return "";
  if (card.at > 0 && now - card.at >= AGENT_GRAPH_QUIET_AFTER_MS) return "quiet";
  const newest = agentGraphNewestBeat(card.pane);
  if (!newest) return "waiting";
  if (newest.phase === "started") return "tooling";
  if (newest.verb === "stop") return "waiting";
  return "answering";
}

function agentTurnPhaseWord(phase) {
  if (phase === "tooling") return t("board.phase.tooling", "도구 실행 중");
  if (phase === "answering") return t("board.phase.answering", "답 쓰는 중");
  if (phase === "waiting") return t("board.phase.waiting", "모델 대기");
  if (phase === "quiet") return t("board.graph.wentQuiet", "조용해짐");
  return "";
}

/* zo가 스스로 무엇을 계속하고 있는지, 턴 단계보다 구체적인 한 문장으로.
 * goal과 loop를 따로 그리는 함수가 생기면 카드 두 표면이 갈라지므로 선택,
 * 시간·간격 정규화, i18n 조립을 이 한 builder가 모두 소유한다. */
function agentAutonomyPhrase(autonomy) {
  if (!autonomy || typeof autonomy !== "object") return null;
  const clock = (value) => {
    const millis = Number(value);
    if (!Number.isFinite(millis) || millis <= 0) return "";
    const date = new Date(millis);
    if (Number.isNaN(date.getTime())) return "";
    return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  };
  const phases = {
    running: t("board.autonomy.pursuing", "추구 중"),
    saved: t("board.autonomy.saved", "저장됨"),
    paused: t("board.autonomy.paused", "일시 정지"),
    stopped: t("board.autonomy.stopped", "중단됨"),
  };
  const items = [];
  const goal = autonomy.goal;
  if (goal && typeof goal === "object") items.push({ kind: "goal", value: goal });
  for (const loop of Array.isArray(autonomy.loops) ? autonomy.loops : []) {
    if (loop && typeof loop === "object") items.push({ kind: "loop", value: loop });
  }
  items.sort((a, b) => Number(b.value.phase === "running") - Number(a.value.phase === "running"));
  if (items.length === 0) return null;
  const lines = items.map(({ kind, value }) => {
    const phase = agentGraphCleanText(value.phase);
    const next = phase === "running" ? clock(value.next_at) : "";
    const phaseWord = next ? t("board.autonomy.waitingNext", "다음 실행 대기")
      : phase === "completed" ? (kind === "goal"
        ? t("board.autonomy.achieved", "달성") : t("board.autonomy.completed", "반복 완료"))
        : phases[phase] ?? phase;
    const words = [kind === "goal" ? t("board.autonomy.goal", "목표")
      : `${t("board.autonomy.loop", "루프")} ${agentGraphCleanText(value.id)}`.trim(), phaseWord];
    if (kind === "goal") {
      const passed = Number(value.gates_passed);
      const total = Number(value.gates_total);
      if (Number.isFinite(passed) && Number.isFinite(total) && total > 0) {
        words.push(t("board.autonomy.gate", "게이트 {{passed}}/{{total}}", { passed, total }));
      }
    } else {
      const trigger = agentGraphCleanText(value.trigger);
      const every = trigger.match(/^every\s+(\d+)s$/);
      if (every) {
        const seconds = Number(every[1]);
        const duration = seconds % 3_600 === 0 ? `${seconds / 3_600}h`
          : seconds % 60 === 0 ? `${seconds / 60}m` : `${seconds}s`;
        words.push(t("board.autonomy.every", "{{duration}}마다", { duration }));
      } else if (trigger) words.push(trigger);
      const quiet = Number(value.quiet);
      if (Number.isFinite(quiet) && quiet > 0) words.push(t("board.autonomy.quiet", "조용함 ×{{count}}", { count: quiet }));
    }
    if (next) words.push(t("board.autonomy.resumes", "{{time}}에 재개", { time: next }));
    const reason = agentGraphCleanText(kind === "goal" ? value.pause_reason : value.reason);
    if (reason) words.push(reason);
    return words.filter(Boolean).join(" · ");
  });
  return { kind: items[0].kind, word: lines.join(" / "), tip: lines.join("\n") };
}

function agentActivityKind(verb) {
  return AGENT_ACTIVITY_KINDS.get(verb) ?? "other";
}

function agentActivityKindWord(kind) {
  if (kind === "read") return t("board.tick.read", "읽기");
  if (kind === "edit") return t("board.tick.edit", "고치기");
  if (kind === "run") return t("board.tick.run", "실행");
  if (kind === "report") return t("board.tick.report", "보고");
  if (kind === "ask") return t("board.tick.ask", "질문");
  return t("board.tick.other", "그 밖");
}

/* 한 박자를 띠의 한 칸으로. 툴팁은 종류와 대상 전문 — 여덟 글리프가 한 줄에
 * 눕는 자리에서 낱말이 설 곳은 거기뿐이다. */
function agentTickFact(beat) {
  const kind = agentActivityKind(beat.verb);
  const words = [agentActivityKindWord(kind), beat.target].filter(Boolean);
  return {
    kind,
    glyph: AGENT_ACTIVITY_GLYPHS.get(kind) ?? AGENT_ACTIVITY_GLYPHS.get("other"),
    repeat: beat.repeat > 1 ? String(beat.repeat) : "",
    tip: beat.repeat > 1 ? `${words.join(" \u00b7 ")} \u00d7${beat.repeat}` : words.join(" \u00b7 "),
  };
}

/* The card reads the newest tools as words; the eight-slot glyph ring remains
 * the compact detail carried by the tooltip and the mutation probe. */
function agentActivityTrailFact(beat) {
  const kind = agentActivityKind(beat.verb);
  const word = agentActivityKindWord(kind);
  const target = agentGraphActivityTarget(beat.verb, beat.target ?? "");
  const repeat = beat.repeat > 1 ? ` ×${beat.repeat}` : "";
  return {
    kind,
    word,
    target,
    tip: `${[word, beat.target].filter(Boolean).join(" · ")}${repeat}`,
  };
}

function agentCardFacts(card, bucket, now) {
  const state = agentGraphState({ card, bucket });
  const autonomy = !["failed", "error", "needs-attention", "waiting", "blocked", "exited"].includes(card.state) &&
    !(autonomyRunning(card.autonomy) && bucket !== "working")
    ? agentAutonomyPhrase(card.autonomy) : null;
  const phase = autonomy?.kind ?? agentTurnPhase(card, bucket, now);
  const newest = agentGraphNewestBeat(card.pane);
  const ticker = agentGraphTicker(card.pane);
  /* 확인 필요한 카드는 이 자리에 **질문**을 세운다 (설계 §2.3). 무엇을 하는
   * 중이냐는 물음의 답이 「사람을 기다린다」일 때, 그 자리에 옛 도구 호출을
   * 적는 것은 답이 아니라 잡음이다. */
  const asking = state === "needs-attention" && agentGraphCleanText(card.ask) !== "";
  return {
    identity: agentGraphIdentity(card),
    state,
    stateWord: agentGraphStateWord(state, card.ledger),
    phase,
    phaseWord: autonomy?.word ?? agentTurnPhaseWord(phase),
    phaseTip: autonomy?.tip ?? "",
    asking,
    /* 묻는 카드의 질문 첫 줄. **자리**가 아니라 사실로 실어 보내는 것은 그
     * 자리가 그림마다 다르기 때문이다: 그래프의 노드에는 질문이 설 다른 곳이
     * 없으므로 활동 줄이 그것을 들고, 인스펙터의 카드에는 호박색 상자가 이미
     * 서 있으므로 활동 줄은 도구를 말한다. 한 카드가 같은 문장을 두 번 적지
     * 않는다는 이 창의 규칙 그대로(`worktreeInFooter`). */
    question: asking
      ? { kind: "ask", word: t("board.tick.ask", "질문"),
          target: agentGraphCleanText(card.ask), when: "" }
      : null,
    activity: newest
      ? { kind: agentActivityKind(newest.verb), word: activityWord(newest.verb),
          target: agentGraphActivityTarget(newest.verb, newest.target ?? ""),
          tip: newest.target ?? "",
          when: card.at > 0 ? agoWord(card.at, now) : "" }
      : null,
    /* 벤더가 아니라 모델. `paneModels`가 아는 판은 제 모델 id를 적고, 모르는
     * 판만 벤더 이름으로 물러난다 — 「Claude」 셋이 나란히 선 화면에서 무엇이
     * Opus이고 무엇이 Haiku인지 여태 아무 데도 적혀 있지 않았다. */
    model: agentCardModel(card) || agentName(card.agent),
    context: agentCardContext(card),
    ticks: ticker.map(agentTickFact),
    trail: ticker.slice(-AGENT_GRAPH_ACTIVITY_KEEP).map(agentActivityTrailFact),
    trailHidden: Math.max(0, ticker.length - AGENT_GRAPH_ACTIVITY_KEEP),
  };
}

function agentGraphCleanText(value) {
  return String(value ?? "")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "")
    .trim();
}

function agentGraphState(entity) {
  const raw = String(entity.card?.state ?? "").toLowerCase().replaceAll("_", "-");
  if (raw === "failed" || raw === "error") return "failed";
  if (entity.bucket === "attention") return "needs-attention";
  if (entity.bucket === "idle" && raw === "done" &&
      autonomousAgentState(raw, entity.card?.autonomy, entity.card?.ledger) === "paused") return "paused";
  if (["working", "done", "idle"].includes(entity.bucket)) return entity.bucket;
  return "idle";
}

/* The ledger's word rides beside the hook's for ONE case: a worker whose
 * team leader exited keeps working — the ledger says `orphaned`, the hook says
 * `working`, and both are true. Drawn as plain 「작업 중」 the board hid the one
 * fact a person needs before they start a replacement over live work (t-2512).
 * Every other ledger word either takes the claim of work away (the bucket
 * already moved) or adds nothing to it, so this is the only pairing spelled. */
function agentGraphStateWord(state, ledger = "") {
  if (state === "needs-attention") return t("board.attention", "확인 필요");
  if (state === "working" && ledger === "orphaned") {
    return t("board.workingOrphaned", "작업 중(리더 없음)");
  }
  if (state === "working") return t("board.working", "작업 중");
  if (state === "paused") return t("board.autonomy.paused", "일시 정지");
  if (state === "done") return t("board.done", "완료");
  if (state === "failed") return t("board.graph.failed", "실패");
  return t("board.idle", "대기 중");
}

function agentGraphIdentity(card) {
  return agentGraphCleanText(card.task) || agentGraphCleanText(card.heading) || card.pane;
}

function agentGraphMessage(card, state) {
  const mayHeadlineError = state === "needs-attention" || state === "failed";
  for (const value of [card.said, card.you]) {
    const line = agentGraphCleanText(value);
    if (!line) continue;
    if (!mayHeadlineError && /^(?:exit code\s+\d+|error\b|failed\b)/i.test(line)) continue;
    return line;
  }
  return "";
}

/* Is this agent's newest tool call still open? A started call with nothing
 * after it is the one case where the card draws something that IS happening
 * rather than something that happened. */
function agentGraphIsLive(pane) {
  return agentGraphNewestBeat(pane)?.phase === "started";
}

function agentGraphAgentKey(value) {
  const word = agentGraphCleanText(value);
  if (!word) return "";
  return word.startsWith("agent:") ? word : `agent:${word}`;
}

function agentGraphModel(columns, places, reviews,
  now = Date.now(),
  previous = null,
  catalog = projects,
  search = boardQuery,
  overlays = {},
  { scoped = false } = {},
) {
  const counts = new Map(columns.map((column) => [column.bucket, column.cards.length]));
  const agents = columns.flatMap((column) =>
    column.cards.map((card) => ({
      key: `agent:${card.pane}`,
      card,
      bucket: column.bucket,
      place: places.get(card.pane) ?? {},
      review: reviews.get(card.pane) ?? null,
    })),
  );
  const byPane = new Map(agents.map((entry) => [entry.card.pane, entry]));
  const projectsAt = new Map();
  const workspacesAt = new Map();
  const unscopedProject = " unscoped-project";
  const unscopedWorkspace = " unscoped-workspace";

  const ensureProject = (projectPath, known = null) => {
    let project = projectsAt.get(projectPath);
    if (project) return project;
    project = {
      key: `project:${projectPath}`,
      path: projectPath,
      label: projectPath === unscopedProject
        ? t("board.graph.unscopedProject", "범위 밖")
        : known?.name || projectNameAt(projectPath),
      workspaces: new Set(),
      agents: new Set(),
    };
    projectsAt.set(projectPath, project);
    return project;
  };

  const ensureWorkspace = (project, workspacePath, known = null, card = null) => {
    const workspaceKey = `${project.path} ${workspacePath}`;
    let workspace = workspacesAt.get(workspaceKey);
    if (workspace) return workspace;
    workspace = {
      key: `workspace:${workspaceKey}`,
      path: workspacePath,
      project,
      label: workspacePath === unscopedWorkspace
        ? t("board.graph.unscopedWorkspace", "워크스페이스 없음")
        : known ? worktreeDisplayName(known)
          : card?.worktree || basename(workspacePath),
      branch: known?.branch ?? "",
      base: known?.base ?? "",
      scoped: workspacePath !== unscopedWorkspace,
      agents: new Set(),
    };
    workspacesAt.set(workspaceKey, workspace);
    project.workspaces.add(workspace);
    return workspace;
  };

  /* The catalog is the navigator's structural ledger. Most browser fixtures
   * predate this contract and deliberately feed synthetic cards for a wholly
   * different repository; do not graft the real fixture catalog onto those
   * isolated models. A production snapshot always carries the catalog project
   * path, while an actually empty snapshot still needs the catalog alone. */
  const knownCatalog = Array.isArray(catalog) ? catalog : [];
  const catalogPaths = new Set(knownCatalog.map((project) => project.path));
  const catalogBelongsToSnapshot = agents.length === 0 || agents.some((entry) =>
    catalogPaths.has(entry.card.project) ||
    (!entry.card.project && knownCatalog.some((project) =>
      project.worktrees?.some((worktree) => worktree.path === entry.place.checkout))),
  );
  if (catalogBelongsToSnapshot) {
    for (const knownProject of knownCatalog) {
      if (!knownProject?.path) continue;
      const project = ensureProject(knownProject.path, knownProject);
      for (const knownWorkspace of knownProject.worktrees ?? []) {
        if (!knownWorkspace?.path) continue;
        ensureWorkspace(project, knownWorkspace.path, knownWorkspace);
      }
    }
  }

  for (const entry of agents) {
    const projectPath = entry.card.project || unscopedProject;
    const workspacePath = entry.place.checkout || unscopedWorkspace;
    const knownProject = knownCatalog.find((candidate) => candidate.path === projectPath) ?? null;
    const project = ensureProject(projectPath, knownProject);
    const knownWorkspace = workspacePath === unscopedWorkspace
      ? null
      : knownProject?.worktrees?.find((candidate) => candidate.path === workspacePath)
        ?? worktreeAt(workspacePath);
    const workspace = ensureWorkspace(project, workspacePath, knownWorkspace, entry.card);
    entry.project = project;
    entry.workspace = workspace;
    entry.state = agentGraphState(entry);
    project.agents.add(entry);
    workspace.agents.add(entry);
  }

  const compareAgents = (left, right) =>
    left.project.label.localeCompare(right.project.label)
    || left.workspace.label.localeCompare(right.workspace.label)
    || left.card.pane.localeCompare(right.card.pane);
  const children = new Map();
  for (const entry of agents) {
    if (!byPane.has(entry.card.parent)) continue;
    const held = children.get(entry.card.parent) ?? [];
    held.push(entry);
    children.set(entry.card.parent, held);
  }
  for (const held of children.values()) held.sort(compareAgents);

  const needle = agentGraphCleanText(search).toLocaleLowerCase();
  const searchMatches = (...values) =>
    graphSearchMatch(needle, values, agentGraphCleanText);
  for (const project of projectsAt.values()) {
    project.searchOwnMatch = searchMatches(project.label, project.path);
    project.searchMatch = project.searchOwnMatch;
  }
  for (const workspace of workspacesAt.values()) {
    workspace.searchOwnMatch = searchMatches(
      workspace.label,
      workspace.path,
      workspace.branch,
      workspace.base,
    );
    workspace.searchMatch = workspace.project.searchOwnMatch || workspace.searchOwnMatch;
    if (workspace.searchMatch) workspace.project.searchMatch = true;
  }
  for (const entry of agents) {
    entry.searchMatch = entry.project.searchOwnMatch || entry.workspace.searchOwnMatch || searchMatches(
      entry.card.pane,
      entry.card.agent,
      entry.card.task,
      entry.card.heading,
      entry.card.worktree,
      entry.card.you,
      entry.card.said,
      entry.card.ask,
    );
    if (entry.searchMatch) {
      entry.workspace.searchMatch = true;
      entry.project.searchMatch = true;
    }
  }
  for (const workspace of workspacesAt.values()) {
    workspace.idleAgents = [...workspace.agents].filter((entry) => entry.state === "idle");
    workspace.activeAgents = [...workspace.agents].filter((entry) => entry.state !== "idle");
    workspace.idleExpanded = scoped ? !agentGraphScopeIdleCollapsed.has(workspace.key) : agentGraphIdleExpanded.has(workspace.key);
    /* A workspace with no live work becomes a project-rail chip. It is still
     * a model entity, and opening the chip restores the same keyed node. */
    const prior = previous?.entities?.get(workspace.key);
    workspace.collapsed = workspace.activeAgents.length === 0 && !workspace.idleExpanded &&
      prior?.collapsed !== false;
  }

  const rowOf = new Map();
  const depthOf = new Map();
  const placed = new Set();
  const visiting = new Set();
  /* Row 1 is the lane strip — the words naming what each column holds. Every
   * band opens below it. */
  let nextRow = 2;
  const topologySignature = [
    String(scoped),
    [...projectsAt.values()]
      .flatMap((project) => [
        `P${project.path}`,
        ...[...project.workspaces].map((workspace) => `W${project.path}\u0000${workspace.path}`),
      ])
      .sort()
      .join(""),
    agents
      .map((entry) => [
        entry.card.pane,
        entry.card.parent,
        entry.project.path,
        entry.workspace.path,
      ].join(""))
      .sort()
      .join(""),
    [...(scoped ? agentGraphScopeFolded : agentGraphFolded)].sort().join(""),
    [...agentGraphDormantExpanded].sort().join(""),
    [...(scoped ? agentGraphScopeIdleCollapsed : agentGraphIdleExpanded)].sort().join(""),
    /* The rows are ordered by LABELS, and two of those labels are translated
     * ("범위 밖", "워크스페이스 없음"). Changing the language can therefore
     * reorder the bands without a single card moving — and a signature that
     * did not carry the language would call that "unchanged" and leave every
     * edge measured against the order before it. */
    locale,
  ].join("");

  /* A subtree keeps its own rows, and a band keeps its own subtrees: the
   * recursion refuses to follow an edge that leaves the project, so an agent
   * one project's agent spawned inside another is drawn as a root of the band
   * it actually lives in and the spawned edge crosses the bands in the open. A
   * lineage that quietly reparented a card into a neighbouring project would
   * put a running agent under a heading that does not own it. */
  const place = (entry, depth) => {
    if (placed.has(entry.card.pane)) return rowOf.get(entry.card.pane);
    visiting.add(entry.card.pane);
    const childRows = (children.get(entry.card.pane) ?? [])
      .filter((child) => !visiting.has(child.card.pane) && child.project === entry.project)
      .map((child) => place(child, depth + 1));
    visiting.delete(entry.card.pane);
    /* 부모는 첫 자식의 행에 선다 (round three). 자식들의 한가운데에 서면 자식이
     * 둘일 때 부모가 둘째 자식과 나란히 서고, 그 위 행의 왼쪽은 빈 판이 된다 —
     * 실측: 헬퍼 둘을 거느린 카드 하나가 판의 3분의 1을 빈 자리로 만들었다.
     * 원장은 위에서 아래로 읽히므로 부모가 위, 자식이 그 곁에서 아래로. */
    const row = childRows.length > 0 ? childRows[0] : nextRow++;
    rowOf.set(entry.card.pane, row);
    depthOf.set(entry.card.pane, depth);
    placed.add(entry.card.pane);
    return row;
  };

  /* 워크스페이스의 이름표도 제 첫 에이전트의 행에 선다 — 절의 소제목처럼. */
  const centerOf = (entries) => {
    const rows = [...entries]
      .map((entry) => rowOf.get(entry.card.pane))
      .filter(Number.isFinite)
      .sort((left, right) => left - right);
    return rows.length === 0 ? nextRow++ : rows[0];
  };

  const orderedProjects = [...projectsAt.values()]
    /* The first screen is for live work. Catalog-only projects follow it as
     * compact inventory, with alphabetical order stable inside both groups. */
    .sort((left, right) =>
      Number(left.agents.size === 0) - Number(right.agents.size === 0)
      || left.label.localeCompare(right.label));
  for (const project of orderedProjects) {
    project.dormant = project.agents.size === 0;
    project.folded = scoped ? agentGraphScopeFolded.has(project.key) : (project.dormant
      ? !agentGraphDormantExpanded.has(project.key)
      : agentGraphFolded.has(project.key));
    project.counts = new Map();
    for (const entry of project.agents) {
      project.counts.set(entry.bucket, (project.counts.get(entry.bucket) ?? 0) + 1);
    }
  }
  const cachedLayout = previous?.topologySignature === topologySignature
    ? previous.layout
    : null;
  if (cachedLayout) {
    for (const [pane, row] of cachedLayout.rowOf) rowOf.set(pane, row);
    for (const [pane, depth] of cachedLayout.depthOf) depthOf.set(pane, depth);
    for (const project of orderedProjects) {
      const held = cachedLayout.projects.get(project.key);
      project.headRow = held?.headRow ?? 2;
      project.rowEnd = held?.rowEnd ?? project.headRow;
      for (const workspace of project.workspaces) {
        workspace.row = cachedLayout.workspaces.get(workspace.key) ?? project.headRow + 1;
      }
    }
    nextRow = cachedLayout.nextRow;
  } else {
    agentGraphLayoutRuns += 1;
    for (const project of orderedProjects) {
      project.headRow = nextRow;
      nextRow += 1;
      if (project.folded) {
        project.rowEnd = project.headRow;
        continue;
      }
      const inside = [...project.agents].sort(compareAgents);
      for (const entry of inside) {
        const above = byPane.get(entry.card.parent);
        if (!above || above.project !== project) place(entry, 0);
      }
      for (const entry of inside) {
        if (!placed.has(entry.card.pane)) place(entry, 0);
      }
      /* A workspace sits level with the middle of its own agents, and two of
       * them can want the same middle — a parent sharing a row with its one
       * child has put two workspaces on one line. Pushing the later one down
       * costs a row of drift; letting them collide costs one of them entirely. */
      const workspaces = [...project.workspaces]
        .map((workspace) => ({ workspace, wanted: centerOf(workspace.agents) }))
        .sort((left, right) => left.wanted - right.wanted
          || left.workspace.label.localeCompare(right.workspace.label));
      let lastRow = project.headRow;
      for (const { workspace, wanted } of workspaces) {
        workspace.row = Math.max(lastRow + 1, wanted);
        lastRow = workspace.row;
      }
      project.rowEnd = Math.max(project.headRow, nextRow - 1, lastRow);
      nextRow = project.rowEnd + 1;
    }
  }

  const bandNodes = new Map();
  const agentNodes = new Map();
  const bands = [];
  const edges = [];
  const entities = new Map();
  let maxDepth = 0;

  for (const project of orderedProjects) {
    [...project.workspaces]
      .sort((left, right) => left.label.localeCompare(right.label))
      .forEach((workspace, index) => {
        workspace.laneClass = AGENT_GRAPH_LANE_CLASSES[index % AGENT_GRAPH_LANE_CLASSES.length];
      });
    const band = {
      key: project.key,
      type: "project",
      row: project.headRow,
      rowEnd: project.rowEnd,
      folded: project.folded,
      dormant: project.dormant,
      label: project.label,
      line: agentGraphCountWord("workspace", project.workspaces.size),
      meta: project.path === unscopedProject ? "" : project.path,
      counts: project.counts,
      agentCount: project.agents.size,
      faces: [...project.agents].sort(compareAgents),
      searchMatch: project.searchMatch,
      dormantWorkspaces: [...project.workspaces]
        .filter((workspace) => workspace.collapsed)
        .sort((left, right) => left.label.localeCompare(right.label)),
      project,
    };
    bands.push(band);
    entities.set(band.key, band);
    const inside = [];
    bandNodes.set(band.key, inside);
    if (project.folded) continue;

    const workspaceEntities = new Map();
    for (const workspace of [...project.workspaces].sort((left, right) => left.row - right.row)) {
      const workspaceEntity = {
        key: workspace.key,
        type: "workspace",
        column: 1,
        row: workspace.row,
        label: workspace.label,
        line: workspace.branch && workspace.base
          ? `${workspace.branch} ← ${workspace.base}`
          : workspace.branch,
        meta: agentGraphCountWord("agent", workspace.agents.size),
        buckets: [...workspace.agents].sort(compareAgents).map((entry) => entry.bucket),
        idleCount: workspace.idleAgents.length,
        idleExpanded: workspace.idleExpanded,
        collapsed: workspace.collapsed,
        laneClass: workspace.laneClass,
        searchMatch: workspace.searchMatch,
        band,
        workspace,
      };
      const workspaceRows = [...workspace.agents]
        .map((entry) => rowOf.get(entry.card.pane))
        .filter(Number.isFinite);
      workspaceEntity.rowStart = Math.min(workspace.row, ...workspaceRows);
      workspaceEntity.rowEnd = Math.max(workspace.row, ...workspaceRows);
      workspaceEntities.set(workspace, workspaceEntity);
      entities.set(workspaceEntity.key, workspaceEntity);
    }

    for (const entry of [...project.agents].sort(compareAgents)) {
      const depth = depthOf.get(entry.card.pane) ?? 0;
      maxDepth = Math.max(maxDepth, depth);
      /* 발치의 `Claude · 방금 · 2분째`. 시계는 카드의 발치와
       * **같은 손**을 쓴다(`cardClock`): 한 화면에서 같은 판을 그리는 두
       * 그림이 마지막 소식과 이 턴의 길이를 둘 다 같이 말한다. */
      const clock = cardClock(entry.card, entry.bucket, now);
      const entity = {
        ...entry,
        type: "agent",
        depth,
        column: AGENT_GRAPH_CONTEXT_LAYERS.length + depth + 1,
        row: rowOf.get(entry.card.pane) ?? nextRow++,
        laneClass: entry.workspace.laneClass,
        label: agentGraphIdentity(entry.card),
        /* 대화만. 상태 낱말은 눈썹줄이 늘 들고 있으므로(`updateAgentGraphNode`)
         * 여기에 대체값으로 두면 말수 없는 카드 하나가 같은 낱말을 두 줄에
         * 겹쳐 쓴다. */
        state: entry.state,
        line: "",
        clock,
        /* 카드가 말하는 일곱, 한 손으로 (t-2374). 그래프 노드와 인스펙터
         * 카드가 같은 이 묶음을 그린다 — 두 함수가 같은 사실을 따로 지으면
         * 한 화면의 두 그림이 같은 에이전트를 다르게 말한다. */
        facts: agentCardFacts(entry.card, entry.bucket, now),
        /* Both halves, and the state is the one that decides.
         *
         * The ring can end on a `started` that was never closed: the backend
         * throttles by riding the NEXT envelope, so a call that finishes and
         * then goes quiet leaves its `finished` sitting unsent. Read alone,
         * that says a finished agent is still working — a halo pulsing on a
         * card that is done. The bucket is the backend's own answer to "is
         * this agent running", so it is the gate, and the ring only says
         * whether the running agent has a call open right now. */
        live: entry.bucket === "working" && agentGraphIsLive(entry.card.pane),
        wentQuiet: entry.bucket === "working" && entry.card.at > 0 &&
          now - entry.card.at >= AGENT_GRAPH_QUIET_AFTER_MS,
        hiddenByIdle: entry.state === "idle" && !entry.workspace.idleExpanded &&
          previous?.entities?.get(entry.key)?.hiddenByIdle !== false,
        hiddenByWorkspace: entry.workspace.collapsed,
        searchMatch: entry.searchMatch,
        band,
      };
      entity.line = agentGraphMessage(entry.card, entity.state);
      agentNodes.set(entity.key, entity);
      entities.set(entity.key, entity);
      edges.push({
        key: `${entry.workspace.key}>${entry.key}`,
        from: entry.workspace.key,
        to: entry.key,
        type: "contains",
      });
      const parent = byPane.get(entry.card.parent);
      if (parent) {
        edges.push({
          key: `${parent.key}>${entry.key}:spawned`,
          from: parent.key,
          to: entry.key,
          type: "spawned",
        });
      }
    }

    /* DOM order is CONTAINMENT order, because the surface calls itself a tree
     * and a tree is read parent-first: the band, then a workspace, then the
     * agents that workspace holds with each agent's own helpers right behind
     * it. Both other candidates were worse in the same way — "every band head,
     * then every workspace, then every agent" is three lists nobody drew, and
     * strict top-to-bottom splits a parent from the child it spawned whenever
     * the child's row happens to sit above it. */
    for (const workspace of [...project.workspaces].sort((left, right) => left.row - right.row)) {
      if (workspace.collapsed) continue;
      inside.push(workspaceEntities.get(workspace));
      const held = [...workspace.agents]
        .sort(compareAgents)
        .map((entry) => agentNodes.get(entry.key))
        .filter((entity) => entity && !entity.hiddenByIdle && !entity.hiddenByWorkspace);
      const mine = new Map(held.map((entity) => [entity.card.pane, entity]));
      const seen = new Set();
      const walk = (entity) => {
        if (seen.has(entity.card.pane)) return;
        seen.add(entity.card.pane);
        inside.push(entity);
        for (const child of children.get(entity.card.pane) ?? []) {
          const below = mine.get(child.card.pane);
          if (below) walk(below);
        }
      };
      for (const entity of held) {
        const above = byPane.get(entity.card.parent);
        if (!above || !mine.has(above.card.pane)) walk(entity);
      }
      // A cycle inside one workspace reaches nobody from a root. Walked again
      // from the front rather than dropped: a card missing from the tree is a
      // running agent a person cannot tab to.
      for (const entity of held) walk(entity);
    }
  }

  const nodes = bands.flatMap((band) => bandNodes.get(band.key) ?? []);
  const drawn = agents.map((entry) => agentNodes.get(entry.key)).filter(Boolean);
  const visibleKeys = new Set(nodes.map((entity) => entity.key));
  const visibleEdges = edges.filter((edge) => visibleKeys.has(edge.from) && visibleKeys.has(edge.to));
  const mail = (Array.isArray(overlays?.mail) ? overlays.mail : [])
    .map((edge) => ({
      key: `overlay:mail:${agentGraphAgentKey(edge.from)}>${agentGraphAgentKey(edge.to)}`,
      from: agentGraphAgentKey(edge.from),
      to: agentGraphAgentKey(edge.to),
      type: "mail",
      overlay: true,
      count: Math.max(1, Number(edge.count) || 1),
      unread: Math.max(0, Number(edge.unread) || 0),
      at: Number(edge.at) || 0,
      evidence: edge.last_message ?? null,
    }));
  const dependencies = (Array.isArray(overlays?.dependencies) ? overlays.dependencies : [])
    .map((edge) => ({
      key: `overlay:dependency:${agentGraphAgentKey(edge.from)}>${agentGraphAgentKey(edge.to)}`,
      from: agentGraphAgentKey(edge.from),
      to: agentGraphAgentKey(edge.to),
      type: "dependency",
      overlay: true,
      count: Math.max(1, Number(edge.count) || 1),
      at: Number(edge.at) || 0,
      verb: agentGraphCleanText(edge.verb) || "blocks",
    }));
  for (const edge of mail) {
    const recipient = entities.get(edge.to);
    if (recipient?.type === "agent") {
      recipient.mailUnread = (recipient.mailUnread ?? 0) + edge.unread;
    }
  }
  const mergeByWorkspace = new Map();
  for (const merge of Array.isArray(overlays?.merge) ? overlays.merge : []) {
    const workspace = [...workspacesAt.values()].find((candidate) =>
      candidate.path === merge.workspace || candidate.key === merge.workspace);
    if (!workspace) continue;
    mergeByWorkspace.set(workspace.key, {
      base: agentGraphCleanText(merge.base) || workspace.base || "main",
      ahead: Math.max(0, Number(merge.ahead) || 0),
      behind: Math.max(0, Number(merge.behind) || 0),
      landed: merge.landed === true,
    });
  }
  /* The active checkout already has a cached upstream reading on the SCM
   * surface. Reuse it; never run git from the graph, and never claim a landed
   * state from zero distance alone. */
  for (const workspace of workspacesAt.values()) {
    if (mergeByWorkspace.has(workspace.key) || workspace.path !== activeWorktreePath ||
        upstreamState === null) continue;
    const ahead = Math.max(0, Number(upstreamState.ahead) || 0);
    const behind = Math.max(0, Number(upstreamState.behind) || 0);
    if (ahead === 0 && behind === 0) continue;
    mergeByWorkspace.set(workspace.key, {
      base: workspace.base || "main",
      ahead,
      behind,
      landed: false,
    });
  }
  for (const entity of entities.values()) {
    if (entity.type === "workspace") entity.merge = mergeByWorkspace.get(entity.key) ?? null;
  }
  const overlayEdges = agentGraphOverlayMode === "mail"
    ? mail.filter((edge) => visibleKeys.has(edge.from) && visibleKeys.has(edge.to))
    : agentGraphOverlayMode === "dependency"
      ? dependencies.filter((edge) =>
          visibleKeys.has(edge.from) && visibleKeys.has(edge.to) &&
          (edge.from === agentGraphSelectedKey || edge.to === agentGraphSelectedKey))
      : [];
  const overlayLatest = agentGraphAgentKey(overlays?.latest);
  return {
    nodes,
    bands,
    /* The same nodes, still grouped by the band they belong to. The paint needs
     * this and not just the flat list: it builds one keyed child list for the
     * whole grid, and appending every heading before any of the cards puts the
     * SECOND project's heading directly after the first one — so a keyboard
     * walking down from a project it just opened lands on the next project
     * instead of on the workspace inside it. */
    nodesByBand: bandNodes,
    /* An edge with a folded-away end is not an edge this paint can draw.
     * Dropped here rather than skipped at the SVG, so the relation count the
     * legend reads is the count actually on the screen. */
    edges: visibleEdges,
    allEdges: edges,
    overlayEdges,
    overlayData: { mail, dependencies, mergeByWorkspace, latest: overlayLatest },
    overlayMode: agentGraphOverlayMode,
    overlaySignature: overlayEdges.map((edge) =>
      `${edge.key}:${edge.count}:${edge.unread ?? 0}`).join("\u001e"),
    entities,
    agents,
    counts,
    maxDepth,
    maxRow: Math.max(1, nextRow - 1),
    laneCount: AGENT_GRAPH_CONTEXT_LAYERS.length + Math.max(1, maxDepth + 1),
    liveCount: drawn.filter((entity) => !entity.hiddenByIdle && !entity.hiddenByWorkspace && entity.live).length,
    runCount: new Set(agents.map((entry) => entry.place.run).filter(Boolean)).size,
    searchMatchCount: agents.filter((entry) => entry.searchMatch).length,
    searchActive: needle !== "",
    /* Attention first, and the columns already rank it that way. A folded band
     * over that agent falls through to the first one actually on the screen —
     * an inspector opened on an entity nobody can see is a dead panel. */
    // An inventory-only graph has valid destinations but no urgent default.
    // Leaving selection empty keeps the inspector honest and prevents a
    // background catalog repaint from manufacturing user selection.
    defaultKey: drawn.find((entity) => !entity.hiddenByIdle && !entity.hiddenByWorkspace)?.key ?? null,
    topologySignature,
    layout: {
      rowOf: new Map(rowOf),
      depthOf: new Map(depthOf),
      projects: new Map(orderedProjects.map((project) => [project.key, {
        headRow: project.headRow,
        rowEnd: project.rowEnd,
      }])),
      workspaces: new Map([...workspacesAt.values()].map((workspace) => [
        workspace.key,
        workspace.row,
      ])),
      nextRow,
    },
    layoutRecomputed: cachedLayout === null,
    scoped,
    source: { columns, places, reviews, catalog: knownCatalog, search, overlays },
  };
}

/* ---- the words the ontology uses ---------------------------------------- */

/* "1 workspaces" is what one shared key gets you.
 *
 * Korean, Japanese and Chinese count with one form and English and Spanish
 * count with two, so the catalog carries both and the count picks — rather than
 * a formatter this window would then have to teach every other counted string.
 * This one is worth the second key because the band rail is sticky: it is the
 * summary a person reads on every visit, on every project, all day. */
function agentGraphCountWord(kind, count) {
  const many = kind === "workspace"
    ? t("board.graph.workspaceCount", "워크스페이스 {{count}}개", { count })
    : t("board.graph.agentCount", "에이전트 {{count}}개", { count });
  if (count !== 1) return many;
  return kind === "workspace"
    ? t("board.graph.workspaceCountOne", "워크스페이스 1개")
    : t("board.graph.agentCountOne", "에이전트 1개");
}

function agentGraphIdleCountWord(count) {
  return t("board.graph.idleCount", "+{{count}} 대기", { count });
}

function agentGraphIdleActionWord(workspace) {
  return workspace.idleExpanded
    ? t("board.graph.collapseIdle", "{{name}}의 대기 에이전트 접기", {
        name: workspace.label,
      })
    : t("board.graph.expandIdle", "{{name}}의 대기 에이전트 펼치기", {
        name: workspace.label,
      });
}

/* Rebuild from the model's own complete snapshot, not another backend read.
 * Expanding inventory is a local presentation gesture; classification and the
 * catalog cannot have changed between pointerdown and this paint. */
function agentGraphFullModel(view, model = agentGraphModels.get(view)) {
  return agentGraphScopeSources.get(model) ?? model;
}

function agentGraphTasksFor(view, model) {
  const full = agentGraphFullModel(view, model);
  const held = agentGraphTaskScopes.get(view);
  if (held?.source === full) return held.tasks;
  const tasks = taskBoardModel(full, held?.tasks);
  agentGraphTaskScopes.set(view, { source: full, tasks });
  return tasks;
}

function agentGraphRelations(model) {
  if (model.relationList) return model.relationList;
  const full = agentGraphScopeSources.get(model) ?? model;
  const drawn = [...(full.allEdges ?? full.edges), ...(model.overlayData?.mail ?? []),
    ...(model.overlayData?.dependencies ?? [])];
  const connected = new Set((model.overlayData?.dependencies ?? []).map((edge) => JSON.stringify([edge.from, edge.to])));
  const facts = model.source.overlays.task_dependencies ?? [];
  for (const fact of facts) {
    const from = fact.from ? agentGraphAgentKey(fact.from) : null;
    const to = agentGraphAgentKey(fact.to);
    if (from && connected.has(JSON.stringify([from, to]))) continue;
    if (!model.entities.has(to)) continue;
    drawn.push({ key: `task-dependency:${JSON.stringify([fact.run, fact.dependency, fact.task])}`,
      type: "dependency", from, to, fact, outside: true });
  }
  model.relationList = drawn;
  return drawn;
}

function agentGraphDependencyFacts(model, relation) {
  if (relation.fact) return [relation.fact];
  return (model.source.overlays.task_dependencies ?? []).filter((fact) =>
    (fact.from ? agentGraphAgentKey(fact.from) : null) === relation.from
    && agentGraphAgentKey(fact.to) === relation.to);
}

function selectAgentGraphRelation(view, key) {
  const full = agentGraphFullModel(view);
  const visible = agentGraphModels.get(view);
  const relation = visible && agentGraphRelations(visible).find((edge) => edge.key === key)
    || full && agentGraphRelations(full).find((edge) => edge.key === key);
  if (!relation) return;
  agentGraphSelectedEdgeKey = key;
  if (["mail", "dependency"].includes(relation.type)) agentGraphOverlayMode = relation.type;
  else agentGraphOverlayMode = "none";
  agentGraphInspectorTab = "relations";
  if (visible.entities.has(relation.to) || full.entities.has(relation.to)) agentGraphSelectedKey = relation.to;
  setAgentGraphInspectorOpen(view, true);
  // An edge can be chosen from the full relationship list while its target
  // lies outside the current task. Reveal that scope explicitly.
  if (agentGraphScopeKey && !agentGraphModels.get(view)?.entities.has(agentGraphSelectedKey)) {
    agentGraphScopeKey = "";
    agentGraphScopeLabel = "";
  }
  const source = full.source;
  const model = agentGraphModel(source.columns, source.places, source.reviews, Date.now(), full,
    source.catalog, source.search, source.overlays);
  paintAgentGraph(view, model);
  void syncPreviewedTerms();
}

function agentGraphScopedModel(view, full) {
  if (agentGraphScopeKey === "") return full;
  const group = agentGraphTasksFor(view, full).groups.find((one) => one.key === agentGraphScopeKey);
  if (group) agentGraphScopeLabel = group.title;
  const members = group?.members ?? [];
  const panes = new Set(members.map((entry) => entry.card.pane));
  const workspaces = new Set(members.map((entry) => entry.workspace.path));
  const source = full.source;
  const columns = source.columns.map((column) => ({ ...column,
    cards: column.cards.filter((card) => panes.has(card.pane)) }));
  const catalog = (source.catalog ?? []).map((project) => ({ ...project,
    worktrees: (project.worktrees ?? []).filter((workspace) => workspaces.has(workspace.path)) }))
    .filter((project) => project.worktrees.length > 0 || members.some((entry) => entry.project.path === project.path));
  const scoped = agentGraphModel(columns, source.places, source.reviews, Date.now(),
    agentGraphModels.get(view), catalog, source.search, source.overlays, { scoped: true });
  agentGraphScopeSources.set(scoped, full);
  return scoped;
}

function setAgentGraphScope(view, key) {
  const full = agentGraphFullModel(view);
  if (!full) return;
  const group = agentGraphTasksFor(view, full).groups.find((one) => one.key === key);
  if (key && !group) return;
  agentGraphScopeKey = key;
  agentGraphScopeLabel = group?.title ?? "";
  agentGraphScopeFolded.clear();
  agentGraphScopeIdleCollapsed.clear();
  agentGraphSelectedEdgeKey = null;
  if (group && !group.members.some((entry) => entry.key === agentGraphSelectedKey)) {
    agentGraphSelectedKey = group.lead?.key ?? group.root.key;
  }
  agentGraphZoomTaken = false;
  paintAgentGraph(view, full);
  void syncPreviewedTerms();
}

/* 범위 하나, 답 둘 (t-4145). 시안의 사이드바는 작업 목록이고 툴바의 범위
 * 선택도 같은 목록이다 — 둘 다 여기 한 표(`choices`)에서 그려지므로 한쪽만
 * 다른 작업을 아는 날은 오지 않는다. 레일 항목의 작은 줄은 시안의 그 두 마디:
 * 에이전트 수, 그리고 확인 필요가 있으면 그 수, 없으면 묶음의 상태 낱말. */
function agentRelationsRailItem() {
  const button = taskBoardElement("button", "agent-relations-rail-item");
  button.type = "button";
  const copy = taskBoardElement("span", "agent-relations-rail-copy");
  copy.append(taskBoardElement("b", ""), taskBoardElement("small", ""));
  button.append(taskBoardElement("span", "agent-relations-rail-mark"), copy);
  return button;
}

function agentRelationsScopeChoices(full, tasks) {
  const all = { key: "", title: t("board.graph.allExecutions", "전체 실행"), count: full.agents.length, bucket: "",
    attention: full.counts.get("attention") ?? 0, label: t("board.graph.allExecutions", "전체 실행") };
  const groups = tasks.groups.map((group) => ({ key: group.key, title: group.title, count: group.members.length, bucket: group.bucket,
    attention: group.members.filter((entry) => entry.bucket === "attention").length, label: `${group.title} · ${group.members.length}` }));
  const choices = [all, ...groups];
  if (agentGraphScopeKey && !tasks.groups.some((group) => group.key === agentGraphScopeKey)) {
    choices.push({ key: agentGraphScopeKey, title: agentGraphScopeLabel, count: 0, bucket: "", attention: 0, label: agentGraphScopeLabel });
  }
  return choices;
}

function agentRelationsRailLine(choice, full) {
  const state = choice.attention > 0 ? `${t("board.attention", "확인 필요")} ${choice.attention}`
    : choice.bucket ? agentGraphStateWord(choice.bucket === "attention" ? "needs-attention" : choice.bucket)
      : `${t("board.graph.run", "런")} ${full.runCount}`;
  return `${agentGraphCountWord("agent", choice.count)} · ${state}`;
}

function paintAgentRelationsControls(view, full, model) {
  const tasks = agentGraphTasksFor(view, full);
  const choices = agentRelationsScopeChoices(full, tasks);
  const scope = view.querySelector(".agent-graph-scope");
  if (scope) {
    const existing = new Map([...scope.options].map((option) => [option.value, option]));
    reconcileElementOrder(scope, choices.map(({ key, label }) => {
      const option = existing.get(key) ?? document.createElement("option");
      option.value = key;
      writeTextContent(option, label);
      return option;
    }));
    scope.value = agentGraphScopeKey;
    scope.onchange = () => setAgentGraphScope(view, scope.value);
  }
  /* 레일은 내용이 같으면 손대지 않는다 — 훅 하나마다 지나는 길이고, 항목마다
   * 낱말 둘과 손 하나를 다시 맞추는 값은 아무것도 움직이지 않은 판에서는 낭비다.
   * 복제된 판(docHost)은 서명을 들고 오지만 손은 들고 오지 않으므로 손이
   * 없으면 서명과 무관하게 다시 맨다. */
  const rail = view.querySelector(".agent-relations-rail-list");
  const railSignature = JSON.stringify([locale, agentGraphScopeKey, full.runCount, choices]);
  if (rail && (rail.dataset.railSignature !== railSignature || !rail.firstElementChild?.onclick)) {
    rail.dataset.railSignature = railSignature;
    const existing = new Map([...rail.children].map((item) => [item.dataset.scope, item]));
    reconcileElementOrder(rail, choices.map((choice) => {
      const item = existing.get(choice.key) ?? agentRelationsRailItem();
      writeAttribute(item, "data-scope", choice.key);
      writeAttribute(item, "aria-pressed", String(agentGraphScopeKey === choice.key));
      writeClassName(item.firstElementChild, `agent-relations-rail-mark${choice.bucket ? ` is-${choice.bucket}` : ""}`);
      writeTextContent(item.querySelector("b"), choice.title);
      writeTextContent(item.querySelector("small"), agentRelationsRailLine(choice, full));
      item.onclick = () => setAgentGraphScope(view, choice.key);
      return item;
    }));
  }
  const title = view.querySelector(".agent-graph-scope-title");
  if (title) writeTextContent(title, agentGraphScopeKey ? agentGraphScopeLabel : t("board.graph.allExecutions", "전체 실행"));
  const copy = view.querySelector(".agent-graph-scope-copy");
  if (copy) writeTextContent(copy, agentGraphScopeKey
    ? t("board.graph.scopeCopy", "누가 시작했고, 어떤 결과를 기다리는지 확인하세요.")
    : t("board.graph.allExecutionsCopy", "작업별 관계를 유지하면서 진행 중인 실행을 함께 살펴봅니다."));
  const count = view.querySelector(".agent-graph-scope-count");
  if (count) writeTextContent(count, agentGraphScopeKey
    ? t("board.graph.scopeCount", "{{shown}} / {{total}} 에이전트", { shown: model.agents.length, total: full.agents.length })
    : agentGraphCountWord("agent", full.agents.length));
  const density = view.querySelector(".agent-graph-density");
  if (density) {
    writeAttribute(density, "aria-pressed", String(agentGraphCardDetails));
    density.onclick = () => { agentGraphCardDetails = !agentGraphCardDetails; paintAgentGraph(view, full); scheduleAgentGraphEdges(view); };
  }
  for (const button of view.querySelectorAll("[data-agent-inspector-tab]")) {
    writeAttribute(button, "aria-selected", String(button.dataset.agentInspectorTab === agentGraphInspectorTab));
    button.tabIndex = button.dataset.agentInspectorTab === agentGraphInspectorTab ? 0 : -1;
    button.onclick = () => {
      agentGraphInspectorTab = button.dataset.agentInspectorTab;
      paintAgentGraph(view, agentGraphFullModel(view));
      void syncPreviewedTerms();
    };
    button.onkeydown = (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const tabs = [...view.querySelectorAll("[data-agent-inspector-tab]")];
      const index = tabs.indexOf(button);
      const next = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1
        : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
      tabs[next].click();
      tabs[next].focus();
    };
  }
}

function agentGraphToggleIdle(view, workspaceKey) {
  const preference = agentGraphScopeKey ? agentGraphScopeIdleCollapsed : agentGraphIdleExpanded;
  if (preference.has(workspaceKey)) preference.delete(workspaceKey);
  else preference.add(workspaceKey);
  const previous = agentGraphFullModel(view);
  const source = previous?.source;
  if (!source) return;
  const model = agentGraphModel(
    source.columns,
    source.places,
    source.reviews,
    Date.now(),
    previous,
    source.catalog,
    source.search,
    source.overlays,
  );
  paintAgentGraph(view, model);
}

function agentGraphSetOverlay(view, mode) {
  if (!AGENT_GRAPH_OVERLAYS.has(mode)) return false;
  agentGraphOverlayMode = agentGraphOverlayMode === mode ? "none" : mode;
  const previous = agentGraphFullModel(view);
  const source = previous?.source;
  if (!source) return false;
  const model = agentGraphModel(
    source.columns,
    source.places,
    source.reviews,
    Date.now(),
    previous,
    source.catalog,
    source.search,
    source.overlays,
  );
  paintAgentGraph(view, model);
  return true;
}

function agentGraphPauseFollow(view) {
  if (!agentGraphFollowing) return;
  agentGraphFollowing = false;
  const follow = view?.querySelector(".agent-graph-follow");
  if (follow) {
    follow.setAttribute("aria-pressed", "false");
    follow.classList.remove("is-active");
  }
}

function agentGraphKindWord(type) {
  if (type === "project") return t("board.graph.project", "프로젝트");
  if (type === "workspace") return t("board.graph.workspace", "워크스페이스");
  return t("board.graph.agent", "에이전트");
}

/* What an edge MEANS, in a word. Drawn on the selected chain and named in the
 * legend: a line nobody can name is decoration, and this surface has two
 * relations that look alike at a glance and mean nothing alike. */
function agentGraphRelationWord(type) {
  if (type === "spawned") return t("board.graph.relSpawned", "시작함");
  if (type === "mail") return t("board.graph.mail", "메일");
  if (type === "dependency") return t("board.graph.dependency", "의존");
  return t("board.graph.relContains", "품음");
}

function agentGraphLaneWord(lane) {
  if (lane < AGENT_GRAPH_CONTEXT_LAYERS.length) {
    return agentGraphKindWord(AGENT_GRAPH_CONTEXT_LAYERS[lane]);
  }
  const depth = lane - AGENT_GRAPH_CONTEXT_LAYERS.length;
  if (depth === 0) return t("board.graph.agent", "에이전트");
  return t("board.graph.subagentLane", "하위 {{depth}}단", { depth });
}

function agentGraphNodeIcon(entity) {
  const host = document.createElement("span");
  host.className = "agent-graph-node-icon";
  if (entity.type === "agent") {
    host.appendChild(
      agentIcon(agentRows.find((row) => row.id === entity.card.agent) ?? {
        id: entity.card.agent,
        name: agentName(entity.card.agent),
      }),
    );
    return host;
  }
  host.innerHTML = icon(entity.type === "project" ? "folder" : "branch");
  return host;
}

/* ---- the live wire ------------------------------------------------------ */

/* One tool call, drawn. The verb is translated and the target never is — a
 * path, a command and a query are the machine's own words, and the line a
 * person supervising five agents reads is "실행 · cargo test --workspace". */
/* A path is identified by its TAIL and a command by its HEAD, so the two want
 * opposite ends cut. Only the path is cut here: the command's end-clip is one
 * CSS declaration and needs no arithmetic, while cutting a path in CSS means a
 * right-to-left box — and a bidi flip moves the leading `/` of an absolute path
 * to the other end, which draws a path that does not exist. */
function agentGraphTargetWord(target) {
  // A space means an argument list, not a path — `cargo test ./crates` is a
  // command whose first word is the part worth keeping.
  if (target.includes(" ") || !target.includes("/")) return target;
  const parts = target.split("/").filter(Boolean);
  return parts.length < 2 ? target : `\u2026/${parts.slice(-2).join("/")}`;
}

/* A command belongs in a log; a card needs its executable and at most one
 * useful operand. Prefer the command inside a substitution (`$(df ...)`) over
 * its surrounding shell loop, discard switches and shell punctuation, and do
 * no shell parsing or execution. The untouched source stays in `data-tip`. */
function agentGraphCommandWord(target) {
  const source = agentGraphCleanText(target);
  const substitution = source.match(/\$\(\s*([\w./:-]+)([^)]*)/);
  const words = (substitution ? `${substitution[1]}${substitution[2]}` : source)
    .split(/\s+/)
    .filter(Boolean);
  const shellWords = new Set(["if", "then", "else", "elif", "fi", "for", "in",
    "while", "until", "do", "done", "case", "esac", "time", "command", "env", "sudo"]);
  const safe = (word) => word.replace(/^["']+|["',;|&()<>`]+$/g, "");
  let command = "";
  let operand = "";
  for (const raw of words) {
    if (/^[|;&]/.test(raw)) {
      if (command) break;
      continue;
    }
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(raw)) continue;
    const word = safe(raw);
    if (!word || /[$|;&()<>`]/.test(word) || shellWords.has(word)) continue;
    if (!command) {
      command = word.split("/").filter(Boolean).at(-1) ?? word;
      continue;
    }
    if (!raw.startsWith("-")) {
      operand = agentGraphTargetWord(word);
      break;
    }
  }
  return [command, operand].filter(Boolean).join(" ");
}

function agentGraphActivityTarget(verb, target) {
  const clean = agentGraphCleanText(target);
  return agentActivityKind(verb) === "run"
    ? agentGraphCommandWord(clean)
    : agentGraphTargetWord(clean);
}

function agentGraphBeatNode(beat) {
  const row = document.createElement("details");
  row.className = `agent-graph-beat is-${beat.phase}`;
  row.dataset.verb = beat.verb;
  const summary = document.createElement("summary");
  summary.className = "agent-graph-beat-summary";
  const mark = document.createElement("span");
  mark.className = "agent-graph-beat-mark";
  mark.setAttribute("aria-hidden", "true");
  const markName = beat.verb === "bash"
    ? "terminal"
    : ["edit", "write"].includes(beat.verb)
      ? "pencil"
      : ["prompt", "stop", "task"].includes(beat.verb)
        ? "message-square"
        : "activity";
  mark.innerHTML = icon(markName);
  const verb = document.createElement("span");
  verb.className = "agent-graph-beat-verb";
  verb.textContent = activityWord(beat.verb);
  summary.append(mark, verb);
  if (beat.target) {
    const target = document.createElement("span");
    target.className = "agent-graph-beat-target";
    if (beat.verb === "bash") target.classList.add("is-mono");
    target.textContent = agentGraphActivityTarget(beat.verb, beat.target);
    target.dataset.tip = beat.target;
    summary.appendChild(target);
  }
  if (beat.repeat > 1) {
    const repeat = document.createElement("span");
    repeat.className = "agent-graph-beat-repeat";
    repeat.textContent = `×${beat.repeat}`;
    summary.appendChild(repeat);
  }
  const when = document.createElement("span");
  when.className = "agent-graph-beat-when";
  when.textContent = beat.at > 0
    ? agoWord(beat.at, Date.now())
    : t("board.graph.recent", "최근");
  summary.appendChild(when);
  const raw = document.createElement("code");
  raw.className = "agent-graph-beat-raw";
  raw.textContent = beat.target || activityWord(beat.verb);
  row.append(summary, raw);
  return row;
}

/* ---- 그 사실을 그리는 한 손 (t-2374) ------------------------------------
 *
 * 짓는 일과 입히는 일이 갈라져 있다. 짓는 것은 노드가 태어날 때 한 번이고,
 * 입히는 것은 훅 하나마다다 — 그래서 활동이 백 번 바뀌어도 만들어지는 노드는
 * 0이고 쓰이는 것은 글자·속성·인라인 값뿐이다(설계 §5). 띠의 여덟 칸을 미리
 * 지어 두는 것도 같은 규칙이다: 링 버퍼라 칸의 수는 절대 움직이지 않고,
 * 도는 것은 글리프와 클래스뿐이다.
 *
 * 그래프 노드와 인스펙터 카드가 이 둘을 **같이** 쓴다. `is-graph`는 이 블록이
 * 그래프의 축척(`--agent-graph-text-*`) 안에 서 있다는 표시일 뿐, 무엇을 그릴지
 * 고르는 자리가 아니다. */

/* 커스텀 속성은 `node.style[name]`으로 읽히지 않으므로 위의 셋과 문이 다르다.
 * 같은 약속을 지키려고 같은 모양으로 둔다: 같은 값이면 쓰지 않는다. */
function writeStyleProperty(node, name, value) {
  if (node.style.getPropertyValue(name) !== value) node.style.setProperty(name, value);
}

function agentCardDoingNode({ graph = false } = {}) {
  const doing = document.createElement("span");
  doing.className = graph ? "agent-card-doing agent-graph-wire" : "agent-card-doing";
  if (graph) doing.dataset.graphBlock = "1";

  const activity = document.createElement("span");
  activity.className = "agent-card-activity";
  const mark = document.createElement("span");
  mark.className = "agent-card-activity-mark";
  mark.setAttribute("aria-hidden", "true");
  const verb = document.createElement("span");
  verb.className = "agent-card-activity-verb";
  const target = document.createElement("span");
  target.className = "agent-card-activity-target";
  const when = document.createElement("span");
  when.className = "agent-card-activity-when";
  activity.append(mark, verb, target, when);

  /* 여덟 칸, 미리. `aria-hidden`인 것은 이 띠가 활동 줄이 이미 낱말로 말한
   * 것의 **모양**이기 때문이다 — 글리프 여덟을 낭독기가 하나씩 읽으면 그것은
   * 정보가 아니라 소음이고, 같은 사실의 전문은 인스펙터의 연대기가 든다. */
  const ticker = document.createElement("span");
  ticker.className = "agent-card-ticker";
  const more = document.createElement("span");
  more.className = "agent-card-ticker-more";
  ticker.appendChild(more);
  for (let at = 0; at < AGENT_GRAPH_ACTIVITY_KEEP; at += 1) {
    const tool = document.createElement("span");
    tool.className = "agent-card-tool is-empty";
    const toolVerb = document.createElement("span");
    toolVerb.className = "agent-card-tool-verb";
    const toolTarget = document.createElement("span");
    toolTarget.className = "agent-card-tool-target";
    tool.append(toolVerb, toolTarget);
    ticker.appendChild(tool);
  }
  const glyphs = document.createElement("span");
  glyphs.className = "agent-card-ticker-glyphs";
  glyphs.setAttribute("aria-hidden", "true");
  for (let at = 0; at < AGENT_GRAPH_TICKER_KEEP; at += 1) {
    const tick = document.createElement("i");
    tick.className = "agent-card-tick is-empty";
    glyphs.appendChild(tick);
  }
  ticker.appendChild(glyphs);

  const meter = document.createElement("span");
  meter.className = "agent-card-meter";
  meter.hidden = true;
  meter.setAttribute("role", "img");
  const fill = document.createElement("span");
  fill.className = "agent-card-meter-fill";
  meter.appendChild(fill);

  doing.append(activity, ticker, meter);
  return doing;
}

function dressAgentCardDoing(doing, facts, { askInline = true } = {}) {
  /* 아무 말도 없는 판은 이 블록을 세우지 않는다 — 테두리와 여백만 남은 상자는
   * "아무것도 모른다"를 "무언가 있는데 비었다"로 읽히게 한다. 노드를 지우는
   * 대신 클래스 하나인 것은 §5 그대로다: DOM은 하나이고, 다음 도구 호출은
   * 새 자식이 아니라 글자 몇 개로 돌아온다. */
  const said = askInline && facts.question ? facts.question : facts.activity;
  const bare = said === null && facts.ticks.length === 0 && facts.context === null;
  writeClassName(doing, [
    "agent-card-doing",
    doing.dataset.graphBlock === "1" ? "agent-graph-wire" : "",
    bare ? "is-bare" : "",
  ].filter(Boolean).join(" "));
  const activity = doing.querySelector(".agent-card-activity");
  writeClassName(activity, said
    ? `agent-card-activity is-${said.kind}${said === facts.question ? " is-asking" : ""}`
    : "agent-card-activity is-none");
  writeTextContent(activity.querySelector(".agent-card-activity-verb"), said?.word ?? "");
  const target = activity.querySelector(".agent-card-activity-target");
  writeTextContent(target, said?.target ?? "");
  writeAttribute(target, "data-tip", said?.tip ?? said?.target ?? "");
  writeTextContent(activity.querySelector(".agent-card-activity-when"), said?.when ?? "");
  /* 활동 줄이 **바뀔 때** 한 번 (설계 §4). 상시 애니메이션을 늘리지 않으려고
   * 이름 다른 두 keyframes를 번갈아 가리킨다 — 규칙이 갈리므로 브라우저가
   * 애니메이션을 새로 세운다. 타이머는 없다. */
  const drawn = `${said?.kind ?? ""}\u001f${said?.word ?? ""}\u001f${said?.target ?? ""}`;
  if (activity.dataset.beatSaid !== drawn) {
    activity.dataset.beatSaid = drawn;
    writeAttribute(activity, "data-beat", activity.dataset.beat === "a" ? "b" : "a");
  }

  const trail = doing.querySelectorAll(".agent-card-tool");
  trail.forEach((tool, at) => {
    const fact = facts.trail[at] ?? null;
    writeClassName(tool, fact ? `agent-card-tool is-${fact.kind}` : "agent-card-tool is-empty");
    writeTextContent(tool.querySelector(".agent-card-tool-verb"), fact?.word ?? "");
    writeTextContent(tool.querySelector(".agent-card-tool-target"), fact?.target ?? "");
    writeAttribute(tool, "data-tip", fact?.tip ?? "");
  });
  writeTextContent(
    doing.querySelector(".agent-card-ticker-more"),
    facts.trailHidden > 0 ? `+${facts.trailHidden}` : "",
  );
  const ticker = doing.querySelector(".agent-card-ticker");
  writeAttribute(ticker, "data-tip", facts.ticks.map((fact) => fact.tip).join(" · "));
  /* 도구가 하나뿐인 카드의 띠는 활동 줄의 메아리다 (round three) — 같은 낱말을
   * 한 줄 아래 작은 글씨로 한 번 더 적는 것. 둘째 도구가 오면 다시 선다. */
  writeClassName(ticker, facts.trail.length === 1 && facts.trailHidden === 0
    ? "agent-card-ticker is-echo"
    : "agent-card-ticker");

  const ticks = doing.querySelectorAll(".agent-card-tick");
  const held = facts.ticks;
  const first = ticks.length - held.length;
  ticks.forEach((tick, at) => {
    const fact = at >= first ? held[at - first] : null;
    writeClassName(tick, fact
      ? `agent-card-tick is-${fact.kind}${at === ticks.length - 1 ? " is-last" : ""}`
      : "agent-card-tick is-empty");
    writeTextContent(tick, fact?.glyph ?? "");
    writeAttribute(tick, "data-kind", fact?.kind ?? "");
    writeAttribute(tick, "data-repeat", fact?.repeat ?? "");
    writeAttribute(tick, "data-tip", fact?.tip ?? "");
  });

  const meter = doing.querySelector(".agent-card-meter");
  const context = facts.context;
  meter.hidden = context === null;
  writeStyleProperty(meter, "--meter-ratio", context ? String(Math.round(context.ratio * 1e4) / 1e4) : "");
  writeClassName(meter, context && context.ratio >= AGENT_GRAPH_CONTEXT_FULL
    ? "agent-card-meter is-full"
    : "agent-card-meter");
  writeAttribute(meter, "aria-label", context
    ? t("board.card.context", "컨텍스트 {{used}} / {{all}}", {
        used: context.tokens.toLocaleString(), all: context.window.toLocaleString(),
      })
    : "");
}


/* The pips a workspace wears: one per agent inside it, in that agent's own
 * state colour. A workspace holding four agents where one of them wants an
 * answer says so on the workspace, so a deep lineage scrolled off to the right
 * still has its state visible at the left edge. */
function agentGraphPipsNode(buckets) {
  const pips = document.createElement("span");
  pips.className = "agent-graph-pips";
  pips.setAttribute("aria-hidden", "true");
  for (const bucket of buckets.slice(0, 8)) {
    const pip = document.createElement("i");
    pip.className = `agent-graph-pip is-${bucket}`;
    pips.appendChild(pip);
  }
  if (buckets.length > 8) {
    const more = document.createElement("span");
    more.className = "agent-graph-pip-more";
    more.textContent = `+${buckets.length - 8}`;
    pips.appendChild(more);
  }
  return pips;
}

/* ---- nodes -------------------------------------------------------------- */

/* Draws one node, and answers whether its CONTENTS were rebuilt.
 *
 * The caller needs that answer and not just "did the topology move": a live
 * wire appearing under a card makes the card taller, which moves every row
 * below it — while the topology, the selection, and (on a graph smaller than
 * the canvas it is drawn on) the canvas's own size all stay exactly as they
 * were. Edges measured before that rebuild point at where the nodes used to be,
 * and stay there until something unrelated happens to move them. */
function updateAgentGraphNode(node, entity, view) {
  const selected = entity.key === agentGraphSelectedKey;
  /* 입힐 옷을 먼저 한 문자열로 짓고, 지금 입은 것과 다를 때만 갈아입힌다.
   * `classList.add`를 네 번 부르면 아무것도 달라지지 않은 판에도 이 노드의
   * 스타일이 네 번 무효가 된다 — 노드가 백 개인 표면에서 그것이 값이다. */
  const dress = ["agent-graph-node", `is-${entity.type}`];
  if (entity.laneClass) dress.push(entity.laneClass);
  if (selected) dress.push("is-selected");
  if (entity.relationEndpoint) dress.push("is-relation-endpoint");
  if (entity.searchMatch === false) dress.push("is-search-dimmed");
  if (entity.type === "agent") {
    dress.push(`is-${entity.bucket}`);
    if (entity.state !== entity.bucket) dress.push(`is-${entity.state}`);
    if (entity.card.unseen) dress.push("is-unseen");
    if (entity.live) dress.push("is-live");
    if (entity.wentQuiet) dress.push("is-went-quiet");
  }
  writeClassName(node, dress.join(" "));
  writeAttribute(node, "data-graph-key", entity.key);
  writeAttribute(node, "data-graph-type", entity.type);
  writeAttribute(node, "data-search-match", String(entity.searchMatch !== false));
  if (entity.type === "agent") writeAttribute(node, "data-agent-state", entity.state);
  else if (node.hasAttribute("data-agent-state")) node.removeAttribute("data-agent-state");
  writeStyleValue(node, "gridColumn", String(entity.column));
  writeStyleValue(node, "gridRow", String(entity.row));
  /* 이 노드가 몇 번째 층인가 — 좌표가 아니라 사실이다. 넓은 판에서는 열이
   * 그것을 말하고 목록 모드에서는 들여쓰기가 말하지만, 어느 쪽을 그릴지 고르는
   * 것은 CSS다. 노드는 제 깊이를 적을 뿐 티어를 모른다 (t-2374). */
  writeStyleProperty(node, "--graph-depth", String(Math.max(0, entity.column - 1)));
  writeAttribute(node, "aria-selected", String(selected));
  /* The tree this markup declares is the CONTAINMENT one — project, workspace,
   * agent — and it has exactly three levels no matter how deep a lineage gets.
   * Counting the agent lanes instead would announce a helper as level 4 under a
   * workspace that is level 2, with no level 3 between them, and would disagree
   * with where ArrowLeft actually goes (its own workspace). Who spawned whom is
   * the other relation on this surface, and it is said in the inspector and
   * drawn as a dashed edge rather than folded into this ladder. */
  writeAttribute(
    node,
    "aria-level",
    String(AGENT_GRAPH_CONTEXT_LAYERS.length + (entity.type === "agent" ? 2 : 1)),
  );
  writeAttribute(node, "tabindex", selected ? "0" : "-1");
  /* 상태를 낱말로. 눈이 읽는 것은 카드 머리의 눈썹줄이고, 그 줄은 이 레이블에
   * 가려 화면 낭독기에 닿지 않는다 — 같은 사실을 두 길에 같이 실어야 한다. */
  writeAttribute(node, "aria-label", [
    agentGraphKindWord(entity.type),
    entity.type === "agent" ? agentGraphStateWord(entity.state) : "",
    entity.label,
  ].filter(Boolean).join(" · "));

  /* 서명 셋 — 무엇이 바뀌었고 무엇을 다시 해야 하는가 (1-t353, t-2374).
   *
   * `signature`는 이 노드가 그리는 모든 것, `stableSignature`는 그 중 **자리에
   * 써 넣을 수 있는 것을 뺀** 나머지, `geometrySignature`는 높이를 바꾸는 것들.
   * 셋째가 따로 있는 것은 첫째가 움직여도 간선이 그대로일 수 있고, 둘째가
   * 그대로여도 카드가 자랄 수 있기 때문이다 — 활동 줄 하나가 나타나면 그
   * 아래 모든 줄이 내려간다.
   *
   * t-2374이 둘째에서 걷어낸 것은 활동·단계·모델·컨텍스트다. 그 넷은 훅 하나에
   * 한 번씩 움직이는 값이고, 서명에 남겨 두면 「무엇을 하는 중인가」가 바뀔
   * 때마다 카드의 자식 전부가 다시 지어졌다 — 카드가 그 물음에 답하기 시작한
   * 바로 그 순간부터 매 박자가 전면 재건이 되는 셈이다. */
  const facts = entity.type === "agent" ? entity.facts : null;
  const volatileSignature = [
    entity.state ?? "",
    entity.bucket ?? "",
    entity.clock?.word ?? "",
    entity.clock?.running ?? "",
    facts?.phase ?? "",
    facts?.phaseWord ?? "",
    facts?.phaseTip ?? "",
    facts?.model ?? "",
    [facts?.question, facts?.activity]
      .map((one) => (one ? [one.kind, one.word, one.target, one.when].join("\u001f") : ""))
      .join("\u001d"),
    facts?.ticks.map((tick) => `${tick.kind}:${tick.repeat}:${tick.tip}`).join(",") ?? "",
    facts?.trail.map((tool) => `${tool.kind}:${tool.word}:${tool.target}`).join(",") ?? "",
    String(facts?.trailHidden ?? 0),
    facts?.context ? String(Math.round(facts.context.ratio * 1e4)) : "",
  ].join("\u001e");
  const stableSignature = [
    locale,
    entity.type,
    entity.label,
    entity.line,
    entity.card?.agent ?? "",
    entity.buckets?.join(",") ?? "",
    String(entity.idleCount ?? 0),
    String(entity.idleExpanded ?? false),
    String(entity.wentQuiet ?? false),
    String(entity.mailUnread ?? 0),
    entity.merge ? JSON.stringify(entity.merge) : "",
    entity.type === "workspace" ? agentGraphOverlayMode : "",
    String(entity.previousAttempt ?? false),
  ].join("\u001f");
  const signature = `${stableSignature}\u001d${volatileSignature}`;
  const geometrySignature = [
    entity.type,
    String(Boolean(entity.line)),
    String(Boolean(facts?.question ?? facts?.activity)),
    String((facts?.ticks.length ?? 0) > 0),
    String(Boolean(facts?.context)),
    String(entity.idleCount > 0),
    String(Boolean(entity.merge) && agentGraphOverlayMode === "merge"),
    String(Boolean(entity.meta)),
    String(Boolean(entity.wentQuiet)),
    String(entity.mailUnread > 0),
  ].join("\u001f");
  const geometryChanged = node.dataset.graphGeometry !== geometrySignature;
  if (node.dataset.graphSignature !== signature &&
      node.dataset.graphStable === stableSignature && entity.type === "agent") {
    node.dataset.graphSignature = signature;
    node.dataset.graphGeometry = geometrySignature;
    const mark = node.querySelector(".agent-graph-node-state");
    if (mark) dressAgentGraphStateMark(mark, entity.state);
    const state = node.querySelector(".agent-graph-state-word");
    if (state) writeTextContent(state, facts.stateWord);
    const phase = node.querySelector(".agent-card-phase");
    if (phase) {
      writeClassName(phase, `agent-card-phase is-${facts.phase || "none"}`);
      writeTextContent(phase, facts.phaseWord);
      writeAttribute(phase, "data-tip", facts.phaseTip);
    }
    const model = node.querySelector(".agent-card-model");
    if (model) writeTextContent(model, facts.model);
    const clocks = node.querySelectorAll(".agent-graph-node-clock");
    if (clocks[0]) writeTextContent(clocks[0], entity.clock?.word ?? "");
    if (clocks[1]) writeTextContent(clocks[1], entity.clock?.running ?? "");
    const doing = node.querySelector(".agent-card-doing");
    if (doing) dressAgentCardDoing(doing, facts);
    /* 자리에 써 넣었어도 **높이는** 바뀔 수 있다: 첫 도구 호출이 활동 줄을
     * 세우면 그 아래 줄이 전부 내려간다. 그 사실을 삼키면 간선은 카드가 한
     * 박자 전에 서 있던 자리를 가리킨 채로 남는다. */
    return geometryChanged;
  }
  if (node.dataset.graphSignature !== signature) {
    node.dataset.graphSignature = signature;
    node.dataset.graphStable = stableSignature;
    node.dataset.graphGeometry = geometrySignature;
    const head = document.createElement("span");
    head.className = "agent-graph-node-head";
    const copy = document.createElement("span");
    copy.className = "agent-graph-node-copy";
    const kind = document.createElement("span");
    kind.className = "agent-graph-node-kind";
    /* 눈썹줄은 종류와 **상태**다 (1-t353).
     *
     * 벤더가 아니다: 이 카드는 벤더를 이미 두 번 말한다 — 머리의 마크가 한 번,
     * 발치의 `Claude · 2분`이 한 번. 세 번째 사본을 걷어 낸 자리에 들어오는 것은
     * 카드가 여태 낱말로 말하지 않던 하나다. 상태는 지금까지 7px 점 하나였고,
     * 대화 줄이 없는 카드에서만 우연히 낱말이 되었다(`line`의 옛 대체값) —
     * 그래서 같은 화면의 카드 둘이 같은 사실을 서로 다른 자리에서 말했다. */
    kind.textContent = agentGraphKindWord(entity.type);
    const title = document.createElement("span");
    title.className = "agent-graph-node-title";
    title.textContent = entity.label;
    title.dataset.tip = entity.label;
    copy.append(kind, title);
    if (entity.line && entity.type !== "agent") {
      const line = document.createElement("span");
      line.className = entity.type === "agent"
        ? "agent-graph-node-message"
        : "agent-graph-node-line";
      line.dataset.tip = entity.line;
      if (entity.type === "workspace" && entity.workspace?.branch && entity.workspace?.base) {
        const basePart = document.createElement("span");
        basePart.className = "agent-graph-base-part";
        basePart.textContent = `← ${entity.workspace.base}`;
        /* 이름줄이 이미 그 브랜치면, 아래 줄은 조상만 말한다 (t-1853).
         *
         * 워크스페이스의 이름은 거의 언제나 브랜치 그 자체이고(`label`), 그
         * 아래 줄은 `branch ← base`였다 — 116px 레인 안에서 같은 문자열을 두 번
         * 쓴 셈이라 두 줄이 나란히 말줄임표가 되었다(실측 1440px, 배율 75%:
         * 레인 134.8px에 「wt/t-142…」와 「wt… ← main」). 위가 브랜치, 아래가
         * 조상이면 두 줄이 함께 `branch ← base`를 말하면서 아무것도 잘리지
         * 않는다. 전문은 툴팁이 계속 든다. */
        if (entity.label === entity.workspace.branch) {
          line.appendChild(basePart);
        } else {
          const branchPart = document.createElement("span");
          branchPart.className = "agent-graph-branch-part";
          branchPart.textContent = entity.workspace.branch;
          line.append(branchPart, document.createTextNode(" "), basePart);
        }
      } else {
        line.textContent = entity.line;
      }
      copy.appendChild(line);
    }
    head.append(agentGraphNodeIcon(entity), copy);
    if (entity.type === "agent" && entity.mailUnread > 0) {
      const unread = document.createElement("span");
      unread.className = "agent-graph-mail-badge";
      unread.textContent = entity.mailUnread > 99 ? "99+" : String(entity.mailUnread);
      unread.dataset.tip = t("board.graph.unreadMail", "읽지 않은 메일 {{count}}개", {
        count: entity.mailUnread,
      });
      unread.setAttribute("aria-label", unread.dataset.tip);
      head.appendChild(unread);
    }
    const parts = [head];
    if (entity.type === "agent") {
      /* 상태 줄의 다섯 칸은 **언제나** 선다 (t-2374).
       *
       * 셋이 조건부였다 — 시계 둘과 조용함의 도장. 조건부 자식은 그 조건이
       * 바뀔 때마다 카드를 다시 짓게 만들고, 카드가 「무엇을 하는 중인가」를
       * 말하기 시작하면 그 조건은 훅 하나마다 바뀐다. 빈 칸은 CSS가 접는다
       * (`:empty`), 그래서 그림은 그대로이고 서명만 조용해진다. */
      const status = document.createElement("span");
      status.className = "agent-graph-node-status";
      const stateWord = document.createElement("span");
      stateWord.className = "agent-graph-state-word";
      stateWord.textContent = facts.stateWord;
      /* 상태 낱말 옆의 한 마디 — 여섯째 상태가 아니라 `working` 위의 옷. */
      const phase = document.createElement("span");
      phase.className = `agent-card-phase is-${facts.phase || "none"}`;
      phase.textContent = facts.phaseWord;
      phase.dataset.tip = facts.phaseTip;
      status.append(agentGraphStateMark(entity.state), stateWord, phase);
      parts.push(status);
      const doing = agentCardDoingNode({ graph: true });
      dressAgentCardDoing(doing, facts);
      parts.push(doing);
      if (entity.line) {
        const message = document.createElement("span");
        message.className = "agent-graph-node-message";
        message.textContent = entity.line;
        message.dataset.tip = entity.line;
        parts.push(message);
      }
      const meta = document.createElement("span");
      meta.className = "agent-card-meta";
      const model = document.createElement("span");
      model.className = "agent-card-model";
      model.textContent = facts.model;
      const clock = document.createElement("span");
      clock.className = "agent-graph-node-clock";
      clock.textContent = entity.clock?.word ?? "";
      const running = document.createElement("span");
      running.className = "agent-graph-node-clock is-running";
      running.textContent = entity.clock?.running ?? "";
      meta.append(model, clock, running);
      if (entity.previousAttempt) meta.append(taskBoardElement("span", "agent-graph-previous-attempt", t("board.graph.previousAttempt", "이전 시도")));
      parts.push(meta);
    }
    if (entity.type === "workspace" && entity.idleCount > 0) {
      const idle = document.createElement("button");
      idle.type = "button";
      idle.className = "agent-graph-idle-chip";
      idle.textContent = agentGraphIdleCountWord(entity.idleCount);
      idle.dataset.tip = agentGraphIdleActionWord(entity);
      idle.setAttribute("aria-label", idle.dataset.tip);
      idle.setAttribute("aria-expanded", String(entity.idleExpanded));
      idle.onclick = (event) => {
        event.stopPropagation();
        agentGraphToggleIdle(view, entity.key);
      };
      parts.push(idle);
    }
    if (entity.type === "workspace" && entity.merge && agentGraphOverlayMode === "merge") {
      const port = document.createElement("span");
      port.className = "agent-graph-merge-port";
      const standing = entity.merge.landed
        ? t("board.graph.mergeLanded", "반영됨")
        : t("board.graph.mergeDistance", "앞 {{ahead}} · 뒤 {{behind}}", {
            ahead: entity.merge.ahead,
            behind: entity.merge.behind,
          });
      port.textContent = `→ ${entity.merge.base} · ${standing}`;
      port.dataset.tip = t("board.graph.mergeFlow", "{{branch}}에서 {{base}}로 병합 흐름", {
        branch: entity.workspace.branch || entity.label,
        base: entity.merge.base,
      });
      port.setAttribute("role", "status");
      parts.push(port);
    }
    if (entity.type === "workspace" && entity.buckets.length > 0) {
      parts.push(agentGraphPipsNode(entity.buckets));
    }
    if (entity.type !== "agent" && entity.meta) {
      const meta = document.createElement("span");
      meta.className = "agent-graph-node-meta";
      meta.textContent = entity.meta;
      parts.push(meta);
    }
    reconcileElementOrder(node, parts);
  }
  return geometryChanged;
}

function agentGraphStateMark(state) {
  const mark = document.createElement("span");
  mark.setAttribute("aria-hidden", "true");
  dressAgentGraphStateMark(mark, state);
  return mark;
}

function dressAgentGraphStateMark(mark, state) {
  writeClassName(mark, `agent-graph-node-state is-${state}`);
  const drawn = state === "done"
    ? icon("circle-check")
    : state === "failed"
      ? icon("circle-x")
      : state === "needs-attention"
        ? icon("help")
        : "";
  if (mark.innerHTML !== drawn) mark.innerHTML = drawn;
}

/* 손은 태어날 때 한 번만 맨다 (1-t385).
 *
 * 이 둘은 판마다 다시 매여 있었다 — 그리는 일이 카드마다 클로저 둘을 새로
 * 짓는다는 뜻이고, 훅이 초당 쉰 번 오는 표면에서 카드 백 장이면 초당 만 개의
 * 함수다. 노드는 키로 살아남으므로 그 손도 살아남아야 한다: 손이 지금 무엇을
 * 가리키는지는 **잡을 때** 판에게 물으면 되고, 노드는 제 키를 이미 들고 있다. */
function agentGraphNode(entity, view) {
  agentGraphNodeCreations += 1;
  const node = document.createElement("button");
  node.type = "button";
  node.setAttribute("role", "treeitem");
  node.onclick = () => selectAgentGraphEntity(view, node.dataset.graphKey);
  node.ondblclick = () => {
    const held = agentGraphModels.get(view)?.entities.get(node.dataset.graphKey);
    if (held?.type === "agent") revealAgentGraphCard(held.card);
  };
  updateAgentGraphNode(node, entity, view);
  return node;
}

/* ---- the project band --------------------------------------------------- */

function agentGraphFoldProject(view, key, { focus = false } = {}) {
  const band = agentGraphModels.get(view)?.entities.get(key);
  if (agentGraphScopeKey) {
    if (agentGraphScopeFolded.has(key)) agentGraphScopeFolded.delete(key);
    else agentGraphScopeFolded.add(key);
    if (focus) agentGraphSelectedKey = key;
    paintAgentGraph(view, agentGraphFullModel(view));
    if (focus) focusAgentGraphSelection(view);
    return;
  }
  if (band?.dormant) {
    if (agentGraphDormantExpanded.has(key)) agentGraphDormantExpanded.delete(key);
    else agentGraphDormantExpanded.add(key);
  } else if (agentGraphFolded.has(key)) agentGraphFolded.delete(key);
  else {
    agentGraphFolded.add(key);
    /* Folding a band must not fold the inspector's subject away with it. The
     * band is itself an entity, so the selection lands on the heading a person
     * just closed instead of jumping to some other project's first agent. */
    const held = agentGraphModels.get(view)?.entities.get(agentGraphSelectedKey);
    if (held?.band?.key === key) agentGraphSelectedKey = key;
  }
  /* Folding rebuilds the heading's own contents, so the button a keyboard was
   * standing on is replaced by a new one and the focus falls to the document.
   * Put back only when the fold came from the keyboard: a pointer that closed a
   * band did not ask to be moved anywhere. */
  const drawn = paintBoardView(boardTab(), { force: true });
  if (focus) void drawn.then(() => focusAgentGraphSelection(view));
  else void drawn;
}

function updateAgentGraphBandHead(head, band, view) {
  const selected = band.key === agentGraphSelectedKey;
  writeClassName(head, [
    "agent-graph-band-head",
    band.folded ? "is-folded" : "",
    band.dormant ? "is-dormant" : "",
    selected ? "is-selected" : "",
    band.searchMatch === false ? "is-search-dimmed" : "",
  ].filter(Boolean).join(" "));
  writeStyleValue(head, "gridRow", String(band.row));
  const signature = [
    locale,
    band.label,
    band.line,
    band.meta,
    String(band.folded),
    String(band.agentCount),
    [...band.counts].sort().join(","),
    band.faces.map((entry) => `${entry.card.agent}:${entry.bucket}`).join(","),
    band.dormantWorkspaces
      .map((workspace) => `${workspace.key}:${workspace.idleAgents.length}`)
      .join(","),
  ].join("");
  const geometrySignature = [
    band.label,
    String(band.folded),
    band.dormantWorkspaces.map((workspace) => workspace.key).join(","),
  ].join("");
  const geometryChanged = head.dataset.graphGeometry !== geometrySignature;
  if (head.dataset.graphSignature !== signature) {
    head.dataset.graphSignature = signature;
    head.dataset.graphGeometry = geometrySignature;
    const rail = document.createElement("div");
    rail.className = "agent-graph-band-rail";

    const fold = document.createElement("button");
    fold.type = "button";
    fold.className = "agent-graph-band-fold";
    fold.innerHTML = icon("chevron", !band.folded);
    /* Not a tab stop. The treeitem beside it already carries `aria-expanded`
     * and the arrow keys already fold, so leaving this in the tab order would
     * add one stop per project to a surface whose contract is that Tab enters
     * the tree once. It stays a real button because a pointer needs one. */
    fold.tabIndex = -1;
    fold.setAttribute("aria-expanded", String(!band.folded));
    fold.setAttribute(
      "aria-label",
      band.folded
        ? t("board.graph.unfoldBand", "{{name}} 펼치기", { name: band.label })
        : t("board.graph.foldBand", "{{name}} 접기", { name: band.label }),
    );

    const name = document.createElement("button");
    name.type = "button";
    name.className = "agent-graph-band-name";
    name.setAttribute("role", "treeitem");
    name.setAttribute("aria-level", "1");
    name.dataset.graphKey = band.key;
    name.innerHTML = icon("folder");
    const label = document.createElement("strong");
    label.textContent = band.label;
    const line = document.createElement("span");
    line.className = "agent-graph-band-line";
    const counts = document.createElement("span");
    counts.className = "agent-graph-band-counts";
    for (const [kind, count] of [
      ["workspace", band.project.workspaces.size],
      ["agent", band.agentCount],
    ]) {
      const chip = document.createElement("span");
      chip.className = "agent-graph-count-chip";
      const value = document.createElement("b");
      value.textContent = String(count);
      const word = document.createElement("span");
      word.textContent = agentGraphKindWord(kind);
      chip.append(value, word);
      counts.appendChild(chip);
    }
    line.appendChild(counts);
    name.append(label, line);

    const roll = document.createElement("span");
    roll.className = "agent-graph-band-roll";
    for (const bucket of ["attention", "working", "done"]) {
      const count = band.counts.get(bucket) ?? 0;
      if (count === 0) continue;
      const chip = document.createElement("span");
      chip.className = `agent-graph-band-chip is-${bucket}`;
      const mark = document.createElement("i");
      mark.setAttribute("aria-hidden", "true");
      const value = document.createElement("b");
      value.textContent = String(count);
      chip.append(mark, value);
      chip.dataset.tip = bucketWord(bucket);
      roll.appendChild(chip);
    }
    rail.append(fold, name, roll);

    if (!band.folded && band.dormantWorkspaces.length > 0) {
      const dormant = document.createElement("span");
      dormant.className = "agent-graph-dormant-list";
      for (const workspace of band.dormantWorkspaces) {
        const chip = document.createElement("button");
        chip.type = "button";
        chip.className = "agent-graph-dormant-chip";
        chip.dataset.idleWorkspace = workspace.key;
        const count = workspace.idleAgents.length;
        chip.textContent = count > 0
          ? `${workspace.label} · ${agentGraphIdleCountWord(count)}`
          : workspace.label;
        chip.dataset.tip = count > 0
          ? agentGraphIdleActionWord(workspace)
          : t("board.graph.expandWorkspace", "{{name}} 레인 펼치기", {
              name: workspace.label,
            });
        chip.setAttribute("aria-label", chip.dataset.tip);
        chip.setAttribute("aria-expanded", "false");
        chip.onclick = (event) => {
          event.stopPropagation();
          agentGraphToggleIdle(view, workspace.key);
        };
        dormant.appendChild(chip);
      }
      rail.appendChild(dormant);
    }

    /* A folded band still has to answer "who is in there". The faces are the
     * shortest true answer — the agent marks a person already recognises, each
     * wearing its own state ring. */
    if (band.folded) {
      const faces = document.createElement("span");
      faces.className = "agent-graph-band-faces";
      for (const entry of band.faces.slice(0, 6)) {
        const face = document.createElement("span");
        face.className = `agent-graph-band-face is-${entry.bucket}`;
        face.appendChild(
          agentIcon(agentRows.find((row) => row.id === entry.card.agent) ?? {
            id: entry.card.agent,
            name: agentName(entry.card.agent),
          }),
        );
        face.dataset.tip = entry.card.heading;
        faces.appendChild(face);
      }
      if (band.faces.length > 6) {
        const more = document.createElement("span");
        more.className = "agent-graph-band-more";
        more.textContent = `+${band.faces.length - 6}`;
        faces.appendChild(more);
      }
      rail.appendChild(faces);
    }
    head.replaceChildren(rail);
  }
  head.querySelector(".agent-graph-band-fold").onclick = (event) => {
    event.stopPropagation();
    agentGraphFoldProject(view, band.key);
  };
  const name = head.querySelector(".agent-graph-band-name");
  name.setAttribute("aria-selected", String(selected));
  // The expansion belongs to the ITEM, not to the chevron beside it: a tree
  // reader asks the treeitem whether it is open, and the chevron is one way of
  // answering rather than the fact itself.
  name.setAttribute("aria-expanded", String(!band.folded));
  name.tabIndex = selected ? 0 : -1;
  name.onclick = () => selectAgentGraphEntity(view, band.key);
  name.ondblclick = () => agentGraphFoldProject(view, band.key);
  return geometryChanged;
}

function revealAgentGraphCard(card) {
  const { kind, id, term, valid } = boardCardDestination(card);
  if (!valid || kind === "worker") return;
  if (isPopout) return void invoke("reveal_board_agent", { pane: card.pane }).catch(showError);
  if (term !== null) return void openPaneFromBoard(term);
  focusLane(id);
}

async function spawnAgentFromGraph(workspace, agent) {
  if (workspace.path !== activeWorktreePath && !(await activateWorktree(workspace.path))) return;
  try {
    const term = await launchAgentTab({ agent: agent.id, prompt: "", ...spawnGrid({ placement: "tab" }) });
    mountTermTab(term, { agent: agent.name }, { placement: "tab" });
  } catch (error) {
    showError(error);
  }
}

/* ---- the inspector ------------------------------------------------------ */

/* The typed triples behind the selected entity — subject, relation, object.
 * The canvas draws relations as lines because a line is readable at a glance;
 * this says them in words, because a line cannot say WHICH relation it is once
 * two of them meet at the same node. */
function agentGraphTriples(entity) {
  const held = [];
  if (entity.type === "agent") {
    held.push([entity.project.label, agentGraphRelationWord("contains"), entity.workspace.label, entity.workspace.key]);
    held.push([entity.workspace.label, agentGraphRelationWord("contains"), entity.label, entity.key]);
    if (entity.card.parent) {
      held.push([
        t("board.graph.parentAgent", "상위 에이전트"),
        agentGraphRelationWord("spawned"),
        entity.label,
        `agent:${entity.card.parent}`,
      ]);
    }
    if (entity.card.lineage?.child_count > 0) {
      held.push([
        entity.label,
        agentGraphRelationWord("spawned"),
        agentGraphCountWord("agent", entity.card.lineage.child_count),
      ]);
    }
    return held;
  }
  if (entity.type === "workspace") {
    held.push([entity.workspace.project.label, agentGraphRelationWord("contains"), entity.label, entity.workspace.project.key]);
    held.push([entity.label, agentGraphRelationWord("contains"), entity.meta]);
    if (entity.line) held.push([entity.label, t("board.graph.onBranch", "브랜치"), entity.line]);
    return held;
  }
  held.push([entity.label, agentGraphRelationWord("contains"), entity.line]);
  held.push([
    entity.label,
    agentGraphRelationWord("contains"),
    agentGraphCountWord("agent", entity.agentCount),
  ]);
  if (entity.meta) held.push([entity.label, t("board.graph.location", "위치"), entity.meta]);
  return held;
}

function agentGraphSummaryCardNode(entity) {
  const card = document.createElement("div");
  card.className = "agent-inspector-summary-card";
  const stats = document.createElement("div");
  stats.className = "agent-inspector-stats";

  if (entity.type === "project") {
    const wsStat = document.createElement("div");
    wsStat.className = "agent-inspector-stat-item";
    const wsVal = document.createElement("b");
    wsVal.textContent = String(entity.project?.workspaces?.size ?? 0);
    const wsLbl = document.createElement("span");
    wsLbl.textContent = t("board.graph.workspace", "워크스페이스");
    wsStat.append(wsVal, wsLbl);

    const agStat = document.createElement("div");
    agStat.className = "agent-inspector-stat-item";
    const agVal = document.createElement("b");
    agVal.textContent = String(entity.agentCount ?? 0);
    const agLbl = document.createElement("span");
    agLbl.textContent = t("board.graph.agent", "에이전트");
    agStat.append(agVal, agLbl);

    stats.append(wsStat, agStat);
  } else if (entity.type === "workspace") {
    const agStat = document.createElement("div");
    agStat.className = "agent-inspector-stat-item";
    const agVal = document.createElement("b");
    agVal.textContent = String(entity.workspace?.agents?.size ?? 0);
    const agLbl = document.createElement("span");
    agLbl.textContent = t("board.graph.agent", "에이전트");
    agStat.append(agVal, agLbl);
    stats.append(agStat);

    if (entity.line) {
      const brStat = document.createElement("div");
      brStat.className = "agent-inspector-stat-item";
      const brVal = document.createElement("b");
      brVal.textContent = entity.line;
      const brLbl = document.createElement("span");
      brLbl.textContent = t("board.graph.onBranch", "브랜치");
      brStat.append(brVal, brLbl);
      stats.append(brStat);
    }
  }
  card.appendChild(stats);
  return card;
}

function agentGraphTriplesNode(entity, view = null) {
  const list = document.createElement("div");
  list.className = "agent-inspector-triples";
  const heading = document.createElement("p");
  heading.className = "agent-inspector-context-heading";
  heading.textContent = t("board.graph.relation", "관계");
  list.appendChild(heading);
  const model = view ? agentGraphModels.get(view) : null;
  for (const [subject, relation, object, targetKey] of agentGraphTriples(entity)) {
    if (!object) continue;
    const row = document.createElement("p");
    row.className = "agent-inspector-triple";
    const one = document.createElement("span");
    one.textContent = subject;
    const rel = document.createElement("em");
    rel.textContent = relation;
    const other = document.createElement("strong");
    other.textContent = object;
    row.append(one, rel, other);

    if (model && view && targetKey && model.entities.has(targetKey) && targetKey !== entity.key) {
      row.classList.add("is-navigable");
      row.tabIndex = 0;
      row.setAttribute("role", "button");
      row.setAttribute(
        "aria-label",
        [subject, relation, object, "—", t("board.graph.focus", "선택한 노드로 이동")].join(" "),
      );
      row.onclick = () => selectAgentGraphEntity(view, targetKey, { focus: true });
      row.onkeydown = (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectAgentGraphEntity(view, targetKey, { focus: true });
        }
      };
    }
    list.appendChild(row);
  }
  return list;
}

function agentGraphBacktraceNode(entity, model) {
  const section = document.createElement("section");
  section.className = "agent-inspector-backtrace";
  const heading = document.createElement("p");
  heading.className = "agent-inspector-context-heading";
  heading.textContent = t("board.graph.backtrace", "왜 이 에이전트가 있는가");
  const rows = document.createElement("ol");
  const seen = new Set();
  let here = entity;
  while (here?.type === "agent" && here.card.parent && !seen.has(here.key)) {
    seen.add(here.key);
    const parent = model.entities.get(agentGraphAgentKey(here.card.parent));
    if (!parent || parent.type !== "agent") break;
    const row = document.createElement("li");
    row.textContent = t("board.graph.spawnTrace", "{{parent}}이(가) {{child}} 시작", {
      parent: parent.label,
      child: here.label,
    });
    rows.prepend(row);
    here = parent;
  }
  for (const edge of model.overlayData?.dependencies ?? []) {
    if (edge.to !== entity.key) continue;
    const upstream = model.entities.get(edge.from);
    if (!upstream) continue;
    const row = document.createElement("li");
    row.textContent = t("board.graph.dispatchTrace", "{{agent}} 작업이 선행 조건", {
      agent: upstream.label,
    });
    rows.appendChild(row);
  }
  section.append(heading, rows);
  section.hidden = rows.childElementCount === 0;
  return section;
}

/* Everything the ring still holds for one agent, newest first — the panel's
 * long form of the card's three-line wire. */
function agentGraphStreamNode(entity, view) {
  const beats = agentGraphFoldedActivities(entity.card.pane);
  const expanded = agentGraphExpandedTimelines.has(entity.card.pane);
  const hidden = Math.max(0, beats.length - ACTIVITY_RING);
  const shown = expanded ? beats : beats.slice(-ACTIVITY_RING);
  const stream = document.createElement("div");
  stream.className = "agent-inspector-stream agent-inspector-timeline";
  const heading = document.createElement("p");
  heading.className = "agent-inspector-context-heading";
  heading.textContent = t("board.graph.liveWork", "지금 하는 일");
  stream.appendChild(heading);
  if (shown.length === 0) {
    const quiet = document.createElement("p");
    quiet.className = "agent-inspector-stream-quiet";
    quiet.textContent = t("board.graph.noWork", "아직 기록된 도구 호출이 없습니다.");
    stream.appendChild(quiet);
    return stream;
  }
  const wire = document.createElement("div");
  wire.className = "agent-graph-wire is-stream";
  for (const beat of shown) wire.appendChild(agentGraphBeatNode(beat));
  stream.appendChild(wire);
  if (hidden > 0 && !expanded) {
    const more = document.createElement("button");
    more.type = "button";
    more.className = "btn agent-inspector-more";
    more.textContent = t("board.graph.moreActivities", "{{count}}개 더 보기", { count: hidden });
    more.onclick = () => {
      agentGraphExpandedTimelines.add(entity.card.pane);
      paintAgentGraphInspector(view, agentGraphModels.get(view));
    };
    stream.appendChild(more);
  }
  return stream;
}

function agentGraphBreadcrumbNode(entity, model) {
  const crumb = document.createElement("nav");
  crumb.className = "agent-inspector-breadcrumb";
  crumb.setAttribute("aria-label", t("board.graph.breadcrumb", "위치"));
  const parent = model.entities.get(`agent:${entity.card.parent}`);
  const words = [entity.project.label, entity.workspace.label, entity.label];
  words.forEach((word, index) => {
    const one = document.createElement("span");
    one.textContent = word;
    crumb.appendChild(one);
    if (index < words.length - 1) crumb.append(document.createTextNode(" › "));
  });
  if (parent) {
    const lineage = document.createElement("span");
    lineage.className = "agent-inspector-lineage";
    lineage.textContent = ` (${agentGraphRelationWord("spawned")}: ${parent.label})`;
    crumb.appendChild(lineage);
  }
  return crumb;
}

function paintAgentGraphContextInspector(view, entity) {
  const body = view.querySelector(".agent-inspector-body");
  /* The triples themselves, not a list of the fields they were built from.
   *
   * Every field this panel draws has been forgotten here at least once — the
   * agent count, then the workspace's own renamed label, then the project label
   * a triple quotes but the workspace entity never carries. Serialising the
   * drawn answer ends that class of bug rather than adding a fourth field to
   * the list and waiting for the fifth. */
  const signature = [
    locale,
    entity.key,
    String(entity.folded ?? ""),
    entity.buckets?.join(",") ?? "",
    JSON.stringify(agentGraphTriples(entity)),
  ].join("");
  if (body.dataset.graphSignature === signature) return;
  body.dataset.graphSignature = signature;
  const facts = document.createElement("div");
  facts.className = "agent-inspector-facts";
  facts.appendChild(agentGraphSummaryCardNode(entity));
  facts.appendChild(agentGraphTriplesNode(entity, view));
  if (entity.type === "workspace" && entity.workspace.scoped) {
    const actions = document.createElement("div");
    actions.className = "agent-inspector-context-actions";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "btn";
    open.textContent = t("board.openWorktree", "워크스페이스 열기");
    open.onclick = () => void activateWorktree(entity.workspace.path);
    actions.appendChild(open);
    const heading = document.createElement("p");
    heading.className = "agent-inspector-context-heading";
    heading.textContent = t("board.graph.startAgent", "새 에이전트 시작");
    actions.appendChild(heading);
    for (const agent of installedAgents()) {
      const start = document.createElement("button");
      start.type = "button";
      start.className = "agent-inspector-agent";
      start.append(agentIcon(agent), document.createTextNode(agent.name));
      const plus = document.createElement("span");
      plus.innerHTML = icon("plus");
      plus.setAttribute("aria-hidden", "true");
      start.appendChild(plus);
      start.onclick = () => void spawnAgentFromGraph(entity.workspace, agent);
      actions.appendChild(start);
    }
    facts.appendChild(actions);
  }
  if (entity.type === "project") {
    const actions = document.createElement("div");
    actions.className = "agent-inspector-context-actions";
    const fold = document.createElement("button");
    fold.type = "button";
    fold.className = "btn";
    fold.textContent = entity.folded
      ? t("board.graph.unfold", "밴드 펼치기")
      : t("board.graph.fold", "밴드 접기");
    fold.onclick = () => agentGraphFoldProject(view, entity.key);
    actions.appendChild(fold);

    const fit = document.createElement("button");
    fit.type = "button";
    fit.className = "btn";
    fit.textContent = t("board.graph.fit", "화면에 맞추기");
    fit.onclick = () => fitAgentGraph(view);
    actions.appendChild(fit);

    facts.appendChild(actions);
  }
  body.replaceChildren(facts);
}

/* 「보고서 보기」 — 이 자리의 워커가 보고서를 남겼을 때만 (t-2720 §4). 원장
 * 명부(`paneLedger`)가 이 판에 붙여 둔 워커와 카탈로그의 수(`artifactCounts`)
 * 둘 다 이미 손에 있으므로 묻지 않는다; 단추는 한 번 짓고 그림마다 옷만
 * 갈아입는다(서명 밖의 사실이라 서명 비교를 지나치지 않도록). 누르면 아티팩트
 * 뷰가 그 보고서 위에서, 그 워커로 걸러져 열린다. */
function paintAgentInspectorReport(actions, entity) {
  let button = actions.querySelector(".agent-inspector-report");
  if (!button) {
    button = document.createElement("button");
    button.type = "button";
    button.className = "btn agent-inspector-report";
    button.innerHTML = icon("ft-file-text");
    button.appendChild(document.createElement("span"));
    actions.appendChild(button);
  }
  // Said on every paint, not through `data-i18n`: the node is born at paint
  // time in whatever language is on, and a source captured then would be
  // read back as the Korean fallback by the next locale swap.
  button.querySelector("span").textContent = t("artifacts.viewReport", "보고서 보기");
  const seatPane = String(entity.card?.pane ?? "");
  const seatTerm = seatPane.startsWith("term:") ? Number(seatPane.slice(5)) : NaN;
  const seatWorker = entity.place?.workerId || paneLedger.get(seatTerm)?.worker || null;
  const report = seatWorker ? artifactCounts?.report_by_worker?.[seatWorker] ?? null : null;
  button.hidden = report === null;
  button.onclick = report === null ? null : () => {
    leavePagesForStage();
    openArtifacts({
      origin: { field: "worker", value: seatWorker, label: entity.facts?.identity ?? seatWorker },
      select: report,
    });
  };
}

function agentGraphInspectorParts(view) {
  const body = view.querySelector(".agent-inspector-body");
  let held = agentGraphInspectorViews.get(body);
  if (held?.relations.parentNode === body) return held;
  held = { stamps: {} };
  for (const name of ["response", "relations", "activity", "details"]) {
    held[name] = taskBoardElement("section", `agent-inspector-pane is-${name}`);
    if (name !== "response") held[name].setAttribute("role", "tabpanel");
  }
  body.replaceChildren(held.response, held.relations, held.activity, held.details);
  agentGraphInspectorViews.set(body, held);
  return held;
}

function agentGraphDetailField(label, value) {
  if (value === null || value === undefined || value === "") return null;
  const row = taskBoardElement("div", "agent-relation-fact");
  row.append(taskBoardElement("span", "", label), taskBoardElement("strong", "", String(value)));
  return row;
}

function agentGraphTimeWord(value) {
  const date = new Date(Number(value));
  return Number(value) > 0 && Number.isFinite(date.getTime()) ? date.toLocaleString(locale) : null;
}

function agentGraphTaskStateWord(state) {
  switch (state) {
    case "pending": return t("board.graph.taskPending", "선행 조건 대기");
    case "ready": return t("board.graph.taskReady", "실행 대기");
    case "dispatched": return t("board.graph.taskDispatched", "배차됨");
    case "completed": return t("board.graph.taskCompleted", "과업 완료");
    case "failed": return t("board.graph.taskFailed", "과업 실패");
    case "blocked": return t("board.graph.taskBlocked", "결정 대기");
    default: return state || t("board.graph.factUnknown", "확인되지 않음");
  }
}

function agentGraphRelationRow(view, relation, model) {
  const full = agentGraphFullModel(view, model);
  const endpoint = (key) => model.entities.get(key) ?? full.entities.get(key)
    ?? full.agents.find((entry) => entry.key === key);
  const from = endpoint(relation.from);
  const to = endpoint(relation.to);
  const button = taskBoardElement("button", "agent-relation-row");
  button.type = "button";
  button.dataset.relationKey = relation.key;
  writeAttribute(button, "aria-pressed", String(agentGraphSelectedEdgeKey === relation.key));
  button.append(
    taskBoardElement("span", "agent-relation-row-kind", agentGraphRelationWord(relation.type)),
    taskBoardElement("strong", "", `${from?.label ?? (from?.card ? taskBoardTitle(from.card) : relation.fact?.dependency ?? relation.from ?? "")} → ${to?.label ?? (to?.card ? taskBoardTitle(to.card) : relation.fact?.task ?? relation.to ?? "")}`),
  );
  if (relation.outside) button.append(taskBoardElement("small", "", t("board.graph.endpointOutside", "현재 보드에 없는 실행의 관계")));
  button.onclick = () => selectAgentGraphRelation(view, relation.key);
  return button;
}

function agentGraphRelationDetail(view, relation, model) {
  const block = taskBoardElement("section", "agent-relation-detail");
  const route = taskBoardElement("div", "agent-relation-route");
  const appendEnd = (key, fallback) => {
    const full = agentGraphFullModel(view);
    const entity = model.entities.get(key) ?? full.entities.get(key);
    const entry = entity ?? full.agents.find((one) => one.key === key);
    if (!entry) { route.append(taskBoardElement("span", "is-unavailable", fallback || t("board.graph.factUnknown", "확인되지 않음"))); return; }
    const button = taskBoardElement("button", "", entity?.label ?? agentGraphIdentity(entry.card));
    button.type = "button";
    button.dataset.relationEndpoint = key;
    button.onclick = () => selectAgentGraphEntity(view, key, { focus: true });
    route.append(button);
  };
  appendEnd(relation.from, relation.fact?.dependency);
  route.append(taskBoardElement("span", "agent-relation-route-kind", `↓ ${agentGraphRelationWord(relation.type)}`));
  appendEnd(relation.to, relation.fact?.task);
  block.append(route);
  const facts = taskBoardElement("div", "agent-relation-evidence");
  const add = (label, value) => { const row = agentGraphDetailField(label, value); if (row) facts.append(row); };
  if (relation.type === "mail") {
    block.append(taskBoardElement("p", "agent-relation-explanation", t("board.graph.mailEvidence", "현재 표시된 실행 사이에 원장이 기록한 메시지입니다. 수신 여부와 응답 완료는 서로 다른 사실입니다.")));
    add(t("board.graph.messageCount", "메시지 수"), relation.count);
    add(t("board.graph.unreadCount", "미확인 수"), relation.unread);
    add(t("board.graph.lastMessageTime", "마지막 메시지"), agentGraphTimeWord(relation.at));
    const source = relation.evidence;
    if (source) {
      add(t("board.graph.run", "런"), source.run);
      add(t("board.graph.messageId", "메시지 ID"), source.id);
      add(t("board.graph.sender", "발신 주소"), source.from);
      add(t("board.graph.recipient", "수신 주소"), source.to);
    }
  } else if (relation.type === "dependency") {
    block.append(taskBoardElement("p", "agent-relation-explanation", t("board.graph.dependencyEvidence", "후속 과업에 등록된 선행 조건입니다. 이 연결만으로 에이전트가 현재 멈춰 있거나 검증이 끝났다고 판단하지 않습니다.")));
    const dependencies = agentGraphDependencyFacts(model, relation);
    if (dependencies.length === 0) {
      add(t("board.graph.relationCount", "등록된 관계 수"), relation.count);
      add(t("board.graph.taskCreated", "후속 과업 생성"), agentGraphTimeWord(relation.at));
      facts.append(taskBoardElement("p", "agent-relation-note", t("board.graph.dependencyDetailUnknown", "이 스냅샷에는 과업별 선행 조건의 상세 상태가 없습니다.")));
    }
    for (const dependency of dependencies) {
      const item = taskBoardElement("div", "agent-relation-dependency-fact");
      const rows = [
        agentGraphDetailField(t("board.graph.run", "런"), dependency.run),
        agentGraphDetailField(t("board.graph.upstreamTask", "선행 과업"), `${dependency.dependency} · ${agentGraphTaskStateWord(dependency.dependency_state)}`),
        agentGraphDetailField(t("board.graph.downstreamTask", "후속 과업"), `${dependency.task} · ${agentGraphTaskStateWord(dependency.task_state)}`),
        agentGraphDetailField(t("board.graph.taskCreated", "후속 과업 생성"), agentGraphTimeWord(dependency.task_created_ms)),
      ].filter(Boolean);
      item.append(...rows);
      if (!dependency.from) item.append(taskBoardElement("p", "agent-relation-note", t("board.graph.upstreamNotShown", "선행 과업의 실행은 현재 보드에 없습니다. 과업의 상태는 원장 기록으로 확인합니다.")));
      facts.append(item);
    }
  } else {
    block.append(taskBoardElement("p", "agent-relation-explanation", relation.type === "spawned"
      ? t("board.graph.lineageEvidence", "판이 보고한 실제 부모 관계입니다. 작업의 선행 조건이나 실행 순서와는 구분합니다.")
      : t("board.graph.containmentEvidence", "프로젝트와 실제 체크아웃에 속한 실행을 나타냅니다.")));
    for (const key of [relation.from, relation.to]) {
      const entity = model.entities.get(key);
      if (!entity) continue;
      add(entity.label, entity.type === "agent" ? entity.card.pane : entity.meta || entity.label);
    }
  }
  block.append(facts);
  return block;
}

function paintAgentInspectorDestinations(view, entity) {
  const actions = view.querySelector(".agent-inspector-actions");
  if (entity?.type !== "agent") { actions.hidden = true; return; }
  const card = entity.card;
  const destination = boardCardDestination(card);
  const preview = view.querySelector(".agent-inspector-preview");
  const open = view.querySelector(".agent-inspector-open");
  const worker = destination.kind === "worker";
  actions.hidden = false;
  preview.hidden = destination.kind === "lane";
  preview.disabled = !destination.valid;
  writeAttribute(preview, "data-i18n", worker ? "board.graph.ackRecord" : "board.graph.preview");
  writeTextContent(preview, worker ? t("board.graph.ackRecord", "확인했어요") : t("board.graph.preview", "미리보기"));
  preview.onclick = destination.valid ? () => openBoardCard(card, entity.bucket) : null;
  open.hidden = worker || !destination.valid;
  open.disabled = !isPopout && destination.term !== null && !tabOfTerm(destination.term);
  writeAttribute(open, "data-i18n", destination.kind === "lane" ? "board.graph.openExecution" : "board.graph.openTerminal");
  writeTextContent(open, destination.kind === "lane" ? t("board.graph.openExecution", "실행 열기") : t("board.graph.openTerminal", "터미널 열기"));
  open.onclick = open.hidden || open.disabled ? null : () => revealAgentGraphCard(card);
  paintAgentInspectorReport(actions, entity);
}

// Reuse the existing live card renderer. Its response subtree stays connected
// when clocks or surrounding metadata change, including during IME composition.
function paintAgentInspectorCard(parts, subject, draft) {
  if (!subject) {
    parts.response.replaceChildren(); parts.response.hidden = true; parts.card = null;
    delete parts.stamps.card; delete parts.stamps.response;
    return;
  }
  if (draft && !draft.autoOpened) { draft.open = true; draft.autoOpened = true; }
  const question = JSON.stringify([locale, subject.key, subject.card.ask_prompt, subject.card.approval,
    draft?.open, draft?.index, draft?.sending, draft?.selections]);
  const { at, changed_at, ...visible } = subject.card;
  const preview = agentGraphInspectorTab === "activity";
  const signature = JSON.stringify([locale, visible, subject.bucket, subject.review, subject.place,
    subject.clock, subject.facts, question, preview]);
  if (parts.stamps.card !== signature) {
    const fresh = boardCardNode(subject.card, Date.now(), subject.bucket, subject.review, subject.place, { preview });
    fresh.classList.add("agent-inspector-card");
    if (parts.card?.dataset.pane === subject.card.pane && parts.card.tagName === fresh.tagName) {
      const response = parts.stamps.response === question ? parts.card.querySelector(".board-ask, .board-approve") : null;
      const children = [...fresh.children].map((child) => response && child.matches(".board-ask, .board-approve") ? response : child);
      parts.card.className = fresh.className;
      parts.card.style.cssText = fresh.style.cssText;
      parts.card.onclick = fresh.onclick;
      if (response) {
        // Keep the form as an immovable anchor. insertBefore(response, ...)
        // would detach it and cancel IME even if its value were restored.
        for (const child of [...parts.card.children]) if (child !== response) child.remove();
        let before = true;
        for (const child of children) {
          if (child === response) { before = false; continue; }
          if (before) parts.card.insertBefore(child, response);
          else parts.card.appendChild(child);
        }
      } else reconcileElementOrder(parts.card, children);
    } else {
      parts.card = fresh;
      parts.response.replaceChildren(fresh);
    }
    parts.stamps.card = signature;
    parts.stamps.response = question;
  }
  parts.response.classList.toggle("is-response-only", !preview);
  parts.response.hidden = !preview && !draft;
}

/* 인스펙터 머리의 글리프 타일 (t-4145, 시안의 detail-icon). 관계는 고리, 나머지는
 * 그 노드가 그림에서 쓰는 바로 그 아이콘(`agentGraphNodeIcon`) — 같은 것을 두
 * 모양으로 그리지 않는다. 주체가 같으면 다시 짓지 않는다. */
function paintAgentInspectorGlyph(view, relation, entity) {
  const tile = view.querySelector(".agent-inspector-icon");
  if (!tile) return;
  const subject = relation ? `relation:${relation.type}` : entity ? `${entity.type}:${entity.type === "agent" ? entity.card.agent : ""}` : "";
  tile.hidden = subject === "";
  if (tile.dataset.subject === subject) return;
  tile.dataset.subject = subject;
  if (relation) tile.innerHTML = icon("link");
  else if (entity) tile.replaceChildren(...agentGraphNodeIcon(entity).childNodes);
  else tile.replaceChildren();
}

function paintAgentGraphInspector(view, model) {
  const full = agentGraphFullModel(view, model);
  const inspection = model.scoped ? model : full;
  const relations = agentGraphRelations(inspection);
  const relation = relations.find((edge) => edge.key === agentGraphSelectedEdgeKey) ?? null;
  if (!relation) agentGraphSelectedEdgeKey = null;
  const entity = inspection.entities.get(agentGraphSelectedKey) ?? full.entities.get(agentGraphSelectedKey);
  paintAgentInspectorGlyph(view, relation, entity);
  const kind = view.querySelector(".agent-inspector-kind");
  const title = view.querySelector(".agent-inspector-title");
  const meta = view.querySelector(".agent-inspector-meta");
  const body = view.querySelector(".agent-inspector-body");
  const actions = view.querySelector(".agent-inspector-actions");
  const tabs = view.querySelector(".agent-inspector-tabs");
  if (tabs) tabs.hidden = !relation && entity?.type !== "agent";
  if (!entity && !relation) {
    writeTextContent(kind, t("board.graph.inspectorEyebrow", "SELECTED NODE"));
    writeTextContent(title, t("board.graph.inspectorEmptyTitle", "에이전트를 선택하세요"));
    writeTextContent(meta, t("board.graph.inspectorEmptyCopy", "그래프에서 관계를 선택하면 상태와 실행 동작이 여기에 표시됩니다."));
    if (body.childNodes.length) body.replaceChildren();
    agentGraphInspectorViews.delete(body);
    actions.hidden = true;
    return;
  }
  writeTextContent(kind, relation ? t("board.graph.selectedRelation", "선택한 관계") : agentGraphKindWord(entity.type));
  writeTextContent(title, relation ? agentGraphRelationWord(relation.type) : entity.label);
  if (!relation && entity.type === "agent") {
    let id = meta.querySelector(".agent-inspector-id");
    if (!id) {
      id = taskBoardElement("span", "agent-inspector-id");
      meta.replaceChildren(id, taskBoardElement("span", "agent-inspector-state"));
    }
    writeTextContent(id, entity.card.pane);
    writeTextContent(meta.lastElementChild, ` · ${agentName(entity.card.agent)} · ${entity.facts.stateWord}`);
  } else writeTextContent(meta, relation ? t("board.graph.relationHint", "연결의 방향과 근거를 확인하세요.") : entity.meta || "");
  if (!relation && entity.type !== "agent") {
    actions.hidden = true;
    agentGraphInspectorViews.delete(body);
    paintAgentGraphContextInspector(view, entity);
    return;
  }
  const parts = agentGraphInspectorParts(view);
  const subjectKey = relation?.key ?? entity?.key;
  if (parts.subjectKey !== subjectKey) {
    parts.subjectKey = subjectKey;
    for (const name of ["relations", "activity", "details"]) {
      if (name !== agentGraphInspectorTab) { parts[name].replaceChildren(); delete parts.stamps[name]; }
    }
  }
  for (const name of ["relations", "activity", "details"]) {
    parts[name].hidden = name !== agentGraphInspectorTab;
    const button = view.querySelector(`[data-agent-inspector-tab="${name}"]`);
    if (button) writeAttribute(parts[name], "aria-label", button.textContent);
  }
  const subject = entity?.type === "agent" ? entity : null;
  const draft = subject?.bucket === "attention" && (subject.card.ask_prompt || subject.card.approval)
    ? askDraftFor(subject.card) : null;
  paintAgentInspectorCard(parts, subject, draft);
  if (agentGraphInspectorTab === "relations") {
    const relevant = relation ? [relation] : relations.filter((edge) => edge.from === entity.key || edge.to === entity.key);
    const signature = JSON.stringify([locale, entity?.key, entity?.label, agentGraphSelectedEdgeKey, relevant,
      relevant.map((edge) => [inspection.entities.get(edge.from)?.label, inspection.entities.get(edge.to)?.label]),
      inspection.source.overlays.task_dependencies ?? [], subject?.place, secondBrainVault, taskBoardRecallsRevision]);
    if (parts.stamps.relations !== signature) {
      parts.stamps.relations = signature;
      const nodes = relation ? [agentGraphRelationDetail(view, relation, inspection)]
        : [agentGraphBreadcrumbNode(entity, inspection), ...relevant.map((edge) => agentGraphRelationRow(view, edge, inspection))];
      if (!relation && relevant.length === 0) nodes.push(taskBoardElement("p", "agent-relation-note", t("board.graph.noRecordedRelations", "현재 스냅샷에 표시할 관계가 없습니다.")));
      if (subject) nodes.push(workbenchRelatedActions(subject), taskBoardRecallsNode(subject));
      parts.relations.replaceChildren(...nodes);
    }
  }
  if (agentGraphInspectorTab === "activity") {
    const signature = JSON.stringify([locale, subject?.key, subject?.card.said,
      subject ? agentGraphTickerSignature(subject.card.pane) : "", subject?.clock?.word,
      subject ? agentGraphExpandedTimelines.has(subject.card.pane) : false]);
    if (parts.stamps.activity !== signature) {
      parts.stamps.activity = signature;
      const nodes = [];
      if (subject) nodes.push(agentGraphBreadcrumbNode(subject, inspection), agentGraphStreamNode(subject, view));
      parts.activity.replaceChildren(...nodes);
    }
  }
  if (agentGraphInspectorTab === "details") {
    const signature = JSON.stringify([locale, subject?.key, subject?.label, subject?.project?.label,
      subject?.workspace?.label, subject?.facts.model, subject?.place, subject?.review]);
    if (parts.stamps.details !== signature) {
      parts.stamps.details = signature;
      const nodes = subject ? [
        agentGraphBreadcrumbNode(subject, full),
        agentGraphDetailField(t("board.graph.agent", "에이전트"), agentName(subject.card.agent)),
        agentGraphDetailField(t("board.graph.model", "모델"), subject.facts.model),
        agentGraphDetailField(t("board.graph.run", "런"), subject.place.run),
        agentGraphDetailField(t("board.graph.taskId", "과업 ID"), subject.place.taskId),
        agentGraphDetailField(t("board.graph.workerId", "워커 ID"), subject.place.workerId),
        agentGraphDetailField(t("board.graph.dispatchId", "배차 ID"), subject.place.dispatchId),
        agentGraphDetailField(t("board.graph.attemptStarted", "시도 시작"), agentGraphTimeWord(subject.place.dispatchStarted)),
        agentGraphDetailField(t("board.graph.retryOf", "이전 시도"), subject.place.retryOf),
        agentGraphBacktraceNode(subject, full),
      ].filter(Boolean) : [];
      if (subject?.card.agent === "zo") nodes.push(zoIntegrationNode(subject.card.pane));
      parts.details.replaceChildren(...nodes);
    }
  }
  paintAgentInspectorDestinations(view, subject);
}

function selectAgentGraphEntity(view, key, { focus = false } = {}) {
  const drawn = agentGraphModels.get(view);
  let model = agentGraphFullModel(view);
  if (!model) return;
  const entity = drawn?.entities.get(key) ?? model.entities.get(key);
  const entry = entity ?? model.agents.find((one) => one.key === key);
  if (!entry) return;
  if (!drawn?.entities.has(key)) {
    agentGraphScopeKey = "";
    agentGraphScopeLabel = "";
  }
  if (agentGraphScopeKey === "" && !model.nodes.some((one) => one.key === key) && entry.workspace) {
    agentGraphIdleExpanded.add(entry.workspace.key);
    agentGraphFolded.delete(entry.project.key);
    agentGraphDormantExpanded.add(entry.project.key);
    const source = model.source;
    model = agentGraphModel(source.columns, source.places, source.reviews, Date.now(), model,
      source.catalog, source.search, source.overlays);
  }
  agentGraphSelectedEdgeKey = null;
  agentGraphSelectedKey = key;
  /* 좁은 판에서 인스펙터는 **고른 다음에** 온다 (t-2374). 첫 판의 기본 선택이
   * 서랍을 열면, 그림을 보려고 보드를 연 사람이 맨 처음 보는 것은 그림을 덮은
   * 판이다. 이 손은 사람이 실제로 고른 길이고 — 클릭과 화살표 — 그래서 여는
   * 자리도 여기다. 넓은 판에서는 이 클래스가 아무 규칙에도 걸리지 않는다. */
  setAgentGraphInspectorOpen(view, true);
  paintAgentGraph(view, model);
  void syncPreviewedTerms();
  if (focus) focusAgentGraphSelection(view);
}

function focusAgentGraphSelection(view) {
  if (!agentGraphSelectedKey) return;
  const node = [...view.querySelectorAll("[data-graph-key]")]
    .find((candidate) => candidate.dataset.graphKey === agentGraphSelectedKey);
  node?.scrollIntoView({ block: "center", inline: "center", behavior: "smooth" });
  node?.focus({ preventScroll: true });
}

function softlyFollowAgentGraphSelection(view) {
  if (!agentGraphSelectedKey) return;
  const node = [...view.querySelectorAll("[data-graph-key]")]
    .find((candidate) => candidate.dataset.graphKey === agentGraphSelectedKey);
  node?.scrollIntoView({ block: "center", inline: "center", behavior: "smooth" });
}

/* ---- shared graph mechanics ---------------------------------------------
 *
 * Two surfaces in this window draw a graph: the agent ontology below, and the
 * second brain's knowledge graph (`shell-knowledge.js`). They disagree about nearly
 * everything a graph can disagree about — one is a CSS grid whose zoom is a
 * reflow, the other is a force layout in its own coordinate space — and they
 * agree exactly here: the gestures a person makes on a canvas, the arithmetic
 * of fitting a picture into a scrollport, and the read-then-write discipline
 * that keeps an SVG edge repaint from costing one layout per edge.
 *
 * That last one is the reason this section exists rather than a second copy.
 * It was measured once (1-t353: 96 edges, 98 layouts, 16 of 20.2ms in one
 * function) and the fix is not obvious from reading the fixed version — a
 * second surface reinventing the loop would reinvent the bug with it. */

const GRAPH_SVG_NS = "http://www.w3.org/2000/svg";

function graphSvgElement(name) {
  return document.createElementNS(GRAPH_SVG_NS, name);
}

/* One pending frame per view, cancelled if a second is asked for before it
 * runs. Both surfaces schedule two kinds of work this way (a repaint of the
 * edges, and a fit), so the map that holds the pending frame is the caller's
 * and this only owns the cancel-and-replace. */
function scheduleGraphFrame(frames, view, run) {
  const held = frames.get(view);
  if (held !== undefined) cancelAnimationFrame(held);
  const frame = requestAnimationFrame(() => {
    frames.delete(view);
    run();
  });
  frames.set(view, frame);
}

/* Two decimal places, inside the surface's own range. Rounded because a zoom
 * is written into the DOM and 0.8200000000000001 is a different string from
 * 0.82 to every write guard in this file. */
/* The zoom grid: hundredths. A hand's zoom rounds to the nearest step; a fit
 * (`fittedGraphZoom`) takes the step BELOW its ratio, never the one above. */
const GRAPH_ZOOM_STEPS = 100;

function clampGraphZoom(next, min, max) {
  return Math.min(max, Math.max(min, Math.round(next * GRAPH_ZOOM_STEPS) / GRAPH_ZOOM_STEPS));
}

/* A fit never rounds up.
 *
 * The nearest step above the ratio draws the picture up to half a step wider
 * than its frame — measured in the release lane (v1.3.77 gate-root): a ratio of
 * about 0.845 rounded to 0.85 stood a 1074px picture 4px past a frame it had
 * fitted at 0.84 in the run before, and the second pass, dividing by the
 * reflowed width, landed on the same step. The step below fits by
 * construction. The auto-fit floor is applied after this, so the floor itself
 * still rounds to its own step (1/1.55 → 0.65). */
function fittedGraphZoom(ratio) {
  return Math.floor(ratio * GRAPH_ZOOM_STEPS + 1e-9) / GRAPH_ZOOM_STEPS;
}

/* A wheel notch, as a multiplier. */
const GRAPH_WHEEL_ZOOM_IN = 1.08;
const GRAPH_WHEEL_ZOOM_OUT = 0.92;

function graphWheelZoomFactor(deltaY) {
  return deltaY > 0 ? GRAPH_WHEEL_ZOOM_OUT : GRAPH_WHEEL_ZOOM_IN;
}

/* A click is a pointerdown that moved a little. */
const GRAPH_DRAG_SLOP = 3;

/* Drag the empty canvas to move the picture.
 *
 * The guard is what the pointer went down ON: a drag that starts on a node is
 * that node's own gesture, and stealing it would make every node un-clickable
 * the moment its surface grew a pan. `grabbed` is the surface's own override —
 * a held space bar on the ontology — for panning from on top of a node.
 *
 * `onGrab` runs at pointerdown so the caller can record where the picture
 * stood; `onDrag` gets the offset from that point and only once the slop has
 * been crossed; `onEnd` is told whether a pan actually happened, because a
 * pointerup that never moved is a click somebody else is about to handle. */
function wireGraphDrag(surface, { grabbed, onGrab, onDrag, onEnd } = {}) {
  surface.onpointerdown = (event) => {
    if (event.button !== 0 && event.button !== 1) return;
    if (event.button === 0 && !grabbed?.(event) &&
        event.target.closest("button, a, input, textarea")) return;
    const startX = event.clientX;
    const startY = event.clientY;
    let panning = false;
    onGrab?.(event);
    const move = (held) => {
      if (!panning
        && Math.hypot(held.clientX - startX, held.clientY - startY) < GRAPH_DRAG_SLOP) return;
      panning = true;
      onDrag?.(held.clientX - startX, held.clientY - startY);
    };
    const stop = () => {
      onEnd?.(panning);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  };
}

/* The far corner of what is actually drawn inside `host`.
 *
 * Neither a canvas nor a band head can answer this on the ontology: both are
 * stretched to the width of the scrollport, so dividing by either one says
 * "already fitted" no matter how small the graph really is — which is why a
 * fit could only ever zoom OUT. The host's own rectangle is read once, not
 * per node: measuring one picture must not read one rectangle per card twice. */
function graphDrawnSize(host, selector) {
  let wide = 0;
  let tall = 0;
  const frame = host.getBoundingClientRect();
  for (const drawn of host.querySelectorAll(selector)) {
    const box = drawn.getBoundingClientRect();
    wide = Math.max(wide, box.right - frame.left);
    tall = Math.max(tall, box.bottom - frame.top);
  }
  return { wide, tall };
}

/* The width a picture has to fit into.
 *
 * The padding is the scrollport's own rather than a number written twice:
 * `clientWidth` includes it, and a surface whose padding moved would otherwise
 * fit its picture outside itself (measured on the ontology: padding 40 fitted
 * a 463px picture into a 429px seat). */
function graphFitRoom(scroll) {
  const dress = getComputedStyle(scroll);
  return scroll.clientWidth
    - Number.parseFloat(dress.paddingLeft) - Number.parseFloat(dress.paddingRight);
}

/* One ResizeObserver for every graph canvas in this window, and one callback
 * per canvas. Registered once per element — a repaint passes through here on
 * every hook event, and an observer added per repaint is a leak that grows
 * with how busy the surface is. */
let graphResizeObserver = null;
const graphResizeRuns = new WeakMap();

function watchGraphResize(canvas, run) {
  if (graphResizeRuns.has(canvas) || typeof ResizeObserver !== "function") return;
  if (graphResizeObserver === null) {
    graphResizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) graphResizeRuns.get(entry.target)?.(entry.target);
    });
  }
  graphResizeRuns.set(canvas, run);
  graphResizeObserver.observe(canvas);
}

/* Does any of these words hold the needle? Both sides locale-folded, so a
 * query in 한글 or with an accent matches the way the language says it does.
 * `clean` is the surface's own reading of a value — the ontology strips the
 * control bytes a pty put in a title before it compares. */
function graphSearchMatch(needle, values, clean = (value) => String(value ?? "")) {
  if (needle === "") return true;
  return values.some((value) => clean(value).toLocaleLowerCase().includes(needle));
}

/* The coordinates one edge was last drawn at, per `<path>`.
 *
 * Numbers rather than the `d` string: comparing strings would mean building
 * the string this guard exists to avoid building. Shared between the two
 * surfaces because the map is keyed by the element. */
const graphEdgeGeometry = new WeakMap();

function sameGraphGeometry(left, right) {
  if (left === undefined || left.length !== right.length) return false;
  for (let at = 0; at < left.length; at += 1) {
    if (left[at] !== right[at]) return false;
  }
  return true;
}

/* One `<g data-graph-edge>` per relation, reused across repaints.
 *
 * Everything the caller hands in is already MEASURED. Writing a `d` dirties
 * the layout, and the next edge's `offsetLeft` would then force a synchronous
 * re-layout — that is how one repaint's layout count came to follow the edge
 * count. Read above this call, write inside it.
 *
 * Each `edge` carries a `key`, an `at` array of the numbers its shape is made
 * of, and whatever else the two callbacks need. `path(edge)` answers the `d`
 * and is called only when `at` actually moved; `dress(group, edge, line)` puts
 * the classes and any label on it, because the shape of a relation and the
 * words beside it are the part of an edge that belongs to its own surface. */
function paintGraphEdges(host, edges, { path, dress, marker = null, frame = null } = {}) {
  /* 담는 <g>를 받는다 — 그 <svg> 안의 첫 <g>가 아니라. 지식 그래프는 점과 선을
   * 한 <svg>에 두므로 「첫 <g>」는 문서 순서에 따라 달라지는 답이고, 순서에
   * 기대는 계약은 층을 하나 더 얹는 날 조용히 어긋난다. */
  const svg = host.ownerSVGElement ?? host;
  if (frame) writeAttribute(svg, "viewBox", `0 0 ${frame[0]} ${frame[1]}`);
  if (marker && !svg.querySelector("defs")) {
    const defs = graphSvgElement("defs");
    defs.innerHTML = `<marker id="${marker}" viewBox="0 0 6 6" refX="5" refY="3" `
      + `markerWidth="6" markerHeight="6" orient="auto-start-reverse">`
      + `<polygon points="0 0, 6 3, 0 6" fill="currentColor"></polygon></marker>`;
    svg.prepend(defs);
  }
  const existing = new Map([...host.children].map((group) => [group.dataset.graphEdge, group]));
  const wanted = [];
  for (const edge of edges) {
    const group = existing.get(edge.key) ?? graphSvgElement("g");
    writeAttribute(group, "data-graph-edge", edge.key);
    const line = group.querySelector("path") ?? group.appendChild(graphSvgElement("path"));
    /* 좌표가 그대로면 **짓지도** 않는다 (1-t385). 쓰기를 막는 것은 지어 놓고
     * 버리는 일까지 막지 못했다: 카드 백 장이면 아무것도 움직이지 않은 박자마다
     * 아흔여섯 개의 경로가 새로 지어져 비교되고 버려졌다.
     *
     * 들고 있는 좌표는 **복사본**이다. 매 프레임 배치가 움직이는 표면은 같은
     * 배열을 제자리에서 고쳐 쓰므로(프레임마다 이천 개를 새로 짓지 않으려고),
     * 참조를 그대로 쥐고 있으면 이 문이 언제나 「같다」고 답한다. */
    if (!sameGraphGeometry(graphEdgeGeometry.get(line), edge.at)) {
      graphEdgeGeometry.set(line, [...edge.at]);
      writeAttribute(line, "d", path(edge));
    }
    dress?.(group, edge, line);
    wanted.push(group);
  }
  reconcileElementOrder(host, wanted);
}

/* ---- zoom, pan and fit -------------------------------------------------- */

/* Zoom is a REFLOW, not a transform.
 *
 * A transformed canvas keeps its layout size, so the scroll extents stop
 * matching what is drawn and the sticky lane strip sticks to the wrong box.
 * Scaling the lane tokens instead means the browser lays the graph out at the
 * size a person asked for, and every measure this file takes afterwards — the
 * edge endpoints included — is already true without a correction factor. */
function applyAgentGraphZoom(view, { remeasure = true } = {}) {
  // On the canvas and not on the layout: the inspector is the layout's other
  // half, and its words are read at the size the rest of the window uses. A
  // zoom that reached it would shrink a panel nobody asked to zoom.
  const surface = view.querySelector(".agent-graph-surface");
  surface.style.setProperty("--agent-graph-zoom", String(agentGraphZoom));
  const tuning = agentGraphTuning(view);
  if (tuning) surface.classList.toggle("is-lod", agentGraphZoom < tuning.lod);
  const label = view.querySelector(".agent-graph-zoom-label");
  // 같은 낱말을 다시 써 넣지 않는다: 매 판 지나가는 길이고, 같은 글자로 텍스트
  // 노드를 갈아 끼우는 것도 그 자리의 스타일을 다시 계산시키는 쓰기다.
  if (label) writeTextContent(label, `${Math.round(agentGraphZoom * 100)}%`);
  // A repaint spends this to restore the zoom on a freshly built view; it must
  // not also remeasure, or the topology guard below stops meaning anything and
  // every hook event walks every edge again.
  if (remeasure) scheduleAgentGraphEdges(view);
}

function setAgentGraphZoom(view, next) {
  const tuning = agentGraphTuning(view);
  if (!tuning) return;
  const held = clampGraphZoom(next, tuning.zoomMin, tuning.zoomMax);
  if (held === agentGraphZoom) return;
  agentGraphZoom = held;
  applyAgentGraphZoom(view);
}

/* 사람이 배율을 손에 쥔 순간부터 판은 저절로 앉지 않는다.
 *
 * 자동 맞춤은 "아직 아무도 배율을 정하지 않았다"는 동안만 하는 일이다. 이것이
 * 없으면 에이전트 하나가 시작할 때마다 사람이 방금 맞춰 둔 배율을 창이 되돌리고,
 * 그 되돌림은 하필 판이 가장 바쁠 때 가장 자주 일어난다. 「전체 보기」는 여기에
 * 들지 않는다 — 그것은 맞춰 달라는 청이지 배율을 정하겠다는 뜻이 아니다. */
let agentGraphZoomTaken = false;

function takeAgentGraphZoom(view, next) {
  agentGraphZoomTaken = true;
  setAgentGraphZoom(view, next);
}

/* The canvas is stretched to fill its frame, so its own box says nothing about
 * how big the PICTURE is. The drawn extent does: the far corner of the furthest
 * node is the only number a fit can honestly divide by, and it is the number
 * that lets a small graph zoom back IN instead of sitting at 50% forever. */
/* The far corner of the drawn picture.
 *
 * Neither the canvas nor a band head can answer this: both are stretched to
 * the width of the scrollport by `min-width: 100%`, so dividing by either one
 * says "already fitted" no matter how small the graph really is — which is why
 * a fit could only ever zoom OUT. What is actually drawn is the cards and the
 * band rails, and a rail is `width: max-content`, so it measures the heading
 * rather than the stretch behind it. */
function agentGraphDrawnSize(view) {
  const drawn = graphDrawnSize(
    view.querySelector(".agent-graph-nodes"),
    ".agent-graph-node, .agent-graph-band-rail",
  );
  /* 폭은 그림의 **제** 폭이다 (round three). 레인이 남는 폭을 나눠 가지므로
   * 그려진 카드의 오른쪽 끝은 언제나 판의 끝이고, 그것을 나누면 답은 늘
   * 「이미 맞다」다 — 한 번 줄어든 배율이 판이 넓어져도 돌아오지 못했다(실측:
   * 파일 패널을 접어도 71%). 빈 상자가 레인이 실측 폭일 때의 폭을 CSS의 같은
   * 셈으로 들고 있으므로 그것을 잰다; 높이는 그려진 것에서 그대로 읽는다. */
  const extent = view.querySelector(".agent-graph-extent");
  return extent && extent.offsetWidth > 0 ? { wide: extent.offsetWidth, tall: drawn.tall } : drawn;
}

/* 저절로 앉는 판이 내려갈 수 있는 바닥은, 판이 받은 밀도만큼이다 (t-1853).
 *
 * 손으로 적은 0.75는 한 화면 폭을 보고 고른 수였고, 인스펙터가 선 1440px에서는
 * 그 수 때문에 그림이 104px 잘린 채로 맞춤이 끝났다(실측: 그림 581px, 앉을
 * 자리 477px). 진짜 바닥은 이미 토큰에 적혀 있다 — 에이전트 레인은 실측
 * 172px에 밀도(`--agent-graph-density`)를 곱한 폭이고, 그 밀도는 카드가 도구
 * 호출의 경로를 담기 위해 **덧입은** 것이다. 그러므로 저절로 하는 맞춤은 그
 * 덧입은 것을 도로 내어놓을 수 있고, 실측 레인보다 좁아지는 일은 할 수 없다:
 * 바닥은 `1 / density`(1.55 → 0.65)이며, 그 자리에서 카드는 172px, 제목은 제
 * `max()` 바닥인 10px이다. 손으로는 `AGENT_GRAPH_ZOOM_MIN`까지 더 내려간다.
 *
 * 수는 CSS에서 읽는다 — 이 파일이 픽셀 어휘를 두 벌 들지 않는다는 규칙 그대로.
 * 토큰을 읽지 못하는 판에서만 옛 상수가 대신 선다. */
function agentGraphAutoFitFloor(view) {
  const tuning = agentGraphTuning(view);
  if (!tuning) return 1;
  const surface = view.querySelector(".agent-graph-surface");
  const density = Number.parseFloat(
    getComputedStyle(surface).getPropertyValue("--agent-graph-density"));
  /* 밀도가 읽히지 않으면 토큰에 적힌 바닥을 그대로 쓴다 — 그 수는 지금의
   * 밀도에서 나온 값이므로 둘은 늘 같은 답이고, 여기 손으로 적을 수는 그래서
   * 하나도 없다. */
  if (!Number.isFinite(density) || density <= 1) return tuning.fitFloor;
  return Math.max(tuning.zoomMin, 1 / density);
}

/* Widths scale with the zoom exactly; heights do not, because a card's text has
 * a floor it will not shrink past. So the fit lands in two steps — the ratio,
 * then whatever the reflow actually gave — and stops as soon as the second one
 * has nothing left to correct. */
function fitAgentGraph(view, { asked = true, pass = 0 } = {}) {
  /* 사람이 배율을 쥐었으면 저절로 앉는 일은 끝난다 — 예약한 프레임이 이미
   * 날아가고 있었더라도 (1-t385).
   *
   * 예약은 위상이 움직인 판에서 걸리고 그 프레임은 16밀리초 뒤에 온다. 그
   * 사이는 사람도 쓰는 시간이고, 위상은 에이전트 하나가 시작할 때마다 움직이므로
   * 그 창은 판이 가장 바쁠 때 가장 자주 열린다 — 실측: 축소를 누른 손이 97%를
   * 82%로 내린 직후 예약된 프레임이 75%로 되돌렸다. 「사람이 쥐었는가」의 답은
   * 예약할 때가 아니라 **쓸 때** 유효해야 한다. 둘째 판(`pass === 1`)도 같은
   * 문을 지난다: 그 사이에도 손은 움직인다. */
  if (!asked && agentGraphZoomTaken) return;
  const scroll = view.querySelector(".agent-graph-scroll");
  const { wide, tall } = agentGraphDrawnSize(view);
  if (wide <= 0 || tall <= 0) return;
  /* 여백은 판이 이미 들고 있는 그것이다 (1-t385).
   *
   * 여기 서 있던 것은 "캔버스의 제 여백과 같게"라고 적힌 24였고, 그것은
   * `--space-5`를 손으로 한 번 더 적은 값이었다. 같은 간격이 두 곳에 적히면
   * 한 곳만 움직이는 날이 오고, 그날 맞춘 그림은 판 밖으로 나간다 — 실측:
   * 판의 여백을 40으로 두면 맞춘 그림이 안쪽 429px 자리에 463px로 앉았다.
   * `clientWidth`는 여백을 품은 폭이므로, 빼고 남는 것이 그림이 앉을 자리다. */
  /* 나누는 것은 **폭**이다 (t-1853).
   *
   * 여기 있던 것은 폭과 높이 중 작은 비율이었고, 그래서 카드가 늘어나 그림이
   * 길어지는 것만으로 배율이 바닥까지 눌렸다 — 실측(1440×900, 파일 패널 접음,
   * 세 열): 에이전트 둘이면 100%, 같은 폭에 스무 개를 쌓으면 75%였고 그때
   * 가로로 넘친 것은 0px이었다. 사람이 보는 화면은 카드가 왼쪽에 몰리고
   * 오른쪽은 빈 모눈이며 눈금은 75%에 붙어 있는 그림이 된다.
   *
   * 두 축은 같은 축이 아니다. 세로는 스크롤이 있는 축이고, 아래로 이어지는
   * 것은 「더 많은 에이전트」다 — 이 온톨로지가 세로에 두기로 한 바로 그것.
   * 가로로 잘려 나가는 것은 한 층(하위 에이전트)이고 그것이 사라진 자리에는
   * 아무 표시도 남지 않는다. 그래서 맞춤은 폭을 맞추고, 길이는 스크롤에
   * 맡긴다. 그림이 판보다 작을 때 가운데로 가는 일은 캔버스의 몫이다
   * (`place-content`). */
  const room = graphFitRoom(scroll) / wide;
  /* 청해서 하는 맞춤은 위아래로 다 간다. 저절로 앉는 판은 100%와 낱말의 바닥
   * 사이에만 머문다 — 카드 셋을 160%로 부풀린 것은 「맞춘」 그림이 아니라 다른
   * 그림이고(작은 그림을 판 한가운데 놓는 일은 이제 캔버스가 한다,
   * `place-content`), 낱말이 더 줄지 않는 지점 밑으로 내려간 그림은 맞춘 것이
   * 아니라 읽을 수 없게 된 것이다. */
  const fitted = fittedGraphZoom(agentGraphZoom * room);
  const want = asked ? fitted : Math.min(1, Math.max(agentGraphAutoFitFloor(view), fitted));
  const before = agentGraphZoom;
  setAgentGraphZoom(view, want);
  // 사람의 스크롤을 빼앗는 것은 사람이 청했을 때만. 에이전트 하나가 시작할
  // 때마다 읽던 자리가 맨 위로 튕기면 그 자동은 도움이 아니라 방해다.
  if (asked) scroll.scrollTo({ left: 0, top: 0 });
  if (pass === 0 && agentGraphZoom !== before) {
    requestAnimationFrame(() => fitAgentGraph(view, { asked, pass: 1 }));
  }
}

function agentGraphShortcut(view, key) {
  if (key === "1") {
    agentGraphZoomTaken = true;
    fitAgentGraph(view);
    return true;
  }
  if (key === "2") {
    agentGraphZoomTaken = true;
    setAgentGraphZoom(view, Math.max(1.15, agentGraphZoom));
    softlyFollowAgentGraphSelection(view);
    return true;
  }
  return false;
}

/* 그림이 판을 넘쳤을 때, 아직 배율이 사람의 것이 아니면 판에 맞춰 앉힌다.
 *
 * 실측: 카드 100장이면 그림은 775×11282이고 스크롤포트는 749×889 — 112 노드 중
 * 103이 첫 화면 밖에 있는데 배율은 그대로 100%였다. 「전체 보기」 단추가 있었지만
 * 그것은 누른 사람에게만 있는 기능이고, 처음 열어 본 사람에게 이 표면은 끝없는
 * 세로 목록이다. 배치가 끝난 다음 프레임에 재는 것은 지금 세운 DOM의 크기를
 * 물어야 하기 때문이고, 판마다가 아니라 **위상이 움직인 판**에만 도는 것은
 * 카드의 낱말 하나가 바뀐 박자에는 잴 것이 없기 때문이다. */
const agentGraphFitFrames = new WeakMap();

function scheduleAgentGraphFit(view) {
  if (agentGraphZoomTaken) return;
  /* 목록 모드에는 맞춤이 없다 (t-2374). 한 열로 접힌 그림에 배율은 뜻이
   * 없고 — 줄이면 카드가 좁아지는 것이 아니라 낱말만 작아진다 — 계속 돌면
   * 폭을 바꿀 때마다 눈금이 흔들린다. 판단은 폭을 이미 재는
   * `watchAgentGraphSize`가 내려 이 클래스로 적어 두었으므로, 여기서 다시
   * 재지 않는다: 이 함수는 위상이 움직인 판마다 지나는 길이다. */
  if (view.classList.contains("is-graph-list")) return;
  scheduleGraphFrame(agentGraphFitFrames, view, () => fitAgentGraph(view, { asked: false }));
}

/* Arrow keys, because the surface says `role="tree"`.
 *
 * A tree that can only be walked with Tab is a tree in name only, and the name
 * is what a screen reader announces before anything else. Up and down walk the
 * items in the order they are drawn; right opens a band or steps along a
 * relation to what it holds; left closes a band or steps back to what holds
 * this. One item carries `tabIndex 0` at a time (the selected one), so Tab
 * enters the graph once and the arrows do the rest. */
function agentGraphWalk(view, model, from, key) {
  const items = [...view.querySelectorAll(".agent-graph-nodes [data-graph-key]")];
  const at = items.indexOf(from);
  if (at < 0) return null;
  if (key === "ArrowDown") return items[Math.min(items.length - 1, at + 1)];
  if (key === "ArrowUp") return items[Math.max(0, at - 1)];
  if (key === "Home") return items[0];
  if (key === "End") return items[items.length - 1];
  const here = from.dataset.graphKey;
  const entity = model.entities.get(here);
  if (key === "ArrowRight") {
    if (entity?.type === "project" && entity.folded) {
      agentGraphFoldProject(view, here, { focus: true });
      return null;
    }
    const below = model.edges.find((edge) => edge.from === here);
    return below
      ? items.find((item) => item.dataset.graphKey === below.to) ?? null
      : items[Math.min(items.length - 1, at + 1)];
  }
  if (key === "ArrowLeft") {
    if (entity?.type === "project") {
      if (!entity.folded) agentGraphFoldProject(view, here, { focus: true });
      return null;
    }
    const above = model.edges.find((edge) => edge.to === here);
    const target = above?.from ?? entity?.band?.key;
    return items.find((item) => item.dataset.graphKey === target) ?? null;
  }
  return null;
}

function wireAgentGraphKeys(view) {
  const host = view.querySelector(".agent-graph-nodes");
  // 한 번. 걸을 판은 누를 때 물으면 되므로 이 손도 판을 붙들 이유가 없다.
  if (host.onkeydown) return;
  host.onkeydown = (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
      return;
    }
    const from = event.target.closest("[data-graph-key]");
    const model = agentGraphModels.get(view);
    if (!from || !model) return;
    event.preventDefault();
    const next = agentGraphWalk(view, model, from, event.key);
    if (next) selectAgentGraphEntity(view, next.dataset.graphKey, { focus: true });
  };
}

/* Drag the empty grid to move the picture.
 *
 * The guard is what the pointer went down ON: a drag that starts on a node is
 * that node's own gesture, and stealing it would make every node un-clickable
 * the moment this surface grew a pan. The three-pixel slop is the same reason —
 * a click is a pointerdown that moved a little. */
function wireAgentGraphCanvas(view) {
  /* 접은 사실은 사람의 것이라 다시 그려도 유지된다 — 그래서 이 둘은 한 번이
   * 아니라 판마다 맞춘다. 나머지 손들은 아래에서 한 번만 맨다. */
  const toggle = view.querySelector(".agent-graph-legend-toggle");
  const legend = view.querySelector(".agent-graph-legend");
  if (toggle && legend) {
    legend.hidden = !agentGraphLegendOpen;
    writeAttribute(toggle, "aria-expanded", String(agentGraphLegendOpen));
  }
  /* 서랍·시트·오버레이 팝오버의 손. 표면의 나머지 손들과 같은 규칙으로 한
   * 번만 맨다 — 이 함수는 훅 하나마다 지나는 길이다 (t-2374). */
  const scrim = view.querySelector(".agent-graph-scrim");
  if (scrim && !scrim.onclick) scrim.onclick = () => setAgentGraphInspectorOpen(view, false);
  const close = view.querySelector(".agent-inspector-close");
  if (close && !close.onclick) close.onclick = () => setAgentGraphInspectorOpen(view, false);
  const overlays = view.querySelector(".agent-graph-overlay-toggle");
  if (overlays && !overlays.onclick) {
    overlays.onclick = () => {
      const open = overlays.getAttribute("aria-expanded") !== "true";
      overlays.setAttribute("aria-expanded", String(open));
      view.classList.toggle("is-overlays-open", open);
    };
  }
  /* 시안의 머리 오른쪽 아이콘 — 서랍 티어에서 인스펙터를 여닫는 손잡이 (t-4145).
   * 넓은 판에서는 CSS가 감추고, 열림은 `setAgentGraphInspectorOpen` 한 곳이 적는다. */
  const inspectorToggle = view.querySelector(".agent-inspector-toggle");
  if (inspectorToggle && !inspectorToggle.onclick) {
    inspectorToggle.onclick = () => setAgentGraphInspectorOpen(view, !view.classList.contains("is-inspector-open"));
  }
  const layout = view.querySelector(".agent-graph-layout");
  if (layout && !layout.onkeydown) {
    /* Escape 는 가장 바깥의 덮개부터 걷는다: 팝오버가 서 있으면 그것을, 아니면
     * 서랍을. 둘 다 없으면 이 키는 이 판의 것이 아니므로 위로 흘려보낸다. */
    layout.onkeydown = (event) => {
      if (event.key !== "Escape") return;
      if (view.classList.contains("is-overlays-open")) {
        view.classList.remove("is-overlays-open");
        overlays?.setAttribute("aria-expanded", "false");
        event.stopPropagation();
        return;
      }
      if (!view.classList.contains("is-inspector-open")) return;
      setAgentGraphInspectorOpen(view, false);
      event.stopPropagation();
    };
  }
  const scroll = view.querySelector(".agent-graph-scroll");
  /* 손은 표면에 한 번만 맨다 (1-t385).
   *
   * 이 표면은 훅 하나에 한 번씩 다시 그려지고, 이 함수는 그 길 위에 있다 —
   * 여섯 개의 클로저를 판마다 새로 짓는다는 뜻이었다. 이 손들이 잡는 것은
   * 그때그때의 판이 아니라 표면 자체이고, 표면은 다시 그려도 그대로다. */
  if (scroll.onpointerdown) return;
  /* 판을 미는 것은 스크롤이다 — 이 표면은 배율을 리플로로 주므로 그려진 그림의
   * 크기가 곧 스크롤 범위다. 지식 그래프는 같은 몸짓을 받아 제 viewBox를
   * 옮긴다(`wireGraphDrag`가 나누는 것은 몸짓이고, 그것이 무엇을 움직이는지는
   * 표면마다 다르다). */
  let fromLeft = 0;
  let fromTop = 0;
  wireGraphDrag(scroll, {
    grabbed: () => agentGraphSpaceHeld,
    onGrab: () => {
      fromLeft = scroll.scrollLeft;
      fromTop = scroll.scrollTop;
    },
    onDrag: (moveX, moveY) => {
      agentGraphPauseFollow(view);
      scroll.classList.add("is-panning");
      scroll.scrollLeft = fromLeft - moveX;
      scroll.scrollTop = fromTop - moveY;
    },
    onEnd: () => scroll.classList.remove("is-panning"),
  });
  // Only with the modifier held: a bare wheel over a graph this tall is a
  // scroll, and a surface that zooms on it is one nobody can read.
  scroll.onwheel = (event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    agentGraphPauseFollow(view);
    takeAgentGraphZoom(view, agentGraphZoom * graphWheelZoomFactor(event.deltaY));
  };
  const out = view.querySelector(".agent-graph-zoom-out");
  const into = view.querySelector(".agent-graph-zoom-in");
  const fit = view.querySelector(".agent-graph-fit");
  // 눈금 한 칸은 잡을 때 묻는다 — 손을 맬 때가 아니라. 이 손은 뷰보다 오래
  // 살고, 조율은 그 뷰의 표면이 서 있어야 읽힌다.
  if (out) {
    out.onclick = () =>
      takeAgentGraphZoom(view, agentGraphZoom - (agentGraphTuning(view)?.zoomStep ?? 0));
  }
  if (into) {
    into.onclick = () =>
      takeAgentGraphZoom(view, agentGraphZoom + (agentGraphTuning(view)?.zoomStep ?? 0));
  }
  /* 「전체 보기」도 배율에 손을 대는 일이다 (1-t385).
   *
   * 여기 적혀 있던 판단은 "그것은 맞춰 달라는 청이지 배율을 정하겠다는 뜻이
   * 아니다"였다. 그러나 그 청의 답은 눈금에 남고 — 실측: 카드 예순 장을
   * 맞추면 45% — 다음 에이전트 하나가 시작하는 순간 저절로 앉는 판이 그것을
   * 75%로 되돌렸다. 저절로 앉는 판은 100%와 낱말의 바닥 사이에서만 움직이므로
   * 사람이 방금 본 그림을 **다시 만들지도 못한다**. 되돌릴 수 없는 것을
   * 되돌리는 것은 도움이 아니고, 다시 맞추는 일은 여전히 한 번 누르면 된다. */
  if (fit) {
    fit.onclick = () => {
      agentGraphZoomTaken = true;
      fitAgentGraph(view);
    };
  }
  if (toggle && legend) {
    toggle.onclick = () => {
      agentGraphLegendOpen = !agentGraphLegendOpen;
      legend.hidden = !agentGraphLegendOpen;
      toggle.setAttribute("aria-expanded", String(agentGraphLegendOpen));
    };
  }
}

/* ---- edges -------------------------------------------------------------- */

function scheduleAgentGraphEdges(view) {
  scheduleGraphFrame(agentGraphEdgeFrames, view, () => paintAgentGraphEdges(view));
}

/* ---- 세 티어, DOM 하나 (t-2374) -----------------------------------------
 *
 * 티어를 아는 것은 CSS다. 그림은 어느 폭에서도 같은 노드·같은 키·같은 순서로
 * 서고, 좁아지면 격자가 한 열로 접히고 인스펙터가 곁판에서 서랍으로, 서랍에서
 * 시트로 옮겨 앉는다 — `paintAgentGraph`는 그 셋 중 무엇이 서 있는지 모른다.
 *
 * JS가 알아야 하는 것은 하나뿐이다: **목록 모드에서는 맞춤을 돌리지 않는다.**
 * 한 열로 접힌 그림에 배율은 뜻이 없고(줄이면 낱말만 작아진다), 맞춤이 계속
 * 돌면 폭을 바꿀 때마다 눈금이 흔들린다. 문턱은 토큰에 적혀 있으므로 여기서
 * 다시 고르지 않는다 — 읽기만 한다. */
const agentGraphTiers = new WeakMap();

/* ---- 그림의 조율은 토큰이 든다 (t-2374) ----------------------------------
 *
 * 배율의 범위, 눈금 한 칸, LOD 문턱, 저절로 앉는 판의 바닥, 간선이 카드를
 * 비껴 도는 거리 — 여섯 다 이 파일에 손으로 적힌 수였고, 여섯 다 CSS가 정한
 * 밀도와 글자 바닥에서 나온 수였다. 두 곳에 적힌 수는 한 곳만 움직이는 날이
 * 오고, 이 표면에서 그날은 이미 한 번 왔다(맞춤의 여백 24가 `--space-5`의
 * 두 번째 사본이었다, 1-t385).
 *
 * 뷰마다 한 번 읽는다: 이 여섯은 사람이 창을 여는 동안 움직이지 않고,
 * `getComputedStyle`은 배치를 강제하지 않지만 공짜도 아니다.
 *
 * 하나라도 읽히지 않으면 **아무 답도 하지 않는다**. 대비값을 쥐면 그것이
 * 곧 두 번째 사본이고, 이 함수가 없애려던 그것이다. 읽지 못하는 판은
 * 스타일시트가 아직 붙지 않은 판이라는 뜻이므로, 캐시하지 않고 다음 판에
 * 다시 묻는다 — 그 사이 배율은 움직이지 않고, 움직이지 않은 배율은 아무것도
 * 그르치지 않는다. */
const agentGraphTunings = new WeakMap();

function agentGraphTuning(view) {
  const held = agentGraphTunings.get(view);
  if (held) return held;
  const surface = view.querySelector(".agent-graph-surface");
  if (!surface) return null;
  const dress = getComputedStyle(surface);
  const read = (name) => Number.parseFloat(dress.getPropertyValue(name));
  const tuning = {
    zoomMin: read("--agent-graph-zoom-min"),
    zoomMax: read("--agent-graph-zoom-max"),
    zoomStep: read("--agent-graph-zoom-step"),
    lod: read("--agent-graph-lod"),
    fitFloor: read("--agent-graph-fit-floor"),
    edgeStub: read("--agent-graph-edge-stub"),
  };
  if (!Object.values(tuning).every(Number.isFinite)) return null;
  agentGraphTunings.set(view, tuning);
  return tuning;
}

function agentGraphTierWidths(view) {
  const held = agentGraphTiers.get(view);
  if (held) return held;
  const dress = getComputedStyle(view);
  const read = (name, fallback) => {
    const value = Number.parseFloat(dress.getPropertyValue(name));
    return Number.isFinite(value) && value > 0 ? value : fallback;
  };
  const widths = {
    side: read("--agent-graph-tier-side", 1100),
    list: read("--agent-graph-tier-list", 700),
    tight: read("--agent-graph-tier-tight", 400),
  };
  agentGraphTiers.set(view, widths);
  return widths;
}

function agentGraphListMode(view) {
  return view.clientWidth > 0 && view.clientWidth < agentGraphTierWidths(view).list;
}

/* 목록 모드의 배율은 1이고 그 단추는 눌리지 않는다. 한 열에서 축소는 카드를
 * 좁히지 못하고 낱말만 줄이므로, 눌리는 채로 두면 아무 일도 하지 않는 단추가
 * 된다 — 그리고 눈금은 사람이 마지막으로 넓은 판에서 고른 수에 굳어 있다. */
function syncAgentGraphTier(view) {
  const list = agentGraphListMode(view);
  view.classList.toggle("is-graph-list", list);
  for (const button of view.querySelectorAll(
    ".agent-graph-zoom-in, .agent-graph-zoom-out, .agent-graph-fit")) {
    button.disabled = list;
  }
  if (list && agentGraphZoom !== 1) setAgentGraphZoom(view, 1);
  return list;
}

/* 서랍과 시트는 열려 있거나 닫혀 있다 — 그 둘 사이에 세 번째 상태는 없고,
 * 넓은 판에서는 이 클래스가 아무 규칙에도 걸리지 않으므로 곁판은 늘 서 있다. */
function setAgentGraphInspectorOpen(view, open) {
  view.classList.toggle("is-inspector-open", open);
  const inspector = view.querySelector(".agent-inspector");
  if (inspector) writeAttribute(inspector, "aria-expanded", String(open));
  const toggle = view.querySelector(".agent-inspector-toggle");
  if (toggle) writeAttribute(toggle, "aria-expanded", String(open));
}

function watchAgentGraphSize(view) {
  /* 재는 것은 판이지 캔버스가 아니다 (round three). 그림이 판을 넘치면
   * 캔버스는 그림만큼 넓고, 판이 좁아져도 캔버스는 그대로라 알림이 오지
   * 않았다 — 파일 패널을 편 사람의 그림은 좁아진 판에 맞춰 앉지 않았다. */
  watchGraphResize(view.querySelector(".agent-graph-scroll"), (scroll) => {
    const owner = scroll.closest(".file-view");
    if (!owner || !agentGraphModels.has(owner)) return;
    /* 폭을 이미 재고 있는 자리가 여기다 — 티어를 묻기에 가장 싼 자리이기도
     * 하다. 이 콜백은 뷰가 관찰에 들어갈 때 한 번 저절로 오므로, 처음 열린
     * 판도 제 티어를 안다. */
    if (syncAgentGraphTier(owner)) {
      scheduleAgentGraphEdges(owner);
      return;
    }
    scheduleAgentGraphEdges(owner);
    /* 판이 넓어졌으면 그림도 그 자리를 받는다 (t-1853).
     *
     * 맞춤은 여태 **위상이 움직인 판**에서만 걸렸다. 그래서 파일 패널을 접거나
     * 창을 넓혀 자리가 생겨도 그림은 방금 좁던 판에 맞춰 앉은 채로 남았고,
     * 사람이 본 것은 왼쪽에 몰린 카드와 오른쪽의 빈 모눈이다 (실측: 패널을 펴
     * 앉을 자리가 477px일 때 맞춘 65%가, 패널을 접어 827px이 되어도 65%
     * 그대로 — 넘치는 298px 대신 남는 200px). 사람이 배율을 쥐었으면 이 문은
     * `scheduleAgentGraphFit` 안에서 닫힌다. 되먹임은 한 판에서 멎는다: 다시
     * 잰 비율이 1이면 배율이 움직이지 않고, 움직이지 않은 배율은 리플로도 새
     * 알림도 만들지 않는다. */
    scheduleAgentGraphFit(owner);
  });
}

function agentGraphSelectedEdges(model) {
  const selected = agentGraphSelectedKey;
  if (!selected) return new Set();
  const incoming = new Map();
  for (const edge of model.edges) {
    const held = incoming.get(edge.to) ?? [];
    held.push(edge);
    incoming.set(edge.to, held);
  }
  const held = new Set();
  const seen = new Set();
  const pending = [selected];
  while (pending.length > 0) {
    const here = pending.pop();
    if (!here || seen.has(here)) continue;
    seen.add(here);
    for (const edge of incoming.get(here) ?? []) {
      held.add(edge.key);
      pending.push(edge.from);
    }
  }
  return held;
}

/* An elbow, not a bezier.
 *
 * The lanes are twenty pixels apart and the rows they join can be half a band
 * apart, so a curve whose control points sit at the midpoint is a near-vertical
 * stroke crammed into that gap — five of them read as one bar. The orthogonal
 * run says the same thing in the space it has: out of the source, down the
 * gutter, into the target, with a corner small enough to stay inside it. */
function agentGraphEdgePath(
  startX, startY, endX, endY, track = null, stub = 0, gutterX = null,
) {
  const span = endX - startX;
  const rise = endY - startY;
  if (Math.abs(rise) < 1) return `M ${startX} ${startY} H ${endX}`;
  /* Backwards, or too tight to turn in.
   *
   * An agent that one project's agent spawned inside ANOTHER project is drawn
   * as a root of the band it actually lives in, so its lane can be to the LEFT
   * of the lane its parent sits in — the one relation on this surface that runs
   * uphill. There is no gutter between the two to run down, so the route steps
   * clear of the source, crosses on a track in the GAP above the row it is
   * heading for, and comes back in at the target's own edge. The track matters:
   * the long horizontal run passes over every lane between the two, so a track
   * halfway between the rows goes straight through whatever card a third
   * project happens to have at that height, and the caller hands down the gap
   * instead. Answering this case with a straight `H` — the first version — made
   * a cross-band lineage draw a line that stopped in mid-air. */
  if (span <= stub * 2) {
    return [
      `M ${startX} ${startY}`,
      `H ${startX + stub}`,
      `V ${track ?? startY + rise / 2}`,
      `H ${endX - stub}`,
      `V ${endY}`,
      `H ${endX}`,
    ].join(" ");
  }
  /* Forward relations own exactly one column gap. Run horizontally to its
   * centre, vertically through that gutter, then horizontally to the target's
   * edge. A bezier's painted curve can bow back over either card even when its
   * control points live in the gap; orthogonal segments cannot. */
  const turnX = gutterX ?? startX + span * 0.5;
  return [
    `M ${startX} ${startY}`,
    `H ${turnX}`,
    `V ${endY}`,
    `H ${endX}`,
  ].join(" ");
}

/* 재는 일을 다 끝내고 나서 그린다 (1-t353).
 *
 * 예전에는 간선 하나마다 좌표를 읽고 곧바로 그 간선의 `d`를 썼다. `d`를 쓰는
 * 순간 레이아웃이 더러워지고, 바로 다음 간선의 `offsetLeft`가 그것을 **동기로**
 * 다시 계산하게 만든다 — 그래서 다시 그리기 한 번의 레이아웃 횟수가 간선 수를
 * 따라갔다(실측: 카드 100장·간선 96개에 한 판 레이아웃 98회, 20.2ms 중 16ms가
 * 이 함수). 읽는 것은 전부 이 위에서, 쓰는 것은 전부 그 아래에서.
 *
 * 세로 여백도 같은 이유로 한 번만 묻는다: 모든 노드가 `.agent-graph-node` 한
 * 규칙에서 같은 `margin-block`을 받으므로, 간선마다 `getComputedStyle`을
 * 부르는 것은 같은 답을 아흔여섯 번 받으려고 스타일을 아흔여섯 번 다시
 * 계산시키는 일이다. */
function paintAgentGraphEdgeTargets(view, measured, frame) {
  const canvas = view.querySelector(".agent-graph-canvas");
  let layer = canvas.querySelector(".agent-graph-edge-targets");
  if (!layer) {
    layer = graphSvgElement("svg");
    layer.setAttribute("class", "agent-graph-edge-targets");
    layer.setAttribute("aria-hidden", "true");
    layer.appendChild(graphSvgElement("g"));
    canvas.prepend(layer);
  }
  paintGraphEdges(layer.querySelector("g"), measured.filter(({ edge }) =>
    !(edge.type === "contains" && (agentGraphModels.get(view).entities.get(edge.to)?.depth ?? 0) > 0)), {
    frame,
    path: ({ at }) => agentGraphEdgePath(...at),
    dress: (group, { edge }, line) => {
      writeAttribute(line, "class", "agent-graph-edge-hit");
      group.onpointerdown = (event) => {
        group._graphPointer = [event.clientX, event.clientY];
        if (!agentGraphSpaceHeld) event.stopPropagation();
      };
      group.onclick = (event) => {
        if (agentGraphSpaceHeld) return;
        const began = group._graphPointer;
        if (began && Math.hypot(event.clientX - began[0], event.clientY - began[1]) > GRAPH_DRAG_SLOP) return;
        event.stopPropagation();
        selectAgentGraphRelation(view, edge.key);
      };
    },
  });
}

function paintAgentGraphEdges(view, { styleOnly = false } = {}) {
  const model = agentGraphModels.get(view);
  const canvas = view.querySelector(".agent-graph-canvas");
  if (!model || view.hidden || !canvas.isConnected) return;
  const svg = canvas.querySelector(".agent-graph-edges");
  let geometry = agentGraphEdgeMeasurements.get(view);
  const current = geometry && geometry.signature === model.topologySignature
    && geometry.zoom === agentGraphZoom && geometry.details === agentGraphCardDetails
    && geometry.frameWide === canvas.offsetWidth && geometry.frameTall === canvas.offsetHeight;
  if (!styleOnly || !current) {
    agentGraphEdgeMeasureRuns += 1;
    const nodesHost = canvas.querySelector(".agent-graph-nodes");
    const drawn = [...canvas.querySelectorAll(".agent-graph-node")];
    const nodes = new Map(drawn.map((node) => [node.dataset.graphKey, {
      offsetLeft: node.offsetLeft, offsetTop: node.offsetTop,
      offsetWidth: node.offsetWidth, offsetHeight: node.offsetHeight,
    }]));
    geometry = {
      signature: model.topologySignature, zoom: agentGraphZoom, details: agentGraphCardDetails,
      nodes, originX: nodesHost.offsetLeft, originY: nodesHost.offsetTop,
      frameWide: canvas.offsetWidth, frameTall: canvas.offsetHeight,
      gap: drawn.length ? Number.parseFloat(getComputedStyle(drawn[0]).marginTop) || 0 : 0,
      stub: agentGraphTuning(view)?.edgeStub ?? 0,
    };
    agentGraphEdgeMeasurements.set(view, geometry);
  }
  const { nodes, originX, originY, frameWide, frameTall, gap, stub } = geometry;
  const measured = [];
  for (const edge of [...model.edges, ...(model.overlayEdges ?? [])]) {
    const from = nodes.get(edge.from);
    const to = nodes.get(edge.to);
    if (!from || !to) continue;
    const toTop = originY + to.offsetTop;
    const startX = originX + from.offsetLeft + from.offsetWidth;
    const endX = originX + to.offsetLeft;
    /* 두 끝은 같은 높이에서 나가고 든다 (round three). 행의 카드들은 윗선을
     * 맞추므로, 짧은 쪽의 한가운데 높이를 둘 다 쓰면 같은 행의 이름표와
     * 카드 사이는 곧은 한 획이 된다 — 각자의 한가운데를 쓰면 키 차이만큼
     * 도랑 안에서 꺾이는 열다섯 픽셀짜리 계단이 그려졌다. 다른 행으로 가는
     * 선은 그 높이에서 도랑을 타고 내려가 같은 높이로 들어간다. */
    const anchor = Math.min(from.offsetHeight, to.offsetHeight) / 2;
    measured.push({
      key: edge.key,
      edge,
      at: [
        startX,
        originY + from.offsetTop + anchor,
        endX,
        toTop + anchor,
        toTop - gap / 2,
        stub,
        startX + (endX - startX) / 2,
      ],
    });
  }

  const selected = agentGraphSelectedEdges(model);
  /* 값이 같으면 쓰지 않고, 좌표가 그대로면 경로를 짓지도 않는다 — 그 규율은
   * `paintGraphEdges`가 들고 있다. 여기 남은 것은 이 표면만의 옷: 선의 종류와
   * 강조, 그리고 오버레이가 선 위에 얹는 낱말과 맥박이다. */
  paintGraphEdges(svg.querySelector("g"), measured, {
    marker: "agent-graph-arrow",
    frame: [frameWide, frameTall],
    path: ({ at }) => agentGraphEdgePath(...at),
    dress: (group, { edge, at: [startX, startY, endX, endY] }, line) => {
      const lit = selected.has(edge.key) || edge.type === "dependency" || edge.key === agentGraphSelectedEdgeKey;
      const searchDimmed = model.searchActive && (
        model.entities.get(edge.from)?.searchMatch === false ||
        model.entities.get(edge.to)?.searchMatch === false
      );
      /* 하위 에이전트의 「품음」은 계보가 이미 말한다 (round three). 이름표에서
       * 둘째 층으로 가는 선은 첫째 층의 카드를 가로질러야만 닿는데, 그 카드가
       * 곧 이 에이전트를 시작한 부모다 — 부모의 점선이 같은 담김을 말하고
       * 있다. 선은 모델과 DOM에 남고(범례의 수, 선택 사슬) 그림에서만 물러난다. */
      const implied = edge.type === "contains" &&
        (model.entities.get(edge.to)?.depth ?? 0) > 0;
      writeAttribute(line, "class", [
        "agent-graph-edge",
        `is-${edge.type}`,
        edge.overlay ? "is-overlay" : "",
        lit ? "is-selected" : "",
        implied ? "is-implied" : "",
        searchDimmed ? "is-search-dimmed" : "",
      ].filter(Boolean).join(" "));
      if (!edge.overlay) return;
      const label = group.querySelector("text")
        ?? group.appendChild(graphSvgElement("text"));
      writeAttribute(label, "class", "agent-graph-overlay-label");
      writeAttribute(label, "x", String((startX + endX) / 2));
      writeAttribute(label, "y", String((startY + endY) / 2 - 5));
      const verb = edge.type === "mail"
        ? t("board.graph.mailVerb", "메일")
        : t("board.graph.dependencyVerb", "선행");
      writeTextContent(label, `${verb} · ${edge.count}`);
      if (edge.type !== "mail") return;
      const pulse = group.querySelector("circle")
        ?? group.appendChild(graphSvgElement("circle"));
      writeAttribute(pulse, "class", "agent-graph-mail-pulse");
      writeAttribute(pulse, "cx", String((startX + endX) / 2));
      writeAttribute(pulse, "cy", String((startY + endY) / 2));
      writeAttribute(pulse, "r", "3");
    },
  });
  paintAgentGraphEdgeTargets(view, measured, [frameWide, frameTall]);
}

/* 머리의 칩이 무엇을 고르는지는 **누를 때**의 판에게 묻는다 (1-t385).
 *
 * 그려질 때의 판을 클로저에 담으면 그 손은 판마다 새로 매여야 하고, 이 판은
 * 훅 하나에 한 번씩 다시 그려진다. 여기 있는 것은 그때 담겨 있던 것 그대로다 —
 * 접힌 띠 아래 있는 에이전트를 청하면 그 띠가 열린다는 것까지. */
function pickAgentGraphBucket(view, bucket) {
  const model = agentGraphModels.get(view);
  const target = model?.agents.find((entry) => entry.bucket === bucket);
  if (!target) return;
  // A band folded over the agent a person just asked for opens to show it.
  if (!model.entities.has(target.key)) {
    agentGraphFolded.delete(target.project.key);
    agentGraphSelectedKey = target.key;
    void paintBoardView(boardTab(), { force: true });
    return;
  }
  selectAgentGraphEntity(view, target.key, { focus: true });
}

/* A person's pane-menu command. Eligibility and transfer are ledger facts;
 * the renderer only names a run, a terminal and the generation it observed. */
let coordinatorPanel = null;

function closeCoordinatorPanel() {
  coordinatorPanel = null;
  hideModal(el("coordinator-scrim"));
}

function paintCoordinatorPanel() {
  const panel = coordinatorPanel;
  if (!panel) return;
  const current = panel.status;
  const busy = panel.busy || !current;
  el("coordinator-run").disabled = panel.busy;
  el("coordinator-refresh").disabled = panel.busy;
  el("coordinator-claim").disabled = busy;
  el("coordinator-target").disabled = busy;
  const selected = el("coordinator-target").value;
  const eligible = selected === "" || current?.eligiblePanes?.some((seat) => String(seat.term) === selected);
  el("coordinator-save").disabled = busy || !eligible || (!current?.quotaSupported && selected !== "");
  el("coordinator-state").textContent = panel.busy
    ? t("coordinator.loading", "원장을 확인하고 있습니다…")
    : !current ? t("coordinator.noRuns", "인수할 작업 런이 없습니다.")
      : current.coordinator?.held ? t("coordinator.held", "현재 코디네이터가 있습니다. 이 판으로 인수할 수 있습니다.")
        : t("coordinator.vacant", "코디네이터 자리가 비어 있습니다.");
  const policy = current?.policy;
  const targetGone = policy?.status === "armed" &&
    !current.eligiblePanes?.some((seat) => seat.term === policy.targetTerm);
  el("coordinator-policy").textContent = !current ? ""
    : targetGone ? t("coordinator.targetGone", "예약한 판을 사용할 수 없습니다. 인계 대상을 다시 선택해 주세요.")
    : !current.quotaSupported ? t("coordinator.noQuota", "현재 에이전트의 사용량을 확인할 수 없어 자동 인계를 켤 수 없습니다.")
      : policy?.status === "armed" ? t("coordinator.armed", "자동 인계가 예약돼 있습니다.")
        : policy?.status === "completed" ? t("coordinator.completed", "예약한 인계가 완료됐습니다.")
          : policy?.status === "interrupted" ? t("coordinator.interrupted", "이전 예약이 해제됐습니다. 대상을 다시 지정할 수 있습니다.")
            : t("coordinator.off", "자동 인계를 사용하지 않습니다.");
}

async function loadCoordinatorStatus(panel) {
  const runId = el("coordinator-run").value;
  const revision = ++panel.revision;
  panel.busy = true;
  panel.status = null;
  paintCoordinatorPanel();
  try {
    const status = runId ? await invoke("coordinator_handover_status", { runId }) : null;
    if (coordinatorPanel !== panel || panel.revision !== revision) return;
    panel.status = status;
    const picker = el("coordinator-target");
    const none = document.createElement("option");
    none.value = "";
    none.textContent = t("coordinator.off", "자동 인계를 사용하지 않습니다.");
    picker.replaceChildren(none, ...(status?.eligiblePanes ?? []).map((seat) => {
      const option = document.createElement("option");
      option.value = String(seat.term);
      const owner = tabOfTerm(seat.term);
      const title = owner ? paneTitleOf(owner, seat.term) || tabLabel(owner) : "";
      option.textContent = t("coordinator.pane", "{{agent}} · 터미널 {{term}}", {
        agent: title || agentName(seat.agent), term: seat.term,
      });
      return option;
    }));
    picker.value = status?.policy?.status === "armed" ? String(status.policy.targetTerm) : "";
    if (picker.selectedIndex < 0) {
      const missing = document.createElement("option");
      missing.value = String(status.policy.targetTerm);
      missing.textContent = t("coordinator.targetUnavailable", "예약된 판을 사용할 수 없음");
      missing.disabled = true;
      picker.appendChild(missing);
      picker.value = missing.value;
    }
  } catch (error) {
    if (coordinatorPanel === panel && panel.revision === revision) el("coordinator-result").textContent = String(error);
  } finally {
    if (coordinatorPanel === panel && panel.revision === revision) {
      panel.busy = false;
      paintCoordinatorPanel();
    }
  }
}

async function openCoordinatorPanel(term) {
  const panel = { term, busy: true, status: null, revision: 0, retry: null };
  coordinatorPanel = panel;
  el("coordinator-context").textContent = t("coordinator.context", "선택한 터미널 {{term}}", { term });
  el("coordinator-result").textContent = "";
  el("coordinator-run").replaceChildren();
  el("coordinator-target").replaceChildren();
  paintCoordinatorPanel();
  showModal(el("coordinator-scrim"));
  try {
    const answer = await invoke("coordinator_seat_runs", {});
    if (coordinatorPanel !== panel) return;
    el("coordinator-run").replaceChildren(...(answer.runs ?? []).map((run) => {
      const option = document.createElement("option");
      option.value = run.runId;
      option.textContent = run.name || run.runId;
      return option;
    }));
    await loadCoordinatorStatus(panel);
  } catch (error) {
    if (coordinatorPanel === panel) {
      panel.busy = false;
      el("coordinator-result").textContent = String(error);
      paintCoordinatorPanel();
    }
  }
}

async function changeCoordinator(action) {
  const panel = coordinatorPanel;
  if (!panel || panel.busy || !panel.status) return;
  const runId = el("coordinator-run").value;
  const expectedGeneration = panel.status.coordinator?.generation ?? 0;
  const targetTerm = action === "claim" ? panel.term
    : el("coordinator-target").value === "" ? null : Number(el("coordinator-target").value);
  if (action !== "claim" && targetTerm !== null && !panel.status.quotaSupported) return;
  const signature = JSON.stringify([action, runId, expectedGeneration, targetTerm]);
  if (panel.retry?.signature !== signature) panel.retry = { signature, id: `ui-coordinator-${crypto.randomUUID()}` };
  const retryRequest = panel.retry.id;
  panel.busy = true;
  el("coordinator-result").textContent = "";
  paintCoordinatorPanel();
  try {
    if (action === "claim") await invoke("claim_coordinator_seat", { runId, targetTerm, expectedGeneration, retryRequest });
    else await invoke("set_coordinator_handover", { runId, targetTerm, expectedGeneration, retryRequest });
    if (coordinatorPanel !== panel) return;
    panel.retry = null;
    el("coordinator-result").textContent = action === "claim"
      ? t("coordinator.claimed", "이 판이 코디네이터를 맡았습니다.") : t("coordinator.saved", "인계 설정을 저장했습니다.");
    await loadCoordinatorStatus(panel);
  } catch (error) {
    // Keep the retry key after an uncertain reply: repeating the same intent
    // must replay its receipt, not transfer the seat a second time.
    if (coordinatorPanel === panel) el("coordinator-result").textContent = String(error);
  } finally {
    if (coordinatorPanel === panel) { panel.busy = false; paintCoordinatorPanel(); }
  }
}

el("coordinator-close").addEventListener("click", closeCoordinatorPanel);
el("coordinator-run").addEventListener("change", () => {
  if (coordinatorPanel) void loadCoordinatorStatus(coordinatorPanel);
});
el("coordinator-target").addEventListener("change", paintCoordinatorPanel);
el("coordinator-refresh").addEventListener("click", () => {
  if (coordinatorPanel) void loadCoordinatorStatus(coordinatorPanel);
});
el("coordinator-claim").addEventListener("click", () => void changeCoordinator("claim"));
el("coordinator-save").addEventListener("click", () => void changeCoordinator("policy"));

/* ---- work at a glance ---------------------------------------------------
 * A task is a presentation of known work, never another orchestration ledger.
 * Explicit dispatches share a group only within the same run and project;
 * helpers follow their actual parent. A workspace or a similar title alone
 * cannot prove that two independent conversations are the same task. */
function taskBoardTitle(card) {
  const task = agentGraphCleanText(card.task);
  if (task) return task;
  const heading = agentGraphCleanText(card.heading);
  const numbered = heading.match(/^(.*?)\s*\d+$/);
  const terminalLabels = [t("terminal.label", "터미널"),
    ...Object.values(CATALOG).map((catalog) => catalog["terminal.label"])];
  const generic = !heading || (numbered && terminalLabels.some((label) =>
    label && label.toLocaleLowerCase() === numbered[1].trim().toLocaleLowerCase())) ||
    /^(?:w-\d+|term:\d+)$/i.test(heading) ||
    [card.agent, agentName(card.agent), card.worktree, basename(card.project || "")]
      .some((word) => word && word.toLocaleLowerCase() === heading.toLocaleLowerCase());
  return (generic ? agentGraphCleanText(card.you).split("\n")[0] : heading) ||
    heading || card.pane;
}

function taskBoardSections() {
  return [
    { id: "attention", word: t("board.tasks.attention", "내 확인 필요") },
    { id: "working", word: t("board.tasks.working", "진행 중") },
    { id: "paused", word: t("board.autonomy.paused", "일시 정지") },
    { id: "done", word: t("board.tasks.results", "응답·종료") },
    { id: "idle", word: t("board.tasks.history", "대기·이전 활동") },
  ];
}

function taskBoardTaskIdentity(entry) {
  const task = agentGraphCleanText(entry.place?.taskId || entry.card.task_id);
  const run = agentGraphCleanText(entry.place?.run || entry.card.run);
  // Checkout/project metadata can arrive after the worker is already visible.
  return task && run ? JSON.stringify([run, task]) : null;
}

function taskBoardModel(model, previous = null, now = Date.now(), workspacePath = null) {
  const entries = new Map(model.agents.map((entry) => [entry.card.pane, entry]));
  const rootOf = (entry) => {
    const trail = new Map();
    let root = entry;
    while (root) {
      // Malformed cyclic lineage has one deterministic representative.
      if (trail.has(root.card.pane)) {
        const path = [...trail.values()];
        return path.slice(path.findIndex((one) => one.card.pane === root.card.pane))
          .sort((a, b) => a.key.localeCompare(b.key))[0];
      }
      trail.set(root.card.pane, root);
      const parent = entries.get(root.card.parent);
      if (!parent || parent.project.key !== root.project.key) return root;
      const identity = taskBoardTaskIdentity(root);
      const parentIdentity = taskBoardTaskIdentity(parent);
      if (identity && identity !== parentIdentity) return root;
      const task = agentGraphCleanText(root.card.task);
      const parentTask = agentGraphCleanText(parent.card.task);
      if (!identity && !parentIdentity && task && task !== parentTask) return root;
      root = parent;
    }
    return entry;
  };
  const groups = new Map();
  for (const entry of model.agents) {
    const root = rootOf(entry);
    // A title can be identical on two independent tasks in the same run.
    // Only the ledger id, or actual ancestry for an unassigned pane, groups it.
    const key = taskBoardTaskIdentity(root) || root.key;
    let group = groups.get(key);
    if (!group) {
      group = { key, root, title: taskBoardTitle(root.card), members: [], project: root.project.label };
      groups.set(key, group);
    }
    group.members.push({ ...entry, type: "agent", label: taskBoardTitle(entry.card),
      attemptId: root.place.dispatchId ?? "",
      attemptStarted: root.place.dispatchStarted ?? 0,
      attemptOpen: root.place.reported === false,
      retryOf: root.place.retryOf ?? null,
      facts: agentCardFacts(entry.card, entry.bucket, now) });
  }
  const order = new Map((previous?.groups ?? []).map((group, index) => [group.key, index]));
  // Scope after grouping, retaining helpers in other checkouts in the same task.
  const ranked = [...groups.values()]
    .filter((group) => !workspacePath || group.members.some((entry) => entry.workspace.path === workspacePath))
    .sort((a, b) =>
    (order.get(a.key) ?? Infinity) - (order.get(b.key) ?? Infinity) || a.key.localeCompare(b.key));
  for (const group of ranked) {
    group.members.sort((a, b) => Number(b.key === group.root.key) - Number(a.key === group.root.key)
      || a.key.localeCompare(b.key));
    const attempts = group.members.filter((entry) => entry.attemptId);
    const open = attempts.filter((entry) => entry.attemptOpen);
    const replaced = new Set(attempts.map((entry) => entry.retryOf).filter(Boolean));
    const tips = attempts.filter((entry) => !replaced.has(entry.attemptId));
    const newest = Math.max(0, ...tips.map((entry) => entry.attemptStarted));
    const current = open.length > 0 ? open : tips.filter((entry) => entry.attemptStarted === newest);
    const currentIds = new Set(current.map((entry) => entry.attemptId));
    group.currentMembers = currentIds.size > 0
      ? group.members.filter((entry) => currentIds.has(entry.attemptId)) : group.members;
    const currentKeys = new Set(group.currentMembers.map((entry) => entry.key));
    group.previousMembers = group.members.filter((entry) => !currentKeys.has(entry.key));
    const primary = group.currentMembers;
    const needs = (entry) => entry.state === "needs-attention" || entry.state === "failed" ||
      (entry.state === "working" && entry.card.ledger === "orphaned");
    group.bucket = primary.some(needs) ? "attention"
      : primary.some((entry) => entry.bucket === "working") ? "working"
        : primary.some((entry) => entry.state === "paused") ? "paused"
          : primary.some((entry) => entry.bucket === "done") ? "done" : "idle";
    group.lead = primary.find(needs) ||
      primary.find((entry) => entry.state === group.bucket || entry.bucket === group.bucket) || primary[0];
    group.matches = !model.searchActive || group.members.some((entry) => entry.searchMatch);
    group.activity = taskBoardActivity(group.lead);
    const latest = [...primary].sort((a, b) => (b.card.at || 0) - (a.card.at || 0));
    const message = latest.find((entry) => agentGraphCleanText(entry.card.said));
    group.message = message ? agentGraphCleanText(message.card.said) : "";
    group.at = Math.max(0, ...group.members.map((entry) => Number(entry.card.at) || 0));
    group.when = group.at > 0 ? agoWord(group.at, now) : "";
  }
  return { groups: ranked, counts: new Map(taskBoardSections().map(({ id }) =>
    [id, ranked.filter((group) => group.bucket === id).length])) };
}

function taskBoardActivity(entry) {
  const { card, facts } = entry;
  if (facts.phase === "goal" || facts.phase === "loop") return facts.phaseWord;
  if (facts.state === "needs-attention") {
    return agentGraphCleanText(card.ask_prompt?.questions?.[0]?.question || card.ask ||
      card.approval?.summary) || t("board.tasks.needsInput", "진행하려면 확인이 필요해요.");
  }
  if (facts.state === "failed") return t("board.tasks.failed", "실행에 실패했어요. 마지막 기록을 확인해 주세요.");
  if (facts.state === "working" && card.ledger === "orphaned") {
    return t("board.tasks.orphaned", "에이전트는 작업 중이고, 조율하던 리더가 종료됐어요.");
  }
  if (facts.state === "done") {
    return t("board.tasks.ended", "이번 실행이 끝났어요. 응답과 변경 내용을 확인해 주세요.");
  }
  if (facts.state !== "working") return t("board.tasks.waiting", "다음 요청을 기다리고 있어요.");
  if (facts.phase === "quiet") return t("board.tasks.quiet", "최근 활동 이후 새 소식이 없어요.");
  if (facts.phase === "tooling" && facts.activity) {
    const target = facts.activity.target;
    if (facts.activity.kind === "read") return target
      ? t("board.tasks.readingTarget", "{{target}} 확인 중", { target })
      : t("board.tasks.reading", "자료와 코드를 확인하고 있어요.");
    if (facts.activity.kind === "edit") return target
      ? t("board.tasks.editingTarget", "{{target}} 수정 중", { target })
      : t("board.tasks.editing", "파일을 수정하고 있어요.");
    if (facts.activity.kind === "run") return target
      ? t("board.tasks.runningTarget", "{{target}} 실행 중", { target })
      : t("board.tasks.running", "명령을 실행하고 있어요.");
    return [facts.activity.word, target].filter(Boolean).join(" · ");
  }
  return facts.phase === "answering"
    ? t("board.tasks.answering", "도구 실행 후 응답을 작성하고 있어요.")
    : t("board.tasks.modelWaiting", "모델의 응답을 기다리고 있어요.");
}

function taskBoardElement(tag, className, text = "") {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}

function selectTaskBoardMember(view, key) {
  agentGraphSelectedKey = key;
  setAgentGraphInspectorOpen(view, true);
  const model = agentGraphModels.get(view);
  if (model) paintTaskBoard(view, model);
  void syncPreviewedTerms();
}

function taskBoardRow(group, view) {
  const row = taskBoardElement("article", "task-board-row");
  row.dataset.taskKey = group.key;
  const main = taskBoardElement("button", "task-board-main");
  main.type = "button";
  const cap = taskBoardElement("span", "task-board-cap");
  cap.append(taskBoardElement("span", "task-board-title"), taskBoardElement("span", "task-board-project"));
  const status = taskBoardElement("span", "task-board-state");
  status.append(agentGraphStateMark("working"), taskBoardElement("span", "task-board-state-word"));
  main.append(cap, taskBoardElement("span", "task-board-activity"), status);
  main.onclick = () => selectTaskBoardMember(view, row.__group.lead.key);
  const members = taskBoardElement("div", "task-board-members");
  const message = taskBoardElement("p", "task-board-message");
  message.append(taskBoardElement("span", "task-board-message-label"), taskBoardElement("span", "task-board-message-text"));
  const foot = taskBoardElement("div", "task-board-foot");
  foot.append(taskBoardElement("span", "task-board-when"));
  const action = taskBoardElement("button", "task-board-action");
  action.type = "button";
  action.onclick = () => selectTaskBoardMember(view, row.__group.lead.key);
  foot.append(action);
  row.append(main, members, message, foot);
  return row;
}

function updateTaskBoardRow(row, group, view) {
  row.__group = group;
  const selected = group.members.some((entry) => entry.key === agentGraphSelectedKey);
  writeClassName(row, `task-board-row is-${group.bucket}${selected ? " is-selected" : ""}`);
  writeTextContent(row.querySelector(".task-board-title"), group.title);
  writeTextContent(row.querySelector(".task-board-project"), group.project);
  writeTextContent(row.querySelector(".task-board-activity"), group.activity);
  writeAttribute(row.querySelector(".task-board-main"), "aria-pressed", String(selected));
  const state = group.bucket === "attention" ? "needs-attention" : group.bucket;
  dressAgentGraphStateMark(row.querySelector(".task-board-state > :first-child"), state);
  const stateWord = taskBoardSections().find(({ id }) => id === group.bucket).word;
  writeTextContent(row.querySelector(".task-board-state-word"), stateWord);
  const host = row.querySelector(".task-board-members");
  const held = new Map([...host.children].map((node) => [node.dataset.memberKey, node]));
  const members = group.members.map((entry) => {
    let member = held.get(entry.key);
    if (!member) {
      member = taskBoardElement("button", "task-board-member");
      member.type = "button";
      member.dataset.memberKey = entry.key;
      member.append(taskBoardElement("span", "task-board-member-name"),
        taskBoardElement("span", "task-board-member-work"), taskBoardElement("span", "task-board-member-status"));
      member.onclick = () => selectTaskBoardMember(view, entry.key);
    }
    writeTextContent(member.firstElementChild, agentName(entry.card.agent));
    const role = entry.key === group.root.key ? taskBoardActivity(entry) : entry.label;
    writeTextContent(member.querySelector(".task-board-member-work"), role);
    writeTextContent(member.lastElementChild, entry.facts.state === "done"
      ? t("board.tasks.results", "응답·종료") : entry.facts.stateWord);
    writeAttribute(member, "data-tip", [entry.facts.model, entry.label, taskBoardActivity(entry)].join(" · "));
    writeAttribute(member, "aria-pressed", String(entry.key === agentGraphSelectedKey));
    return member;
  });
  reconcileElementOrder(host, members);
  // One agent already speaks in the headline. A team exposes the distribution.
  host.hidden = group.members.length < 2;
  const message = row.querySelector(".task-board-message");
  message.hidden = !group.message || group.message === group.activity;
  writeTextContent(message.firstElementChild, t("board.tasks.message", "최근 메시지"));
  writeTextContent(message.lastElementChild, group.message);
  writeTextContent(row.querySelector(".task-board-when"), [
    group.members.length === 1 ? group.lead.facts.model : agentGraphCountWord("agent", group.members.length),
    group.when ? t("board.tasks.lastActivity", "마지막 활동 {{time}}", { time: group.when }) : "",
  ].filter(Boolean).join(" · "));
  writeTextContent(row.querySelector(".task-board-action"), group.bucket === "attention"
    ? t("board.tasks.check", "확인하기") : t("board.tasks.details", "작업 자세히 보기"));
}

/* 이 참여자가 회상한 지식(t-2931의 회상 추적): 훅이 그 판의 프롬프트 앞에 끼운
 * 페이지들이다. 제목 검색(「지식에서 검색」)이 아니라 적힌 사실이라 다른 줄이고,
 * 줄마다 그래프의 그 점으로 간다. 좌석은 세션(창이 알면)·아니면 판이다 — 재시작은
 * 판 번호를 바꾸지 대화를 바꾸지 않는다. 답은 짧게 든다: 같은 참여자를 다시
 * 고르는 일은 흔하고 인스펙터는 활동마다 다시 지어진다. */
const TASK_BOARD_RECALLS_TTL_MS = 20_000;
const taskBoardRecallsHeld = new Map();
let taskBoardRecallsRevision = 0;

function invalidateTaskBoardRecalls() {
  taskBoardRecallsHeld.clear();
  taskBoardRecallsRevision++;
  scheduleAgentPaint(["board"]);
}
function taskBoardSeatOf(card) {
  const term = agentCardTerm(card);
  if (term === null) return null;
  const session = paneSessions.get(term)?.session?.id ?? null;
  return { session, pane: `term-${term}`, key: JSON.stringify([secondBrainVault, session, term]) };
}
function taskBoardRecallsNode(entry) {
  const block = taskBoardElement("section", "task-board-detail-block task-board-recalls");
  block.hidden = true;
  const seat = taskBoardSeatOf(entry.card);
  if (!seat || isPopout || secondBrainVault === "") return block;
  block.append(taskBoardElement("h3", "", t("board.tasks.recalled", "참고한 지식")));
  const list = taskBoardElement("ul", "task-board-recalls-list");
  block.append(list);
  const paint = (rows) => {
    block.hidden = rows.length === 0;
    list.replaceChildren(...rows.map((row) => {
      const item = document.createElement("li");
      const button = taskBoardElement("button", "task-board-member task-board-recall", row.title);
      button.type = "button";
      button.dataset.knowledgePage = row.page;
      button.dataset.tip = row.page;
      button.onclick = () => revealKnowledgePage(row.page);
      item.append(button, taskBoardElement("span", "task-board-recall-when", knowledgeClock(row.at_ms, Date.now())));
      return item;
    }));
  };
  const held = taskBoardRecallsHeld.get(seat.key);
  if (held && Date.now() - held.at < TASK_BOARD_RECALLS_TTL_MS) {
    paint(held.rows);
    return block;
  }
  const vault = secondBrainVault;
  const revision = taskBoardRecallsRevision;
  invoke("second_brain_seat_recalls", { session: seat.session, pane: seat.pane })
    .then((rows) => {
      if (vault !== secondBrainVault || revision !== taskBoardRecallsRevision) return;
      const answer = Array.isArray(rows) ? rows : [];
      taskBoardRecallsHeld.set(seat.key, { at: Date.now(), rows: answer });
      if (block.isConnected) paint(answer);
    })
    .catch(() => {});
  return block;
}

function paintTaskBoardInspector(view, tasks) {
  const group = tasks.groups.find((one) => one.members.some((entry) => entry.key === agentGraphSelectedKey));
  const entry = group?.members.find((one) => one.key === agentGraphSelectedKey);
  const body = view.querySelector(".agent-inspector-body");
  agentGraphInspectorViews.delete(body);
  const tabs = view.querySelector(".agent-inspector-tabs");
  if (tabs) tabs.hidden = true;
  paintAgentInspectorGlyph(view, null, null);
  const actions = view.querySelector(".agent-inspector-actions");
  writeTextContent(view.querySelector(".agent-inspector-kind"), t("board.tasks.details", "작업 자세히 보기"));
  writeTextContent(view.querySelector(".agent-inspector-title"), group?.title || t("board.tasks.select", "작업을 선택하세요"));
  writeTextContent(view.querySelector(".agent-inspector-meta"), entry
    ? [group.project, entry.workspace.label, entry.facts.model].filter(Boolean).join(" · ")
    : t("board.tasks.selectCopy", "현재 활동과 응답, 확인이 필요한 내용을 여기서 볼 수 있어요."));
  actions.hidden = !entry;
  if (!entry) {
    body.replaceChildren();
    delete body.dataset.taskSignature;
    return;
  }
  const { card } = entry;
  const draft = entry.bucket === "attention" && (card.ask_prompt || card.approval)
    ? askDraftFor(card) : null;
  // Clock/ack updates do not change this panel's content. In particular, a
  // minute passing must not remount a question while a person is typing.
  const { at, changed_at, unseen, ...contentCard } = card;
  const signature = JSON.stringify([locale, group.title, entry.key, entry.bucket,
    taskBoardActivity(entry), entry.facts.model, entry.workspace.path, entry.workspace.label, contentCard,
    group.members.map((one) => [one.key, one.label]),
    draft ? [draft.index, draft.sending, draft.selections] : null, agentGraphTickerSignature(card.pane),
    agentGraphExpandedTimelines.has(card.pane)]);
  if (body.dataset.taskSignature !== signature) {
    body.dataset.taskSignature = signature;
    delete body.dataset.graphSignature;
    const content = [workbenchRelatedActions(entry)];
    const section = (label, text) => {
      const block = taskBoardElement("section", "task-board-detail-block");
      block.append(taskBoardElement("h3", "", label), taskBoardElement("p", "", text));
      return block;
    };
    const goal = agentGraphCleanText(card.you);
    if (goal && goal !== group.title) content.push(section(t("board.tasks.request", "작업 요청"), goal));
    if (group.members.length > 1) {
      const members = taskBoardElement("div", "task-board-detail-members");
      for (const one of group.members) {
        const button = taskBoardElement("button", "task-board-member", `${agentName(one.card.agent)} · ${one.label}`);
        button.type = "button";
        button.setAttribute("aria-pressed", String(one.key === entry.key));
        button.onclick = () => selectTaskBoardMember(view, one.key);
        members.append(button);
      }
      content.push(members);
    }
    if (!draft) content.push(section(t("board.tasks.current", "지금 하는 일"), taskBoardActivity(entry)));
    if (draft) content.push(card.ask_prompt ? askPanelNode(card, draft) : approvalPanelNode(card, draft));
    if (card.said) content.push(section(t("board.tasks.message", "최근 메시지"), agentGraphCleanText(card.said)));
    content.push(taskBoardRecallsNode(entry));
    const stream = agentGraphStreamNode(entry, view);
    stream.querySelector(".agent-inspector-context-heading").textContent = t("board.tasks.activityLog", "최근 활동 기록");
    content.push(stream);
    body.replaceChildren(...content);
    body.dataset.taskRecalls = JSON.stringify([taskBoardSeatOf(card), taskBoardRecallsRevision]);
  }
  const recallsSignature = JSON.stringify([taskBoardSeatOf(card), taskBoardRecallsRevision]);
  if (body.dataset.taskRecalls !== recallsSignature) {
    body.querySelector(".task-board-recalls")?.replaceWith(taskBoardRecallsNode(entry));
    body.dataset.taskRecalls = recallsSignature;
  }
  paintAgentInspectorDestinations(view, entry);
}

function paintTaskBoard(view, model) {
  const previous = taskBoardModels.get(view);
  if (!previous) {
    // docHost may clone populated markup. Rows and inspector actions need
    // their own listeners; paint signatures cannot stand in for ownership.
    view.querySelector(".task-board-sections").replaceChildren();
    delete view.querySelector(".agent-inspector-body").dataset.taskSignature;
  }
  const tasks = taskBoardModel(model, previous, Date.now(), workbenchScopes.tasks?.path);
  taskBoardModels.set(view, tasks);
  const surface = view.querySelector(".task-board-surface");
  const sections = taskBoardSections();
  writeTextContent(surface.querySelector(".task-board-summary"), t("board.tasks.summary",
    "작업 {{total}}개 · 진행 중 {{working}}개 · 내 확인 필요 {{attention}}개", {
      total: tasks.groups.length, working: tasks.counts.get("working"), attention: tasks.counts.get("attention"),
    }));
  const filters = surface.querySelector(".task-board-filters");
  const choices = [{ id: "all", word: t("board.tasks.all", "전체") }, ...sections];
  const filterNodes = new Map([...filters.children].map((node) => [node.dataset.taskFilter, node]));
  reconcileElementOrder(filters, choices.map(({ id, word }) => {
    const button = filterNodes.get(id) || taskBoardElement("button", "task-board-filter");
    button.type = "button";
    button.dataset.taskFilter = id;
    button.onclick = () => {
      taskBoardFilter = id;
      paintTaskBoard(view, agentGraphModels.get(view));
    };
    writeTextContent(button, `${word} ${id === "all" ? tasks.groups.length : tasks.counts.get(id)}`);
    writeAttribute(button, "aria-pressed", String(taskBoardFilter === id));
    return button;
  }));
  const host = surface.querySelector(".task-board-sections");
  const heldSections = new Map([...host.children].map((node) => [node.dataset.taskSection, node]));
  // Reuse a row even when a state change moves it into another section.
  const rows = new Map([...host.querySelectorAll(".task-board-row")].map((node) => [node.dataset.taskKey, node]));
  let shown = 0;
  const wanted = [];
  for (const { id, word } of sections) {
    const groups = tasks.groups.filter((group) => group.bucket === id && group.matches &&
      (taskBoardFilter === "all" || taskBoardFilter === id));
    if (!groups.length) continue;
    shown += groups.length;
    const history = id === "idle";
    let section = heldSections.get(id);
    if (!section) {
      section = taskBoardElement(history ? "details" : "section", "task-board-section");
      section.dataset.taskSection = id;
      section.append(taskBoardElement(history ? "summary" : "h3", "task-board-section-head"),
        taskBoardElement("div", "task-board-list"));
      if (history) section.ontoggle = () => {
        if (section.open === section.__paintedOpen) return;
        if (section.open) taskBoardHistoryOpen.add(view);
        else taskBoardHistoryOpen.delete(view);
      };
    }
    if (history) {
      section.__paintedOpen = taskBoardHistoryOpen.has(view) || model.searchActive || taskBoardFilter === "idle";
      section.open = section.__paintedOpen;
    }
    writeTextContent(section.firstElementChild, `${word} · ${groups.length}`);
    const rowNodes = groups.map((group) => {
      const row = rows.get(group.key) || taskBoardRow(group, view);
      updateTaskBoardRow(row, group, view);
      return row;
    });
    reconcileElementOrder(section.lastElementChild, rowNodes);
    wanted.push(section);
  }
  reconcileElementOrder(host, wanted);
  const empty = surface.querySelector(".task-board-empty");
  empty.hidden = shown > 0;
  writeTextContent(empty, tasks.groups.length === 0 ? t("board.tasks.empty", "에이전트에게 일을 맡기면 여기에 표시돼요.")
    : t("board.tasks.noMatch", "이 조건에 맞는 작업이 없어요."));
  const results = view.querySelector(".board-results");
  results.hidden = !model.searchActive;
  if (!results.hidden) writeTextContent(results, t("board.results", "총 {{total}}개 중 {{shown}}개 표시", {
    total: tasks.groups.length, shown,
  }));
  paintTaskBoardInspector(view, tasks);
  // The inspector's existing Escape/scrim controls apply to either view.
  wireAgentGraphCanvas(view);
}

/* ---- the paint ---------------------------------------------------------- */

function paintAgentGraph(view, model) {
  const full = agentGraphFullModel(view, model);
  const former = agentGraphFullModel(view);
  if (former && former !== full) {
    const oldEntries = new Map(former.agents.map((entry) => [entry.key, entry]));
    const byWorker = new Map(full.agents.filter((entry) => entry.place.workerId)
      .map((entry) => [entry.place.workerId, entry.key]));
    const migrate = (key) => byWorker.get(oldEntries.get(key)?.place.workerId) ?? key;
    agentGraphSelectedKey = migrate(agentGraphSelectedKey);
    if (agentGraphSelectedEdgeKey) {
      const oldRelation = agentGraphRelations(former).find((edge) => edge.key === agentGraphSelectedEdgeKey);
      if (oldRelation) {
        agentGraphSelectedEdgeKey = agentGraphRelations(full).find((edge) => edge.type === oldRelation.type
          && edge.from === migrate(oldRelation.from) && edge.to === migrate(oldRelation.to))?.key ?? null;
      }
    }
  }
  const taskMode = agentBoardMode === "tasks";
  model = taskMode ? full : agentGraphScopedModel(view, full);
  view.classList.toggle("is-task-board", taskMode);
  view.classList.toggle("is-scoped-relations", !taskMode && agentGraphScopeKey !== "");
  view.classList.toggle("is-detailed-relations", agentGraphCardDetails);
  view.querySelector(".task-board-surface").hidden = !taskMode;
  view.querySelector(".agent-graph-surface").hidden = taskMode;
  const heading = view.querySelector(".board-title");
  writeAttribute(heading, "data-i18n", taskMode ? "board.tasks.heading" : "board.graph.heading");
  writeTextContent(heading, taskMode
    ? t("board.tasks.heading", "작업 상황판") : t("board.graph.heading", "에이전트 그래프"));
  const query = view.querySelector(".board-query");
  writeAttribute(query, "data-i18n-placeholder", taskMode ? "board.tasks.search" : "board.search");
  writeAttribute(query, "placeholder", taskMode
    ? t("board.tasks.search", "작업·프로젝트·에이전트 검색…")
    : t("board.search", "워크트리·프로젝트·에이전트 검색…"));
  if (taskMode) {
    agentGraphModels.set(view, model);
    paintTaskBoard(view, model);
    return;
  }
  paintAgentRelationsControls(view, full, model);
  delete view.querySelector(".agent-inspector-body").dataset.taskSignature;
  const { selectDefault = true } = arguments[2] ?? {};
  const previous = agentGraphModels.get(view);
  const topologyMoved = previous?.topologySignature !== model.topologySignature;
  const overlayMoved = previous?.overlayMode !== model.overlayMode ||
    previous?.overlaySignature !== model.overlaySignature;
  const hot = agentGraphHotKey || model.overlayData?.latest || "";
  const followMoved = agentGraphFollowing && hot && hot !== agentGraphSelectedKey &&
    model.nodes.some((entity) => entity.key === hot);
  if (followMoved) agentGraphSelectedKey = hot;
  agentGraphModels.set(view, model);
  /* Selection is shared between docked and pop-out views, but a newly opened
   * view must not inherit an inventory-only workspace from an earlier empty
   * graph when live work is now available. Once a view has a model, deliberate
   * project/workspace selection remains stable across ordinary repaints. */
  if (!model.entities.has(agentGraphSelectedKey) ||
      (previous === undefined && model.defaultKey?.startsWith("agent:") &&
       !agentGraphSelectedKey?.startsWith("agent:"))) {
    agentGraphSelectedKey = selectDefault ? model.defaultKey : null;
  }
  if (agentGraphSelectedEdgeKey && !agentGraphRelations(model).some((edge) => edge.key === agentGraphSelectedEdgeKey)) agentGraphSelectedEdgeKey = null;
  const selectionMoved = view.dataset.graphSelected !== (agentGraphSelectedKey ?? "")
    || view.dataset.graphSelectedEdge !== (agentGraphSelectedEdgeKey ?? "");
  view.dataset.graphSelected = agentGraphSelectedKey ?? "";
  view.dataset.graphSelectedEdge = agentGraphSelectedEdgeKey ?? "";
  const nodesHost = view.querySelector(".agent-graph-nodes");
  const agentLayers = Math.max(1, model.maxDepth + 1);
  /* 층의 수를 격자에 적어 둔다 (round three). 에이전트 레인의 폭은 실측 폭과
   * 「판의 몫」 중 큰 쪽이고, 그 몫은 판의 폭을 층의 수로 나눈 것이다 — 나누는
   * 수를 아는 것은 이 함수뿐이라 여기서 한 번 적고 셈은 CSS가 한다
   * (`.agent-graph-nodes`의 `--agent-graph-lane-share`). */
  writeStyleProperty(nodesHost.parentElement, "--agent-graph-agent-lanes", String(agentLayers));
  writeStyleValue(nodesHost, "gridTemplateColumns", [
    ...AGENT_GRAPH_CONTEXT_LAYERS.map(() => "var(--agent-graph-context-lane)"),
    ...Array.from({ length: agentLayers }, () => "var(--agent-graph-agent-lane)"),
  ].join(" "));

  /* Four kinds of child share one keyed list — the lane strip, the band fills,
   * the band heads and the nodes — because they share one grid, and a second
   * reconcile over a second container is a second answer to where a row is. */
  const existing = new Map([...nodesHost.children].map((node) => [node.dataset.graphPart, node]));
  const wanted = [];
  const take = (part, make) => {
    const held = existing.get(part) ?? make();
    writeAttribute(held, "data-graph-part", part);
    wanted.push(held);
    return held;
  };

  const lanes = model.bands.length > 0 ? model.laneCount : 0;
  for (let lane = 0; lane < lanes; lane += 1) {
    const strip = take(`lane:${lane}`, () => document.createElement("span"));
    writeClassName(strip, "agent-graph-lane");
    writeStyleValue(strip, "gridColumn", String(lane + 1));
    writeStyleValue(strip, "gridRow", "1");
    writeTextContent(strip, agentGraphLaneWord(lane));
  }
  /* Band by band, and inside a band the fill, the heading and then everything
   * that heading holds. Grouping the headings together instead — every project
   * first, then every card — reads as one flat list to anything that walks the
   * DOM: a keyboard opening a project and pressing down would land on the NEXT
   * project rather than on the workspace it just opened.
   *
   * `dressMoved` rides along because anything rebuilt here may have changed
   * height, and a height change moves every row under it. Collected rather than
   * assumed: it is the difference between an edge that follows its nodes and
   * one that points at where they stood one hook event ago. */
  const selectedRelation = agentGraphRelations(model).find((edge) => edge.key === agentGraphSelectedEdgeKey);
  const previousAttempts = new Set(agentGraphTasksFor(view, full).groups.flatMap((group) => group.previousMembers.map((entry) => entry.key)));
  let dressMoved = false;
  for (const band of model.bands) {
    const fill = take(`band:${band.key}`, () => document.createElement("div"));
    writeClassName(fill, [
      "agent-graph-band",
      band.folded ? "is-folded" : "",
      band.searchMatch === false ? "is-search-dimmed" : "",
    ].filter(Boolean).join(" "));
    writeStyleValue(fill, "gridRow", `${band.row} / ${band.rowEnd + 1}`);
    writeAttribute(fill, "aria-hidden", "true");
    const head = take(`head:${band.key}`, () => document.createElement("div"));
    if (updateAgentGraphBandHead(head, band, view)) dressMoved = true;
    for (const workspace of (model.nodesByBand.get(band.key) ?? [])
      .filter((entity) => entity.type === "workspace")) {
      const lane = take(`swimlane:${workspace.key}`, () => document.createElement("div"));
      writeClassName(lane, ["agent-graph-swimlane", workspace.laneClass]
        .filter(Boolean).join(" "));
      writeStyleValue(lane, "gridRow", `${workspace.rowStart} / ${workspace.rowEnd + 1}`);
      writeAttribute(lane, "data-graph-workspace", workspace.key);
      writeAttribute(lane, "aria-hidden", "true");
    }
    for (const entity of model.nodesByBand.get(band.key) ?? []) {
      entity.relationEndpoint = entity.key === selectedRelation?.from || entity.key === selectedRelation?.to;
      entity.previousAttempt = previousAttempts.has(entity.key);
      const node = take(`node:${entity.key}`, () => agentGraphNode(entity, view));
      if (updateAgentGraphNode(node, entity, view)) dressMoved = true;
    }
  }
  reconcileElementOrder(nodesHost, wanted);
  /* 그림이 없으면 그림에 딸린 것도 서지 않는다. 없는 선의 범례와 없는 그림의
   * 배율은 "아직 아무도 시작하지 않았다"를 "무언가 고장 났다"로 읽히게 하는 두
   * 상자였다 — 빈 판에서 사람이 읽을 것은 한 문장이면 된다. */
  const drawnNothing = model.bands.length === 0;
  const empty = view.querySelector(".agent-graph-empty");
  empty.hidden = !drawnNothing;
  writeAttribute(empty, "data-i18n", agentGraphScopeKey ? "board.graph.scopeEmpty" : "board.graph.empty");
  writeTextContent(empty, agentGraphScopeKey
    ? t("board.graph.scopeEmpty", "이 범위에 표시할 실행이 없습니다. 전체 실행에서 다른 작업을 확인하세요.")
    : t("board.graph.empty", "표시할 활성 에이전트가 없습니다."));
  view.querySelector(".agent-graph-surface").classList.toggle("is-empty", drawnNothing);

  const run = view.querySelector(".agent-graph-run");
  if (run) {
    writeTextContent(run.querySelector(".agent-graph-run-count"), String(model.runCount));
  }

  for (const button of view.querySelectorAll("[data-graph-bucket]")) {
    const bucket = button.dataset.graphBucket;
    const count = model.counts.get(bucket) ?? 0;
    writeTextContent(button.querySelector("[data-graph-count]"), String(count));
    button.disabled = count === 0;
    if (!button.onclick) button.onclick = () => pickAgentGraphBucket(view, bucket);
  }
  for (const button of view.querySelectorAll("[data-graph-overlay]")) {
    const active = button.dataset.graphOverlay === agentGraphOverlayMode;
    writeAttribute(button, "aria-pressed", String(active));
    button.classList.toggle("is-active", active);
    if (!button.onclick) {
      button.onclick = () => agentGraphSetOverlay(view, button.dataset.graphOverlay);
    }
  }
  const follow = view.querySelector(".agent-graph-follow");
  if (follow) {
    writeAttribute(follow, "aria-pressed", String(agentGraphFollowing));
    follow.classList.toggle("is-active", agentGraphFollowing);
    if (!follow.onclick) {
      follow.onclick = () => {
        agentGraphFollowing = !agentGraphFollowing;
        paintAgentGraph(view, agentGraphModels.get(view));
      };
    }
  }
  paintAgentGraphInspector(view, model);
  watchAgentGraphSize(view);
  wireAgentGraphCanvas(view);
  wireAgentGraphKeys(view);
  applyAgentGraphZoom(view, { remeasure: false });
  if (topologyMoved) scheduleAgentGraphFit(view);
  if (topologyMoved || dressMoved) scheduleAgentGraphEdges(view);
  else if (selectionMoved || overlayMoved) paintAgentGraphEdges(view, { styleOnly: true });
  if (followMoved) requestAnimationFrame(() => softlyFollowAgentGraphSelection(view));
}

/* 팝아웃 창에는 탭 목록이 없다 — 창 전체가 보드 한 장이다. 그 한 장을
 * `paintBoardView`가 찾을 수 있도록 여기에 세워 둔다: 그리는 함수는 두 창이
 * 공유하는 바로 그 함수여야 하고, 그러려면 탭처럼 생긴 것 하나가 필요하다.
 * `pane: 0`은 첫 leaf, 곧 `docHost`가 원본 `#board-view`를 돌려주는 자리다. */
const POPOUT_BOARD_TAB = { id: "board", kind: "board", pane: 0 };

function boardTab() {
  return isPopout ? POPOUT_BOARD_TAB : tabs.find((held) => held.kind === "board");
}

/* 그리다 멈춘 보드 (1-g78d).
 *
 * Orca의 `recoverableError`. 상태가 하나이고 열마다 따로 없는 것은 그리는 일이
 * 한 번의 훑기이기 때문이다 — 카드도 열도 그 한 번이 만드는 것이라, 그 훑기가
 * 멈추면 멈춘 것은 열 하나가 아니라 보드 전체다.
 *
 * 서 있는 동안 다시 그리지 않는 이유는 이벤트가 계속 도착하기 때문이다: 훅 하나에
 * 한 번씩 같은 예외로 다시 넘어지는 표면은 고장을 초당 몇 번씩 재현하고, 그
 * 사이에 사람이 누를 수 있는 것은 아무것도 없다. 이 문을 여는 것은 다시 시도뿐이다. */
let boardBroken = false;

/* 그 카드 한 장. 세 그림 중 무엇이 서 있었든 그 자리에 대신 선다 — 부서진 것은
 * 한 그림이 아니라 그리는 일이므로, 남은 그림을 반쯤 세워 두면 화면은 어느 쪽이
 * 지금인지 말하지 않는다. */
function paintBoardBroken(view) {
  // 부서진 판은 지난 그림의 기억도 버린다 — 남겨 두면 고쳐진 첫 판이 지난
  // 서명과 같아서 건너뛰고, 숨겨 둔 표면이 영영 다시 서지 않는다.
  delete view.dataset.said;
  view.querySelector(".agent-graph-layout").hidden = true;
  /* 머리의 수도 함께 내려놓는다 (1-t353).
   *
   * 그 넷은 **그리지 못한 그림**의 수다. 그대로 두면 "작업 중 3" 바로 밑에
   * "대시보드가 그리다 멈췄습니다"가 서고, 둘 중 하나는 반드시 거짓말이다 —
   * 게다가 누를 수 있는 채로 남아, 이제 없는 모델에서 노드를 고르려 든다.
   * 다시 세워지는 첫 판이 넷 다 제 수로 되돌린다. */
  for (const button of view.querySelectorAll("[data-graph-bucket]")) {
    writeTextContent(button.querySelector("[data-graph-count]"), "0");
    button.disabled = true;
  }
  const run = view.querySelector(".agent-graph-run");
  if (run) writeTextContent(run.querySelector(".agent-graph-run-count"), "0");
  const broken = view.querySelector(".board-broken");
  if (!broken) return;
  broken.hidden = false;
  // 복제된 판은 리스너를 들고 오지 않는다 — 머리의 손들과 같은 이유로, 같은
  // 방법으로 판마다 다시 맨다.
  const retry = broken.querySelector(".board-retry");
  if (retry) retry.onclick = retryBoardPaint;
}

/* 「다시 시도」는 이어 그리지 않는다 — 그리고 그 리마운트는 이 손의 일이
 * 아니다: `paintBoardView`의 `swap`이 매 판 세 표면을 비우고 처음부터 세우므로
 * (한 화면에 두 시각의 카드가 서지 않는 그 이유로), 여기서 한 번 더 비우는
 * 것은 같은 약속의 두 번째 사본이다. 이 손은 문만 다시 연다. */
function retryBoardPaint() {
  boardBroken = false;
  void paintBoardView();
}

let agentGraphSnapshotOverlays = {};

function agentGraphSaid(columns, reviews, places, now) {
  const cards = columns.flatMap((column) => column.cards.map((card) => {
    const { at, changed_at, ...visible } = card;
    const clock = cardClock(card, column.bucket, now);
    return {
      bucket: column.bucket,
      ...visible,
      clock: at > 0 ? agoWord(at, now) : "",
      // 그리고 도는 카드가 추가로 그리는 낱말 (1-t1153). 「50분째」가
      // 「51분째」로 넘어가는
      // 박자에 `at`의 낱말은 그대로일 수 있고(도구 이벤트는 계속 오므로 그쪽은
      // 「방금」에 머문다), 그러면 서명이 같아서 그 카드만 옛 낱말에 굳는다.
      // 원료인 `changed_at`은 위에서 걷어 낸 그대로 둔다 — 밀리초는 매 박자
      // 달라서 서명이 두 번 같지 않다.
      running: clock?.running ?? "",
      activity: activityLine(card.pane),
      // The card draws eight tool calls, not one, so one line's worth of
      // signature would freeze the seven behind it at whatever they said first.
      wire: agentGraphTickerSignature(card.pane),
      // 그리고 컨텍스트 미터가 그리는 값. 카드에 실려 오지 않고 이 창의
      // 세션 지도에서 오므로, 서명이 그것을 세지 않으면 미터만 옛 비율에
      // 굳는다 — `usage` 프레임은 카드의 어느 필드도 움직이지 않는다.
      ctx: JSON.stringify(agentCardContext(card) ?? null),
      // 모델도 같은 이유로. `hook:agent`가 실어 온 모델은 카드의 어느 필드도
      // 움직이지 않으므로, 서명이 그것을 세지 않으면 칩만 옛 id에 굳는다.
      model: agentCardModel(card),
    };
  }));
  const selectedDraft = agentGraphSelectedKey?.startsWith("agent:")
    ? askDrafts.get(agentGraphSelectedKey.slice("agent:".length))
    : null;
  return JSON.stringify({
    locale,
    query: boardQuery,
    boardMode: agentBoardMode,
    recalls: [secondBrainVault, taskBoardRecallsRevision],
    workspaceScope: workbenchScopes.tasks?.path,
    cards,
    reviews: [...reviews],
    places: [...places],
    catalog: projects.map((project) => [
      project.path,
      project.name,
      ...(project.worktrees ?? []).map((workspace) =>
        [workspace.path, workspace.branch ?? "", workspace.base ?? ""].join("")),
    ]),
    folded: [...agentGraphFolded].sort(),
    dormantExpanded: [...agentGraphDormantExpanded].sort(),
    idleExpanded: [...agentGraphIdleExpanded].sort(),
    overlays: agentGraphSnapshotOverlays,
    overlayMode: agentGraphOverlayMode,
    following: agentGraphFollowing,
    draft: selectedDraft
      ? {
          open: selectedDraft.open,
          sending: selectedDraft.sending,
          index: selectedDraft.index,
          selections: selectedDraft.selections,
        }
      : null,
  });
}

async function paintAgentGraphView(
  tab,
  { force = false, snapshot = null, selectDefault = true } = {},
) {
  if (!tab) return null;
  const view = docHost(tab.pane, "board");
  if (boardBroken) {
    paintBoardBroken(view);
    return null;
  }
  try {
    wireBoardHead(view);
    const cards = snapshot?.cards ?? await boardCards();
    const reviews = new Map();
    const checkouts = new Set();
    const places = new Map();
    for (const card of cards) {
      if (card.checkout) checkouts.add(card.checkout);
      const review = boardReviews.get(card.checkout);
      if (review) reviews.set(card.pane, review);
      places.set(card.pane, {
        checkout: card.checkout,
        host: card.host,
        hostName: card.host_name ?? "",
        run: card.run ?? "",
        taskId: card.task_id ?? "",
        workerId: card.worker_id ?? "",
        dispatchId: card.dispatch_id ?? "",
        dispatchStarted: Number(card.dispatch_started_ms) || 0,
        reported: card.reported ?? null,
        retryOf: card.retry_of ?? null,
      });
    }
    const answer = snapshot?.answer ?? await invoke("board_snapshot", {
      cards,
      query: agentGraphSnapshotQuery(),
    });
    applyBoardBadge(answer);

    /* 정리할 초안이 있을 때만 (1-t353). 이 줄들은 아래 서명 비교 — 그림이
     * 그대로면 빠져나가는 문 — **앞**에 서 있으므로, 여는 판마다 모든 열의
     * 카드를 훑는다. 물어보는 카드에 사람이 손을 댄 적이 없으면 `askDrafts`는
     * 비어 있고, 빈 표에서 지울 것을 찾는 훑기는 매 훅마다 치르는 값이면서
     * 언제나 아무것도 지우지 않는다. */
    if (askDrafts.size > 0) {
      const stillAsking = new Set(
        answer.columns.flatMap((column) =>
          column.cards
            .filter((card) => card.ask_prompt || card.approval)
            .map((card) => card.pane)),
      );
      for (const pane of [...askDrafts.keys()]) {
        if (!stillAsking.has(pane)) askDrafts.delete(pane);
      }
    }

    if (agentGraphExpandedTimelines.size > 0) {
      const present = new Set(answer.columns.flatMap((column) => column.cards.map((card) => card.pane)));
      for (const pane of agentGraphExpandedTimelines) if (!present.has(pane)) agentGraphExpandedTimelines.delete(pane);
    }
    const now = Date.now();
    agentGraphSnapshotOverlays = answer.overlays ?? {};
    const said = agentGraphSaid(answer.columns, reviews, places, now);
    if (!force && view.dataset.said === said) return { cards, answer };
    view.dataset.said = said;
    view.querySelector(".agent-graph-layout").hidden = false;
    const broken = view.querySelector(".board-broken");
    if (broken) broken.hidden = true;
    const model = agentGraphModel(
      answer.columns,
      places,
      reviews,
      now,
      agentGraphFullModel(view),
      projects,
      boardQuery,
      answer.overlays ?? {},
    );
    paintAgentGraph(view, model, { selectDefault });

    const taskCounts = agentBoardMode === "tasks" ? taskBoardModels.get(view) : null;
    const shown = taskCounts ? taskCounts.groups.filter((group) => group.matches &&
      (taskBoardFilter === "all" || taskBoardFilter === group.bucket)).length : model.searchActive
      ? model.searchMatchCount
      : answer.columns.reduce((total, column) => total + column.cards.length, 0);
    const results = view.querySelector(".board-results");
    results.hidden = boardQuery.trim() === "";
    if (!results.hidden) {
      results.textContent = t("board.results", "총 {{total}}개 중 {{shown}}개 표시", {
        total: taskCounts ? taskCounts.groups.length : answer.total_count,
        shown,
      });
    }
    void askBoardReviews([...checkouts]);
    agentGraphClockBeat(answer.columns);
    void syncPreviewedTerms();
    return { cards, answer };
  } catch (error) {
    console.error("agent graph paint failed", error);
    boardBroken = true;
    paintBoardBroken(view);
    return null;
  }
}

async function paintBoardView(tab = boardTab(), { force = false } = {}) {
  if (!skillsReport) void refreshSkills();
  paintRequiredSkillBadges();
  const selectDefault = arguments[1]?.selectDefault !== false;
  return paintAgentGraphView(tab, { force, selectDefault });
}

/* ---- the session vault ---- */

/* A copied resume command stays acknowledged long enough to read before the
 * row restores its ordinary action label. */
const VAULT_COPY_FEEDBACK_MS = 1200;

/* Orca's `AiVaultPanel`: every conversation these agents have had, read out of
 * their own stores. The whole decision — filter, sort, group, label — is one
 * Rust call, for the same reason the board's columns are: the header counts and
 * the rows under them cannot disagree if one function produced both.
 *
 * Nothing on this screen writes. The two actions are reopen (a new terminal) and
 * copy (the clipboard); there is deliberately no rename, no delete, and no
 * "clean up old sessions", because these files are a vendor's record of somebody's
 * work and a browser that can damage them is not worth having. */
let vaultQuery = "";
let vaultGroup = "project";
let vaultSort = "updated";
/* 전체로 시작한다. 이 판은 보관소 표면이고(그래서 프로젝트별로 묶고 열일곱
 * 벤더를 한 목록에 싣는다), 지금 있는 작업 공간 옆에 서는 쪽은 독이다 —
 * 원본의 기본값 `workspace`는 그 독이 이미 쓰고 있다. */
let vaultScope = "all";
let vaultHideEmpty = false;
let vaultLoading = false;
let vaultPending = null;

function openVault() {
  openTab({ id: "vault", kind: "vault" });
}

/* The scan is not cheap — fourteen directory walks over stores that can hold
 * years of transcripts. So a keystroke does not re-walk the disk: the answer is
 * kept and only the query is re-asked, and the reload is what the refresh button
 * is for. */
let vaultAnswer = null;
/* The agents switched OFF, which is the way the original stores this too
 * (`disabledAgents`): a vendor this window learns to read next month appears in
 * the list on its own, where a set of enabled ones would leave it out. */
const vaultOffAgents = new Set();

/* Which directories a scope means.
 *
 * Takes the scope rather than reading one surface's own: both vault surfaces ask
 * this now, and the answer is a function of the workspace catalog and nothing
 * else. The version that read a global needed its caller to SET that global and
 * put it back to ask about a scope it was not in. */
function vaultScopePaths(scope) {
  if (scope === "workspace") {
    return activeWorktreePath ? [activeWorktreePath] : [];
  }
  if (scope === "project") {
    const home = projects.find((project) =>
      project.worktrees.some((worktree) => worktree.path === activeWorktreePath),
    );
    return home ? home.worktrees.map((worktree) => worktree.path) : [];
  }
  return [];
}

/* The scope a surface can actually ask for, and the paths that go with it.
 *
 * Empty is "everywhere" to the backend, so a scope that resolves to no directory
 * would MEAN 전체 while the control still said 워크스페이스. It retreats to 전체
 * and the control is repainted from what came back — Orca's
 * `normalizeAiVaultScopeForContext` makes the same retreat. */
function vaultScopeAsked(scope) {
  const paths = vaultScopePaths(scope);
  return paths.length === 0 ? { scope: "all", paths: [] } : { scope, paths };
}

async function refreshVault(force) {
  if (vaultLoading) {
    vaultPending = vaultPending === true || force === true;
    return;
  }
  const asked = vaultScopeAsked(vaultScope);
  vaultScope = asked.scope;
  vaultLoading = true;
  paintVaultView();
  try {
    vaultAnswer = await invoke("vault_sessions", {
      query: {
        query: vaultQuery,
        disabled_agents: [...vaultOffAgents],
        sort: vaultSort,
        group: vaultGroup,
        hide_empty: vaultHideEmpty,
        scope_paths: asked.paths,
        limit: vaultSessionLimit,
      },
      /* The disk is re-read when somebody ASKS for it. Every other call from
       * here is a different question about the same walk — which is what the
       * note above this function has claimed since it was written. */
      force: force === true,
    });
  } catch (error) {
    vaultAnswer = null;
    showError(String(error));
  } finally {
    vaultLoading = false;
  }
  paintVaultView();
  if (vaultPending !== null) {
    const pending = vaultPending;
    vaultPending = null;
    return refreshVault(pending);
  }
}

/* A card being carried to a workspace.
 *
 * The transfer holds the card's id and nothing else, and the session itself
 * stays in this variable: a `dragover` may not read a transfer's DATA — only
 * its types — so a drop target that had to unpack the payload to know whether
 * to accept it could never light up, and the backend refuses a resume command
 * it did not build anyway, which makes shipping the whole record across a
 * clipboard channel a copy for no reader. The id is what the drop checks the
 * held session against, so a transfer from somewhere else cannot resume a card
 * this window happens to still be holding. */
const VAULT_DRAG_TYPE = "application/x-zerocode-vault-session";
let vaultDragged = null;

/* Is what is being dragged one of our session cards? All `dragover` can ask. */
function carriesVaultSession(transfer) {
  return !!transfer && [...transfer.types].includes(VAULT_DRAG_TYPE);
}

/* The session a drop is carrying, or nothing. */
function droppedVaultSession(transfer) {
  if (!carriesVaultSession(transfer) || vaultDragged === null) return null;
  return transfer.getData(VAULT_DRAG_TYPE) === vaultDragged.id ? vaultDragged : null;
}

/* Both vault surfaces share the saved limit and child disclosure state. */
const vaultChildrenOpen = new Set();
let vaultLimits = null;
let vaultSessionLimit = null;

function vaultIsChild(session) {
  return session.depth > 0 || Boolean(session.parent_id);
}

/* Both surfaces keep their own row renderer; the disclosure and recorded
 * provenance are shared. Children are rendered only when their parent opens. */
function vaultSubagents(session, renderer) {
  if (vaultIsChild(session) || !session.children?.length) return null;
  const section = document.createElement("section");
  section.className = "vault-subagents";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "vault-subagents-toggle";
  toggle.textContent = t("vaultDock.subagents", "서브에이전트 {{n}}", { n: session.children.length });
  const list = document.createElement("div");
  list.className = "vault-subagents-list";
  const paint = () => {
    const open = vaultChildrenOpen.has(session.id);
    toggle.setAttribute("aria-expanded", String(open));
    list.hidden = !open;
    list.replaceChildren(...(open ? session.children.map(renderer) : []));
  };
  const set = (open) => {
    if (open) vaultChildrenOpen.add(session.id);
    else vaultChildrenOpen.delete(session.id);
    paint();
  };
  toggle.addEventListener("click", () => set(!vaultChildrenOpen.has(session.id)));
  toggle.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    event.stopPropagation();
    set(event.key === "ArrowRight");
  });
  section.addEventListener("click", event => event.stopPropagation());
  section.append(toggle, list);
  paint();
  return section;
}

function vaultChildContext(session) {
  const context = document.createElement("div");
  context.className = "vault-child-context";
  const origin = session.origin ?? {};
  for (const [value, label] of [
    [origin.pane, t("vaultDock.subagentsPane", "판 {{id}}", { id: origin.pane })],
    [origin.worker, t("vaultDock.subagentsWorker", "워커 {{id}}", { id: origin.worker })],
    [origin.tool_call_id, t("vaultDock.subagentsTool", "도구 {{id}}", { id: origin.tool_call_id })],
  ]) {
    if (!value) continue;
    const badge = document.createElement("span");
    badge.className = "vault-origin";
    badge.textContent = label;
    context.appendChild(badge);
  }
  const hint = document.createElement("span");
  hint.className = "vault-child-hint";
  hint.textContent = t("vaultDock.subagentsParent", "부모 세션에서 이어집니다");
  context.appendChild(hint);
  return context;
}

async function setVaultSessionLimit(limit) {
  await commitSetting("vault.sessionLimit", "set_vault_session_limit", { limit });
}

function foldVaultIssues(issues) {
  if (!Array.isArray(issues)) return [];
  const map = new Map();
  for (const issue of issues) {
    if (!issue || typeof issue !== "object") continue;
    const agent = issue.agent ?? "";
    const reason = issue.reason ?? "";
    const key = `${agent}::${reason}`;
    const held = map.get(key);
    const count = Math.max(1, Number(issue.count) || 1);
    if (held) {
      held.count += count;
    } else {
      map.set(key, { ...issue, agent, reason, count });
    }
  }
  return [...map.values()];
}

function vaultIssueText(issue) {
  const reasons = {
    "child-parent-missing": t("vaultDock.subagentsMissing", "부모 세션을 찾을 수 없습니다"),
    "child-parent-ambiguous": t("vaultDock.subagentsAmbiguous", "부모 세션을 구분할 수 없습니다"),
    "child-depth": t("vaultDock.subagentsDepth", "한 단계 아래 서브에이전트만 표시합니다"),
    "child-limit": t("vaultDock.subagentsLimit", "부모당 서브에이전트 한도에 도달했습니다"),
    "unreadable": t("vaultDock.subagentsUnreadable", "전사를 읽을 수 없습니다"),
  };
  const reason = reasons[issue.reason] ?? t("vault.unread", "형식은 아직 못 읽습니다");
  const prefix = issue.agent ? `${agentName(issue.agent)}: ` : "";
  const count = issue.count ? ` (${issue.count})` : "";
  return `${prefix}${reason}${count}`;
}

/* One session. The title is what was said; under it the directory, branch,
 * and agent distinguish similar conversations. */
function vaultRow(session) {
  const child = vaultIsChild(session);
  if (child) session = { ...session, resume: null };
  const row = document.createElement("div");
  row.className = "vault-row";
  row.dataset.agent = session.agent;
  // Draggable only where there is a command, for the same reason the 다시 열기
  // button is only there: a card that cannot be reopened would light up every
  // workspace row on the way past and then fail on the drop.
  if (session.resume) {
    row.draggable = true;
    // Said on the card, because a drag nobody knows about is a feature nobody
    // has. The button beside it stays the way to reopen a session where it ran.
    row.dataset.tip = t("vault.dragHint", "워크스페이스로 끌어다 놓으면 그곳에서 이어집니다");
    row.addEventListener("dragstart", (event) => {
      vaultDragged = session;
      event.dataTransfer.effectAllowed = "copy";
      event.dataTransfer.setData(VAULT_DRAG_TYPE, session.id);
    });
    row.addEventListener("dragend", () => {
      vaultDragged = null;
    });
  }

  const body = document.createElement("div");
  body.className = "vault-row-body";

  const title = document.createElement("span");
  title.className = "vault-row-title";
  title.textContent = session.title;
  body.appendChild(title);

  const meta = document.createElement("span");
  meta.className = "vault-row-meta";
  const parts = [agentName(session.agent)];
  if (session.branch) parts.push(session.branch);
  if (session.message_count > 0) {
    // The number first: every language in the catalogs reads "12 turns" and none
    // of them reads "turns 12".
    parts.push(`${session.message_count} ${t("vault.messages", "번 주고받음")}`);
  }
  meta.textContent = parts.join(" · ");
  body.appendChild(meta);

  if (session.cwd) {
    const where = document.createElement("span");
    where.className = "vault-row-path";
    where.textContent = session.cwd;
    body.appendChild(where);
  }
  if (child) body.appendChild(vaultChildContext(session));
  row.appendChild(body);

  const actions = document.createElement("div");
  actions.className = "vault-row-actions";
  if (session.resume) {
    const open = document.createElement("button");
    open.type = "button";
    open.className = "vault-act";
    open.textContent = t("vault.resume", "다시 열기");
    open.onclick = () => resumeVaultSession(session);
    actions.appendChild(open);

    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "vault-act";
    copy.textContent = t("vault.copy", "명령 복사");
    copy.onclick = async () => {
      if (!(await clipboardText.write(session.resume))) return;
      // The button says what happened, and puts itself back. There is no toast
      // in this window and inventing one for a copy would be a new surface.
      copy.textContent = t("vault.copied", "명령을 복사했습니다");
      setTimeout(() => {
        copy.textContent = t("vault.copy", "명령 복사");
      }, VAULT_COPY_FEEDBACK_MS);
    };
    actions.appendChild(copy);
  } else if (!child) {
    // Said rather than shown as a dead button: an agent this window has no
    // resume spelling for is a fact, and a button that fails is a bug report.
    const no = document.createElement("span");
    no.className = "vault-row-meta";
    no.textContent = t("vault.noResume", "이 에이전트는 여기서 다시 열 수 없습니다");
    actions.appendChild(no);
  }
  // Always offered, including for the agents that cannot be reopened: the
  // transcript is on disk either way, and looking at it is the one thing that
  // always works.
  const show = document.createElement("button");
  show.type = "button";
  show.className = "vault-act";
  show.textContent = t("vault.reveal", "파일 보기");
  show.onclick = () => {
    invoke("reveal_vault_session", { path: session.file_path }).catch(showError);
  };
  if (!child) actions.appendChild(show);
  row.appendChild(actions);
  const children = vaultSubagents(session, vaultRow);
  if (children) row.appendChild(children);
  if (child) {
    row.setAttribute("aria-expanded", "false");
    const toggle = () => {
      const detail = row.querySelector(".vdock-detail");
      if (detail) detail.remove();
      else row.appendChild(vaultDockDetail(session));
      row.setAttribute("aria-expanded", String(!detail));
    };
    actsAsButton(row, toggle);
  }
  return row;
}

/* Reopen a conversation. `worktree` is the workspace a card was dropped on and
 * is absent for the button, which is the whole difference between the two:
 * dropped, the session continues in that workspace; pressed, where it ran. */
async function resumeVaultSession(session, worktree) {
  // Moved to first, so the tab is born in the checkout its shell will run in —
  // and a refused move (documents with unsaved typing kept) stops the resume
  // rather than leaving a terminal in a workspace nobody is standing in.
  if (worktree && worktree !== activeWorktreePath && !(await activateWorktree(worktree))) return;
  try {
    const term = await invoke("resume_vault_session", {
      session,
      worktree: worktree ?? null,
      ...spawnGrid({ placement: "tab" }),
    });
    // Named for what was SAID in it, not for the agent: a vault with five Claude
    // conversations would otherwise open five tabs all called `claude`, which is
    // the exact thing reopening a specific one is for. A tab for the same reason
    // the pane menu's 이어서 is one — a conversation reopened is somewhere to go.
    mountTermTab(term, { agent: session.title }, { placement: "tab" });
  } catch (error) {
    showError(String(error));
  }
}

function vaultGroupNode(group) {
  const box = document.createElement("section");
  box.className = "vault-group";

  const head = document.createElement("header");
  head.className = "vault-group-head";
  const label = document.createElement("span");
  label.className = "vault-group-label";
  label.textContent = group.label;
  head.appendChild(label);
  const count = document.createElement("span");
  count.className = "vault-group-count";
  count.textContent = String(group.sessions.length);
  head.appendChild(count);
  box.appendChild(head);

  for (const session of group.sessions) box.appendChild(vaultRow(session));
  return box;
}

/* Which agents the list is showing. The rows come from the ANSWER rather than
 * from a table here, so an agent this window learns to read arrives in the list
 * without the window being taught its name — and each row carries how many
 * sessions it brought, which is the part that tells you whether switching it off
 * would change anything.
 *
 * One master row rather than the original's two buttons ("Select all" and
 * "Clear"), because the two directions are never both worth offering: while
 * something is switched off the only useful press is "show everything again",
 * and while nothing is, it is "show nothing" — which is the first half of
 * isolating one vendor, the second being that vendor's own row. So one row
 * that reads the state it is in, and both buttons' work is in it.
 *
 * Rebuilt in place after every press — `openSidebarMenu` replaces its rows on
 * the spot, which is what lets a filter be set without the menu blinking. */
function openVaultFilters(x, y, opener) {
  const rows = vaultAnswer?.agents ?? [];
  const allOn = rows.every((row) => !vaultOffAgents.has(row.slug));
  const again = () => {
    openVaultFilters(x, y, opener);
    return refreshVault();
  };
  const items = [{ caption: t("vault.agents", "에이전트") }];
  if (rows.length === 0) {
    items.push({ label: t("vault.empty", "세션이 없습니다"), disabled: true, run: () => {} });
  } else {
    items.push({
      check: true,
      checked: allOn,
      label: t("vault.allAgents", "모두"),
      run: () => {
        if (allOn) for (const row of rows) vaultOffAgents.add(row.slug);
        else vaultOffAgents.clear();
        return again();
      },
    });
    for (const row of rows) {
      const on = !vaultOffAgents.has(row.slug);
      items.push({
        check: true,
        checked: on,
        indent: true,
        label: `${row.label} (${row.sessions})`,
        run: () => {
          if (on) vaultOffAgents.add(row.slug);
          else vaultOffAgents.delete(row.slug);
          return again();
        },
      });
    }
  }
  openSidebarMenu(x, y, items, opener);
}

function wireVaultHead(view) {
  const query = view.querySelector(".vault-query");
  query.value = vaultQuery;
  // Only the query is re-asked on a keystroke; the disk is walked once. See
  // `refreshVault`.
  query.oninput = (event) => {
    vaultQuery = event.target.value;
    refreshVault();
  };
  const group = view.querySelector("#vault-group") ?? view.querySelector(".vault-group-select");
  if (group) {
    group.value = vaultGroup;
    group.onchange = (event) => {
      vaultGroup = event.target.value;
      refreshVault();
    };
  }
  const sort = view.querySelector("#vault-sort") ?? view.querySelector(".vault-sort-select");
  if (sort) {
    sort.value = vaultSort;
    sort.onchange = (event) => {
      vaultSort = event.target.value;
      refreshVault();
    };
  }
  const scope = view.querySelector("#vault-scope");
  if (scope) {
    scope.value = vaultScope;
    /* A scope with nowhere to point is offered and dead, not absent: there is no
     * project to narrow to until a workspace is open, and a select whose items
     * come and go is one you have to read on every visit. */
    for (const option of scope.options) {
      option.disabled = option.value !== "all" && vaultScopePaths(option.value).length === 0;
    }
    scope.onchange = (event) => {
      vaultScope = event.target.value;
      refreshVault();
    };
  }
  const empty = view.querySelector("#vault-hide-empty") ?? view.querySelector(".vault-hide-empty");
  if (empty) {
    empty.checked = vaultHideEmpty;
    empty.onchange = (event) => {
      vaultHideEmpty = event.target.checked;
      refreshVault();
    };
  }
  const filters = view.querySelector(".vault-filters");
  if (filters) {
    /* 좁혀 놓은 것은 팝오버를 열지 않고도 보인다 — GitHub 필터 단추가 같은
     * 규칙을 쓴다(`task-gh-filters`의 `is-active`). 답에 있는 줄과 맞춰 읽으므로
     * 세션이 사라진 벤더의 낡은 슬러그 하나가 판을 영원히 「좁혀진」 것으로
     * 보이게 하지 않는다. */
    const narrowed = (vaultAnswer?.agents ?? []).some((row) => vaultOffAgents.has(row.slug));
    filters.classList.toggle("is-active", narrowed);
    filters.onclick = (event) => {
      const box = event.currentTarget.getBoundingClientRect();
      openVaultFilters(box.left, box.bottom + 4, event.currentTarget);
    };
  }
  const again = view.querySelector(".vault-refresh");
  if (again) again.onclick = () => refreshVault(true);
}

function paintVaultView(tab = tabs.find((held) => held.kind === "vault")) {
  if (!tab) return;
  const view = docHost(tab.pane, "vault");
  wireVaultHead(view);
  const host = view.querySelector(".vault-list");
  const count = view.querySelector(".vault-count");
  const note = view.querySelector(".vault-note");

  if (!vaultAnswer) {
    host.replaceChildren();
    count.textContent = vaultLoading ? t("vault.reading", "세션을 읽고 있습니다…") : "";
    note.hidden = true;
    return;
  }
  host.replaceChildren();
  for (const group of vaultAnswer.groups) host.appendChild(vaultGroupNode(group));
  if (vaultAnswer.groups.length === 0) {
    const line = document.createElement("p");
    line.className = "vault-row-meta";
    line.textContent = t("vault.empty", "세션이 없습니다");
    host.appendChild(line);
  }
  // Both numbers, because they answer different questions: how many sessions
  // exist, and how many this search is showing.
  count.textContent =
    vaultAnswer.shown === vaultAnswer.total
      ? String(vaultAnswer.total)
      : `${vaultAnswer.shown} ${t("vault.of", "/")} ${vaultAnswer.total}`;

  // What the scan could not do. Never hidden: a list that is a recent slice and
  // looks complete is the one failure this surface can hide.
  const lines = [];
  if (vaultAnswer.truncated) lines.push(t("vault.truncated", "최근 세션만 보여줍니다"));
  for (const issue of foldVaultIssues(vaultAnswer.issues)) {
    lines.push(vaultIssueText(issue));
  }
  note.textContent = lines.join(" · ");
  note.hidden = lines.length === 0;
}

/* ---- the session history, docked in the right column (1-g77) ----
 *
 * Orca's AiVaultPanel is a right-column activity, not a stage tab (App's
 * RightSidebarPanelContent renders it for tab "vault"). The stage view above
 * keeps the deep tools; this is the glanceable face of the same ledger:
 * header with the shown/recent counts, the three-way scope, a search, and the
 * grouped cards — title, the latest turn, then the agent, the message count,
 * the age and the model on one muted line (VaultSessionRow / SessionMetadata,
 * AiVaultPanel-DoHZwnjG.js:2611/:2518). */
let vaultDockAnswer = null;
let vaultDockLoading = false;
// DEFAULT_AI_VAULT_SCOPE is "workspace" (AiVaultPanel-DoHZwnjG.js:448).
let vaultDockScope = "workspace";
let vaultDockQuery = "";
const vaultDockShut = new Set();
/* Which cards stand open to their inline details — Orca's expandedSessionIds,
 * a Set so several can be open at once, keyed by the session's one id. */
const vaultDockOpen = new Set();
/* The dock's own view menu (Orca's VaultViewMenu): which vendors are switched
 * off, and whether empty sessions hide. A pair separate from the stage view's
 * on purpose — the original persists view options per surface too. */
const vaultDockOff = new Set();
let vaultDockHideEmpty = false;
let vaultDockPending = null;

async function refreshVaultDock(force) {
  if (vaultDockLoading) {
    vaultDockPending = vaultDockPending === true || force === true;
    return;
  }
  const asked = vaultScopeAsked(vaultDockScope);
  vaultDockScope = asked.scope;
  vaultDockLoading = true;
  paintVaultDock();
  try {
    // Orca's docked ordering is fixed (updated-first, grouped by project);
    // the stage view is where THOSE are choices. The vendors and the empty
    // sessions are the dock's own view menu (VaultViewMenu).
    vaultDockAnswer = await invoke("vault_sessions", {
      query: {
        query: vaultDockQuery,
        disabled_agents: [...vaultDockOff],
        sort: "updated",
        group: "project",
        hide_empty: vaultDockHideEmpty,
        scope_paths: asked.paths,
        limit: vaultSessionLimit,
      },
      /* Same walk as the panel's, and the same rule about re-reading it: a
       * scope switch is a question, the reload is an instruction. Two surfaces
       * over one store is the point — opening both used to cost two walks. */
      force: force === true,
    });
  } catch (error) {
    vaultDockAnswer = null;
    showError(String(error));
  } finally {
    vaultDockLoading = false;
  }
  paintVaultDock();
  if (vaultDockPending !== null) {
    const pending = vaultDockPending;
    vaultDockPending = null;
    return refreshVaultDock(pending);
  }
}

/* How long ago, in Orca's buckets (formatTimeAgo, AiVaultPanel-DoHZwnjG.js:
 * 2502): under a minute, then minutes, hours, days, months as days/30, years.
 * The words come from the catalogs; the arithmetic is the measured one. */
function vaultDockAge(iso) {
  const then = Date.parse(iso ?? "");
  if (!Number.isFinite(then)) return t("vaultDock.unknownTime", "알 수 없는 시간");
  const gap = Math.max(0, Date.now() - then);
  if (gap < 60_000) return t("vaultDock.justNow", "방금 전");
  const minutes = Math.floor(gap / 60_000);
  if (minutes < 60) return t("vaultDock.minutesAgo", "{{n}}분 전", { n: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("vaultDock.hoursAgo", "{{n}}시간 전", { n: hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return t("vaultDock.daysAgo", "{{n}}일 전", { n: days });
  const months = Math.floor(days / 30);
  if (months < 12) return t("vaultDock.monthsAgo", "{{n}}개월 전", { n: months });
  return t("vaultDock.yearsAgo", "{{n}}년 전", { n: Math.floor(months / 12) });
}

/* One card. Orca's row is title, the latest turn with its speaker named, then
 * the muted meta line — agent mark, agent, count, age, model, dots between
 * (VaultSessionRow). The three hands — reopen, copy the command, show the
 * file — are the stage view's, shown on hover the way Orca's are. */
function vaultDockCard(session) {
  const child = vaultIsChild(session);
  if (child) session = { ...session, resume: null };
  const card = document.createElement("div");
  card.className = "vdock-card";
  card.dataset.agent = session.agent;

  // 제목 줄 — 원본 SessionRowTrailingActions의 늘 보이는 반쪽: 접힘 화살표와
  // ⋯ 메뉴가 제목 오른쪽에 선다. 다시 열기는 호버 손으로 남는다(그쪽 반).
  const top = document.createElement("div");
  top.className = "vdock-card-top";
  const title = document.createElement("span");
  title.className = "vdock-card-title";
  title.textContent = session.title;
  top.appendChild(title);
  const twist = document.createElement("span");
  twist.className = "vdock-twist";
  twist.innerHTML = icon("chevron", vaultDockOpen.has(session.id));
  top.appendChild(twist);
  const more = document.createElement("button");
  more.type = "button";
  more.className = "vdock-act vdock-more";
  more.dataset.tip = t("vaultDock.cardMenu", "세션 동작");
  more.setAttribute("aria-label", t("vaultDock.cardMenu", "세션 동작"));
  more.innerHTML = icon("more");
  more.onclick = (event) => {
    const box = event.currentTarget.getBoundingClientRect();
    const items = [];
    if (session.resume) {
      items.push({ label: t("vault.resume", "다시 열기"), run: () => resumeVaultSession(session) });
      items.push({
        label: t("vault.copy", "명령 복사"),
        run: () => void clipboardText.write(session.resume),
      });
    }
    items.push({
      label: t("vault.reveal", "파일 보기"),
      run: () => invoke("reveal_vault_session", { path: session.file_path }).catch(showError),
    });
    openSidebarMenu(box.left, box.bottom + 4, items, event.currentTarget);
  };
  if (!child) top.appendChild(more);
  card.appendChild(top);

  // The last thing anyone said, with who said it — user and assistant lines
  // only, the two roles the preview keeps (`displayableSessionPreviewMessages`
  // filters to them; the card takes the LAST).
  const turn = [...session.preview]
    .reverse()
    .find((line) => line.role === "user" || line.role === "assistant");
  const preview = document.createElement("p");
  preview.className = "vdock-card-preview";
  if (turn) {
    const who = document.createElement("span");
    who.className = "vdock-role";
    who.textContent =
      turn.role === "user"
        ? t("vaultDock.roleUser", "나")
        : t("vaultDock.roleAgent", "에이전트");
    const said = document.createElement("span");
    said.textContent = `: ${turn.text}`;
    preview.append(who, said);
  } else {
    preview.textContent = t("vaultDock.noPreview", "대화 미리보기를 사용할 수 없습니다");
  }
  card.appendChild(preview);

  const meta = document.createElement("div");
  meta.className = "vdock-card-meta";
  const reg = agentRows.find((one) => one.id === session.agent);
  const face = document.createElement("span");
  face.className = "vdock-agent-face";
  face.appendChild(
    agentIcon(reg ?? { id: session.agent, name: agentName(session.agent), favicon_domain: "" }),
  );
  meta.appendChild(face);
  const words = document.createElement("span");
  words.className = "vdock-meta-words";
  const bits = [agentName(session.agent)];
  if (session.message_count > 0) {
    bits.push(t("vaultDock.messageCount", "메시지 {{n}}개", { n: session.message_count }));
  }
  // The time a card wears is updated-or-mtime, the same fallback the sort
  // stands on (`updatedAt ?? modifiedAt`, :2612).
  bits.push(vaultDockAge(session.updated_at ?? session.modified_at));
  if (session.model) bits.push(session.model);
  words.textContent = bits.join(" · ");
  meta.appendChild(words);
  card.appendChild(meta);
  if (child) card.appendChild(vaultChildContext(session));

  // 호버의 손은 하나 — 원본의 hover-reveal 무리에서 이 창이 쥔 것(다시
  // 열기). 복사와 파일 보기는 ⋯ 메뉴와 펼친 상세의 띠가 이미 쥐고 있다.
  if (session.resume) {
    const acts = document.createElement("div");
    acts.className = "vdock-acts";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "vdock-act";
    open.dataset.tip = t("vault.resume", "다시 열기");
    open.setAttribute("aria-label", t("vault.resume", "다시 열기"));
    open.innerHTML = icon("play");
    open.onclick = () => resumeVaultSession(session);
    acts.appendChild(open);
    card.appendChild(acts);
  }
  // 카드 몸이 문이다(라이브 보고 "세션창 클릭 orca처럼"): 클릭이 인라인
  // 상세를 여닫는다(toggleSessionDetails) — 손 위의 클릭은 제 일만 한다.
  card.setAttribute("aria-expanded", vaultDockOpen.has(session.id) ? "true" : "false");
  actsAsButton(card, (event) => {
    if (event.target.closest(".vdock-act") || event.target.closest(".vdock-detail")) return;
    if (vaultDockOpen.has(session.id)) vaultDockOpen.delete(session.id);
    else vaultDockOpen.add(session.id);
    paintVaultDock();
  });
  if (vaultDockOpen.has(session.id)) card.appendChild(vaultDockDetail(session));
  const children = vaultSubagents(session, vaultDockCard);
  if (children) card.appendChild(children);
  return card;
}

/* The inline details — Orca's expanded row (AiVaultSessionDetails): an action
 * strip over a receipt body of the first ask, the latest turns (the preview's
 * own three — PREVIEW_LINES is Orca's constant too), and the worktree lines.
 * Recorded deviations: our one resume stands where Orca splits worktree/new-tab,
 * and "Continue in New Session…" waits on a flow this window does not hold yet. */
function vaultDockDetail(session) {
  const child = vaultIsChild(session);
  if (child) session = { ...session, resume: null };
  const detail = document.createElement("div");
  detail.className = "vdock-detail";
  const strip = document.createElement("div");
  strip.className = "vdock-detail-acts";
  if (session.resume) {
    const open = document.createElement("button");
    open.type = "button";
    open.className = "vdock-detail-act vdock-detail-act--primary";
    open.innerHTML = `${icon("play")}<span>${t("vault.resume", "다시 열기")}</span>`;
    open.addEventListener("click", () => resumeVaultSession(session));
    strip.appendChild(open);
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "vdock-detail-act";
    copy.innerHTML = `${icon("copy")}<span>${t("vault.copy", "명령 복사")}</span>`;
    copy.addEventListener("click", () => void clipboardText.write(session.resume));
    strip.appendChild(copy);
  }
  const reveal = document.createElement("button");
  reveal.type = "button";
  reveal.className = "vdock-detail-act";
  reveal.innerHTML = `${icon("file")}<span>${t("vault.reveal", "파일 보기")}</span>`;
  reveal.addEventListener("click", () => {
    invoke("reveal_vault_session", { path: session.file_path }).catch(showError);
  });
  if (!child) {
    strip.appendChild(reveal);
    detail.appendChild(strip);
  }

  const body = document.createElement("div");
  body.className = "vdock-detail-body";
  const section = (label) => {
    const box = document.createElement("section");
    box.className = "vdock-receipt";
    const head = document.createElement("div");
    head.className = "vdock-receipt-label";
    head.textContent = label;
    box.appendChild(head);
    return box;
  };
  const turnCard = (line) => {
    const one = document.createElement("div");
    one.className = "vdock-turn";
    one.dataset.role = line.role;
    const who = document.createElement("div");
    who.className = "vdock-turn-role";
    who.textContent =
      line.role === "user" ? t("vaultDock.roleUser", "나") : t("vaultDock.roleAgent", "에이전트");
    const said = document.createElement("p");
    said.className = "vdock-turn-text";
    said.textContent = line.text;
    one.append(who, said);
    return one;
  };
  const firstAsk = session.preview.find((line) => line.role === "user");
  if (firstAsk) {
    const first = section(t("vaultDock.firstPrompt", "첫 프롬프트"));
    first.appendChild(turnCard(firstAsk));
    body.appendChild(first);
  }
  const latest = section(t("vaultDock.latestTurns", "최근 대화"));
  const turns = session.preview.filter(
    (line) => line.role === "user" || line.role === "assistant",
  );
  if (turns.length === 0) {
    const none = document.createElement("p");
    none.className = "vdock-turn-none";
    none.textContent = t("vaultDock.noPreview", "대화 미리보기를 사용할 수 없습니다");
    latest.appendChild(none);
  } else {
    for (const line of turns) latest.appendChild(turnCard(line));
  }
  body.appendChild(latest);
  if (session.cwd) {
    const where = section(t("vaultDock.worktree", "worktree"));
    const path = document.createElement("p");
    path.className = "vdock-receipt-line";
    path.textContent = session.branch ? `${session.cwd} · ${session.branch}` : session.cwd;
    where.appendChild(path);
    body.appendChild(where);
  }
  detail.appendChild(body);
  return detail;
}

/* One group: Orca's sticky header — chevron, the label, a count pill — over
 * its cards, folding on click (VaultGroupHeader, collapsedGroups). */
function vaultDockGroup(group) {
  const wrap = document.createElement("div");
  wrap.className = "vdock-group";
  const head = document.createElement("button");
  head.type = "button";
  head.className = "vdock-group-head";
  const shut = vaultDockShut.has(group.key);
  head.setAttribute("aria-expanded", shut ? "false" : "true");
  head.innerHTML =
    `<span class="vdock-group-twist">${icon("chevron", !shut)}</span>` +
    '<span class="vdock-group-label"></span><span class="vdock-group-count"></span>';
  head.querySelector(".vdock-group-label").textContent = group.label;
  head.querySelector(".vdock-group-count").textContent = String(group.sessions.length);
  wrap.appendChild(head);
  const body = document.createElement("div");
  body.className = "vdock-group-body";
  body.hidden = shut;
  for (const session of group.sessions) body.appendChild(vaultDockCard(session));
  wrap.appendChild(body);
  head.onclick = () => {
    if (vaultDockShut.has(group.key)) vaultDockShut.delete(group.key);
    else vaultDockShut.add(group.key);
    body.hidden = !body.hidden;
    head.setAttribute("aria-expanded", body.hidden ? "false" : "true");
    head.querySelector(".icon--twist")?.classList.toggle("is-open", !body.hidden);
  };
  return wrap;
}

function paintVaultDock() {
  const sub = el("vdock-sub");
  const list = el("vdock-list");
  const note = el("vdock-note");
  for (const scope of ["workspace", "project", "all"]) {
    const btn = el(`vdock-scope-${scope}`);
    const on = vaultDockScope === scope;
    btn.classList.toggle("is-on", on);
    btn.setAttribute("aria-pressed", on ? "true" : "false");
  }
  // A scope with nothing to mean is a door that opens onto the wrong room —
  // Orca disables these two the same way (VaultScopeSwitch).
  el("vdock-scope-workspace").disabled = !activeWorktreePath;
  el("vdock-scope-project").disabled = vaultScopePaths("project").length === 0;

  if (!vaultDockAnswer) {
    list.replaceChildren();
    note.hidden = true;
    // Before the first scan the header makes the panel's offer instead of
    // counting nothing (`resumePastSessions`).
    sub.textContent = t("vaultDock.resume", "이전 세션 재개");
    return;
  }
  sub.textContent = t("vaultDock.limitShown", "{{total}}개 중 {{shown}}개 표시", {
    shown: vaultDockAnswer.shown,
    total: vaultDockAnswer.matched ?? vaultDockAnswer.total,
  });

  // The same honesty line as the stage view: what the walk cut or could not
  // read, above the list rather than under it (AiVaultScanIssueBanners sits
  // between header and list).
  const lines = [];
  if (vaultDockAnswer.truncated) lines.push(t("vault.truncated", "최근 세션만 보여줍니다"));
  for (const issue of foldVaultIssues(vaultDockAnswer.issues)) {
    lines.push(vaultIssueText(issue));
  }
  note.textContent = lines.join(" · ");
  const canShowMore = vaultDockAnswer.shown < (vaultDockAnswer.matched ?? vaultDockAnswer.total)
    && vaultSessionLimit !== 0;
  if (canShowMore) {
    const more = document.createElement("button");
    more.type = "button";
    more.className = "vault-limit-more";
    more.textContent = t("vaultDock.limitMore", "더 보기");
    more.onclick = () => setVaultSessionLimit(vaultLimits?.choices.find(value => value > (vaultSessionLimit ?? vaultLimits.default)) ?? 0);
    note.appendChild(more);
  }
  note.hidden = lines.length === 0 && !canShowMore;

  list.replaceChildren();
  for (const group of vaultDockAnswer.groups) list.appendChild(vaultDockGroup(group));
  if (vaultDockAnswer.groups.length === 0) {
    const empty = document.createElement("p");
    empty.className = "vdock-empty";
    empty.textContent =
      vaultDockAnswer.total === 0
        ? t("vaultDock.empty", "agent 세션을 찾을 수 없습니다")
        : t("vaultDock.noMatch", "현재 필터와 일치하는 세션이 없습니다");
    list.appendChild(empty);
  }
}

el("vdock-refresh").addEventListener("click", () => void refreshVaultDock(true));
el("vdock-query").addEventListener("input", (event) => {
  vaultDockQuery = event.target.value;
  el("vdock-clear").hidden = vaultDockQuery === "";
  void refreshVaultDock();
});
el("vdock-clear").addEventListener("click", () => {
  vaultDockQuery = "";
  el("vdock-query").value = "";
  el("vdock-clear").hidden = true;
  el("vdock-query").focus();
  void refreshVaultDock();
});
for (const scope of ["workspace", "project", "all"]) {
  el(`vdock-scope-${scope}`).addEventListener("click", () => {
    vaultDockScope = scope;
    void refreshVaultDock();
  });
}

/* The dock's ≡ menu — Orca's VaultViewMenu, the essentials: the vendor rows
 * (counts from the answer, one master row the way the stage filter has), the
 * empty-session hide, and reset. Rebuilt in place per press, like
 * `openVaultFilters`, so a toggle does not blink the menu shut. */
function openVaultDockView(x, y, opener) {
  const rows = vaultDockAnswer?.agents ?? [];
  const allOn = rows.every((row) => !vaultDockOff.has(row.slug));
  const again = () => {
    openVaultDockView(x, y, opener);
    return refreshVaultDock();
  };
  const items = [{ caption: t("vaultDock.limit", "세션 한도") }];
  const limits = vaultLimits ?? vaultDockAnswer?.limits;
  for (const limit of [...(limits?.choices ?? []), 0]) {
    items.push({
      check: true,
      checked: limit === (vaultSessionLimit ?? limits?.default),
      label: limit === 0 ? t("vaultDock.limitAll", "전체") : t("vaultDock.limitCount", "{{n}}개", { n: limit }),
      run: () => setVaultSessionLimit(limit),
    });
  }
  items.push({ separator: true }, { caption: t("vault.agents", "에이전트") });
  if (rows.length === 0) {
    items.push({ label: t("vault.empty", "세션이 없습니다"), disabled: true, run: () => {} });
  } else {
    items.push({
      check: true,
      checked: allOn,
      label: t("vault.allAgents", "모두"),
      run: () => {
        if (allOn) for (const row of rows) vaultDockOff.add(row.slug);
        else vaultDockOff.clear();
        return again();
      },
    });
    for (const row of rows) {
      items.push({
        check: true,
        checked: !vaultDockOff.has(row.slug),
        label: `${agentName(row.slug)} (${row.sessions ?? row.count})`,
        run: () => {
          if (vaultDockOff.has(row.slug)) vaultDockOff.delete(row.slug);
          else vaultDockOff.add(row.slug);
          return again();
        },
      });
    }
  }
  items.push({ separator: true });
  items.push({
    check: true,
    checked: vaultDockHideEmpty,
    label: t("vault.hideEmpty", "빈 세션 숨기기"),
    run: () => {
      vaultDockHideEmpty = !vaultDockHideEmpty;
      return again();
    },
  });
  items.push({ separator: true });
  items.push({
    label: t("vaultDock.reset", "보기 초기화"),
    run: () => {
      vaultDockOff.clear();
      vaultDockHideEmpty = false;
      return again();
    },
  });
  openSidebarMenu(x, y, items, opener);
}

el("vdock-view").addEventListener("click", (event) => {
  const box = event.currentTarget.getBoundingClientRect();
  openVaultDockView(box.left, box.bottom + 4, event.currentTarget);
});
/* Orca's VaultHostScopeMenu. Sessions run on this machine only, so the menu
 * tells that truth — one row, checked; the chip is the same honest word. */
el("vdock-host").addEventListener("click", (event) => {
  const box = event.currentTarget.getBoundingClientRect();
  openSidebarMenu(
    box.left,
    box.bottom + 4,
    [
      { caption: t("vaultDock.hostScope", "실행 호스트") },
      { check: true, checked: true, label: t("vaultDock.hostLocal", "Local Mac"), run: () => {} },
    ],
    event.currentTarget,
  );
});

/* Is the board the surface in front of somebody right now? Repainting it costs
 * two commands, so a state change that nobody is looking at should not. */
function boardIsOpen() {
  // 팝아웃 창에는 보드 말고 아무것도 없으므로 언제나 앞에 있다 — 상태가
  // 바뀔 때마다 다시 그려야 하는 창이 바로 그 창이다.
  return isPopout || tabs.some((tab) => tab.kind === "board" && tab.id === activeTabId);
}

/* Which of two states is the one a person needs to see. Waiting beats working
 * beats done — a tab with one agent asking and one grinding is a tab you have
 * to go to.
 *
 * `idle` — a pane whose agent walked out of it — is the floor, under `done`.
 * It is the one state that says nothing happened, so a split tab where one
 * agent finished and the other was quit is a tab that reports the finish. */
const HOOK_RANK = { "needs-attention": 3, working: 2, done: 1, idle: 0 };

/* ---- the agents running inside a workspace card (1-eo) ----
 *
 * Orca draws these inside the card itself — a row per agent under the
 * workspace it is working in (`WorktreeCardAgents`,
 * WorktreeCard-D1AQ61B3.js:2976). Ours had the same facts and no surface for
 * them: the only way to see that an agent in another workspace was waiting was
 * to click into it.
 *
 * NOTHING IS FETCHED FOR THIS. Every row is built from state this window
 * already holds because it is already drawing it somewhere else: `hookStates`
 * (what the agent last said), `paneSessions` (which agent, which
 * conversation), the tabs, and `paneParents` above. Orca's own list is
 * push-only too — an IPC subscription plus one startup snapshot, no poll
 * (`window.api.agentStatus.onSet`, App-BaqTRjaA.js:5259) — and the only timers
 * anywhere near it are for relative time labels. A poll here would be a poll
 * per workspace per tick for facts that arrive on an event anyway.
 *
 * Who opened whom, by shell id. */
const paneParents = new Map();

/* And who is running INSIDE one — the helpers an agent spawns in its own
 * process, by shell id.
 *
 * A different kind of child from `paneParents`. A teammate is a real pty and
 * gets a pane, because the coordinator asked tmux for one; a background helper
 * never calls tmux at all, so there is no shell to split and never was. Orca
 * draws exactly these as rows too rather than as panes — its splits also come
 * only from teams and its fake tmux — which is why the fix here is a row and
 * not a pane.
 *
 * They arrived and were dropped: `hook_state` answers nothing for
 * `SubagentStart` on purpose (a helper starting must not repaint its parent's
 * pane), so the event fell out of the loop that forwards hooks and reached no
 * surface at all. Somebody watching five workers spin up saw none of them.
 *
 * Push-only, like everything else on this road: `hook:subagent` carries the
 * WHOLE list for a pane on every change, so this map is replaced rather than
 * patched and cannot drift; `pane_subagents` seeds a window that opened after
 * the helpers did. No timer anywhere. */
const paneSubagents = new Map();

/* 한 신원, 한 행(t-3024) — 어느 판이 어느 헬퍼인가, 셸 id → 헬퍼 id.
 *
 * zo 판 레인의 서브에이전트는 위의 두 길로 **둘 다** 온다: zo가 SubagentStart
 * 훅을 쏘아 `paneSubagents`에 헬퍼 행이 서고, 같은 레인이 tmux split-window로
 * 판을 갈라 `paneParents`에 자식 판이 선다. 둘을 잇는 것이 없어 같은 헬퍼가
 * 두 줄로 그려졌다(사용자 스크린샷 09-07 18:40: `code-reviewer·…`와 `ZO
 * claude-fable-5…`). 이제 split이 제 헬퍼 id를 `-e ZO_AGENT_ID=<id>`로 싣고,
 * `term:split`이 그 id를 `helper`로 나른다 — 이 지도가 그것이다.
 * `worktreeAgentRows`가 판 행을 표면으로 삼아 헬퍼 행을 접는다.
 *
 * 시드는 없다 — `paneParents`와 같은 사정이다: 그 지도도 `term:split`·
 * `term:worker`로만 채워지고 재시작 뒤 되살리는 손이 없다. 그래서 창을 다시
 * 연 뒤에는 헬퍼 id도 없다. 그때 접힘은 **두 행으로 물러날 뿐 0으로 가지
 * 않는다**: 훅 행은 훅 행대로, 판 행은 판 행대로 오늘처럼 선다. 판이 끝나면
 * `dropTermView`가 지운다. */
const paneHelpers = new Map();
/* The workers whose pane the placement seat answered for and whose label is
 * still open (t-5806): term → { worker }. Filled when the door says the
 * answer is in its book (`placed`), emptied by the first move this surface
 * reports of that pane or by the pane ending — a second move of the same
 * pane is a fact about the person's afternoon, not about the answer. */
const placedWorkers = new Map();

/* The pane that IS this helper, or `null` — the map above read the other way.
 * Asked by a roster row's click: a helper whose seat named it has a pane to go
 * to even before that pane has spoken and the two rows have folded. */
function paneOfHelper(id) {
  if (typeof id !== "string" || id.length === 0) return null;
  for (const [term, helper] of paneHelpers) {
    if (helper === id) return term;
  }
  return null;
}

/* Seed it. Once, at boot, from state the backend kept while nobody was
 * listening — the same argument `pane_sessions` is reseeded on, and the reason
 * a popped-out board is not blind to helpers that started before it opened. */
function seedSubagents() {
  return invoke("pane_subagents")
    .then((rows) => {
      for (const row of rows ?? []) {
        if (row.rows?.length) paneSubagents.set(row.term, row.rows);
      }
    })
    .catch(() => {});
}

/* ---- 그리고 그 에이전트가 지금 무엇을 하고 있는가 (1-ey) -----------------
 *
 * `hookStates`는 셋 중 하나를 말한다 — 작업 중 / 물어보는 중 / 끝. 다섯이
 * 동시에 "작업 중"일 때 그 다섯이 각각 무엇을 하는지는 아무 데도 없었다.
 * 이벤트에는 처음부터 들어 있었다: `PreToolUse`에는 도구 이름과 인자가 실려
 * 오고, 분류가 끝난 자리에서 그대로 버려지고 있었을 뿐이다.
 *
 * 카드 이름(`term:3`, `sub:3:a-1`)으로 키를 잡는다 — 백엔드가 그 이름으로
 * 보내오고, 보드의 카드도 같은 이름을 쓴다. 프로세스 안의 헬퍼는 자기 판이
 * 없으므로 판 번호로는 부모의 일과 제 일을 가를 수 없다.
 *
 * 백엔드와 같은 스무 개로 끊는다. 창의 지도는 백엔드 링의 사본이고, 사본이
 * 원본보다 길게 자라는 것은 그냥 새는 것이다. */
const ACTIVITY_RING = 20;
const paneActivities = new Map();

/* Seed it, once, for the same reason the helpers are seeded: this window may
 * have opened an hour into the work, and waiting for the next event means
 * waiting for an agent to do something new. One round trip for every card —
 * the command answers for all of them when it is asked for none. */
function seedActivities() {
  // `pane: null` spelled out, the way every other optional argument in this
  // window is (`repo_trust_standing`): the backend's argument is an `Option`,
  // and a key that is present and null cannot be mistaken for a key the
  // window forgot to send.
  return invoke("pane_activities", { pane: null })
    .then((cards) => {
      for (const card of cards ?? []) {
        if (card?.pane && card.activities?.length) paneActivities.set(card.pane, card.activities);
      }
    })
    .catch(() => {});
}

/* The newest thing one card did, or nothing. */
function newestActivity(pane) {
  const held = paneActivities.get(pane);
  return held?.length ? held[held.length - 1].activity : null;
}

/* One activity as a row reads it: a word for the verb, then what it is on.
 *
 * The verb is translated and the target never is — a path, a command and a
 * query are the machine's own words, and "실행 · cargo test --workspace" is
 * the line a person supervising five agents actually reads. A verb this
 * window has no word for (an MCP tool nobody here has heard of) draws the
 * vendor's own name, which is better than a category invented for it. */
function activityWord(verb) {
  switch (verb) {
    case "read":
      return t("activity.read", "읽기");
    case "edit":
      return t("activity.edit", "고치기");
    case "write":
      return t("activity.write", "쓰기");
    case "bash":
      return t("activity.bash", "실행");
    case "grep":
      return t("activity.grep", "찾기");
    case "task":
      return t("activity.task", "맡기기");
    case "web":
      return t("activity.web", "웹");
    case "prompt":
      return t("activity.prompt", "물음");
    case "stop":
      return t("activity.stop", "끝");
    default:
      return verb;
  }
}

function activityLine(pane) {
  const activity = newestActivity(pane);
  if (!activity) return "";
  const word = activityWord(activity.verb);
  return activity.target ? `${word} · ${activity.target}` : word;
}

/* 헬퍼가 지금까지 집어 든 도구의 수, 낱말로 — Claude Code가 도는 Task 줄에
 * 다는 그 수("12 tool uses"). 세는 것은 백엔드고(`SubagentRow.tool_calls`,
 * zo는 제 프레임에서, 나머지는 헬퍼 카드에 쌓이는 도구 호출에서), 이 줄은
 * 그 한 필드를 낱말로 바꾼다 — 벤더가 달라도 창이 보는 곳은 하나다.
 *
 * 0은 낱말이 아니다: 아직 아무것도 집어 들지 않은 헬퍼에게 「0 tool uses」를
 * 다는 것은 빈칸을 낱말로 채우는 일이고, 행에는 그럴 자리가 없다.
 *
 * 단수는 카탈로그가 정한다 — 열쇠 하나에 `{{s}}` 자리를 두고, 그 자리를 쓰는
 * 언어(영어·스페인어)만 쓴다. 「1 tool use」와 「1 uso de herramienta」가
 * 그렇게 서고, 수를 세지 않는 언어의 문장은 그 자리를 아예 담지 않는다. */
function toolUsesWords(count) {
  if (!(count > 0)) return "";
  return t("agent.toolUses", "도구 {{count}}회", { count, s: count === 1 ? "" : "s" });
}

listen("hook:activity", (event) => {
  const { pane, activities } = event.payload ?? {};
  if (!pane || !activities?.length) return;
  agentGraphHotKey = agentGraphAgentKey(pane);
  // 배치로 온다 — 백엔드가 카드마다 100ms에 한 번만 보내고, 그 사이의 것을
  // 모아서 함께 싣는다. 이어 붙이고 스무 개에서 끊는다.
  const held = [...(paneActivities.get(pane) ?? []), ...activities];
  paneActivities.set(pane, held.slice(-ACTIVITY_RING));
  // A tool call is a line in the transcript already; the conversation view on
  // this pane, if it is up, reads it now (`pollHelperPages` is the one reader).
  const term = Number(/^term:(\d+)$/.exec(pane)?.[1]);
  if (Number.isInteger(term) && paneChatOn(term) && activeHelperPage()?.worker.term === term) {
    void pollHelperPages();
  }
  // Sidebar rows and graph nodes both show the current activity. The shared
  // paint coalescer still asks the backend once; keyed graph nodes update only
  // this line and reuse the unchanged topology and edges.
  scheduleAgentPaint(["cards", "board"]);
});

/* 손으로 켠 에이전트가 판에 도착했거나 떠났다.
 *
 * 실행 기록도 훅도 이 사실을 말하지 않는다 — 커널만 안다(백엔드
 * `sweep_arrived_agents`). 그래서 이 한 줄이 온다: 명부가 바뀌었고, 그 명부를
 * 읽는 표면들이 다시 그려야 한다는 말. 무엇이 바뀌었는지는 싣지 않는다 —
 * 각 표면은 이미 자기 몫을 명부에서 통째로 읽고, 조각을 실어 보내는 순간
 * 같은 사실의 두 번째 사본이 생긴다. */
listen("agents:changed", () => {
  void refreshPaneLedger();
  scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
});

listen("hook:subagent", (event) => {
  const { term, rows } = event.payload ?? {};
  if (typeof term !== "number") return;
  // The whole list every time, so a missed event corrects itself on the next
  // one. An empty list is the clear — the last helper stopping is a pane with
  // no rows, not a pane with a stale one.
  if (rows?.length) paneSubagents.set(term, rows);
  else paneSubagents.delete(term);
  syncHelperPagesWith(term, rows ?? []);
  // The composer's agents pill counts this roster; it is the only thing on
  // the page that does, so one line repaints it.
  paintComposerChipsFor(term);
  // The tab strip is NOT repainted: a helper is not its pane's state, and the
  // badge on the tab says what the pane is doing. Repainting it here would be
  // this window drawing the very confusion the backend refuses to create.
  // The door's badge is not recomputed either: it counts the attention column,
  // and a helper is never in it — every subagent row is a thing that is
  // running. A round trip that cannot change its answer is a round trip per
  // helper per spawn, on the road that spawns five at a time.
  //
  // 예약으로 지나간다. 다섯을 한꺼번에 띄우는 길이 바로 이 길이고, 그 다섯이
  // 각각 카드를 다시 그릴 이유는 없다 — 형제들이 함께 도착하면 그림은 한 번이다.
  scheduleAgentPaint(["cards", "board"]);
});

/* The agents to draw under one workspace, parents first and children under
 * them.
 *
 * Depth follows the actual generation: a child is one level below its parent,
 * and a grandchild is two. The stylesheet caps the visual indent for a narrow
 * column, while the row model keeps the real depth so the parent/child chain
 * remains unambiguous even after the visual cap. */
function worktreeAgentRows(path) {
  const here = [];
  for (const tab of tabs) {
    if (tab.kind !== "term" || tab.worktree !== path) continue;
    for (const term of paneLeaves(tab.layout)) {
      const state = hookStates.get(term);
      const session = paneSessions.get(term);
      // An agent that has never said anything is a shell, not an agent. This
      // is also the empty state: a workspace with no agents draws no rows and
      // no placeholder, exactly as Orca's card does (`if (agents.length === 0)
      // return null`).
      //
      // A pane with helpers running counts even if it has said nothing else —
      // spawning five workers IS the agent speaking, and the row it needs is
      // the one those five hang under.
      //
      // And a pane the LAUNCH LEDGER names counts before it ever speaks —
      // the board's own rule (`pane_agents`, main.rs: a live pty we launched
      // as an agent is doing something; only a dead one may rest). A vendor
      // with no hook wired — zo has none, codex's slot on this machine is
      // held by the real Orca — otherwise ran invisibly, which is the exact
      // situation these rows exist to show ("지금 프로젝트에 모델들이 안 떠,
      // claude만 뜨는데"). Liveness is the tab itself: an exited pane is
      // pruned from its layout by `term:exited`, so a pane still here runs.
      const launched = paneAgents.has(term);
      if (!state && !session && !paneSubagents.has(term) && !launched) continue;
      // The face never waits for the session hook. The launch ledger knows
      // the agent from the spawn (`agent_terms` seeds `paneAgents`), and
      // Orca's row draws its identity icon from the row model itself, not
      // from anything said later (`AgentIcon
      // agent={agentTypeToIconAgent(agent.agentType)}` size 14,
      // WorktreeCard-B91w6t_B.js) — a row that showed a dot and a name but
      // no face is how "왼쪽 네비바에 에이전트 아이콘이 보이는데 우리는
      // 그렇지 않아" was reported.
      here.push({
        term,
        tab,
        worktree: tab.worktree,
        // A launched agent that has not spoken yet is WORKING, not resting —
        // the same verdict the board hands a hookless live pty. The rows that
        // qualified by session or helpers keep their settled word.
        state: autonomousPaneState(term, state ?? (session || paneSubagents.has(term) ? "done" : "working")),
        agent: session?.agent ?? paneAgents.get(term) ?? null,
      });
    }
  }
  // 판을 닫아도 돌고 있는 에이전트. 탭이 없으므로 위의 훑기가 볼 수 없고,
  // 그래서 이 줄이 그 에이전트에게 남은 유일한 문이다 — 누르면 다시 붙는다.
  for (const [term, worktree] of detachedAgents) {
    if (worktree !== path) continue;
    const session = paneSessions.get(term);
    here.push({
      term,
      tab: null,
      worktree,
      detached: true,
      state: autonomousPaneState(term, hookStates.get(term) ?? (session || paneSubagents.has(term) ? "done" : "working")),
      agent: session?.agent ?? paneAgents.get(term) ?? null,
    });
  }
  const held = new Set(here.map((row) => row.term));
  const ordered = [];
  const seen = new Set();
  // Whether anything under this row — a helper still at work, a child pane
  // at work or asking — is live. Asked of the WHOLE subtree, not the root:
  // a coordinator resting at `done` with a worker still going is the case a
  // fold must never hide, and the case the summary must never fold away.
  const liveBelow = (row, visiting = new Set()) => {
    if (visiting.has(row.term)) return false;
    visiting.add(row.term);
    if ((paneSubagents.get(row.term) ?? []).some((sub) => sub.state !== "done")) return true;
    return here.some(
      (child) =>
        paneParents.get(child.term) === row.term &&
        (LIVE_HOOK_STATES.has(child.state) || liveBelow(child, visiting)),
    );
  };
  // The whole subtree under one root, carrying the generation depth as it is
  // walked. The CSS keeps the visible indent within a narrow column, but the
  // model must retain the distinction between a child and a grandchild.
  const walk = (row, depth, buried = false) => {
    if (seen.has(row.term)) return;
    seen.add(row.term);
    const children = here.filter((child) => paneParents.get(child.term) === row.term);
    // 한 신원, 한 행(t-3024). zo 판 레인의 헬퍼는 두 길로 온다 — 훅이 세운
    // 헬퍼 행(`paneSubagents`)과 split이 세운 판 행(`paneParents`) — 그리고
    // `term:split`이 실어 온 헬퍼 id(`paneHelpers`)가 둘을 잇는다. 판 행이
    // 표면이다: 그 헬퍼 행은 세우지 않고, 판 행이 헬퍼의 이름과 전사 손을
    // 이어받는다(`sub`를 달되, 손이 여는 명부의 주인은 `subHost`). 판이
    // 떠나면(`term:exited`) 접힘도 풀려, 끝난 헬퍼는 오늘처럼 완료 이력으로
    // 간다. id가 명부에 없거나 판이 아직 행이 아니면 접지 않는다 — 두 행으로
    // 물러날지언정 0으로 가지 않는다.
    const folded = new Map();
    for (const child of children) {
      const helper = paneHelpers.get(child.term);
      if (helper !== undefined) folded.set(helper, child);
    }
    // 이 행 밑에 몇이 사는가 — 셰브론이 세는 그 수(실측: childAgentCount).
    // 접힌 한 쌍은 판으로 한 번만 센다.
    const subs = (paneSubagents.get(row.term) ?? []).filter((sub) => !folded.has(sub.id));
    const kids = subs.length + children.length;
    // 접힌 부모의 후손은 화면에서 빠지되 소비된다(seen) — 고아 패스가 되살려
    // 루트인 척 세우는 길을 막는다. 기본은 펼침(실측:
    // `expanded = !collapsedLineageParents.has(paneKey)`) — 접은 손만 기억한다.
    //
    // 빠지는 것은 정착한 것뿐이다. 살아 있는 후손 — 일하거나 묻는 자식, 도는
    // 헬퍼 — 은 접힌 부모 밑에서도 선다(설계: navigator-active-history). 그
    // 사이의 정착한 조상은 맥락으로 함께 선다(`ancestry`): 들여쓰기만 남은
    // 고아 행이 아니라, 누구 밑에서 도는지 읽히게.
    const shown = !agentFolded.has(row.term);
    const live = LIVE_HOOK_STATES.has(row.state) || liveBelow(row);
    if (!buried) ordered.push({ ...row, depth, kids, kidsShown: shown });
    else if (live) {
      ordered.push({
        ...row, depth, kids, kidsShown: shown,
        ancestry: !LIVE_HOOK_STATES.has(row.state),
      });
    }
    const hideBelow = buried || (kids > 0 && !shown);
    // The helpers running inside THIS agent, immediately under it. They carry
    // the parent's tab and term on purpose: a helper has no pane of its own,
    // so clicking its row goes where its work is being written.
    //
    // 도는 헬퍼는 접힘과 무관하게 선다. 끝난 헬퍼는 기본으로 한 줄 「완료
    // N개」로 접히고(정확히 그 수), 펼치면 각각의 행이 전사 손을 달고 선다 —
    // 접기는 지우기가 아니다: 행도 전사도 명부(`paneSubagents`)에 그대로다.
    const running = subs.filter((sub) => sub.state !== "done");
    const done = subs.filter((sub) => sub.state === "done");
    for (const sub of running) {
      ordered.push({ ...row, depth: depth + 1, sub, kids: 0, kidsShown: false });
    }
    if (!hideBelow && done.length > 0) {
      const historyShown = agentHistoryShown.has(row.term);
      if (historyShown) {
        for (const sub of done) {
          ordered.push({ ...row, depth: depth + 1, sub, kids: 0, kidsShown: false });
        }
      }
      ordered.push({
        ...row, depth: depth + 1, sub: null, kids: 0, kidsShown: false,
        history: done.length, historyShown,
      });
    }
    for (let child of children) {
      const wearing = (paneSubagents.get(row.term) ?? []).find(
        (sub) => sub.id === paneHelpers.get(child.term),
      );
      // 접어 입은 판은 헬퍼의 이름과 손을 달고 내려간다(t-3024).
      if (wearing) child = { ...child, sub: wearing, subHost: row.term };
      walk(child, depth + 1, hideBelow);
    }
  };
  for (const row of here) {
    const parent = paneParents.get(row.term);
    if (parent !== undefined && held.has(parent)) continue;
    walk(row, 0);
  }
  // A child whose parent is in ANOTHER workspace — or in a cycle — still
  // belongs on screen. Dropping it would lose an agent because of where its
  // coordinator happens to sit, and `seen` is what keeps this from drawing one
  // twice.
  for (const row of here) walk(row, 0);
  return ordered;
}

/* What those rows SAY, as one string — what the repaint compares to decide
 * whether anything moved.
 *
 * The sidebar's shape guard used to read this too, through a wrapper that has
 * gone with it: what an agent is DOING is not the shape of the list, and a
 * guard that carried it rebuilt every project header and every listener under
 * them on every turn of every agent — undoing exactly what the guard was for. */
function agentRowsSaid(rows) {
  return rows
    .map(
      (row) =>
        `${row.term}:${row.state}:${row.agent ?? ""}:${row.depth}:${row.sub?.id ?? ""}` +
        // 접힌 이력의 수와 펼침, 맥락으로만 선 조상, 원장의 말도 행이 하는 말이다.
        `:${row.history ?? 0}:${row.historyShown ? 1 : 0}:${row.ancestry ? 1 : 0}` +
        `:${row.sub ? "" : paneLedgerWord(row.term)}:${row.sub ? "" : (paneLedger.get(row.term)?.task ?? "")}` +
        // 헬퍼가 쓴 도구의 수도 행이 하는 말이다 — 같은 도구를 계속 쓰는
        // 헬퍼는 activity 줄이 그대로여도 이 수가 오르고, 이것이 빠져 있으면
        // 서명이 같아서 그 행만 옛 수에 굳는다.
        `:${row.sub?.tool_calls ?? 0}` +
        // 접힘과 이름도 행이 하는 말이다 — 셰브론이 돌거나 터미널이 제목을
        // 말하면 그 행은 다시 서야 한다.
        `:${row.kids ?? 0}:${row.kidsShown ? 1 : 0}:${agentConversationName(row) ?? ""}` +
        // 무엇을 하고 있는지가 바뀌면 상태가 그대로여도 행은 다시 그려야 한다.
        // 이것이 빠져 있으면 도구가 바뀌어도 서명이 같아서 카드가 첫 줄에 멈춘다.
        // 시계도 행이 하는 말의 일부다 — "now"가 "1m"으로 넘어가는 순간에는
        // 상태도 도구도 그대로이므로, 낱말을 여기 적어 두지 않으면 서명이 같아서
        // 30초 박자가 두드려도 그 행만 옛 낱말에 굳는다.
        `:${activityLine(agentRowPane(row))}:${agentRowAgo(row)}` +
        // 사다리의 나머지 재료와 무대 위 여부 — 프롬프트·마지막 말·모델이
        // 바뀌거나 이 판이 무대에 서고 내릴 때, 상태는 그대로여도 행은 말을
        // 바꾼다. 원료를 적는다(사다리를 두 번 오르지 않기 위해).
        `:${panePrompts.get(row.term) ?? ""}:${paneSaid.get(row.term) ?? ""}` +
        `:${paneModels.get(row.term) ?? ""}:${restoredWorkers.get(row.term) ?? ""}` +
        `:${row.sub && row.subHost === undefined ? "" : JSON.stringify(paneAutonomyValue(row.term))}` +
        `:${agentRowHere(row) ? 1 : 0}`,
    )
    .join(",");
}

/* Which card a row IS, in the name the backend files activities under.
 *
 * A helper has no pane of its own — its row carries its parent's term — so the
 * two are told apart by the helper's own id, exactly as the board's cards are
 * (`sub:${pane.term}:${sub.id}`). One spelling, here, because a row that asked
 * under the wrong name would silently draw its parent's work as its own. */
function agentRowPane(row) {
  // 헬퍼를 접어 입은 판(t-3024)의 활동은 제 판의 것.
  if (row.sub && row.subHost === undefined) return `sub:${row.term}:${row.sub.id}`;
  return `term:${row.term}`;
}

/* 행이 툴팁으로 하는 말 — 원본의 title(`${primary}${secondary ? ` - ${secondary}` : ''}`)에
 * 헬퍼의 도구 수를 이어 붙인 것.
 *
 * 행은 좁아서 activity 줄을 말줄임으로 자르지만 툴팁은 자르지 않는다: 잘린
 * 나머지를 읽을 곳이 여기 하나뿐이다. 세우는 길(`makeAgentRow`)과 갈아입히는
 * 길(`redressAgentRows`)이 같은 문장을 두 번 적으면, 다시 그리지 않은 행만
 * 다른 말을 하게 된다. */
function agentRowTip(primary, secondary, uses) {
  return `${primary}${secondary ? ` - ${secondary}` : ""}${uses ? ` · ${uses}` : ""}`;
}

/* What a row READS AS — which is not always what its state says.
 *
 * A helper's row wears its OWN state, not its parent's: a coordinator sitting
 * at `done` with five workers still going is the case that had no picture at
 * all, and a worker that finished while its siblings run on stays as a
 * finished row (the backend keeps it, greyed) rather than vanishing — five in
 * parallel used to disappear one by one, each taking the page its transcript
 * opens from. Spelled once because the shape, the words and the clock all
 * have to agree about it. */
function agentRowState(row) {
  if (row.history) return "done";
  // 헬퍼를 접어 입은 판(t-3024)은 판이다 — 상태는 제 훅의 것.
  if (!row.sub || row.subHost !== undefined) return autonomousPaneState(row.term, row.state);
  return row.sub.state === "done" ? "done" : "working";
}

/* Orca's `formatShortTimeAgo` (worktree-card-compact-agent-row.tsx:16-30) —
 * the compact row's OWN clock vocabulary, narrower than the board's `agoWord`
 * on purpose: the original ships both, and this column is ten pixels of
 * tabular digits at the row's right edge. Unlocalized like the original —
 * "3m" is a unit, not prose, and it reads the same in all four catalogues. */
function shortAgo(at, now) {
  const delta = now - at;
  if (delta < 60_000) return "now";
  const minutes = Math.floor(delta / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/* The moment a row's clock counts from. Orca's CompactAgentRow shows elapsed
 * on EVERY row, counted from the moment it last entered `done` for a finished
 * one and from the session's start for anything else. `paneBorn` is that
 * start; the state ledger stands in for a shell this window never drew a view
 * for, and `now` for one that has neither.
 *
 * Named on its own because the card's summary picks its one row by this
 * number, and the row it keeps has to be the row the eye reads as the last
 * one — two clocks would let the card hold a row printing `2d` while a `now`
 * sat in the hidden half. */
function agentRowAt(row, now = Date.now()) {
  return agentRowState(row) === "done"
    ? (hookStamps.get(row.term) ?? paneBorn.get(row.term) ?? now)
    : (paneBorn.get(row.term) ?? hookStamps.get(row.term) ?? now);
}

/* And how long it has been at it. */
function agentRowAgo(row) {
  const now = Date.now();
  return shortAgo(agentRowAt(row, now), now);
}

/* The one-line card summary keeps one session, the same limit for live and
 * paneless cards. A named constant keeps that rule from becoming two magic
 * slices in the two rendering branches. */
const WORKTREE_SUMMARY_GROUP_LIMIT = 1;

/* Fold groups for a card in one place. The summary folds away what is OVER,
 * never what is running — a card that hid a working agent behind the twist
 * made the sidebar deny work in flight ("main에서 작업중인게 다 표시 안됨,
 * 터미널을 클릭해야 보임"). So a group stands when it is live (working, or
 * waiting on a person — a question nobody can see is a question nobody
 * answers), when it holds the pane on stage (this row is also the row that
 * fills, and the pane being typed in must not live in the hidden half), or
 * when it is the last of the rest — the one-line summary the resting card
 * exists for. */
function compactWorktreeGroups(groups) {
  if (groups.length <= WORKTREE_SUMMARY_GROUP_LIMIT) return groups;
  const showing = groups.filter((group) => group.live || group.staged);
  if (showing.length > 0) return showing;
  // `born` is the session's own start/last-state clock. A tie keeps the later
  // group in the list, matching the old `>=` rule for rows with no clock.
  const keepLast = groups.reduce((held, group) => (group.born >= held.born ? group : held));
  return [keepLast];
}

/* Apply the shared fold and return its count at the same time. Keeping the
 * count beside the rows is what makes the twist and the DOM see precisely the
 * same cut, including the paneless restart path. `open` and `full` both mean
 * that this card must show every group. */
function summarizeWorktreeGroups(groups, { open = false, full = false } = {}) {
  const all = groups.flatMap((group) => group.members);
  const visibleGroups = open || full ? groups : compactWorktreeGroups(groups);
  const rows = visibleGroups.flatMap((group) => group.members);
  return { rows, hidden: all.length - rows.length, total: all.length };
}

/* What a card shows while it is not opened out: every agent still AT WORK,
 * and of the finished ones only the last.
 *
 * The summary folds away what is OVER, never what is running — a card that
 * hid a working agent behind the twist made the sidebar deny work in flight
 * ("main에서 작업중인게 다 표시 안됨, 터미널을 클릭해야 보임"). So a group
 * stands when it is live (working, or waiting on a person — a question
 * nobody can see is a question nobody answers), when it holds the pane on
 * stage (this row is also the row that fills, and the pane being typed in
 * must not live in the hidden half), or when it is the last of the rest —
 * the one-line summary the resting card exists for.
 *
 * Only ROOTS are candidates. A helper, and an agent a coordinator opened, is
 * part of what that root is doing rather than another session of this
 * workspace — and a group is contiguous in this list, because `walk` pushes a
 * root and then its descendants before it reaches the next root. */
function summarizeAgentRows(rows, options = {}) {
  const now = Date.now();
  const groups = [];
  for (let seat = 0; seat < rows.length; seat += 1) {
    if (rows[seat].depth !== 0) continue;
    let end = seat + 1;
    while (end < rows.length && rows[end].depth !== 0) end += 1;
    const members = rows.slice(seat, end);
    const state = agentRowState(rows[seat]);
    groups.push({
      members,
      // 판이 살아 있으면 도는 것이다. 훅의 done은 "이 턴이 끝났다"지 "이
      // 일이 끝났다"가 아니고, 턴 사이의 Codex를 접은 카드가 "열면 옆에
      // 에이전트 표시가 사라짐"으로 보고됐다 — 접어도 되는 것은 판이 이미
      // 닫힌(detached) 에이전트뿐이다.
      // 구성원 전체를 본다: 부모가 detached+done이어도 그 밑에 묻는 자식이
      // 있으면 그 그룹은 산 것이고, 접힌 카드가 그 물음을 숨기면 아무도 답하지
      // 않는다(설계: navigator-active-history, Fable 감사 B1).
      live:
        LIVE_HOOK_STATES.has(state) ||
        members.some((row) => LIVE_HOOK_STATES.has(agentRowState(row))) ||
        !rows[seat].detached,
      staged: members.some((row) => agentRowHere(row)),
      born: agentRowAt(rows[seat], now),
    });
  }
  // A malformed tree can contain rows without a root. Keep those rows in the
  // same summary machinery rather than growing a second fold rule for them.
  if (groups.length === 0) {
    groups.push(...rows.map((row) => ({
      members: [row],
      live: false,
      staged: false,
      born: agentRowAt(row, now),
    })));
  }
  return summarizeWorktreeGroups(groups, options);
}

/* Keep the original row-only reader for callers that only need the compact
 * list. The painter uses the summary object so its hidden count comes from the
 * exact same fold. */
function latestAgentRows(rows) {
  return summarizeAgentRows(rows).rows;
}

/* Orca's tool-preview rule (`agent-row-tool-preview.ts`), whole: the tool
 * fields outlive the turn that set them, so on any state but the two LIVE
 * ones they read as work still in flight — a lie the original refuses on all
 * three of its surfaces at once. `working` names the tool the agent is
 * running; `needs-attention` (their `waiting`) names the tool a permission
 * request is blocked on. The line itself is `activityLine`, the one reader
 * of the activity ring — this function adds only the gate. */
function agentToolPreview(pane, state) {
  if (state !== "working" && state !== "needs-attention") return "";
  return activityLine(pane);
}

/* The row's first words — Orca's `getCompactAgentPrimary`: the conversation's
 * own name, else the live prompt, else the state said as a word. The vendor's
 * name is NOT here — identity is the face icon's job, and the type label is
 * the ladder's last SECONDARY rung. A helper's first words are its name. */
function agentRowPrimary(row, state) {
  if (row.history) {
    return t("board.historyCount", "완료 {{count}}개", { count: row.history });
  }
  if (row.sub) return row.sub.name;
  // 과업명이 첫 자리다(설계: navigator-active-history — 「Claude」가 여럿
  // 서는 목록은 목록이 아니다). 원장이 이 판에 과업을 앉혔으면 그 제목,
  // 아니면 대화명 → 프롬프트 → 상태어의 사다리 그대로.
  const task = paneLedger.get(row.term)?.task?.trim();
  if (task) return task;
  return (
    agentConversationName(row)
    ?? (panePrompts.get(row.term)?.trim() || null)
    ?? bucketWord(state === "needs-attention" ? "attention" : state)
  );
}

/* And the words after the dash — Orca's `getCompactAgentSecondary` ladder:
 * the tool preview while it is live, else the last assistant words, else the
 * type label. (`Interrupted by user` heads the original's ladder; this
 * window's hook vocabulary has no such word to report, so that rung has no
 * producer here.) The duplicate guard is the original's own :52-55 — a
 * repeated primary adds no information — generalized past helper rows,
 * because a vendor label under a vendor-labelled primary lies the same way. */
function agentRowSecondary(row, state, primary) {
  const restored = row.sub ? null : restoredWorkers.get(row.term);
  if (restored === "session") return t("board.restored", "이어서");
  if (restored === "fresh") return t("board.restoredFresh", "이어서 (새 대화)");
  const autonomy = !row.history && (!row.sub || row.subHost !== undefined) && hookStates.get(row.term) !== "needs-attention"
    ? agentAutonomyPhrase(paneAutonomyValue(row.term)) : null;
  if (autonomy && (!autonomyRunning(paneAutonomyValue(row.term)) || state === "working")) return autonomy.word;
  const preview = agentToolPreview(agentRowPane(row), state);
  if (preview) return preview;
  // 원장의 말(검증 대기/검증됨/병합됨)은 이제 상태 칸의 것이다
  // (`agentRowStatusWord`) — 같은 낱말을 문장에서 되풀이하지 않는다.
  const said = row.sub ? "" : (paneSaid.get(row.term) ?? "").trim();
  if (said) return said;
  // 공급자명은 둘째 자리 — 과업명이 첫 자리를 차지했을 때 비로소 정체성이
  // 여기 선다.
  const label = row.sub ? row.sub.name : row.agent ? agentName(row.agent) : "";
  return label === primary ? "" : label;
}

/* The row's status, said in one short word at the row's right edge — the same
 * place on every row, so the eye reads state down one column instead of
 * hunting for it inside the sentence. The ledger's word first on a settled
 * seat (검증 대기·검증됨·병합됨·배포됨): a turn that ended is not work that was
 * verified, and the hook's 완료 alone had made the two look the same. The
 * words are the board's own (`bucketWord`, `paneLedgerWord`) — no second
 * vocabulary for one fact. */
function agentRowStatusWord(row, state) {
  if (row.history) return "";
  const ledgerWord = row.sub ? "" : paneLedgerWord(row.term);
  if (ledgerWord && !LIVE_HOOK_STATES.has(state)) return ledgerWord;
  return bucketWord(state === "needs-attention" ? "attention" : state);
}

/* Whether the coordinator has stood behind this seat's work — verified,
 * merged or deployed in the ledger. Only then does the row's check turn
 * green; a turn that merely ended keeps a quiet check. */
function agentRowVerified(row) {
  if (row.sub || row.history) return false;
  const review = paneLedger.get(row.term)?.review;
  return Boolean(review && (review.verified || review.merged || review.deployed));
}

/* The task a workspace is working on, when the ledger seated one in a pane
 * here — the card's first words then, ahead of a branch named after a task id
 * (`wt/t-4238/…`). Roots only, in the order the card lists them. */
function worktreeTaskTitle(path) {
  for (const row of worktreeAgentRows(path)) {
    if (row.sub) continue;
    const task = paneLedger.get(row.term)?.task?.trim();
    if (task) return task;
  }
  return "";
}

/* Whether this row IS the pane on stage — Orca's `isFocusedPane`, which fills
 * the row and lifts its dimmed words to full ink. A helper never is: it has
 * no pane of its own, and its parent's focus is the parent row's to wear. */
function agentRowHere(row) {
  // 떼어 둔 행에는 탭이 없다 — 무대에 선 것일 수 없다. 아래 문장은 게이트가
  // 글자 그대로 고정한 것이라(`the_agent_row_speaks_orcas_compact_sentence`)
  // 그 안이 아니라 앞에서 막는다.
  if (!row.tab) return false;
  // 헬퍼를 접어 입은 판(t-3024)은 제 판이 무대에 서면 여기다 — 같은 이유로
  // 아래 문장 앞에서 답한다.
  if (row.sub && row.subHost !== undefined) {
    return row.tab.id === activeTabId && activePaneOf(row.tab) === row.term;
  }
  return !row.sub && row.tab.id === activeTabId && activePaneOf(row.tab) === row.term;
}

/* 어느 부모를 사람이 접었나 — 기본은 펼침(실측: collapsedLineageParents).
 * 카드가 몇 번을 다시 서도 남는 세션 기억이고, 창이 닫히면 함께 진다. */
const agentFolded = new Set();

/* 그리고 어느 부모의 끝난 이력을 사람이 펼쳤나 — 기본은 접힘. `agentFolded`와
 * 극이 반대라 따로 둔다: 하나는 접은 손을, 하나는 펼친 손을 기억한다. */
const agentHistoryShown = new Set();

/* Orca의 대화명 사다리(getAgentRowConversationName)를 이 창의 재료로:
 * 사람이 지은 창 이름 → 터미널이 말한 제목 — 앞머리 장식을 벗기고, 상태나
 * 경로 같은 「제목 아닌 제목」은 버린다(측정한 그 걸러냄의 축약 — 기록된
 * 이탈). 도우미 행은 제 이름이 곧 이름이다. */
function agentConversationName(row) {
  if (row.sub) return null;
  const named = row.tab ? paneTitleOf(row.tab, row.term) : null;
  if (named) return named;
  const spoken = (termTitles.get(row.term) ?? "").trim();
  if (!spoken) return null;
  const stripped = spoken.replace(/^(?:[✳.*]\s+|[\u2800-\u28FF]+\s*)/u, "").trim();
  if (!stripped) return null;
  // 에이전트의 제 이름·상태 제목("Claude Code — working")은 이름이 아니라
  // 정체성이고, 경로는 이름이 아니라 자리다.
  const agent = row.agent ? agentName(row.agent).toLowerCase() : "";
  const lower = stripped.toLowerCase();
  if (agent && (lower === agent || lower.startsWith(`${agent} `))) return null;
  if (stripped.startsWith("/") || stripped.startsWith("~")) return null;
  return stripped;
}

/* One agent's row inside a workspace card — Orca's CompactAgentRow, cell for
 * cell: [셰브런/빈 자리] [상태 점] [얼굴] [primary - secondary 한 줄 절단]
 * [모델 모노] [+N 접힌 자식] [경과]. (원본의 CacheTimer 칸은 캐시 TTL
 * 생산자가 없어 정직 공백.) */
/* The row's classes, from the three facts that pick them — one spelling, read
 * back by `redressAgentRows` to tell a row whose words moved from a row whose
 * shape did. */
function agentRowClasses(state, here, foldedSummary, row = null) {
  return `wt-agent is-${state === "needs-attention" ? "waiting" : state}${
    here ? " is-here" : ""
  }${foldedSummary ? " is-folded-summary" : ""}${row?.history ? " is-history" : ""}${
    row?.ancestry ? " is-ancestry" : ""
  }${!row?.history && LIVE_HOOK_STATES.has(state) ? " is-live" : ""}${
    row && agentRowVerified(row) ? " is-verified" : ""
  }`;
}

function agentHistoryLabel(row) {
  return row.historyShown
    ? t("board.historyHide", "완료 {{count}}개 접기", { count: row.history })
    : t("board.historyShow", "완료 {{count}}개 펼치기", { count: row.history });
}

function agentFoldLabel(row) {
  return row.kidsShown
    ? t("board.kidsHide", "자식 에이전트 {{count}}개 숨기기", { count: row.kids })
    : t("board.kidsShow", "자식 에이전트 {{count}}개 표시", { count: row.kids });
}

/* What a folded summary row says: identity, the state's word, and the
 * disclosure — the tip carries the first two, the label all three. */
function foldedSummaryWords(row, state, fold) {
  const stateWord = bucketWord(state === "needs-attention" ? "attention" : state);
  const identity = row.agent ? agentName(row.agent) : stateWord;
  const disclosure = fold?.getAttribute("aria-label") ?? "";
  return {
    tip: `${identity} · ${stateWord}`,
    label: [identity, stateWord, disclosure].filter(Boolean).join(" · "),
  };
}

/* Resolve one standing compact row into the words its existing leaves wear.
 * This is deliberately the same shape check used by both painters below: a
 * title-only write must never guess that a missing span can be manufactured
 * in place, and a full card redress must make the same decision. */
function fittedAgentRowWords(node, row) {
  const state = agentRowState(row);
  const foldedSummary = row.kids > 0 && !row.kidsShown;
  if (
    node.dataset.term !== String(row.term) ||
    (node.dataset.sub ?? "") !== (row.sub?.id ?? "") ||
    (node.dataset.agent ?? "") !== (row.agent ?? "") ||
    (node.dataset.history ?? "") !== (row.history ? String(row.history) : "") ||
    node.className !== agentRowClasses(state, agentRowHere(row), foldedSummary, row)
  ) return null;
  // 이력 한 줄은 모양이 곧 말이다 — 수나 펼침이 바뀌면 다시 세운다.
  if (row.history) {
    const fold = node.querySelector(":scope > .wt-agent-fold");
    if (fold?.getAttribute("aria-expanded") !== String(Boolean(row.historyShown))) return null;
    return { node, row, state, foldedSummary: false, fold: null, history: true };
  }
  const fold = node.querySelector(":scope > .wt-agent-fold:not(.is-quiet)");
  if ((row.kids > 0) !== (fold !== null)) return null;
  if (fold !== null && fold.getAttribute("aria-expanded") !== String(row.kidsShown)) return null;
  const fit = { node, row, state, foldedSummary, fold };
  if (!foldedSummary) {
    fit.primary = agentRowPrimary(row, state);
    fit.secondary = agentRowSecondary(row, state, fit.primary);
    fit.name = node.querySelector(".wt-agent-name");
    fit.said = node.querySelector(".wt-agent-said");
    if (fit.name === null || Boolean(fit.secondary) !== (fit.said !== null)) return null;
    fit.model = row.sub ? null : paneModels.get(row.term);
    fit.wears = node.querySelector(".wt-agent-model");
    if (Boolean(fit.model) !== (fit.wears !== null)) return null;
    // 도구 수가 처음 서거나 사라지는 것은 낱말이 아니라 모양이다 — 첫 도구를
    // 집는 순간에만 한 번 다시 세우고, 그 뒤로는 숫자만 갈아입힌다.
    fit.uses = toolUsesWords(row.sub?.tool_calls ?? 0);
    fit.usesNode = node.querySelector(".wt-agent-uses");
    if (Boolean(fit.uses) !== (fit.usesNode !== null)) return null;
    fit.status = agentRowStatusWord(row, state);
    fit.stateNode = node.querySelector(".wt-agent-state");
    if (fit.stateNode === null) return null;
  }
  return fit;
}

/* Re-dress the rows a card already holds, when only their words moved.
 *
 * `makeAgentRow` builds a button with its listeners, its state mark and its
 * agent's face, and the card's signature includes what the agent last said
 * — so an agent renaming its terminal a few times a second had its whole
 * row rebuilt each time: new nodes, a layout of the sidebar, listeners
 * bound again (the freeze probe's `wt-agents childList×59`, sixty frames).
 * Words are text nodes and can be written in place. Anything that would
 * change a row's SHAPE — its agent, state, depth, fold, or a span that
 * would have to appear or go — is left to the rebuild, and the answer says
 * which it was. A card is re-dressed whole or rebuilt whole, never half of
 * each: every row is checked before any is written. */
function redressAgentRows(host, rows) {
  const held = host.children;
  if (held.length !== rows.length) return false;
  const fitted = [];
  for (const [at, row] of rows.entries()) {
    const fit = fittedAgentRowWords(held[at], row);
    if (fit === null) return false;
    fitted.push(fit);
  }
  for (const fit of fitted) {
    const { node, row, state, fold } = fit;
    if (fit.history) continue;
    if (fold !== null) writeAttribute(fold, "aria-label", agentFoldLabel(row));
    if (fit.foldedSummary) {
      const words = foldedSummaryWords(row, state, fold);
      if (node.dataset.tip !== words.tip) node.dataset.tip = words.tip;
      writeAttribute(node, "aria-label", words.label);
      continue;
    }
    writeTextContent(fit.name, fit.primary);
    if (fit.said !== null) writeTextContent(fit.said, fit.secondary);
    if (fit.wears !== null) {
      writeTextContent(fit.wears, fit.model);
      if (fit.wears.dataset.tip !== fit.model) fit.wears.dataset.tip = fit.model;
    }
    if (fit.usesNode !== null) writeTextContent(fit.usesNode, fit.uses);
    writeTextContent(fit.stateNode, fit.status);
    writeTextContent(node.querySelector(".wt-agent-when"), agentRowAgo(row));
    const tip = agentRowTip(fit.primary, fit.secondary, fit.uses);
    if (node.dataset.tip !== tip) node.dataset.tip = tip;
  }
  return true;
}

/* 헬퍼 행의 읽기 문 — 행이 아니라 행 안의 작은 손.
 *
 * t2163이 정한 것은 **행 클릭의 뜻**이었다: 헬퍼에게는 제 판이 없으므로 그
 * 행을 누르는 것은 그 일이 실제로 쓰이고 있는 부모 터미널로 가는 공간적
 * 약속이고, 그 자리를 전사 페이지에 내주었을 때 행은 제 한 가지 일을 잃었다.
 * 그 결정은 그대로다. 그렇다고 전사가 없어져야 하는 것은 아니다 — 벤더가
 * 디스크에 적어 둔 그 대화는 어떤 터미널도 보여 준 적이 없고
 * (`openHelperPage`), 결정 이후 그 페이지는 부르는 곳이 없어 광고만 남았다
 * ("kg-core를 클릭해도 볼수없음"). 그래서 둘은 한 행 안에서 갈라선다:
 * **행은 공간이고, 이 단추는 읽기 표면이다.**
 *
 * 행 자체가 손에 쥐는 것(`<button>`, 또는 `actsAsButton`을 두른 행)이므로 안의
 * 손은 span이다 — `.wt-agent-fold`가 같은 이유로 그렇게 서 있다. 문구는 한
 * 열쇠로 네 카탈로그를 지나고(`data-i18n-title`/`-aria`가 언어가 바뀔 때 같은
 * 낱말을 두 자리에 다시 쓴다), 열리지 않을 문은 `openHelperPage`가 스스로
 * 판별한다(전사가 없으면 부모 판으로). */
function makeHelperPeek(open) {
  const peek = document.createElement("span");
  peek.className = "wt-agent-peek";
  peek.dataset.i18nTitle = "session.subagentOpenTranscript";
  peek.dataset.i18nAria = "session.subagentOpenTranscript";
  const words = t("session.subagentOpenTranscript", "헬퍼 대화 보기");
  peek.dataset.tip = words;
  peek.setAttribute("aria-label", words);
  peek.innerHTML = icon("message-square");
  actsAsButton(peek, (event) => {
    // 행의 약속은 행의 것이다 — 이 손을 누른 것이 부모 판으로도 가면, 문
    // 하나를 눌러 두 곳이 열린다.
    event.stopPropagation();
    open();
  });
  return peek;
}

function makeAgentRow(row, gutter = false) {
  const node = document.createElement("button");
  node.type = "button";
  const state = agentRowState(row);
  const here = agentRowHere(row);
  const foldedSummary = row.kids > 0 && !row.kidsShown;
  // A row at work reads in two lines — what it is doing, then who and how —
  // and a resting one stays a single line, so the list keeps its density.
  const live = LIVE_HOOK_STATES.has(state);
  node.className = agentRowClasses(state, here, foldedSummary, row);
  node.dataset.term = String(row.term);
  // 「완료 N개」 — 정착한 헬퍼 이력의 한 줄. 누르면 그 N개가 각각의 전사 손을
  // 달고 서고, 다시 누르면 접힌다. 지우는 것이 아니다: 명부도 페이지도 그대로.
  if (row.history) {
    node.dataset.history = String(row.history);
    node.dataset.depth = String(row.depth ?? 0);
    if (row.depth > 0) node.style.setProperty("--card-depth", String(row.depth));
    const fold = document.createElement("span");
    fold.className = "wt-agent-fold";
    fold.setAttribute("role", "button");
    fold.tabIndex = 0;
    fold.setAttribute("aria-expanded", String(Boolean(row.historyShown)));
    fold.setAttribute("aria-label", agentHistoryLabel(row));
    fold.innerHTML = icon("chevron", Boolean(row.historyShown));
    const flip = (event) => {
      event.stopPropagation();
      event.preventDefault();
      if (agentHistoryShown.has(row.term)) agentHistoryShown.delete(row.term);
      else agentHistoryShown.add(row.term);
      paintWorktreeAgents();
    };
    fold.addEventListener("click", flip);
    fold.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") flip(event);
    });
    node.addEventListener("click", flip);
    const dot = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    dot.setAttribute("class", "icon wt-agent-dot");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-circle-check");
    dot.appendChild(use);
    dot.setAttribute("aria-hidden", "true");
    const body = document.createElement("span");
    body.className = "wt-agent-body";
    const name = document.createElement("span");
    name.className = "wt-agent-name";
    name.textContent = agentRowPrimary(row, state);
    body.appendChild(name);
    node.append(fold, dot, body);
    const words = agentHistoryLabel(row);
    node.dataset.tip = words;
    node.setAttribute("aria-label", words);
    return node;
  }
  // Which agent the row was built for, on the row: `redressAgentRows` reads
  // it back to know whether the face it built still belongs.
  node.dataset.agent = row.agent ?? "";
  node.dataset.depth = String(row.depth ?? 0);
  if (row.sub) node.dataset.sub = row.sub.id;
  if (row.depth > 0) node.style.setProperty("--card-depth", String(row.depth));
  // Orca's AgentStateDot is only a dot for the states that have no better
  // shape: a spinning ring while it works (drawn by CSS on the bare span), a
  // check once it is done, a question mark when it is waiting on a person.
  // Blocked and failed would be a red dot — this window's agents cannot say
  // either, so there is no class here that nothing can ever wear.
  // 자식이 있는 행은 셰브론을(누르면 그 밑이 열리고 닫힌다), 없는 행은 같은
  // 폭의 빈 자리를 — 형제 중 하나라도 셰브론을 차면 점들이 한 줄에 선다
  // (실측: reserveDisclosureGutter). 행 자체가 button이라 안의 손은 span이다.
  let fold = null;
  if (row.kids > 0) {
    fold = document.createElement("span");
    fold.className = "wt-agent-fold";
    fold.setAttribute("role", "button");
    fold.tabIndex = 0;
    fold.setAttribute("aria-expanded", String(row.kidsShown));
    fold.setAttribute("aria-label", agentFoldLabel(row));
    fold.innerHTML = icon("chevron", row.kidsShown);
    const flip = (event) => {
      event.stopPropagation();
      event.preventDefault();
      if (agentFolded.has(row.term)) agentFolded.delete(row.term);
      else agentFolded.add(row.term);
      paintWorktreeAgents();
    };
    fold.addEventListener("click", flip);
    fold.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") flip(event);
    });
  } else if (gutter && !row.sub && row.depth === 0) {
    fold = document.createElement("span");
    fold.className = "wt-agent-fold is-quiet";
    fold.setAttribute("aria-hidden", "true");
  }
  const mark =
    state === "done" ? "#i-circle-check" : state === "needs-attention" ? "#i-msg-ask" : "";
  let dot;
  if (mark) {
    dot = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    dot.setAttribute("class", "icon wt-agent-dot");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", mark);
    dot.appendChild(use);
  } else {
    dot = document.createElement("span");
    dot.className = "wt-agent-dot";
  }
  dot.setAttribute("aria-hidden", "true");
  // The agent's REAL face beside its state ("어떤 에이전트가 도는지 아이콘
  // 실제") — the registry's favicon chain, with its letter tile standing in
  // until the mark lands. A row that predates the registry, or a helper with
  // no vendor of its own, still gets the letter: a blank square would read
  // as a third state the dot never claimed.
  let face = null;
  if (row.agent) {
    const reg = agentRows.find((one) => one.id === row.agent);
    face = agentIcon(reg ?? { id: row.agent, name: agentName(row.agent), favicon_domain: "" });
    face.classList.add("wt-agent-face");
  }
  node.addEventListener("click", (event) => {
    event.stopPropagation();
    // A helper WITH a pane of its own — the seat said which (`paneHelpers`)
    // — goes to that pane, even while its row is still the roster's and not
    // yet folded onto the pane's (the pane has not spoken its first hook).
    // Going to the parent instead is the click that "does nothing": the
    // parent is the pane the person is already looking at.
    const own = row.sub && row.subHost === undefined ? paneOfHelper(row.sub.id) : null;
    if (own !== null) {
      const seat = tabOfTerm(own);
      void focusAgentPane(seat?.worktree ?? row.worktree, seat?.id ?? null, own, paneAgents.get(own) ?? row.agent);
      return;
    }
    // A helper without one has no pane of its own. Its navigator row therefore
    // opens the parent terminal whose process is running it, just like the
    // walk model says below. A vendor transcript is a separate reading
    // surface; choosing it here made the navigator row fail its one spatial
    // promise.
    void focusAgentPane(row.worktree, row.tab?.id ?? null, row.term, row.agent);
  });
  const activityBorder = document.createElement("span");
  activityBorder.className = "wt-activity-border";
  activityBorder.setAttribute("aria-hidden", "true");
  // Orca's collapsed compact summary is identity, not a shortened sentence:
  // one state shape and the actual running agent's icon, with the disclosure
  // at the far edge. Names, model, clock and +N return only when expanded.
  if (foldedSummary) {
    node.append(...[dot, face, fold].filter(Boolean));
    node.insertBefore(activityBorder, fold);
    const words = foldedSummaryWords(row, state, fold);
    node.dataset.tip = words.tip;
    node.setAttribute("aria-label", words.label);
    return node;
  }
  // 본문은 한 칸이다: primary와 secondary가 한 truncate 안에서 함께 줄어든다
  // (원본 `min-w-0 flex-1 truncate` 한 span). 따로 세우면 긴 도구 줄이 행을
  // 밀어내거나 이름만 잘리고 도구 줄은 통째로 남는다. " - " 구분은 said의
  // ::before가 긋는다 — 낱말 자체는 낱말로 남게.
  const body = document.createElement("span");
  body.className = "wt-agent-body";
  const name = document.createElement("span");
  name.className = "wt-agent-name";
  const primary = agentRowPrimary(row, state);
  name.textContent = primary;
  body.appendChild(name);
  const secondary = agentRowSecondary(row, state, primary);
  let said = null;
  if (secondary) {
    said = document.createElement("span");
    said.className = "wt-agent-said";
    said.textContent = secondary;
  }
  // The model chip, Orca's row exactly ("gpt-5.6-sol" — #33): muted mono,
  // after the words, only when the pane's agent has SAID a model. A helper
  // row carries its parent's term and would wear its parent's model as its
  // own, so it stays bare.
  const model = row.sub ? null : paneModels.get(row.term);
  let wears = null;
  if (model) {
    wears = document.createElement("span");
    wears.className = "wt-agent-model";
    wears.textContent = model;
    wears.dataset.tip = model;
  }
  // 헬퍼는 모델 칩을 달지 않는다(부모의 모델을 제 것인 양 입게 되므로). 그
  // 자리에 서는 것이 이 수다 — 도는 헬퍼가 무엇을 얼마나 했는지, Claude Code가
  // 도는 Task 줄에 다는 그 낱말로. 아직 아무것도 집지 않았으면 아무것도 서지
  // 않는다.
  const usesWords = toolUsesWords(row.sub?.tool_calls ?? 0);
  let uses = null;
  if (usesWords) {
    uses = document.createElement("span");
    uses.className = "wt-agent-uses";
    uses.textContent = usesWords;
  }
  // Then the clock, ALONE and on every row — Orca's own order is model chip,
  // folded count, then time, and the time is not the chip's tail: a row with
  // no model still has an age. The words are `shortAgo` — the compact row's
  // own vocabulary (now/Nm/Nh/Nd), which the original keeps apart from the
  // board's `formatStartedAgo` steps.
  const when = document.createElement("span");
  when.className = "wt-agent-when";
  when.textContent = agentRowAgo(row);
  // 그리고 헬퍼 행에만, 행의 오른쪽 끝에 그 헬퍼의 전사로 가는 손 하나
  // (`makeHelperPeek`가 그 갈라섬을 적어 둔다). 쉴 때 폭이 0이므로 이 손이
  // 없는 행과 있는 행의 낱말이 서는 자리는 같다.
  // 접어 입은 판(t-3024)의 손은 헬퍼 명부의 주인 — 부모 — 의 페이지를 연다:
  // 전사는 부모의 등록부에 있고, `helper:<term>:<id>` 탭도 그 이름이다.
  const peek = row.sub
    ? makeHelperPeek(() => void openHelperPage(
      row.subHost === undefined ? row : { ...row, term: row.subHost },
      row.sub,
    ))
    : null;
  // The status, in the same place on every row (`agentRowStatusWord`): a
  // short word at the right, before the clock — 작업 중, 확인 필요, 완료, or
  // the ledger's word once the work was handed in.
  const status = document.createElement("span");
  status.className = "wt-agent-state";
  status.textContent = agentRowStatusWord(row, state);
  if (live) {
    // Line two: who is at it (the agent's name, unless the first line is
    // already that name), the model it wears, and what it is doing now.
    const meta = document.createElement("span");
    meta.className = "wt-agent-meta";
    const label = row.sub ? row.sub.name : row.agent ? agentName(row.agent) : "";
    if (label && label !== primary) {
      const who = document.createElement("span");
      who.className = "wt-agent-who";
      who.textContent = label;
      meta.appendChild(who);
    }
    if (wears) meta.appendChild(wears);
    if (said) meta.appendChild(said);
    if (meta.childElementCount > 0) body.appendChild(meta);
  } else if (said) {
    body.appendChild(said);
  }
  const parts = (live
    ? [fold, dot, face, body, uses, status, when, peek]
    : [fold, dot, face, body, wears, uses, status, when, peek]
  ).filter(Boolean);
  node.append(...parts, activityBorder);
  node.dataset.tip = agentRowTip(primary, secondary, usesWords);
  return node;
}

async function focusAgentPane(worktree, tabId, term, agent = null) {
  if (worktree && worktree !== activeWorktreePath) {
    if (!(await activateWorktree(worktree))) return;
  }
  const tab = tabId === null ? null : tabs.find((held) => held.id === tabId);
  if (!tab) {
    // 떼어 둔 에이전트는 판이 없다. 그 줄을 누르는 것이 곧 다시 붙는 것이고,
    // 화면은 백엔드에 그대로 있으므로 새로 띄우는 것이 아니라 돌려받는 것이다.
    if (detachedAgents.has(term)) {
      if (revealListedWorker(term, worktree, agent)) return;
      detachedAgents.delete(term);
      mountTermTab(term, { worktree, ...(agent ? { agent: agentName(agent) } : {}) }, { placement: "tab" });
      // The person brought a parked worker back onto the stage.
      noteWorkerRoomChange(term, "tab");
    }
    return;
  }
  tab.activePane = term;
  setActiveTab(tab.id);
  paintActivePane(tab);
  if (termFloat.hidden) keySink.focus();
}

/* 지난 세션 — 앱을 껐다 켜면 판은 죽지만 대화는 디스크의 세션 스토어에
 * 남는다(실측 AI Vault). 라이브 에이전트 판이 하나도 없는 카드에만 서는
 * 이유: 판이 살아 있는 동안은 그 판이 곧 대화의 문이고, 같은 대화가 두
 * 행으로 서면 어느 쪽이 진짜인지 카드가 답을 잃는다. */
const retainedHeld = new Map();
const RETAINED_TTL = 30_000;

/* The ASK, on its own. One in-flight guard, one writer of the cache — both
 * callers (first sight below, the beat further down) go through here. */
/* 두 목록이 같은 그림인가.
 *
 * 줄 전체를 그대로 비교한다 — 백엔드가 보내는 것은 평평한 작은 객체 셋이고,
 * 「어느 필드가 그림인가」를 여기 적어 두면 백엔드가 필드를 하나 늘릴 때마다
 * 두 곳이 어긋난다. 같은 JSON이면 같은 그림이라는 말은 언제나 참이다. */
/* ---- 재시작을 건너온 판들 (P0-13) ----
 *
 * 백엔드 원장(Orca last-status.json v2의 이식, `last_status.rs`)이 부팅에
 * 건네는 지난 소식 목록. 살아있는 행이 있는 카드에는 끼지 않고 — 달리는
 * 판이 그 워크트리의 현재다 — 아래 Claude 디스크 스캔(retained)과 같은
 * 대화가 두 줄이 되지 않게 세션 id로 걸러 병치한다. */
let lastNews = [];

function lastNewsFor(path) {
  return lastNews.filter((one) => one.worktree === path);
}

/* 원장의 한 줄 — 지난 세션 행과 같은 옷(.is-retained)을 입되 얼굴은 그
 * 판의 에이전트, 낱말은 마지막 상태다. 눌러 되살릴 수 있는 것만 문이
 * 된다: resumable이 아닌 소식은 사실이지 초대가 아니다. */
function makeSurvivorRow(one) {
  const canResume = Boolean(one.resumable && one.session);
  const node = document.createElement(canResume ? "button" : "div");
  if (canResume) node.type = "button";
  node.className = "wt-agent is-retained";
  const dot = document.createElement("span");
  dot.className = "wt-agent-dot";
  dot.setAttribute("aria-hidden", "true");
  const reg = agentRows.find((row) => row.id === one.agent);
  const face = agentIcon(reg ?? { id: one.agent, name: agentName(one.agent), favicon_domain: "" });
  face.classList.add("wt-agent-face");
  // 같은 행 문법의 한 몸통: 이름과 상태어가 한 절단 안에 서고, 시계는 압축
  // 어휘를 쓴다 — 살아있는 행과 지난 행이 같은 칸을 다르게 접으면 목록이
  // 두 표가 된다.
  const body = document.createElement("span");
  body.className = "wt-agent-body";
  const name = document.createElement("span");
  name.className = "wt-agent-name";
  name.textContent = one.you ?? agentName(one.agent);
  const said = document.createElement("span");
  said.className = "wt-agent-said";
  said.textContent = bucketWord(one.state === "needs-attention" ? "attention" : one.state);
  body.append(name, said);
  const when = document.createElement("span");
  when.className = "wt-agent-when";
  when.textContent = shortAgo(one.at, Date.now());
  node.append(dot, face, body, when);
  node.dataset.tip = `${name.textContent} - ${said.textContent}`;
  if (canResume) {
    node.addEventListener("click", (event) => {
      event.stopPropagation();
      void reopenConversationIn(one.worktree, { agent: one.agent, session: one.session });
    });
  }
  return node;
}

/* 사이드바의 지난 대화 한 줄을 눌렀을 때 — 원장의 소식 행이든 디스크 스캔의
 * 「지난 세션」 행이든, 문은 하나다.
 *
 * 워크트리가 먼저다. `resume_session`은 판의 cwd를 **활성 루트**에서 가져오므로
 * (`board.rs`), 다른 체크아웃의 대화를 여기 선 채로 되살리면 그 대화가 엉뚱한
 * 디렉터리에서 깨어난다 — 카드가 그 워크트리의 것이라고 말해 놓고서.
 *
 * 그리고 그 워크트리를 여는 것이 곧 저장된 판들을 되살리는 것이므로, 묻는 것은
 * 그 되살림이 **끝난 뒤**다. 한 대화에 한 판이라는 판정은 백엔드가 하고
 * (`resumeSession` → `wakeConversation`), 이 문은 그 답이 가리키는 판으로 간다.
 * 2026-09-16의 두 번째 `zo --resume`은 활성화 직후에 물은 결과였다: 그때 대화는
 * 아직 깨어나는 중이었고, 이 창의 사본 판정은 그것을 보지 못했다. */
async function reopenConversationIn(path, known) {
  if (path && path !== activeWorktreePath) {
    if (!(await activateWorktree(path))) return;
  }
  await storedWakesSettled(path);
  await resumeSession(known);
}

function retainedRowKey(row) {
  return JSON.stringify(row);
}

function sameRetainedRows(was, now) {
  if (!was) return now.length === 0;
  return was.length === now.length
    && was.every((one, at) => retainedRowKey(one) === retainedRowKey(now[at]));
}

function refreshRetainedSessions(path) {
  const held = retainedHeld.get(path);
  if (held?.want) return;
  const want = invoke("list_claude_sessions", { path })
    .then((rows) => {
      const fresh = rows ?? [];
      const moved = !sameRetainedRows(held?.rows, fresh);
      retainedHeld.set(path, { at: Date.now(), rows: fresh });
      // Only when the ROWS would differ. Revalidation answers the same list
      // almost every time — a repaint for an answer that changes nothing is
      // the whole cost of the beat with none of its point, and it lands in
      // the same frame budget the hook storm is measured against. Through the
      // coalescer when it does move: an answer arriving mid-frame joins that
      // frame's one repaint rather than standing a second beside it.
      if (moved) scheduleAgentPaint(["cards"]);
    })
    .catch(() => retainedHeld.set(path, { at: Date.now(), rows: [] }));
  retainedHeld.set(path, { at: held?.at ?? 0, rows: held?.rows, want });
}

function warmRetainedSessions(path, now = Date.now()) {
  const held = retainedHeld.get(path);
  if (held?.want || (held && now - held.at < RETAINED_TTL)) return;
  refreshRetainedSessions(path);
}

function warmMaterializedRetainedSessions(now = Date.now()) {
  const hosts = [...worktreeList.querySelectorAll(".wt-agents[data-worktree-path]")];
  if (hosts.length === 0) {
    retainedTicker.sync();
    return;
  }
  for (const host of hosts) {
    warmRetainedSessions(host.dataset.worktreePath, now);
  }
  retainedTicker.sync();
}

/* What the paint reads. A CACHE READ and nothing more: this function runs
 * inside `paintWorktreeAgents`, which every `hook:activity` batch repaints,
 * and any ask from here — first sight or TTL expiry alike — was a backend
 * round trip riding the per-tool-call hot path, which is the exact cost the
 * one-frame-one-paint gate exists to refuse. First sight is warmed where a
 * virtual row is materialized (`renderSidebarWindow`), so an off-screen
 * logical member pays nothing; expiry is the 30-second beat's below. */
function retainedSessionsFor(path) {
  return retainedHeld.get(path)?.rows ?? [];
}

/* The revalidation clock — the board's own cadence (`boardClockBeat`), for
 * the same reason: staleness is a fact about TIME, so time is what asks.
 * Only paths whose card is actually drawn with a retained row revisit; a
 * hidden window lets the whole thing sleep. */
const retainedTicker = idlePoller({
  // Empty answers need another look too. Keying the beat off an existing
  // `.is-retained` child made an empty result permanent; the materialized
  // workspace host is the actual visibility boundary.
  wanted: () => worktreeList.querySelector(".wt-agents[data-worktree-path]") !== null,
  every: RETAINED_TTL,
  tick: warmMaterializedRetainedSessions,
  // Back after an absence, the rows may have aged past the beat — look now.
  onResume: warmMaterializedRetainedSessions,
});

/* 지난 세션 한 줄 — 얼굴은 Claude, 이름은 그 대화의 첫 문장, 시각은
 * 마지막으로 움직인 때. 누르면 그 자리에서 `--resume`으로 되살아난다 —
 * Orca의 동면 행은 눌러도 침묵하지만(실측 useRetainedAgents), 사람이 이
 * 행을 누르는 뜻은 하나뿐이라 이 창은 그것을 문으로 만든다(기록된 이탈). */
function makeRetainedRow(path, row) {
  const node = document.createElement("button");
  node.type = "button";
  node.className = "wt-agent is-retained";
  node.dataset.session = row.session.id;
  const dot = document.createElement("span");
  dot.className = "wt-agent-dot";
  dot.setAttribute("aria-hidden", "true");
  const reg = agentRows.find((one) => one.id === "claude");
  const face = agentIcon(reg ?? { id: "claude", name: agentName("claude"), favicon_domain: "" });
  face.classList.add("wt-agent-face");
  const body = document.createElement("span");
  body.className = "wt-agent-body";
  const name = document.createElement("span");
  name.className = "wt-agent-name";
  name.textContent = row.title;
  const said = document.createElement("span");
  said.className = "wt-agent-said";
  said.textContent = t("board.retained", "지난 세션");
  body.append(name, said);
  const when = document.createElement("span");
  when.className = "wt-agent-when";
  when.textContent = shortAgo(row.at_ms, Date.now());
  node.append(dot, face, body, when);
  node.dataset.tip = `${row.title} - ${said.textContent}`;
  node.addEventListener("click", (event) => {
    event.stopPropagation();
    // The store's own record of the conversation — key, id and file — so this
    // door hands the resume road the same shape every other door does.
    void reopenConversationIn(path, { agent: "claude", session: row.session });
  });
  return node;
}

/* A sleeping worker whose coordinator workspace has not mounted yet. It is a
 * status row, never a launch button: clicking it must not open the generic
 * agent that the ledger reservation exists to keep out. */
function makeRestoreWaitingRow(agent) {
  const node = document.createElement("div");
  node.className = "wt-agent is-retained";
  const dot = document.createElement("span");
  dot.className = "wt-agent-dot";
  dot.setAttribute("aria-hidden", "true");
  const reg = agentRows.find((row) => row.id === agent);
  const face = agentIcon(reg ?? { id: agent, name: agentName(agent), favicon_domain: "" });
  face.classList.add("wt-agent-face");
  const body = document.createElement("span");
  body.className = "wt-agent-body";
  const name = document.createElement("span");
  name.className = "wt-agent-name";
  name.textContent = agentName(agent);
  const said = document.createElement("span");
  said.className = "wt-agent-said";
  said.textContent = t("board.restoreWaiting", "복원 대기");
  body.append(name, said);
  node.append(dot, face, body);
  node.dataset.tip = `${name.textContent} - ${said.textContent}`;
  return node;
}

/* The agent rows of every card, refreshed in place.
 *
 * In place, because this runs on every `hook:agent` — which is every turn of
 * every agent in the window — and rebuilding the sidebar for it would undo
 * exactly what the shape guard in `refreshWorktrees` just bought. Nothing is
 * fetched and nothing outside these small containers is touched. */
/* The card's handle, and the only place that knows how much it is hiding.
 *
 * One writer, because the two lists it speaks for are filled by two different
 * roads — the lanes when the sidebar is rebuilt, the agents on every turn of
 * every agent — and a handle each of them dressed would say two different
 * numbers about one card. Standing at all is the same question: nothing to
 * reveal and no handle, unless it is what would put the card back. */
function dressWorktreeTwist(row, path, hidden) {
  const twist = row?.querySelector(".wt-twist");
  if (!twist) return;
  const open = expandedWorktrees.has(path);
  // Each written only when it differs: this is dressed on every change of
  // the card's words, and an attribute set to its own value is still a
  // mutation record (the freeze probe counted five a frame on this handle).
  const shut = !open && hidden === 0;
  if (twist.hidden !== shut) twist.hidden = shut;
  const chevron = icon("chevron", open);
  if (twistDrawn.get(twist) !== chevron) {
    twist.innerHTML = chevron;
    twistDrawn.set(twist, chevron);
  }
  const label = open
    ? t("sidebar.showLastSession", "마지막 세션만 보기")
    : t("sidebar.showMoreSessions", "세션 {{count}}개 더 보기", { count: hidden });
  if (twist.getAttribute("aria-label") !== label) twist.setAttribute("aria-label", label);
  const expanded = String(open);
  if (twist.getAttribute("aria-expanded") !== expanded) twist.setAttribute("aria-expanded", expanded);
  if (twist.dataset.tip !== label) twist.dataset.tip = label;
}

/* The chevron markup each handle last received — the serialized `innerHTML`
 * never reads back equal to the string that made it, so the string itself
 * is remembered. */
const twistDrawn = new WeakMap();

/* A paneless card joins two stores. `at` is the last-status event time in the
 * restart ledger; `at_ms` is the last-activity time in the retained-session
 * store. Each is the authoritative clock for its source, so sorting this
 * joined list before the one-session summary makes the newest row win even
 * when either source answered in a different order. */
function restartSessionAt(item) {
  return item.kind === "survivor" ? (item.row.at ?? 0) : (item.row.at_ms ?? 0);
}

function restartSessionsSaid(items) {
  return items.map(({ kind, row }) => `${kind}:${JSON.stringify(row)}`).join(",");
}

function restartSessionGroups(items) {
  return items
    .slice()
    .sort((left, right) => restartSessionAt(right) - restartSessionAt(left))
    .map((item) => ({
      members: [item],
      live: false,
      staged: false,
      born: restartSessionAt(item),
    }));
}

function paintWorktreeAgents() {
  if (revealStartupActiveProjects()) void refreshWorktrees();
  const held = worktreeLanes();
  for (const host of worktreeList.querySelectorAll(".wt-agents[data-worktree-path]")) {
    const path = host.dataset.worktreePath;
    const all = worktreeAgentRows(path);
    const open = expandedWorktrees.has(path);
    const full = agentActivityDisplay === "full";
    const owned = held.get(path) ?? [];
    const laneHidden = open ? 0 : owned.length - summaryLanes(owned).length;
    // The summary object carries both the cut and its count. The same helper
    // is used again for restart sessions below, so live and dead cards cannot
    // drift into different folding rules.
    const liveSummary = summarizeAgentRows(all, { open, full });
    const rows = liveSummary.rows;
    const survivors = rows.length === 0 ? lastNewsFor(path) : [];
    // 같은 대화가 원장과 디스크 스캔에 다 있으면 원장이 이긴다 — 상태와
    // 낱말을 아는 쪽이 그 줄의 더 나은 화자다.
    const retained = rows.length === 0
      ? retainedSessionsFor(path).filter(
          (one) => !survivors.some((past) => past.session?.id === one.session.id),
        )
      : [];
    const restoring = rows.length === 0 ? restoringWorkers.get(path) : null;
    const restartSessions = rows.length === 0
      ? [
        ...survivors.map((row) => ({ kind: "survivor", row })),
        ...retained.map((row) => ({ kind: "retained", row })),
      ]
      : [];
    // Sorting is only needed when the sources changed. A hook from another
    // card still visits this painter, so compare the unsorted source before
    // doing any work to order it; the final signature below includes the
    // sliced rows and count once a change actually needs a repaint.
    const sourceSignature = rows.length === 0
      ? `dead:${restartSessionsSaid(restartSessions)}|restoring:${restoring ?? ""}`
        + `|lanes:${laneHidden}|${open ? 1 : 0}|${full ? 1 : 0}`
      : null;
    if (sourceSignature !== null && host.dataset.restartSource === sourceSignature) continue;
    if (sourceSignature === null) delete host.dataset.restartSource;
    else host.dataset.restartSource = sourceSignature;
    const deadSummary = rows.length === 0
      ? summarizeWorktreeGroups(restartSessionGroups(restartSessions), { open, full })
      : { rows: [], hidden: 0, total: 0 };
    // This is the one count for the card's three owners: live agents, joined
    // restart sessions, and lanes. It is deliberately derived from each
    // summary's pre-slice total, so the handle says what is actually hidden.
    const hidden = liveSummary.hidden + deadSummary.hidden + laneHidden;
    // A restore-waiting row is an active ledger reservation, not a dead
    // session, so it stands beside the compacted session summary rather than
    // being counted as one of the sessions behind the handle.
    // The same sentence the sidebar's own shape guard compares — one function,
    // because two spellings of "did anything move" is how the guard and the
    // repaint end up disagreeing about whether it did.
    //
    // What the card is HOLDING BACK is part of that sentence, and it has to
    // be: the handle is dressed below this gate, and a lane arriving in a card
    // whose agents did not move would otherwise leave the count on it stale.
    const signature = (rows.length > 0
        ? agentRowsSaid(rows)
        : `dead:${restartSessionsSaid(deadSummary.rows)}|total:${deadSummary.total}`
        + `|restoring:${restoring ?? ""}`)
      + `|more:${hidden}|${open ? 1 : 0}|${full ? 1 : 0}`;
    if (host.dataset.said === signature) continue;
    host.dataset.said = signature;
    dressWorktreeTwist(host.previousElementSibling, path, hidden);
    dressWorktreeTitle(host.previousElementSibling);
    if (rows.length > 0) {
      // A root agent is still a CHILD of the workspace row. Reserve the
      // disclosure seat even when it is the only agent and has no children;
      // without that quiet seat its state dot lands on the workspace dot's
      // exact axis and the two levels read as siblings. Rows that later gain
      // children replace the quiet seat with the real chevron in place.
      if (!redressAgentRows(host, rows)) {
        host.replaceChildren(...rows.map((row) => makeAgentRow(row, true)));
      }
    } else {
      host.replaceChildren(
        ...deadSummary.rows.map(({ kind, row }) => kind === "survivor"
          ? makeSurvivorRow(row)
          : makeRetainedRow(path, row)),
        ...(restoring ? [makeRestoreWaitingRow(restoring)] : []),
      );
    }
    // Visibility uses the full, pre-slice totals. A collapsed card still has
    // content and must keep its handle; only a genuinely empty card hides the
    // host (a restore-waiting row also keeps it visible).
    host.hidden = all.length + deadSummary.total === 0 && !restoring;
    const unseated = host.previousElementSibling?.querySelector(".wt-unseated");
    if (unseated) {
      unseated.hidden = !host.hidden;
      // The branch line's standing is one decision (`dressWorktreeTitle`):
      // a name the title already said, a base chip, this marker.
      dressWorktreeTitle(host.previousElementSibling);
    }
    // The card boundary and the list read the SAME fact. Deriving ownership
    // again with `:has([hidden])` left an inactive worker looking like a
    // top-level sibling in the measured browser even though this painter had
    // already decided the list was visible. One owner writes the state; CSS
    // only dresses it.
    host.closest(".wt-node")?.classList.toggle("has-agents", !host.hidden);
  }
  retainedTicker.sync();
}

function hookStateOfTab(tab) {
  if (tab.kind !== "term") return null;
  let worst = null;
  for (const term of paneLeaves(tab.layout)) {
    const said = hookStates.get(term);
    if (!said) continue;
    if (worst === null || (HOOK_RANK[said] ?? 0) > (HOOK_RANK[worst] ?? 0)) worst = said;
  }
  return worst;
}

/* ---- 에이전트 소식 하나에 화면 넷씩 그리던 것을, 한 프레임에 한 번으로 ----
 *
 * `hook:agent`는 에이전트마다 도구 호출마다 온다. 넷이 돌면 초에 수십 번이고,
 * 그때마다 탭 줄 전체 + 카드의 에이전트 행 + 문의 배지(**왕복 한 번**) + 열려
 * 있는 보드(왕복 두 번)를 각각 그렸다. 상태가 그대로인 이벤트조차 보드를
 * 그렸다. 넷 다 "지금 무엇이 참인가"를 그리는 일이므로, 사이사이의 중간 상태를
 * 그리는 것은 값만 치르고 아무도 보지 못하는 그림이다.
 *
 * 그래서 **장부는 즉시, 그림은 한 박자 뒤에**. `hookStates`·`paneSessions`·
 * `acknowledgedPanes`는 이벤트가 닿는 즉시 쓰인다 — 늦춰도 되는 것은 화면뿐이고,
 * 늦춰진 그림은 마지막 사실 하나만 그린다.
 *
 * 프레임과 타이머를 함께 건다. rAF는 창이 가려지면 멈추는데, 문의 배지는 가려진
 * 창에서도 맞아야 하는 단 하나다 — 그 숫자가 그대로 독 배지로 나가기 때문이다.
 * 먼저 도착한 쪽이 그리고 다른 쪽을 끈다. */
const AGENT_PAINT_FLOOR_MS = 150;
const agentPaintDue = { tabs: false, cards: false, badge: false, board: false };
let agentPaintFrame = null;
let agentPaintTimer = null;

function flushAgentPaint() {
  if (agentPaintFrame !== null) cancelAnimationFrame(agentPaintFrame);
  if (agentPaintTimer !== null) clearTimeout(agentPaintTimer);
  agentPaintFrame = null;
  agentPaintTimer = null;
  // 빚은 그리기 **전에** 지운다: 그리는 도중에 도착한 소식은 다음 프레임의
  // 것이고, 지금 그림에 담겼다고 볼 수 없다.
  const due = { ...agentPaintDue };
  for (const surface of Object.keys(agentPaintDue)) agentPaintDue[surface] = false;
  if (due.tabs) renderTabs();
  // The card is the rows AND the dot on the row above them: one agent moving
  // is one workspace changing what it says, and drawing half of that would
  // leave a card reading "작업 중" under a dot that had gone quiet.
  if (due.cards) {
    paintWorktreeDots();
    paintWorktreeUnread();
    paintWorktreeAgents();
    // 같은 사실의 다섯째 표면: 아티팩트 스튜디오의 「초안을 받을 에이전트」는
    // 에이전트가 앉은 판의 목록이다. 서 있을 때만 그린다(2026-09-13, 「에이전트가
    // codex만 표시」 — 새 판의 에이전트가 도착해도 목록이 그대로였다).
    noteArtifactSeats();
  }
  // Badge and graph are one Rust snapshot. The badge remains global even when
  // the open graph is searched, so one answer can feed both without a second
  // pane gather or state-classification pass.
  const paintGraph = due.board && boardIsOpen();
  if (due.badge || paintGraph) void refreshAgentGraphSurfaces({ badge: due.badge, graph: paintGraph });
}

let agentGraphRefreshing = false;
const agentGraphRefreshDue = { badge: false, graph: false };
async function refreshAgentGraphSurfaces({ badge, graph }) {
  if (agentGraphRefreshing) {
    agentGraphRefreshDue.badge ||= badge;
    agentGraphRefreshDue.graph ||= graph;
    return;
  }
  agentGraphRefreshing = true;
  try {
    const cards = await boardCards();
    const answer = await invoke("board_snapshot", {
      cards,
      query: agentGraphSnapshotQuery(),
    });
    if (badge) applyBoardBadge(answer);
    if (graph) await paintAgentGraphView(boardTab(), { snapshot: { cards, answer } });
  } catch (error) {
    if (graph) showError(String(error));
  } finally {
    agentGraphRefreshing = false;
    const due = [];
    if (agentGraphRefreshDue.badge) due.push("badge");
    if (agentGraphRefreshDue.graph) due.push("board");
    agentGraphRefreshDue.badge = false;
    agentGraphRefreshDue.graph = false;
    if (due.length) scheduleAgentPaint(due);
  }
}

function scheduleAgentPaint(surfaces) {
  for (const surface of surfaces) agentPaintDue[surface] = true;
  if (agentPaintFrame !== null || agentPaintTimer !== null) return;
  agentPaintFrame = requestAnimationFrame(flushAgentPaint);
  agentPaintTimer = window.setTimeout(flushAgentPaint, AGENT_PAINT_FLOOR_MS);
}

/* A terminal title changes two visible leaves and no shape: the label in its
 * standing tab and the conversation name in its compact agent row. Writing
 * those through the same guarded leaf helper as the full painters avoids a
 * strip + every-card traversal on each OSC-title frame.
 *
 * All preconditions are checked before either write. If a tab/row has not
 * been built yet, if the row's shape would change, or if a whole tab/card
 * paint is already due this frame, the caller schedules the ordinary paint.
 * The last condition is important: the full painter owns that frame and the
 * fast path must not write a first copy of words it is about to reconcile. */
function writeTermTitleLeaves(term) {
  if (agentPaintDue.tabs || agentPaintDue.cards) return false;
  const tab = tabOfTerm(term);
  if (!tab) return false;

  let tabNode = null;
  for (const group of stageGroups()) {
    const candidate = groupOf(group).tabNodes?.get(tab.id)?.node ?? null;
    if (candidate?.isConnected) {
      tabNode = candidate;
      break;
    }
  }
  const label = tabNode?.querySelector(".tab-label") ?? null;
  if (label === null) return false;

  const host = [...worktreeList.querySelectorAll(".wt-agents[data-worktree-path]")]
    .find((one) => one.dataset.worktreePath === tab.worktree) ?? null;
  if (host === null) return false;
  const open = expandedWorktrees.has(tab.worktree);
  const full = agentActivityDisplay === "full";
  const rows = summarizeAgentRows(worktreeAgentRows(tab.worktree), { open, full }).rows;
  const at = rows.findIndex((row) => row.term === term && !row.sub);
  if (at < 0) return false;
  const rowNode = [...host.children].find(
    (node) => node.dataset.term === String(term) && !node.dataset.sub,
  ) ?? null;
  if (rowNode === null) return false;
  const fit = fittedAgentRowWords(rowNode, rows[at]);
  if (fit === null || fit.foldedSummary) return false;

  writeTextContent(label, tabLabel(tab));
  writeTextContent(fit.name, fit.primary);
  if (fit.said !== null) writeTextContent(fit.said, fit.secondary);
  return true;
}

/* Reconcile the navigator's in-memory copy with the backend's retained copy.
 *
 * This does not infer a state. In particular, `pane_agents` uses `at: 0` for
 * a live launched pane that has never reported; its fallback `working` is a
 * liveness judgement for the board, not a hook fact, so it is never seeded
 * here. A positive `at` names an actual `PaneState` retained by the backend.
 *
 * The fence has two clocks with one unit. A hook is stamped when this webview
 * observes it; a snapshot carries the backend epoch at which that same stored
 * fact was written. A late answer may only replace an older observation. The
 * observed clock moves on EVERY hook, including a repeated state, because a
 * Done snapshot already in flight must not rewind a newer repeated Working.
 * `hookStamps` remains the state-change clock shown on the row. */
function reconcilePaneAgentSnapshot(panes) {
  let changed = false;
  for (const pane of panes ?? []) {
    if (!paneAutonomy.has(pane.term) && pane.autonomy) {
      rememberPaneAutonomy(pane.term, pane.autonomy, paneSessions.get(pane.term)?.session?.id ?? null);
    }
    if (!(pane.at > 0) || !(pane.at > hookStates.observedAt(pane.term))) continue;
    hookStates.observe(pane.term, pane.at);
    if (!(pane.state in HOOK_RANK) || hookStates.get(pane.term) === pane.state) continue;
    hookStates.set(pane.term, pane.state);
    hookStates.noteSnapshot(pane.term);
    hookStamps.set(pane.term, pane.state_started_at > 0 ? pane.state_started_at : pane.at);
    changed = true;
  }
  // Every consumer the push path moves must hear the repaired transition.
  // Otherwise a matching push takes the listener's same-state early return
  // and leaves (for example) the door badge on its pre-snapshot count.
  if (changed) scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
  return changed;
}

let paneSnapshotAsking = false;
async function refreshPaneAgentSnapshot() {
  if (paneSnapshotAsking) return;
  paneSnapshotAsking = true;
  try {
    reconcilePaneAgentSnapshot(await invoke("pane_agents"));
  } catch {
    // The push path remains live. A periodic repair must never become an error
    // storm when the window is closing or the backend is briefly unavailable.
  } finally {
    paneSnapshotAsking = false;
  }
}

/* 경과 낱말이 굳지 않게 — 카드 페인트는 훅 이벤트에 실려 오므로, 조용한
 * 1분을 넘기는 "now"는 이 박자가 바로잡는다. 그릴 것이 있을 때만 두드린다:
 * 이제 시계는 **모든** 행이 차므로, 모델을 말한 판이 하나도 없어도 말을 한
 * 판이나 대화가 붙은 판이 있으면 두드릴 것이 있다. */
function paneAgentClockWanted() {
  return paneAgents.size > 0 || paneModels.size > 0
    || hookStates.size > 0 || paneSessions.size > 0;
}

/* Whether the clock below has anything to tick for — the pane rows above, or
 * a lane's helper rows. Named, so whoever needs the clock quiet asks this one
 * answer instead of keeping a list of the maps it reads: a window test that
 * cleared "the three maps" kept the beat alive once a fourth and a lane joined
 * the answer, and its 300 ms count caught a 30-second tick (v1.3.103 lane,
 * cards 2 and one `pane_agents` more, twice in three runs). */
function agentClockWanted() {
  return paneAgentClockWanted() || laneSubagents.size > 0;
}

const agentClock = idlePoller({
  wanted: agentClockWanted,
  every: AGENT_RELATIVE_TIME_TICK_MS,
  tick: () => {
    if (paneAgentClockWanted()) {
      scheduleAgentPaint(["cards"]);
      void refreshPaneAgentSnapshot();
    }
    paintLaneSubagentClocks();
  },
});

listen("hook:agent", (event) => {
  const { term, state, agent, session } = event.payload;
  agentGraphHotKey = agentGraphAgentKey(`term:${term}`);
  // A run NESTED in this pane reports through the pane's own terminal — it
  // runs inside that pty — so its news arrives here wearing the pane's name.
  // Measured: a `codex exec` under Claude Code set the PANE's model to
  // codex's, and the sidebar row wore it under the project's own name ("지금
  // claude 창에 프로젝트 창 밑에 코덱스로 바뀌고"). A nested run has its own
  // page and its own row; renaming the pane that started it is the one thing
  // it must not do.
  //
  // The backend already says which runs are nested — `hook:subagent` carries
  // one row per live run, id `run:<vendor>` — so this asks that rather than
  // guessing from a vendor name. Only a FOREIGN vendor is turned away: a pane
  // running a nested run of its OWN vendor cannot be told apart from itself
  // from here, and turning that away would silence the pane instead of its
  // child. And with no nested run in flight nothing is turned away at all, so
  // quitting one agent and starting another in the same pty still renames the
  // pane, which is what it should do.
  const ownAgent = paneAgents.get(term);
  const fromNested = (paneSubagents.get(term) ?? []).some(
    (row) => row.id === `run:${agent}` && row.state !== "done",
  );
  if (agent && ownAgent && agent !== ownAgent && fromNested) return;
  // The snapshot fence is about observations, not transitions. A repeated
  // Working is still newer than a Done snapshot whose request was already in
  // flight, even though it must not reset the row's elapsed clock below.
  hookStates.observe(term, Date.now());
  const confirmsSnapshot = hookStates.consumeSnapshot(term);
  // The restore word says how the pane was born. Its first real hook replaces
  // that with what the agent is doing now, even when the hook repeats the
  // placeholder state already painted for the launch.
  const clearedRestoreBirth = restoredWorkers.delete(term);
  // The model rides raw per event; the stickiness lives here (1-fv). An
  // event without one is an event about something else, not a forgetting.
  if (event.payload.model) paneModels.set(term, event.payload.model);
  if (event.payload.permission_mode) panePermissionModes.set(term, event.payload.permission_mode);
  // The row's word ladder eats both of these (Orca's entry.prompt /
  // entry.lastAssistantMessage) — sticky for the same reason as the model.
  if (event.payload.prompt) panePrompts.set(term, event.payload.prompt);
  if (event.payload.said) paneSaid.set(term, event.payload.said);
  // Which agent, for the pane header's continue button — the same fact the
  // backend's ledger holds, kept current here so the button never waits.
  if (agent) paneAgents.set(term, agent);
  // The toggle speaks for an agent's pane; this is where a pane becomes one.
  paintViewToggle();
  // A question the hook DESCRIBED — a permission request with its tool and
  // its edit, an AskUserQuestion with its options — stands in the
  // conversation as the extension's card, answered down the board's own
  // roads (`answer_approval`, `answer_ask`). A question the hook only
  // SIGNALLED is on the program's own screen and nowhere else — a menu, a
  // prompt with no payload — and a conversation standing over it would hide
  // the very thing it asks, so the screen comes back.
  const asking = state === "needs-attention" || state === "awaiting-permission" || state === "waiting";
  const shaped = asking && Boolean(event.payload.approval || event.payload.ask_prompt);
  const askBefore = paneAsks.get(term)?.sig ?? null;
  if (shaped) {
    const ask = { agent, approval: event.payload.approval ?? null, ask_prompt: event.payload.ask_prompt ?? null };
    paneAsks.set(term, { ...ask, sig: askSignature(ask) });
  } else {
    paneAsks.delete(term);
  }
  if (paneChatOn(term)) {
    if (asking && !shaped) setPaneChat(term, false);
    else if ((paneAsks.get(term)?.sig ?? null) !== askBefore) paintPaneChat(term);
    // The transcript this page reads was written before the hook fired — the
    // answer before Stop, the call before PreToolUse — so the page on this
    // pane reads it now, not on its next tick.
    if (activeHelperPage()?.worker.term === term) void pollHelperPages();
  }
  // Which conversation, when the event said. Recorded before the early return
  // below: a repeated state is still the event that told us the session id, and
  // returning first would drop it for any agent whose first report is a second
  // `working`.
  // `resumable` is the BACKEND's answer, not a guess from the presence of an id:
  // two agents report a session and offer no way back to it, and the table of
  // vendor flags that knows which lives over there.
  if (state === "idle" || event.payload.session_boundary ||
      (session && paneSessions.get(term)?.session?.id !== session.id)) {
    paneAutonomy.delete(term);
  }
  const named = Boolean(session) && paneSessions.get(term)?.session?.id !== session.id;
  if (session) paneSessions.set(term, { agent, session, resumable: event.payload.resumable });
  // The session id a hand-over needs arrived (or changed) while the person
  // already had the conversation up as the transcript view: the wire is
  // tried now — between turns; mid-turn `handPaneToWire` waits for the end.
  if (named) {
    const chat = paneChats.get(term);
    if (chat?.on && !chat.handing && wireRowFor(term)) void handPaneToWire(term);
  }
  // A state change in the pane the person is already looking at is seen the
  // moment it happens — without this, working in the front tab still grew an
  // unseen dot on the board behind you. One event, one moment: the read and
  // the stamp below take the same clock reading. Two `Date.now()` calls a
  // millisecond apart left the read 1 ms before the stamp, and the workspace
  // in front of the person turned bold as unread (…507 < …508).
  const heard = Date.now();
  const owner = tabOfTerm(term);
  if (owner && owner.id === activeTabId) acknowledgedPanes.set(`term:${term}`, heard);
  if (hookStates.get(term) === state) {
    // The state did not move but the card's lines may have — a second prompt
    // inside one working stretch, or a turn ending with new words. The open
    // board and the sidebar's rows both speak those lines now; the tab strip
    // draws states, not sentences.
    if (confirmsSnapshot) {
      // The retained copy repaired the state before this matching push
      // arrived. Consume that origin once so all state-change consumers hear
      // the real push; later repeated tool events remain on the quiet path.
      scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
    } else if (clearedRestoreBirth || event.payload.prompt || event.payload.said) {
      scheduleAgentPaint(["cards", "board"]);
    }
    // The state stood still, but the model or the mode may have moved.
    paintComposerChipsFor(term);
    return;
  }
  const wasMidTurn = isMidTurn(hookStates.get(term));
  hookStates.set(term, state);
  // The composer wears the state the pane now has — send or stop, the
  // queue's placeholder or its own words. Painted after the set, not before
  // it: painted above, it wore the old word until the next repaint (measured:
  // the helper page's stop button outlived the end of the turn).
  paintComposerChipsFor(term);
  // 판의 상태가 **여기서** 움직인다 — 칩을 다시 그린 자리(위)에서는 훅의
  // 낱말이 아직 옛것이라, 턴이 끝난 그 순간을 그 자리에서는 볼 수 없다.
  // 그래서 기다리던 글을 내보내는 문은 상태가 실제로 옮겨 앉은 이 줄 뒤에
  // 선다(그리는 문과 보내는 문은 여전히 따로다).
  settleComposerQueuesFor(term);
  agentClock.sync();
  // A conversation view asked for mid-turn is handed to the wire now that the
  // turn is over — if the person still has it up.
  const chat = paneChats.get(term);
  if (chat?.pendingWire && !isMidTurn(state) && chat.on) {
    chat.pendingWire = false;
    void handPaneToWire(term);
  }
  // When it moved, for the row's elapsed word — state changes only, so a
  // busy turn's dozens of events do not pin the clock at "now".
  hookStamps.set(term, heard);
  // 계정 전환을 기다리던 판이 쉼에 들었다 — 갈아탈 숨이다. 큐가 비어 있으면
  // 셋 조회 하나로 끝나는 길이라 바쁜 턴의 수십 이벤트에 얹혀도 무게가 없다.
  if (state === "done" && accountHandoffQueue.has(term)) void drainAccountHandoffs();
  // The layout file must know whether this pane is MID-TURN, because that
  // file is what a restart reads — and a window can die at any moment, so
  // waiting for the next tab-shaped persist would store yesterday's answer.
  // Only the mid-turn boundary matters and only crossings reach here (the
  // repeat-state return above), so this costs one write per turn edge, not
  // one per tool call.
  if (wasMidTurn !== isMidTurn(state) && owner?.worktree) {
    persistPaneLayouts(owner.worktree);
  }
  // An agent that LEFT took its tool children with it — every page still
  // waiting on this pane is orphaned by the same verdict (1-fe).
  if (state === "idle") orphanWorkersOf(term);
  // 네 표면이 같은 사실을 말한다: 탭의 배지, 카드 안의 에이전트 행, 문의 배지,
  // 그리고 열려 있다면 보드. 화면 자체는 당김 프레임이 이미 나르므로 여기서
  // 그리는 것은 전부 "어느 에이전트가 무엇을 하고 있는가"뿐이고, 그 답은 한
  // 프레임에 한 번만 있으면 된다. 문의 배지는 언제나 예약된다 — 닫힌 문에서
  // 맞는 것이 그것의 일 전부다.
  scheduleAgentPaint(["tabs", "cards", "badge", "board"]);
  // 그리고 다섯째, 서 있을 때만: 세컨드 브레인의 지식 그래프. 「raw 취합」이
  // 끝나면 볼트에 페이지가 생겼다는 뜻이고, 그 그림은 보고 있는 사람에게만
  // 다시 읽힌다(`noteKnowledgeAgentState`). 이 이벤트에 두 번째 리스너를 매지
  // 않는 것은 그 편이 깨끗해서가 아니라, 이 블록의 **첫 일치**를 읽는 소스 핀이
  // 셋 있어서다.
  noteKnowledgeAgentState(state);
});

/* What one pull brought for one shell, painted once (`pullTermFrames`,
 * shell-term.js).
 *
 * A pull carries a list, oldest first: a floor and the deltas after it, or the
 * frames another window's pulls queued for this one. Every frame reaches the
 * view's model in order; the DOM is told the sum once (`applyFrames`). The
 * state the frames carry beside the rows — the mouse modes, the program's
 * title, the size the pty wears — is the newest frame's word. The bell is not
 * read here: every shell's bell is announced on the round it rang
 * (`term:bell`), and counting it off a frame too would count it twice. */
function applyTermFrames(term, frames) {
  const newest = frames[frames.length - 1];
  noteMouseModes(`term:${term}`, newest);
  // The program's own name for what it is doing — state, so the newest word
  // any of these frames said wins.
  for (let at = frames.length - 1; at >= 0; at -= 1) {
    if (frames[at].title) {
      noteTermTitle(term, frames[at].title);
      break;
    }
  }
  // A frame says nothing about whether the floating shell runs — its open
  // (`setTermVisible`) and its exit do. The push road marked the panel stopped
  // here and got away with it only because a frame rarely raced the open; a
  // pull pays the panel's floor through this very door on every reveal.
  if (term === FLOAT_TERM) {
    floatView.applyFrames(frames);
    return;
  }
  // One view per shell, and the board's peek does not need a second line here:
  // it borrows this very host, so a frame that lands while the dialog is open
  // paints INTO the dialog. Which is the whole point of borrowing rather than
  // copying — there is only ever one screen, wherever it is being read.
  const view = termViews.get(term);
  view?.applyFrames(frames);
  // The pty's own word about its size, folded back into the cache. `termGrids`
  // remembers what this window last SAID; the frame says what the shell
  // actually WEARS, and when the two disagree the saying was lost — believing
  // it anyway is what kept a survivor pane cut at its old width until a window
  // drag moved the numbers. The cache takes the pty's report, and the shell is
  // re-told only if the screen in front of the person still wants something
  // else. On the frame path this is one map compare — the re-telling itself
  // (a layout read) waits for `wornSweep`, so a burst of disagreeing frames
  // costs one measure, not one per frame; the paint road stays free of
  // layout, which `앞에 선 화면에 폭풍…` holds. Only for a shell that still
  // has a screen: a late frame of a dropped one must not plant a cache entry
  // nothing will ever delete again.
  const worn = newest.size;
  if (view && Array.isArray(worn)) {
    const held = termGrids.get(term);
    if (held === undefined || held.rows !== worn[0] || held.cols !== worn[1]) {
      termGrids.set(term, { rows: worn[0], cols: worn[1] });
      wornDiverged.add(term);
      if (wornSweep === 0) wornSweep = setTimeout(sweepWornSizes);
    }
  }
}

/* The shells whose pty reported a size this window never told it, waiting for
 * one re-telling off the frame path. A Set, so a shell diverging on every
 * frame of a burst is still one entry and one measure. */
const wornDiverged = new Set();
let wornSweep = 0;

function sweepWornSizes() {
  wornSweep = 0;
  for (const term of wornDiverged) resizeTermTab(term);
  wornDiverged.clear();
}

/* 셸이 종을 울렸다 — 어느 셸이든, 울린 그 라운드에.
 *
 * 프레임은 이제 화면이 당겨 갈 때만 건너오고, 가려지거나 최소화된 창은 당기지
 * 않는다. 그런데 종은 하필 **배경 탭에서만 뜻이 있는** 소식이다: 앞에 있는 탭은
 * 사람이 이미 보고 있으므로 `noteBell`이 배지를 달지도 않는다. 그래서 펌프가
 * 모든 셸의 종을 울린 라운드에 따로 알리고, 프레임에 실린 같은 종은 세지 않는다
 * (`applyTermFrames`) — 한 번 울린 종은 한 번만 센다. */
listen("term:bell", (event) => {
  noteBell(event.payload.term);
});

/* 같은 이유의 두 번째 줄: 셸의 프로그램이 자기 이름을 바꿨다(OSC 0/2). 이름은
 * 상태라 프레임에도 실려 오지만, 같은 이름을 두 번 적는 것은 아무 일도 아니다
 * (`noteTermTitle`) — 당기지 않는 창의 탭도 이 줄로 제때 이름을 바꾼다. */
listen("term:title", (event) => {
  noteTermTitle(event.payload.term, event.payload.title);
});

/* Local shells, supervised lanes and SSH-backed lanes all arrive here. Their
 * grids share the same OSC 52 parser, so the renderer owns only the live
 * preference gate and the one official system-clipboard adapter. */
listen(TERMINAL_CLIPBOARD_WRITE_EVENT, (event) => {
  const text = event.payload?.text;
  if (typeof text === "string") acceptOsc52ClipboardWrite(text);
});

/* The pump said whether a launch prompt actually landed. Only the failure is
 * worth a sentence: the agent is sitting there with nothing to do and no
 * error anywhere else to explain why — the exact silence the backend emits
 * this event to break. */
listen("term:prompt", (event) => {
  const { term, delivered, pasted, why, text } = event.payload;
  if (delivered) return;
  // A write the window WITHHELD is a different sentence from one that timed
  // out: the guard yielded to something a person owns on that line — their
  // draft, a parked question, a hand that arrived while the words settled —
  // and says so. Words that reached the composer are on screen already;
  // words that did not go to the clipboard like a timed-out prompt's.
  const kept = typeof text === "string" && text.length > 0;
  if (kept) void clipboardText.write(text);
  if (typeof why === "string" && why.length > 0) {
    showError(pasted
      ? t("term.promptLeftUnsubmitted", "터미널 {{term}}에 프롬프트를 붙여넣었지만 보내지는 않았습니다 — {{why}}", { term, why })
      : t("term.promptWithheld", "터미널 {{term}}에 프롬프트를 넣지 않았습니다 — {{why}}", { term, why }));
    return;
  }
  showError(kept
    ? t("term.promptLostCopied", "터미널 {{term}}의 에이전트가 프롬프트를 받지 못했습니다 — 준비되기 전에 시간이 다 됐습니다. 본문을 클립보드에 복사해 두었으니 붙여넣어 주세요.", { term })
    : t("term.promptLost", "터미널 {{term}}의 에이전트가 프롬프트를 받지 못했습니다 — 준비되기 전에 시간이 다 됐습니다. 직접 붙여넣어 주세요.", { term }));
});

/* ---- 중첩 에이전트의 페이지 (1-fk) ----
 *
 * An agent running an agent as a COMMAND got a roster row (1-fh) and the
 * report came straight back: "코덱스 창이 안 보임. 창이 열리면서 보여야
 * 하는데." So a run gets a PAGE now — a tab that opens itself when the run
 * starts, wears the whole command and a running clock, and fills with the
 * text the vendor handed back when it ends. What it cannot show is the run's
 * live screen: the output belongs to the parent agent until the tool call
 * ends, and the page says so in words instead of sitting empty.
 *
 * The template is BUILT rather than written into index.html — the markup
 * file is another session's to edit — so it joins the other built surfaces
 * in `DOC_VIEWS`, which is what puts it in the one list the stage reads when
 * it decides what to show. Seeding it here on its own is what left it
 * `hidden` forever: a page that opens and never appears. */
function buildWorkerView() {
  const root = document.createElement("section");
  root.className = "worker-view";
  // The first leaf's copy answers to this name, exactly as the surfaces the
  // markup declares do — `docHost` strips it from every clone after.
  root.id = "worker-view";
  root.hidden = true;
  return root;
}

/* Runs whose page the person closed — the same run must not reopen it. */
const workerDismissed = new Set();

/* A running worker page speaks elapsed seconds, so its clock advances once
 * per second and stops as soon as no running page remains. */
const WORKER_TICK_MS = 1000;
/* 지난 시간을 낱말로 — 머리의 시계와 「{{time}} 동안 작업」 줄이 같은 함수를
 * 쓴다: 한 페이지에 시간 문법이 두 벌이면 하나는 거짓말이 된다. */
function elapsedWords(ms) {
  const total = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(total / 60);
  return minutes > 0
    ? t("worker.elapsedLong", "{{m}}분 {{s}}초", { m: minutes, s: total % 60 })
    : t("worker.elapsedShort", "{{s}}초", { s: total });
}

function workerElapsedWords(run) {
  return elapsedWords((run.endedAt ?? Date.now()) - run.startedAt);
}

/* The clock alone, so a second's tick never rebuilds the page under a
 * person reading the command. */
function activeRunningWorker() {
  const active = tabs.find((tab) => tab.id === activeTabId);
  return active?.kind === "worker" && active.worker.status === "running" ? active : null;
}

const workerClock = idlePoller({
  wanted: () => activeRunningWorker() !== null,
  every: WORKER_TICK_MS,
  tick: tickWorkers,
});

function tickWorkers() {
  const active = activeRunningWorker();
  if (!active) return;
  const host = docHost(active.pane, "worker");
  const face = host.querySelector(".worker-elapsed");
  if (face) face.textContent = workerElapsedWords(active.worker);
}

function workerStatusWords(run) {
  const state = run.status === "running"
    ? t("worker.live", "실행 중")
    : run.status === "idle" ? t("worker.idle", "대기 중")
      : run.status === "failed" ? t("worker.endedFailed", "실패")
        : run.status === "orphaned" ? t("worker.endedOrphaned", "종료됨")
          : t("worker.ended", "완료");
  const skipped = run.helper?.skipped ? t("worker.recordsSkipped", "") : "";
  return skipped ? `${state} ${skipped}`.trim() : state;
}

/* ---- 헬퍼의 대화 페이지 ----
 *
 * 서브에이전트에게는 자기 pty가 없다. 부모의 프로세스 안에서 돌기 때문이고,
 * 그래서 Orca도 헬퍼는 행으로만 그린다(useWorktreeAgentRows). 그런데 진실의
 * 나머지 절반이 있다: **벤더가 그 헬퍼의 전사를 디스크에 적는다.** 어떤
 * 터미널도 보여 준 적 없는 대화가 이미 파일로 존재하는 것이다 — 이 페이지는
 * 그 파일을 읽는다(백엔드 `subagent_log`, 경로는 우리 기록에서 유도한다).
 *
 * 폴링이지만 **보고 있는 페이지만** 묻는다. 파일은 이벤트를 보내지 않으므로
 * 묻는 것 말고는 길이 없고, 보이지 않는 페이지에 대해 묻는 것은 아무도 읽지
 * 않을 답을 위해 초당 한 번 디스크를 건드리는 일이다. */
const HELPER_POLL_MS = 1000;
/* 페이지가 들고 있는 턴의 상한. 한 시간짜리 헬퍼의 전사를 통째로 DOM에 세우면
 * 그 페이지가 창을 무겁게 만든다 — 사람이 읽는 것은 언제나 끝이다. */
const HELPER_TURN_CAP = 400;

/* 새 턴이 장부에 앉는 유일한 문. DOM 행의 열쇠가 될 seq를 여기서 찍고,
 * 상한을 넘친 만큼만 앞에서 지운다 — 자르는 쪽과 그리는 쪽이 같은 장부를
 * 읽으므로, 화면의 행과 데이터의 턴은 언제나 같은 구간이다
 * (`syncHelperTurns`가 그 열쇠로 지우고 잇는다). */
function holdHelperTurns(held, turns) {
  const now = Date.now();
  for (const turn of turns) {
    // 벤더가 줄에 찍은 시각이 있으면 그것(`TranscriptTurn.at_ms`), 없으면
    // 도착한 순간 — 「{{time}} 동안 작업」의 시간은 이 시각들의 차다.
    if (turn.role === "tool_result" && turn.tool?.call_id) {
      const call = held.turns.findLast((one) => one.role === "tool" &&
        one.tool?.call_id === turn.tool.call_id && one.output === undefined);
      if (call) {
        call.output = turn.text;
        call.outputError = turn.tool.is_error === true;
        call.outputAt = turn.at_ms ?? now;
        // An edit the result describes and the call did not (ACP hands the
        // diff over on the update) dresses the call's row.
        if (turn.tool.edits?.length > 0 && !(call.tool?.edits?.length > 0)) {
          call.tool = { ...(call.tool ?? {}), edits: turn.tool.edits };
        }
        held.seq += 1;
        continue;
      }
    }
    // A thought lasted until the next thing happened — the extension's
    // 「Thought for 3s」. Only between stamps of one kind: the file's own
    // clock and the window's are not the same clock.
    // A length of nothing is no length: turns held in one read share one
    // clock reading, and a file can stamp two lines alike.
    const last = held.turns.at(-1);
    const fromClock = turn.at_ms === undefined;
    if (last?.role === "thinking" && last.thoughtMs === undefined && last.fromClock === fromClock) {
      const lasted = (turn.at_ms ?? now) - last.at;
      if (lasted > 0) last.thoughtMs = lasted;
    }
    held.turns.push({ role: turn.role, text: turn.text, tool: turn.tool ?? null,
      seq: held.seq, at: turn.at_ms ?? now, fromClock });
    held.seq += 1;
  }
  if (held.turns.length > HELPER_TURN_CAP) {
    held.turns.splice(0, held.turns.length - HELPER_TURN_CAP);
  }
}

/* 이 헬퍼의 페이지를 연다 — 벤더가 전사를 남겼을 때에만.
 *
 * 남기지 않았으면 행은 늘 하던 일을 한다(부모 판으로). 물어보고 나서 여는
 * 이유가 그것이다: 열리지 않을 문을 그려 두는 것보다 눌러 보고 아무 일도
 * 일어나지 않는 편이 낫다는 규칙은 이 창에 이미 있다(죽은 컨트롤 금지). */
async function openHelperPage(row, sub) {
  const tabId = `helper:${row.term}:${sub.id}`;
  const held = tabs.find((tab) => tab.id === tabId);
  if (held) {
    setActiveTab(held.id);
    return;
  }
  let first = null;
  try {
    first = await invoke("subagent_log", { term: row.term, id: sub.id, after: 0 });
  } catch {
    first = null;
  }
  if (!first?.found) {
    void focusAgentPane(row.worktree, row.tab?.id ?? null, row.term, row.agent);
    return;
  }
  const helper = { id: sub.id, next: first.next ?? 0, turns: [], seq: 0, skipped: first.skipped === true };
  holdHelperTurns(helper, first.turns ?? []);
  openTab({
    id: tabId,
    kind: "worker",
    worker: {
      id: tabId,
      term: row.term,
      agent: row.agent ?? "claude",
      name: sub.name,
      command: "",
      status: sub.state === "done" ? "done" : "running",
      toolCalls: sub.tool_calls ?? 0,
      startedAt: Date.now(),
      endedAt: sub.state === "done" ? Date.now() : null,
      output: null,
      truncated: false,
      helper,
    },
  });
  helperClock.sync();
}

/* A CLI driven on its wire — Codex `app-server`, Gemini CLI `--acp` — has no
 * screen, so its tab IS its conversation: the helper page's skeleton, fed by
 * `wire_log` the way a helper's is fed by its transcript, its composer
 * sending down the wire (`wire_send`), its questions answered from the card
 * (`wire_answer`). docs/design/agent-wire-sessions-20260915.md. */
/* Open a wire session as a tab. A fresh one from the composer's menu, or —
 * with `resume` and `fromTerm` — the CONTINUATION of a pane's own conversation:
 * the wire resumes the pane's session, the backend tells the pane's CLI to
 * exit, and the new tab stands where the pane's tab stood (`at`) carrying the
 * turns already said (`history`), so the swap reads as the same conversation
 * changing renderer, not as a new page. */
async function openWirePage(agent, cwd, { resume = null, fromTerm = null, at = -1, history = [], label = null, refused = showError } = {}) {
  let started;
  try {
    started = await invoke("wire_start", { agent, cwd, resume, from_pane: fromTerm });
  } catch (error) {
    refused(error);
    return null;
  }
  const tabId = `wire:${started.id}`;
  const worker = {
    id: tabId,
    wire: started.id,
    session: started.session ?? null,
    term: null,
    agent,
    name: label ?? agentName(agent),
    command: "",
    status: "idle",
    toolCalls: 0,
    startedAt: Date.now(),
    endedAt: null,
    output: null,
    truncated: false,
    cwd,
    helper: { id: WIRE_LOG_ID, next: 0, turns: [], seq: 0, skipped: false, found: true },
    wireLog: {
      status: "idle",
      asks: [],
      live: [],
      model: started.model ?? null,
      models: [],
      mode: null,
      modes: [],
      commands: [],
      version: started.version ?? null,
      protocol: started.protocol,
    },
  };
  if (history.length) holdHelperTurns(worker.helper, history);
  openTab({ id: tabId, kind: "worker", worker });
  if (at >= 0) placeTabAt(tabId, at);
  helperClock.sync();
  paintViewToggle();
  return tabId;
}

/* Move a tab to an index on the strip — the seat another tab holds, so a tab
 * that replaces one can take its place before the old one goes. */
function placeTabAt(id, at) {
  const from = tabs.findIndex((tab) => tab.id === id);
  if (from < 0 || at < 0 || at === from || at >= tabs.length) return;
  tabs.splice(at, 0, ...tabs.splice(from, 1));
  renderTabs();
}

/* The whole transcript a pane has so far, read through the same door the
 * page polls — chunk by chunk until the file's end — for the wire tab that
 * continues the conversation to open with the words already said. */
async function readPaneTranscript(term) {
  const turns = [];
  let after = null;
  for (let reads = 0; reads < 64; reads += 1) {
    let more = null;
    try {
      more = await invoke("pane_log", { term, after });
    } catch {
      break;
    }
    // Nothing past the cursor: the file is read to its end.
    if (!more?.found || !(after === null || more.next > after) || !more.turns?.length) break;
    turns.push(...more.turns);
    if (more.more === false) break;
    after = more.next;
  }
  return turns.slice(-HELPER_TURN_CAP);
}

/* The wire an agent's pane can hand its conversation to: the catalog row
 * with a wire that resumes a session and an exit command the window can
 * type (`wire_resumes`). */
function wireRowFor(term) {
  const row = installedAgents().find((one) => one.id === paneAgents.get(term));
  return row?.wire && row.wire_resumes ? row : null;
}

/* Hand a pane's conversation to its wire — the toggle's 「대화」 on a Claude
 * Code pane. Same session, other renderer: the wire resumes the pane's
 * session id, the pane's CLI exits, and the tab that stood here is the
 * conversation, streaming the way the CLI's own panel does. Only between
 * turns: a pane mid-turn keeps its transcript view and is handed over when
 * the turn ends (`pendingWire`). False when this pane cannot be handed
 * over — no wire, no session yet, mid-turn, or the start failed — and the
 * caller falls back to the transcript view. */
async function handPaneToWire(term) {
  const row = wireRowFor(term);
  const session = paneSessions.get(term)?.session?.id ?? null;
  if (!row || !session) return false;
  const held = paneChatOf(term);
  if (held.handing) return true;
  if (isMidTurn(hookStates.get(term))) {
    held.pendingWire = true;
    return false;
  }
  held.pendingWire = false;
  held.handing = true;
  try {
    const owner = tabOfTerm(term);
    const history = await readPaneTranscript(term);
    const tabId = await openWirePage(row.id, owner?.worktree ?? activeWorktreePath, {
      resume: session,
      fromTerm: term,
      at: owner ? tabs.indexOf(owner) : -1,
      history,
      label: owner ? tabLabel(owner) : null,
      // The refusal stays on the page (`paintPaneChat`), not only in a toast
      // — the backend logs it too (`refusal_line`), so the log and the page
      // give the same reason.
      refused: (reason) => { held.wireRefusal = String(reason?.message ?? reason); },
    });
    if (tabId === null) paintPaneChat(term);
    return tabId !== null;
  } finally {
    held.handing = false;
  }
}

/* The other way: the wire tab's 「화면」. The CLI's own screen resumes the
 * same session in a new pane that takes the wire tab's seat, and the wire
 * closes (its tab's close stops the child). */
async function handWireToScreen(tab) {
  const run = tab.worker;
  const at = tabs.indexOf(tab);
  try {
    const term = await launchAgentTab({
      agent: run.agent,
      prompt: "",
      ...spawnGrid({ placement: "tab" }),
      resume: run.session,
    });
    mountTermTab(term, { agent: agentName(run.agent) }, { placement: "tab" });
    const opened = tabOfTerm(term);
    if (opened && at >= 0) placeTabAt(opened.id, at);
    closeTab(tab.id);
  } catch (error) {
    showError(error);
  }
}

/* The wire tab in front that continues a pane's conversation — the tab the
 * view toggle speaks for when no agent pane does. */
function activeWireTab() {
  const tab = currentTab();
  return tab?.kind === "worker" && tab.worker.wire && tab.worker.session ? tab : null;
}

/* The wire's status in the run's words: out or asking is running (the status
 * line and the live dots), between turns idle, a wire that died or closed
 * failed or done. */
function wireRunStatus(status) {
  if (status === "working" || status === "asking" || status === "starting") return "running";
  if (status === "failed") return "failed";
  if (status === "ended") return "done";
  return "idle";
}

/* Keep a `wire_log` answer's state on the run; true when something the page
 * or the composer draws from it changed. */
function holdWireState(run, log) {
  const before = run.wireLog
    ? JSON.stringify([run.wireLog.status, run.wireLog.asks, run.wireLog.model, run.wireLog.mode,
      run.wireLog.modes, run.wireLog.commands, run.wireLog.live, run.wireLog.usage ?? null])
    : "";
  run.wireLog = log;
  run.status = wireRunStatus(log.status);
  if (log.status === "ended" && run.endedAt === null) run.endedAt = Date.now();
  run.toolCalls = run.helper.turns.filter((turn) => turn.role === "tool").length;
  return before !== JSON.stringify([log.status, log.asks, log.model, log.mode, log.modes, log.commands,
    log.live, log.usage ?? null]);
}

/* A wire session said it has something new — a delta of what it is saying, a
 * turn, a question, its status: the page on it reads at once instead of on
 * its next tick. That is what makes the words stream the way Claude Code's
 * own panel streams them; the poll itself is unchanged, only its timing.
 * Another session's news, or a page that is not on screen, waits for the
 * tick as before. */
listen("wire:update", (event) => {
  const tab = activeHelperPage();
  if (tab?.worker.wire === event.payload?.id) void pollHelperPages();
});

/* A wire's question in the card's shape: an approval with the wire's own
 * options (the agent's words where it gave them — ACP names its options —
 * the decision's kind otherwise, worded by the window), or a question list. */
function wireAskShape(run, ask) {
  const approval = ask.kind === "approval"
    ? { tool: ask.tool, summary: ask.summary ?? null, plan: ask.plan ?? null, edits: ask.edits ?? [], options: ask.options ?? [] }
    : null;
  const askPrompt = ask.kind === "question"
    ? {
      questions: (ask.questions ?? []).map((question) => ({
        question: question.question,
        header: question.header,
        options: (question.options ?? []).map((option) => ({ label: option.label, description: option.description })),
        multi: false,
      })),
    }
    : null;
  return { pane: `wire:${run.wire}`, agent: run.agent, approval, ask_prompt: askPrompt };
}

/* The card a question stands in, under the transcript and above the composer
 * — the pane's conversation and a wire's page both stand theirs here. */
function standAskCard(host, panel) {
  let card = host.querySelector(".pane-chat-ask");
  if (!card) {
    card = document.createElement("section");
    card.className = "pane-chat-ask";
    card.setAttribute("aria-label", t("board.tasks.needsInput", "진행하려면 확인이 필요해요."));
  }
  card.replaceChildren(panel);
  // Inside the dock, above the composer — the extension's card stands in its
  // `inputContainer`; a page without a composer keeps it under the list.
  const dock = host.querySelector(".chat-dock");
  const home = dock ?? host;
  if (card.parentElement !== home) {
    const anchor = dock?.querySelector(":scope > .worker-composer") ?? null;
    if (anchor) anchor.before(card);
    else home.appendChild(card);
  }
  return card;
}

/* The question a wire agent asked, as the card the pane's conversation shows
 * (`paintPaneAsk`) — the panel is the board's, the options the wire's, and
 * the answer goes back down the wire with the option's id or the answers per
 * question (`wire_answer`). Rebuilt when the question changes or the panel
 * asks; never on a quiet poll. */
function paintWireAsk(host, run, force = false) {
  const ask = run.wireLog?.asks?.[0] ?? null;
  const card = host.querySelector(".pane-chat-ask");
  if (!ask) {
    card?.remove();
    run.askDraft = null;
    return;
  }
  const sig = JSON.stringify(ask.id);
  if (run.askDraft?.sig !== sig) {
    const shape = wireAskShape(run, ask);
    run.askShape = shape;
    run.askDraft = askDraftOf(shape, sig, {
      repaint: () => paintWireAsk(host, run, true),
      base: helperBase(run),
      // `message` is the words a refusal carries where the protocol has a
      // place for them — the plan card's feedback field.
      answer: (choice, message = null) => invoke("wire_answer", {
        id: run.wire,
        ask: ask.id,
        ...(ask.kind === "question"
          ? {
            answers: choice.map((one, index) => [
              ...one.indices.map((at) => shape.ask_prompt.questions[index].options[at]?.label ?? ""),
              one.other,
            ].filter(Boolean)),
          }
          // The words only ride a refusal that has some; a plain answer is
          // the same request it always was.
          : { option: choice, ...(message ? { message } : {}) }),
      }),
    });
    force = true;
  }
  if (card && !force) return;
  const panel = run.askShape.ask_prompt
    ? askPanelNode(run.askShape, run.askDraft)
    : approvalPanelNode(run.askShape, run.askDraft);
  standAskCard(host, panel);
}

/* A helper's open page learns that its helper finished from the roster, the
 * way its row does: the tail tool line stops saying "실행 중", and the head
 * says done. A row the roster no longer carries at all (the session turned,
 * the pane left) is finished too — nothing will be written to it again. */
function syncHelperPagesWith(term, rows) {
  for (const tab of tabs) {
    if (tab.kind !== "worker" || !tab.worker.helper || tab.worker.term !== term) continue;
    const row = rows?.find((one) => one.id === tab.worker.helper.id);
    const status = row && row.state !== "done" ? "running" : "done";
    // 도구 수도 명부가 나르는 것이다. 상태는 한 번만 바뀌지만 이 수는 도구마다
    // 오르므로, 상태만 보고 있으면 페이지 머리가 처음 받은 수에 굳는다. 명부가
    // 더 이상 이 헬퍼를 나르지 않으면(세션이 넘어갔다) 마지막으로 안 수를
    // 지킨다 — 끝난 헬퍼가 0으로 되돌아가지 않게.
    const uses = row?.tool_calls ?? tab.worker.toolCalls ?? 0;
    if (tab.worker.status === status && tab.worker.toolCalls === uses) continue;
    if (tab.worker.status !== status) tab.worker.endedAt = status === "done" ? Date.now() : null;
    tab.worker.status = status;
    tab.worker.toolCalls = uses;
    if (tab.id === activeTabId) paintWorkerView(tab);
  }
}

/* ---- a pane's conversation view ------------------------------------------
 *
 * A terminal tab can wear its conversation instead of its screen (the design
 * the person approved 2026-09-15: the title bar's segment, the same slot, a
 * 220ms fade). The pty and the tab stay; the screen's host hides — and a
 * hidden view paints nothing, as every hidden view here does — and a chat
 * page stands in the slot, fed from the transcript the agent reported when it
 * started (`pane_log`). ONE page implementation: the helper page's skeleton,
 * turns, head and composer, so the two surfaces cannot drift — only where the
 * turns come from differs (`helperLog`). A question the program asks on its
 * own screen brings the screen back (`hook:agent`), because that question is
 * not in the transcript yet and a page standing over it would hide it. */
const PANE_LOG_ID = "pane";
/* A wire session's page reads `wire_log` — the same log shape a pane's
 * transcript comes in, with the session's own state beside the turns. */
const WIRE_LOG_ID = "wire";
const paneChats = new Map();

function paneChatOf(term) {
  let held = paneChats.get(term);
  if (held) return held;
  const run = {
    id: `chat:${term}`,
    term,
    agent: paneAgents.get(term) ?? "claude",
    name: "",
    command: "",
    status: "running",
    toolCalls: 0,
    startedAt: paneBorn.get(term) ?? Date.now(),
    endedAt: null,
    output: null,
    truncated: false,
    /* `next: null` is the TAIL: the first read starts at the transcript's last
     * chunk, not its first byte — a pane hours into a session has tens of
     * megabytes behind it, and a view that read them all painted the whole
     * past as if it were arriving now (09-16: 66 MB, minutes of old turns
     * scrolling in). What stands above the tail is said to be folded. */
    helper: { id: PANE_LOG_ID, next: null, turns: [], seq: 0, skipped: false, found: null, folded: false },
  };
  held = {
    on: false,
    arriving: null,
    host: null,
    run,
    // The page's tab-shaped record: what `paintHelperPage` and the poll read.
    // `pane` is its own key, so its screen bookkeeping never touches a worker's.
    tab: { id: run.id, kind: "chat", pane: run.id, term, worker: run },
  };
  paneChats.set(term, held);
  return held;
}

function paneChatOn(term) {
  return paneChats.get(term)?.on === true;
}

async function setPaneChat(term, on) {
  const held = paneChatOf(term);
  if (held.on === on) {
    // 「대화」 pressed on a conversation already standing as the transcript
    // view: the wire is asked for again — a session the hooks had not yet
    // named, a wire that was refused, may be there now. Until 2026-09-21 a
    // pane that once fell to the transcript view never tried the wire again.
    if (on && !held.handing) void handPaneToWire(term);
    return;
  }
  if (!on) held.wireRefusal = null;
  // A pane whose agent can be driven on a wire is handed over to it: the
  // conversation view IS the wire then, streaming. Otherwise — or until the
  // turn ends — the transcript view stands.
  if (on && await handPaneToWire(term)) return;
  held.on = on;
  held.arriving = on ? "chat" : "term";
  updateStage();
  paintViewToggle();
  helperClock.sync();
  // The first turns now, not on the poll's next beat: the page a person just
  // asked for should not stand empty for a second.
  if (on) void pollHelperPages();
}

function forgetPaneChat(term) {
  const held = paneChats.get(term);
  if (!held) return;
  dropWorkerScreen(held.tab.pane);
  held.host?.remove();
  // 기다리던 글들은 이 대화의 것이었다 — 대화가 사라지면 같이 사라진다.
  held.run.queue = null;
  paneChats.delete(term);
  paneLive.delete(term);
  paneUsage.delete(term);
}

/* The active pane's term when it runs an agent — the pane the toggle speaks
 * for. `null` for a plain shell, a document tab, nothing on stage. */
function activeAgentTerm() {
  const active = currentTab();
  if (active?.kind !== "term") return null;
  const term = activePaneOf(active);
  return term !== null && paneAgents.has(term) ? term : null;
}

function paintViewToggle() {
  const seg = el("view-toggle");
  const term = activeAgentTerm();
  const wire = activeWireTab();
  seg.hidden = term === null && wire === null;
  if (seg.hidden) return;
  // A wire tab that continues a pane's conversation IS its 「대화」.
  const on = wire !== null || paneChatOn(term);
  for (const [id, pressed] of [["view-toggle-term", !on], ["view-toggle-chat", on]]) {
    const button = el(id);
    button.classList.toggle("is-active", pressed);
    button.setAttribute("aria-pressed", String(pressed));
  }
}

el("view-toggle-term").addEventListener("click", () => {
  const wire = activeWireTab();
  if (wire) {
    void handWireToScreen(wire);
    return;
  }
  const term = activeAgentTerm();
  if (term !== null) void setPaneChat(term, false);
});
el("view-toggle-chat").addEventListener("click", () => {
  const term = activeAgentTerm();
  if (term !== null) void setPaneChat(term, true);
});

/* Stand the pane's conversation in its slot, or take it down — called from the
 * stage's per-slot pass, so a page is shown exactly when its screen would have
 * been and the pane's tab is on stage. The surface that arrives wears the fade. */
function placePaneChat(term, slot, shown) {
  const held = paneChats.get(term);
  if (!held) return;
  if (shown) {
    if (!held.host) {
      held.host = document.createElement("section");
      held.host.className = "worker-view pane-chat";
      held.host.dataset.term = String(term);
      held.host.setAttribute("aria-label", t("worker.paneTranscript", "이 판의 대화"));
    }
    if (held.host.parentElement !== slot) slot.appendChild(held.host);
    held.host.hidden = false;
    if (held.arriving === "chat") arriveView(held.host);
    held.arriving = null;
    paintPaneChat(term);
    return;
  }
  if (held.host) held.host.hidden = true;
  if (held.arriving === "term") {
    const view = termViews.get(term);
    if (view && !view.host.hidden) arriveView(view.host);
  }
  held.arriving = null;
}

function arriveView(node) {
  node.classList.remove("is-arriving");
  // Restart the animation on a node that wore it a moment ago.
  void node.offsetWidth;
  node.classList.add("is-arriving");
  node.addEventListener("animationend", () => node.classList.remove("is-arriving"), { once: true });
}

/* What the pane is doing, in the run's words — the hook's state, not the
 * "running" a page is born with: the status line's mark and a tool row's
 * live dot breathe only while a turn is out. A turn that ended is idle; an
 * agent that left is done. */
function paneChatStatus(state) {
  if (isMidTurn(state)) return "running";
  return state === "idle" ? "done" : "idle";
}

function paintPaneChat(term) {
  const held = paneChats.get(term);
  if (!held?.host || held.host.hidden) return;
  const run = held.run;
  run.agent = paneAgents.get(term) ?? run.agent;
  run.name = agentName(run.agent);
  const status = paneChatStatus(hookStates.get(term));
  // The head's clock is THIS turn's: a state change is stamped as it arrives
  // (`hookStamps`), so the count starts where the turn did — not where the
  // pane was born, which a view opened on an hour-old pane would otherwise
  // report as an hour of work. A pane already mid-turn when the view opens
  // takes the turn's stamp on its first paint.
  const stamp = hookStamps.get(term) ?? null;
  const moved = run.status !== status;
  if (moved || (status === "running" && stamp !== null && run.startedAt !== stamp)) {
    run.status = status;
    if (status === "running") {
      run.startedAt = stamp ?? Date.now();
      run.endedAt = null;
    } else {
      run.endedAt = stamp ?? Date.now();
    }
  }
  paintHelperPage(held.host, held.tab);
  // The composer's send is a stop while the turn is out and the send again
  // when it ends — the state it reads (`run.status`) moved here, so it is
  // repainted here; a hook's chip repaint ran before the state landed.
  if (moved) syncWorkerComposers(run);
  if (moved) settleComposerQueue(run);
  paintPaneAsk(held);
  // Said once, where the turns would be: the agent has not named a transcript.
  noticeOnPaneChat(
    held,
    "worker.noPaneTranscript",
    run.helper.found === false,
    t("worker.noPaneTranscript", "이 판의 대화 기록을 아직 읽을 수 없습니다 — 에이전트가 전사의 자리를 알려 준 뒤에 보입니다."),
  );
  // A conversation asked for mid-turn is handed to the wire when the turn
  // ends (`handPaneToWire`); until then the page says so, where a person
  // would otherwise wonder why nothing streams yet.
  noticeOnPaneChat(
    held,
    "worker.handoverPending",
    held.pendingWire === true,
    t("worker.handoverPending", "이 턴이 끝나면 대화가 실시간 세션(선)으로 이어집니다 — 그때부터 답이 낱말 단위로 흐릅니다."),
  );
  // A hand-over the backend refused (a worker's pane, a wire that failed to
  // stand, a screen that kept its conversation): the page says why, in the
  // backend's words, and stays the transcript view — 「대화」 again asks again.
  noticeOnPaneChat(
    held,
    "worker.wireRefused",
    typeof held.wireRefusal === "string" && held.wireRefusal !== "",
    t("worker.wireRefused", "실시간 세션(선)으로 넘기지 못했습니다: {{reason}} — 전사 뷰로 봅니다(답은 턴이 닫힐 때 한 번에). 「대화」를 다시 누르면 다시 시도합니다.", { reason: held.wireRefusal ?? "" }),
  );
  noticeOnPaneChat(
    held,
    "worker.historyFolded",
    run.helper.folded === true,
    t("worker.historyFolded", "긴 기록의 끝부분만 보입니다 — 이전 턴은 판의 화면과 세션 기록에 있습니다."),
  );
}

/* One line said where the turns would be, keyed by what it says: each notice
 * stands or goes on its own condition, so two of them never take each other
 * for the one already there. The words arrive translated (`t(key, …)` at the
 * call, where the catalog contract reads them); the key names the line. */
function noticeOnPaneChat(held, key, wanted, words) {
  const shown = held.host.querySelector(`.pane-chat-notice[data-says="${key}"]`);
  if (!wanted) {
    shown?.remove();
    return;
  }
  if (shown) {
    // The same line with new words (a refusal's reason): the words change,
    // the line stays.
    writeTextContent(shown, words);
    return;
  }
  const said = document.createElement("p");
  said.className = "pane-chat-notice";
  said.dataset.says = key;
  said.textContent = words;
  held.host.querySelector(".helper-turns")?.before(said);
}

/* The question the pane's program is asking, as the extension's card in the
 * conversation — under the transcript, above the composer. The panel is the
 * board's own (`approvalPanelNode` / `askPanelNode`); what differs is the
 * surface it stands on, the repaint it asks for, and the third road a
 * permission card has here: decline and say what to do instead, the composer
 * taking the words. Built when the question arrives or changes and on the
 * panel's own repaints — never on the poll, whose paint must touch nothing. */
function paintPaneAsk(held, force = false) {
  const term = held.run.term;
  const ask = paneAsks.get(term);
  const card = held.host.querySelector(".pane-chat-ask");
  if (!ask) {
    card?.remove();
    held.askDraft = null;
    return;
  }
  if (held.askDraft?.sig !== ask.sig) {
    held.askDraft = askDraftOf(ask, ask.sig, {
      repaint: () => paintPaneAsk(held, true),
      base: helperBase(held.run),
      instead: () => held.host.querySelector(".worker-composer-box")?.focus(),
    });
    force = true;
  }
  if (card && !force) return;
  const shape = { pane: `term:${term}`, agent: ask.agent, approval: ask.approval, ask_prompt: ask.ask_prompt };
  const panel = ask.ask_prompt ? askPanelNode(shape, held.askDraft) : approvalPanelNode(shape, held.askDraft);
  standAskCard(held.host, panel);
}

function paintHelperSurface(tab) {
  if (tab.kind === "chat") paintPaneChat(tab.term);
  else paintWorkerView(tab);
}

function activeHelperPage() {
  const active = tabs.find((tab) => tab.id === activeTabId);
  if (active?.kind === "worker" && active.worker.helper) return active;
  // A terminal tab wearing its conversation is the same kind of page: the
  // poll feeds whichever page is on screen.
  if (active?.kind === "term") {
    const term = activePaneOf(active);
    const held = term === null ? null : paneChats.get(term);
    if (held?.on) return held.tab;
  }
  return null;
}

const helperClock = idlePoller({
  wanted: () => activeHelperPage() !== null,
  every: HELPER_POLL_MS,
  tick: () => void pollHelperPages(),
});

async function pollHelperPages() {
  const tab = activeHelperPage();
  if (!tab) return;
  const held = tab.worker.helper;
  // News that arrives while a read is out is not lost: the read that is out
  // cannot carry it, so one more follows it (`wire:update` between polls).
  if (held.polling) {
    held.again = true;
    return;
  }
  held.polling = true;
  const after = held.next;
  let more = null;
  try {
    // Asked HERE, by name: a helper's file under the pane, or the pane's own
    // transcript. The harness reads the clock's own body to learn what it
    // asks (`derivePollerCommands`) and parks exactly that while a test holds
    // the pollers — a body that only called out to a helper let it walk on
    // into the page painters and park the composer's send and the launch
    // road with them (the leak test's first `openTermTab` never returned).
    more = held.id === PANE_LOG_ID
      ? await invoke("pane_log", { term: tab.worker.term, after })
      : held.id === WIRE_LOG_ID
        ? await invoke("wire_log", { id: tab.worker.wire, after })
        : await invoke("subagent_log", { term: tab.worker.term, id: held.id, after });
  } catch {
    return;
  } finally {
    held.polling = false;
    if (held.again) {
      held.again = false;
      queueMicrotask(() => void pollHelperPages());
    }
  }
  if (!more?.found) {
    // Said once on the page, then left alone: the poll keeps asking, because
    // the agent may name its transcript on a later report.
    if (held.found !== false) {
      held.found = false;
      if (activeHelperPage() === tab) paintHelperSurface(tab);
    }
    return;
  }
  held.found = true;
  // 전사가 말한 컨텍스트 사용량. 판의 길에서만 담는다 — 헬퍼의 전사는
  // 부모 판이 서 있는 자리가 아니고, 그것을 판의 값으로 적으면 칩이 남의
  // 수를 이 판의 것이라 말하게 된다.
  const usageMoved = held.id === PANE_LOG_ID && more.usage
    ? rememberPaneUsage(tab.worker.term, more.usage)
    : false;
  // The transcript names the model that wrote it. Only taken where nothing
  // has said yet: a hook that carries one is the pane's live word and this is
  // the file's memory of it, which is a turn behind whenever both speak.
  if (more.model && tab.worker.term !== undefined && !paneModels.get(tab.worker.term)) {
    paneModels.set(tab.worker.term, more.model);
    if (activeHelperPage() === tab) paintHelperSurface(tab);
  }
  const replaced = after !== null && more.next < after;
  if (replaced) { held.turns.length = 0; held.skipped = false; }
  // The first read of a pane's transcript began at its tail: the turns above
  // are not carried, and the page says so once (`paintPaneChat`).
  if (more.folded === true) held.folded = true;
  const skippedNow = !held.skipped && more.skipped === true;
  held.skipped = held.skipped || skippedNow;
  // Retain the answer before advancing the cursor, even if the person changed
  // tabs while the read was in flight. Visibility governs paint, not delivery.
  if (more.turns?.length) holdHelperTurns(held, more.turns);
  held.next = more?.next ?? held.next;
  // A wire's log carries the session's state beside its turns — status, the
  // open question, model, mode, commands. It lives on the run (the page and
  // the composer read it there), and a change repaints with no new turn.
  const wireChanged = held.id === WIRE_LOG_ID && holdWireState(tab.worker, more);
  if (document.hidden || activeHelperPage() !== tab) return;
  if (!more?.turns?.length && !replaced && !skippedNow && !wireChanged && !usageMoved) return;
  if (wireChanged) syncWorkerComposers(tab.worker);
  if (wireChanged) settleComposerQueue(tab.worker);
  // 컨텍스트가 움직였다 — 미터만 갈아입는다(턴은 그대로일 수 있다).
  if (usageMoved) syncWorkerComposers(tab.worker);
  // 새로 온 것이 있을 때에만 다시 그린다 — 쉬는 페이지는 아무 값도 치르지
  // 않고, 그림도 온 턴만 잇는다(`syncHelperTurns`).
  paintHelperSurface(tab);
  // The file goes on past this chunk (a page opened late on a long
  // transcript): read on now, chunk after chunk, rather than one chunk a
  // beat — the history a person asked for must not stream in as if it were
  // being said. Only while the cursor moves; a chunk that yielded nothing
  // (one oversized line) waits for the beat like any other.
  if (more.more === true && (after === null || more.next > after)) {
    queueMicrotask(() => void pollHelperPages());
  } else if (activeHelperPage() === tab) {
    const term = tab.worker?.term;
    const working = term !== undefined && (hookStates.get(term) === "working" || isMidTurn(hookStates.get(term)));
    if ((working || (more?.turns?.length ?? 0) > 0) && !held.quickFollow) {
      held.quickFollow = true;
      setTimeout(() => {
        held.quickFollow = false;
        if (activeHelperPage() === tab) void pollHelperPages();
      }, 160);
    }
  }
}

/* 어느 체크아웃의 무대에 앉힐지 — 그 체크아웃의 나무가 아는 자리 하나.
 *
 * 판 그룹 번호는 체크아웃마다 따로 사는 나무의 것이다(`stageTrees`). 그래서
 * 다른 체크아웃의 번호를 물려받은 탭은 `paneTabs`의 두 조건 중 하나를 영원히
 * 통과하지 못한다 — 있지만 어느 줄에도 없는 탭이 된다. 나무가 아직 없으면
 * 앞에 있는 자리로 둔다: 그 체크아웃이 처음 열릴 때 나무가 생기고, 그때
 * 있어야 할 곳에 있게 된다. */
function homeStageSeat(worktree) {
  const tree = stageTrees.get(worktree);
  if (!tree) return focusedPane;
  return stageGroups(tree)[0] ?? focusedPane;
}

/* 미러가 가능한지, 한 번 묻고 창의 수명 동안 들고 있는다.
 *
 * 답이 "없음"이면 중첩 벤더 실행의 페이지는 **도는 동안 보여줄 것이 없다** —
 * 미러 shim이 사라진 바이너리를 가리키면 조용히 진짜 에이전트로 넘어가도록
 * 설계돼 있기 때문이다(그 폴백이 터미널을 살린다). 그때 사람이 보는 것은 명령
 * 한 줄과 빈 화면이고, 조용한 실행과 고장 난 배관을 구별할 방법이 없다.
 *
 * 모를 때는 말하지 않는다: 물어보는 데 실패하면 "있음"으로 둔다 — 없다고
 * 잘못 말하는 것보다 조용한 편이 낫다. */
let mirrorReady = null;

async function askMirrorReady() {
  if (mirrorReady !== null) return mirrorReady;
  try {
    mirrorReady = Boolean(await invoke("mirror_ready"));
  } catch {
    mirrorReady = true;
  }
  return mirrorReady;
}

/* 대화가 접는 기준 — 한 눈보다 긴 것(여러 줄이거나 문단 하나를 넘는
 * 길이)만 접는다. 헬퍼의 브리핑·도구 덤프·중첩 실행의 명령이 전부 이
 * 하나를 쓴다 — 접는 자가 셋이라고 기준이 세 벌일 이유는 없다. */
const CHAT_FOLD_LIMIT = 200;

function chatFolds(text) {
  return text.includes("\n") || text.length > CHAT_FOLD_LIMIT;
}

/* 접힘의 몸 하나: 요약 줄(cap)과, 펼치면 나오는 전문(body).
 * 헬퍼의 브리핑, 긴 도구 덤프, 중첩 실행의 exec 카드와 결과가 전부 이
 * 하나를 입는다 — 접는 자가 넷이어도 접힘의 문법은 한 벌이다(Orca의
 * 실행 뷰가 명령과 긴 답을 접는 바로 그 문법). <details>라서 키보드가
 * 그냥 닿는다 — summary는 제 발로 초점을 받고 Enter로 여닫힌다. */
function foldCardNode(kind, cap, body, open = false) {
  const fold = document.createElement("details");
  fold.className = kind;
  fold.open = open;
  const lid = document.createElement("summary");
  lid.className = "helper-cap";
  lid.append(...cap);
  fold.append(lid, body);
  return fold;
}

/* 요약 줄의 세 조각. 라벨과 상태어는 창의 글, 발췌만 기계의 글(모노)이다. */
function foldWhoNode(words) {
  const who = document.createElement("span");
  who.className = "helper-who";
  who.textContent = words;
  return who;
}

function foldCueNode(words) {
  const cue = document.createElement("span");
  cue.className = "helper-cue";
  cue.textContent = words;
  return cue;
}

function foldBodyNode(text) {
  const body = document.createElement("pre");
  body.className = "helper-fold-body";
  body.textContent = text;
  return body;
}

/* ---- the Claude Code grammar --------------------------------------------
 *
 * The page reads the way the Claude Code extension's panel does
 * (docs/design/agent-conversation-claude-code-grammar-20260915.md): every
 * turn the agent makes is one row of the same two-column grid — a mark in
 * the gutter, the words beside it. The answer's mark is a quiet dot; a tool
 * call is `● Read ui/shell.js` with `└ …` under it, its dot the state of the
 * call; a thought folds behind its own heading; and while the run is out the
 * status line under the transcript says what the CLI's own screen would
 * (`✻ Pondering…` — `agentVoice`). Only the accent changes per agent
 * (`data-agent` on the page → `--chat-accent`); the four the catalog voices
 * wear their own, everyone else the window's one accent and one mark. */

/* The tool's own name and its target, read off the turn the way the
 * transcript wrote it — `name · target` on the first line
 * (`transcript::tool_line`), or the vendor's call record when the turn
 * carries one. The name stands as the CLI wrote it: the window's verb
 * vocabulary (`activityWord`) is the sidebar row's, not this page's. A dump
 * with no name is a tool all the same, and its first line is the target. */
function toolWords(turn) {
  if (turn.role === "tool_result") return { name: turn.tool?.name ?? "", arg: "", input: "" };
  const head = turn.text.split("\n", 1)[0];
  const cut = head.indexOf(" · ");
  let target = cut > 0 ? head.slice(cut + 3) : "";
  let name;
  let input;
  if (turn.tool?.name) {
    name = turn.tool.name;
    input = turn.tool.input || (cut > 0 ? turn.text.slice(cut + 3) : "");
  } else if (cut > 0) {
    name = head.slice(0, cut);
    input = turn.text.slice(cut + 3);
  } else if (/^[\w-]+$/.test(head)) {
    name = head;
    input = turn.text.slice(head.length).replace(/^\n/, "");
  } else {
    name = t("worker.tool", "도구");
    input = turn.text;
  }

  // CapabilityInvoke unwrap: CapabilityInvoke {"name":"WebSearch", "input":{...}}
  // shows as "WebSearch · query" rather than generic wrapper clutter.
  if ((name === "CapabilityInvoke" || name.startsWith("CapabilityInvoke ")) && input) {
    try {
      const parsed = typeof input === "object" ? input : JSON.parse(input.trim());
      if (parsed?.name) {
        name = parsed.name;
        if (parsed.input && typeof parsed.input === "object") {
          target = parsed.input.query || parsed.input.url || parsed.input.command || parsed.input.path || parsed.input.prompt || "";
        }
      }
    } catch {}
  }

  // Extract clean target from JSON argument objects ({ "path": "...", "command": "...", "pattern": "..." })
  if (!target && typeof input === "string" && input.trim().startsWith("{")) {
    try {
      const parsed = JSON.parse(input.trim());
      target = parsed.path || parsed.command || parsed.pattern || parsed.query || parsed.url || "";
    } catch {}
  }

  if (target && target.length > 80) target = `${target.slice(0, 77)}…`;

  return { name, arg: target || input.split("\n", 1)[0], input };
}

function cleanseAssistantText(text) {
  if (!text) return "";
  return text
    .replace(/<system-reminder>[\s\S]*?<\/system-reminder>/gi, "")
    .replace(/<task-notification>[\s\S]*?<\/task-notification>/gi, "")
    .replace(/<local-command-caveat>[\s\S]*?<\/local-command-caveat>/gi, "")
    .replace(/\[earlier reasoning\][\s\S]*?(?=\n\n|\n[#A-Z]|$)/gi, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/* One row per turn, whatever the turn is — the sync appends these in order
 * and keys them by `data-turn`. `spoken` is the seq of the last thing said
 * (person or agent): a call after it with no result yet is still out. */
function helperTurnRowNode(run, turn, spoken) {
  if (turn.role === "thinking") return thoughtTurnNode(run, turn);
  if (turn.role === "tool" || turn.role === "tool_result") return toolTurnNode(run, turn, spoken);
  const briefing = turn.role === "user" && turn.seq === 0;
  const row = document.createElement("article");
  const said = document.createElement("div");
  said.className = "helper-said";
  if (turn.role === "user") {
    // The person's words are the extension's bubble — the briefing and
    // every word after it alike. No label node: what it is, the name
    // (aria-label) and the window's tooltip say.
    row.className = briefing ? "helper-turn is-user is-briefing" : "helper-turn is-user";
    const who = briefing ? t("worker.briefing", "브리핑") : t("board.you", "나");
    row.setAttribute("aria-label", who);
    if (briefing) row.dataset.tip = who;
    said.textContent = turn.text;
  } else {
    // The agent's rows stand on the timeline rail (`is-step`); a system line
    // stands off it, the way the extension's meta messages do.
    row.className = turn.role === "assistant" ? "helper-turn is-assistant is-step" : `helper-turn is-${turn.role}`;
    // Announced by who said it, as the extension announces every message
    // (2.1.272): the person's rows say 「나」 above, an answer says the
    // agent's own name — the catalog's, never a word invented here.
    if (turn.role === "assistant") row.setAttribute("aria-label", agentName(run.agent));
    const clean = turn.role === "assistant" ? cleanseAssistantText(turn.text) : turn.text;
    paintHelperProse(said, clean || turn.text, helperBase(run));
  }
  row.appendChild(said);
  // Every answer carries the extension's action row (`assistantActions`):
  // a copy, shown while the pointer or the focus is on the answer.
  if (turn.role === "assistant") row.appendChild(helperActionsNode(turn));
  row.dataset.turn = String(turn.seq);
  return row;
}

/* `● Read ui/shell.js` / `└ 256 lines` — the extension's tool row. The name
 * and the target's first line on the call line; the first line of what came
 * back under it, born when it comes (`dressToolTurn`); anything longer than
 * an eye — a multi-line input, the rest of an output — behind one fold that
 * never widens the page. The dot in the gutter is the row's `::before`, and
 * its state is the row's class: live (out, on the accent), done (green),
 * failed (halt). */
function toolTurnNode(run, turn, spoken) {
  const row = document.createElement("article");
  row.className = "helper-turn is-tool is-step";
  const call = document.createElement("p");
  call.className = "helper-tool-call";
  const name = document.createElement("span");
  name.className = "helper-tool-name";
  const arg = document.createElement("span");
  arg.className = "helper-tool-arg";
  call.append(name, arg);
  row.appendChild(call);
  row.dataset.turn = String(turn.seq);
  row.__turn = turn;
  dressToolTurn(row, turn, run, spoken);
  return row;
}

/* The fold under a tool row: the input past its first line, the output past
 * its first line — each its own well, shown only when it has something. */
function toolMoreNode() {
  const body = document.createElement("div");
  body.className = "helper-tool-more-body";
  for (const kind of ["input", "output"]) {
    const well = document.createElement("pre");
    well.className = `helper-fold-body helper-tool-${kind}`;
    well.hidden = true;
    body.appendChild(well);
  }
  return foldCardNode("helper-tool-more", [foldCueNode("")], body);
}

/* The extension's inline diff under `● Edit path` / `● Write path`: the rows
 * the backend cut from the call's own input (`tool.edits` — an Edit's old and
 * new strings, a Write's whole content, a Codex patch), drawn with the review
 * surface's row (`diffLineNode`) and its word marks, in a well that scrolls
 * on its own. One block per file; the file's name stands over its rows only
 * when the call touched more than one or the name is not the one already on
 * the call line. Built once — an edit never changes after it was written. */
function toolDiffNode(edits, spoken = "") {
  const block = document.createElement("div");
  block.className = "helper-tool-diff";
  for (const edit of edits) {
    if (edits.length > 1 || (edit.path && edit.path !== spoken)) {
      const path = document.createElement("p");
      path.className = "helper-tool-diff-path";
      path.textContent = edit.path;
      block.appendChild(path);
    }
    const rows = document.createElement("div");
    rows.className = "helper-tool-diff-rows";
    const words = diffWordSpans(edit.lines);
    edit.lines.forEach((line, index) => rows.appendChild(diffLineNode(line, words.get(index))));
    if (edit.truncated > 0) {
      const more = document.createElement("div");
      more.className = "diff-line diff-line--meta";
      more.textContent = t("worker.moreLines", "{{n}}줄 더", { n: edit.truncated });
      rows.appendChild(more);
    }
    block.appendChild(rows);
  }
  return block;
}

/* A class that is set only when it changes — a poll that brings nothing must
 * leave the DOM untouched, and a same-value toggle still records a mutation. */
function writeClass(node, name, on) {
  if (node.classList.contains(name) !== on) node.classList.toggle(name, on);
}

function writeHidden(node, hidden) {
  if (node.hidden !== hidden) node.hidden = hidden;
}

/* Dress a tool row from its turn — at birth and again when its result joins
 * (`holdHelperTurns` writes the result onto the call's own turn) or the run's
 * state turns. Every write is guarded, so a quiet poll costs no mutation. */
function dressToolTurn(row, turn, run, spoken) {
  const words = toolWords(turn);
  writeTextContent(row.querySelector(".helper-tool-name"), words.name);
  writeTextContent(row.querySelector(".helper-tool-arg"), words.arg);
  // A tool row is announced by the tool it ran (2.1.272) — the CLI's own
  // name for it, which the row already shows. Guarded like every other
  // write here: a quiet poll costs no mutation.
  writeAttribute(row, "aria-label", t("worker.toolRow", "{{name}} 도구", { name: words.name }));
  // What the call took, once its result is in — the CLI feeds say it beside
  // the call (Hermes: `┊ 💻 terminal  ls -la  (0.3s)`), and so does this row.
  if (turn.outputAt !== undefined && turn.at !== undefined && !row.querySelector(".helper-tool-took")) {
    const took = document.createElement("span");
    took.className = "helper-tool-took";
    took.textContent = t("worker.elapsedShort", "{{s}}초", { s: (Math.max(0, turn.outputAt - turn.at) / 1000).toFixed(1) });
    row.querySelector(".helper-tool-call").appendChild(took);
  }
  const output = turn.role === "tool_result" ? turn.text : turn.output;
  const failed = turn.role === "tool_result" ? turn.tool?.is_error === true : turn.outputError === true;
  writeClass(row, "is-live", output === undefined && run.status === "running" && turn.seq > spoken);
  writeClass(row, "is-done", output !== undefined && !failed);
  writeClass(row, "is-failed", failed);
  if (output !== undefined) {
    let result = row.querySelector(":scope > .helper-tool-result");
    if (!result) {
      result = document.createElement("p");
      result.className = "helper-tool-result";
      row.querySelector(".helper-tool-call").after(result);
    }
    writeTextContent(result, output.split("\n", 1)[0] || t("worker.noOutput", "출력 없음"));
  }
  // The edit under its row, once. The diff IS the input — the well would
  // only repeat it as JSON — so an edit row folds its output alone.
  const edits = turn.tool?.edits ?? [];
  if (edits.length > 0 && !row.querySelector(":scope > .helper-tool-diff")) {
    row.appendChild(toolDiffNode(edits, words.arg));
  }
  const inputMore = edits.length === 0 && chatFolds(words.input) ? words.input : "";
  const outputMore = output !== undefined && chatFolds(output) ? output : "";
  let more = row.querySelector(":scope > .helper-tool-more");
  if (!inputMore && !outputMore) return;
  if (!more) {
    more = toolMoreNode();
    row.appendChild(more);
  }
  const lines = (inputMore ? inputMore.split("\n").length : 0) +
    (outputMore ? outputMore.split("\n").length : 0);
  writeTextContent(more.querySelector(".helper-cue"), t("worker.moreLines", "{{n}}줄 더", { n: lines }));
  for (const [kind, text] of [["input", inputMore], ["output", outputMore]]) {
    const well = more.querySelector(`.helper-tool-${kind}`);
    writeHidden(well, text === "");
    writeTextContent(well, text);
  }
}

/* The model's reasoning, folded behind its own heading (the bold first line
 * the summary opens with, or its first words) — the extension's collapsed
 * thought. The body is prose a shade quieter than an answer, painted the
 * first time the fold opens: a long run thinks often, and a page that
 * rendered every thought up front would pay for words nobody unfolded. */
function thoughtTitle(text) {
  const first = text.split("\n", 1)[0].trim();
  const bold = first.startsWith("**") && first.length > 4 && first.indexOf("**", 2) > 2
    ? first.slice(2, first.indexOf("**", 2)).trim()
    : first;
  return bold.length > CHAT_FOLD_LIMIT ? `${bold.slice(0, CHAT_FOLD_LIMIT - 1)}…` : bold;
}

/* The fold's label: 「3초 동안 생각」 once the next thing happened (the
 * extension's 「Thought for 3s」), the bare word until then. */
function thoughtLabel(turn) {
  if (turn.thoughtMs !== undefined) {
    return t("worker.thoughtFor", "{{s}}초 동안 생각", { s: Math.max(1, Math.round(turn.thoughtMs / 1000)) });
  }
  return t("worker.thought", "생각");
}

function dressThoughtRow(row, turn) {
  writeTextContent(row.querySelector(":scope > .helper-cap > .helper-who"), thoughtLabel(turn));
}

function thoughtTurnNode(run, turn) {
  const body = document.createElement("div");
  body.className = "helper-thought-body";
  const row = foldCardNode("helper-turn is-thinking is-step",
    [foldWhoNode(thoughtLabel(turn)), foldCueNode(thoughtTitle(turn.text))], body);
  row.dataset.turn = String(turn.seq);
  row.__turn = turn;
  row.addEventListener("toggle", () => {
    if (row.open && !body.hasChildNodes()) paintHelperProse(body, turn.text, helperBase(run));
  });
  return row;
}

/* The mark an agent wears — in the head, on its composer chip, on the status
 * line. One node shape, three places. */
function agentMarkNode(className, glyph) {
  const mark = document.createElement("span");
  mark.className = className;
  mark.textContent = glyph;
  return mark;
}

/* `✻ Pondering…` — the line under the transcript while the run is out, in
 * the CLI's own mark and word. Hidden the moment the run is not running. */
function helperStatusNode(run) {
  const line = document.createElement("p");
  line.className = "helper-status";
  line.append(agentMarkNode("helper-status-mark", ""), agentMarkNode("helper-status-word", ""));
  updateHelperStatus(line, run);
  return line;
}

/* The spinner's own cadence: Claude Code's panel and its TUI both turn the
 * mark every 120 ms through `·✢*✶✻✽` and back (`agent_voice.glyph_cycle`). */
const STATUS_CYCLE_MS = 120;

function updateHelperStatus(line, run) {
  const voice = agentVoice(run.agent);
  const shown = run.status === "running";
  writeHidden(line, !shown);
  // The mark wears the permission mode's reach, as the extension's spinner
  // does (`[data-permission-mode]` on its container).
  wearReach(line, composerReachOf(run));
  const mark = line.querySelector(".helper-status-mark");
  writeTextContent(line.querySelector(".helper-status-word"), voice.busy_word);
  // Forward and back, as the CLI plays it; a console with one mark keeps it.
  const cycle = shown && voice.glyph_cycle.length > 1
    ? [...voice.glyph_cycle, ...[...voice.glyph_cycle].reverse()]
    : [];
  const key = cycle.join("");
  if (line.__cycleKey === key) {
    if (!key) writeTextContent(mark, voice.glyph);
    return;
  }
  stopStatusCycle(line);
  line.__cycleKey = key;
  if (!key) {
    writeTextContent(mark, voice.glyph);
    mark.classList.remove("is-cycling");
    return;
  }
  mark.classList.add("is-cycling");
  let at = Math.max(0, cycle.indexOf(voice.glyph));
  writeTextContent(mark, cycle[at]);
  line.__cycle = window.setInterval(() => {
    if (!line.isConnected) {
      stopStatusCycle(line);
      return;
    }
    if (document.hidden) return;
    at = (at + 1) % cycle.length;
    writeTextContent(mark, cycle[at]);
  }, STATUS_CYCLE_MS);
}

function stopStatusCycle(line) {
  if (line.__cycle) clearInterval(line.__cycle);
  line.__cycle = null;
  line.__cycleKey = undefined;
}

/* The agent's prose, drawn as the document viewer draws markdown — the one
 * renderer this window has for it. The document behind it is the parent's
 * checkout (`mdWhere.base`), so a relative path in the answer opens the file
 * it names there; an image stays its caption; and a renderer that trips on a
 * line leaves the words standing as text rather than a blank turn. Bare paths
 * and addresses become the same doors a markdown link is (t-2973). */
function paintHelperProse(host, text, base) {
  const previous = mdWhere;
  mdWhere = { base, page: null };
  try {
    paintMarkdown(host, text);
    linkifyHelperProse(host);
  } catch {
    host.replaceChildren();
    host.textContent = text;
  } finally {
    mdWhere = previous;
  }
}

/* 답이 이름한 파일과 주소(t-2973) — 「완성했습니다: index.html」의 그 낱말.
 * 모양은 셋: http(s) 주소, 슬래시로 시작하는(또는 ./ ../ ~/) 경로, 확장자를
 * 단 파일 이름(디렉터리 접두 허용, 뒤에 `:줄` 허용). 문장 끝의 문장부호는
 * 낱말의 것이 아니다. 문은 `mdLink`의 것이다 — 주소는 브라우저 링크 라우팅,
 * 경로는 문서 뷰어 — 여기서는 옷(아이콘·액센트)만 입힌다. */
const HELPER_FILE_EXTENSIONS =
  "html?|md|rs|js|mjs|cjs|ts|tsx|jsx|css|json|toml|ya?ml|py|sh|txt|csv|svg|png|jpe?g|gif|pdf";
const HELPER_LINK_RE = new RegExp(
  String.raw`(?<![\w/.:@-])(?:(https?:\/\/[^\s<>()"'「」『』]+)|((?:~\/|\.{1,2}\/|\/)[\w.@%+~-]+(?:\/[\w.@%+~-]+)*)|((?:[\w@%+-]+\/)*[\w@%+-]+\.(?:${HELPER_FILE_EXTENSIONS})))(?::\d+(?:-\d+)?)?(?![\w/])`,
  "g",
);
const HELPER_LINK_TAIL_RE = /[.,;:!?)]+$/;
const HELPER_LINK_LINE_RE = /:\d+(?:-\d+)?$/;

/* 한 글의 경로·주소 전부. 전역 정규식 하나를 나눠 쓰므로 `lastIndex`를 매번
 * 0으로 — `matchAll`은 그 값을 물려받아(스펙) 앞 검색이 남긴 자리부터 찾는다:
 * 그래서 두 번째 답부터 링크도 카드도 조용히 사라졌다. */
function helperLinkHits(text) {
  HELPER_LINK_RE.lastIndex = 0;
  return [...text.matchAll(HELPER_LINK_RE)];
}

/* 문장 안의 경로·주소를 찾아 문으로 바꾼다. 코드 칩·블록과 이미 문인 것 안은
 * 건드리지 않는다 — 코드는 기계의 글이고, 문 안의 문은 없다. */
function linkifyHelperProse(host) {
  const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
  const found = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.parentElement?.closest("code, pre, .md-link, a")) continue;
    const hits = helperLinkHits(node.data);
    if (hits.length) found.push([node, hits]);
  }
  for (const [node, hits] of found) {
    const parts = [];
    let last = 0;
    for (const hit of hits) {
      const said = hit[0].replace(HELPER_LINK_TAIL_RE, "");
      if (!said) continue;
      if (hit.index > last) parts.push(node.data.slice(last, hit.index));
      parts.push(helperLinkNode(said, Boolean(hit[1])));
      last = hit.index + said.length;
    }
    if (last < node.data.length) parts.push(node.data.slice(last));
    node.replaceWith(...parts);
  }
}

function helperLinkNode(said, isUrl) {
  const link = mdLink(said, isUrl ? said : said.replace(HELPER_LINK_LINE_RE, ""));
  link.classList.add("helper-link");
  link.prepend(iconNode(isUrl ? "globe" : "file"));
  return link;
}

/* 이 헬퍼의 문서 자리 — 부모 판의 체크아웃. 답의 상대 경로는 거기서 잰다. */
function helperBase(run) {
  return run.cwd ?? tabOfTerm(run.term)?.worktree ?? "";
}

/* 마지막 답의 꼬리(t-2973): 답이 .html이나 주소를 이름하면 미리보기 카드 하나
 * (원형 아이콘, 제목, 이름한 것, 오른쪽 알약 「다음에서 열기」 — 브라우저 탭 /
 * 기본 앱, 창에 이미 있는 두 문), 그리고 그 턴의 원문을 복사하는 단추 하나.
 * 기능 없는 아이콘은 두지 않는다. 꼬리는 마지막 어시스턴트 턴의 것이라 새
 * 턴이 오면 옮겨 간다(`syncHelperTurns`). */
function helperPreviewTarget(text) {
  for (const hit of helperLinkHits(text)) {
    const said = hit[0].replace(HELPER_LINK_TAIL_RE, "");
    if (hit[1]) return { said, url: true };
    if (/\.html?$/i.test(said)) return { said, url: false };
  }
  return null;
}

function helperTailNode(run, turn) {
  const tail = document.createElement("div");
  tail.className = "helper-tail";
  const target = helperPreviewTarget(turn.text);
  if (target) tail.appendChild(helperPreviewNode(run, target));
  return tail;
}

/* The extension's `assistantActions` under an answer: one copy button, on
 * every answer, visible while the pointer or the focus is on the row. */
function helperActionsNode(turn) {
  const actions = document.createElement("div");
  actions.className = "helper-actions";
  const copy = document.createElement("button");
  copy.type = "button";
  copy.className = "helper-copy";
  const copyWords = t("worker.copyAnswer", "답 복사");
  copy.setAttribute("aria-label", copyWords);
  copy.dataset.tip = copyWords;
  copy.appendChild(iconNode("copy"));
  copy.addEventListener("click", () => void clipboardText.write(turn.text));
  actions.appendChild(copy);
  return actions;
}

/* The last thing the agent SAID on this page, or "" — what the copy under an
 * answer copies, reached from the palette instead of the pointer
 * (`/copy`, 2.1.275). The turns are the page's own memory, so there is
 * nothing to ask the backend for. */
function lastAnswerOf(run) {
  return (run?.helper?.turns ?? []).findLast((turn) => turn.role === "assistant")?.text ?? "";
}

/* `/copy` — the last answer to the clipboard, through the one clipboard door
 * this window has. A page that has been answered nothing does not list the
 * command at all (`windowSlashCommands`), so this is never a press that does
 * nothing; it still checks, because the turns can be capped away between the
 * palette opening and the Enter. */
function copyLastAnswer(run) {
  const said = lastAnswerOf(run);
  if (!said) return;
  void clipboardText.write(said);
}

function helperPreviewNode(run, target) {
  const card = document.createElement("div");
  card.className = "helper-preview";
  const mark = document.createElement("span");
  mark.className = "helper-preview-mark";
  mark.appendChild(iconNode(target.url ? "globe" : "code"));
  const body = document.createElement("div");
  body.className = "helper-preview-body";
  const title = document.createElement("span");
  title.className = "helper-preview-title";
  title.textContent = t("worker.preview", "미리보기");
  const sub = document.createElement("span");
  sub.className = "helper-preview-sub";
  sub.textContent = target.said;
  body.append(title, sub);
  const open = document.createElement("details");
  open.className = "helper-preview-open";
  const lid = document.createElement("summary");
  lid.className = "worker-composer-pill helper-preview-lid";
  lid.textContent = t("worker.openIn", "다음에서 열기");
  const menu = document.createElement("div");
  menu.className = "helper-preview-menu";
  const where = target.url ? target.said : resolveDocPath(helperBase(run), target.said);
  menu.append(
    previewDoorNode(open, t("worker.openInBrowser", "브라우저 탭에서"), () => {
      void openBrowserTab(where);
    }),
    previewDoorNode(open, t("worker.openInApp", "기본 앱으로"), () => {
      if (target.url) openExternal(where);
      else invoke("fs_open_default", { path: where }).catch((error) => showError(String(error)));
    }),
  );
  open.append(lid, menu);
  card.append(mark, body, open);
  return card;
}

function previewDoorNode(open, words, go) {
  const door = document.createElement("button");
  door.type = "button";
  door.className = "helper-preview-door";
  door.textContent = words;
  door.addEventListener("click", () => {
    open.open = false;
    go();
  });
  return door;
}

/* 꼬리를 마지막 어시스턴트 턴으로 옮긴다 — 바뀌었을 때만 DOM을 만진다. */
function dressLastAnswer(list, run, newest) {
  if (!newest || newest.row === list.__lastAnswer) return;
  list.__lastAnswer?.querySelector(":scope > .helper-tail")?.remove();
  const tail = helperTailNode(run, newest.turn);
  if (tail.childElementCount) newest.row.appendChild(tail);
  list.__lastAnswer = newest.row;
}

/* The list's fixed tail — the status line the extension's panel keeps under
 * the last row (`spinnerRow`) — that every turn and streaming row stands
 * before. `null` while the page has none. */
function helperListTail(list) {
  return list.querySelector(":scope > .helper-status");
}

function scrollHelperToBottom(list) {
  if (!list) return;
  list.scrollTop = list.scrollHeight;
  requestAnimationFrame(() => {
    // 늦게 도착한 프레임은 그 사이 일어난 일을 모른다: 이 줄을 예약할 때
    // 바닥이던 사람이 프레임이 오기 전에 위를 읽기 시작했을 수 있고, 부하가
    // 클수록 그 틈이 넓다. 다시 묻고 나서 옮긴다 — 방금 바닥으로 보낸 판은
    // 여전히 바닥이므로 따라가던 사람은 그대로 따라간다.
    if (!list.isConnected || !helperFollowsTail(list)) return;
    // The list is the one thing on this page that scrolls, so it is the one
    // thing this moves. `scrollIntoView` on the last row is not the same
    // move: it scrolls every scrollable ancestor as well — the document
    // included, `overflow: hidden` notwithstanding — and a page whose
    // conversation stood a title bar's height below the viewport was dragged
    // down (and, on a wide row, sideways) until the tab strip and the side
    // rail were out of the window (2026-09-21, installed 1.1.11).
    list.scrollTop = list.scrollHeight;
  });
}

/* 바닥 근처의 폭. 이 안에 있으면 새 턴을 따라가고, 위를 읽는 중이면 자리를
 * 지킨다. 그리는 픽셀이 아니라 스크롤 판정의 문턱이라 간격 스케일 밖의
 * px다. */
const HELPER_FOLLOW_SLACK_PX = 160;

/* 읽는 사람이 꼬리를 따라가는 중인가 — 판정은 이 한 곳에서만 한다.
 *
 * 새 턴이 왔다는 것은 스크롤을 빼앗을 이유가 되지 못한다: 09-18의 자동 스크롤
 * 최적화가 이 조건을 잃고 `newest || follow`로 부르면서, 위를 읽던 사람을 폴
 * 마다 바닥으로 끌어내렸고 앞쪽이 잘릴 때의 자리 보정까지 덮어썼다. 창 하네스
 * 둘(`heldPlace`·`anchorHeld`)이 그날부터 그것을 말하고 있었다. */
function helperFollowsTail(list) {
  return !!list && list.scrollHeight - list.scrollTop - list.clientHeight <= HELPER_FOLLOW_SLACK_PX;
}

/* ---- Focus view: 한 턴의 도구 일을 요약 한 줄 뒤로 ----
 *
 * 확장 2.1.221의 「Focus view hides tool activity behind per-turn summaries
 * with a live running-tool indicator」. 연속한 활동 행 — 도구 호출과 생각 —
 * 이 한 묶음이 되고, 묶음의 머리가 「도구 호출 n회 · 실패 m」과 지금 나가
 * 있는 호출 하나를 말한다. 사람의 말·답·시스템 줄은 활동이 아니므로 묶음을
 * 닫는다: 그래서 한 묶음은 「한 턴이 한 일」과 같은 구간이다.
 *
 * 구성원은 목록의 직계 자식으로 **남는다**. 옮기지 않는 것이 요점이다 —
 * 행의 열쇠(`data-turn`)로 잇고 지우는 `syncHelperTurns`의 회계도, t-110의
 * 정체성 핀들도 모두 그 자리를 전제로 서 있다. 묶음 노드는 `data-turn`이
 * 없어 그 회계에 보이지 않고, 접힘은 구성원의 `hidden` 한 비트다.
 *
 * 계수는 묶음이 들고 다닌다(`__calls`·`__failed`·`__live`). 폴마다 구성원을
 * 다시 세는 대신 상태가 움직인 행 하나만 계상하므로(`accountFocusMember`),
 * 한 폴이 치르는 값은 그 폴에 온 행과 아직 나가 있는 호출의 수에 비례한다. */

/* 이 창의 모든 대화가 입는 한 값 — 전역 설정 `conversation_focus_view`.
 * 스냅샷이 옮기고(`applyAgentSettingsSnapshot`), 머리의 단추가 쓴다. */
let conversationFocusView = false;

function focusViewOn() {
  return conversationFocusView;
}

/* 토글이 움직였다. 서 있는 목록은 그 자리에서 따라가고 — 사람이 누른 그
 * 페이지가 다음 폴을 기다리지 않게 — 아직 안 그려진 페이지는 제 다음 그림의
 * `syncHelperTurns` 첫 줄에서 따라온다. */
function repaintFocusView() {
  const on = focusViewOn();
  for (const list of document.querySelectorAll(".helper-turns")) applyFocusView(list, on);
  for (const button of document.querySelectorAll(".worker-focus")) {
    writeAttribute(button, "aria-pressed", on ? "true" : "false");
  }
}

/* 설정을 쓰는 한 곳. 값은 먼저 화면에 서고(사람의 손끝은 왕복을 기다리지
 * 않는다) 권위 있는 스냅샷이 돌아와 같은 값을 다시 놓는다. */
function setConversationFocusView(on) {
  conversationFocusView = on;
  repaintFocusView();
  void commitSetting("conversation_focus_view", "set_conversation_focus_view", { on });
}

/* 머리의 토글. 아이콘 단추라 이름을 두 번 단다(`labelButton`) — 손끝의
 * 말풍선에게 한 번, 보조기술에게 한 번. */
function focusViewButtonNode() {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "worker-focus";
  button.appendChild(iconNode("list-filter"));
  labelButton(button, t("worker.focusView", "집중 보기"));
  button.setAttribute("aria-pressed", focusViewOn() ? "true" : "false");
  button.addEventListener("click", () => setConversationFocusView(!focusViewOn()));
  return button;
}

/* 묶음 머리 하나. 요약의 말과, 지금 나가 있는 호출을 말하는 자리 — 그 자리는
 * 도구 행의 마크업 그대로(`.helper-tool-name` + `.helper-tool-arg`)라 규칙이
 * 두 벌 되지 않는다. 계수는 여기서 0으로 시작한다. */
function focusGroupNode() {
  const group = document.createElement("article");
  group.className = "helper-group is-step";
  const cap = document.createElement("button");
  cap.type = "button";
  cap.className = "helper-group-cap";
  const words = document.createElement("span");
  words.className = "helper-group-words";
  const live = document.createElement("span");
  live.className = "helper-group-live";
  const name = document.createElement("span");
  name.className = "helper-tool-name";
  const arg = document.createElement("span");
  arg.className = "helper-tool-arg";
  live.append(name, arg);
  live.hidden = true;
  cap.append(words, live);
  group.appendChild(cap);
  group.__calls = 0;
  group.__failed = 0;
  group.__live = 0;
  group.__members = 0;
  group.__liveRow = null;
  group.__open = false;
  cap.addEventListener("click", () => openFocusGroup(group, !group.__open));
  return group;
}

/* 활동 행인가 — 도구 호출과 생각만이 묶인다. 스트리밍 행과 상태 행은
 * `data-turn`이 없어 부르는 쪽에서 이미 걸러진다. */
function isFocusActivityRow(row) {
  return row.classList.contains("is-tool") || row.classList.contains("is-thinking");
}

/* 행이 지금 계상되는 칸. 생각 행은 호출이 아니므로 제 칸을 따로 갖는다. */
function focusRowState(row) {
  if (row.classList.contains("is-thinking")) return "thought";
  if (row.classList.contains("is-failed")) return "failed";
  if (row.classList.contains("is-live")) return "live";
  return "done";
}

/* 계수 한 칸을 옮긴다 — 붙을 때 +1, 상태가 바뀌거나 떨어질 때 -1. 셋을 한
 * 곳에서 옮기므로 「호출 수」와 「실패 수」가 서로 다른 셈을 하지 못한다. */
function tallyFocusState(group, state, by) {
  if (state === undefined || state === "thought") return;
  group.__calls += by;
  if (state === "failed") group.__failed += by;
  if (state === "live") group.__live += by;
}

/* 한 행이 바뀐 만큼만 묶음의 계수를 옮긴다 — 행당 O(1). */
function accountFocusMember(group, row) {
  const after = focusRowState(row);
  if (row.__focusState === after) return;
  tallyFocusState(group, row.__focusState, -1);
  tallyFocusState(group, after, 1);
  row.__focusState = after;
  if (after === "live") group.__liveRow = row;
  else if (group.__liveRow === row) group.__liveRow = null;
}

/* 앞이 잘려 나가는 행 하나를 묶음에서 뺀다. 마지막 구성원이 떠나면 묶음
 * 노드도 그 자리에서 사라진다 — 빈 머리가 남으면 그것이 곧 거짓 요약이다. */
function dropFocusMember(row) {
  const group = row.__group;
  if (!group) return;
  tallyFocusState(group, row.__focusState, -1);
  if (group.__liveRow === row) group.__liveRow = null;
  row.__group = null;
  row.__focusState = undefined;
  group.__members -= 1;
  if (group.__members <= 0) group.remove();
}

/* 새로 선 활동 행 하나를 열린 묶음에 들인다 — 없으면 이 행 바로 앞에 새
 * 묶음을 세운다. */
function attachFocusRow(list, row) {
  let group = list.__focusOpen;
  if (!group || !group.isConnected) {
    group = focusGroupNode();
    list.insertBefore(group, row);
    list.__focusOpen = group;
  }
  row.classList.add("is-grouped");
  row.__group = group;
  group.__members += 1;
  accountFocusMember(group, row);
  writeHidden(row, !group.__open);
  paintFocusGroup(group);
}

/* 새로 선 행 하나가 묶음 회계에 들어가거나, 묶음을 닫는다. */
function holdFocusRow(list, row) {
  if (isFocusActivityRow(row)) attachFocusRow(list, row);
  else list.__focusOpen = null;
}

/* 머리가 말할 살아 있는 호출 한 행 — 가장 최근에 나간 것. 계상이 그때그때
 * 적어 두므로 보통은 물어보기만 하면 되고, 형제가 먼저 끝나 비었을 때만
 * 구성원을 따라가며 다시 찾는다(폴마다가 아니라 그 한 번). */
function liveFocusMember(group) {
  const held = group.__liveRow;
  if (held && held.__group === group && held.__focusState === "live") return held;
  let row = group.nextElementSibling;
  while (row && row.__group === group) {
    if (row.__focusState === "live") {
      group.__liveRow = row;
      return row;
    }
    row = row.nextElementSibling;
  }
  group.__liveRow = null;
  return null;
}

/* 나가 있는 호출 하나를 머리가 말한다 — 도구 행이 이미 쓴 낱말 그대로
 * 옮겨 적는다(`dressToolTurn`이 `toolWords`로 쓴 그 두 칸), 같은 규칙을
 * 두 번 돌리지 않기 위해서다. */
function paintFocusGroupLive(group) {
  const live = group.querySelector(":scope > .helper-group-cap > .helper-group-live");
  const row = group.__live > 0 ? liveFocusMember(group) : null;
  writeHidden(live, row === null);
  if (!row) return;
  writeTextContent(live.querySelector(".helper-tool-name"), row.querySelector(".helper-tool-name")?.textContent ?? "");
  writeTextContent(live.querySelector(".helper-tool-arg"), row.querySelector(".helper-tool-arg")?.textContent ?? "");
}

/* 계수에서 말과 클래스와 live 표시를 — 바뀔 때만 쓴다. 조용한 폴은
 * mutation 0이어야 하므로 여기의 모든 쓰기는 문지기를 지난다. 머리의 이름은
 * 펼침 여부에만 달려 있으니 그 비트가 움직일 때만 다시 단다. */
function paintFocusGroup(group) {
  const cap = group.querySelector(":scope > .helper-group-cap");
  const calls = group.__calls;
  const words = calls === 0
    ? t("worker.focusThinking", "생각")
    : t("worker.focusCalls", "도구 호출 {{n}}회", { n: calls }) +
      (group.__failed > 0 ? t("worker.focusFailed", " · 실패 {{n}}", { n: group.__failed }) : "");
  writeTextContent(cap.querySelector(".helper-group-words"), words);
  const open = group.__open === true;
  writeAttribute(cap, "aria-expanded", open ? "true" : "false");
  if (group.__labelled !== open) {
    labelButton(cap, open
      ? t("worker.focusCollapse", "도구 호출 접기")
      : t("worker.focusExpand", "도구 호출 펼치기"));
    group.__labelled = open;
  }
  // 점은 묶음의 상태다: 하나라도 나가 있으면 액센트, 아니면 실패가 있으면
  // 멈춤의 잉크, 아니면 초록 — 도구 행의 규칙을 그대로 입는다.
  writeClass(group, "is-live", group.__live > 0);
  writeClass(group, "is-failed", group.__live === 0 && group.__failed > 0);
  writeClass(group, "is-done", group.__live === 0 && group.__failed === 0);
  paintFocusGroupLive(group);
}

/* 묶음을 펼치거나 접는다 — 구성원만 따라가며, 사람의 한 동작에만. */
function openFocusGroup(group, open) {
  group.__open = open;
  let row = group.nextElementSibling;
  while (row && row.__group === group) {
    writeHidden(row, !open);
    row = row.nextElementSibling;
  }
  paintFocusGroup(group);
}

/* 묶음을 모두 거둔다 — 구성원은 제자리에 남고 표시만 벗는다. */
function clearFocusGroups(list) {
  for (const group of list.querySelectorAll(":scope > .helper-group")) group.remove();
  for (const row of list.querySelectorAll(":scope > .is-grouped")) {
    row.classList.remove("is-grouped");
    row.__group = null;
    row.__focusState = undefined;
    writeHidden(row, false);
  }
  list.__focusOpen = null;
}

/* 목록 하나를 토글의 지금 값에 맞춘다. 켤 때만 목록을 **한 번** 훑고, 그
 * 값이 이미 적용되어 있으면 아무것도 만지지 않는다 — 그래서 폴마다 불러도
 * 조용하다. */
function applyFocusView(list, on) {
  if (list.__focus === on) return;
  clearFocusGroups(list);
  list.__focus = on;
  if (!on) return;
  for (const row of [...list.children]) {
    // 묶음 머리·스트리밍 행·상태 행은 턴이 아니다.
    if (row.dataset.turn === undefined) continue;
    if (isFocusActivityRow(row)) attachFocusRow(list, row);
    else list.__focusOpen = null;
  }
}

/* 전사를 장부에 맞춘다 — 통째로 다시 세우지 않고.
 *
 * 폴마다 400턴을 `replaceChildren`으로 재생성하던 것이 이 페이지의 무게였다:
 * 새 턴만 뒤에 잇고, 상한이 데이터에서 지운 턴만 앞에서 지운다. 행의 열쇠는
 * `data-turn`(seq) — 장부와 화면이 언제나 같은 구간을 들게 하는 끈이다.
 *
 * 스크롤은 읽는 사람의 것이다: 바닥 근처에 있을 때만 따라가고, 위를 읽는
 * 중이면 앞에서 지워진 키만큼 스크롤을 되물려 읽던 줄이 제자리에 남는다
 * (그래서 CSS가 브라우저의 자동 앵커를 끈다 — 보정이 두 벌이면 한 벌이
 * 거짓말이 된다). */
function syncHelperTurns(list, run) {
  const held = run.helper.turns;
  // 이 목록이 입고 있는 값이 토글의 값과 다르면 먼저 맞춘다 — 같으면
  // 아무것도 만지지 않으므로 조용한 폴은 여기서 값을 치르지 않는다.
  const focus = focusViewOn();
  applyFocusView(list, focus);
  if (!held.length) {
    clearFocusGroups(list);
    for (const row of list.querySelectorAll(":scope > [data-turn]")) row.remove();
    list.__lastAnswer = null;
    syncStreamingTurns(list, run);
    return;
  }
  const first = held[0].seq;
  const follow = helperFollowsTail(list);
  const beforeHeight = list.scrollHeight;
  const beforeTop = list.scrollTop;
  let dropped = false;
  // 앞에서 지워지는 것은 장부가 놓은 턴들이다. 그 사이에 선 묶음 머리는
  // `data-turn`이 없으므로 건너뛰고(`Number(undefined) < first`는 거짓이라
  // 예전 루프는 거기서 멈춰 상한을 잃었다), 턴이 아닌 다른 행 — 스트리밍과
  // 상태 — 에서는 멈춘다: 그 뒤로는 지울 턴이 없다. 지워지는 행은 제 묶음의
  // 계수에서 빠지고, 비게 된 묶음은 그 자리에서 사라진다.
  let front = list.firstElementChild;
  while (front) {
    const next = front.nextElementSibling;
    if (front.dataset.turn === undefined) {
      if (!front.classList.contains("helper-group")) break;
      front = next;
      continue;
    }
    if (Number(front.dataset.turn) >= first) break;
    dropFocusMember(front);
    front.remove();
    dropped = true;
    front = next;
  }
  if (dropped && !follow) {
    list.scrollTop = Math.max(0, beforeTop - (beforeHeight - list.scrollHeight));
  }
  // The last turn drawn: the streaming rows under it carry no turn number,
  // and a new turn goes above them — it is what they were becoming.
  let last = list.lastElementChild;
  while (last && last.dataset.turn === undefined) last = last.previousElementSibling;
  const drawn = last ? Number(last.dataset.turn) : -1;
  const streaming = list.querySelector(":scope > .is-streaming");
  // What was last said, by either voice: a call after it is still out.
  const spoken = held.findLast((turn) => turn.role === "user" || turn.role === "assistant")?.seq ?? -1;
  let newest = null;
  for (const turn of held) {
    if (turn.seq <= drawn) continue;
    const row = helperTurnRowNode(run, turn, spoken);
    list.insertBefore(row, streaming ?? helperListTail(list));
    if (turn.role === "assistant") newest = { row, turn };
    // The words this turn carries were streaming a moment ago: their
    // finished piece leaves in this same paint (`syncStreamingTurns` below).
    settlePaneLive(run, turn.role);
    // The thought before this turn now knows how long it lasted.
    const before = row.previousElementSibling;
    if (before?.classList.contains("is-thinking") && before.__turn?.thoughtMs !== undefined) {
      dressThoughtRow(before, before.__turn);
    }
    // 활동 행은 열린 묶음에 들고, 말한 행은 묶음을 닫는다 — 앞줄의 이웃을
    // 물은 뒤라야 묶음 머리가 그 사이에 끼어들지 않는다.
    if (focus) holdFocusRow(list, row);
  }
  // A result that joined its call after the row stood, a word that closed
  // the calls before it, a run that ended: the rows still out are dressed
  // again — and only those, so a settled row is never touched.
  // 이 루프가 도는 행이 곧 상태가 움직일 수 있는 행 전부다(살아 있던 호출이
  // 결과를 받아 done/failed가 되는 자리가 여기뿐이다). 그래서 묶음의 계수도
  // 여기서만 옮기면 되고, 목록을 다시 훑을 이유가 없다. 행은 문서 차례로
  // 오므로 한 묶음의 행들은 붙어 있고, 묶음은 한 번만 다시 그린다.
  let touched = null;
  for (const row of list.querySelectorAll(":scope > .is-tool.is-live")) {
    dressToolTurn(row, row.__turn, run, spoken);
    const group = row.__group;
    if (!group) continue;
    accountFocusMember(group, row);
    if (touched && group !== touched) paintFocusGroup(touched);
    touched = group;
  }
  if (touched) paintFocusGroup(touched);
  // A turn that arrived stands whole, the moment it arrived — the terminal
  // already showed these words as they were said, and a page that released
  // them again a word at a time (09-16 → 09-20) only lagged behind it. The
  // wire's page streams the words themselves (`syncStreamingTurns`).
  dressLastAnswer(list, run, newest);
  syncStreamingTurns(list, run);
  if (follow) scrollHelperToBottom(list);
}

/* The words the agent is saying right now, under the last turn — the rows
 * Claude Code's own panel streams into. One row per voice, a thought and
 * then the answer, each updated in place as the wire's live text grows
 * (`wire_log.live`), and gone the moment the words close into a turn: the
 * poll that brings the turn brings the empty live list, so the swap is one
 * paint. What has arrived is on screen by the next frame — the terminal's
 * pace and the extension's, which append each delta to the message and
 * repaint (2.1.278 webview, `content_block_delta` → `text +=`). The
 * word-paced reveal of 09-16 held words back at 60 → 33 ms each and fell
 * seconds behind a fast model; the person asked for the terminal's own
 * timing (09-20). A page fed from a transcript draws what the pane's own
 * channel streams (`paneLive`, zo's frames) — the vendor writes its file a
 * turn at a time — and a pane with neither wire nor channel draws nothing
 * here. */
function syncStreamingTurns(list, run) {
  const live = run.wire ? run.wireLog?.live ?? [] : paneLiveOf(run);
  const rows = [...list.querySelectorAll(":scope > .is-streaming")];
  live.forEach((piece, index) => {
    let row = rows[index];
    if (row && row.dataset.role !== piece.role) {
      for (const stale of rows.splice(index)) stale.remove();
      row = null;
    }
    if (!row) {
      row = streamingTurnNode(piece.role);
      list.insertBefore(row, helperListTail(list));
      rows[index] = row;
    }
    if (row.__text === piece.text) return;
    row.__text = piece.text;
    if (piece.role === "thinking") {
      writeTextContent(row.querySelector(".helper-cue"), thoughtTitle(piece.text));
      writeTextContent(row.querySelector(".helper-thought-body"), piece.text);
    } else {
      paintLiveAnswer(row, piece.text, run);
    }
  });
  // Rows the live list no longer names: the words closed into a turn, which
  // the same paint stood above them.
  for (const stale of rows.slice(live.length)) stale.remove();
}

/* Where the settled part of a streaming answer ends: after the last blank
 * line before `upTo` that is not inside a fence, and never inside the line
 * still being written. What lies before it is painted as markdown once and
 * left alone; the rest is the block being written. */
function settledCut(text, upTo) {
  const lines = text.slice(0, upTo).split("\n");
  let cut = 0;
  let fence = null;
  let offset = 0;
  for (let at = 0; at < lines.length - 1; at += 1) {
    const line = lines[at];
    const mark = fenceOf(line);
    if (mark !== null) fence = fence === null ? mark : fence === mark ? null : fence;
    offset += line.length + 1;
    if (fence === null && line.trim() === "") cut = offset;
  }
  return cut;
}

/* Paint what the agent has said so far into its streaming row, coalesced to
 * the frame: thirty deltas a second arrive, and the DOM is touched at most
 * once per frame, with the newest text. */
function paintLiveAnswer(row, text, run) {
  row.__live = text;
  if (row.__liveFrame !== undefined) return;
  row.__liveFrame = requestAnimationFrame(() => {
    row.__liveFrame = undefined;
    if (row.isConnected) paintLiveAnswerNow(row, row.__live, run);
  });
}

/* The blocks that closed (a blank line outside a fence, `settledCut`) as
 * markdown, painted once each; the block still being written as markdown
 * too, repainted with each frame that brought a delta — a paragraph at
 * most, so the repaint is cheap, and never behind what arrived. */
function paintLiveAnswerNow(row, text, run) {
  const said = row.querySelector(".helper-said");
  let settled = said.querySelector(":scope > .helper-said-settled");
  let tail = said.querySelector(":scope > .helper-said-tail");
  if (!settled) {
    settled = document.createElement("div");
    settled.className = "helper-said-settled";
    tail = document.createElement("div");
    tail.className = "helper-said-tail";
    said.append(settled, tail);
    row.__settledEnd = 0;
    row.__tailText = "";
  }
  const list = row.closest(".helper-turns");
  const follow = helperFollowsTail(list);
  const cut = settledCut(text, text.length);
  if (cut > row.__settledEnd) {
    settled.replaceChildren();
    paintHelperProse(settled, text.slice(0, cut), helperBase(run));
    row.__settledEnd = cut;
  }
  const rest = text.slice(row.__settledEnd);
  if (rest !== row.__tailText) {
    row.__tailText = rest;
    tail.replaceChildren();
    if (rest.trim() !== "") paintHelperProse(tail, rest, helperBase(run));
  }
  if (follow) list.scrollTop = list.scrollHeight;
}

/* The row a voice streams into: an answer's row before it closes (the same
 * rail and dot as the answer it becomes), or a thought's fold, open while it
 * is being thought and closed by the turn that replaces it. */
function streamingTurnNode(role) {
  if (role === "thinking") {
    const body = document.createElement("div");
    body.className = "helper-thought-body";
    const row = foldCardNode("helper-turn is-thinking is-step is-streaming",
      [foldWhoNode(t("worker.thinking", "생각 중…")), foldCueNode("")], body, true);
    row.dataset.role = role;
    return row;
  }
  const row = document.createElement("article");
  row.className = "helper-turn is-assistant is-step is-streaming";
  row.dataset.role = role;
  const said = document.createElement("div");
  said.className = "helper-said";
  row.appendChild(said);
  return row;
}

/* 한 헬퍼의 대화 — 읽기 중심의 평평한 전사. 페이지에서 스크롤하는 것은 이
 * 목록뿐이라, 이름과 키보드가 닿을 탭 멈춤을 함께 단다(읽어 주는 지역:
 * `role="log"`가 새 턴을 공손히 알린다). */
function helperTurnsNode(run) {
  const list = document.createElement("div");
  list.className = "helper-turns";
  list.setAttribute("role", "log");
  list.setAttribute("aria-label", t("worker.transcript", "헬퍼 대화 기록"));
  list.tabIndex = 0;
  // A person's words stuck at the top (the extension's sticky header) are a
  // door back to where they were said: a click scrolls the row home.
  list.addEventListener("click", (event) => {
    const row = event.target.closest?.(".helper-turn.is-user");
    if (!row || !list.contains(row)) return;
    if (Math.round(row.getBoundingClientRect().top) > Math.round(list.getBoundingClientRect().top)) return;
    // Scroll the list, never the page: see `scrollHelperToBottom`.
    list.scrollTop += row.getBoundingClientRect().top - list.getBoundingClientRect().top;
  });
  syncHelperTurns(list, run);
  return list;
}

/* 부모 판을 읽을 수 없을 때 중첩 실행에 주는 화면 크기.
 *
 * 갈라지는 미러 판과 끝난 실행의 페이지가 같은 답을 쓴다 — 둘 다 "그 실행이
 * 그려진 화면"을 묻고 있고, 답이 두 벌일 이유가 없다. */
const DEFAULT_TERMINAL_GRID = { rows: 24, cols: 96 };

/* 끝난 실행이 남긴 것은 글자가 아니라 화면이다.
 *
 * 페이지는 그 바이트를 `<pre>`에 글자로 붙였다. 스스로를 그리는 실행 —
 * 에이전트의 TUI — 은 그래서 사람이 지켜본 화면이 아니라 잔해로 도착했다
 * (`[2K`, `[1A`, `[?2026h`; 2026-08-21 스크린샷). 방향을 트는 이스케이프
 * 바이트는 보이지 않고, 그 뒤를 따르던 것만 글자로 남기 때문이다.
 *
 * 창에는 그 바이트를 읽을 것이 없다. 이 창이 그리는 모든 화면은 Rust의
 * 에뮬레이터가 만든 프레임이고(`term_snapshot`), 여기서도 같은 문을 두드린다
 * (`worker_screen`) — 창에 두 번째 파서를 두지 않기 위해서다.
 *
 * 한 번만 묻는다. 실행이 끝났으니 더 올 바이트가 없다.
 *
 * `null`도 답이다 — "이건 화면이 아니라 글이다". 커서를 한 번도 움직이지 않은
 * 로그는 글로 보여 주는 것이 옳고, 스물네 줄짜리 화면에 부으면 위로 흘러간
 * 줄이 통째로 사라진다. */
const workerScreens = new Map();

function askWorkerScreen(tab) {
  const run = tab.worker;
  if (run.helper || !run.output) return;
  // 크기는 부모 판의 것. 그 실행이 실제로 그려진 화면이 거기이고, 미러 판이
  // 이미 같은 자리에서 크기를 얻는다(`openMirrorPane`).
  const grid = viewOfTerm(run.term)?.gridSize() ?? DEFAULT_TERMINAL_GRID;
  invoke("worker_screen", { text: run.output, rows: grid.rows, cols: grid.cols })
    .then((delta) => {
      run.screen = delta ?? null;
      // 화면을 얻었으면 그 바이트는 죽은 무게다. 실행 하나가 최대 256K자를
      // 물고 있는데(`WORKER_OUTPUT_CHARS`), 화면은 판 크기만큼으로 묶여 있고
      // 페이지는 이제 그것만 그린다. 사람이 글자를 가져가야 할 때는 화면에서
      // 끌어 간다 — 터미널이 원래 하는 일이다.
      if (run.screen) run.output = null;
    })
    .catch(() => {
      // 그리지 못한 화면은 글로 돌아간다 — 못 그렸다는 이유로 아무것도 안
      // 보여 주는 것이 가장 나쁘다.
      run.screen = null;
    })
    .finally(() => {
      const held = tabs.find((one) => one.id === tab.id);
      if (held && held.id === activeTabId) paintWorkerView(held);
    });
}

/* 화면 하나가 페이지에서 떠날 때. 뷰는 호스트에 관찰자를 걸어 두므로
 * (`release`), 노드만 버리면 죽은 화면 위의 관찰이 살아남는다.
 *
 * 열쇠는 탭이 아니라 **판**이다. 한 판의 워커 호스트는 `docHost`가 하나만
 * 만들어 돌려 쓰므로, 같은 판에서 워커 탭을 갈아타면 앞 화면의 노드는 다음
 * `replaceChildren` 한 번에 사라진다 — 그 순간이 곧 놓아 줄 순간이다. */
function dropWorkerScreen(pane) {
  const held = workerScreens.get(pane);
  if (held === undefined) return;
  held.view?.release();
  workerScreens.delete(pane);
}

/* 끝난 실행의 몸통: 에뮬레이터가 그린 화면이거나, 화면이 아니었다면 그 글.
 *
 * 답을 기다리는 한 왕복 동안에는 자리만 잡는다. 잔해를 먼저 보여 주었다가
 * 갈아 끼우면, 사람이 보는 첫 화면이 바로 그 잔해다. */
function workerBodyNode(tab) {
  const run = tab.worker;
  if (run.output && run.screen === undefined) {
    const waiting = document.createElement("div");
    waiting.className = "worker-screen-waiting";
    return waiting;
  }
  if (!run.screen) {
    const body = document.createElement("pre");
    body.className = "worker-output";
    body.textContent = run.output ?? t("worker.noOutput", "(돌려받은 출력이 없습니다)");
    return body;
  }
  const box = document.createElement("section");
  box.className = "terminal worker-screen";
  // 그 실행이 그려진 줄 수만큼 높다. 판과 달리 페이지에는 남는 높이를
  // 나눠 줄 것이 없으므로, 화면이 자기 키를 스스로 말한다.
  box.style.setProperty("--worker-screen-rows", String(run.screen.size[0]));
  const pre = document.createElement("pre");
  pre.className = "term";
  const caret = document.createElement("div");
  caret.className = "term-caret";
  caret.hidden = true;
  box.append(pre, caret);
  // 뷰는 문서에 붙은 뒤에야 잴 수 있다 — 부르는 쪽이 붙이고 나서 그린다.
  workerScreens.set(tab.pane, { box, pre, caret, view: null, delta: run.screen });
  return box;
}

/* The page's composer — the Claude Code extension's box (docs/design/
 * agent-conversation-claude-code-grammar-20260915.md §4).
 *
 * An honest door: a helper and a piped run have no stdin of their own, so
 * what is typed here goes to the PARENT agent's pane, and the placeholder
 * says so; a pane's own conversation sends to the pane. Words travel the
 * measured road — paste (bracketed), Orca's submit breath, one Enter — the
 * same road the Rust answer walks (`answer_ask`) and the chips walk
 * (`deliverToPane`).
 *
 * One rounded box: the growing textarea above (Enter sends, Shift+Enter adds
 * a line; an Enter inside a Korean composition belongs to the composition),
 * the tool row below. Left: `+` (attachments), the agent chip — the CLI's
 * mark, name and model, opening the model menu — and the permission-mode
 * chip; a helper's page also wears the door to its parent tab. Right: `/`
 * (the palette) and the round send. A command run's page has its own context
 * line, so it calls without `owner`. */
function workerComposerNode(run, owner = null) {
  const form = document.createElement("form");
  form.className = "worker-composer";
  form.__workerRun = run;
  const box = document.createElement("textarea");
  box.className = "worker-composer-box";
  box.rows = 1;
  // A pane's own conversation sends to the pane; a helper's page sends to the
  // parent that runs it, and says so.
  const ownPane = run.helper?.id === PANE_LOG_ID || Boolean(run.wire);
  box.placeholder = ownPane
    ? t("worker.sayTo", "{{name}}에게 보내기…", { name: run.name || agentName(run.agent) })
    : t("worker.say", "부모 에이전트에게 보내기…");
  box.setAttribute("aria-label", box.placeholder);
  // 실행 중에는 상자가 「대기열에 추가」라고 말하고 턴 사이에는 제 본래의
  // 말로 돌아온다 — 그 본래의 말은 여기서 한 번 정해지므로, 갈아입히는
  // 페인터(`paintWorkerComposerState`)가 규칙을 다시 짓지 않게 들려 둔다.
  form.__composerSay = box.placeholder;
  // 쓰다 만 문장은 실행의 것이다 — 페이지가 다시 서도(탭 전환, 부모의
  // 생사) 상자가 그 문장을 다시 입는다. 폴은 애초에 이 상자를 다시 만들지
  // 않는다(`paintHelperPage`의 뼈대).
  box.value = run.draft ?? "";
  // 상자는 제 글만큼 자란다 — 상한은 CSS의 것(`--chat-composer-max-h`).
  // 붙기 전에는 잴 수 없으므로 첫 맞춤은 다음 프레임에.
  const fit = () => {
    box.style.height = "auto";
    box.style.height = `${box.scrollHeight}px`;
  };
  form.__workerFit = fit;
  requestAnimationFrame(fit);
  box.addEventListener("input", () => {
    run.draft = box.value;
    fit();
  });
  box.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey) return;
    if (event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    form.requestSubmit();
  });
  const tools = document.createElement("div");
  tools.className = "worker-composer-tools";
  // 첨부(t-2993)는 제 모듈의 것 — 칩 줄은 상자 위에, 「+」는 도구 줄 맨 왼쪽에.
  // 브라우저의 시작점은 부모 판의 체크아웃이다.
  const attachments = composerAttachments(form, box, tools, { start: owner?.worktree ?? null });
  // 턴이 끝나기를 기다리는 글들 — 첨부 칩 줄 위, 상자 바로 위 두 층.
  form.prepend(composerQueueNode());
  // The extension's row: after `+`, the agent chip (mark · name · model, opening
  // the model menu) and the permission-mode chip; on the right the `/` and the
  // send. The door to the parent stands only on a helper's page — a pane's own
  // conversation is already inside its tab.
  const spec = installedAgents().find((row) => row.id === run.agent) ?? null;
  const cwd = owner?.worktree ?? helperBase(run);
  if (spec) {
    tools.appendChild(composerAgentChip(run, spec));
    tools.appendChild(composerModeChip(run, spec));
    tools.appendChild(composerContextChip(run, spec));
  }
  // The helpers running inside this pane (the extension's footer pill) —
  // hidden while there are none, which is most of the time.
  tools.appendChild(composerAgentsChip(run));
  if (owner && !ownPane) {
    const door = document.createElement("button");
    door.type = "button";
    door.className = "worker-composer-pill worker-composer-door";
    door.append(
      iconNode("folder"),
      pillWordsNode(t("worker.inside", "{{title}} 안에서", { title: tabLabel(owner) })),
    );
    door.addEventListener("click", () => setActiveTab(owner.id));
    tools.appendChild(door);
  }
  const right = document.createElement("div");
  right.className = "worker-composer-right";
  const palette = composerSlash(form, box, run, spec ?? { id: run.agent, name: agentName(run.agent) }, cwd);
  right.appendChild(palette.slash);
  const send = document.createElement("button");
  send.type = "submit";
  send.className = "worker-composer-send";
  send.disabled = Boolean(run.sendUncertain);
  send.setAttribute("aria-label", t("worker.send", "보내기"));
  send.appendChild(iconNode("arrow-up"));
  send.addEventListener("click", async (event) => {
    if (send.classList.contains("is-stop")) {
      event.preventDefault();
      event.stopPropagation();
      if (run.wire) {
        void composerRoad(run).interrupt();
      } else if (run.term !== undefined && run.term !== null) {
        void invoke("term_key", { term: run.term, press: { key: "c", ctrl: true, alt: false } });
      }
      run.status = "idle";
      run.sending = false;
      syncWorkerComposers(run);
    }
  });
  right.appendChild(send);
  tools.appendChild(right);
  form.append(box, tools);
  const delivery = document.createElement("div");
  delivery.className = "worker-delivery";
  const deliveryWords = document.createElement("span");
  deliveryWords.textContent = t("worker.sendUncertain", "부모 입력칸에는 전달됐지만 전송 완료를 확인하지 못했습니다. 부모 터미널에서 확인해 주세요.");
  const deliveryReset = document.createElement("button");
  deliveryReset.type = "button";
  deliveryReset.className = "worker-composer-pill";
  deliveryReset.textContent = t("worker.resumeInput", "확인 후 입력칸 다시 사용");
  deliveryReset.onclick = () => {
    run.sendUncertain = false;
    delivery.hidden = true;
    send.disabled = false;
    box.focus();
  };
  delivery.append(deliveryWords, deliveryReset);
  delivery.hidden = !run.sendUncertain;
  form.appendChild(delivery);
  paintWorkerComposerState(form, run);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (run.sending || run.sendUncertain) return;
    const draft = box.value;
    run.draft = draft;
    const text = box.value.trim();
    run.sending = true;
    syncWorkerComposers(run);
    let pasted = false;
    // 칩이 없으면 글만 — 확인할 것이 없으니 첫 붙여넣기가 동기로 나간다. 칩이
    // 있으면 보내기 직전에 경로를 확인하고(없는 것은 칩에 남는다) 「글 + 빈 줄
    // + 첨부: + 경로들」의 형식을 입는다(t-2993). 칩은 보낸 뒤 비고, 살아남는
    // 초안은 글뿐이다.
    try {
      const message = attachments.count() === 0 ? text || null : await attachments.outgoing(text);
      if (message === null) return;
      // 실행 중에 친 글은 창이 들고 있다가 턴이 끝나면 보낸다 — 첨부까지
      // 조립된 그대로가 대기열에 든다(`settleComposerQueue`). 여기서
      // `sendUncertain`을 다시 보지 않는 것은 이 핸들러의 첫 줄이 이미
      // 그 상태를 돌려보냈기 때문이다.
      if (composerWorking(run)) queueComposerMessage(run, message);
      // 아니면 그 자리에서 실행의 길로 — 선이면 한 요청, 판이면
      // 붙여넣기·숨·Enter. `pasted`는 Enter가 실패했을 때 글이 이미 부모의
      // 상자에 있다는 사실을 남긴다.
      else await composerDeliver(run, message, () => { pasted = true; });
      if (run.draft === draft && box.value === draft) {
        box.value = "";
        run.draft = "";
        syncWorkerComposers(run, draft);
        fit();
      }
    } catch (error) {
      if (pasted) {
        run.sendUncertain = true;
        delivery.hidden = false;
      }
      showError(error);
    } finally {
      run.sending = false;
      syncWorkerComposers(run);
    }
  });
  return form;
}

function paintWorkerComposerState(form, run, delivered = null) {
  const box = form.querySelector(".worker-composer-box");
  if (delivered !== null && box.value === delivered && run.draft === "") {
    box.value = "";
    form.__workerFit?.();
  }
  box.readOnly = Boolean(run.sending);
  const send = form.querySelector(".worker-composer-send");
  const working = composerWorking(run);
  const stopping = working && !run.sending && !run.sendUncertain;
  // 기다리는 글들, 그리고 상자가 무엇을 하겠다고 말하는지: 실행 중이면
  // 「대기열에 추가」, 턴 사이면 제 본래의 말.
  paintComposerQueue(form, run);
  const saying = working
    ? t("composer.queue.placeholder", "다음 메시지 대기열에 추가…")
    : form.__composerSay;
  writeAttribute(box, "placeholder", saying);
  writeAttribute(box, "aria-label", saying);

  if (stopping) {
    send.disabled = false;
    send.classList.add("is-stop");
    send.setAttribute("aria-label", t("worker.stop", "중지"));
    send.replaceChildren(iconNode("square"));
  } else {
    send.classList.remove("is-stop");
    send.disabled = Boolean(run.sending || run.sendUncertain);
    send.setAttribute("aria-label", t("worker.send", "보내기"));
    send.replaceChildren(iconNode("arrow-up"));
  }

  const spec = installedAgents().find((row) => row.id === run.agent) ?? null;
  const agentChip = form.querySelector(".worker-composer-agent");
  if (agentChip) paintComposerAgentChip(agentChip, run, agentModelLists.get(run.agent)?.rows ?? run.wireModels ?? null);
  const modeChip = form.querySelector(".worker-composer-mode");
  if (modeChip && spec) paintComposerModeChip(modeChip, run, spec);
  const agentsChip = form.querySelector(".worker-composer-agents");
  if (agentsChip) paintComposerAgentsChip(agentsChip, run);
  const contextChip = form.querySelector(".worker-composer-context");
  if (contextChip && spec) paintComposerContextChip(contextChip, run, spec);
  // The send and the focus ring wear the permission mode's reach — the
  // extension colours both by it (`[data-permission-mode]`).
  wearReach(form, composerReachOf(run));
  form.querySelector(".worker-delivery").hidden = !run.sendUncertain;
}

function syncWorkerComposers(run, delivered = null) {
  for (const form of document.querySelectorAll(".worker-composer")) {
    if (form.__workerRun === run) paintWorkerComposerState(form, run, delivered);
  }
}

/* Every composer that speaks to `term` — its chips and its state —
 * repainted when the pane's model, permission mode or hook state moves
 * (`hook:agent`, once the state has moved). */
function paintComposerChipsFor(term) {
  for (const form of document.querySelectorAll(".worker-composer")) {
    const run = form.__workerRun;
    if (run?.term === term) paintWorkerComposerState(form, run);
  }
}

/* 알약의 글 — 아이콘 옆의 낱말 한 덩이. 아이콘은 글이 없으므로 알약의
 * textContent는 이 낱말 그대로다. */
function pillWordsNode(words) {
  const span = document.createElement("span");
  span.className = "worker-composer-pill-words";
  span.textContent = words;
  return span;
}

function paintWorkerScreen(pane) {
  const held = workerScreens.get(pane);
  if (held === undefined || held.view !== null) return;
  held.view = makeTermView(held.box, held.pre, held.caret);
  held.view.measure();
  held.view.apply(held.delta);
}

/* 페이지 머리 하나 — 이름, 시계, 상태. 헬퍼의 뼈대는 폴마다 이 글자들만
 * 갈아입고(`updateWorkerHead`), 명령 실행의 페이지는 세울 때마다 새로
 * 받는다. */
function workerHeadNode(run) {
  const head = document.createElement("header");
  head.className = "worker-head";
  // The agent's own mark (`✻` for Claude Code) in the accent, then the name.
  const mark = agentMarkNode("worker-mark", "");
  const name = document.createElement("span");
  name.className = "worker-name";
  const clock = document.createElement("span");
  clock.className = "worker-elapsed";
  // 행이 말하는 그 수를 페이지도 말한다 — 같은 헬퍼를 두 표면이 다른 수로
  // 부르지 않도록 낱말은 한 곳(`toolUsesWords`)에서 온다. 명령 실행의 페이지는
  // 셀 것이 없으므로 이 자리가 비고, 빈 자리는 그리지 않는다(`:empty`).
  const uses = document.createElement("span");
  uses.className = "worker-uses";
  const state = document.createElement("span");
  state.className = "worker-state";
  // 이름은 왼쪽, 나머지는 오른쪽의 연한 메타 한 덩이(t-2973). 그 덩이의
  // 맨 앞에 Focus view의 토글이 선다 — 턴을 가진 페이지에만: 명령 실행의
  // 페이지가 그리는 것은 화면이지 턴이 아니라, 접을 것이 없다.
  const meta = document.createElement("span");
  meta.className = "worker-meta";
  if (run.helper) meta.appendChild(focusViewButtonNode());
  meta.append(clock, uses, state);
  head.append(mark, name, meta);
  updateWorkerHead(head, run);
  return head;
}

function updateWorkerHead(head, run) {
  writeTextContent(head.querySelector(".worker-mark"), agentVoice(run.agent).glyph);
  // 토글의 눌림은 창의 한 값이라, 다른 페이지에서 바뀌었어도 이 머리가
  // 다음 그림에 따라온다 — 값이 움직였을 때만 쓴다.
  const focus = head.querySelector(".worker-focus");
  if (focus) writeAttribute(focus, "aria-pressed", focusViewOn() ? "true" : "false");
  head.querySelector(".worker-name").textContent = run.name;
  head.querySelector(".worker-elapsed").textContent = workerElapsedWords(run);
  head.querySelector(".worker-uses").textContent = toolUsesWords(run.toolCalls ?? 0);
  const state = head.querySelector(".worker-state");
  state.className = `worker-state is-${run.status}`;
  state.textContent = workerStatusWords(run);
}

/* Where it runs: the parent pane by its own tab name, or the fact that
 * the parent is gone — both are one line the header owes. While the parent
 * stands, the line is a DOOR to it: the conversation this run happened
 * inside is the one a person pressing around this page is looking for
 * ("수동으로 resume 했어 … 아무것도 안해" — they pressed the fresh-launch
 * button expecting to continue THIS run's conversation, which lives in the
 * parent tab, not behind a new launch). */
function workerWhereNode(run, owner) {
  const where = document.createElement(owner ? "button" : "p");
  where.className = owner ? "worker-where is-door" : "worker-where";
  where.textContent = owner
    ? t("worker.inside", "{{title}} 안에서", { title: tabLabel(owner) })
    : t("worker.parentGone", "부모 터미널 없음");
  if (owner) {
    where.type = "button";
    where.addEventListener("click", () => setActiveTab(owner.id));
  }
  return where;
}

/* 헬퍼 페이지는 뼈대를 한 번 세우고 그 위에 산다.
 *
 * 머리·문맥 줄·입력줄은 고정이고 전사만 스크롤한다. 같은 페이지의 다음
 * 그림은 글자와 턴만 갈아입는다 — 입력줄의 초점과 쓰다 만 문장이 폴마다
 * 살아남는 것은 이 뼈대가 서 있기 때문이다. 다시 세우는 것은 페이지가
 * 바뀌었거나, 부모의 생사가 바뀌었거나(입력줄과 문이 함께 바뀐다), 창의
 * 말이 바뀌었을 때뿐이다. */
function paintHelperPage(host, tab) {
  const run = tab.worker;
  const owner = tabOfTerm(run.term);
  const held = host.__helperPage;
  // The accent the page wears is the agent's (`--chat-accent` reads it).
  writeAttribute(host, "data-agent", run.agent ?? "");
  if (held && held.id === tab.id && held.owned === Boolean(owner) && held.locale === locale) {
    updateWorkerHead(held.head, run);
    syncHelperTurns(held.turns, run);
    updateHelperStatus(held.status, run);
    if (run.wire) paintWireAsk(host, run);
    return;
  }
  dropWorkerScreen(tab.pane);
  host.__dockWatch?.disconnect();
  host.classList.add("is-chat-page");
  host.replaceChildren();
  const head = workerHeadNode(run);
  const turns = helperTurnsNode(run);
  // The status line is the list's own last row, as the extension's spinner
  // row is (`spinnerRow` under the last message): it scrolls with the words
  // and stands under the last of them.
  const status = helperStatusNode(run);
  turns.appendChild(status);
  host.append(head, turns);
  // 문맥은 입력줄의 알약이 말한다. 부모가 없으면 입력줄도 없으므로 그 사실
  // 한 줄만 남는다 — 선 위의 세션은 부모 없이도 제 입력줄을 가진다(보내기가
  // 선으로 간다). The composer floats over the list's foot in the
  // extension's dock (`inputContainer`), the question card inside it.
  if (owner || run.wire) host.appendChild(chatDockNode(host, workerComposerNode(run, owner)));
  else host.insertBefore(workerWhereNode(run, null), turns);
  if (run.wire) paintWireAsk(host, run);
  // 처음 서는 페이지는 끝에서 연다 — 사람이 읽는 것은 언제나 끝이다.
  turns.scrollTop = turns.scrollHeight;
  host.__helperPage = { id: tab.id, owned: Boolean(owner), locale, head, turns, status };
}

/* The extension's dock at the foot of the conversation (`inputContainer`):
 * the composer — and the question card, when one stands — floating over the
 * list's last rows, inset by the dock's margins and no wider than its
 * measure. The list keeps room under its words for it: the dock's height,
 * watched, rides the page as `--chat-dock-h`. */
function chatDockNode(host, composer) {
  const dock = document.createElement("div");
  dock.className = "chat-dock";
  dock.appendChild(composer);
  if (typeof ResizeObserver === "function") {
    const watch = new ResizeObserver(() => {
      host.style.setProperty("--chat-dock-h", `${dock.offsetHeight}px`);
    });
    watch.observe(dock);
    host.__dockWatch = watch;
  }
  return dock;
}

function paintWorkerView(tab) {
  const host = docHost(tab.pane, "worker");
  const run = tab.worker;
  // A HELPER's page is its conversation, standing on its own skeleton. The
  // blocks below belong to a nested COMMAND run — a command line to show, a
  // door to an interactive session, an output the pipe handed back when it
  // ended — and a helper has none of those: it was never a command, and it
  // is still speaking.
  if (run.helper) {
    paintHelperPage(host, tab);
    return;
  }
  // 앞 그림이 이 호스트에 걸어 둔 것을 먼저 놓아 준다 — 노드가 사라지는 줄이
  // 바로 다음 줄이다. 헬퍼의 뼈대도 같은 순간에 놓인다: 이 호스트는 이제
  // 다른 페이지의 것이다.
  dropWorkerScreen(tab.pane);
  host.__helperPage = null;
  host.classList.remove("is-chat-page");
  writeAttribute(host, "data-agent", run.agent ?? "");
  host.replaceChildren();
  host.appendChild(workerHeadNode(run));
  const owner = tabOfTerm(run.term);
  host.appendChild(workerWhereNode(run, owner));
  // The command as typed — folded behind its first line when it is longer
  // than an eye (Orca's exec card): an argv is a fact to keep, not a story
  // the page scrolls past.
  const commandWords = run.command || t("worker.noCommand", "(명령을 읽지 못했습니다)");
  if (chatFolds(commandWords)) {
    host.appendChild(foldCardNode(
      "worker-command",
      [iconNode("terminal"), foldWhoNode("exec"), foldCueNode(commandWords.split("\n", 1)[0])],
      foldBodyNode(commandWords),
    ));
  } else {
    const command = document.createElement("pre");
    command.className = "worker-command";
    command.textContent = commandWords;
    host.appendChild(command);
  }
  // The run this page witnesses is a PIPE the coordinator owns — `codex exec`
  // never draws a composer, so the input box a person reaches for cannot be
  // here ("인풋창도 없음"). The door to one can: the same interactive launch
  // every other road uses (`launch_agent_tab` — injection table, ready
  // signal), fresh conversation, same checkout.
  const spec = installedAgents().find((row) => row.id === run.agent);
  if (spec) {
    const live = document.createElement("button");
    live.className = "btn worker-open-live";
    live.type = "button";
    // Named for what it DOES: a fresh conversation. The old name ("open
    // interactively") read as "continue this run", and a person who pressed
    // it met an empty composer and called it broken — the run is a pipe, and
    // a pipe has no conversation to resume.
    live.textContent = t("worker.openLive", "새 {{name}} 대화 열기", { name: spec.name });
    live.addEventListener("click", async () => {
      try {
        const term = await launchAgentTab({
          agent: spec.id,
          prompt: "",
          ...spawnGrid(),
        });
        mountTermTab(term, { agent: spec.name });
      } catch (error) {
        showError(error);
      }
    });
    host.appendChild(live);
    const why = document.createElement("p");
    why.className = "worker-open-live-why";
    why.textContent = t(
      "worker.openLiveWhy",
      "이 실행은 파이프로 돌아 이어받을 대화가 없습니다 — 새 대화가 열립니다.",
    );
    host.appendChild(why);
  }
  // 도는 동안 보여줄 것이 없는 이유가 있으면, 그 이유를 말한다.
  if (run.status === "running" && mirrorReady === false) {
    const why = document.createElement("p");
    why.className = "worker-no-mirror";
    why.textContent = t(
      "worker.noMirror",
      "이 기계에는 미러가 없어 도는 동안의 출력을 보여줄 수 없습니다 — 실행이 끝나면 여기에 옵니다.",
    );
    host.appendChild(why);
  }
  if (run.status !== "running") {
    // The answer under its own folding header — "결과 · 완료 ⌄", Orca's run
    // view exactly, worn as the ONE fold this surface owns. Open by default:
    // the page exists to show it. Reopening re-measures the screen — a box
    // sized while `details` kept it closed measured a zero-width room.
    const result = foldCardNode(
      "worker-result",
      [foldWhoNode(t("worker.result", "결과")), foldCueNode(workerStatusWords(run))],
      workerBodyNode(tab),
      true,
    );
    if (run.truncated) {
      const cut = document.createElement("p");
      cut.className = "worker-cut";
      cut.textContent = t("worker.cut", "출력이 길어 앞부분만 보입니다.");
      result.appendChild(cut);
    }
    host.appendChild(result);
    paintWorkerScreen(tab.pane);
    result.addEventListener("toggle", () => {
      if (result.open) workerScreens.get(tab.pane)?.view?.measure();
    });
  }
  if (owner) host.appendChild(workerComposerNode(run));
}

/* 페이지도 판과 같은 숨을 쉰다.
 *
 * 갈라지는 판에는 이미 그 규칙이 있다(`MIRROR_SETTLE_MS`, 바로 아래 — "실행이
 * 판을 얻으려면 한 숨은 살아 있어야 한다"): 코디네이터는 에이전트를 먼저 찔러
 * 본다(버전 확인, 능력 탐지). 그때마다 화면을 가르면 도움말 덤프로 가득 찬다.
 *
 * 그런데 그 숨을 **판만 쉬고 페이지는 안 쉬었다**. 그래서 0초에 끝난
 * `claude --help | sed -n '1,260p'` 하나가 탭을 통째로 가져갔다(신고
 * 2026-08-21, 스크린샷 "이것도 버그인지"). 같은 박자를 페이지에도 준다 —
 * 그 안에 끝난 실행은 페이지를 열지 않는다. 그 출력은 부모의 화면에 이미
 * 찍혀 있고(shim은 사본만 떠 간다), 남는 것은 아무도 안 본 탭 하나뿐이다.
 *
 * 시작 시각은 **소식이 온 순간**을 들고 간다. 박자를 기다리느라 시계가 한
 * 숨씩 늦어지면, 페이지가 말하는 경과가 실제보다 짧아진다. */
const pendingWorkerPages = new Map();

listen("worker:opened", (event) => {
  const { id, term, agent, name, command, worktree } = event.payload;
  if (workerDismissed.has(id)) return;
  const tabId = `worker:${id}`;
  const held = tabs.find((tab) => tab.id === tabId);
  if (held) {
    Object.assign(held.worker, { term, agent, name, command });
    if (held.id === activeTabId) paintWorkerView(held);
    return;
  }
  if (pendingWorkerPages.has(id)) return;
  const startedAt = Date.now();
  pendingWorkerPages.set(id, {
    term,
    timer: setTimeout(() => {
      pendingWorkerPages.delete(id);
      openWorkerPage({ id, term, agent, name, command, worktree }, startedAt);
    }, MIRROR_SETTLE_MS),
  });
});

function openWorkerPage({ id, term, agent, name, command, worktree }, startedAt) {
  if (workerDismissed.has(id)) return;
  const tabId = `worker:${id}`;
  // The page belongs to the checkout the RUN happened in, which is the one
  // its parent terminal was cut for — not whichever workspace happened to be
  // in front when the vendor's first line arrived. `openTab` stamps
  // `activeWorktreePath` by default, and that default put a zerocode run's
  // page (and its live door, which launches into `active_root()`) on lotto's
  // strip: "코덱스 창이 왜 lotto에서 열려 ... 내가 요청한창은 zerocode".
  //
  // The payload's own answer first: the hook envelope carries the checkout
  // the run reported from, so a run that announces itself before its parent
  // terminal has been mounted — a restored tab, a background pane — still
  // lands where it belongs. The parent TAB is the fallback, and the active
  // checkout only when neither can say.
  const parent = tabOfTerm(term);
  const home = worktree || parent?.worktree || activeWorktreePath;
  // And a seat that exists in THAT checkout's stage. Groups are per checkout
  // (`stageTrees`), so `openTab`'s default — the group focused in whatever is
  // in front — names a pane the home checkout's tree has never heard of, and
  // `paneTabs` would then never list the page on the strip it belongs to: a
  // tab that is neither here nor there.
  const seat = parent?.pane ?? homeStageSeat(home);
  // In FRONT, unconditionally. The first cut opened it behind unless the
  // person was watching the parent pane, and the report came straight back a
  // third time: "아직도 안 보이는데". A page that opens where nobody sees it
  // has not opened; the person who finds it rude has the close button, and a
  // closed page stays closed for the run.
  //
  // "Unconditionally" means within its own workspace. `paneTabs` shows a tab
  // only on the strip of the checkout that owns it, so pulling another
  // workspace's page to the front would make it the active tab of a strip it
  // is not on — active and invisible at once. It waits on its own strip.
  openTab({
    id: tabId,
    kind: "worker",
    worktree: home,
    pane: seat,
    worker: {
      id,
      term,
      agent,
      name,
      command,
      status: "running",
      startedAt,
      output: null,
      truncated: false,
    },
  }, { focus: home === activeWorktreePath });
  workerClock.sync();
  // 처음 한 번만 진짜로 묻는다. 답이 "없음"이고 이 페이지가 앞에 있으면 이유를
  // 실어 다시 그린다 — 페이지가 이미 그려진 뒤에 답이 오기 때문이다.
  void askMirrorReady().then((ready) => {
    if (ready) return;
    const page = tabs.find((one) => one.id === tabId);
    if (page && page.id === activeTabId) paintWorkerView(page);
  });
}

listen("worker:done", (event) => {
  const { id, status, output, truncated } = event.payload;
  // 아직 숨을 쉬는 중이었다면 페이지는 서지 않는다 — 미러 판이 같은 자리에서
  // 같은 이유로 하는 일과 같다(`worker:mirror_end`).
  const waiting = pendingWorkerPages.get(id);
  if (waiting !== undefined) {
    clearTimeout(waiting.timer);
    pendingWorkerPages.delete(id);
    return;
  }
  const tab = tabs.find((one) => one.id === `worker:${id}`);
  if (!tab) return;
  Object.assign(tab.worker, {
    status,
    output: output ?? null,
    truncated: Boolean(truncated),
    endedAt: Date.now(),
  });
  workerClock.sync();
  // 이 바이트가 화면이었는지 글이었는지는 에뮬레이터만 안다. 페이지를 그리기
  // 전에 물어 두면, 앞에 서 있는 페이지도 한 왕복 뒤에 답을 받아 다시 그린다.
  askWorkerScreen(tab);
  if (tab.id === activeTabId) paintWorkerView(tab);
  renderTabs();
});

/* ---- 미러가 살아나면 페이지 대신 진짜 판이 갈라진다 (1-fm) ----
 *
 * The shim on the terminals' own PATH tees a nested run's bytes into a file,
 * and this window pours that file into a REAL pane beside the parent — the
 * orchestration shape the person asked for by name: "창이 분할되면서
 * 터미널로". ANSI arrives intact and the grid draws it natively; the info
 * page (1-fk) stays as the fallback for panes with no tab on screen and for
 * runs the shim never saw. */
/* A run must outlive a breath before it earns a pane. A coordinator sniffs
 * its agents first — a version check, a capability probe — and splitting the
 * screen for each of those filled it with help dumps ("codex 화면이 재대로
 * 동작안해"). The split waits one settle beat; a run whose end arrives inside
 * it never touches the layout. */
const MIRROR_SETTLE_MS = 1200;
const pendingMirrors = new Map();
/* One seat per (parent, agent): the next run replaces the last run's tail
 * instead of stacking a new pane beside a dead one. */
const mirrorPanes = new Map();

listen("worker:mirror", (event) => {
  const { term, agent, path } = event.payload;
  const timer = setTimeout(() => {
    pendingMirrors.delete(path);
    openMirrorPane(term, agent, path);
  }, MIRROR_SETTLE_MS);
  pendingMirrors.set(path, timer);
});

listen("worker:mirror_end", (event) => {
  const { path } = event.payload;
  const timer = pendingMirrors.get(path);
  if (timer === undefined) return;
  clearTimeout(timer);
  pendingMirrors.delete(path);
});

async function openMirrorPane(term, agent, path) {
  const tab = tabOfTerm(term);
  if (!tab || tab.kind !== "term") return;
  // The page yields to the real screen — same run, better window. Closing it
  // records a dismissal, which is also right: the mirror pane is this run's
  // surface now, and the page must not come back beside it.
  const page = tabs.find(
    (one) =>
      one.kind === "worker" &&
      one.worker.term === term &&
      one.worker.agent === agent &&
      one.worker.status === "running",
  );
  if (page) closeTab(page.id);
  // The pane's first bytes wrap at the width the pty is BORN with, and a
  // fixed 96 in a half-width pane painted three broken columns ("내용이 다
  // 안그려지고"). The parent's own grid says what half of this tab actually
  // holds; `renderPanes` still resizes to the real cell box after the split.
  const grid = viewOfTerm(term)?.gridSize() ?? DEFAULT_TERMINAL_GRID;
  let mirror;
  try {
    mirror = await invoke("open_mirror_term", {
      path,
      rows: grid.rows,
      cols: Math.max(40, Math.floor((grid.cols - 1) / 2)),
    });
  } catch {
    // The page (if any) already told the story; a broken mirror must never
    // break the run it was only watching.
    return;
  }
  const seat = `${term}:${agent}`;
  const held = mirrorPanes.get(seat);
  if (held !== undefined && paneLeaves(tab.layout).includes(held)) {
    invoke("close_term", { term: held }).catch(() => {});
    tab.layout = prunePanes(
      tab.layout,
      new Set(paneLeaves(tab.layout).filter((leaf) => leaf !== held)),
    );
    dropTermView(held);
  }
  mirrorPanes.set(seat, mirror);
  const source = activePaneOf(tab) ?? paneLeaves(tab.layout)[0];
  tab.layout = addPane(tab.layout, source, mirror, "row");
  // The keyboard stays with the parent — the person is watching a run, not
  // typing at a tail.
  renderPanes(tab);
  updateStage();
  paintRunningCount();
  // Named as what it IS — a log the run writes, not the agent's own screen.
  // A pane titled "Codex" invites typing at a tail that cannot hear it.
  noteTermTitle(
    mirror,
    t("worker.logPane", "{{name}} 로그", {
      name: agent.charAt(0).toUpperCase() + agent.slice(1),
    }),
  );
}

/* The parent leaving ends every page still waiting on it — a clock that
 * keeps counting for a process whose parent died is a page lying. */
function orphanWorkersOf(term) {
  for (const [id, waiting] of pendingWorkerPages) {
    if (waiting.term !== term) continue;
    clearTimeout(waiting.timer);
    pendingWorkerPages.delete(id);
  }
  let touched = false;
  for (const tab of tabs) {
    if (tab.kind !== "worker" || tab.worker.term !== term) continue;
    if (tab.worker.status !== "running") continue;
    Object.assign(tab.worker, { status: "orphaned", endedAt: Date.now() });
    if (tab.id === activeTabId) paintWorkerView(tab);
    touched = true;
  }
  workerClock.sync();
  if (touched) renderTabs();
}

listen("term:exited", (event) => {
  zoIntegrationRecords.delete(`term-${event.payload.term}`);
  if (event.payload.term === skillsInstallTerm) { skillsNeedsRescan = true; void refreshSkills(true); }
  const { term } = event.payload;
  forgetManagedTerminalSession(term);
  forgetCodexPtyFallback(term);
  paneUsage.delete(term);
  orphanWorkersOf(term);
  if (term === FLOAT_TERM) {
    // The chord is read inside, not captured: it can be rebound while the
    // dead terminal's header is still on screen.
    say(el("term-title"), () => {
      const shellChord = optionalShortcutLabel("terminal.toggle");
      return shellChord === null
        ? t("terminal.exited", "터미널 — 종료됨")
        : t("terminal.exitedChord", "터미널 — 종료됨 ({{chord}}로 새 셸)", { chord: shellChord });
    });
    floatView.reset();
    setTermVisible(false);
    return;
  }
  // If a job's history is open, that shell may be one of its runs — the
  // backend stamped the ending into the ledger before it told us, so the row
  // is re-read rather than guessed at here. Only while the form is open:
  // nothing else on screen reads this file, and a closed panel would be an
  // invoke per terminal anybody ever closes.
  if (!autoView.hidden && autoSelectedId) void refreshAutomationRuns();
  // 미리보기가 보고 있던 셸이 죽었다. 화면을 조용히 치우면 왜 비었는지
  // 말하지 않는 상자가 남으므로, 말하고 나서 치운다.
  if (peekTerm === term) el("peek-closed").hidden = false;
  // The shell ended on its own (the person typed `exit`). What goes with it
  // is the PANE it was, not the tab — a tab that was split still has shells
  // in it, and taking the whole tab away would kill the ones still running.
  // The tab goes when its last pane does: a terminal tab with no process is a
  // tab showing nothing. The tab is found by looking for the shell rather
  // than by deriving an id from it, because only the first pane's id names
  // the tab and any pane can be the one that exits.
  const holder = tabs.find(
    (tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(term),
  );
  dropTermView(term);
  // No tab holds this shell YET: a restore assigns its layout only after
  // every leaf has spawned, and a resume that dies inside that window would
  // otherwise slip this prune and mount as a pane nothing ever clears. The
  // id is parked and `mountStoredLayout` prunes it before the layout stands.
  if (!holder) {
    exitedUnheld.add(term);
    return;
  }
  const staying = paneLeaves(holder.layout).filter((held) => held !== term);
  if (staying.length === 0) {
    dropTab(holder.id);
    return;
  }
  holder.layout = prunePanes(holder.layout, new Set(staying));
  schedulePaneAlignment(holder);
  if (holder.activePane === term) holder.activePane = staying[0];
  renderPanes(holder);
  updateStage();
  paintRunningCount();
});

/* An orchestration worker owns a terminal in its resolved worktree.
 *
 * This is deliberately a different surface from Claude Agent Teams below.
 * A protocol worker created in a child checkout must appear under that
 * checkout's card without splitting or focusing the coordinator's tab. The
 * parent edge remains agent lineage, not visual pane ownership. */
function seatLedgerManagedTerm(term, worktree, agent) {
  return reseatTerm(term, {
    worktree,
    ledgerManaged: true,
    ...(agent ? { agent: agentSaidName(agent) } : {}),
  });
}

/* Put `term` in the worktree `extra.worktree` names, as one operation: mount
 * it there if no tab holds it, detach its leaf if a split does, or adopt the
 * tab that is only it and move the ownership metadata. Shared by the ledger's
 * seating and by a pane following its own process (`followPaneIntoWorktree`). */
function reseatTerm(term, extra) {
  const { worktree } = extra;
  const held = tabs.find(
    (tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(term),
  );
  if (!held) {
    return mountTermTab(term, extra, { focus: false, placement: "tab" });
  }

  // A restored worker can be tiled into its coordinator's tab before the
  // ledger re-seats it. Detach only that leaf; adopting the tab would turn
  // the coordinator's whole split into the worker's checkout.
  const staying = paneLeaves(held.layout).filter((leaf) => leaf !== term);
  if (staying.length > 0) {
    held.layout = prunePanes(held.layout, new Set(staying));
    if (held.activePane === term) held.activePane = staying[0];
    renderPanes(held);
    return mountTermTab(term, extra, { focus: false, placement: "tab" });
  }

  // A generic layout restored just before the ledger re-seated this term can
  // already have a tab around it. Adopt that one instead of opening a second
  // view, and move its ownership metadata as one operation.
  const previousWorktree = held.worktree;
  Object.assign(held, extra);
  if (previousWorktree && previousWorktree !== worktree) {
    persistPaneLayouts(previousWorktree);
  }
  // And the checkout it belongs to NOW. This tab has just become the
  // ledger's, so the generic set must stop naming it — the adopt branch is
  // reached exactly when the file still holds the conversation the ledger has
  // taken over, and a file left saying so resumes that one conversation twice
  // on the next restart: once from the record, once from the ledger.
  persistPaneLayouts(worktree);
  renderTabs();
  updateStage();
  return held;
}

/* A pane follows its agent. zo's EnterWorktree — and any `cd` — moves the
 * foreground process into another checkout while the tab keeps the worktree
 * it was born in; the sidebar reads the tab, so the work showed under `main`
 * and the new worktree looked empty (live report 2026-09-09). The backend
 * says where the process stands (pane_cwd_runtime, `term:cwd`); the tab moves
 * to the worktree that contains that path — the deepest one, so a checkout
 * under `.zo/worktrees/` wins over the project root that also contains it,
 * and a sibling that merely shares a prefix never counts. */
function worktreeContaining(path) {
  let best = null;
  for (const project of projects) {
    for (const worktree of project.worktrees) {
      const root = worktree.path.replace(/\/+$/, "");
      if (path !== root && !path.startsWith(`${root}/`)) continue;
      if (!best || root.length > best.path.replace(/\/+$/, "").length) best = worktree;
    }
  }
  return best;
}

function followPaneIntoWorktree(term, cwd, { refreshed = false } = {}) {
  const tab = tabOfTerm(term);
  if (!tab) return;
  // Only an agent's pane follows. A plain shell reports where its foreground
  // process stands too, but a person cd-ing into another checkout must not
  // have their tab jump groups under them.
  if (!tab.agent && !paneAgents.has(term)) return;
  const home = worktreeContaining(cwd);
  if (!home) {
    // A checkout the agent just created is not in the catalog yet — ask once.
    if (!refreshed) {
      void refreshWorktrees()
        .then(() => followPaneIntoWorktree(term, cwd, { refreshed: true }))
        .catch(() => {});
    }
    return;
  }
  if (home.path === tab.worktree) return;
  const agent = paneAgents.get(term);
  reseatTerm(term, { worktree: home.path, ...(agent ? { agent: agentSaidName(agent) } : {}) });
  void refreshWorktrees().then(paintWorktreeAgents).catch(showError);
}

listen("term:cwd", (event) => {
  const { term, cwd } = event.payload ?? {};
  if (typeof term !== "number" || typeof cwd !== "string" || cwd.length === 0) return;
  followPaneIntoWorktree(term, cwd);
});

/* Where a fresh worker's pane goes, when the placement seat acts.
 *
 * The seat is asked with what this surface knows — how many panes the
 * coordinator's tab holds, whether the layout rule would cut one, what is in
 * front, whether anybody is at the keyboard — and the words the backend sent
 * with the event (the summons' brief, its ids). The door records the row
 * either way; only an answer from a seat that ACTS (`applied`) moves the
 * pane, and any failure on this road seats the pane as a tab, which is what
 * every worker got before the seat existed (2026-09-20, "전부 자동 기록하며
 * 실제 적용되어야"). */
async function roomForWorker({ parent, term, worktree, seat }) {
  if (!seat || typeof seat.brief !== "string" || seat.brief.length === 0) return "tab";
  const host = tabs.find((tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(parent)) ?? null;
  const front = tabs.find((tab) => tab.id === activeTabId) ?? null;
  const shape = activePaneShape(host);
  const sameWorkspace = Boolean(host && host.worktree === activeWorktreePath);
  const placement = host
    ? tilePlacement({ sameWorktree: sameWorkspace, activeIsTermTab: front?.kind === "term", ...shape })
    : { tab: true };
  const look = {
    brief: seat.brief,
    briefChars: seat.briefChars ?? seat.brief.length,
    startedBy: document.hasFocus() ? "person" : "schedule",
    inFront: front?.kind === "term" ? "terminal" : front ? "page" : "nothing",
    sameWorkspace,
    panes: shape.paneCount,
    maySplit: Boolean(placement.split),
    run: seat.run,
    worker: seat.worker,
    dispatch: seat.dispatch ?? null,
    task: seat.task ?? null,
    checkout: worktree,
  };
  try {
    const judged = await invoke("judge_worker_room", { look });
    // An answer the door put in its book waits for what the person does with
    // the pane; this surface is the only one that can see that, so it keeps
    // the worker's name by the term until it has reported one move.
    if (judged?.placed && typeof seat.worker === "string") placedWorkers.set(term, { worker: seat.worker });
    if (judged?.applied && typeof judged.chosen === "string") return judged.chosen;
  } catch {
    // A backend without the door, or a door that refused: the tab it is.
  }
  return "tab";
}

/* The person moved a placed worker's pane to `room` — one of the seat's
 * three rooms — and the placement seat's label is written from it (t-5806).
 * Reported once per placed pane, by the roads a PERSON takes: a tab dragged
 * to another group or split, a tiled pane closed into the background, a
 * parked worker brought back to a tab. The seat's own re-seat and the
 * window's parking of a team's overflow are not the person and never come
 * through here. A pane no answer is waiting on reports nothing. */
function noteWorkerRoomChange(term, room) {
  const placed = placedWorkers.get(term);
  if (!placed) return;
  placedWorkers.delete(term);
  invoke("note_worker_room_change", { worker: placed.worker, room }).catch(() => {});
}

/* Move a just-seated worker to the room the placement seat named, when the
 * seat acts. `split` puts the pane beside its coordinator the way
 * `term:split` does — its own tab is dropped first, so the term is in one
 * place; `background` parks it as a detached agent, whose roster row is its
 * door back. `tab` and every failure on the road leave the tab as seated. */
function reseatWorkerByAnswer(term, parent, worktree, agent, room) {
  if (room !== "split" && room !== "background") return;
  const own = tabOfTerm(term);
  if (!own || paneLeaves(own.layout).length !== 1 || own.activePane !== term) return;
  const host = tabs.find((tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(parent)) ?? null;
  if (room === "split" && !host) return;
  tabs.splice(tabs.indexOf(own), 1);
  if (room === "split") {
    const onStage = host.id === activeTabId && host.worktree === activeWorktreePath;
    tileTermPane(host, term, agent ? { agent: agentSaidName(agent) } : {}, "down", onStage, parent);
    schedulePaneAlignment(host);
  } else {
    detachedAgents.set(term, worktree);
    dropTermScreen(term);
  }
  renderTabs();
  updateStage();
  scheduleAgentPaint(["cards", "board"]);
}

listen("term:worker", (event) => {
  const { parent, term, worktree, agent, resumed, helper, seat } = event.payload ?? {};
  if (
    typeof parent !== "number" ||
    typeof term !== "number" ||
    typeof worktree !== "string" ||
    worktree.length === 0
  ) return;
  paneParents.set(term, parent);
  // And which helper this pane IS, when the seat said — the same word
  // `term:split` carries (t-3024). A worker-surface helper arrived without it
  // and never folded: its roster row kept the parent's clock, and its click
  // went to the parent's pane while its own tab stood unreached
  // (2026-09-13, "implement#0 눌러도 안 보임").
  if (typeof helper === "string" && helper.length > 0) paneHelpers.set(term, helper);
  restoringWorkers.delete(worktree);
  if (agent) paneAgents.set(term, agent);
  if (resumed === "session" || resumed === "fresh") restoredWorkers.set(term, resumed);
  // Seat first. The card may not exist yet, but the tab is the authoritative
  // term→worktree edge that `paintWorktreeAgents` reads when the refresh
  // materializes it. Refreshing first leaves one frame with no owner and, if
  // no hook follows, leaves it that way indefinitely.
  seatLedgerManagedTerm(term, worktree, agent);
  // Then the placement seat, off this handler: the tab above is what every
  // worker got before the seat existed and what it keeps unless a seat that
  // ACTS names another room (`reseatWorkerByAnswer`).
  void roomForWorker({ parent, term, worktree, seat }).then((room) => reseatWorkerByAnswer(term, parent, worktree, agent, room));
  void (async () => {
    try {
      await refreshWorktrees();
      paintWorktreeAgents();
    } catch (error) {
      showError(error);
    }
  })();
});

listen("worktree:removed", () => {
  void refreshWorktrees();
});

/* A worker finished and the window took its workspace back.
 *
 * This is the one deletion in the product that nobody asked for in the
 * moment it happens — the beat decides, on facts the person is not looking
 * at — so it is the one that most has to say so. The reason travels with it
 * because "회수했습니다" alone leaves somebody wondering whether their work
 * was in there; the sentence names the branch and what took it, which is the
 * answer to that question.
 *
 * Refused reclaims deliberately do NOT come this way. A checkout that was
 * KEPT is the outcome the person wanted anyway — the directory is still
 * there, with everything in it — and a window that toasted every dirty
 * worker tree it walked past would be teaching people to dismiss the layer
 * that carries the deletions. Those go to the window log with their reason,
 * beside the refusals the existing automatic cleanup already writes there. */
listen("worktree:reclaimed", (event) => {
  const { name, branch, base } = event.payload ?? {};
  // The list refresh is not repeated here: the backend emits
  // `worktree:removed` for the same reclaim, and that listener already owns
  // it. This one carries the sentence and nothing else.
  if (typeof name !== "string" || name === "") return;
  toast(
    t(
      "worktree.reclaimed",
      "워커가 끝나 워크스페이스 {{name}}을(를) 회수했습니다 — {{branch}}의 커밋은 모두 {{base}}에 있고 저장되지 않은 변경은 없었습니다. 브랜치는 그대로 남아 있습니다.",
      { name, branch: branch ?? "", base: base ?? "" },
    ),
    "done",
  );
});

/* An agent started another agent, and the stage divides for it.
 *
 * THE reported break, and the one road into this window that did not exist:
 * every other way a terminal is born starts with a gesture — ⌘T, a menu, a
 * schedule — and each of them already ends at `mountTermTab`. A coordinator
 * delegating to teammates starts with a PROCESS, so the backend cuts the pane
 * and says so here (`term:split`, main.rs). Orca does the same thing from the
 * same direction: its fake tmux turns `split-window` into a real split of the
 * caller's leaf (`splitPtyBackedTerminal` → `splitFromLeafId`,
 * out/main/index.js:187951).
 *
 * The pane is put beside the shell the AGENT named, not beside the one
 * somebody is standing in — that is the whole reason `tileTermPane` takes a
 * source. It is also why nothing here consults `tilePlacement`: that rule
 * answers "should this become a pane or a tab", and this road has already
 * been told. A team's layout is the team's, and its cap is however many
 * teammates the leader asked for.
 *
 * A leader whose tab is not on the stage still gets its panes; what it does
 * not get is the keyboard, because the person is somewhere else and a caret
 * that jumped to a checkout they are not looking at is a keystroke lost. */
// Beyond three visible terminals, retain the processes in the worker list.
// Selecting a worker uses one reusable full-size viewer; no terminal restarts.
const TEAM_VISIBLE_PANE_LIMIT = 3;
const workerListTeams = new Set();

function parkTeamWorkers(host, parent, arriving) {
  const members = paneLeaves(host.layout).filter((term) => paneParents.get(term) === parent);
  if (!workerListTeams.has(parent) && paneLeaves(host.layout).length + 1 <= TEAM_VISIBLE_PANE_LIMIT) return false;
  workerListTeams.add(parent);
  const parked = new Set([...members, arriving]);
  const remaining = paneLeaves(host.layout).filter((term) => !parked.has(term));
  host.layout = prunePanes(host.layout, new Set(remaining));
  if (!remaining.includes(host.activePane)) host.activePane = parent;
  for (const term of parked) {
    detachedAgents.set(term, host.worktree);
    dropTermScreen(term);
  }
  renderPanes(host);
  updateStage();
  paintRunningCount();
  scheduleAgentPaint(["cards", "board"]);
  persistPaneLayouts(host.worktree);
  return true;
}

function revealListedWorker(term, worktree, agent) {
  const parent = paneParents.get(term);
  if (!workerListTeams.has(parent)) return false;
  const viewer = tabs.find((tab) => tab.kind === "term" && tab.teamParent === parent);
  detachedAgents.delete(term);
  if (viewer) {
    for (const previous of paneLeaves(viewer.layout)) {
      if (previous !== term) { detachedAgents.set(previous, worktree); dropTermScreen(previous); }
    }
    viewer.term = term;
    viewer.activePane = term;
    viewer.layout = paneLeaf(term);
    viewer.agent = agent ? agentName(agent) : viewer.agent;
    renderPanes(viewer);
    setActiveTab(viewer.id);
  } else {
    mountTermTab(term, { worktree, teamParent: parent, ...(agent ? { agent: agentName(agent) } : {}) }, { placement: "tab" });
  }
  scheduleAgentPaint(["cards", "board"]);
  return true;
}

listen("term:split", (event) => {
  const { parent, term, direction, agent, helper } = event.payload ?? {};
  if (typeof parent !== "number" || typeof term !== "number") return;
  // Who opened whom, recorded where it is already being told. The board learns
  // lineage from the backend (`pane_agents` carries it), but the sidebar's
  // rows must not cost a round trip — and they do not have to, because this
  // event is the moment the edge is created and it already names both ends.
  paneParents.set(term, parent);
  // 그리고 이 판이 어느 헬퍼인가 — split이 제 id를 실었을 때만(t-3024).
  if (typeof helper === "string" && helper.length > 0) paneHelpers.set(term, helper);
  const host = tabs.find(
    (tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(parent),
  );
  // The leader's tab closed between the ask and the answer. The shell is
  // already running and the backend owns it; there is simply no tab to draw
  // it in, and inventing one would put a teammate on the stage without the
  // coordinator it belongs to.
  if (!host) return;
  if (parkTeamWorkers(host, parent, term)) return;
  // The new pane is the tab's active one — Orca's own outcome, where the
  // reveal focuses the leaf it just made (`focusTerminalInitiatedTab`,
  // App-BaqTRjaA.js:4128) even though the TAB is not activated. So: the pane
  // takes the caret, the tab does not take the stage, and the workspace does
  // not change.
  // The bar starts as the agent's NAME when an agent was asked for, and
  // nothing otherwise — the measured rule (a split carries no title of its
  // own; the tab's name shows through, and the shell's OSC title overwrites
  // either). "cd %2" — the program plus tmux's pane id — was the reported bug.
  host.activePane = term;
  const onStage = host.id === activeTabId && host.worktree === activeWorktreePath;
  tileTermPane(
    host,
    term,
    agent ? { agent: agentSaidName(agent) } : {},
    helper || direction === "vertical" ? "right" : "down",
    onStage,
    parent,
  );
  schedulePaneAlignment(host);
});

/* The leader asked for one of its panes by name. Same rule as above: the
 * caret moves inside the tab, and the stage is not rearranged around it. */
listen("term:focus-pane", (event) => {
  const term = event.payload;
  if (typeof term !== "number") return;
  const host = tabs.find(
    (tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(term),
  );
  if (!host || host.activePane === term) return;
  host.activePane = term;
  renderPanes(host);
  updateStage();
  if (host.id === activeTabId && host.worktree === activeWorktreePath) keySink.focus();
});

/* A write this window did not make, seen while the person is looking.
 *
 * The backend watches the open file tabs (`watch_files`) and says only that
 * something moved. What to DO about it is decided here, per tab, by the same
 * function that answers when a tab is brought forward — so the push and the
 * pull cannot disagree: clean tabs are re-read in place, tabs with typing in
 * them are marked instead (Orca's split, index-ftls8Hg_.js:147351-147462).
 *
 * `checkFileMoved` compares content stamps, and that comparison is also what
 * swallows the echo of this window's OWN saves: the version a save hands back
 * describes the bytes it wrote, so the watcher seeing that write finds
 * nothing newer. Orca verifies self-writes by re-reading and comparing
 * content (:147404) — the stamp is that comparison, already paid for. */
listen("fs:changed", (event) => {
  for (const change of event.payload?.events ?? []) {
    const held = tabs.find((tab) => tab.kind === "file" && tab.path === change.path);
    if (!held) continue;
    if (change.kind === "delete") {
      markFileGone(held);
      continue;
    }
    // Back, or rewritten — either way the gone mark comes off first: the
    // banner says the file is missing, and it is not.
    if (held.gone) {
      held.gone = false;
      renderTabs();
      if (stillShowing(held)) paintSaveNote(held);
    }
    checkFileMoved(held);
  }
});

/* The structured channel is the ONLY path a permission prompt takes, so a
 * lane whose subscription ended can no longer say that it is waiting. Gating
 * it puts the notch on its row instead of leaving it looking idle
 * — a lane that cannot report being blocked must not look calm (제품 원칙 1).
 * A session with no lane on screen is one the person already closed. */
listen("session:ended", (event) => {
  const { session, reason } = event.payload;
  // Its questions go first, and for every session — including one whose lane
  // this window never drew. A prompt id retired with its channel cannot be
  // answered by anybody, and leaving it on screen blocks the agents behind it.
  withdrawPermissions(session);
  for (const term of paneTermsBySession(session)) rememberPaneAutonomy(term, null, session);
  clearPaneZoSubagents(session);
  clearLaneSubagents(session);
  const entry = laneBySession(session);
  if (!entry) return;
  invoke("gate_lane", { id: entry.lane.id, gate: "blocked" }).catch(() => {});
  if (reason) console.error(t("session.channelLost", "세션 {{session}}의 구조화 채널이 끊겼습니다: {{reason}}", { session, reason }));
});

/* A zo helper's row says only facts the frame owns: its best available name,
 * the shared running mark, and elapsed time from the reported epoch. The
 * latter deliberately calls the board clock's two existing rungs. There is
 * no second minute/hour/day judgement here for the two clocks to drift on. */
function laneSubagentName(agent) {
  if (agent.label) return agent.label;
  if (agent.model) return agent.model;
  const parts = agent.id.split("-").filter(Boolean);
  return parts[parts.length - 1] ?? agent.id;
}

function laneSubagentClock(agent, now = Date.now()) {
  if (!(agent.startedAt > 0) || agent.startedAt > now) return "";
  return runningWord(agent.startedAt, now) ?? agoWord(agent.startedAt, now);
}

function normalizeLaneSubagents(running) {
  const unique = new Map();
  for (const raw of running) {
    if (typeof raw !== "object" || raw === null) continue;
    const id = typeof raw.id === "string" ? raw.id.trim() : "";
    if (!id) continue;
    const clean = (value) => typeof value === "string" && value.trim() ? value.trim() : null;
    const epoch = Number(raw.started_epoch);
    unique.set(id, {
      id,
      label: clean(raw.label),
      model: clean(raw.model),
      startedAt: Number.isFinite(epoch) && epoch > 0 ? epoch * 1000 : null,
    });
  }
  return [...unique.values()];
}

function paneTermsBySession(session) {
  const terms = [];
  for (const [term, held] of paneSessions) {
    if (!held?.carriedOnly && held?.session?.id === session) terms.push(term);
  }
  return terms;
}

function clearPaneZoSubagents(session) {
  const terms = paneTermsBySession(session);
  if (terms.length === 0) return;
  for (const term of terms) paneSubagents.delete(term);
  scheduleAgentPaint(["cards", "board"]);
}

/* 이 세션이 서 있는 레인으로 — 헬퍼 행과 그 행의 손이 함께 쓰는 한 자리.
 * 두 곳에 적으면 한쪽만 고쳐지는 날이 온다. */
function focusLaneOfSession(session) {
  const parent = laneBySession(session);
  if (parent) void focusLane(parent.lane.id);
}

function makeLaneSubagentRow(session, agent) {
  const row = document.createElement("div");
  row.className = "wt-agent lane-subagent is-working";
  row.dataset.subagentId = agent.id;
  // The window's delegated tooltip, never the platform's native title. The
  // key stays on the row so `applyLocale` can repaint it in all four catalogs.
  row.dataset.i18nTitle = "session.subagentOpenParent";
  row.dataset.tip = t("session.subagentOpenParent", "부모 에이전트 열기");
  // The child owns its door. Relying on the enclosing lane row's bubbling
  // click left this row unreachable from the keyboard and made its behaviour
  // depend on where the DOM happened to be nested.
  actsAsButton(row, (event) => {
    event.stopPropagation();
    focusLaneOfSession(session);
  });
  const dot = document.createElement("span");
  dot.className = "wt-dot is-working";
  dot.setAttribute("aria-hidden", "true");
  const body = document.createElement("span");
  body.className = "wt-agent-body";
  const name = document.createElement("span");
  name.className = "wt-agent-name";
  body.appendChild(name);
  const when = document.createElement("span");
  when.className = "wt-agent-when";
  // 판 위의 헬퍼 행과 같은 손, 같은 문(`makeHelperPeek`). 이 행에는 제 term이
  // 없다 — zo는 한 세션을 한 TUI에서 돌리므로, 전사를 읽어 줄 백엔드에게 댈
  // 이름은 그 세션을 안고 있는 판의 term이다. 그런 판이 아직 없으면 이 창이
  // 아는 유일한 자리는 부모 레인이고, 손은 행과 같은 곳으로 간다.
  const peek = makeHelperPeek(() => {
    const term = paneTermsBySession(session)[0];
    if (term === undefined) {
      focusLaneOfSession(session);
      return;
    }
    // 낱말은 지금의 스냅샷에서 — 행은 id로 살아남고 label·model은 갱신된다
    // (아래 시계가 같은 이유로 같은 길을 간다).
    const current = laneSubagents.get(session)?.running
      .find((held) => held.id === row.dataset.subagentId) ?? agent;
    const owner = tabOfTerm(term);
    void openHelperPage(
      { term, agent: "zo", worktree: owner?.worktree ?? null, tab: owner },
      { id: current.id, name: laneSubagentName(current) },
    );
  });
  row.append(dot, body, when, peek);
  // A language change asks this producer again. It resolves through the
  // current full snapshot, so a keyed node kept across updates never keeps an
  // old start time in its closure.
  say(when, () => {
    const current = laneSubagents.get(session)?.running
      .find((held) => held.id === row.dataset.subagentId);
    return current ? laneSubagentClock(current) : "";
  });
  return row;
}

function forgetLaneSubagentRow(row) {
  const when = row.querySelector(".wt-agent-when");
  if (when) say(when, null);
  row.remove();
}

function paintLaneSubagents(session) {
  if (!session) return;
  const held = laneSubagents.get(session);
  if (!held) return;
  const entry = laneBySession(session);
  if (!entry) return;
  let host = entry.row.querySelector(":scope > .lane-subagents");
  if (!host) {
    host = document.createElement("div");
    host.className = "lane-subagents";
    entry.row.appendChild(host);
  }
  const wanted = [];
  const live = new Set(held.running.map((agent) => agent.id));
  for (const [id, row] of held.rows) {
    if (live.has(id)) continue;
    forgetLaneSubagentRow(row);
    held.rows.delete(id);
  }
  for (const agent of held.running) {
    let row = held.rows.get(agent.id);
    if (!row) {
      row = makeLaneSubagentRow(session, agent);
      held.rows.set(agent.id, row);
    }
    writeTextContent(row.querySelector(".wt-agent-name"), laneSubagentName(agent));
    writeTextContent(row.querySelector(".wt-agent-when"), laneSubagentClock(agent));
    writeAttribute(
      row,
      "data-tip",
      t("session.subagentOpenParent", "부모 에이전트 열기"),
    );
    wanted.push(row);
  }
  reconcileElementOrder(host, wanted);
  wakeWorktreeSpin();
}

function paintLaneSubagentClocks() {
  const now = Date.now();
  for (const held of laneSubagents.values()) {
    for (const agent of held.running) {
      const when = held.rows.get(agent.id)?.querySelector(".wt-agent-when");
      if (when) writeTextContent(when, laneSubagentClock(agent, now));
    }
  }
}

function clearLaneSubagents(session) {
  const held = laneSubagents.get(session);
  if (held) {
    for (const row of held.rows.values()) forgetLaneSubagentRow(row);
  }
  laneSubagents.delete(session);
  laneBySession(session)?.row.querySelector(":scope > .lane-subagents")?.remove();
  agentClock.sync();
}

function replaceLaneSubagents(session, running) {
  if (!Array.isArray(running)) return;
  const normalized = normalizeLaneSubagents(running);
  if (normalized.length === 0) {
    clearLaneSubagents(session);
    return;
  }
  const held = laneSubagents.get(session) ?? { running: [], rows: new Map() };
  held.running = normalized;
  laneSubagents.set(session, held);
  paintLaneSubagents(session);
  agentClock.sync();
}

/* A lane can arrive after its history snapshot. Keep the normal renderer as
 * the one row factory, then hang any already-known helper rows from that exact
 * persistent node. If a registry entry changes sessions, its old complete
 * snapshot is no longer owned by anything visible and leaves first. */
function upsertLaneWithSubagents(lane) {
  const previous = lanes.get(lane.id)?.lane.session_id;
  if (previous && previous !== lane.session_id) clearLaneSubagents(previous);
  upsertLane(lane);
  paintLaneSubagents(lane.session_id);
}

/* The structured channel, as signals: permission prompts become the modal,
 * subagent snapshots become lane children, and usage/rate-limit keep the
 * status bar honest. One function for both roads — the live stream and the
 * hydration below — so a frame means the same thing however late it arrives. */
function laneSignal(session, frame) {
  if (typeof frame !== "object" || frame === null) return;
  if (frame.type === "permission_prompt") {
    raisePermission(session, frame);
  } else if (frame.type === "prompt_resolved") {
    withdrawPrompt(session, frame.prompt_id);
  } else if (frame.type === "subagents") {
    // A terminal-owned session was already projected by the backend through
    // `hook:subagent`, the one pane source both navigator and graph read. Keep
    // this session-scoped cache for lanes only; copying an adopted pane here
    // would create a second source that can resurrect children after detach.
    if (paneTermsBySession(session).length === 0) {
      replaceLaneSubagents(session, frame.running);
    }
  } else if (frame.type === "text_delta" || frame.type === "reasoning") {
    // The channel's own stream of what the agent is saying: the pane's
    // conversation view draws it as it arrives (`paneLive`).
    notePaneLive(session, frame);
  } else if (frame.type === "turn" && frame.phase === "start") {
    // A new turn: what the last one streamed has landed in the transcript
    // or never will; the streaming rows start clean.
    for (const term of paneTermsBySession(session)) {
      paneLive.delete(term);
      paintPaneLive(term);
    }
  } else if (frame.type === "usage") {
    /* 상태바와 보드는 같은 손으로 읽는다 (t-2374). 두 번째 파서를 지으면
     * 「상태바는 24,489인데 카드는 비었다」가 언제든 생긴다 — 그리고 그것은
     * 프레임이 안 왔는지 파서가 다른지 화면에서 구별되지 않는다. */
    const reading = rememberSessionUsage(session, frame);
    writeTextContent(el("sb-usage"), usageCtxWord(reading));
    // 카드의 미터가 이 값을 그린다. 합류기가 150ms로 모으므로 턴마다 오는
    // 이 프레임이 다시 그리기를 몰아치게 하지 않는다.
    scheduleAgentPaint(["board"]);
  } else if (frame.type === "rate_limit") {
    // Live ingest, Orca's way: a running session knows the 5h window sooner
    // than the next scan does, so the segment takes its number now. Display
    // only — the backend snapshot stays the scan's.
    const five = frame.five_hour_utilization;
    if (typeof five === "number" && claudeUsage?.session) {
      claudeUsage.session.used_percent = Math.min(100, Math.max(0, Math.round(five * 100)));
      paintUsageSegments();
      paintUsagePanel();
    }
  }
}

listen("session:frame", (event) => {
  laneSignal(event.payload.session, event.payload.frame);
});

/* One delta of a pane's live text: appended to the piece it continues (same
 * voice, same block, still open) or begun as a new piece — the shape a wire's
 * live list has (`WireLive`: role, text, done), so the page draws both alike.
 * The pane on screen paints it by the next frame (`paintLiveAnswer`); a pane
 * whose conversation is not up keeps the words for when it is. */
function notePaneLive(session, frame) {
  const role = frame.type === "reasoning" ? "thinking" : "assistant";
  for (const term of paneTermsBySession(session)) {
    const pieces = paneLive.get(term) ?? [];
    const last = pieces.at(-1);
    if (last && last.role === role && last.id === frame.id && !last.done) {
      last.text += frame.text ?? "";
      last.done = frame.done === true;
    } else {
      pieces.push({ role, id: frame.id, text: frame.text ?? "", done: frame.done === true });
    }
    paneLive.set(term, pieces);
    paintPaneLive(term);
  }
}

/* The streaming rows of a pane's conversation, when it is on screen, follow
 * its live pieces now — and a reader at the tail stays at the tail. */
function paintPaneLive(term) {
  const held = paneChats.get(term);
  if (!held?.on || !held.host || held.host.hidden) return;
  const list = held.host.querySelector(".helper-turns");
  if (!list) return;
  const follow = helperFollowsTail(list);
  syncStreamingTurns(list, held.tab.worker);
  if (follow) scrollHelperToBottom(list);
}

/* The live pieces a pane's page draws — none for a run with no pane. */
function paneLiveOf(run) {
  return run.term === undefined || run.term === null ? [] : paneLive.get(run.term) ?? [];
}

/* A turn of `role` arrived from the transcript: the finished pieces of that
 * voice were those words, and leave in the same paint. A person's turn closes
 * every finished piece — whatever streamed before it has landed, or never
 * will. */
function settlePaneLive(run, role) {
  const pieces = paneLiveOf(run);
  if (!pieces.length) return;
  const kept = pieces.filter((piece) => !piece.done || (role !== "user" && piece.role !== role));
  if (kept.length) paneLive.set(run.term, kept);
  else paneLive.delete(run.term);
}

/* The subscribe snapshot — what the session said before this window attached.
 * Walked through the same switch, with one guard: a permission prompt counts
 * only as the history's LAST word. One followed by more frames was answered
 * before we arrived, and re-raising it would put a modal in front of somebody
 * whose `prompt_id` the server has already retired. */
listen("session:history", (event) => {
  const { session, history } = event.payload;
  if (!Array.isArray(history)) return;
  history.forEach((frame, at) => {
    if (frame?.type === "permission_prompt" && at !== history.length - 1) return;
    laneSignal(session, frame);
  });
});

/* The agent panes a person deliberately put away in this workspace.
 *
 * This is the same registry `worktreeAgentRows` reads. A second source here
 * would let the sidebar say an agent exists while the empty stage denies it,
 * which is the defect this sentence exists to close. */
function detachedWorktreeAgents(path) {
  const rows = [];
  for (const [term, worktree] of detachedAgents) {
    if (worktree !== path) continue;
    const agent = paneSessions.get(term)?.agent ?? paneAgents.get(term) ?? null;
    rows.push({ term, agent });
  }
  return rows;
}

function paintDetachedStagePlaceholder(rows) {
  delete placeholder.dataset.i18nHtml;
  placeholder.replaceChildren();
  const message = document.createElement("span");
  message.className = "stage-agent-message";
  message.textContent = t(
    "stage.detachedAgents",
    "이 워크트리에 에이전트 {{count}}개가 탭 없이 실행 중입니다.",
    { count: rows.length },
  );
  const doors = document.createElement("span");
  doors.className = "stage-agent-doors";
  const worktree = activeWorktreePath;
  for (const row of rows) {
    const name = row.agent ? agentName(row.agent) : t("board.agent", "에이전트");
    const door = document.createElement("button");
    door.type = "button";
    door.className = "stage-door";
    door.dataset.stageAgent = String(row.term);
    door.textContent = t("stage.openAgent", "{{agent}} 열기", { agent: name });
    door.addEventListener("click", () => {
      void focusAgentPane(worktree, null, row.term, row.agent);
    });
    doors.appendChild(door);
  }
  placeholder.append(message, doors);
}

/* What the empty stage says, which is one of three things.
 *
 * The markup's own message invites you to open a terminal; when `zo` is not
 * installed that invitation is beside the point, and this element says so
 * instead. Only the first is `applyLocale`'s to write, so the key comes OFF the
 * element while the second is showing — otherwise the next language change
 * quietly replaces a true statement about this machine with an invitation. */
function paintStagePlaceholder() {
  // 프로젝트가 하나도 없으면 이 화면이 할 말은 터미널이 아니다. Orca의
  // Landing이 리포 개수로 갈리는 그 자리이고(0개면 "프로젝트를 추가하면
  // 시작합니다", 1개 이상이면 사이드바에서 고르라는 안내), 여기서는 그
  // 갈림을 이 창이 이미 가진 빈-무대 자리에 얹었다 — 빈 화면을 하나 더 만들면
  // 둘 중 어느 것이 뜰지 아무도 모르는 상태가 생긴다. */
  if (projectsRead && !projects.length) {
    paintStageLanding();
    return;
  }
  if (stageError !== null) {
    // As text, and with the key off: this is what a server sent back, it is
    // not one of this window's sentences, and nothing translates it. No door
    // under it either — a window reporting that the last attempt failed is not
    // a window that should be offering the same attempt as its main gesture.
    delete placeholder.dataset.i18nHtml;
    placeholder.textContent = stageError;
    return;
  }
  const detached = detachedWorktreeAgents(activeWorktreePath);
  if (detached.length > 0) {
    paintDetachedStagePlaceholder(detached);
    return;
  }
  if (zoAvailable) {
    // Re-keyed AND redrawn. `applyLocale` has already swept the document by
    // the time this runs, so putting the attribute back on its own would
    // leave an element that is still holding the other message holding it —
    // which is exactly what a window whose language changed before
    // `boot_report` landed would have shown, permanently.
    const hint = optionalShortcutLabel("terminal.newTab");
    if (hint === null) {
      // Nothing opens a terminal from the keyboard right now, so the sentence
      // that tells you which key to press is the wrong sentence — the tab
      // strip's `＋` is still there and is what this should be pointing at.
      // The key comes OFF the element for the same reason it does below.
      delete placeholder.dataset.i18nHtml;
      placeholder.innerHTML = t("stage.emptyNoChord", "터미널이 열려 있지 않습니다. 탭 줄의 <kbd>＋</kbd>로 하나 엽니다 — 당신의 셸이고, 안에서 <code>zo</code>나 <code>claude</code>를 실행할 수 있습니다.");
      seatStageDoor();
      return;
    }
    placeholder.dataset.i18nHtml = "stage.empty";
    applyLocale(placeholder);
    // The sentence invites you to press a key, so it has to name the key that
    // is in force. `applyLocale` writes the catalog text with its `{{chord}}`
    // still in it — the source is this file's own catalog either way, which is
    // the same reason assigning `innerHTML` here is not a way in.
    placeholder.innerHTML = placeholder.innerHTML.replace("{{chord}}", hint);
    seatStageDoor();
    return;
  }
  delete placeholder.dataset.i18nHtml;
  placeholder.innerHTML = t("stage.noZo", "<code>zo</code>를 PATH에서 찾지 못했습니다 — 레인 없이 계속 쓸 수 있고, 설치하면 여기에서 바로 세션이 열립니다.");
  seatStageDoor();
}

/* 프로젝트가 하나도 없을 때의 첫 화면 (Orca의 `Landing`, 1-ea).
 *
 * 워드마크와 한 줄, 그리고 문 둘. 오는 순서가 요점이다: 프로젝트가 없으면
 * 워크트리도 만들 수 없으므로 그 버튼은 **회색으로 남되 사라지지 않고**, 왜
 * 눌리지 않는지 제목으로 말한다 — Orca가 이 화면에서 하는 가장 좋은 일이고,
 * 없는 버튼은 아무것도 가르치지 않는다.
 *
 * `innerHTML`로 통째 짓지 않는다: 버튼에 핸들러가 붙어야 하고, 문단을
 * 덮어쓰는 다른 분기가 이 노드를 나중에 갈아 끼우기 때문이다. */
function paintStageLanding() {
  delete placeholder.dataset.i18nHtml;
  placeholder.replaceChildren();
  const mark = document.createElement("span");
  mark.className = "landing-mark";
  mark.setAttribute("aria-hidden", "true");
  mark.textContent = "Z";
  const wordmark = document.createElement("span");
  wordmark.className = "landing-wordmark";
  wordmark.textContent = "ZeroCode";
  const lede = document.createElement("span");
  lede.className = "landing-lede";
  say(lede, () => t("landing.lede", "프로젝트를 추가하면 시작합니다."));
  const doors = document.createElement("span");
  doors.className = "landing-doors";
  const add = document.createElement("button");
  add.type = "button";
  add.className = "btn btn--primary";
  say(add, () => t("landing.add", "프로젝트 추가"));
  // 사이드바의 ＋와 같은 문. 프로젝트를 여는 길이 둘이 되면 하나는 반드시
  // 목록 갱신이나 문서 정리를 잊는다.
  add.addEventListener("click", () => void openAnotherProject());
  const create = document.createElement("button");
  create.type = "button";
  create.className = "btn";
  create.disabled = true;
  create.dataset.tip = t("landing.createBlocked", "프로젝트를 먼저 추가하세요");
  say(create, () => t("landing.create", "워크트리 만들기"));
  doors.append(add, create);
  placeholder.append(mark, wordmark, lede, doors);
}

/* The empty stage's own door.
 *
 * Orca never shows this screen for long: activating a worktree with no
 * renderable tabs CREATES a terminal (`ensureWorktreeHasInitialTerminal` →
 * `shouldAutoCreateInitialTerminal(count) = count === 0`,
 * worktree-activation-3qRw45tK.js:5,1656). This window does the same on boot, on
 * a project switch and on a worktree activation — so the only way to arrive
 * here is to close the last tab yourself, and then the screen has to hold a way
 * back.
 *
 * It was a sentence and nothing else, and the sentence named a chord. That was
 * reported as exactly what it is: "이게 뜨고.. 여기서 사용자는 어떻게 해야하는지
 * 알 수 없음". A person who does not already know `⌘T` cannot act on a paragraph.
 *
 * Rebuilt on every paint rather than kept, because the paragraph around it is
 * assigned as `innerHTML` and that takes any child with it — a button held in a
 * variable would be a detached node nobody could press from the second paint
 * onward. */
function seatStageDoor() {
  const door = document.createElement("button");
  door.type = "button";
  door.className = "stage-door";
  door.textContent = t("stage.openTerminal", "터미널 열기");
  // The paragraph is `pointer-events: none` so it cannot swallow clicks meant
  // for the terminal underneath; the door has to hand them back to itself.
  door.addEventListener("click", () => {
    // Into whichever leaf is focused, exactly like the strip's `＋` — there is
    // only one group standing when this screen is up.
    void openTermTab({ door: "terminal" });
  });
  placeholder.append(door);
}

/* ---- 첫 실행 마법사 (Orca의 OnboardingFlow) ----------------------------------
 *
 * 뜨는 조건은 하나다: 저장된 `closed_at`이 비어 있는 것. Orca도 이 한 줄이
 * 전부이고(`shouldShowOnboarding`), 그 값을 만들어 내는 판정 — 새 기계인가,
 * 이 기능 이전부터 쓰던 사람인가 — 은 Rust가 실행 시작에서 한 번만 한다
 * (docs/reverse/orca-ui-inventory.md 1-ea). 창은 물어보지 않는다.
 *
 * 단계 본문은 저마다 이 창이 이미 가진 함수를 부른다 — 에이전트 카드는
 * `agentIcon`/`chooseDefaultAgent`, 테마 카드는 `setTheme`. 마법사가 자기
 * 사본을 들면 설정 화면과 반드시 갈라지고, 갈라진 쪽은 아무도 고치지 않는다. */

/* 선언된 단계. 실제로 보이는 것은 이 중 일부다 — 연동 단계는 GitHub CLI가 이미
 * 연결돼 있으면 빠진다(Orca의 `shouldSkipIntegrationsStep`). 진행 표시는 빠진
 * 단계를 빼고 세므로 "3 중 1"처럼 읽힌다. */
const ONBOARDING_STEPS = [
  { id: "agent", step: 1 },
  { id: "theme", step: 2 },
  { id: "integrations", step: 3 },
  { id: "notifications", step: 4 },
];

/* 저장된 첫 실행 상태, 백엔드가 준 그대로. `null`은 "아직 안 읽었다"이지
 * "안 닫혔다"가 아니다 — 그 둘을 한 값으로 쓰면 부팅 실패가 마법사로 보인다. */
let onboarding = null;
let onboardIndex = 0;
let onboardBusy = false;
/* 테마 단계에 들어설 때의 테마. "프로젝트 설정으로 건너뛰기"는 이것을
 * 되돌린다 — Orca의 `themeStepEntryThemeRef`와 같은 이유로, 고르는 중이던
 * 화면을 그 사람의 선택으로 굳혀 두고 나가지 않는다. */
let themeAtEntry = null;
/* 알림 시험의 마지막 결과. `null`이면 아직 안 눌렀다. */
let notifyProbe = null;

/* 마법사를 띄울까. 판정의 전부. */
function shouldShowOnboarding(state) {
  return state !== null && state !== undefined && state.closed_at === null;
}

function paintSettingsOnboarding() {
  const status = el("settings-onboarding-status");
  const detail = el("settings-onboarding-detail");
  const closed = onboarding?.closed_at !== null && onboarding?.closed_at !== undefined;
  if (onboarding === null) {
    status.dataset.state = "unchecked";
    status.textContent = t("settings.onboarding.unchecked", "확인 전");
    detail.textContent = t("settings.onboarding.uncheckedHint", "첫 실행 상태를 아직 읽지 못했습니다.");
  } else if (!closed) {
    status.dataset.state = "checking";
    status.textContent = t("settings.onboarding.inProgress", "진행 중");
    detail.textContent = t("settings.onboarding.inProgressHint", "마지막으로 마친 단계부터 이어서 시작합니다.");
  } else {
    status.dataset.state = "connected";
    status.textContent = onboarding.outcome === "skipped"
      ? t("settings.onboarding.skipped", "건너뜀")
      : t("settings.onboarding.completed", "완료");
    detail.textContent = t("settings.onboarding.completedHint", "필요하면 처음부터 다시 볼 수 있습니다.");
  }

  const steps = guideReport?.steps ?? [];
  const done = steps.filter((step) => step.done).length;
  el("settings-guide-count").textContent = steps.length > 0 ? `${done}/${steps.length}` : "";
  el("settings-guide-detail").textContent = steps.length > 0
    ? t("settings.onboarding.checklistProgress", "{{done}}개 완료, {{total}}개 중", {
      done,
      total: steps.length,
    })
    : t("settings.onboarding.checklistLoading", "현재 프로젝트와 연동 상태에서 체크리스트를 다시 계산합니다.");
}

function onboardingStepSkipped(id) {
  // Orca: `shouldSkipIntegrationsStep(status) { return status?.gh.installed === true; }`
  return id === "integrations" && currentGithubStanding() === "connected";
}

function visibleOnboardingSteps() {
  return ONBOARDING_STEPS.filter((one) => !onboardingStepSkipped(one.id));
}

function onboardingStepName(id) {
  const said = {
    agent: () => t("onboard.stepAgent", "기본 에이전트"),
    theme: () => t("onboard.stepTheme", "모양"),
    integrations: () => t("onboard.stepIntegrations", "연동"),
    notifications: () => t("onboard.stepNotifications", "알림"),
  };
  return said[id] ? said[id]() : id;
}

function onboardingCopy(id) {
  const said = {
    agent: () => ({
      title: t("onboard.agentTitle", "기본 에이전트를 고르세요"),
      subtitle: t("onboard.agentSubtitle", "ZeroCode는 어떤 CLI 에이전트와도 함께 돕니다. 가장 자주 쓸 하나를 고르세요. 언제든 바꿀 수 있습니다."),
    }),
    theme: () => ({
      title: t("onboard.themeTitle", "눈에 맞는 모양으로"),
      subtitle: t("onboard.themeSubtitle", "몇 시간을 들여다볼 화면입니다."),
    }),
    integrations: () => ({
      title: t("onboard.integrationsTitle", "GitHub 작업 준비"),
      subtitle: t("onboard.integrationsSubtitle", "GitHub CLI를 설치하면:"),
    }),
    notifications: () => ({
      title: t("onboard.notifyTitle", "알림 켜기"),
      subtitle: t("onboard.notifySubtitle", "에이전트가 끝나거나 사람을 기다릴 때 알려 드립니다."),
    }),
  };
  return said[id] ? said[id]() : { title: id, subtitle: "" };
}

/* 마법사를 연다. 부팅에서 한 번, 도움말의 숨은 항목에서 한 번. */
function openOnboarding(state) {
  void refreshSkills();
  onboarding = state;
  notifyProbe = null;
  // 중도 이탈 후의 재개 지점. 저장된 진행도의 다음 단계이고, 건너뛴 단계는
  // 목록에 없으므로 인덱스는 보이는 것들 안에서만 움직인다.
  const done = Math.max(-1, Number(state?.last_completed_step ?? -1));
  const shown = visibleOnboardingSteps();
  const resume = shown.findIndex((one) => one.step > done);
  onboardIndex = resume === -1 ? 0 : resume;
  paintOnboarding();
  showModal(el("onb-scrim"));
  // 연동 단계가 필요한지는 이 답이 온 뒤에야 안다. 늦게 와도 화면은 다시
  // 그려지고, 이미 지나온 단계는 인덱스가 지킨다.
  void refreshGithubIntegration().then(() => {
    if (!el("onb-scrim").hidden) paintOnboarding();
  });
}

function paintOnboarding() {
  const shown = visibleOnboardingSteps();
  if (!shown.length) return;
  onboardIndex = Math.min(onboardIndex, shown.length - 1);
  const current = shown[onboardIndex];

  const progress = el("onb-progress");
  progress.replaceChildren();
  shown.forEach((one, at) => {
    const dot = document.createElement("button");
    dot.type = "button";
    dot.className = "onb-dot";
    dot.dataset.step = one.id;
    if (at === onboardIndex) dot.dataset.state = "here";
    else if (at < onboardIndex) dot.dataset.state = "done";
    dot.setAttribute("role", "tab");
    dot.setAttribute("aria-selected", at === onboardIndex ? "true" : "false");
    dot.setAttribute(
      "aria-label",
      t("onboard.goToStep", "{{step}}단계로 이동: {{name}}", {
        step: at + 1,
        name: onboardingStepName(one.id),
      }),
    );
    // 뒤로만 눌린다. 앞의 단계는 아직 지나지 않았고, 뛰어넘어 들어가면 그
    // 단계가 저장할 진행도가 지나온 것보다 앞서게 된다.
    dot.disabled = at > onboardIndex;
    dot.addEventListener("click", () => {
      onboardIndex = at;
      paintOnboarding();
    });
    progress.appendChild(dot);
  });

  say(el("onb-count"), () =>
    t("onboard.counter", "{{step}} / {{total}}", {
      step: onboardIndex + 1,
      total: shown.length,
    }),
  );
  // Orca는 첫 화면에서만 제목 위에 환영 한 줄을 얹는다.
  el("onb-eyebrow").hidden = onboardIndex !== 0;
  // `say`로 쓴다: 언어가 바뀌면 `applyLocale`이 키를 가진 요소를 자기 문장으로
  // 덮어쓰는데, 이 둘은 단계마다 달라지는 문장이라 키로 표현할 수 없다.
  say(el("onb-title"), () => onboardingCopy(current.id).title);
  say(el("onb-subtitle"), () => onboardingCopy(current.id).subtitle);

  el("onb-back").hidden = onboardIndex === 0;
  el("onb-skip").hidden = onboardIndex === shown.length - 1;
  const last = onboardIndex === shown.length - 1;
  say(el("onb-next"), () =>
    last ? t("onboard.finish", "첫 프로젝트 추가") : t("onboard.continue", "계속"),
  );

  paintOnboardingStep(current.id);
}

function paintOnboardingStep(id) {
  const host = el("onb-body");
  host.replaceChildren();
  host.dataset.step = id;
  if (id === "agent") paintOnboardingAgents(host);
  else if (id === "theme") paintOnboardingThemes(host);
  else if (id === "integrations") paintOnboardingIntegrations(host);
  else paintOnboardingNotifications(host);
}

/* 에이전트 카드. 이 기계에서 찾은 것이 먼저이고, 하나도 없으면 카탈로그의
 * 앞자락을 "많이 쓰는" 목록으로 보여 준다(Orca와 같다 — 고를 것이 없는 화면을
 * 보여 주느니 나중에 설치할 것을 고르게 한다). */
function paintOnboardingAgents(host) {
  const installed = installedAgents();
  const rows = installed.length ? installed : agentRows.slice(0, 6);
  const head = document.createElement("p");
  head.className = "onb-section";
  say(head, () =>
    installed.length
      ? t("onboard.agentDetected", "이 기계에서 찾은 에이전트")
      : t("onboard.agentPopular", "많이 쓰는 에이전트"),
  );
  host.appendChild(head);
  if (!installed.length) {
    const warn = document.createElement("p");
    warn.className = "onb-warn";
    say(warn, () => t("onboard.agentNone", "PATH에서 에이전트를 하나도 찾지 못했습니다. 나중에 설치할 것을 고르거나, 빈 터미널로 계속하세요."));
    host.appendChild(warn);
  }
  const grid = document.createElement("div");
  grid.className = "onb-grid";
  for (const row of rows) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "onb-card";
    card.dataset.agent = row.id;
    card.setAttribute("aria-pressed", row.id === defaultAgentId() ? "true" : "false");
    const name = document.createElement("span");
    name.className = "onb-card-name";
    name.textContent = row.name;
    card.append(agentIcon(row), name);
    card.addEventListener("click", () => {
      // 설정 화면과 같은 문. 기본 에이전트가 저장되는 길이 둘이 되면 하나는
      // 반드시 알림·펠릿 갱신을 잊는다.
      void chooseDefaultAgent(agentPreference(row.id));
      paintOnboardingStep("agent");
    });
    grid.appendChild(card);
  }
  host.appendChild(grid);
}

function paintOnboardingThemes(host) {
  if (themeAtEntry === null) themeAtEntry = theme;
  const grid = document.createElement("div");
  grid.className = "onb-grid onb-grid--theme";
  for (const choice of THEMES) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "onb-card onb-card--theme";
    card.dataset.theme = choice.code;
    card.setAttribute("aria-pressed", choice.code === theme ? "true" : "false");
    const name = document.createElement("span");
    name.className = "onb-card-name";
    say(name, () => t(choice.key, choice.name));
    card.append(name);
    card.addEventListener("click", () => {
      // 즉시 적용된다. 테마는 읽어 보고 고르는 것이지 설명을 읽고 고르는
      // 것이 아니다.
      setTheme(choice.code);
      paintOnboardingStep("theme");
    });
    grid.appendChild(card);
  }
  host.appendChild(grid);
}

function paintOnboardingIntegrations(host) {
  const standing = currentGithubStanding();
  const list = document.createElement("ul");
  list.className = "onb-bullets";
  const bullets = [
    () => t("onboard.ghBullet1", "워크트리마다 풀 리퀘스트 상태와 CI 검사를 봅니다."),
    () => t("onboard.ghBullet2", "패널에서 바로 풀 리퀘스트를 만듭니다."),
    () => t("onboard.ghBullet3", "실패한 검사의 로그를 에이전트에게 그대로 넘깁니다."),
  ];
  for (const produce of bullets) {
    const item = document.createElement("li");
    say(item, produce);
    list.appendChild(item);
  }
  host.appendChild(list);

  const row = document.createElement("div");
  row.className = "onb-row";
  const status = document.createElement("span");
  status.className = "onb-badge";
  status.dataset.standing = standing ?? "checking";
  say(status, () => {
    if (standing === "connected") return t("onboard.ghConnected", "연결됨");
    if (standing === "signin_needed") return t("onboard.ghSignin", "로그인이 필요합니다");
    if (standing === "missing") return t("onboard.ghMissing", "CLI가 설치되지 않았습니다");
    return t("onboard.ghChecking", "확인 중…");
  });
  row.appendChild(status);

  if (standing === "missing") {
    const install = document.createElement("button");
    install.type = "button";
    install.className = "btn";
    say(install, () => t("onboard.ghInstall", "gh 설치 안내 열기"));
    install.addEventListener("click", () => openExternal("https://cli.github.com"));
    row.appendChild(install);
  }
  const recheck = document.createElement("button");
  recheck.type = "button";
  recheck.className = "btn";
  say(recheck, () => t("onboard.ghRecheck", "다시 확인"));
  recheck.addEventListener("click", () => {
    githubStatusLoaded = false;
    paintOnboardingStep("integrations");
    void refreshGithubIntegration(null, true).then(() => {
      // 연결되면 이 단계 자체가 목록에서 빠지므로 전체를 다시 그린다.
      paintOnboarding();
    });
  });
  row.appendChild(recheck);
  host.appendChild(row);
}

/* 알림 단계. 권한을 물어볼 API가 없으므로 **한 번 쏴 보고** 배달 여부로
 * 판정한다(Orca의 probe 패턴). 다만 Orca는 이 요청을 온보딩 다음 실행으로
 * 미루는데, 여기서는 이 단계 안에서 직접 부른다 — 사람이 지금 이 화면을 보고
 * 있을 때 묻는 것이 요점이다. */
function paintOnboardingNotifications(host) {
  const row = document.createElement("div");
  row.className = "onb-row";
  const send = document.createElement("button");
  send.type = "button";
  send.className = "btn btn--primary";
  say(send, () => t("onboard.notifyTest", "시험 알림 보내기"));
  send.addEventListener("click", () => {
    invoke("notification_probe", {
      title: "ZeroCode",
      body: t("onboard.notifyProbeBody", "알림이 이렇게 도착합니다."),
    })
      .then((delivered) => {
        notifyProbe = delivered === true;
        // 도착한 것만 적는다. 막힌 알림을 "켰다"로 세면 체크리스트가 이 기계에
        // 대해 거짓을 말하게 된다.
        if (notifyProbe) markFirstRun("notified");
        paintOnboardingStep("notifications");
      })
      .catch(() => {
        notifyProbe = false;
        paintOnboardingStep("notifications");
      });
  });
  row.appendChild(send);
  host.appendChild(row);

  if (notifyProbe === null) return;
  const said = document.createElement("p");
  said.className = notifyProbe ? "onb-section" : "onb-warn";
  say(said, () =>
    notifyProbe
      ? t("onboard.notifyDelivered", "알림이 도착했습니다.")
      : t("onboard.notifyBlocked", "시스템이 알림을 막고 있습니다. 시스템 설정 > 알림에서 허용하세요."),
  );
  host.appendChild(said);
}

/* 한 단계를 지날 때마다 적는다. 건너뛴 단계는 그 앞자리까지 한 번에 적어,
 * 재개가 이미 지나온 자리로 되돌아가지 않게 한다(Orca와 같다). */
function persistOnboardingStep(step, choseAgent = false) {
  return invoke("save_onboarding_step", { step, choseAgent })
    .then((next) => {
      onboarding = next;
    })
    .catch(() => {});
}

async function onboardingNext() {
  if (onboardBusy) return;
  const shown = visibleOnboardingSteps();
  const current = shown[onboardIndex];
  if (!current) return;
  if (onboardIndex === shown.length - 1) {
    await closeOnboardingWith("completed");
    return;
  }
  const next = shown[onboardIndex + 1];
  // 다음 단계의 바로 앞자리까지. 사이에 건너뛴 단계가 있으면 그 자리들도
  // 지나온 것으로 적힌다.
  await persistOnboardingStep(next.step - 1, current.id === "agent");
  onboardIndex += 1;
  paintOnboarding();
  el("onb-next").focus();
}

function onboardingBack() {
  if (onboardIndex === 0) return;
  onboardIndex -= 1;
  paintOnboarding();
}

/* 나가는 문 둘, 그리고 저장되는 것.
 *
 * `completed`는 끝까지 갔거나 프로젝트 설정으로 건너뛴 것이고, 두 경우 모두
 * 곧바로 프로젝트를 고르는 창으로 넘긴다(Orca의 `openModal("add-repo")`).
 * `dismissed`는 도중에 나간 것이라 진행도가 지워진다. 어느 쪽이든 다음 실행에
 * 다시 뜨지 않는다 — 그것이 `closed_at`의 뜻이다. */
async function closeOnboardingWith(outcome) {
  if (onboardBusy) return;
  onboardBusy = true;
  try {
    onboarding = await invoke("close_onboarding", { outcome });
  } catch (error) {
    showError(error);
  } finally {
    onboardBusy = false;
  }
  hideModal(el("onb-skip-scrim"));
  hideModal(el("onb-scrim"));
  themeAtEntry = null;
  if (outcome === "completed") await openAnotherProject();
}

/* "프로젝트 설정으로 건너뛰기". 완료로 적히되, 테마 단계에서 고르던 중이었다면
 * 들어설 때의 테마로 되돌린다 — 고르다 만 것은 고른 것이 아니다. */
async function skipToProjectSetup() {
  const shown = visibleOnboardingSteps();
  if (shown[onboardIndex]?.id === "theme" && themeAtEntry !== null && themeAtEntry !== theme) {
    setTheme(themeAtEntry);
  }
  await closeOnboardingWith("completed");
}

function requestOnboardingSkip() {
  if (!el("onb-skip-scrim").hidden) return;
  showModal(el("onb-skip-scrim"));
}

function closeOnboardingSkipConfirm(skip) {
  hideModal(el("onb-skip-scrim"));
  if (skip) void closeOnboardingWith("dismissed");
}

el("onb-next").addEventListener("click", () => void onboardingNext());
el("onb-back").addEventListener("click", onboardingBack);
el("onb-skip").addEventListener("click", () => void skipToProjectSetup());
el("onb-skip-yes").addEventListener("click", () => closeOnboardingSkipConfirm(true));
el("onb-skip-no").addEventListener("click", () => closeOnboardingSkipConfirm(false));
/* 바깥 클릭은 건너뛰기 요청이다 — 다이얼로그 안에서 시작한 클릭은 아니다.
 * `event.target === scrim`이 이 창의 다른 다이얼로그가 쓰는 바로 그 검사이고,
 * Orca가 인터랙티브 레이어 화이트리스트로 하는 일을 여기서는 이 한 줄이 한다:
 * 셀렉트도 팝오버도 이 마법사 안에 없다. */
el("onb-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("onb-scrim")) requestOnboardingSkip();
});

/* ---- 마법사 다음의 세 표면 — 체크리스트·코치마크·팁, 그리고 수동 기능 투어 ----
 *
 * 마법사는 한 번 닫히면 끝이다. Orca의 첫 실행 경험이 얇지 않은 이유는 그
 * 다음에 오는 것들에 있고(1-eb), 그 넷은 저장 키도 트리거도 해제 경로도
 * 서로 다르다:
 *
 *   시작 체크리스트 — 사이드바 한 줄과 모달. 완료는 **저장되지 않는다**.
 *   코치마크 투어   — 백드롭 없는 말풍선. 세션당 하나.
 *   일회성 팁       — 앱 오픈당 하나. 마법사가 떴던 실행은 통째로 봉인.
 *   기능 투어       — 자동 노출 없음. 도움말의 한 항목에서만.
 *
 * 규칙은 하나도 여기 있지 않다. 넷 다 코어의 순수 함수가 정하고 창은 그 답을
 * 그린다 — 창이 자기 판단을 하나라도 더하면 "왜 안 뜨지"에 아무도 답할 수 없게
 * 된다. */

/* 사람이 한 일을 적는 문 하나.
 *
 * 켜기만 하고, 이미 켜져 있으면 백엔드가 파일을 다시 쓰지 않는다. 클릭마다
 * 두들겨지는 문이라 실패는 삼킨다 — 체크 한 칸 때문에 diff가 안 열리는 것이 더
 * 나쁜 실패다. */
function markFirstRun(mark) {
  // 이미 켜져 있으면 묻지 않는다. 이 문은 기둥을 한 칸 넓히는 화살표 하나에도
  // 두들겨지므로, 백엔드가 "바뀐 것 없음"으로 돌려보내 주는 것만으로는 키
  // 반복 하나가 왕복 수십 번이 된다.
  if (onboarding?.checklist?.[mark] === true) return;
  invoke("mark_onboarding", { mark })
    .then((next) => {
      onboarding = next;
      void refreshSetupGuide();
    })
    .catch(() => {});
}

/* 본 것을 적는 문. 셋(투어·팁·기능 투어)이 같은 문을 쓴다. */
function markFirstRunSeen(kind, id) {
  return invoke("mark_first_run_seen", { kind, id })
    .then((next) => {
      onboarding = next;
    })
    .catch(() => {});
}

/* 지금 무언가가 화면을 덮고 있나.
 *
 * 코치마크도 팁도 덮인 화면 위에는 뜨지 않는다. 스크림 하나라도 서 있으면 그
 * 아래를 가리키는 말풍선은 가리킬 수 없는 것을 가리키는 것이고, 그 위에 뜨는
 * 팁은 사람이 답하던 질문을 덮는 것이다. 설정 화면은 스크림이 아니라 한 장이라
 * 따로 묻는다. */
function anySurfaceCovers() {
  return (
    document.querySelector(".scrim:not([hidden])") !== null ||
    !el("settings-view").hidden
  );
}

/* ---- 시작 체크리스트 ---- */

/* 백엔드가 마지막으로 답한 목록. `null`은 아직 안 물었다는 뜻이고, 그 상태에서는
 * 사이드바의 한 줄이 서지 않는다 — 재지 않은 것을 그리지 않는다. */
let guideReport = null;
/* GitHub CLI 상태를 이 실행에서 이미 물었나. 체크리스트가 읽는 사실이지만
 * 프로세스를 하나 띄우는 답이라 실행당 한 번만 묻는다. */

function guideStepName(id) {
  const said = {
    "default-agent": () => t("guide.stepAgent", "기본 에이전트 고르기"),
    notifications: () => t("guide.stepNotify", "알림 켜기"),
    github: () => t("guide.stepGithub", "GitHub 연결하기"),
    "two-projects": () => t("guide.stepProjects", "저장소 둘 이상 열기"),
    "two-worktrees": () => t("guide.stepWorktrees", "워크트리에서 따로 작업하기"),
    "parallel-agents": () => t("guide.stepParallel", "에이전트 둘을 나란히 굴리기"),
    "review-diff": () => t("guide.stepDiff", "바뀐 곳 살펴보기"),
    "open-pr": () => t("guide.stepPr", "풀 리퀘스트 만들기"),
  };
  return said[id] ? said[id]() : id;
}

function guideStepWhy(id) {
  const said = {
    "default-agent": () =>
      t("guide.whyAgent", "새 터미널이 곧바로 그 에이전트로 열립니다."),
    notifications: () =>
      t("guide.whyNotify", "에이전트가 끝나거나 사람을 기다릴 때 알려 드립니다."),
    github: () =>
      t("guide.whyGithub", "워크트리마다 풀 리퀘스트 상태와 CI 검사가 보입니다."),
    "two-projects": () =>
      t("guide.whyProjects", "저장소를 오갈 때 폴더를 다시 찾지 않아도 됩니다."),
    "two-worktrees": () =>
      t("guide.whyWorktrees", "체크아웃이 나뉘어 있으면 두 과제가 서로를 밟지 않습니다."),
    "parallel-agents": () =>
      t("guide.whyParallel", "같은 워크스페이스에서 두 에이전트가 다른 각도로 붙습니다."),
    "review-diff": () =>
      t("guide.whyDiff", "에이전트가 무엇을 바꿨는지 줄 단위로 읽습니다."),
    "open-pr": () =>
      t("guide.whyPr", "패널에서 바로 열고, 검사 결과를 그 자리에서 봅니다."),
  };
  return said[id] ? said[id]() : "";
}

function guideSectionName(section) {
  return section === "setup"
    ? t("guide.sectionSetup", "설정")
    : t("guide.sectionMilestone", "이정표");
}

/* 창이 지금 재는 것들. 어디에도 저장되지 않으므로 프로젝트를 하나 지우면 다음
 * 답에서 그 칸이 스스로 돌아간다 — 저장했다면 지워진 저장소가 초록 체크로
 * 남는다. */
function guideFacts() {
  let side = 0;
  for (const project of projects) {
    side += (project.worktrees ?? []).filter((one) => !one.is_main).length;
  }
  return {
    ready: projectsRead,
    projects: projects.length,
    sideWorktrees: side,
    github: currentGithubStanding() === "connected",
  };
}

/* GitHub 상태를 실행당 한 번. 마법사가 이미 물었으면 그 답을 쓴다. */
function ensureGithubStatus() {
  if (githubStatusLoaded) return Promise.resolve();
  if (githubIntegrationPending) return githubIntegrationPending.then(() => undefined, () => undefined);
  return refreshGithubIntegration();
}

/* DELIBERATELY NOT MEMOISED, and the attempt is worth recording.
 *
 * The obvious saving is to skip the round trip when the answer cannot have
 * moved — and it cannot be done from here, because the answer is derived from
 * state the BACKEND owns: the four counts this window passes in AND the
 * onboarding record it keeps. A step completed by doing the thing it describes
 * moves the answer with every count standing still, and the window does not
 * always see that happen. A cache over an answer whose inputs you do not all
 * hold is a cache that serves a stale checklist, which is worse than the round
 * trip it saved. `the checklist is derived from what is true now` caught
 * exactly this.
 *
 * What IS skippable is one CALLER — see `refreshWorktrees`, where the stated
 * reason to ask is that two counts may have changed, and so not asking when
 * they did not is precise rather than hopeful. */
async function refreshSetupGuide() {
  try {
    guideReport = await invoke("setup_guide", guideFacts());
  } catch {
    return;
  }
  paintGuideEntry();
  if (!el("guide-scrim").hidden) paintGuide();
}

/* 사이드바의 한 줄. 뜨는 조건 셋은 백엔드가 정한다(`entry_visible`) — 다 끝나면
 * 스스로 사라지고, 치운 사람에게는 돌아오지 않는다. */
function paintGuideEntry() {
  const entry = el("guide-entry");
  if (!guideReport) {
    entry.hidden = true;
    return;
  }
  entry.hidden = !guideReport.entry;
  const done = guideReport.steps.filter((one) => one.done).length;
  const total = guideReport.steps.length;
  el("guide-ring").style.setProperty("--guide-done", total ? String(done / total) : "0");
  say(el("guide-entry-label"), () =>
    t("guide.entry", "시작하기 ({{done}}/{{total}})", { done, total }),
  );
}

function paintGuide() {
  const host = el("guide-body");
  host.replaceChildren();
  if (!guideReport) return;
  for (const section of ["setup", "milestone"]) {
    const mine = guideReport.steps.filter((one) => one.section === section);
    if (!mine.length) continue;
    const group = document.createElement("section");
    const head = document.createElement("h3");
    head.className = "guide-section-head";
    const name = document.createElement("span");
    say(name, () => guideSectionName(section));
    const tally = document.createElement("span");
    const done = mine.filter((one) => one.done).length;
    say(tally, () =>
      t("guide.tally", "{{done}}/{{total}}", { done, total: mine.length }),
    );
    head.append(name, tally);
    group.appendChild(head);
    for (const step of mine) {
      const row = document.createElement("div");
      row.className = "guide-step";
      row.dataset.step = step.id;
      row.dataset.done = step.done ? "true" : "false";
      const mark = document.createElement("span");
      mark.className = "guide-mark";
      mark.setAttribute("aria-hidden", "true");
      const words = document.createElement("span");
      const label = document.createElement("span");
      label.className = "guide-step-name";
      say(label, () => guideStepName(step.id));
      const why = document.createElement("span");
      why.className = "guide-step-why";
      say(why, () => guideStepWhy(step.id));
      words.append(label, why);
      // 끝난 칸에도 이름을 그대로 읽힌다 — 화면으로는 표시가 갈리지만 소리로는
      // 갈리지 않으므로, 상태를 문장으로 한 번 더 말한다.
      row.setAttribute(
        "aria-label",
        `${guideStepName(step.id)} — ${
          step.done ? t("guide.done", "완료") : t("guide.todo", "아직")
        }`,
      );
      row.append(mark, words);
      group.appendChild(row);
    }
    host.appendChild(group);
  }
}

function openSetupGuide() {
  paintGuide();
  showModal(el("guide-scrim"));
  void refreshSetupGuide();
}

function closeSetupGuide() {
  hideModal(el("guide-scrim"));
}

el("settings-onboarding-open").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    const state = await invoke("reopen_onboarding");
    setSettingsOpen(false);
    openOnboarding(state);
  } catch (error) {
    showError(error);
  } finally {
    button.disabled = false;
  }
});

el("settings-guide-open").addEventListener("click", () => {
  setSettingsOpen(false);
  openSetupGuide();
});

el("guide-entry").addEventListener("click", openSetupGuide);
el("guide-close").addEventListener("click", closeSetupGuide);
/* 치우기. 되돌리는 길은 도움말 메뉴이고, 그 사실을 툴팁이 말한다 — 돌아올 길
 * 없는 숨기기는 삭제이지 숨기기가 아니다. */
el("guide-hide").addEventListener("click", () => {
  invoke("set_guide_dismissed", { dismissed: true })
    .then((next) => {
      onboarding = next;
      closeSetupGuide();
      return refreshSetupGuide();
    })
    .catch(showError);
});
el("guide-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("guide-scrim")) closeSetupGuide();
});

/* ---- 기능 투어 (수동 전용) ---- */

/* 갈래 넷. 이름이 무엇을 막는 벽처럼 들리지만 아무것도 막지 않는다 — 자동으로
 * 뜨는 길이 없고 도움말의 한 항목에서만 열린다. */
const WALL = ["workspaces", "agents", "workbench", "review"];
let wallAt = 0;

function wallCopy(id) {
  const said = {
    workspaces: () => ({
      title: t("wall.workspacesTitle", "워크스페이스"),
      meta: t("wall.workspacesMeta", "격리된 체크아웃 · 나란히"),
      lede: t("wall.workspacesLede", "과제마다 자기 체크아웃을 갖습니다. 같은 저장소 안에서도 서로를 밟지 않으므로 에이전트 둘이 동시에 일할 수 있습니다."),
      points: [
        t("wall.workspacesPoint1", "워크트리마다 브랜치가 따로 섭니다."),
        t("wall.workspacesPoint2", "쓰지 않는 체크아웃은 사이드바가 먼저 알려 줍니다."),
      ],
    }),
    agents: () => ({
      title: t("wall.agentsTitle", "에이전트"),
      meta: t("wall.agentsMeta", "보드 · 사용량 · 알림"),
      lede: t("wall.agentsLede", "여러 에이전트를 한 번에 굴리고, 무엇이 일하고 무엇이 사람을 기다리는지 보드 한 장에서 봅니다."),
      points: [
        t("wall.agentsPoint1", "확인이 필요한 에이전트가 먼저 섭니다."),
        t("wall.agentsPoint2", "계정별 사용량과 한도가 상태바에 붙습니다."),
      ],
    }),
    workbench: () => ({
      title: t("wall.workbenchTitle", "작업대"),
      meta: t("wall.workbenchMeta", "터미널 · 편집기 · 파일"),
      lede: t("wall.workbenchLede", "터미널을 나누어 서버와 시험과 로그를 한 화면에 세우고, 파일은 그 자리에서 열어 고칩니다."),
      points: [
        t("wall.workbenchPoint1", "패널은 트리로 나뉘고 탭은 페인마다 섭니다."),
        t("wall.workbenchPoint2", "열린 파일은 에이전트가 바꿔도 덮이지 않습니다."),
      ],
    }),
    review: () => ({
      title: t("wall.reviewTitle", "리뷰"),
      meta: t("wall.reviewMeta", "diff · 메모 · 풀 리퀘스트"),
      lede: t("wall.reviewLede", "무엇이 바뀌었는지 읽고, 줄에 메모를 달아 그대로 에이전트에게 돌려보냅니다."),
      points: [
        t("wall.reviewPoint1", "메모는 보낸 뒤 자동으로 정리됩니다."),
        t("wall.reviewPoint2", "풀 리퀘스트와 검사 결과가 같은 패널에 섭니다."),
      ],
    }),
  };
  return said[id] ? said[id]() : { title: id, meta: "", lede: "", points: [] };
}

function paintFeatureWall() {
  const rail = el("wall-rail");
  rail.replaceChildren();
  const seen = new Set(onboarding?.wall_seen ?? []);
  WALL.forEach((id, at) => {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "wall-rail-item";
    item.dataset.wall = id;
    item.dataset.seen = seen.has(id) ? "true" : "false";
    if (at === wallAt) item.setAttribute("aria-current", "page");
    say(item, () => wallCopy(id).title);
    item.addEventListener("click", () => {
      wallAt = at;
      paintFeatureWall();
    });
    rail.appendChild(item);
  });

  const id = WALL[wallAt];
  const host = el("wall-step");
  host.replaceChildren();
  host.dataset.wall = id;
  const title = document.createElement("h3");
  title.className = "wall-step-title";
  say(title, () => wallCopy(id).title);
  const meta = document.createElement("p");
  meta.className = "wall-step-meta";
  say(meta, () => wallCopy(id).meta);
  const lede = document.createElement("p");
  lede.className = "wall-step-lede";
  say(lede, () => wallCopy(id).lede);
  const points = document.createElement("ul");
  points.className = "wall-points";
  wallCopy(id).points.forEach((_, at) => {
    const point = document.createElement("li");
    say(point, () => wallCopy(id).points[at]);
    points.appendChild(point);
  });
  host.append(title, meta, lede, points);

  const last = wallAt === WALL.length - 1;
  say(el("wall-next"), () =>
    last ? t("wall.done", "완료") : t("wall.next", "다음"),
  );
  // 읽은 것으로 적는다. Orca도 방문 시점에 적고, 그것이 레일의 점이 뜻하는
  // 전부다 — 끝까지 읽었는가가 아니라 들렀는가다.
  if (!seen.has(id)) void markFirstRunSeen("wall", id);
}

/* 도움말 메뉴에서만. 자동으로 이것을 여는 길은 이 창에 없다. */
function openFeatureWall() {
  wallAt = 0;
  paintFeatureWall();
  showModal(el("wall-scrim"));
}

function closeFeatureWall() {
  hideModal(el("wall-scrim"));
}

el("wall-close").addEventListener("click", closeFeatureWall);
el("wall-next").addEventListener("click", () => {
  if (wallAt === WALL.length - 1) {
    closeFeatureWall();
    return;
  }
  wallAt += 1;
  paintFeatureWall();
});
el("wall-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("wall-scrim")) closeFeatureWall();
});

/* ---- 일회성 팁 ---- */

/* 이 실행이 마법사 때문에 봉인됐나, 그리고 이미 하나 띄웠나. 둘 다 이 창의
 * 수명만큼만 사는 사실이라 저장하지 않는다. */
let tipSealed = false;
let tipSpent = false;
let tipShowing = null;

function tipCopy(id) {
  const said = {
    palette: () => ({
      title: t("tip.paletteTitle", "어디로든 한 번에"),
      body: t("tip.paletteBody", "빠른 열기로 워크트리와 파일을 키보드만으로 찾아 엽니다."),
      cta: t("tip.paletteCta", "지금 열어 보기"),
    }),
    panels: () => ({
      title: t("tip.panelsTitle", "패널 폭은 손으로 정합니다"),
      body: t("tip.panelsBody", "기둥 사이의 경계를 끌면 폭이 바뀌고, 그 폭은 다음 실행에도 그대로 섭니다."),
      cta: t("tip.panelsCta", "알겠습니다"),
    }),
    editor: () => ({
      title: t("tip.editorTitle", "파일은 여기서 바로 고칩니다"),
      body: t("tip.editorBody", "파일 패널에서 연 문서는 에이전트가 같은 파일을 바꿔도 덮이지 않습니다."),
      cta: t("tip.editorCta", "파일 패널 열기"),
    }),
  };
  return said[id] ? said[id]() : { title: id, body: "", cta: "" };
}

/* 팁의 행동. 없는 팁은 닫기만 한다 — 아무 데도 데려가지 않는 CTA는 버튼이
 * 아니라 장식이다. */
function runTipAction(id) {
  if (id === "palette") openPalette("worktree");
  // 파일 패널의 자기 문. 이름으로 찾는 쪽으로 도착하는 것까지 그 문이 정한다 —
  // 팁이 자기 사본을 들면 두 문이 서로 다른 곳에 내린다.
  else if (id === "editor") revealActivity("files", "name");
}

function openTip(id) {
  tipShowing = id;
  say(el("tip-title"), () => tipCopy(id).title);
  say(el("tip-body"), () => tipCopy(id).body);
  say(el("tip-go"), () => tipCopy(id).cta);
  showModal(el("tip-scrim"));
}

function closeTip(run) {
  const id = tipShowing;
  tipShowing = null;
  hideModal(el("tip-scrim"));
  if (run && id) runTipAction(id);
}

/* 이번 오픈에 하나. 판정은 코어가 하고, 여기서는 그 답을 따를 뿐이다.
 *
 * `suppress`는 마법사가 떠 있다는 뜻이고, 그 답을 받으면 이 실행 **전체**를
 * 봉인한다 — 그러지 않으면 마법사를 닫는 순간 팁이 튀어나와 첫 성공의 자리를
 * 덮는다. */
async function maybeShowTip() {
  if (isPopout || tipSealed || tipSpent) return;
  let verdict;
  try {
    verdict = await invoke("tip_verdict", {
      ready: projectsRead,
      sealed: tipSealed,
      spentThisOpen: tipSpent,
      modalOpen: anySurfaceCovers(),
    });
  } catch {
    return;
  }
  if (verdict.verdict === "suppress") {
    tipSealed = true;
    return;
  }
  if (verdict.verdict !== "show" || !verdict.id) return;
  // 이미 그 기능을 쓰고 있는 사람에게는 보여 주지 않고 조용히 끝낸다.
  for (const id of verdict.settle ?? []) await markFirstRunSeen("tip", id);
  tipSpent = true;
  await markFirstRunSeen("tip", verdict.id);
  openTip(verdict.id);
}

el("tip-later").addEventListener("click", () => closeTip(false));
el("tip-go").addEventListener("click", () => closeTip(true));
el("tip-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("tip-scrim")) closeTip(false);
});

/* ---- 코치마크 투어 ---- */

/* 스텝 목록. 앵커는 `data-tour-target` 하나의 규약이고, 그 속성이 붙은 요소가
 * 화면에 없으면 그 스텝은 존재하지 않는 것으로 읽힌다 — 가리킬 것이 없는
 * 말풍선은 화면 한복판에 뜨는 설명문이 되고, 그것은 투어가 아니다.
 *
 * Orca의 일곱 중 셋만 옮겼다. `browser`·`workspace-agent-sessions`·
 * `floating-workspace`·`workspace-creation`은 우리에게 대응 표면이 없거나
 * (브라우저 페인) 가리킬 고정 앵커가 없어(떠 있는 터미널·워크트리 만들기
 * 다이얼로그) 죽은 코치마크가 된다. */
const TOURS = {
  board: [
    { target: "agent-graph", place: "top" },
    { target: "board-search", place: "bottom" },
  ],
  tasks: [
    { target: "task-sources", place: "bottom" },
    { target: "task-list", place: "top" },
  ],
  automations: [
    { target: "automations-create", place: "bottom" },
    { target: "automations-list", place: "right" },
  ],
};

/* 지금 떠 있는 투어와 그 안의 자리. `null`이면 아무것도 안 떠 있다. */
let activeTour = null;
/* 이 실행에서 이미 하나를 보여 줬다. Orca의 "세션당 1개"가 이 한 줄이다. */
let tourSpent = false;
/* 자리를 다시 재게 하는 구독들. 투어가 닫히면 함께 걷힌다. */
/* A coachmark tracks moving anchors twice a second while it is open; faster
 * checks add layout work without making the pointer visibly steadier. */
const TOUR_REMEASURE_MS = 500;
let tourWatch = [];

function tourCopy(id, at) {
  const said = {
    "board:0": () => ({
      title: t("tour.boardTitle", "상태로 보는 한 장"),
      body: t("tour.boardBody", "프로젝트가 아니라 상태로 늘어놓습니다. 무엇이 사람을 기다리는지가 먼저 섭니다."),
    }),
    "board:1": () => ({
      title: t("tour.boardSearchTitle", "찾는 에이전트만"),
      body: t("tour.boardSearchBody", "이름이나 워크스페이스로 좁힙니다. 대기 중은 기본으로 접혀 있습니다."),
    }),
    "tasks:0": () => ({
      title: t("tour.tasksTitle", "일감이 오는 곳"),
      body: t("tour.tasksBody", "연결된 곳들을 탭으로 오갑니다. 쓰지 않는 곳은 설정에서 치울 수 있습니다."),
    }),
    "tasks:1": () => ({
      title: t("tour.tasksListTitle", "여기서 바로 시작"),
      body: t("tour.tasksListBody", "이슈나 풀 리퀘스트를 고르면 그 맥락을 그대로 안고 워크스페이스가 섭니다."),
    }),
    "automations:0": () => ({
      title: t("tour.autoTitle", "자동화란"),
      body: t("tour.autoBody", "정해진 시각에 에이전트가 스스로 일합니다. 이 버튼으로 하나 만듭니다."),
    }),
    "automations:1": () => ({
      title: t("tour.autoListTitle", "무엇이 언제 돌았나"),
      body: t("tour.autoListBody", "예약한 것들이 여기 서고, 하나를 고르면 지난 실행과 그 결과가 붙습니다."),
    }),
  };
  const key = `${id}:${at}`;
  return said[key] ? said[key]() : { title: key, body: "" };
}

/* 이 요소가 실제로 재어질 수 있나.
 *
 * 조상을 거슬러 올라가며 감춰진 것을 거절하고, 유한하고 크기가 0이 아닌 자리를
 * 요구한다(Orca의 측정 규칙 그대로). `hidden` 하나만 보면 부모가 접힌 채로 남은
 * 요소를 "보인다"고 읽는다. */
function tourTargetOf(step) {
  const node = document.querySelector(`[data-tour-target="${step.target}"]`);
  if (!node) return null;
  for (let walk = node; walk && walk !== document.documentElement; walk = walk.parentElement) {
    if (walk.hidden || walk.getAttribute("aria-hidden") === "true") return null;
    const shown = getComputedStyle(walk);
    if (shown.display === "none" || shown.visibility === "hidden") return null;
  }
  const box = node.getBoundingClientRect();
  if (!Number.isFinite(box.width) || box.width <= 0 || box.height <= 0) return null;
  return node;
}

/* 이번 측정이 무엇을 뜻하나 — 네 동사 중 하나.
 *
 * Orca가 앵커 소실을 자가 치유하는 방식이고, 이식할 값이 가장 높은 부분이다.
 * 지금 스텝의 앵커가 사라졌을 때 그냥 닫아 버리면 접히는 패널 하나가 투어를
 * 죽이고, 그냥 붙잡고 있으면 아무것도 가리키지 않는 말풍선이 남는다. */
function tourVerb(steps, at) {
  if (tourTargetOf(steps[at])) return "render";
  const later = steps.slice(at + 1).some((one) => tourTargetOf(one));
  if (later) return "advance";
  const earlier = steps.slice(0, at).some((one) => tourTargetOf(one));
  if (earlier && at < steps.length - 1) return "wait";
  return "cancel";
}

/* 말풍선을 타깃 옆에. 창 밖으로 나가면 안쪽으로 밀어 넣는다 — 반대쪽으로
 * 뒤집는 것까지는 하지 않는다: 이 창의 앵커는 전부 화면 가장자리에서 한참
 * 떨어져 있고, 뒤집기는 그 사실이 바뀌는 날 필요해진다. */
function placeTourPanel(node, place) {
  const gap = 12;
  const box = node.getBoundingClientRect();
  const panel = el("tour-panel");
  const size = panel.getBoundingClientRect();
  let left = box.left + box.width / 2 - size.width / 2;
  let top = box.top + box.height / 2 - size.height / 2;
  if (place === "top") top = box.top - size.height - gap;
  else if (place === "bottom") top = box.bottom + gap;
  else if (place === "left") left = box.left - size.width - gap;
  else left = box.right + gap;
  const edge = 12;
  left = Math.min(Math.max(edge, left), window.innerWidth - size.width - edge);
  top = Math.min(Math.max(edge, top), window.innerHeight - size.height - edge);
  panel.style.left = `${Math.round(left)}px`;
  panel.style.top = `${Math.round(top)}px`;

  const arrow = el("tour-arrow");
  const across = place === "top" || place === "bottom";
  const centre = across
    ? Math.min(Math.max(16, box.left + box.width / 2 - left), size.width - 16)
    : Math.min(Math.max(16, box.top + box.height / 2 - top), size.height - 16);
  arrow.style.left = across ? `${Math.round(centre - 5)}px` : place === "left" ? "auto" : "-6px";
  arrow.style.right = across ? "auto" : place === "left" ? "-6px" : "auto";
  arrow.style.top = across ? (place === "top" ? "auto" : "-6px") : `${Math.round(centre - 5)}px`;
  arrow.style.bottom = across && place === "top" ? "-6px" : "auto";

  const ring = el("tour-ring");
  ring.hidden = false;
  ring.style.left = `${Math.round(box.left - 4)}px`;
  ring.style.top = `${Math.round(box.top - 4)}px`;
  ring.style.width = `${Math.round(box.width + 8)}px`;
  ring.style.height = `${Math.round(box.height + 8)}px`;
}

function paintTour() {
  if (!activeTour) return;
  const steps = TOURS[activeTour.id];
  if (!steps) return;
  const verb = tourVerb(steps, activeTour.at);
  if (verb === "cancel") {
    closeTour();
    return;
  }
  if (verb === "advance") {
    activeTour.at += 1;
    paintTour();
    return;
  }
  // `wait`은 아직 못 그리는 것이지 끝난 것이 아니다. 말풍선을 감춰 두고 다음
  // 측정을 기다린다 — 다시 그리는 것은 아래의 구독들이 부른다.
  if (verb === "wait") {
    el("tour-panel").hidden = true;
    el("tour-ring").hidden = true;
    return;
  }
  el("tour-panel").hidden = false;
  // 지금 자리를 **값으로** 붙든다. `say`가 들고 있는 것은 문장이 아니라 문장을
  // 만드는 법이고, 언어가 바뀌면 그것을 다시 부른다 — 그때 `activeTour`를 다시
  // 읽으면 이미 닫힌 투어의 `null`을 읽어 창 전체가 그 자리에서 멈춘다.
  const where = activeTour.at;
  const which = activeTour.id;
  const step = steps[where];
  const said = tourCopy(which, where);
  say(el("tour-title"), () => tourCopy(which, where).title);
  say(el("tour-body"), () => tourCopy(which, where).body);
  say(el("tour-count"), () =>
    t("tour.counter", "{{step}} / {{total}}", {
      step: where + 1,
      total: steps.length,
    }),
  );
  const last = where === steps.length - 1;
  say(el("tour-next"), () => (last ? t("tour.done", "완료") : t("tour.next", "다음")));
  el("tour-back").hidden = where === 0;
  el("tour-panel").setAttribute("aria-label", said.title);
  placeTourPanel(tourTargetOf(step), step.place);
}

/* 자리를 다시 재게 하는 것들. 스크롤과 리사이즈는 앵커를 움직이고, 화면이
 * 접히거나 펴지는 것은 앵커를 없애거나 되살린다. */
function watchTour() {
  const again = () => paintTour();
  window.addEventListener("resize", again);
  window.addEventListener("scroll", again, true);
  const timer = window.setInterval(() => {
    if (!document.hidden) again();
  }, TOUR_REMEASURE_MS);
  tourWatch = [
    () => window.removeEventListener("resize", again),
    () => window.removeEventListener("scroll", again, true),
    () => window.clearInterval(timer),
  ];
}

function startTour(id) {
  activeTour = { id, at: 0 };
  tourSpent = true;
  el("tour").hidden = false;
  // 뜬 시점에 본 것으로 적는다. 완료가 아니라 **최초 렌더**가 기준이라는 것이
  // Orca의 선택이고, 그 편이 옳다 — 한 번 보고 닫은 것을 다음 실행에 다시
  // 띄우면 그것은 안내가 아니라 반복이다.
  void markFirstRunSeen("tour", id);
  watchTour();
  paintTour();
  el("tour-panel").focus({ preventScroll: true });
}

function closeTour() {
  activeTour = null;
  for (const drop of tourWatch) drop();
  tourWatch = [];
  el("tour").hidden = true;
  el("tour-panel").hidden = false;
  el("tour-ring").hidden = true;
}

/* 이 화면이 그 투어의 자리가 됐다 — 띄울까?
 *
 * 판정은 백엔드가 한다. 창은 자기가 아는 사실만 실어 보내고 그 답을 따른다;
 * 거절 사유는 이름으로 돌아오므로 "왜 안 뜨지"에 답할 수 있다. */
function askForTour(id) {
  if (isPopout) return Promise.resolve();
  const steps = TOURS[id];
  if (!steps) return Promise.resolve();
  return invoke("tour_decision", {
    ask: {
      id,
      here: true,
      ready: projectsRead,
      modalOpen: anySurfaceCovers(),
      activeTour: activeTour !== null,
      spentThisSession: tourSpent,
      hasTarget: tourTargetOf(steps[0]) !== null,
    },
  })
    .then((why) => {
      if (why === null || why === undefined) startTour(id);
    })
    .catch(() => {});
}

el("tour-x").addEventListener("click", () => closeTour());
el("tour-back").addEventListener("click", () => {
  if (!activeTour || activeTour.at === 0) return;
  activeTour.at -= 1;
  paintTour();
});
el("tour-next").addEventListener("click", () => {
  if (!activeTour) return;
  const steps = TOURS[activeTour.id];
  if (activeTour.at === steps.length - 1) {
    closeTour();
    return;
  }
  activeTour.at += 1;
  paintTour();
});

/* ---- last process diagnostics ---- */
let lastCrashShown = false;
let hangBadgeCount = 0;

function showLastCrash(report) {
  if (!report || lastCrashShown || isPopout) return;
  lastCrashShown = true;
  say(el("crash-kind"), () => report.kind === "killed"
    ? t("crash.killed", "지난 창이 정상 종료 기록 없이 닫혔습니다.")
    : report.kind === "hang"
      ? t("crash.hang", "지난 실행에서 창의 응답이 늦어졌습니다.")
      : t("crash.panic", "지난 실행에서 내부 오류가 발생했습니다."));
  el("crash-summary").textContent = report.summary ?? "";
  say(el("crash-ledger"), () => t("crash.ledger", "기록 당시 워커 {{workers}}개 · 배차 {{dispatches}}개", {
    workers: report.ledger?.workers ?? 0, dispatches: report.ledger?.dispatches ?? 0,
  }));
  const list = el("crash-crumbs");
  list.replaceChildren();
  for (const crumb of report.crumbs ?? []) {
    const row = document.createElement("li");
    row.textContent = `${new Date(crumb.at_ms).toLocaleTimeString()}  ${crumb.line}`;
    list.append(row);
  }
  if (!list.childElementCount) {
    const row = document.createElement("li");
    say(row, () => t("crash.noCrumbs", "강제 종료는 메모리의 활동 기록을 남기지 못할 수 있습니다."));
    list.append(row);
  }
  showModal(el("crash-sheet"));
}

function closeCrashSheet() { hideModal(el("crash-sheet")); }

function showHangBadge(report) {
  if (isPopout || !Number.isFinite(report?.ms) || report.ms <= 0 || report.count <= hangBadgeCount) return;
  hangBadgeCount = report.count;
  const badge = el("sb-hang");
  say(badge, () => t("crash.hangBadge", "창이 {{seconds}}초 멈췄었습니다", { seconds: Math.round(report.ms / 1000) }));
  badge.hidden = false;
}

/* The beat filed the sheet's incident as a task (t-3014 §2.4). One line on
 * the sheet, whether it is open yet or not: the event can land before
 * `boot()` shows the sheet, and the line waits on the hidden dialog. */
function noteCrashTriaged(payload) {
  const task = typeof payload?.taskId === "string" ? payload.taskId : "";
  if (!task || isPopout) return;
  const line = el("crash-filed");
  say(line, () => t("crash.filed", "과업 {{task}} 으로 올렸습니다", { task }));
  line.hidden = false;
}

listen("crash:hang", (event) => showHangBadge(event.payload));
listen("crash:triaged", (event) => noteCrashTriaged(event.payload));
el("sb-hang").addEventListener("click", () => { el("sb-hang").hidden = true; });
el("crash-close").addEventListener("click", closeCrashSheet);
el("crash-sheet").addEventListener("click", (event) => { if (event.target === el("crash-sheet")) closeCrashSheet(); });
el("crash-open-log").addEventListener("click", () => {
  invoke("crash_open_log").catch(() => {
    say(el("crash-result"), () => t("crash.logFailed", "로그를 열지 못했습니다."));
  });
});
el("crash-bundle").addEventListener("click", async () => {
  const button = el("crash-bundle");
  if (button.disabled) return;
  button.disabled = true;
  say(el("crash-result"), () => t("crash.creating", "진단 폴더를 만들고 있습니다…"));
  try {
    await invoke("crash_bundle");
    say(el("crash-result"), () => t("crash.created", "진단 폴더를 열었습니다. 필요한 곳에 직접 첨부하세요."));
  } catch {
    button.disabled = false;
    say(el("crash-result"), () => t("crash.bundleFailed", "진단 폴더를 만들지 못했습니다. 다시 시도해 주세요."));
  }
});

/* ---- boot ---- */

async function boot() {
  const report = await invoke("boot_report");
  // `boot_report` flattens the same document `settings_snapshot` returns.
  // Applying it through the same door is what makes the first frame, a
  // reopened settings page and another window agree on every preference.
  applySettingsSnapshot(report);
  showLastCrash(report.last_crash);
  if (report.lane_error) {
    stageError = t("stage.laneStorage", "세션 토큰 저장소를 읽거나 쓸 수 없습니다. 터미널과 설정은 계속 사용할 수 있지만, 레인을 열기 전에 앱 데이터 접근을 복구해 주세요.");
  }
  zoAvailable = report.zo != null;
  // Decided here, once, with the real answer. `zoAvailable` is `false` until
  // this line and the settings dialog is already reachable — a language change
  // in that window would otherwise paint "zo is missing" from the placeholder
  // value and take the element's key off with it, and booting would never say
  // otherwise because the only call sat inside the branch below.
  paintStagePlaceholder();
  // After `zoAvailable`, because the sweep repaints that message too and it
  // has two forms — running it earlier wrote the wrong one and then corrected
  // itself, which is the flash the boot report exists to avoid.
  paintChordTitles();
  // The right column names its project too, the way Orca's does — it matters
  // the moment a second window is open on a different checkout.
  paintProjectName(report.project ?? null);
  // No `serve ready` segment: Orca's bar leads straight into the usage
  // segments and carries no daemon-state field (StatusBar-DNeqEWup.js,
  // segment roster), and ours was reported as noise in so many words
  // ("serve ready 없어도 되고"). A serve failure still reaches the person:
  // `stageError` is painted on the stage placeholder, which is where a
  // window with no working lanes is looking.
  if (report.branch) {
    const branch = el("branch");
    branch.textContent = report.branch;
    branch.hidden = false;
  }
  refreshScm()
    .then(() => loadTree(fileTree, ""))
    .catch(() => loadTree(fileTree, ""));
  // Before the `zo` check: worktrees are git's, not the harness's, so they
  // list on a machine that has no `zo` at all (F2b). Recents are the state
  // directory's, which is the same argument. Await it because the first PTY
  // must be owned by the active workspace before its tab is created.
  await refreshWorktrees();
  // The workspace the window opened on is where 뒤로 eventually bottoms out.
  if (activeWorktreePath) recordNavVisit(activeWorktreePath);
  paintCommandLabel();
  // The strip first, and before anything is in it. It carries the `＋`, and
  // `renderTabs` otherwise only runs once a tab exists — so a window whose
  // terminal failed to open had no strip, no `＋` and no way forward but a
  // chord nobody had been told about. That is the window that was reported.
  renderTabs();
  // The centre is a terminal. Orca's centre tab is literally `Terminal 1`
  // running a shell, and what you run in it is your business — `zo`,
  // `claude`, anything on PATH. Opening one here is what makes that true from
  // the first frame instead of after you find `⌘T`.
  //
  // Through the workspace's own road rather than straight to `openTermTab`,
  // because a window that has been open before is not opening for the first
  // time: this workspace has a set of tabs it was last looked at with, and
  // Orca's boot puts that set back (§11 — the tab set is persisted per
  // worktree and restored on activation; nothing is spawned for the
  // workspaces you are not standing in). The road ends in exactly the plain
  // terminal this used to open when there is nothing stored, which is every
  // first run and every workspace nobody has opened a tab in.
  //
  // Awaited, so a terminal that will not open is something the window says
  // rather than something it hides behind the copy that invites you to open
  // one yourself.
  await restoreActiveWorktreeTab();
  // Browser addresses are settings-backed and restored only when explicitly
  // enabled. They open after the workspace owns its first leaf, so no native
  // page can attach to the checkout that happened to be active before boot.
  await restoreBrowserTabs();
  // The plan segment and the agent registry, off the critical path: the
  // last snapshot paints at once and the scan renews it behind the bar.
  refreshAgents();
  refreshBrowserProfiles();
  refreshClaudeUsage(false);
  refreshCodexUsage(false);
  // The release lane's files, once at boot and then on the bar's own clock:
  // a build swapped under a window that was closed is the ordinary case.
  askReleaseStatus();
  // 치울 것이 있는지, 부팅 경로 밖에서. 사이드바의 한 줄은 이 답이 오면
  // 나타나고, 오지 않으면 나타나지 않는다 — 정리 버튼 하나 때문에 창이 뜨는
  // 시간이 늘어나서는 안 된다.
  scanCleanup().catch(() => {});
  // The resume map, reseeded: the backend outlives a webview reload, and
  // every conversation it still holds deserves its 이어서 row back — waiting
  // for each agent's NEXT hook event would leave the menu empty exactly as
  // long as the agent is quietly working.
  const startupSessions = invoke("pane_sessions")
    .then((rows) => {
      for (const row of rows ?? []) {
        paneSessions.set(row.term, {
          agent: row.agent,
          session: row.session,
          resumable: row.resumable,
        });
      }
      agentClock.sync();
    })
    .catch(() => {});
  // And the helpers running inside those agents, for the same reason and off
  // the same critical path: the backend kept them while the webview was
  // reloading, and waiting for each one's NEXT event means waiting for it to
  // STOP — which is the one event that takes its row away.
  const startupSubagents = seedSubagents().then(paintWorktreeAgents);
  // Which agent holds which pane, for the header's continue button — same
  // argument: the backend registered these at launch, and waiting for each
  // agent's next hook means a reloaded window hides the button until then.
  const startupAgents = seedPaneAgents();
  // 재시작을 건너온 판들 (P0-13) — Orca가 last-status.json을 hydrate해
  // 보드를 채우듯, 원장의 지난 소식이 빈 카드의 흐린 행이 된다. 같은
  // 자리·같은 이유: 백엔드가 파일을 들고 있었고, 다음 훅을 기다리면 그
  // 행은 영영 안 온다.
  invoke("last_statuses")
    .then((rows) => {
      lastNews = rows ?? [];
      if (lastNews.length > 0) scheduleAgentPaint(["cards"]);
    })
    .catch(() => {});
  // 그리고 그 에이전트들이 무엇을 하고 있는지. 같은 이유·같은 자리다: 백엔드가
  // 링을 들고 있었고, 다음 이벤트를 기다린다는 것은 이 창이 지금 돌고 있는
  // 도구 호출 하나를 통째로 놓친다는 뜻이다.
  const startupActivities = seedActivities().then(paintWorktreeAgents);
  void Promise.allSettled([startupSessions, startupSubagents, startupAgents, startupActivities]).then(() => {
    startupProjectFoldsReady = true;
    paintWorktreeAgents();
  });
  // 첫 실행 마법사. 판정은 저장된 상태 하나로 끝난다 — 이 창은 "처음인가"를
  // 계산하지 않고 물어보지도 않는다(1-ea). 사이드바와 첫 터미널이 선 다음에
  // 열리므로, 건너뛰고 나면 그 뒤에 완성된 창이 이미 서 있다.
  onboarding = report.onboarding ?? null;
  if (shouldShowOnboarding(report.onboarding)) openOnboarding(report.onboarding);
  // 레인 둘만 zo의 것이다. 이 문이 `return`이던 시절, zo 없는 기계는
  // 워크트리도 첫 터미널도 GitHub 상태도 받지 못했다 — 전부 git과 이 창의
  // 것이지 zo의 것이 아닌데도. zo.rs 첫 줄의 계약("IDE first, agent
  // front-end second")이 부트에서 어겨진 자리였고, 접히는 것은 레인 표면
  // 하나여야 한다.
  if (zoAvailable) {
    report.lanes.forEach(upsertLaneWithSubagents);
    if (report.focused) await focusLane(report.focused);
  }
  await refreshWorktrees();
  // 마법사 다음의 표면들, 창이 다 선 뒤에. 순서가 뜻이다: GitHub 상태는
  // 체크리스트가 읽는 사실이므로 먼저 오고, 팁은 맨 끝이다 — 마법사가 떠 있는
  // 실행이면 그 답이 이 실행 전체를 봉인한다.
  await ensureGithubStatus();
  await refreshSetupGuide();
  await maybeShowTip();
}

/* 팝아웃 창의 부팅.
 *
 * `boot`가 하는 일의 대부분은 이 창에 없는 것들에 대한 것이다 — 사이드바,
 * 워크트리, 첫 터미널, 훅 브리지, 독 배지. 그 함수에 분기를 스무 개 심는 대신
 * 여기서 필요한 것만 순서대로 한다. **그리는 함수는 하나도 새로 쓰지 않는다**:
 * 보드는 `paintBoardView`가, 카드는 `boardCardNode`가, 미리보기는
 * `openBoardPeek`이 그린다 — 메인 창이 쓰는 바로 그것들이다.
 *
 * 스냅샷 릴레이도 없다. 이 창은 `pane_agents`·`board_columns`를 직접 부르고
 * `hook:agent`·`term:exited`를 직접 듣는다(Tauri의 `emit`은 모든 웹뷰에
 * 닿는다). 그래서 두 창은 같은 백엔드를 각자 읽을 뿐, 한쪽이 다른 쪽에게
 * 보드를 먹여 주지 않는다 — 이것이 릴레이 계층 전체를 안 만든 이유다. */
async function bootPopout() {
  const report = await invoke("boot_report");
  // The pop-out consumes the same snapshot and settings events as the main
  // window. Its OS frame is opaque, so blur intent is retained but active
  // material is false for this process window.
  applySettingsSnapshot({ ...report, window_blur_active: false });
  // 헬퍼 목록을 먼저 씨앗으로 받는다 — 이 창은 그것들이 시작된 뒤에 열리는
  // 창이고, 다음 이벤트를 기다린다는 것은 곧 그것들이 **멈추기를** 기다린다는
  // 뜻이다. 그리기 전에 await 하는 이유가 그것이다: 헬퍼 없는 보드를 한 프레임
  // 보여 주고 고치는 것이 바로 이 창이 하지 않기로 한 일이다.
  await seedSubagents();
  // 그리고 그들이 무엇을 하고 있는지. 같은 await인 이유도 같다 — 스로틀이
  // 있는 흐름이라 "다음 이벤트"는 100ms 뒤일 수도, 그 에이전트가 다음 도구를
  // 집을 때일 수도 있다.
  await seedActivities();
  // 탭도 무대도 세우지 않는다: 보드 한 장을 직접 드러내고 그린다.
  el("board-view").hidden = false;
  await paintBoardView();
}

/* ---- what this window is still holding (1-ev) ----------------------------
 *
 * Every registry keyed by a shell, a tab, a lane or a checkout, counted. The
 * leak gate opens thirty terminals, streams frames into all of them, closes
 * them and asks this whether the window came back to where it started — a
 * question no assertion about the SCREEN can answer, because a map that grows
 * forever draws nothing at all until the machine is on its knees.
 *
 * Sizes and never contents: this exists to be cheap enough that its presence
 * is not itself a cost, and a debug door that serialises the window's state is
 * a debug door somebody eventually calls in a loop. Nothing in the window
 * reads it; it is written here rather than in the test because the registries
 * are this file's own `const`s and a test cannot name them from outside.
 *
 * `spokenStrong` is the one entry that is not a size. `spokenNodes` holds
 * `WeakRef`s so that a repaint's leftovers stay collectable, and the way that
 * fails is silent — someone puts an element back in the set and everything
 * still works, for months, while the window grows. So the count that matters
 * is how many entries are elements rather than handles, and the answer has to
 * be zero. */
window.__LEAKCHECK__ = () => ({
  // 셸 하나가 남기는 것들 — 전부 `dropTermView`가 지운다.
  termViews: termViews.size,
  termTitles: termTitles.size,
  termGrids: termGrids.size,
  hookStates: hookStates.size,
  paneModels: paneModels.size,
  hookStamps: hookStamps.size,
  paneSessions: paneSessions.size,
  // 브라우저 판이 마지막으로 들은 자리 — 탭이 갈 때 `dropTab`이 지운다.
  browserSaid: browserSaid.size,
  browserEarly: browserEarly.size,
  acknowledgedPanes: acknowledgedPanes.size,
  paneSubagents: paneSubagents.size,
  // 카드 이름으로 키를 잡는 유일한 지도라, 셸이 닫힐 때 지워 주는 손이 없으면
  // 어떤 것도 그 키를 다시 부르지 않는다 — 세는 이유가 그것이다.
  paneActivities: paneActivities.size,
  paneParents: paneParents.size,
  paneHelpers: paneHelpers.size,
  bellRang: bellRang.size,
  mouseModes: mouseModes.size,
  watchedTerms: watchedTerms.size,
  // 탭·판·무대.
  tabs: tabs.length,
  paneHosts: paneHosts.size,
  paintedViews: paintedViews.size,
  groups: groups.size,
  groupRecents: groupRecents.size,
  editorViews: editorViews.size,
  diffViews: diffViews.size,
  // 워크스페이스와 레인.
  stageTrees: stageTrees.size,
  activeTabByWorktree: activeTabByWorktree.size,
  lanes: lanes.size,
  // 설계상 유계인 것들 — 자라기는 하지만 천장이 있다. 세는 이유는 그 천장이
  // 여전히 있는지 보기 위해서다.
  scrollCache: scrollCache.size,
  contrastLift: contrastLift.size,
  // 보드.
  askDrafts: askDrafts.size,
  agentGraphSelected: Number(agentGraphSelectedKey !== null),
  // 답을 기다리는 물음들. 자라기만 하고 줄지 않으면 철회가 어딘가에서 끊긴
  // 것이고, 그 증상은 "아무도 안 물었는데 대화상자가 서 있다"이다.
  pinnedAsks: pinnedAsks.length,
  permissionQueue: permissionQueue.length,
  // 그리고 말해 둔 문장들.
  spokenNodes: spokenNodes.size,
spokenStrong: [...spokenNodes].filter((held) => !(held instanceof WeakRef)).length,
});

// The registry names builders and painters from every script part. Declare it
// after all parts have run so classic-script function hoisting never has to
// cross a file boundary while the object is being initialized.
const DOC_VIEWS = {
  file: { paint: paintFileView, wire: wireFind },
  diff: { paint: paintDiffView, wire: wireDocFind },
  image: { paint: paintImageView },
  board: { paint: paintBoardView },
  changes: { paint: paintChangesView, wire: wireDocFind },
  imagediff: { paint: paintImageDiffView },
  vault: {},
  browser: { build: buildBrowserView, paint: paintBrowserView },
  emulator: { build: buildEmulatorView, paint: paintEmulatorView },
  mdview: { build: buildMdView, paint: paintMdView },
  csv: { build: buildCsvView, paint: paintCsvView },
  ipynb: { build: buildIpynbView, paint: paintIpynbView },
  worker: { build: buildWorkerView, paint: paintWorkerView },
  knowledge: { build: buildKnowledgeView, paint: paintKnowledgeStage },
  skills: { build: buildSkillsView, paint: paintSkillsStage },
  artifacts: { build: buildArtifactsView, paint: paintArtifactsStage },
  jev: { build: buildJevView, paint: paintJevStage, wire: wireJevView },
  tokens: { build: buildTokensView, paint: paintTokensView, wire: wireTokensView },
};

const DOC_KINDS = Object.keys(DOC_VIEWS);

function initializeDocTemplates() {
  for (const kind of DOC_KINDS) {
    const { build } = DOC_VIEWS[kind];
    docTemplates.set(kind, build ? build() : el(`${kind}-view`));
  }
}

initializeDocTemplates();

// Input handlers are declared in an earlier part, while their initial chord
// map and floating trigger depend on workspace state from another part.
rebuildBound();
paintFloatToggleState();

// Fonts arriving after the first measurement change the cell box, and a pty
// sized with the fallback font truncates the TUI mid-screen. Re-measure on
// the real face and let the children redraw at the true width.
if (document.fonts?.ready) {
  document.fonts.ready.then(() => {
    // The generation is what tells a view its cached metrics were taken under
    // a face that is no longer drawing — `measure` refuses to work twice under
    // the same one, and the stylesheet's answer does not change when the
    // loader's does.
    fontGeneration += 1;
    stageView.measure();
    floatView.measure();
    // And the tab terminals, which this never reached. They are made at
    // runtime, so at first boot there were none to measure and the omission
    // did not show; a shell opened before the faces landed kept the fallback's
    // cell box and asked the pty for a grid that did not fit its screen.
    for (const [term, view] of termViews) {
      view.measure();
      resizeTermTab(term);
    }
    resizeStageLane();
  });
}

(isPopout ? bootPopout() : boot()).catch(showError);
