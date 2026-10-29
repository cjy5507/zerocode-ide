/* ---- 받은 편지함 (t-67483) ---------------------------------------------------
 *
 * 「지금 누가 나를 필요로 하는가」를 한 페이지에: 모든 프로젝트의 질문·넘김·막힘·알림이
 * 분류 탭 하나에 놓이고, 질문은 그 자리에서 답한다. 예전에는 답 하나에 세 화면을 지났다
 * (사이드바 코디 카드 → 작업 상황판 → 답하기 칸). 시안은 사람이 통과시킨 것이다
 * (「받은 편지함 시안」 p-c046c4e765aa0a50, 질문 다섯 모두 추천안).
 *
 * 이 화면은 편지를 고르지도 세지도 않는다. 목록과 수는 Rust의 한 읽기(`board_inbox`:
 * 원장의 데스크 + 판의 상태, `orchestration::inbox`)이고, 탭의 수와 사이드바의 수는 그
 * `counts` 그대로다. 답은 상황판과 같은 원장 길(`desk_reply`), 답 없이 닫기는 원장의
 * `question-close`(`desk_question_close`, t-83281), 통과는 `desk_review_pass`,
 * 알림의 확인은 `desk_ack`(원장은 묶음 하나를 통째로 확인한다)로만 간다. 막힘에서는
 * 허용·거절을 누르지 않는다 — 권한은 그 판의 화면을 보고 눌러야 안전하다(질문 3).
 *
 * 읽음은 창의 기억이다(질문 4): 편지를 열면 점이 같은 프레임에 꺼진다. 질문은 답하면,
 * 넘김은 「확인」을 누르면 원장이 목록에서 뺀다. 알림은 한 묶음의 편지를 모두 읽었을 때
 * 같은 걸음에 그 묶음의 ack가 나간다 — 묶음에 읽지 않은 편지가 남아 있으면 나가지 않는다.
 *
 * 100통의 그림은 3 ms 이내다(질문 5): 편지마다 줄 노드를 키로 두고 다시 쓰며, 줄의 말은
 * 그 줄의 서명(편지·읽음·고름·분·언어)이 바뀐 때만 다시 쓴다. */

/* The tab the page lives in — a document surface like the Jev dashboard, built here
 * rather than declared in index.html (`DOC_VIEWS.inbox`). */
const INBOX_TAB = Object.freeze({ id: "inbox", kind: "inbox" });

/* How long the page waits to be read after the last report. A busy turn reports many
 * times a second; the page is read once per settled beat, not per report. */
const INBOX_REFRESH_MS = 150;

/* The kinds, in the page's order: the word, its glyph (a shape beside the word, so a kind
 * is never told by colour alone) and the tab that holds it. */
const INBOX_KINDS = Object.freeze({
  question: { key: "inbox.tab.question", word: "질문", icon: "i-msg-ask" },
  handoff: { key: "inbox.tab.handoff", word: "넘김", icon: "i-pr-review" },
  blocked: { key: "inbox.tab.blocked", word: "막힘", icon: "i-alert" },
  notice: { key: "inbox.tab.notice", word: "알림", icon: "i-bell" },
});

const INBOX_TABS = Object.freeze(["all", "question", "handoff", "blocked", "notice"]);

/* What a pane that waits says, in the pane's own words (`pane_blocked`, `pane_question`,
 * `pane_finished`). */
const INBOX_PANE_WORDS = Object.freeze({
  pane_blocked: { key: "inbox.pane.blocked", word: "권한 확인을 기다립니다" },
  pane_question: { key: "inbox.pane.question", word: "묻는 말에 답하기를 기다립니다" },
  pane_finished: { key: "inbox.pane.finished", word: "끝났는데 아직 아무도 보지 않았습니다" },
});

const INBOX_NONE = Object.freeze({ items: [], counts: { all: 0, question: 0, handoff: 0, blocked: 0, notice: 0, waiting: 0 } });

/* Where the letters the person opened are kept, so a restart does not light every dot
 * again. Per-viewer convenience only: the page is right without it. */
const INBOX_READ_KEY = "zerocode.inbox.read";
const INBOX_READ_MAX = 500;

let inboxSnapshot = INBOX_NONE;
let inboxSaid = "";
let inboxRefreshTimer = 0;
/* What the person chose: the tab, the switch and the letter shown. Kept across repaints. */
const inboxChoice = { tab: "all", unreadOnly: false, selected: null };
/* The letters opened (`id`s) — unread is the absence of the id. */
const inboxRead = new Set();
/* Written answers and reasons to close without one (per letter), and pressed passes and acks
 * (per key): the draft and the retry name of a send that was not answered, so pressing again
 * replays one receipt. */
const inboxDrafts = new Map();
const inboxCloses = new Map();
const inboxPresses = new Map();
const inboxViewsWired = new WeakSet();
/* A view's row nodes by letter id: kept and re-ordered, never rebuilt. */
const inboxRows = new WeakMap();

try {
  for (const id of JSON.parse(localStorage.getItem(INBOX_READ_KEY) ?? "[]")) inboxRead.add(String(id));
} catch {
  // A private window or cleared site data: every letter reads as unread, which is true enough.
}

function saveInboxRead() {
  try {
    localStorage.setItem(INBOX_READ_KEY, JSON.stringify([...inboxRead].slice(-INBOX_READ_MAX)));
  } catch {
    // Same as above: the page does not depend on remembering.
  }
}

function openInboxView({ tab = null } = {}) {
  if (tab !== null) inboxChoice.tab = tab;
  leavePagesForStage();
  openTab({ ...INBOX_TAB });
}

function scheduleInbox() {
  if (inboxRefreshTimer) return;
  inboxRefreshTimer = setTimeout(() => {
    inboxRefreshTimer = 0;
    void refreshInbox();
  }, INBOX_REFRESH_MS);
}

/* Ask Rust once and repaint what draws the answer. The same answer paints nothing. */
async function refreshInbox() {
  if (isPopout) return;
  let answer = null;
  try {
    answer = await invoke("board_inbox");
  } catch {
    // The page reads the rows as they are now. A failed read shows none, never the rows of
    // the last good read.
  }
  const next = answer && Array.isArray(answer.items) && answer.counts ? answer : INBOX_NONE;
  const said = JSON.stringify(next);
  if (said === inboxSaid) return;
  inboxSaid = said;
  inboxSnapshot = next;
  const standing = new Set(next.items.map((item) => item.id));
  let moved = false;
  for (const id of inboxRead) {
    if (!standing.has(id)) {
      inboxRead.delete(id);
      moved = true;
    }
  }
  for (const drafts of [inboxDrafts, inboxCloses]) {
    for (const id of drafts.keys()) if (!standing.has(id)) drafts.delete(id);
  }
  const deliveries = new Set(next.items.map((item) => inboxBatchKey(item)).filter(Boolean));
  for (const key of inboxPresses.keys()) {
    if (key.startsWith("ack:") ? !deliveries.has(key.slice(4)) : !standing.has(key.slice(key.indexOf(":") + 1))) {
      inboxPresses.delete(key);
    }
  }
  if (moved) saveInboxRead();
  paintInboxBadge();
  paintInboxViews();
}

listen("ledger:changed", () => scheduleInbox());

/* Whether the page holds a finish nobody has looked at for this pane. Releasing a mark
 * costs a round trip, so it is asked only when there is a finish to release. */
function inboxFinishStands(term) {
  return inboxSnapshot.items.some((item) => item.what === "pane_finished" && item.pane?.pane === `term:${term}`);
}

/* The sidebar's number: what waits on the person (Rust's `counts.waiting`). */
function paintInboxBadge() {
  const badge = el("inbox-badge");
  if (!badge) return;
  const waiting = Number(inboxSnapshot.counts?.waiting) || 0;
  writeTextContent(badge, String(waiting));
  writeHidden(badge, waiting === 0);
}

function inboxViews() {
  const views = [];
  for (const tab of tabs) {
    if (tab.kind !== "inbox") continue;
    const host = groupOf(tab.pane).inboxView;
    if (host && !host.hidden) views.push(host);
  }
  return views;
}

function paintInboxViews() {
  for (const view of inboxViews()) paintInboxView(view);
}

function paintInboxStage(tab) {
  paintInboxView(docHost(tab.pane, "inbox"));
  void refreshInbox();
}

/* ---- the view ---- */

function inboxNode(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.append(...children);
  return node;
}

/* The page's fixed words, as catalog rows (`INBOX_KINDS` carries the kinds'). */
const INBOX_LABELS = Object.freeze({
  all: { key: "inbox.tab.all", word: "전체" },
  unreadOnly: { key: "inbox.unreadOnly", word: "안 읽음만" },
  title: { key: "inbox.title", word: "받은 편지함" },
  project: { key: "inbox.fact.project", word: "프로젝트" },
  task: { key: "inbox.fact.task", word: "과업" },
  from: { key: "inbox.fact.from", word: "보낸 이" },
  pick: { key: "inbox.pick", word: "왼쪽에서 편지를 고르면 여기에서 읽고 바로 답합니다." },
});

/* An element whose words are a catalog row's, in the window's language now and re-read by `applyLocale` when
 * the language moves. */
function inboxWord({ key, word }, tag = "span", className = "") {
  const node = inboxNode(tag, className);
  node.dataset.i18n = key;
  node.textContent = t(key, word);
  return node;
}

function inboxGlyph(icon) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#${icon}`);
  svg.append(use);
  return svg;
}

function inboxButton(act, className = "btn") {
  const button = inboxNode("button", className);
  button.type = "button";
  button.dataset.inboxAct = act;
  return button;
}

function buildInboxView() {
  const root = document.createElement("section");
  root.className = "file-view inbox-view";
  // The first leaf's copy answers to this name, exactly as the surfaces the markup declares
  // do — `docHost` strips it from every clone after.
  root.id = "inbox-view";
  root.hidden = true;
  root.dataset.i18nAria = "inbox.title";
  root.setAttribute("aria-label", t("inbox.title", "받은 편지함"));

  const tabsGroup = inboxNode("div", "board-mode inbox-tabs");
  tabsGroup.setAttribute("role", "group");
  tabsGroup.setAttribute("aria-label", t("inbox.tabs.label", "분류"));
  tabsGroup.dataset.i18nAria = "inbox.tabs.label";
  for (const tab of INBOX_TABS) {
    const kind = INBOX_KINDS[tab];
    const button = inboxNode("button", "inbox-tab",
      inboxWord(kind ?? INBOX_LABELS.all, "span", "inbox-tab-word"),
      inboxNode("span", "inbox-tab-count"));
    button.type = "button";
    button.dataset.inboxTab = tab;
    tabsGroup.append(button);
  }
  const unread = inboxNode("label", "inbox-unread");
  const unreadSwitch = inboxNode("button", "auto-row-toggle inbox-unread-switch");
  unreadSwitch.type = "button";
  unreadSwitch.setAttribute("role", "switch");
  unreadSwitch.setAttribute("aria-label", t("inbox.unreadOnly", "안 읽음만"));
  unreadSwitch.dataset.i18nAria = "inbox.unreadOnly";
  unread.append(unreadSwitch, inboxWord(INBOX_LABELS.unreadOnly));

  const head = inboxNode("header", "inbox-head",
    inboxNode("div", "inbox-heading", inboxWord(INBOX_LABELS.title, "h1", "inbox-title"), inboxNode("p", "inbox-sum")),
    inboxNode("div", "inbox-tools", tabsGroup, unread));

  const list = inboxNode("ol", "inbox-list");
  // A row opens its letter, and a read leaves the ledger's ack (`ackInboxBatch`): an agent pressing in the
  // window must not read for the person, so the rows carry the mark the desk's whole panel carries (t-74825).
  list.dataset.personOnly = "";
  list.setAttribute("role", "listbox");
  list.setAttribute("aria-label", t("inbox.list.label", "편지 목록"));
  list.dataset.i18nAria = "inbox.list.label";

  const kind = inboxNode("div", "inbox-pv-kind", inboxNode("span", "inbox-kind"), inboxNode("span", "inbox-age"));
  const facts = inboxNode("dl", "inbox-facts",
    inboxWord(INBOX_LABELS.project, "dt"), inboxNode("dd", "inbox-fact-project"),
    inboxWord(INBOX_LABELS.task, "dt"), inboxNode("dd", "inbox-fact-task"),
    inboxWord(INBOX_LABELS.from, "dt"), inboxNode("dd", "inbox-fact-from"));
  const field = inboxNode("textarea", "settings-input inbox-reply-field");
  field.rows = 4;
  field.setAttribute("aria-label", t("inbox.reply.label", "답장"));
  field.placeholder = t("inbox.reply.placeholder", "답을 적습니다. 보내면 워커가 바로 이어 갑니다.");
  field.dataset.i18nAria = "inbox.reply.label";
  field.dataset.i18nPlaceholder = "inbox.reply.placeholder";
  const reply = inboxNode("div", "inbox-reply", field, inboxNode("p", "inbox-error"));
  const actions = inboxNode("div", "inbox-actions",
    inboxButton("reply", "btn btn--primary"), inboxButton("pass", "btn btn--primary"),
    inboxButton("ack", "btn btn--primary"), inboxButton("open-pane"), inboxButton("open-board"));
  // A question nobody will answer closes without an answer, on one line of why (t-83281).
  const reason = inboxNode("input", "settings-input inbox-close-field");
  reason.type = "text";
  reason.setAttribute("aria-label", t("inbox.close.label", "닫는 까닭"));
  reason.placeholder = t("inbox.close.placeholder", "답 없이 닫는 까닭을 한 줄로 적습니다");
  reason.dataset.i18nAria = "inbox.close.label";
  reason.dataset.i18nPlaceholder = "inbox.close.placeholder";
  const close = inboxNode("div", "inbox-close",
    inboxNode("div", "inbox-close-row", reason, inboxButton("close")), inboxNode("p", "inbox-error"));
  // The answer and its buttons are the person's, as on the desk they repeat (t-73906): an agent pressing in the
  // window reads the letters but sends, closes, passes and acknowledges nothing.
  reply.dataset.personOnly = "";
  actions.dataset.personOnly = "";
  close.dataset.personOnly = "";
  const detail = inboxNode("div", "inbox-detail", kind, inboxNode("h2", "inbox-pv-title"), facts,
    inboxNode("p", "inbox-pv-body"), reply, actions, close, inboxNode("p", "inbox-note"));
  const preview = inboxNode("article", "inbox-preview", inboxWord(INBOX_LABELS.pick, "p", "inbox-empty"), detail);
  preview.setAttribute("aria-live", "polite");

  root.append(head, inboxNode("div", "inbox-body", list, preview));
  return root;
}

function wireInboxView(host) {
  if (inboxViewsWired.has(host)) return;
  inboxViewsWired.add(host);
  host.querySelector(".inbox-tabs").addEventListener("click", (event) => {
    const button = event.target.closest("[data-inbox-tab]");
    if (!button) return;
    inboxChoice.tab = button.dataset.inboxTab;
    paintInboxViews();
  });
  host.querySelector(".inbox-unread-switch").addEventListener("click", () => {
    inboxChoice.unreadOnly = !inboxChoice.unreadOnly;
    paintInboxViews();
  });
  const list = host.querySelector(".inbox-list");
  list.addEventListener("click", (event) => {
    const row = event.target.closest(".inbox-row");
    const item = row && inboxSnapshot.items.find((one) => one.id === row.dataset.id);
    if (item) openInboxItem(item);
  });
  // Arrow keys walk the list; Enter and Space on a row are its click.
  list.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const rows = [...list.querySelectorAll(".inbox-row")];
    const at = rows.indexOf(document.activeElement);
    const next = rows[at + (event.key === "ArrowDown" ? 1 : -1)];
    if (!next) return;
    event.preventDefault();
    next.focus();
  });
  const preview = host.querySelector(".inbox-preview");
  const field = preview.querySelector(".inbox-reply-field");
  field.addEventListener("input", () => {
    const draft = inboxDraft(inboxChoice.selected);
    if (draft) draft.text = field.value;
  });
  field.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      sendInboxReply(inboxSelectedItem());
    }
  });
  const reason = preview.querySelector(".inbox-close-field");
  reason.addEventListener("input", () => {
    const closing = inboxDraft(inboxChoice.selected, inboxCloses);
    if (!closing) return;
    closing.text = reason.value;
    if (closing.error) {
      closing.error = "";
      paintInboxViews();
    }
  });
  reason.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.isComposing) return;
    event.preventDefault();
    void closeInboxQuestion(inboxSelectedItem());
  });
  preview.addEventListener("click", (event) => {
    const button = event.target.closest("[data-inbox-act]");
    const item = button && inboxSelectedItem();
    if (!item) return;
    const act = button.dataset.inboxAct;
    if (act === "reply") void sendInboxReply(item);
    else if (act === "close") void closeInboxQuestion(item);
    else if (act === "pass") void passInboxHandoff(item);
    else if (act === "ack") void ackInboxBatch(item, true);
    else if (act === "open-pane") openInboxPane(item);
    else if (act === "open-board") openBoard();
  });
}

/* ---- what a letter says ---- */

function inboxSelectedItem() {
  return inboxSnapshot.items.find((item) => item.id === inboxChoice.selected) ?? null;
}

/* The words written for a letter — an answer in `inboxDrafts`, a reason to close in `inboxCloses` —
 * and the state of their one send. */
function inboxDraft(id, drafts = inboxDrafts) {
  if (id === null) return null;
  let draft = drafts.get(id);
  if (!draft) {
    draft = { text: "", sending: false, sent: false, error: "", retry: null };
    drafts.set(id, draft);
  }
  return draft;
}

/* The reason a question was closed with from this page, once the ledger took the close. */
function inboxClosedWith(item) {
  const closing = item.what === "question" ? inboxCloses.get(item.id) : null;
  return closing?.sent ? closing.text.trim() : null;
}

/* The ledger's batch a letter rides in, the unit an ack names: `run/delivery`. */
function inboxBatchKey(item) {
  return item.letter?.delivery === "delivered" && item.letter.delivery_id ? `${item.run}/${item.letter.delivery_id}` : "";
}

function inboxTitle(item) {
  const letter = item.letter;
  const closedWith = inboxClosedWith(item);
  if (closedWith !== null) return t("inbox.closed", "닫힘 · {{reason}}", { reason: closedWith });
  if (item.what === "question") return String(letter.body ?? "").split("\n")[0];
  if (item.what === "handoff") return boardReviewWords(item.task);
  if (item.what === "landing") return t("inbox.landing", "착지 대기 — 검증을 마친 일 {{count}}건", { count: item.landing });
  const pane = INBOX_PANE_WORDS[item.what];
  if (pane) return t(pane.key, pane.word);
  const kind = DESK_MAIL[item.what];
  return kind ? t(kind.key, kind.word) : String(item.what);
}

function inboxBody(item, now) {
  if (item.what === "question") return String(item.letter.body ?? "");
  if (item.what === "handoff") {
    return [deskTaskWorkWords(item.task.progress), deskHandInWords(item.task.hand_in)].filter(Boolean).join(" · ");
  }
  if (item.pane) return item.pane.worktree;
  return item.letter ? deskLetterDetail(item.letter, now) : "";
}

function inboxTaskName(item) {
  const name = item.letter?.task ?? item.task?.title ?? "";
  const id = item.letter?.task_id ?? item.task?.id ?? "";
  return [name, id].filter(Boolean).join(" ");
}

function inboxFrom(item) {
  if (item.pane) return item.pane.agent || "Agent";
  return item.letter?.worker ?? "";
}

function inboxUnread(item) {
  return !inboxRead.has(item.id);
}

/* ---- the paint ---- */

function inboxVisible() {
  return inboxSnapshot.items.filter((item) =>
    (inboxChoice.tab === "all" || item.tab === inboxChoice.tab) && (!inboxChoice.unreadOnly || inboxUnread(item)));
}

function inboxRowNode(view, item) {
  let held = inboxRows.get(view);
  if (!held) {
    held = new Map();
    inboxRows.set(view, held);
  }
  let li = held.get(item.id);
  if (!li) {
    const kind = inboxNode("span", "inbox-kind", inboxGlyph(INBOX_KINDS[item.tab].icon), inboxNode("span", "inbox-kind-word"));
    const row = inboxNode("button", "inbox-row",
      inboxNode("span", "inbox-dot"),
      inboxNode("div", "inbox-row-body",
        inboxNode("div", "inbox-row-top", kind, inboxNode("span", "inbox-row-title"), inboxNode("span", "inbox-age")),
        inboxNode("div", "inbox-row-meta")));
    row.type = "button";
    row.setAttribute("role", "option");
    row.dataset.id = item.id;
    row.dataset.kind = item.tab;
    li = inboxNode("li", "inbox-item", row);
    held.set(item.id, li);
  }
  return li;
}

function paintInboxRow(li, item, selected, now, minute) {
  const row = li.firstElementChild;
  const unread = inboxUnread(item);
  writeAttribute(row, "aria-selected", String(selected));
  writeAttribute(row, "data-unread", String(unread));
  const dot = row.firstElementChild;
  writeAttribute(dot, "aria-label", unread ? t("inbox.unread", "안 읽음") : t("inbox.read", "읽음"));
  // The words change with the letter, the minute and the language, not with a click.
  const signature = `${item.at_ms}|${item.letter?.notices ?? ""}|${unread}|${inboxClosedWith(item) ?? ""}|${minute}|${document.documentElement.lang}`;
  if (row.dataset.signature === signature) return;
  row.dataset.signature = signature;
  const kind = INBOX_KINDS[item.tab];
  writeTextContent(row.querySelector(".inbox-kind-word"), t(kind.key, kind.word));
  writeTextContent(row.querySelector(".inbox-row-title"), inboxTitle(item));
  writeTextContent(row.querySelector(".inbox-age"), item.at_ms > 0 ? inboxAge(item.at_ms, now) : "");
  writeTextContent(row.querySelector(".inbox-row-meta"), [item.project, inboxTaskName(item), inboxFrom(item)].filter(Boolean).join(" · "));
}

function inboxAge(at, now) {
  return t("board.desk.mailAge", "{{time}} 전", { time: agoWord(at, now) });
}

function paintInboxView(view) {
  const counts = inboxSnapshot.counts;
  const now = Date.now();
  const minute = Math.floor(now / 60_000);
  writeTextContent(view.querySelector(".inbox-sum"),
    t("inbox.sum", "지금 당신을 기다리는 일 {{count}}개 · 알림 {{notices}}개", { count: counts.waiting, notices: counts.notice }));
  for (const button of view.querySelectorAll(".inbox-tab")) {
    const tab = button.dataset.inboxTab;
    writeAttribute(button, "aria-pressed", String(tab === inboxChoice.tab));
    writeTextContent(button.querySelector(".inbox-tab-count"), String(counts[tab] ?? 0));
  }
  writeAttribute(view.querySelector(".inbox-unread-switch"), "aria-checked", String(inboxChoice.unreadOnly));

  const items = inboxVisible();
  if (!items.some((item) => item.id === inboxChoice.selected)) inboxChoice.selected = items[0]?.id ?? null;
  const list = view.querySelector(".inbox-list");
  const nodes = items.map((item) => {
    const li = inboxRowNode(view, item);
    paintInboxRow(li, item, item.id === inboxChoice.selected, now, minute);
    return li;
  });
  if (nodes.length === 0) {
    const none = inboxNode("li", "inbox-empty");
    none.textContent = counts.all === 0
      ? t("inbox.empty", "받은 편지함이 비었습니다 — 지금 당신을 기다리는 것이 없습니다.")
      : t("inbox.emptyUnread", "안 읽은 편지가 없습니다.");
    nodes.push(none);
  }
  reconcileElementOrder(list, nodes);
  const standing = new Set(inboxSnapshot.items.map((item) => item.id));
  const held = inboxRows.get(view);
  if (held) for (const id of held.keys()) if (!standing.has(id)) held.delete(id);
  paintInboxPreview(view, inboxSelectedItem(), now);
}

function inboxPressed(key) {
  return inboxPresses.get(key) ?? { sending: false, done: false, error: "", retry: null, said: "" };
}

function paintInboxPreview(view, item, now) {
  const preview = view.querySelector(".inbox-preview");
  const detail = preview.querySelector(".inbox-detail");
  writeHidden(preview.querySelector(".inbox-empty"), item !== null);
  writeHidden(detail, item === null);
  if (item === null) return;
  const kind = INBOX_KINDS[item.tab];
  const chip = detail.querySelector(".inbox-pv-kind .inbox-kind");
  if (!chip.firstElementChild) chip.append(inboxGlyph(kind.icon), inboxNode("span", "inbox-kind-word"));
  chip.dataset.kind = item.tab;
  chip.firstElementChild.firstElementChild?.setAttribute("href", `#${kind.icon}`);
  writeTextContent(chip.querySelector(".inbox-kind-word"), t(kind.key, kind.word));
  writeTextContent(detail.querySelector(".inbox-pv-kind .inbox-age"), item.at_ms > 0 ? inboxAge(item.at_ms, now) : "");
  writeTextContent(detail.querySelector(".inbox-pv-title"), inboxTitle(item));
  // A fact the letter does not carry (a pane has no task) takes no line.
  for (const [name, value] of [["project", item.project], ["task", inboxTaskName(item)], ["from", inboxFrom(item)]]) {
    const fact = detail.querySelector(`.inbox-fact-${name}`);
    writeTextContent(fact, value);
    writeHidden(fact, value === "");
    writeHidden(fact.previousElementSibling, value === "");
  }
  const body = inboxBody(item, now);
  const bodyNode = detail.querySelector(".inbox-pv-body");
  writeTextContent(bodyNode, body);
  writeHidden(bodyNode, body === "");

  // What each kind may do from here. Everything goes through the ledger's own doors, as the
  // seat of the run, and only where this window holds that seat.
  const seated = item.seat === true && !isPopout;
  const closedWith = inboxClosedWith(item);
  const canReply = item.what === "question" && seated && closedWith === null;
  // A hand-in whose verification the coordinator withdrew takes a new review, not a pass (t-81913).
  const withdrawn = item.what === "handoff" && boardReviewWithdrawn(item.task) !== null;
  const canPass = item.what === "handoff" && seated && !withdrawn
    && Boolean(item.task.progress?.attempt) && Boolean(item.task.progress?.source);
  const canAck = Boolean(item.letter) && item.what !== "question" && seated && inboxBatchKey(item) !== "";
  const draft = canReply ? inboxDraft(item.id) : null;
  // A question being answered, or answered, is not closed beside it.
  const canClose = canReply && !draft.sending && !draft.sent;
  const closing = canClose ? inboxDraft(item.id, inboxCloses) : null;
  const pass = inboxPressed(`pass:${item.id}`);
  const ack = inboxPressed(`ack:${inboxBatchKey(item)}`);

  const reply = detail.querySelector(".inbox-reply");
  writeHidden(reply, !canReply);
  const field = reply.querySelector(".inbox-reply-field");
  if (canReply) {
    if (field.value !== draft.text && document.activeElement !== field) field.value = draft.text;
    writeDisabled(field, draft.sending || Boolean(closing?.sending));
  }
  const error = reply.querySelector(".inbox-error");
  writeTextContent(error, draft?.error ?? "");
  writeHidden(error, !draft?.error);

  const act = (name) => detail.querySelector(`[data-inbox-act="${name}"]`);
  const close = detail.querySelector(".inbox-close");
  writeHidden(close, !canClose);
  const reason = close.querySelector(".inbox-close-field");
  if (canClose) {
    if (reason.value !== closing.text && document.activeElement !== reason) reason.value = closing.text;
    writeDisabled(reason, closing.sending);
  }
  const closeError = close.querySelector(".inbox-error");
  writeTextContent(closeError, closing?.error ?? "");
  writeHidden(closeError, !closing?.error);
  const closeButton = act("close");
  writeDisabled(closeButton, Boolean(closing?.sending));
  writeTextContent(closeButton, closing?.sending ? t("inbox.close.sending", "닫는 중…") : t("inbox.close.send", "답 없이 닫기"));
  const replyButton = act("reply");
  writeHidden(replyButton, !canReply || draft.sent);
  writeDisabled(replyButton, Boolean(draft?.sending || closing?.sending));
  writeTextContent(replyButton, draft?.sending ? t("board.desk.replySending", "보내는 중…") : t("board.desk.replySend", "답 보내기"));
  const passButton = act("pass");
  writeHidden(passButton, !canPass || pass.done);
  writeDisabled(passButton, pass.sending);
  writeTextContent(passButton, pass.sending ? t("board.desk.acking", "확인하는 중…") : t("inbox.confirm", "확인"));
  const ackButton = act("ack");
  writeHidden(ackButton, !canAck || ack.done);
  writeDisabled(ackButton, ack.sending);
  writeTextContent(ackButton, ack.sending ? t("board.desk.acking", "확인하는 중…")
    : t("board.desk.ack", "확인 · 이 묶음 {{count}}통", { count: item.letter?.batch ?? 0 }));
  const openPane = act("open-pane");
  writeHidden(openPane, !item.pane);
  writeTextContent(openPane, t("inbox.openPane", "판 열기"));
  const openBoardButton = act("open-board");
  writeHidden(openBoardButton, Boolean(item.pane));
  writeTextContent(openBoardButton, t("inbox.openBoard", "상황판에서 열기"));

  const note = detail.querySelector(".inbox-note");
  let said = "";
  if (draft?.sent) said = t("board.desk.replySent", "답을 보냄 · 원장에 적히면 목록에서 빠져요");
  else if (closedWith !== null) said = t("inbox.note.closed", "답 없이 닫음 · 원장에 적히면 목록에서 빠져요");
  else if (pass.done) said = pass.said;
  else if (pass.error) said = pass.error;
  else if (ack.done) said = t("board.desk.ackSent", "묶음을 확인함 · 원장에 적히면 목록에서 빠져요");
  else if (ack.error) said = ack.error;
  else if (item.run && !seated && item.what !== "landing") {
    said = t("board.desk.noSeat", "이 창에 그 런의 코디네이터 자리가 없어 여기서는 답할 수 없어요");
  } else if (item.what === "question") said = t("inbox.note.question", "답하기 전까지 이 워커는 멈춰 있습니다.");
  else if (withdrawn) said = t("inbox.note.handoffWithdrawn", "새 검수 결과를 적거나 새로 넘겨야 다시 통과됩니다.");
  else if (item.what === "handoff") said = t("inbox.note.handoff", "확인하면 검증 기록이 원장에 적힙니다.");
  else if (item.tab === "blocked") said = t("inbox.note.blocked", "허용·거절은 그 판에서만 누릅니다. 여기서는 판을 엽니다.");
  writeTextContent(note, said);
  writeHidden(note, said === "");
}

/* ---- the hands ---- */

/* Opening a letter reads it: the dot is off in this frame, a finish is released, and the
 * ledger's ack leaves when the last letter of its batch has been read. */
function openInboxItem(item) {
  inboxChoice.selected = item.id;
  if (inboxUnread(item)) {
    inboxRead.add(item.id);
    saveInboxRead();
    if (item.what === "pane_finished") {
      void invoke("clear_finish_mark", { pane: item.pane.pane }).catch(() => false).then(() => refreshInbox());
    }
    void ackInboxBatch(item, false);
  }
  paintInboxViews();
}

/* The ledger acknowledges a batch, never one letter of it. A read does it only when every
 * letter of the batch is a read letter of this page; the 「확인」 button (`pressed`) names the
 * whole batch and its count before it is pressed. */
async function ackInboxBatch(item, pressed) {
  const key = inboxBatchKey(item);
  if (key === "" || item.seat !== true || isPopout) return;
  const batch = inboxSnapshot.items.filter((one) => inboxBatchKey(one) === key);
  if (!pressed) {
    const whole = batch.length === item.letter.batch && batch.every((one) => one.what !== "question" && !inboxUnread(one));
    if (!whole) return;
  }
  const held = inboxPressed(`ack:${key}`);
  if (held.sending || held.done) return;
  held.retry ??= { id: `ui-inbox-ack-${crypto.randomUUID()}` };
  held.sending = true;
  held.error = "";
  inboxPresses.set(`ack:${key}`, held);
  if (pressed) {
    for (const one of batch) inboxRead.add(one.id);
    saveInboxRead();
  }
  paintInboxViews();
  try {
    await invoke("desk_ack", { run: item.run, delivery: item.letter.delivery_id, retryRequest: held.retry.id }, { hand: handNow() });
    held.done = true;
    held.retry = null;
    void refreshInbox();
  } catch (error) {
    held.error = String(error);
  } finally {
    held.sending = false;
    paintInboxViews();
  }
}

async function sendInboxReply(item) {
  const draft = item && inboxDraft(item.id);
  const body = draft?.text.trim() ?? "";
  if (!draft || draft.sending || body === "") return;
  const signature = JSON.stringify([item.run, item.letter.id, body]);
  if (draft.retry?.signature !== signature) draft.retry = { signature, id: `ui-inbox-reply-${crypto.randomUUID()}` };
  draft.sending = true;
  draft.error = "";
  inboxRead.add(item.id);
  paintInboxViews();
  try {
    await invoke("desk_reply", { run: item.run, message: item.letter.id, body, retryRequest: draft.retry.id });
    draft.sent = true;
    draft.retry = null;
    void refreshInbox();
  } catch (error) {
    // The name stays: pressing again replays the one receipt rather than writing a second answer.
    draft.error = String(error);
  } finally {
    draft.sending = false;
    paintInboxViews();
  }
}

/* Close a question without an answer, as the run's seat — the ledger's `question-close`, on the
 * person's one line of why. A question the ledger already holds closed (answered, or closed from the
 * other side) answers `already` and reads the same: closed, with the words given here. */
async function closeInboxQuestion(item) {
  const closing = item?.what === "question" ? inboxDraft(item.id, inboxCloses) : null;
  if (!closing || closing.sending || closing.sent || item.seat !== true || isPopout) return;
  const reason = closing.text.trim();
  if (reason === "") {
    closing.error = t("inbox.close.needsReason", "닫는 까닭을 한 줄 적어야 닫힙니다.");
    paintInboxViews();
    return;
  }
  const signature = JSON.stringify([item.run, item.letter.id, reason]);
  if (closing.retry?.signature !== signature) closing.retry = { signature, id: `ui-inbox-close-${crypto.randomUUID()}` };
  closing.sending = true;
  closing.error = "";
  inboxRead.add(item.id);
  paintInboxViews();
  try {
    await invoke("desk_question_close", { run: item.run, message: item.letter.id, reason, retryRequest: closing.retry.id }, { hand: handNow() });
    closing.sent = true;
    closing.retry = null;
    void refreshInbox();
  } catch (error) {
    // The name stays: pressing again replays the one receipt rather than closing twice.
    closing.error = String(error);
  } finally {
    closing.sending = false;
    paintInboxViews();
  }
}

async function passInboxHandoff(item) {
  const key = `pass:${item.id}`;
  const held = inboxPressed(key);
  const { attempt, source } = item.task.progress ?? {};
  if (held.sending || held.done || !attempt || !source || boardReviewWithdrawn(item.task) !== null) return;
  const args = { run: item.task.run, task: item.task.id, attempt, source };
  if (held.retry?.signature !== JSON.stringify(args)) {
    held.retry = { signature: JSON.stringify(args), id: `ui-inbox-pass-${crypto.randomUUID()}` };
  }
  held.sending = true;
  held.error = "";
  inboxPresses.set(key, held);
  inboxRead.add(item.id);
  paintInboxViews();
  try {
    const answer = await invoke("desk_review_pass", { ...args, retryRequest: held.retry.id });
    held.said = t("board.now.reviewSent", "통과를 적음 · 원장에 적히면 줄에서 빠져요 {{receipt}}", { receipt: boardReceipt(answer ?? {}) }).trim();
    held.done = true;
    held.retry = null;
    void refreshInbox();
  } catch (error) {
    held.error = String(error);
  } finally {
    held.sending = false;
    paintInboxViews();
  }
}

/* A pane that waits is opened the way a board card is; looking releases a finish's mark. */
function openInboxPane(item) {
  const bucket = item.what === "pane_finished" ? "done" : "attention";
  openBoardCard({ pane: item.pane.pane, agent: item.pane.agent, worktree: item.pane.worktree }, bucket);
}

el("nav-inbox")?.addEventListener("click", () => openInboxView());
