/* ---- the conversation view: how a row folds (t-6323) -----------------------
 *
 * The Claude Code extension keeps every long thing in its panel to a few
 * lines and lets a press unfold it (2.1.280 webview, docs/design/agent-
 * conversation-claude-code-grammar-20260915.md §10): a tool body's row stops
 * at 60px behind a fade (`toolBodyRowContent`), a diff at 200px
 * (`Math.min(200, contentHeight + 20)`, "Click to expand"), and what the
 * person said at 60px with "Show more" / "Show less" (`EV0`, `maxHeight:
 * 60`). The heights are tokens (`--chat-tool-clip-h`, `--chat-diff-clip-h`,
 * `--chat-user-clip-h`), so the page and the gate read one number each.
 *
 * One grammar for the three: the thing that is cut wears `is-clipped`, and
 * its door stands right after it — 「더 보기」 while it is cut, 「접기」 while
 * it is open (`is-open`). Focus view folds a TURN behind one summary; these
 * fold a ROW, and the two never touch the same node. */

/* When a tool's words are long — the extension's own test (`cN`): more than
 * three lines, or more than 250 characters. The only rule here that is words
 * rather than pixels; a body that is long by it is cut at the clip and wears
 * its door, a shorter one stands whole. */
const CHAT_CLIP = Object.freeze({ lines: 3, chars: 250 });

function chatClips(text) {
  if (text.length > CHAT_CLIP.chars) return true;
  let lines = 1;
  for (let at = text.indexOf("\n"); at >= 0; at = text.indexOf("\n", at + 1)) {
    lines += 1;
    if (lines > CHAT_CLIP.lines) return true;
  }
  return false;
}

/* The door after a clipped thing. `build` runs once, before the first open:
 * a diff's rows past the clip are built then, not before. */
function expandDoorNode(target, build = null) {
  const door = document.createElement("button");
  door.type = "button";
  door.className = "helper-expand";
  paintExpandDoor(door, false);
  door.addEventListener("click", (event) => {
    // A person's words stuck at the top are a door home themselves
    // (`helperTurnsNode`); a press on THIS door is not that press.
    event.stopPropagation();
    const open = !target.classList.contains("is-open");
    if (open && build) {
      build();
      build = null;
    }
    target.classList.toggle("is-open", open);
    paintExpandDoor(door, open);
  });
  return door;
}

function paintExpandDoor(door, open) {
  writeTextContent(door, open ? t("worker.showLess", "접기") : t("worker.showMore", "더 보기"));
  writeAttribute(door, "aria-expanded", open ? "true" : "false");
}

/* Cut `node` with its door after it, or stand it whole — guarded, so a poll
 * that brings nothing writes nothing. */
function clipWith(node, clipped, build = null) {
  writeClass(node, "is-clipped", clipped);
  const door = node.nextElementSibling?.classList.contains("helper-expand") ? node.nextElementSibling : null;
  if (clipped && !door) node.after(expandDoorNode(node, build));
  else if (!clipped && door) door.remove();
}

/* ---- the tool body ---------------------------------------------------------
 *
 * The extension's `toolBody`: one bordered box, a row per side — IN what the
 * call took, OUT what came back — each with its label in the mono column and
 * its own copy over its corner. It stands in the step's body (`paintStepBody`)
 * once the step is opened; a side with nothing stands hidden. Built once,
 * when the row first has something to put in it. */
function toolBodyNode() {
  const body = document.createElement("div");
  body.className = "helper-tool-body";
  for (const kind of ["input", "output"]) {
    const well = document.createElement("div");
    well.className = "helper-tool-well";
    well.hidden = true;
    const words = document.createElement("pre");
    words.className = `helper-fold-body helper-tool-${kind}`;
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "helper-code-copy";
    copy.appendChild(iconNode("copy"));
    labelButton(copy, kind === "input" ? t("worker.copyCode", "코드 복사") : t("worker.copyOutput", "출력 복사"));
    copyOnPress(copy, () => words.textContent);
    well.append(words, copy);
    body.appendChild(well);
  }
  return body;
}

/* The body's two sides from the words they carry (`""` hides a side), in
 * `row`'s own body — a step's, when it is opened. */
function dressToolBody(row, input, output) {
  let body = row.querySelector(":scope > .helper-tool-body");
  if (!input && !output) return;
  if (!body) {
    body = toolBodyNode();
    row.appendChild(body);
  }
  for (const [well, text] of [[body.children[0], input], [body.children[1], output]]) {
    writeHidden(well, text === "");
    const words = well.firstElementChild;
    writeTextContent(words, text);
    clipWith(words, text !== "" && chatClips(text));
  }
}

/* ---- the diff --------------------------------------------------------------
 *
 * How many of a diff's rows its clip shows: the clip's height over one row's
 * (the diff's type size times its leading), and one more under the fade —
 * read from the tokens the first time a diff is drawn. The rest of a long
 * diff is built when its door first opens it: a page of sixty edits keeps
 * the rows a person can see, not the thousands behind the clips. */
let diffClipRowsHeld = 0;

function diffClipRows() {
  if (diffClipRowsHeld > 0) return diffClipRowsHeld;
  const root = getComputedStyle(document.documentElement);
  const clip = parseFloat(root.getPropertyValue("--chat-diff-clip-h"));
  const row = parseFloat(root.getPropertyValue("--text-xs")) * parseFloat(root.getPropertyValue("--chat-diff-leading"));
  // Tokens that cannot be read cut nothing: every row stands.
  diffClipRowsHeld = clip > 0 && row > 0 ? Math.ceil(clip / row) + 1 : Number.POSITIVE_INFINITY;
  return diffClipRowsHeld;
}

/* One file's rows, cut at the clip. `words` are the review surface's word
 * marks for the whole edit (`diffWordSpans`), indexed by row. */
function diffRowsNode(edit, words) {
  const rows = document.createElement("div");
  rows.className = "helper-tool-diff-rows";
  const shown = Math.min(edit.lines.length, diffClipRows());
  const build = (from, to) => {
    for (let at = from; at < to; at += 1) rows.appendChild(diffLineNode(edit.lines[at], words.get(at)));
    if (to === edit.lines.length && edit.truncated > 0) {
      const more = document.createElement("div");
      more.className = "diff-line diff-line--meta";
      more.textContent = t("worker.moreLines", "{{n}}줄 더", { n: edit.truncated });
      rows.appendChild(more);
    }
  };
  build(0, shown);
  return { rows, rest: shown < edit.lines.length ? () => build(shown, edit.lines.length) : null };
}

/* ---- what the person said ---------------------------------------------------
 *
 * The extension measures a message (`scrollHeight > maxHeight`) and cuts one
 * taller than the clip. Measured here too, in one pass after the rows of a
 * paint stand: every read first, then every write — a paint that brought ten
 * long messages lays the list out once for them, not ten times. How tall a
 * message stands depends on the pane's width, so none is judged by its
 * length alone. */
function clipPersonRows(rows) {
  const tall = [];
  for (const row of rows) {
    const said = row.querySelector(":scope > .helper-said");
    if (!said) continue;
    const style = getComputedStyle(said);
    const words = said.scrollHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
    if (words > parseFloat(style.getPropertyValue("--chat-user-clip-h")) + 1) tall.push(said);
  }
  for (const said of tall) clipWith(said, true);
}

/* ---- a tool's file is a door (t-6323 A2) -----------------------------------
 *
 * The extension's tool header links the file a call read or wrote
 * (`fileToolHeader`, 2.1.280): a read opens where it began and says which
 * lines it read, an edit where its new text stands, a write at its top. The
 * call's file is the backend's reading (`tool.file`, `transcript::file_in` —
 * only reads, edits and writes name one); where a read began is the CLI's own
 * count (`read_offset_base` on its catalog row), and a CLI whose count was
 * not measured opens at the top rather than at a guessed line. The door is
 * the document viewer's (`openPath`), measured from the session's checkout
 * as a link in the answer is (`helperBase`). */
function toolFilePlace(file, agent) {
  const base = installedAgents().find((row) => row.id === agent)?.read_offset_base;
  if (typeof file.offset !== "number" || typeof base !== "number") return {};
  const line = file.offset + 1 - base;
  return { line, end: typeof file.limit === "number" ? line + file.limit - 1 : undefined };
}

/* The file the call read or wrote, a door to the file tab, once — at the top
 * of the step's body, where a person who opened the row can press it: the
 * line is one summary, and a summary holds no controls. A read's lines stand
 * after it in the extension's words: 「(11–60행)」 / 「(11행부터)」. */
function dressToolFile(row, turn, run) {
  const file = turn.tool?.file;
  const body = row.querySelector(":scope > .helper-step-body");
  if (!file?.path || !body || row.__fileDoor) return;
  row.__fileDoor = true;
  const line = document.createElement("p");
  line.className = "helper-tool-call helper-step-file";
  const arg = document.createElement("span");
  arg.className = "helper-tool-arg is-door";
  arg.textContent = file.path;
  const place = toolFilePlace(file, run.agent);
  actsAsButton(arg, (event) => {
    event.stopPropagation();
    openPath(resolveDocPath(helperBase(run), file.path), {
      preview: true,
      line: place.line,
      search: place.line === undefined ? file.search : undefined,
    });
  });
  line.appendChild(arg);
  if (place.line !== undefined) {
    const where = document.createElement("span");
    where.className = "helper-tool-where";
    where.textContent = place.end !== undefined
      ? t("worker.readLines", "({{from}}–{{to}}행)", { from: place.line, to: place.end })
      : t("worker.readFrom", "({{from}}행부터)", { from: place.line });
    line.appendChild(where);
  }
  body.prepend(line);
}

/* ---- the list keeps to its foot until the person leaves it (t-6323 A5) -----
 *
 * The extension's list (2.1.280 `VG0`, `lF1`): standing within 50px of its
 * foot (`TF`) and not scrolled away, it follows the words as they come.
 * Leaving is an intent, not a distance — an upward wheel, touch or key
 * (ArrowUp, PageUp, Home, Shift+Space) leaves at once however near the foot
 * the list stood, and so does a move up the page did not cause (the thumb
 * dragged); a move back down into the last 50px comes back. A wheel or a
 * touch that a box inside the list can still scroll (a tool's well, a diff,
 * a code block) is that box's. Sending comes back as well and glides home
 * (`scrollToBottomOnSend`, on by default), and while the glide is young
 * (`g25`) new words keep it gliding. The numbers, the keys and what keeps a
 * Space for itself are the panel's own, held to its snapshot by
 * `the_conversation_wears_the_extensions_own_measures`.
 *
 * Where this page parts from the panel, because the person asked (t-6824,
 * 2026-09-24: 「대화를 누르면 맨 아래부터 — CC처럼 지금 보던 화면부터. 위쪽에
 * 있으면 맨 아래로 가기가 있음 좋겠어」): a list opens at its foot the first
 * time it has a box — a page built while its leaf was off screen had no
 * height to go to the foot of, and opened at its top — and keeps to it when
 * its box changes size; a conversation looked at again stands on the row its
 * reader left it on; and a reader who is not following gets a button back
 * to the foot (`chatFootDoorNode`). The extension grows no such button. */
const CHAT_FOLLOW = Object.freeze({ slack: 50, intent: 300, glide: 2000 });
const CHAT_FOLLOW_KEYS = Object.freeze({ up: new Set(["ArrowUp", "PageUp", "Home"]), down: new Set(["ArrowDown", "PageDown", "End"]) });
const CHAT_FOLLOW_CONTROL = "button, [role=\"button\"], input, textarea, [contenteditable]:not([contenteditable=\"false\"])";
// Where this page parts from the panel's rule: its step rows are `details`, not
// buttons, and a step's line (a `summary`) answers a Space by opening the row —
// that is no wish to go towards the foot, nor to leave it.
const CHAT_FOLLOW_SPACE = `${CHAT_FOLLOW_CONTROL}, summary`;

/* Whether the page is asked for less motion. */
function motionReduced() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/* How far the list stands above its foot. */
function chatFootGap(list) {
  return list.scrollHeight - list.scrollTop - list.clientHeight;
}

/* The list's following, brought up to where the list stands now: a move its
 * `scroll` event has not told yet — a place set a moment ago in this same
 * task — is read here as that event would read it, so no answer lags the
 * list. */
function chatFollowState(list) {
  list.__follow ??= {
    away: false, top: list.scrollTop, height: list.scrollHeight, intent: null, at: 0, touch: null, gliding: null, placed: false,
  };
  noteChatScroll(list, list.__follow);
  return list.__follow;
}

/* One move of the list, read as the extension reads its `scroll`: up is
 * leaving — unless the list only shrank under a reader who never asked to go
 * up, or it stands at its very foot — and down into the last 50px is coming
 * back, unless the person was on the way up or the move only kept pace with
 * rows that grew above. A list with no box (its page off screen) reads a top
 * of 0 that is no move of anyone's, and is not read. */
function noteChatScroll(list, state) {
  const top = list.scrollTop;
  if (top === state.top || list.clientHeight === 0) return;
  const height = list.scrollHeight;
  const gap = height - top - list.clientHeight;
  const intent = Date.now() - state.at < CHAT_FOLLOW.intent ? state.intent : null;
  const grew = height - state.height;
  const kept = intent === null && grew > 0 && Math.abs(top - state.top - grew) <= 1;
  const up = top < state.top;
  state.top = top;
  state.height = height;
  state.intent = null;
  if (gap < CHAT_FOLLOW.slack) state.gliding = null;
  if (up) {
    if (grew < 0 && gap > 1 && intent !== "up") return;
    state.away = gap > 1 || intent === "up";
  } else if (gap < CHAT_FOLLOW.slack && intent !== "up" && !kept) {
    state.away = false;
  }
}

/* Whether the person has left the list's foot. */
function chatAway(list) {
  return chatFollowState(list).away;
}

/* Whether new words should carry the list to its foot — asked before they
 * are drawn, as the extension asks: never once the person has left, yes
 * within the last 50px, and yes while a send's glide is under way. A new turn
 * is no reason to take the scroll from a reader above (09-18: following on
 * every new turn dragged them to the foot at each poll). */
function chatFollows(list) {
  if (!list) return false;
  const state = chatFollowState(list);
  return !state.away && (chatFootGap(list) < CHAT_FOLLOW.slack || state.gliding !== null);
}

/* To the foot — gliding while a send's glide is young and motion is welcome,
 * at once otherwise. */
function carryToFoot(list) {
  const state = chatFollowState(list);
  const young = state.gliding !== null && Date.now() - state.gliding < CHAT_FOLLOW.glide;
  if (!young) state.gliding = null;
  if (young && !motionReduced()) list.scrollTo({ top: list.scrollHeight, behavior: "smooth" });
  else list.scrollTop = list.scrollHeight;
}

/* A send brings the person back: the list follows again and goes home,
 * gliding when it stood more than the slack away. */
function returnToFoot(list) {
  if (!list) return;
  const state = chatFollowState(list);
  state.away = false;
  if (chatFootGap(list) >= CHAT_FOLLOW.slack) state.gliding = Date.now();
  carryToFoot(list);
  paintFootDoor(list);
}

/* Whether the list is not following its words — the reader left, or it
 * stands beyond the slack with no glide on the way: what the foot's button
 * is shown for. */
function chatOffFoot(list) {
  const state = chatFollowState(list);
  return state.away || (state.gliding === null && chatFootGap(list) >= CHAT_FOLLOW.slack);
}

/* The foot's button worn as the list stands: one class, and no write when
 * it already says so. */
function paintFootDoor(list) {
  list.__door?.classList.toggle("is-shown", chatOffFoot(list));
}

/* The way back to the foot (t-6824): a button over the list's lower right,
 * shown while the list is not following its words, that glides home and
 * follows again. Named by the words it shows; no tooltip. */
function chatFootDoorNode(list) {
  const door = document.createElement("button");
  door.type = "button";
  door.className = "chat-foot-door";
  const words = t("worker.toFoot", "맨 아래로");
  door.setAttribute("aria-label", words);
  // Enter and Space, while it has focus, are its own — the window's key
  // sink (`rearmKeySink`) would otherwise take them for the terminal.
  door.dataset.keyboardOwner = "true";
  const said = document.createElement("span");
  said.textContent = words;
  door.append(iconNode("arrow-down"), said);
  door.addEventListener("click", () => {
    returnToFoot(list);
    // Pressed from the keyboard, the button hides under the focus: the keys
    // go on to the list it brought home. A pointer's press leaves the focus
    // where it was (the composer, as often as not).
    if (document.activeElement === door) list.focus({ preventScroll: true });
  });
  list.__door = door;
  paintFootDoor(list);
  return door;
}

/* Where the list stands the first time it has a box: on the row its reader
 * was reading when the page last left the host (`keepChatPlace`), as far
 * under the list's top as it stood — or, never left or that row gone, at
 * its foot, or at its top when a finished helper's report stands there
 * (`syncHelperReport`). A list with no box yet is placed by its watch
 * (`keepToFoot`) when it gets one. */
function standChatPlace(list) {
  if (list.clientHeight === 0) return;
  const state = chatFollowState(list);
  state.placed = true;
  const place = list.__run?.chatPlace ?? null;
  const row = place === null ? null : list.querySelector(`:scope > [data-turn="${place.turn}"]`);
  if (row) {
    state.away = true;
    list.scrollTop += row.getBoundingClientRect().top - list.getBoundingClientRect().top - place.offset;
  } else if (list.__report) {
    list.scrollTop = 0;
    state.away = chatFootGap(list) >= CHAT_FOLLOW.slack;
  } else {
    state.away = false;
    list.scrollTop = list.scrollHeight;
  }
  paintFootDoor(list);
}

/* The reader's place, kept on the run as its page leaves the host: nothing
 * while the list follows its foot, else the first row under the list's top
 * (a person's words stuck there are not where they read) and how far under
 * it that row stands. A list with no box keeps the place it had. */
function keepChatPlace(list) {
  const run = list?.__run;
  if (!run || list.clientHeight === 0) return;
  run.chatPlace = null;
  if (!chatAway(list)) return;
  const top = list.getBoundingClientRect().top;
  for (const row of list.querySelectorAll(":scope > [data-turn]:not(.is-user)")) {
    const box = row.getBoundingClientRect();
    if (box.bottom <= top) continue;
    run.chatPlace = { turn: row.dataset.turn, offset: box.top - top };
    return;
  }
}

/* The conversation list of the page `node` stands on, or null. */
function pageTurnsOf(node) {
  for (let at = node; at; at = at.parentElement) if (at.__helperPage) return at.__helperPage.turns;
  return null;
}

/* Whether a box between `target` and the list can still scroll that way —
 * then the wheel or the touch is that box's, not the list's. */
function innerScrolls(list, target, up) {
  for (let box = target instanceof Element ? target : null; box && box !== list; box = box.parentElement) {
    if (box.scrollHeight <= box.clientHeight + 1) continue;
    const overflow = getComputedStyle(box).overflowY;
    if (overflow !== "auto" && overflow !== "scroll") continue;
    if (up ? box.scrollTop > 0 : box.scrollTop + box.clientHeight < box.scrollHeight - 1) return true;
  }
  return false;
}

/* The person's hand on the list: a wheel, a touch or a key says which way
 * they mean to go, and the scroll says where the list went. The listeners
 * live on the list and go with it. */
function keepToFoot(list) {
  const state = chatFollowState(list);
  const leave = () => {
    if (list.scrollTop <= 0) return;
    state.away = true;
    state.intent = "up";
    state.at = Date.now();
  };
  const toward = () => {
    state.intent = "down";
    state.at = Date.now();
    if (chatFootGap(list) < CHAT_FOLLOW.slack) state.away = false;
  };
  const go = (up, target) => {
    if (innerScrolls(list, target, up)) return;
    if (up) leave();
    else toward();
  };
  list.addEventListener("wheel", (event) => {
    if (event.ctrlKey || event.shiftKey || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    go(event.deltaY < 0, event.target);
  }, { passive: true });
  list.addEventListener("touchstart", (event) => {
    state.touch = event.touches[0]?.clientY ?? null;
  }, { passive: true });
  list.addEventListener("touchmove", (event) => {
    const y = event.touches[0]?.clientY;
    if (y === undefined || state.touch === null || y === state.touch) return;
    const up = y > state.touch;
    state.touch = y;
    go(up, event.target);
  }, { passive: true });
  list.addEventListener("keydown", (event) => {
    if (event.defaultPrevented) return;
    if (event.key === " ") {
      if (event.target instanceof Element && event.target.closest(CHAT_FOLLOW_SPACE)) return;
      if (event.shiftKey) leave();
      else toward();
    } else if (CHAT_FOLLOW_KEYS.up.has(event.key)) leave();
    else if (CHAT_FOLLOW_KEYS.down.has(event.key)) toward();
  });
  // A row's body is built the first time it opens. `toggle` does not bubble,
  // so one capturing listener on the list stands for the one each row of a page
  // of hundreds would carry; a row names the painter it wants (`__paint`).
  list.addEventListener("toggle", (event) => {
    const row = event.target;
    if (row.open && row.__paint && list.__run) row.__paint(row, list.__run);
  }, true);
  // The doors in shell steps' first lines (t-22100), every one of them by these
  // two: a press, or Enter or Space on one the keyboard reached, opens its file
  // — and is no press on the row whose line it stands in.
  const outDoor = (event) => (event.target instanceof Element ? event.target.closest(".helper-out-path") : null);
  list.addEventListener("click", (event) => {
    const door = outDoor(event);
    if (!door) return;
    event.preventDefault();
    openStepOutDoor(list, door);
  });
  list.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const door = outDoor(event);
    if (!door) return;
    event.preventDefault();
    openStepOutDoor(list, door);
  });
  list.addEventListener("scroll", () => {
    noteChatScroll(list, state);
    paintFootDoor(list);
  }, { passive: true });
  // The list's own box: the first one places it (`standChatPlace`), and a
  // list that follows its foot keeps to it when the box changes size — a
  // shorter window, a notice standing over it. Its border box: the room kept
  // under the words for the dock is padding the dock's own watch writes and
  // answers for (`chatDockNode`), and a content box would be asked again in
  // the same frame — WebKit's "ResizeObserver loop" error.
  if (typeof ResizeObserver === "function") {
    list.__footWatch = new ResizeObserver(() => {
      if (list.clientHeight === 0) return;
      if (!state.placed) standChatPlace(list);
      else if (!state.away) carryToFoot(list);
      paintFootDoor(list);
      // The rail beside it changed its height with it (t-22100).
      askTurnRail(list);
    });
    list.__footWatch.observe(list, { box: "border-box" });
  }
}

/* ---- a helper at work (t-6323 A6) ------------------------------------------
 *
 * The extension's live helper rows (2.1.280 `E85`, fed by the session's
 * `task_started` / `task_progress` frames): one row per local agent while it
 * runs — its description, and after a colon its own summary or its latest
 * step (`DU0`); then its tokens and tools once it has spent any (`FU0`) and
 * its time (`MU0`) — `12.3k tokens · 5 tools · 1m 3s`, redrawn each second.
 * Past four rows the first three stand and one more sums the rest (`sD1`,
 * `jU0`). The extension stands them under the fold that holds the Agent call
 * (its Focus view); this list stands them at its foot, over the status line,
 * which is where that fold is while the turn is out, and in either view — a
 * list that leaves a helper's own rows out, as this one does, would show
 * nothing while it works. A wire's rows are its session's frames
 * (`wire_log.tasks`); a pane's are its helper rows (`paneSubagents`), which
 * know a name and a tool count and no clock. The number of rows is the
 * panel's own, held to its snapshot. */
const CHAT_AGENT_ROWS = 3;

/* The helpers at work on `run`'s page, as one shape: `{ key, description,
 * said, tokens, tools, startedMs }` — `said` the summary or latest step,
 * `startedMs` null where nothing says when it began. */
function helperTasksOf(run) {
  if (run.wire) {
    return (run.wireLog?.tasks ?? []).map((task) => ({
      key: task.task,
      description: task.description,
      said: task.summary ?? task.step ?? null,
      tokens: task.tokens ?? 0,
      tools: task.tools ?? 0,
      startedMs: task.started_ms ?? null,
    }));
  }
  if (run.helper?.id !== PANE_LOG_ID) return [];
  return (paneSubagents.get(run.term) ?? [])
    .filter((row) => row.state === "running")
    .map((row) => ({ key: row.id, description: row.name, said: null, tokens: 0, tools: row.tool_calls ?? 0, startedMs: null }));
}

/* `DU0`: the description, or the description and what it last said. */
function helperTaskLabel(task) {
  return task.said === null || task.said === task.description ? task.description : `${task.description}: ${task.said}`;
}

/* `FU0`: the tokens and tools spent, once there are tokens. A row with no
 * clock (a pane's helper, which has no tokens to wait for) says its tools
 * alone. */
function helperSpent(tokens, tools, clocked) {
  const uses = toolUsesWords(tools);
  if (tokens > 0) return [t("worker.agentTokens", "{{count}} 토큰", { count: formatTokens(tokens) }), uses];
  return clocked ? [] : [uses];
}

/* `MU0`: what it spent, then its time. */
function helperTaskMeta(task, now) {
  const clocked = task.startedMs !== null;
  return [...helperSpent(task.tokens, task.tools, clocked), clocked ? elapsedWords(now - task.startedMs) : ""]
    .filter(Boolean).join(" · ");
}

/* `jU0`: the rows past the first three, summed — their tokens and tools once
 * there are tokens, and their time together. */
function helperOverflowMeta(tasks, now) {
  const tokens = tasks.reduce((sum, task) => sum + task.tokens, 0);
  const tools = tasks.reduce((sum, task) => sum + task.tools, 0);
  const clocked = tasks.filter((task) => task.startedMs !== null);
  const time = clocked.length > 0
    ? t("worker.agentsCombined", "합계 {{time}}", { time: elapsedWords(clocked.reduce((sum, task) => sum + now - task.startedMs, 0)) })
    : "";
  return [...helperSpent(tokens, tools, clocked.length > 0), time].filter(Boolean).join(" · ");
}

/* One row: the live dot, the label, the meta — written only where the words
 * changed, so a poll that brings nothing new touches nothing. */
function paintHelperTaskRow(row, label, meta) {
  if (row.childElementCount === 0) {
    row.className = "helper-agent";
    const dot = document.createElement("span");
    dot.className = "helper-agent-dot";
    dot.setAttribute("aria-hidden", "true");
    const said = document.createElement("span");
    said.className = "helper-agent-label";
    const measure = document.createElement("span");
    measure.className = "helper-agent-meta";
    row.append(dot, said, measure);
  }
  writeTextContent(row.children[1], label);
  writeTextContent(row.children[2], meta);
}

/* The rows at the list's foot, kept in step with the helpers at work. The
 * block stands only while one does. */
function syncHelperTasks(list, run, now = Date.now()) {
  const tasks = helperTasksOf(run);
  let block = list.querySelector(":scope > .helper-agents");
  if (tasks.length === 0) {
    block?.remove();
    return;
  }
  if (!block) {
    block = document.createElement("div");
    block.className = "helper-agents";
    list.insertBefore(block, list.querySelector(":scope > .helper-status"));
  }
  const shown = tasks.length <= CHAT_AGENT_ROWS + 1 ? tasks : tasks.slice(0, CHAT_AGENT_ROWS);
  const rest = tasks.slice(shown.length);
  const rows = shown.map((task) => [task.key, helperTaskLabel(task), helperTaskMeta(task, now)]);
  if (rest.length > 0) {
    rows.push(["+", t("worker.agentsMore", "+{{count}}개 더", { count: rest.length, s: rest.length === 1 ? "" : "s" }),
      helperOverflowMeta(rest, now)]);
  }
  const keep = new Set(rows.map(([key]) => key));
  for (const row of [...block.children]) if (!keep.has(row.dataset.task)) row.remove();
  rows.forEach(([key, label, meta], at) => {
    let row = block.children[at];
    if (row?.dataset.task !== key) {
      row = [...block.children].find((one) => one.dataset.task === key) ?? document.createElement("div");
      row.dataset.task = key;
      block.insertBefore(row, block.children[at] ?? null);
    }
    paintHelperTaskRow(row, label, meta);
  });
}

/* ---- the todo list (t-6323 A7) ---------------------------------------------
 *
 * The extension draws a todo tool's call as its list (2.1.280 `qD1` / `PG0`):
 * the head says 「Update Todos」 and nothing beside it, and under it each
 * item's content beside a box — ticked when done, `✽` while under way, empty
 * while it waits — a done item faded and struck through. Only the content:
 * no count, no numbering, never the `activeForm`, and no result line (the
 * list says what the result would). Every call is its own row with the whole
 * list as that call left it; nothing is updated in place — the session's
 * newest list (`todos`) is kept by the extension but drawn nowhere else. In
 * the Focus view the newest list stands out of its fold (`ew0`): the newest
 * call that has not failed — out or back — and none when that call emptied
 * the list. Which tool is the todo tool is the catalog's fact (`todo_tool`):
 * a CLI that names none keeps its generic row, and so does a call whose
 * input is not a list, as the extension's falls back to its generic body. */
function todosOf(turn, agent) {
  const name = agentVoice(agent).todo_tool;
  if (!name || turn.tool?.name !== name) return null;
  let input;
  try {
    input = JSON.parse(turn.tool.input);
  } catch {
    return null;
  }
  return Array.isArray(input?.todos) ? input.todos.filter((todo) => typeof todo?.content === "string") : null;
}

/* The list itself — a disabled checkbox per item, so the state is read out as
 * the box's own (checked, mixed, unchecked) rather than as a glyph. */
function todoListNode(todos) {
  const list = document.createElement("ul");
  list.className = "helper-todos";
  for (const todo of todos) {
    const item = document.createElement("li");
    item.className = todo.status === "completed" ? "helper-todo is-completed" : "helper-todo";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "helper-todo-box";
    box.disabled = true;
    box.checked = todo.status === "completed";
    box.indeterminate = todo.status === "in_progress";
    const content = document.createElement("div");
    content.className = "helper-todo-content";
    content.textContent = todo.content;
    item.append(box, content);
    list.appendChild(item);
  }
  return list;
}

/* A todo call's row knows its list, once — its input never changes after the
 * call stood; the list stands in the step's body when the row opens
 * (`dressStepBody`), an emptied list is a head alone. True when the row is a
 * todo row, so the generic body stays away. */
function dressTodoRow(row, turn, run) {
  if (row.__todos !== undefined) return row.__todos !== null;
  row.__todos = todosOf(turn, run.agent);
  if (row.__todos === null) return false;
  row.classList.add("is-todo");
  return true;
}

/* In the Focus view the newest todo list stands out of its fold, its row
 * open; the one before it goes back in (and closes, if it was this that
 * opened it). Asked once per paint, and it touches the page only when the one
 * that stands changes. */
function standLatestTodo(list, focus) {
  const newest = focus ? [...list.querySelectorAll(":scope > .helper-turn.is-todo:not(.is-failed)")].at(-1) : null;
  const latest = newest?.__todos?.length > 0 ? newest : null;
  const before = list.__standingTodo ?? null;
  if (before === latest) return;
  if (before) {
    before.__standing = false;
    before.classList.remove("is-standing");
    if (before.__group) writeHidden(before, !before.__group.__open);
    if (before.__openedByFocus) {
      before.__openedByFocus = false;
      before.open = false;
    }
  }
  list.__standingTodo = latest;
  if (!latest) return;
  latest.__standing = true;
  latest.classList.add("is-standing");
  writeHidden(latest, false);
  if (!latest.open) {
    latest.__openedByFocus = true;
    latest.open = true;
  }
}

/* ---- images (t-6323 A8) ------------------------------------------------------
 *
 * The extension draws an image a message carries as a pill (2.1.280 `LN` →
 * `pp`): a 12px thumbnail, its name (`image.<kind>`) and, once it has loaded,
 * its size (`W×H`) — the person's above their words, a tool's with what it
 * handed back — and a press (or Enter/Space) opens it whole over the page
 * (`previewOverlay`): a dimmed ground, the image within 90% of the window, a
 * close button that takes the focus, Esc or a press on the ground to leave,
 * the focus back where it was. The extension loads every image with its
 * message; this page asks for a picture only when its pill is in view — the
 * payload stays where the reader set it aside (`pane_image` for a pane's
 * transcript or its helper's, `wire_image` for what a wire session keeps),
 * because a long session carries dozens and each is half a megabyte. The
 * watcher is the list's own (`root: list`), so a pill up the transcript or on
 * a page behind another tab is never fetched, and the watcher and every pill
 * it holds go with the list. */

/* Where `image` is fetched from: the wire session's own keeping, or the
 * transcript the page reads — the pane's, or its helper's. */
function imagePlaceOf(run, image) {
  const id = run.helper?.id;
  if (id === WIRE_LOG_ID) return ["wire_image", { id: run.wire, at: image.at }];
  return ["pane_image", { term: run.term, helper: id === PANE_LOG_ID ? null : id, at: image.at }];
}

/* The pill's name, as the extension names an image block: `image.` and the
 * kind its media type says (`image.png`, `image.jpeg`), `image.image` when it
 * says none. */
function imageName(image) {
  return `image.${image.media_type.split("/")[1] || "image"}`;
}

/* A row of pills for `images`, each empty until it is in view. */
function imagePillsNode(run, images) {
  const pills = document.createElement("div");
  pills.className = "helper-images";
  for (const image of images) {
    const pill = document.createElement("button");
    pill.type = "button";
    pill.className = "helper-image";
    const thumb = document.createElement("img");
    thumb.className = "helper-image-thumb";
    thumb.alt = "";
    const name = document.createElement("span");
    name.className = "helper-image-name";
    name.textContent = imageName(image);
    const size = document.createElement("span");
    size.className = "helper-image-size";
    pill.append(thumb, name, size);
    labelButton(pill, t("worker.imageOpen", "{{name}} 크게 보기", { name: name.textContent }));
    pill.addEventListener("click", (event) => {
      event.stopPropagation();
      void openImagePreview(pill);
    });
    pill.__image = image;
    pill.__run = run;
    pills.appendChild(pill);
  }
  return pills;
}

/* The pills in `node` that the list does not watch yet start being watched —
 * once the row stands in the list, since a row is built before it is put in
 * its place. */
function watchImagePills(list, node) {
  for (const pill of node.querySelectorAll(".helper-image")) {
    if (pill.__watched) continue;
    pill.__watched = true;
    imageWatch(list).observe(pill);
  }
}

/* The picture as a data URL, asked for once per pill. */
function imageSourceOf(pill) {
  pill.__source ??= (async () => {
    const [command, args] = imagePlaceOf(pill.__run, pill.__image);
    const payload = await invoke(command, args);
    if (typeof payload !== "string" || payload === "") throw new Error(t("worker.imageGone", "이 그림을 읽을 수 없습니다"));
    return `data:${pill.__image.media_type};base64,${payload}`;
  })();
  return pill.__source;
}

/* A pill that came into view takes its thumbnail, and its size once the
 * picture has loaded; one whose payload cannot be read keeps its name. */
async function fillImagePill(pill) {
  let source;
  try {
    source = await imageSourceOf(pill);
  } catch {
    pill.classList.add("is-missing");
    return;
  }
  const thumb = pill.querySelector(".helper-image-thumb");
  thumb.addEventListener("load", () => {
    writeTextContent(pill.querySelector(".helper-image-size"), `${thumb.naturalWidth}×${thumb.naturalHeight}`);
  }, { once: true });
  thumb.src = source;
}

/* The list's watcher: a pill is filled the first time it is in the list's
 * view, and then forgotten. */
function imageWatch(list) {
  list.__imageWatch ??= new IntersectionObserver((entries, watch) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      if (coveredByLaterHeader(entry.target)) {
        holdCoveredPill(list, entry.target);
        continue;
      }
      watch.unobserve(entry.target);
      void fillImagePill(entry.target);
    }
  }, { root: list });
  return list.__imageWatch;
}

/* A person's row is the extension's sticky header: every one before the
 * header in view stays stuck at the list's top under it — in view by
 * geometry, which is all an IntersectionObserver judges, and covered by the
 * later one. A pill there is not in view until no later header covers it. */
function coveredByLaterHeader(pill) {
  const row = pill.closest(".helper-turn.is-user");
  if (!row) return false;
  let next = row.nextElementSibling;
  while (next && !next.classList.contains("is-user")) next = next.nextElementSibling;
  return next !== null && next.getBoundingClientRect().top <= row.getBoundingClientRect().top + 1;
}

/* A covered pill waits for the list to scroll it out from under the later
 * header — asked once a frame while the list moves, and only while some pill
 * waits. */
function holdCoveredPill(list, pill) {
  const held = (list.__coveredPills ??= new Set());
  held.add(pill);
  if (list.__coverWatch) return;
  list.__coverWatch = true;
  let frame = 0;
  list.addEventListener("scroll", () => {
    if (frame || held.size === 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const view = list.getBoundingClientRect();
      for (const one of held) {
        if (!one.isConnected) {
          held.delete(one);
          continue;
        }
        const box = one.getBoundingClientRect();
        if (box.bottom < view.top || box.top > view.bottom || coveredByLaterHeader(one)) continue;
        held.delete(one);
        list.__imageWatch?.unobserve(one);
        void fillImagePill(one);
      }
    });
  }, { passive: true });
}

/* The picture whole, over the page. Esc is caught before anything else hears
 * it — it closes the picture and never interrupts the turn (A3's Esc). */
async function openImagePreview(pill) {
  let source;
  try {
    source = await imageSourceOf(pill);
  } catch (error) {
    showError(error);
    return;
  }
  const back = document.activeElement;
  const ground = document.createElement("div");
  ground.className = "chat-image-preview";
  const frame = document.createElement("div");
  frame.className = "chat-image-preview-frame";
  frame.setAttribute("role", "dialog");
  frame.setAttribute("aria-modal", "true");
  frame.setAttribute("aria-label", imageName(pill.__image));
  const picture = document.createElement("img");
  picture.className = "chat-image-preview-image";
  picture.alt = imageName(pill.__image);
  picture.src = source;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "chat-image-preview-close";
  close.appendChild(iconNode("x"));
  labelButton(close, t("worker.imagePreviewClose", "미리보기 닫기 (Esc)"));
  frame.append(picture, close);
  ground.appendChild(frame);
  const leave = () => {
    document.removeEventListener("keydown", onKey, true);
    ground.remove();
    if (back?.isConnected) back.focus({ preventScroll: true });
  };
  const onKey = (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopImmediatePropagation();
    leave();
  };
  ground.addEventListener("click", (event) => {
    if (event.target === ground) leave();
  });
  close.addEventListener("click", leave);
  document.addEventListener("keydown", onKey, true);
  document.body.appendChild(ground);
  close.focus();
}

/* ---- copying (t-6323 A9) ------------------------------------------------------
 *
 * The extension's copy button (2.1.280 `gN`): a press writes the text, and the
 * icon turns to a check for 2 s — the name stays. It stands under each answer
 * (`Copy response`, the action row the page already carries —
 * `helperActionsNode`) and over a tool's IN and OUT. A code block's copy is a
 * word on the bar over the block (t-22100, the approved conversation): 「복사」,
 * the document viewer's own (`dressMarkdownFences`), there without hovering,
 * which says 「복사됨」 for the same 2 s. The page's one clipboard door is
 * `clipboardText`; the 2 s is the panel's own, held to its snapshot. */
const CHAT_COPIED_MS = 2000;

/* A press on `button` writes what `text()` says then, and says so for a
 * moment: an icon's button with the check, a word's (`words`: what it says,
 * and what it says once it copied) with its other word. */
function copyOnPress(button, text, words = null) {
  button.addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      await clipboardText.write(text());
    } catch (error) {
      showError(error);
      return;
    }
    clearTimeout(button.__copied);
    if (words) writeTextContent(button, words.copied);
    else button.replaceChildren(iconNode("check"));
    button.classList.add("is-copied");
    button.__copied = setTimeout(() => {
      if (words) writeTextContent(button, words.copy);
      else button.replaceChildren(iconNode("copy"));
      button.classList.remove("is-copied");
    }, CHAT_COPIED_MS);
  });
}

/* Every code block the prose drew stands on a flat panel (t-22100): a bar over
 * the code with the fence's language — the document viewer's word for it
 * (`fenceLanguageWords`) — and its copy, then the code. The colours inside the
 * code are the fence's own (`colourCodeIn`, t-22095). */
function dressCodeCopies(host) {
  for (const block of host.querySelectorAll("pre.md-block")) {
    if (block.parentElement?.classList.contains("helper-code")) continue;
    const frame = document.createElement("div");
    frame.className = "helper-code";
    const bar = document.createElement("div");
    bar.className = "helper-code-bar";
    const language = document.createElement("span");
    language.className = "helper-code-lang";
    language.textContent = fenceLanguageWords(block);
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "helper-code-copy";
    const words = { copy: t("mdview.copy", "복사"), copied: t("worker.copied", "복사됨") };
    copy.textContent = words.copy;
    // Named for what it copies; the word it shows is part of that name.
    copy.setAttribute("aria-label", t("worker.copyCode", "코드 복사"));
    copyOnPress(copy, () => block.textContent, words);
    bar.append(language, copy);
    block.replaceWith(frame);
    frame.append(bar, block);
  }
}

/* ---- the code in a fence wears colours (t-22095) ---------------------------
 *
 * A fence's code is coloured the way the editor colours a file: by the
 * editor's own parsers (the vendored table, `CM6.languageFor`) and its own
 * ramp — every colour `editorHighlightStyle` writes is a role
 * (`var(--syntax-keyword)` is `keyword`), and a token of that role stands in
 * a `span.chat-code-<role>` (shell.css), which wears the role's token. A
 * fence in a language the table does not know shows its code as it is.
 *
 * Spans, not ranges the engine paints over the words (the CSS Custom
 * Highlight API): an engine that holds a highlight checks its ranges again
 * on each frame in which anything changes. Chromium paid for holding a
 * single range on every key — 0.8 ms, 4 ms on a CPU four times slower — and
 * WebKit paid on every frame of a stream, while spans cost a frame nothing
 * (jobs 1790962540-w-22096-diag-hlcost, 1790962542-w-22096-diag-stream).
 * Only the rows near the view hold a body at all (the shelf), so a 400-turn
 * page holds a few dozen spans.
 *
 * Finding the roles is the work: the parse and the walk over its tree, done
 * in slices that never hold the main thread past `CODE_COLOUR.sliceMs` — when
 * the page is idle, or, where the engine does not say when it is, right after
 * a frame. What a fence's code came to is remembered, and a fence drawn again
 * with the same code (a row back from the shelf, the answer's turn standing
 * where its stream was) wears those spans as it is drawn — no frame in which
 * it stands plain. The fence still being written stands plain (the block
 * being written is repainted every frame) and takes its colours once it has
 * closed. A fence the person's selection reaches keeps its words as they
 * stand until the selection moves: the spans hold the same words, but a node
 * taken from under a selection takes the selection with it. */

/* - `sliceMs`: the most a slice holds the main thread — the rest of a frame
 *   stays the page's, and a key pressed meanwhile waits at most this long.
 *   A unit of work is not begun when the last one of its kind would not fit
 *   in what is left, a kind the page has not timed yet only begins a slice,
 *   and the line is drawn one tick of the page's clock early (a clock that
 *   counts whole milliseconds, WebKit's, reads 4 when nearly 5 have passed;
 *   a page that is not cross-origin isolated counts in tenths), and earlier
 *   again by what the last slices ran past theirs (`overKept`).
 * - A unit is the parser's steps for `stepMs` (a step is microseconds —
 *   below what the clock can tell — but a legacy mode's step is a whole
 *   chunk of lines, and runs alone), `unitChars` characters of the walk,
 *   `unitSpans` spans put in place of the words they hold, or the
 *   highlighter made the first time: the clock is looked at between two.
 * - `overKept`: how much of what a slice ran past its line the next one
 *   still allows for — a collection the engine ran inside a unit is
 *   forgotten in a few slices, a machine that is slow throughout keeps
 *   its due (a 400-turn page's slice ran 0.4 ms past its line, job
 *   1790962547-w-22096-diag-slices).
 * - `waitMs`: a slice asked for while the page stays busy (a long stream)
 *   runs at the latest this long after.
 * - `keptChars`: the code whose roles are remembered, least recently drawn
 *   let go first — the fences a page draws again are a stream's and the
 *   rows near the view, not a long conversation's every one.
 * - `largestChars`: a fence longer than this stands plain — a file pasted
 *   whole would be thousands of spans in one row. */
const CODE_COLOUR = Object.freeze({
  sliceMs: 4,
  stepMs: 0.25,
  overKept: 0.5,
  unitChars: 512,
  unitSpans: 64,
  waitMs: 500,
  keptChars: 128 * 1024,
  largestChars: 32 * 1024,
});

/* The words agents write after a fence that are not the extension of a file
 * the editor's table knows (it is read by file name): the extension they
 * mean. A whole-name language (`dockerfile`) is found by its name. */
const CODE_FENCE_EXTENSIONS = Object.freeze({
  javascript: "js",
  typescript: "ts",
  python: "py",
  rust: "rs",
  golang: "go",
  shell: "sh",
  console: "sh",
  shellsession: "sh",
  ruby: "rb",
  kotlin: "kt",
  csharp: "cs",
  "c++": "cpp",
});

/* The role a colour of the editor's ramp paints. */
const CODE_ROLE_OF_COLOUR = /^var\(--syntax-([a-z]+)\)$/;

/* The roles a fence leaves in the code's own ink, as the approved mockup
 * draws them: names (the ramp's variable is the ink, near enough) and
 * punctuation — two thirds of a fence's tokens. */
const CODE_PLAIN_ROLES = Object.freeze(new Set(["variable", "punctuation"]));

/* A fence waiting its turn — or being coloured; and one that wears its
 * colours. */
const CODE_WAITING = "waiting";
const CODE_WORN = "worn";

const codeColour = {
  // Fences in line (as the colouring each needs), the newest drawn last: it
  // is coloured first — the foot of a page that opens, the block a stream
  // just settled.
  queue: [],
  // The fence being coloured, and how far it has got.
  job: null,
  asked: false,
  // Fences the person's selection reached: they wait for it to move. And
  // whether a selection may stand: set by each change of the page's
  // selection, cleared once one is asked for and found empty.
  held: [],
  heard: false,
  maybeSelected: false,
  // What a fence's code came to: its spans by language and code, least
  // recently drawn first.
  kept: new Map(),
  keptChars: 0,
  languages: new Map(),
  highlighter: null,
  // Whether this page can colour, asked once.
  able: null,
  // One tick of the page's clock, read once.
  tick: null,
  // How long the last unit of each kind took, and what the last slices ran
  // past their line.
  took: new Map(),
  over: 0,
};

/* The clock's tick: how far `performance.now()` moves when it moves — five
 * microseconds in one engine, a whole millisecond in another. Read once, by
 * waiting for the clock to move (at most one tick); a clock that does not
 * move within the slice's own line is taken to tick by the whole line, and
 * each slice then runs one unit. */
function codeClockTick() {
  if (codeColour.tick === null) {
    const from = performance.now();
    const giveUp = Date.now() + CODE_COLOUR.sliceMs;
    let next = from;
    while (next === from && Date.now() <= giveUp) next = performance.now();
    codeColour.tick = next === from ? CODE_COLOUR.sliceMs : Math.min(next - from, CODE_COLOUR.sliceMs);
  }
  return codeColour.tick;
}

/* A slice's clock: its line, how many units it has run, and the person's
 * selection as the slice began. The selection is asked for once, and only
 * while one may stand: it cannot move while the slice runs, and the engine
 * brings the page's layout up to date to answer — asked between units, it
 * laid the 400-turn page out again after every one (a slice ran 4.6 ms; 1.6
 * without, job 1790962549-w-22096-diag-slices2). A fence drawn in the very
 * paint that colours it cannot hold a selection yet, so that clock asks
 * nothing. */
function codeSliceClock(asking = true) {
  const until = performance.now() + CODE_COLOUR.sliceMs - codeClockTick() - codeColour.over;
  return { until, units: 0, selected: asking ? codeSelection() : null };
}

/* The person's selection on the page as a range, or null. */
function codeSelection() {
  if (!codeColour.maybeSelected) return null;
  const selection = document.getSelection();
  if (selection === null || selection.rangeCount === 0 || selection.isCollapsed) {
    codeColour.maybeSelected = false;
    return null;
  }
  return selection.getRangeAt(0);
}

/* A slice has ended: what it ran past its line is allowed for by the next. */
function codeSliceEnded(clock) {
  codeColour.over = Math.max(0, performance.now() - clock.until, codeColour.over * CODE_COLOUR.overKept);
}

/* Whether this page can colour: the editor's table and its walk over a tree. */
function codeColourable() {
  codeColour.able ??= typeof window.CM6?.highlightTree === "function" && typeof window.CM6?.languageFor === "function";
  return codeColour.able;
}

/* The editor's ramp as roles: one class a colour, named by the colour's
 * token, but for the roles left plain. Only colours — a weight or a slant
 * the editor gives a role is not carried: the approved mockup colours a
 * fence's code and nothing more. */
function codeColourHighlighter() {
  if (codeColour.highlighter !== null) return codeColour.highlighter;
  const roles = [];
  for (const spec of editorHighlightStyle().specs) {
    const role = CODE_ROLE_OF_COLOUR.exec(spec.color ?? "")?.[1];
    if (role !== undefined && !CODE_PLAIN_ROLES.has(role)) roles.push({ tag: spec.tag, class: role });
  }
  codeColour.highlighter = window.CM6.HighlightStyle.define(roles);
  return codeColour.highlighter;
}

/* The language a fence's first word names, or null — once a word: a legacy
 * mode is a new language each time the table makes it. */
function codeLanguageOf(info) {
  const word = String(info ?? "").trim().split(/[\s,{]/, 1)[0].toLowerCase();
  if (word === "") return null;
  if (!codeColour.languages.has(word)) {
    const found = window.CM6.languageFor(word) ?? window.CM6.languageFor(`code.${CODE_FENCE_EXTENSIONS[word] ?? word}`);
    codeColour.languages.set(word, found === null ? null : found.language ?? found);
  }
  return codeColour.languages.get(word);
}

/* Every fence `host` holds, coloured: at once when its code's roles are
 * remembered — within one slice's time, the rest wait in line — and in line
 * for a slice when they are not. Called once the prose stands; a colouring
 * that throws leaves its fence as far as it got and never the prose
 * unpainted — the throw is the page's to see, a moment later. */
function colourCodeIn(host) {
  if (!codeColourable()) return;
  followCodeSelection();
  const clock = codeSliceClock(false);
  for (const pre of host.querySelectorAll("pre.md-block")) {
    if (pre.__codeColour !== undefined) continue;
    const words = codeWordsOf(pre);
    const language = words === null ? null : codeLanguageOf(pre.dataset.language);
    if (language === null || words.data.length > CODE_COLOUR.largestChars) continue;
    pre.__codeColour = CODE_WAITING;
    const job = codeJobOf(pre, words, language);
    if (job.spans !== null) {
      try {
        if (drawCodeSpans(job, clock)) continue;
      } catch (error) {
        queueMicrotask(() => {
          throw error;
        });
        continue;
      }
    }
    if (!codeColour.held.includes(job)) codeColour.queue.push(job);
  }
  codeSliceEnded(clock);
  askCodeSlice();
}

/* Ask for a slice, once, while anything waits. */
function askCodeSlice() {
  if (codeColour.asked || (codeColour.queue.length === 0 && codeColour.job === null)) return;
  codeColour.asked = true;
  if (typeof window.requestIdleCallback === "function") {
    window.requestIdleCallback(colourCodeSlice, { timeout: CODE_COLOUR.waitMs });
  } else {
    requestAnimationFrame(() => setTimeout(colourCodeSlice, 0));
  }
}

/* One slice: the fence being coloured goes on where it stopped, then the
 * next in line, until the slice's time is spent. A fence whose colouring
 * throws is let go as far as it got, and the throw is the page's to see. */
function colourCodeSlice() {
  codeColour.asked = false;
  const clock = codeSliceClock();
  for (;;) {
    codeColour.job ??= nextCodeJob();
    const job = codeColour.job;
    if (job === null) break;
    let done;
    try {
      done = stepCodeJob(job, clock);
    } catch (error) {
      codeColour.job = null;
      askCodeSlice();
      throw error;
    }
    if (!done && !codeColour.held.includes(job)) break;
    codeColour.job = null;
  }
  codeSliceEnded(clock);
  askCodeSlice();
}

/* Whether one more unit of `kind` fits in the slice: the first of a slice
 * always does, so every slice moves the colouring on; after it, a kind the
 * page has timed fits when its last unit would end before the line. */
function codeTimeLeft(clock, kind) {
  if (clock.units === 0) return true;
  const took = codeColour.took.get(kind);
  return took !== undefined && performance.now() + took <= clock.until;
}

/* A unit of `kind` begun at `from` has ended: its time is the kind's
 * measure. */
function codeTook(clock, kind, from) {
  codeColour.took.set(kind, performance.now() - from);
  clock.units += 1;
}

/* The next fence in line still standing on the page. */
function nextCodeJob() {
  while (codeColour.queue.length > 0) {
    const job = codeColour.queue.pop();
    if (job.pre.isConnected && job.pre.__codeColour === CODE_WAITING) return job;
  }
  return null;
}

/* What colouring `pre` takes: its words, and the roles its code came to
 * before — or `null`, and the parse that finds them begins in a slice. */
function codeJobOf(pre, words, language) {
  const key = `${pre.dataset.language}\n${words.data}`;
  return { pre, words, key, language, spans: keptCodeSpans(key), parse: null, tree: null, found: [], walked: 0, laid: 0, at: 0 };
}

/* A fence's words as the prose draws them — one text node — or null: a
 * fence holding anything else is left as it stands. */
function codeWordsOf(pre) {
  const node = pre.firstChild;
  return node !== null && node === pre.lastChild && node.nodeType === Node.TEXT_NODE ? node : null;
}

/* Take `job` on as far as the slice allows: the parse, the walk over its tree
 * naming each token's role, then the spans. True when the fence is done —
 * coloured, or gone from the page. */
function stepCodeJob(job, clock) {
  if (!job.pre.isConnected) return true;
  if (job.spans === null && job.parse === null && job.tree === null) {
    if (!codeTimeLeft(clock, "parse")) return false;
    const from = performance.now();
    job.parse = job.language.parser.startParse(job.words.data);
    codeTook(clock, "parse", from);
  }
  while (job.parse !== null) {
    if (!codeTimeLeft(clock, "parse")) return false;
    const from = performance.now();
    let tree = job.parse.advance();
    while (tree === null && performance.now() - from < CODE_COLOUR.stepMs) tree = job.parse.advance();
    codeTook(clock, "parse", from);
    if (tree !== null) {
      job.tree = tree;
      job.parse = null;
    }
  }
  if (job.tree !== null) {
    if (codeColour.highlighter === null) {
      if (!codeTimeLeft(clock, "highlighter")) return false;
      const from = performance.now();
      codeColourHighlighter();
      codeTook(clock, "highlighter", from);
    }
    const text = job.words.data;
    const found = (from, end, classes) => {
      job.found.push(from, end, codeRoleOf(classes));
    };
    while (job.walked < text.length) {
      if (!codeTimeLeft(clock, "walk")) return false;
      const from = performance.now();
      const to = codeUnitEnd(text, job.walked);
      window.CM6.highlightTree(job.tree, codeColour.highlighter, found, job.walked, to);
      job.walked = to;
      codeTook(clock, "walk", from);
    }
    job.tree = null;
    job.spans = job.found;
    keepCodeSpans(job.key, job.spans, text.length);
  }
  return drawCodeSpans(job, clock);
}

/* Where a unit of the walk stops: at the end of the line it reaches, and
 * never past twice its length — a minified line is one long line. */
function codeUnitEnd(text, from) {
  const reach = from + CODE_COLOUR.unitChars;
  if (reach >= text.length) return text.length;
  const line = text.indexOf("\n", reach);
  return Math.min(line < 0 ? text.length : line + 1, reach + CODE_COLOUR.unitChars);
}

/* A span's role: the innermost of the classes the walk gives it. */
function codeRoleOf(classes) {
  return classes.slice(classes.lastIndexOf(" ") + 1);
}

/* Put `job`'s spans in place of the words they hold, a unit at a time from
 * the top: a unit splits the words still plain where its last span ends, and
 * puts its spans and the words between them in place of the part before.
 * True when the fence wears them all (or no longer holds the words the job
 * read); false when the slice is spent, or the person's selection reaches
 * the fence — it then waits for the selection to move. */
function drawCodeSpans(job, clock) {
  const { spans, pre } = job;
  while (job.laid < spans.length) {
    if (job.words.parentNode !== pre) return true;
    if (clock.selected?.intersectsNode(pre)) {
      codeColour.held.push(job);
      return false;
    }
    if (!codeTimeLeft(clock, "spans")) return false;
    const began = performance.now();
    const end = Math.min(spans.length, job.laid + 3 * CODE_COLOUR.unitSpans);
    // Where the unit's last span ends, in the words still plain.
    const stop = spans[end - 2] - job.at;
    const text = job.words.data;
    const rest = stop < text.length ? job.words.splitText(stop) : null;
    const drawn = document.createDocumentFragment();
    let at = 0;
    for (; job.laid < end; job.laid += 3) {
      const from = spans[job.laid] - job.at;
      const to = spans[job.laid + 1] - job.at;
      if (from > at) drawn.append(text.slice(at, from));
      const span = document.createElement("span");
      span.className = `chat-code-${spans[job.laid + 2]}`;
      span.textContent = text.slice(from, to);
      drawn.append(span);
      at = to;
    }
    job.words.replaceWith(drawn);
    job.words = rest;
    job.at += stop;
    codeTook(clock, "spans", began);
    if (job.words === null) break;
  }
  pre.__codeColour = CODE_WORN;
  return true;
}

/* The page's selection, followed from the first fence the page colours: a
 * change of it means a selection may stand (a text box's own caret is not
 * the page's), and the fences that waited for it go back in line. */
function followCodeSelection() {
  if (codeColour.heard) return;
  codeColour.heard = true;
  codeColour.maybeSelected = true;
  document.addEventListener("selectionchange", (event) => {
    if (event.target !== document) return;
    codeColour.maybeSelected = true;
    if (codeColour.held.length === 0) return;
    codeColour.queue.push(...codeColour.held.splice(0));
    askCodeSlice();
  });
}

/* The roles `key`'s code came to, or null — and drawn again, it is the
 * most recently drawn. */
function keptCodeSpans(key) {
  const kept = codeColour.kept.get(key);
  if (kept === undefined) return null;
  codeColour.kept.delete(key);
  codeColour.kept.set(key, kept);
  return kept.spans;
}

function keepCodeSpans(key, spans, chars) {
  const held = codeColour.kept.get(key);
  if (held !== undefined) {
    codeColour.kept.delete(key);
    codeColour.keptChars -= held.chars;
  }
  codeColour.kept.set(key, { spans, chars });
  codeColour.keptChars += chars;
  for (const [oldest, kept] of codeColour.kept) {
    if (codeColour.keptChars <= CODE_COLOUR.keptChars) break;
    codeColour.kept.delete(oldest);
    codeColour.keptChars -= kept.chars;
  }
}

/* ---- a row far from view keeps its height, not its body (t-6323 B1) ---------
 *
 * The extension keeps every message's DOM and trims the list instead — past
 * 600 messages it drops back to 500, finished tool rows first (2.1.280 `wf1`,
 * `Of1`, `bf1`). This page keeps 400 turns (`HELPER_TURN_CAP`), and a 400-turn
 * page stood 8,127 elements, half of them the diff rows of edits nobody was
 * looking at (B0). So an answer more than `CHAT_SHELF.screens` screens from
 * the list's view gives up its prose and keeps the height it stood at;
 * coming back within reach, it is dressed again from its turn, the way a
 * folded thought paints its body the first time it opens (t-2973). Nothing is
 * lost: the page's memory is the turns, and a body is only their drawing. A
 * step has nothing to give up: it is one line until a person opens it, and
 * its diff, its IN/OUT box and its pictures are built by that press (t-15682).
 *
 * A batch of rows — a page opening on a long conversation — builds whole only
 * its last `CHAT_SHELF.warm`, which cover the view at the foot and its reach;
 * the rest are born the way a shelved row stands, so the page never builds
 * 400 bodies to give 380 of them up a frame later (the memory that build
 * took is what the renderer keeps).
 *
 * What never gives up its body: a row still out (it changes), a row whose
 * door the person opened, a row holding the person's selection, a row holding
 * the keyboard's focus (the control the person stands on is not taken from
 * under them by a wheel), and a row the Focus view folded away (it stands at
 * no height, and will be judged when it stands again). One watcher per list
 * (`root: list`), so it goes with the list; a list that is not laid out (a
 * page behind another tab) answers nothing, and its rows are asked again when
 * it is. */
const CHAT_SHELF = Object.freeze({ screens: 2, warm: 30, frameMs: 8, gapMs: 100 });

/* A row dressed is a layout: the list is a column of some 400 flex items, and
 * the height read that keeps the view's place makes the page lay all of them
 * out — about 6 ms at a 4x CPU for one row, the same for three (t-20445; the
 * prose itself is 1.7 ms a row). Dressed one row a callback, a scroll paid it
 * 33 times a second, and the frames it landed in missed 16.7 ms.
 *  - `gapMs`: rows that came within reach wait for one another and are dressed
 *    together, at most this often. A row is two screens from the view when it
 *    comes within reach — over 100 ms of a fast wheel — so it is dressed
 *    before it is seen; one already in the view is never made to wait.
 *  - `frameMs`: what one such batch spends before the rest wait a frame more
 *    (a page's first answer 92 rows at once passed the frame). */
function shelfWatch(list) {
  list.__shelf ??= new IntersectionObserver((entries) => judgeShelf(list, entries), {
    root: list,
    rootMargin: `${CHAT_SHELF.screens * 100}% 0px`,
  });
  return list.__shelf;
}

/* A row born far from view: no body, and no height of its own to keep yet —
 * it stands at what its call line or its actions take until the watcher
 * builds it within reach. */
function shelveBorn(row) {
  row.__shelved = { height: null };
  row.classList.add("is-shelved");
}

/* A row the watcher should judge: an answer with a turn behind it, or a shell
 * step that shows its first lines (`dressStepOut`, t-22100). Any other step is
 * one line until it is opened and has no body to give up. */
function shelvable(row) {
  if (row.__turn === undefined || row.classList.contains("is-streaming")) return false;
  return row.classList.contains("is-assistant") || row.__outText !== undefined;
}

function watchShelf(list, row) {
  if (shelvable(row)) shelfWatch(list).observe(row);
}

function forgetShelf(list, row) {
  list.__shelf?.unobserve(row);
  list.__shelfWaiting?.delete(row);
}

/* Whether words in `node` are chosen — a write into it would collapse them. */
function wordsChosenIn(node) {
  const selection = document.getSelection();
  return Boolean(selection && selection.rangeCount > 0 && !selection.isCollapsed && selection.containsNode(node, true));
}

/* Whether `row` may give up its body now — an opened step neither: the
 * height it would keep is its body's, which the person may close. */
function mayShelve(row) {
  if (row.classList.contains("is-live") || row.open || row.querySelector(".is-open") || row.contains(document.activeElement)) return false;
  return !wordsChosenIn(row);
}

/* One answer of the watcher: rows that left reach give up their bodies at the
 * height they stood; rows that came back are dressed again, and a row that
 * came back above the view with another height than it stood at — a row born
 * without its body, or one kept while the pane was resized — moves the list
 * by the difference, so what the person reads stays put. All the reads come
 * after all the writes: one layout. */
function judgeShelf(list, entries) {
  const run = list.__run;
  if (!run || !list.isConnected) return;
  const back = [];
  for (const entry of entries) {
    const row = entry.target;
    if (!row.isConnected) continue;
    if (!entry.rootBounds || entry.rootBounds.height === 0 || entry.boundingClientRect.height === 0) {
      // Not laid out (the page, or the row, stands at no height): asked again
      // when it stands (`askShelfAgain`).
      list.__shelf.unobserve(row);
      (list.__shelfLater ??= new Set()).add(row);
      continue;
    }
    if (entry.isIntersecting) {
      // The watcher's bounds are the view grown by its margin on each side;
      // the view itself starts a margin below their top.
      const view = entry.rootBounds.top + CHAT_SHELF.screens * (entry.rootBounds.height / (1 + 2 * CHAT_SHELF.screens));
      // What it stands at now — its kept height, or a born row's own.
      if (row.__shelved) back.push({ row, kept: entry.boundingClientRect.height, above: entry.boundingClientRect.bottom <= view });
    } else if (!row.__shelved && mayShelve(row)) {
      shelveRow(list, row, entry.boundingClientRect.height);
    } else {
      // Left reach while still waiting for its body: nothing to dress now.
      list.__shelfWaiting?.delete(row);
    }
  }
  if (back.length === 0) return;
  const waiting = (list.__shelfWaiting ??= new Map());
  for (const one of back) waiting.set(one.row, one);
  dressWaiting(list);
}

/* Dress the rows that came back, the ones in the view first and as many of the
 * rest as `CHAT_SHELF.frameMs` allows (at least one), then ask again next
 * frame for what is left. A row's `above` is read again here: the list may have
 * moved since the watcher answered. */
function dressWaiting(list) {
  const waiting = list.__shelfWaiting;
  const run = list.__run;
  if (!waiting || waiting.size === 0 || !run || !list.isConnected) return;
  const frame = list.getBoundingClientRect();
  const rows = [];
  for (const [row, one] of waiting) {
    if (!row.isConnected || !row.__shelved) {
      waiting.delete(row);
      continue;
    }
    const box = row.getBoundingClientRect();
    one.above = box.bottom <= frame.top;
    one.inView = box.bottom > frame.top && box.top < frame.bottom;
    one.away = one.inView ? 0 : Math.min(Math.abs(box.bottom - frame.top), Math.abs(box.top - frame.bottom));
    rows.push(one);
  }
  rows.sort((a, b) => a.away - b.away);
  const started = performance.now();
  if (rows.length > 0 && rows[0].away > 0 && started - (list.__shelfDressedAt ?? 0) < CHAT_SHELF.gapMs) {
    askShelfFrame(list);
    return;
  }
  list.__shelfDressedAt = started;
  const dressed = [];
  for (const one of rows) {
    if (!one.inView && dressed.length > 0 && performance.now() - started >= CHAT_SHELF.frameMs) break;
    unshelveRow(one.row, run);
    waiting.delete(one.row);
    dressed.push(one);
  }
  let moved = 0;
  for (const one of dressed) if (one.above) moved += one.row.offsetHeight - one.kept;
  if (moved !== 0) list.scrollTop += moved;
  if (waiting.size > 0) askShelfFrame(list);
}

function askShelfFrame(list) {
  if (list.__shelfFrame) return;
  list.__shelfFrame = requestAnimationFrame(() => {
    list.__shelfFrame = 0;
    dressWaiting(list);
  });
}

/* A row gives up its body and keeps its height — as its least height: the
 * list is a column flex box, and an item's plain height is only where it
 * starts before the box shrinks it to its content. */
function shelveRow(list, row, height) {
  row.__shelved = { height };
  row.style.minHeight = `${height}px`;
  row.classList.add("is-shelved");
  // A shell step's body is its first lines (t-22100).
  row.firstElementChild?.querySelector(":scope > .helper-step-out")?.remove();
  const said = row.querySelector(":scope > .helper-said");
  if (!said) return;
  // Its fences' colours go with the body (t-22095).
  said.replaceChildren();
}

/* A row takes its body back from its turn — an answer its prose, a shell step
 * its first lines. */
function unshelveRow(row, run) {
  row.__shelved = null;
  row.style.minHeight = "";
  row.classList.remove("is-shelved");
  if (row.classList.contains("is-tool")) dressStepOut(row, row.__outText ?? "");
  else paintAnswerProse(row.querySelector(":scope > .helper-said"), row.__turn, run);
}

/* Rows that were not laid out when the watcher looked, asked again once they
 * stand — the page came back from behind another tab, or the Focus view let
 * the row out of its fold. Cheap when there are none. */
function askShelfAgain(list) {
  const later = list.__shelfLater;
  if (!later || later.size === 0 || list.clientHeight === 0) return;
  for (const row of later) {
    if (!row.isConnected) {
      later.delete(row);
      continue;
    }
    if (row.hidden) continue;
    later.delete(row);
    list.__shelf.observe(row);
  }
}

/* ---- steps (t-15682) -------------------------------------------------------
 *
 * What an agent did stands as steps, one closed line each: a dot for how it
 * stands, the kind in words and what it touched, what came of it, how long it
 * took (t-22100: the approved conversation's flat rows, a state dot where the
 * kind's icon stood — the kind's words already say what it was). The raw
 * input and output are one press away — a step
 * is a `<details>`, so its line is a summary the keyboard reaches by itself
 * (Tab; Enter and Space press it) and the engine announces closed or open —
 * and the body is built the first time the row opens, so a page of hundreds of
 * steps holds a line each and not a body each. Steps of one kind in a row
 * (nothing said between them) fold into one row that opens to its members; a
 * step that failed is never folded, and one still out waits for its answer
 * before it may join. The Focus view folds by turn instead and keeps its rows
 * single (`applyFocusView`). */

/* A kind's words and the family the head counts it in (t-22100), keyed by the
 * words the core reduces a tool's name to (`hook::Tool::as_str`): a turn
 * carries its call's as `tool.kind`, and the page keeps no list of vendor
 * names to read the name by — one table, the core's. A tool the core has no
 * word for travels as its own name. */
const STEP_LOOKS = new Map(Object.entries({
  read: { family: "file", word: () => t("worker.stepRead", "파일 읽기") },
  edit: { family: "file", word: () => t("worker.stepEdit", "파일 수정") },
  write: { family: "file", word: () => t("worker.stepWrite", "파일 쓰기") },
  bash: { family: "shell", word: () => t("worker.stepShell", "셸") },
  grep: { family: "search", word: () => t("worker.stepSearch", "검색") },
  web: { family: "web", word: () => t("worker.stepWeb", "웹 읽기") },
  websearch: { family: "web", word: () => t("worker.stepWebSearch", "웹 검색") },
  task: { family: "helper", word: () => t("worker.stepTask", "헬퍼 호출") },
}));

/* The one row that is not a call the core has a word for: a todo list (the
 * catalog's own `todo_tool`). */
const STEP_OWN_LOOKS = new Map(Object.entries({
  todo: { family: "todo", word: () => t("worker.todoHead", "할 일 갱신") },
}));

/* The families the head's tally counts by (t-22100, the approved mockup's
 * 「웹 2 · 셸 1 · 파일 6」): a page read and a search are the web, a read, an
 * edit and a write are files — a step's own row still says which. A tool the
 * core has no word for is counted with the others. */
const STEP_FAMILY_WORDS = Object.freeze({
  web: () => t("worker.familyWeb", "웹"),
  shell: () => t("worker.familyShell", "셸"),
  file: () => t("worker.familyFile", "파일"),
  search: () => t("worker.familySearch", "검색"),
  helper: () => t("worker.familyHelper", "헬퍼"),
  todo: () => t("worker.familyTodo", "할 일"),
  other: () => t("worker.familyOther", "기타"),
});

/* The kind a step is drawn by: the catalog's todo tool; else the word the core
 * reduced the call's tool to. A kind the page has no look for — a tool the core
 * has no word for, or a call written before turns carried a kind — is a kind of
 * its own, worded as the tool's own name (`tool:<name>`). */
function stepKindOf(turn, run) {
  const todo = agentVoice(run.agent).todo_tool;
  if (todo && turn.tool?.name === todo) return "todo";
  const kind = turn.tool?.kind;
  return STEP_LOOKS.has(kind) ? kind : `tool:${toolWords(turn).name}`;
}

function stepLook(kind) {
  return STEP_LOOKS.get(kind) ?? STEP_OWN_LOOKS.get(kind) ?? { family: "other", word: () => kind.slice(5) };
}

/* A target as the line says it: an address without its scheme, a long path
 * by its last three names — the file's name is the part a person reads. The
 * whole of it stands in the opened row. */
const STEP_TARGET_FIT = 48;

function stepTargetWords(arg) {
  const bare = arg.replace(/^https?:\/\/(www\.)?/, "");
  if (bare.length <= STEP_TARGET_FIT || /\s/.test(bare) || !bare.includes("/")) return bare;
  const parts = bare.split("/");
  return parts.length > 4 ? `…/${parts.slice(-3).join("/")}` : bare;
}

/* A shell step is titled by what its command does: its own first words, fitted the way a target is. A
 * command that opens by changing folder — `cd <dir> && …`, `cd <dir> ; …`, one after another — says
 * where it ran, not what it did, so the title drops it and closes with the folder's last name after
 * 「 · 」 (the mockup's 「셸 grep SCREEN_WIDTH… · src」). The opened row keeps the whole command. A
 * `cd` that is not the command's opening is part of what it does, and stays in. */
const SHELL_CD_LEAD = /^\s*cd\s+(?:"([^"]*)"|'([^']*)'|((?:\\.|[^\s;&|"'\\])+))\s*(?:&&|;)\s*/;

function shellTargetWords(command) {
  let rest = command;
  let folder = "";
  for (let lead = SHELL_CD_LEAD.exec(rest); lead !== null; lead = SHELL_CD_LEAD.exec(rest)) {
    folder = lead[1] ?? lead[2] ?? lead[3];
    rest = rest.slice(lead[0].length);
  }
  // Nothing after the change of folder: the change is what the step did.
  if (rest.trim() === "") return command;
  const fitted = rest.length > STEP_TARGET_FIT ? `${rest.slice(0, STEP_TARGET_FIT - 1).trimEnd()}…` : rest;
  const where = basename(folder.replace(/[\\/]+$/, ""));
  return where === "" ? fitted : `${fitted} · ${where}`;
}

/* A step's title by its kind: a shell step's is its command's own words (`arg` is the transcript's
 * line, cut at a length nobody wants for a command — `whole` is the command), every other kind's is
 * its target as the line says it. */
function stepTitleWords(kind, arg, whole = arg) {
  return kind === "bash" ? shellTargetWords(whole) : stepTargetWords(arg);
}

function stepFirstLine(text) {
  const line = text.split("\n").find((one) => one.trim() !== "")?.trim() ?? "";
  return line.length > 120 ? `${line.slice(0, 119)}…` : line;
}

function stepLineCount(text) {
  return text === "" ? 0 : text.replace(/\n+$/, "").split("\n").length;
}

/* An edit's size as the diff says it: rows added, rows taken out. */
function stepEditTally(turn) {
  let added = 0;
  let removed = 0;
  for (const edit of turn.tool?.edits ?? []) {
    for (const line of edit.lines) {
      if (line.kind === "add") added += 1;
      else if (line.kind === "del") removed += 1;
    }
  }
  return added + removed > 0 ? `+${added} −${removed}` : "";
}

/* A line a search printed, `path:line:` or `path:line-` (grep -n, rg -n, and
 * their context lines): the path one token with a folder or an extension in
 * it, the line a number. What a shell step found, and the doors in its lines
 * (`dressStepOut`), are read by this one rule. */
const STEP_HIT_RE = /^([^\s:]*[./][^\s:]*):(\d+)(?=[:-])/;

/* How many lines a shell step found: every line it printed is a search's hit,
 * or it found nothing it can be said to have found (0) — a build's errors
 * name files too, among lines that do not. */
function stepFoundLines(output) {
  const lines = output.split("\n").filter((line) => line.trim() !== "");
  return lines.length > 0 && lines.every((line) => STEP_HIT_RE.test(line)) ? lines.length : 0;
}

/* How many results a web search brought back (t-22100): the distinct
 * addresses in what it answered. Every road's search hands its results back as
 * links — Claude Code's `Links: [{title, url}, …]`, a list of sources — and an
 * answer with none says no count: its size in bytes was no answer to "what
 * did it find" (「결과 5 B」). */
const STEP_URL_RE = /https?:\/\/[^\s"'<>()[\]{}]+/g;

function stepSearchCount(output) {
  return new Set(output.match(STEP_URL_RE) ?? []).size;
}

/* What a page read said of itself (t-22100): the HTTP status and the size the
 * fetch got, as the CLI recorded them beside its answer (`tool.facts`, read by
 * the backend off the line's `toolUseResult`) — never the length of the words
 * the model wrote about the page. `null` when the CLI recorded neither. */
function stepFetchFacts(turn) {
  const facts = turn.role === "tool_result" ? turn.tool?.facts : turn.outputFacts;
  const status = Number.isInteger(facts?.status) && facts.status > 0 ? facts.status : null;
  const bytes = Number.isFinite(facts?.bytes) && facts.bytes >= 0 ? facts.bytes : null;
  return status === null && bytes === null ? null : { status, bytes };
}

/* What came of a step, in a few words, by its kind: a failure says how in the
 * first line it printed, a read how long the file was, a search how many
 * lines it found, a command how many it printed — or found, when what it
 * printed are a search's hits — a page read its status and size, a web search
 * how many results it found. */
function stepResultWords(kind, turn, output, failed, row) {
  if (failed) {
    return t("worker.stepFailed", "실패: {{why}}", { why: stepFirstLine(output) || t("worker.noOutput", "출력 없음") });
  }
  const pictured = (turn.role === "tool_result" ? turn.images : turn.outputImages)?.length ?? 0;
  if (pictured > 0 && output.trim() === "") return t("worker.stepImages", "이미지 {{n}}장", { n: pictured });
  switch (kind) {
    case "read":
      return output === "" ? "" : t("worker.stepLines", "{{n}}줄", { n: stepLineCount(output) });
    case "grep": {
      const found = output.trim() === "" ? 0 : stepLineCount(output.trim());
      return found > 0 ? t("worker.stepFound", "{{n}}줄 찾음", { n: found }) : t("worker.stepNoMatch", "찾은 것 없음");
    }
    case "bash": {
      const found = stepFoundLines(output);
      return found > 0
        ? t("worker.stepFound", "{{n}}줄 찾음", { n: found })
        : t("worker.stepPrinted", "{{n}}줄 출력", { n: stepLineCount(output) });
    }
    case "web": {
      const facts = stepFetchFacts(turn);
      return facts === null ? "" : [facts.status, facts.bytes === null ? null : bytesLabel(facts.bytes)].filter((one) => one !== null).join(" · ");
    }
    case "websearch": {
      const found = stepSearchCount(output);
      return found > 0 ? t("worker.stepResults", "결과 {{n}}개", { n: found, s: found === 1 ? "" : "s" }) : "";
    }
    case "edit":
    case "write":
      return stepEditTally(turn);
    case "task":
      return "";
    case "todo": {
      const todos = row.__todos ?? [];
      return todos.length > 0
        ? t("worker.stepTodoDone", "{{done}}/{{total}} 완료", {
          done: todos.filter((todo) => todo.status === "completed").length,
          total: todos.length,
        })
        : "";
    }
    default:
      return stepFirstLine(output);
  }
}

/* How long a call took by the file's own stamps, from `from`'s start to `to`'s answer. A line the file
 * did not stamp was read at the moment the window read it — lines read in one batch share one time — so
 * a length is said only when both its ends carry a time from the file, and only when it comes to
 * something: nothing is better than 「0.0초」, which says a time nobody knows (t-18702). A length under
 * 50 ms is a 0.0 to the eye, and is nothing here too. */
const STEP_TOOK_MIN_MS = 50;

function stepTook(from, to) {
  if (from.fromClock !== false || to.outputFromClock !== false) return "";
  const took = to.outputAt - from.at;
  if (!(took >= STEP_TOOK_MIN_MS)) return "";
  return t("worker.elapsedShort", "{{s}}초", { s: (took / 1000).toFixed(1) });
}

/* One step's line: a dot for how it stands, what it was and touched, what
 * came of it, how long it took — the summary of the row (t-22100). The dot is
 * one empty `span` the row's state colours (`shell.css`): quiet before its
 * answer, the done ink once it came back, the failure's ink when it failed,
 * the agent's accent beating while it is out. A summary holds no controls, so
 * the file's door and the copies stand in the body — the doors a shell step's
 * first lines carry are the one exception (`dressStepOut`). Its Tab, Enter and
 * Space, while it has focus, are its own — the window's key sink
 * (`rearmKeySink`) would otherwise take them for the terminal. */
function stepLineNode() {
  const line = document.createElement("summary");
  line.className = "helper-step-line";
  line.dataset.keyboardOwner = "true";
  const dot = document.createElement("span");
  dot.className = "helper-step-dot";
  dot.setAttribute("aria-hidden", "true");
  const what = document.createElement("span");
  what.className = "helper-step-what";
  const kind = document.createElement("span");
  kind.className = "helper-step-kind";
  const target = document.createElement("span");
  target.className = "helper-step-target";
  what.append(kind, " ", target);
  const res = document.createElement("span");
  res.className = "helper-step-res";
  const meta = document.createElement("span");
  meta.className = "helper-step-meta";
  line.append(dot, what, res, meta);
  return line;
}

/* A page read's result as the approved mockup draws it, 「200 · 256 KB」: the
 * status in the ink of how it went — the done ink for a success, the
 * failure's for anything else — and the size beside it. Written only when the
 * facts move. */
function dressFetchResult(node, facts) {
  const said = `${facts.status}|${facts.bytes ?? ""}|${locale}`;
  if (node.__fetch === said) return;
  node.__fetch = said;
  const status = document.createElement("span");
  status.className = `helper-step-status ${facts.status >= 200 && facts.status < 300 ? "is-ok" : "is-bad"}`;
  status.textContent = String(facts.status);
  node.replaceChildren(status, ...(facts.bytes === null ? [] : [` · ${bytesLabel(facts.bytes)}`]));
}

/* A step's line from its turn — at birth, and again while it is out. Every
 * write is guarded, so a quiet poll costs no mutation. */
function dressStepLine(row, turn, words, state) {
  const line = row.firstElementChild;
  writeTextContent(line.querySelector(".helper-step-kind"), state.todo ? t("worker.todoHead", "할 일 갱신") : stepLook(row.__kind).word());
  writeTextContent(line.querySelector(".helper-step-target"), state.todo ? "" : stepTitleWords(row.__kind, words.arg, words.whole));
  let res = "";
  if (state.live) res = t("worker.stepLive", "진행 중");
  else if (state.output !== undefined) res = stepResultWords(row.__kind, turn, state.output, state.failed, row);
  const resNode = line.querySelector(".helper-step-res");
  const facts = row.__kind === "web" && state.output !== undefined && !state.failed ? stepFetchFacts(turn) : null;
  if (facts?.status !== null && facts?.status !== undefined) {
    dressFetchResult(resNode, facts);
  } else {
    resNode.__fetch = undefined;
    writeTextContent(resNode, res);
  }
  writeTextContent(line.querySelector(".helper-step-meta"), stepTook(turn, turn));
  // A shell step's first lines stand under its line (t-22100); a failed one's
  // too — they are what it said when it failed.
  if (row.__kind === "bash" && !state.live && state.output !== undefined) dressStepOut(row, state.output);
}

/* ---- a shell step's first lines (t-22100) ------------------------------------
 *
 * The approved conversation shows what a shell step printed under its line,
 * the way a person glances at a terminal: as many lines as make a tool's
 * words long (`CHAT_CLIP.lines`), the rest behind the row's chevron, in its
 * body. They stand inside the summary — a closed `details` shows nothing else
 * — as one block of preformatted words, so a page of steps pays one node a
 * shell step and one more for each `path:line` in its lines, which is a door
 * to the file at that line (`openStepOutDoor`; one listener on the list for
 * all of them, `keepToFoot`). A row born far from view takes its lines when
 * it comes within reach, as an answer takes its prose (`shelvable`, B1). */
function stepOutLines(output) {
  return output.replace(/\n+$/, "").split("\n").slice(0, CHAT_CLIP.lines).join("\n");
}

/* The lines as words and doors: a line that opens with `path:line` stands as
 * its door, then the rest of the line after two spaces (the mockup's
 * 「Carousel.tsx:16  const …」). */
function stepOutParts(text) {
  const parts = [];
  let plain = "";
  text.split("\n").forEach((line, at) => {
    if (at > 0) plain += "\n";
    const hit = STEP_HIT_RE.exec(line);
    if (!hit) {
      plain += line;
      return;
    }
    if (plain !== "") parts.push(plain);
    plain = "";
    const door = document.createElement("span");
    door.className = "helper-out-path";
    door.tabIndex = 0;
    door.setAttribute("role", "link");
    door.dataset.path = hit[1];
    door.dataset.line = hit[2];
    door.textContent = `${hit[1]}:${hit[2]}`;
    parts.push(door);
    plain += `  ${line.slice(hit[0].length + 1)}`;
  });
  if (plain !== "") parts.push(plain);
  return parts;
}

/* Keep a shell step's lines as its output says — a row away on the shelf
 * only remembers them. */
function dressStepOut(row, output) {
  if (row.__outText === undefined && row.__cold) shelveBorn(row);
  row.__cold = false;
  row.__outText = output;
  if (row.__shelved) return;
  const line = row.firstElementChild;
  const shown = stepOutLines(output);
  let out = line.querySelector(":scope > .helper-step-out");
  if (shown.trim() === "") {
    out?.remove();
    writeClass(row, "has-out", false);
    return;
  }
  writeClass(row, "has-out", true);
  if (out?.__said === shown) return;
  if (!out) {
    out = document.createElement("span");
    out.className = "helper-step-out";
    line.appendChild(out);
  }
  out.__said = shown;
  out.replaceChildren(...stepOutParts(shown));
}

/* A door in a shell step's lines, pressed: the file at that line, measured
 * from the session's checkout as every door on this page is (`helperBase`). */
function openStepOutDoor(list, door) {
  const run = list.__run;
  if (!run) return;
  openPath(resolveDocPath(helperBase(run), door.dataset.path), { preview: true, line: Number(door.dataset.line) });
}

/* A step's body, built the first time its row opens and followed from then
 * on — a result that joins a row already open comes into it (`dressToolTurn`).
 * The file the call touched is a door at its top; then what it handed back as
 * pictures, its edit's diff, and the two sides of its call, IN and OUT, each
 * with its copy. A todo call's body is its list. */
function paintStepBody(row, run) {
  row.__opened = true;
  dressStepBody(row, row.__turn, run);
}

function dressStepBody(row, turn, run) {
  let body = row.querySelector(":scope > .helper-step-body");
  if (!body) {
    body = document.createElement("div");
    body.className = "helper-step-body";
    row.appendChild(body);
  }
  const words = toolWords(turn);
  const output = turn.role === "tool_result" ? turn.text : turn.output;
  dressToolFile(row, turn, run);
  const images = turn.role === "tool_result" ? turn.images : turn.outputImages;
  if (images?.length > 0 && !body.querySelector(":scope > .helper-images")) {
    const pills = imagePillsNode(run, images);
    body.appendChild(pills);
    const list = pageTurnsOf(row);
    if (list) watchImagePills(list, pills);
  }
  if (row.__todos) {
    if (row.__todos.length > 0 && !body.querySelector(":scope > .helper-todos")) body.appendChild(todoListNode(row.__todos));
    return;
  }
  const edits = turn.tool?.edits ?? [];
  if (edits.length > 0 && !body.querySelector(":scope > .helper-tool-diff")) body.appendChild(toolDiffNode(edits, words.arg));
  // What the diff and the file's door already say is not said again in IN.
  const input = edits.length > 0 || turn.tool?.file?.path ? "" : words.input;
  dressToolBody(body, input, output ?? "");
}

/* A step's row — a closed details with its line; its body waits for the
 * first press. */
function stepRowNode(run, turn, spoken, cold = false) {
  const row = document.createElement("details");
  row.className = "helper-turn is-tool";
  row.dataset.turn = String(turn.seq);
  row.__turn = turn;
  row.__kind = stepKindOf(turn, run);
  row.appendChild(stepLineNode());
  row.__paint = paintStepBody;
  // A shell step born far from view keeps no lines of its own until it is in
  // reach (B1, `dressStepOut`).
  row.__cold = cold;
  dressToolTurn(row, turn, run, spoken);
  return row;
}

/* ---- steps of one kind in a row ---------------------------------------------
 *
 * A row of steps stands for its members: "file read ×6", the first one's
 * target and that five more stand behind it, opening to the members as one
 * closed line each. Its `data-turn` is its last member's — the list's keys
 * (`syncHelperTurns`) are the newest turn drawn. Only settled steps that did
 * not fail fold; a row a person opened is never folded away from under them. */
function runRowNode(run, turns) {
  const row = document.createElement("details");
  row.className = "helper-turn is-tool is-run is-done";
  row.__kind = stepKindOf(turns[0], run);
  row.__members = [...turns];
  row.__turn = turns.at(-1);
  row.dataset.turn = String(row.__turn.seq);
  row.appendChild(stepLineNode());
  row.__paint = paintRunBody;
  dressRunLine(row);
  return row;
}

function dressRunLine(row) {
  const line = row.firstElementChild;
  const turns = row.__members;
  writeTextContent(line.querySelector(".helper-step-kind"), t("worker.stepRun", "{{kind}} {{n}}개", { kind: stepLook(row.__kind).word(), n: turns.length }));
  const first = toolWords(turns[0]);
  writeTextContent(line.querySelector(".helper-step-target"),
    `${stepTitleWords(row.__kind, first.arg, first.whole)} ${t("worker.stepMore", "외 {{n}}개", { n: turns.length - 1 })}`);
  writeTextContent(line.querySelector(".helper-step-meta"), stepTook(turns[0], turns.at(-1)));
}

/* The members, built the first time the row opens and as they join after. */
function paintRunBody(row, run) {
  let body = row.querySelector(":scope > .helper-run-body");
  if (!body) {
    body = document.createElement("div");
    body.className = "helper-run-body";
    row.appendChild(body);
  }
  for (let at = body.children.length; at < row.__members.length; at += 1) {
    body.appendChild(stepRowNode(run, row.__members[at], -1));
  }
}

/* A settled step of `kind`: it came back, it did not fail, it is not a todo. */
function stepSettled(row, kind) {
  return row.classList.contains("is-tool") && row.classList.contains("is-done") && row.__kind === kind && kind !== "todo";
}

/* What a step may fold into: a settled step of its kind that is closed, or a
 * row of such steps, open or not. What may fold away: a closed settled step. */
function stepReceives(row, kind) {
  return stepSettled(row, kind) && (row.classList.contains("is-run") || !row.open);
}

function stepFolds(row, kind) {
  return stepSettled(row, kind) && !row.open;
}

/* Whether the tool turn about to be drawn folds into `before`, the row above
 * it: it came back whole and it is of the same kind. */
function stepJoins(before, turn, kind) {
  return before !== null && turn.role === "tool" && turn.output !== undefined && turn.outputError !== true &&
    stepReceives(before, kind);
}

/* `turn` joins `before` — a step, which becomes a row of two, or a row of
 * steps already. Returns the row that stands. */
/* A step leaving the list for the row that folds it: the shelf forgets it
 * first — a shell step's lines are watched (t-22100), and a row the watcher
 * still holds is a row the page keeps alive. */
function dropStep(row) {
  if (row.parentElement) forgetShelf(row.parentElement, row);
  row.remove();
}

function absorbStep(run, before, turn) {
  let row = before;
  if (!before.classList.contains("is-run")) {
    row = runRowNode(run, [before.__turn]);
    if (before.parentElement) forgetShelf(before.parentElement, before);
    before.replaceWith(row);
  }
  row.__members.push(turn);
  row.__turn = turn;
  row.dataset.turn = String(turn.seq);
  row.querySelector(":scope > .helper-run-body")?.appendChild(stepRowNode(run, turn, -1));
  dressRunLine(row);
  return row;
}

/* A step that was out has come back whole: it folds into the row before it,
 * and takes in the rows after it that came back before it did — calls run
 * side by side and answer in any order. */
function foldSettledStep(row, run) {
  const kind = row.__kind;
  if (!stepFolds(row, kind)) return;
  let at = row;
  const before = row.previousElementSibling;
  if (before && stepReceives(before, kind)) {
    at = absorbStep(run, before, row.__turn);
    dropStep(row);
  }
  for (let next = at.nextElementSibling; next && stepFolds(next, kind); next = at.nextElementSibling) {
    for (const turn of next.__members ?? [next.__turn]) at = absorbStep(run, at, turn);
    dropStep(next);
  }
}

/* The Focus view folds by turn and keeps its rows single: turned on, every
 * row of steps gives its members back as rows; turned off, they fold again.
 * What a reader had of a row of steps is kept: a member that was open comes
 * back open, and the row that stands for where the keyboard was — the member
 * it was on or in, else the first — is returned, for `landFocus` once the
 * Focus view has laid its folds. */
function dissolveSteps(list, run) {
  const held = document.activeElement;
  let lands = null;
  for (const row of list.querySelectorAll(":scope > .is-run")) {
    const members = [...(row.querySelector(":scope > .helper-run-body")?.children ?? [])];
    const rows = row.__members.map((turn) => stepRowNode(run, turn, -1));
    row.replaceWith(...rows);
    members.forEach((member, at) => {
      if (!member.open) return;
      rows[at].open = true;
      paintStepBody(rows[at], run);
    });
    if (row.contains(held)) lands = rows[Math.max(0, members.findIndex((member) => member.contains(held)))];
  }
  return lands;
}

/* The keyboard, put back on what stands for `row` now that the row was
 * rebuilt: its line — or, when the Focus view has folded the row away under a
 * head, that head, the one thing on screen that stands for it. */
function landFocus(row) {
  const line = row.hidden ? row.__group?.firstElementChild : row.firstElementChild;
  line?.focus({ preventScroll: true });
}

function regroupSteps(list, run) {
  let before = null;
  for (const row of [...list.querySelectorAll(":scope > .helper-turn")]) {
    const kind = row.__kind;
    if (before && kind !== undefined && stepFolds(row, kind) && stepReceives(before, kind)) {
      before = absorbStep(run, before, row.__turn);
      dropStep(row);
      continue;
    }
    before = row.classList.contains("is-tool") ? row : null;
  }
}

/* ---- a thought is one line (t-15682) -----------------------------------------
 *
 * The line is what the thought was about — the terminal's own rule for what
 * a thought is doing (`live_heading`, t-5872, held to its test cases by
 * `testConversationSteps`): a `**heading**` that opens a line wins; without one,
 * the newest complete sentence (a run ended by a newline, by a stop followed by
 * white space — so `src/app.rs` and `v1.2` end nothing, nor does the `1.` of a
 * list — or by a full-width stop). A thought still going says nothing until a
 * sentence has closed (`null`: the row keeps the word it has, and a half-written
 * sentence never flashes on it); one that is over (`finished`) counts its
 * unfinished tail, so it always has a line. The terminal cuts at 48 columns; the
 * page's line is cut only at THOUGHT_LINE_MAX characters. */
const THOUGHT_LINE_MAX = 160;

function thoughtLeadingBold(text) {
  const open = text.indexOf("**");
  if (open < 0) return null;
  if (text.slice(text.lastIndexOf("\n", open - 1) + 1, open).trim() !== "") return null;
  const close = text.indexOf("**", open + 2);
  if (close < 0) return null;
  const inner = text.slice(open + 2, close).trim();
  return inner === "" ? null : inner;
}

/* The markdown a line opens with — a list marker, a heading hash, emphasis
 * stars, code ticks — is noise in a title. */
function thoughtClean(segment) {
  let rest = segment.trim();
  for (;;) {
    let next = rest.replace(/^[#>\-*•\s]+/, "");
    const numbered = /^\d+[.)]\s+/.exec(next);
    if (numbered) next = next.slice(numbered[0].length);
    if (next === rest) break;
    rest = next;
  }
  return rest.replaceAll("**", "").replaceAll("`", "").trim();
}

function thoughtFit(line) {
  return line.length > THOUGHT_LINE_MAX ? `${line.slice(0, THOUGHT_LINE_MAX - 1)}…` : line;
}

function thoughtHeading(text, finished = false) {
  const bold = thoughtLeadingBold(text);
  if (bold !== null) return thoughtFit(thoughtClean(bold));
  let last = null;
  let start = 0;
  for (let at = 0; at < text.length; at += 1) {
    const mark = text[at];
    let ends = mark === "\n" || mark === "。" || mark === "！" || mark === "？";
    if (mark === "." || mark === "!" || mark === "?") {
      const run = text.slice(start, at).trimStart();
      ends = (at + 1 === text.length || /\s/.test(text[at + 1])) && run !== "" && !/^\d+$/.test(run);
    }
    if (!ends) continue;
    const segment = text.slice(start, at).trim();
    if (segment !== "") last = segment;
    start = at + 1;
  }
  if (last !== null) {
    const line = thoughtClean(last);
    if (line !== "") return thoughtFit(line);
  }
  const open = thoughtClean(text.slice(start));
  return finished || open.length > THOUGHT_LINE_MAX ? thoughtFit(open) : null;
}

/* A thought's line (t-22100): how long it thought, then what it was about —
 * the label leads, as the approved conversation draws it (「생각 2초 목록
 * 화면부터 ›」), and no mark stands before it: a thought is no step. The
 * label is the line's word for what came of the thought, so it wears the
 * class a step's result wears (`helper-step-res`) and is read there. */
function thoughtLineNode() {
  const line = document.createElement("summary");
  line.className = "helper-step-line";
  line.dataset.keyboardOwner = "true";
  const label = document.createElement("span");
  label.className = "helper-step-res helper-thought-lead";
  const what = document.createElement("span");
  what.className = "helper-step-what";
  const target = document.createElement("span");
  target.className = "helper-step-target";
  what.appendChild(target);
  line.append(label, what);
  return line;
}

/* A thought's row — closed, one line, the body painted the first time it
 * opens: a long run thinks often, and a page that rendered every thought up
 * front would pay for words nobody unfolded. */
function thoughtRowNode(className, run) {
  const row = document.createElement("details");
  row.className = className;
  row.appendChild(thoughtLineNode());
  const body = document.createElement("div");
  body.className = "helper-thought-body";
  row.appendChild(body);
  row.__paint = paintThoughtBody;
  return row;
}

function paintThoughtBody(row, run) {
  const body = row.querySelector(":scope > .helper-thought-body");
  if (row.classList.contains("is-streaming")) writeTextContent(body, row.__text ?? "");
  else if (!body.hasChildNodes()) paintHelperProse(body, row.__turn.text, helperBase(run));
}

/* The open body of a thought that is going, written once a frame with its
 * newest words — thirty deltas a second arrive, as into an answer
 * (`paintLiveAnswer`) — and not by a frame that finds words in it chosen: a
 * write would collapse them. The next delta after they are let go brings the
 * newest words in. */
function paintLiveThought(row) {
  if (row.__bodyFrame !== undefined) return;
  row.__bodyFrame = requestAnimationFrame(() => {
    row.__bodyFrame = undefined;
    if (!row.isConnected || !row.open) return;
    const body = row.querySelector(":scope > .helper-thought-body");
    if (!wordsChosenIn(body)) writeTextContent(body, row.__text ?? "");
  });
}

/* A piece of live text with nothing in it is no row: a voice that opens and
 * closes a block without a word (a thought it keeps to itself) said nothing
 * to show. */
function liveWords(piece) {
  return typeof piece.text === "string" && piece.text.trim() !== "";
}

/* The row that already stands for a piece of live text: the same voice, and
 * the words it shows — or the beginning of the piece's newer ones, for live
 * text only ever grows. A piece the transcript has carried leaves the live
 * list while its neighbours stay, so the row left over is the one that stood
 * for it, wherever it stands — not the last. */
function streamingRowOf(rows, claimed, piece) {
  const free = rows.filter((row) => !claimed.has(row) && row.dataset.role === piece.role);
  return free.find((row) => row.__text === piece.text) ??
    free.find((row) => piece.text.startsWith(row.__text ?? "")) ?? null;
}

/* The line of a thought that streams: what it has said so far while it goes
 * (`thoughtHeading` keeps the row's word until a sentence has closed), and
 * once it is over the newest sentence it ended on, under the bare word — 「생각
 * 중…」 is for a thought that goes. The row is the same node either way, so a
 * body being read and the keyboard stay where they are until its turn takes
 * its place. */
function dressStreamingThought(row, piece) {
  const over = piece.done === true;
  const heading = thoughtHeading(piece.text, over);
  if (heading !== null) writeTextContent(row.querySelector(".helper-step-target"), heading);
  writeClass(row, "is-done", over);
  writeTextContent(
    row.querySelector(":scope > .helper-step-line > .helper-step-res"),
    over ? thoughtLabel(piece) : t("worker.thinking", "생각 중…"),
  );
  if (row.open) paintLiveThought(row);
}

/* The words a streaming thought was saying are this turn: the row that will
 * stand for the turn is noted on the streaming row it replaces, so that what a
 * reader had of the streaming row can pass to it when that row goes. A turn
 * names the first thought not yet taken whose words it begins with, wherever
 * that row stands among the streaming ones. */
function noteThoughtSettled(list, turn, row) {
  const words = turn.text.trim();
  for (const streaming of list.querySelectorAll(":scope > .is-streaming.is-thinking")) {
    const said = (streaming.__text ?? "").trim();
    if (streaming.__settled === undefined && said !== "" && words.startsWith(said)) {
      streaming.__settled = row;
      return;
    }
  }
}

/* A streaming row goes. A thought that was being read — its line open, or the
 * keyboard on it — hands both to the row of the turn that stands in its place,
 * so it does not close under the reader; a thought nobody opened is just gone. */
function removeStreamingRow(stale, run) {
  const row = stale.__settled;
  if (row?.isConnected) {
    const held = stale.firstElementChild === document.activeElement;
    if (stale.open && !row.open) {
      row.open = true;
      paintThoughtBody(row, run);
    }
    if (held) landFocus(row);
  }
  stale.remove();
}

/* ---- the foot line says what is going on (t-15682) ---------------------------
 *
 * While the turn is out the line at the foot names what the agent is doing
 * now, in the words the rows above wear: the step that is out, else the newest
 * thought that is going. A thought that is over (`is-done`) is history, and the
 * first of several streaming ones is not the one going. Nothing to name, and it
 * keeps the CLI's own verb. */
function nowWordsOf(list) {
  if (!list) return "";
  const out = [...list.querySelectorAll(":scope > .is-tool.is-live")].at(-1);
  if (out) {
    const line = out.firstElementChild;
    return `${line.querySelector(".helper-step-kind").textContent} ${line.querySelector(".helper-step-target").textContent}`.trim();
  }
  const going = [...list.querySelectorAll(":scope > .is-streaming.is-thinking:not(.is-done)")].at(-1);
  const thinking = going?.querySelector(".helper-step-target")?.textContent ?? "";
  if (thinking !== "") return thinking;
  // The answer is being written (t-22100): the phase the provider's own stream
  // says it is in, in the window's words.
  return list.querySelector(":scope > .is-streaming.is-assistant") ? t("worker.nowWriting", "답을 쓰는 중") : "";
}

/* ---- the foot line, when no row is out to name (t-18702) ----------------------------
 *
 * The line names the step or thought that is going, or the answer being written (`nowWordsOf`). With
 * none, it says what the page's card last said it was doing: a helper's page reads its own card — the
 * activity zo's `subagents` frame names for each helper, filed under `sub:<term>:<id>` — and a pane's
 * conversation reads what its own channel's `session_status` says (`paneNow`). A wire page names its
 * own live rows. With nothing known it says the window's own word that the agent is working — for every
 * agent (t-22100): a CLI's turning verbs (Claude Code's 「Pondering…」) and a static English word (zo's
 * 「Working…」) are filler the window would be reading aloud, not what the agent is doing. */
function helperCardOf(run) {
  return `sub:${run.term}:${run.helper.id}`;
}

function nowActivityOf(run) {
  if (run.wire) return null;
  if (isHelperPage(run)) return newestActivity(helperCardOf(run)) ?? null;
  return run.helper?.id === PANE_LOG_ID ? paneNow.get(run.term) ?? null : null;
}

/* The kind an activity's verb is drawn by: the catalog's todo tool; else the word the core reduced the
 * tool to, which is the key of the step rows' kinds; else a kind of its own, worded as the tool's own name. */
function activityKindOf(verb, run) {
  const todo = agentVoice(run.agent).todo_tool;
  if (todo && verb === todo) return "todo";
  return STEP_LOOKS.has(verb) ? verb : `tool:${verb}`;
}

/* The verbs a status card carries that name no tool of the core's, with the page's own words for them
 * (an empty word: a fact the page has none for, so the line falls back). They are zo's protocol, spelled
 * once, here: `session_status.activity.verb` says waiting, reconnecting, reasoning silently and quiet
 * (`tui/strings.rs` ACTIVITY_*, `StatusActivity::verb` in `tui/view.rs`), and a zo helper's card says a
 * bare working before its first call (`session/subagent_progress.rs`). A source contract holds this
 * table to those spellings, so a word zo changes breaks that test instead of the line quietly naming
 * nothing. */
const ZO_STATUS_WORDS = {
  waiting: () => t("worker.nowWaiting", "답을 기다리는 중"),
  reconnecting: () => t("worker.nowReconnecting", "다시 연결하는 중"),
  "reasoning silently": () => t("worker.nowThinking", "생각하는 중"),
  quiet: () => "",
  working: () => "",
};

/* What one activity says, in the words the step rows wear — the same kind words and the same titles —
 * and, for the facts an agent's status carries that are no tool, in the page's own (`ZO_STATUS_WORDS`).
 * An activity that is over names nothing. */
function nowActivityWords(activity, run) {
  if (!activity || activity.phase !== "started" || typeof activity.verb !== "string") return "";
  if (Object.hasOwn(ZO_STATUS_WORDS, activity.verb)) return ZO_STATUS_WORDS[activity.verb]();
  const kind = activityKindOf(activity.verb, run);
  const target = kind === "todo" || typeof activity.target !== "string" ? "" : activity.target;
  return `${stepLook(kind).word()} ${stepTitleWords(kind, target)}`.trim();
}

function nowSaidOf(list, run) {
  return nowWordsOf(list) || nowActivityWords(nowActivityOf(run), run) || t("worker.busy", "작업 중…");
}

/* ---- a finished helper opens on its report (t-15682) --------------------------
 *
 * A helper that finished has one thing to say — the answer it ended on — and
 * its page opens on that: the report stands at the top of the list, whole,
 * and what the helper did folds under one door, its steps and thoughts and
 * what it said on the way out of sight until the door is pressed. The list
 * says so with `is-reported` (and `is-unfolded` while the door is open), so
 * no row is written to fold it. A page whose reader has gone up the list is
 * left as it stands. */
function helperReportOf(run) {
  const id = run.helper?.id;
  if (run.status !== "done" || id === WIRE_LOG_ID || id === PANE_LOG_ID) return null;
  // The report is the last thing the helper SAID. What it did after saying it — a
  // hand-in call, a closing thought — leaves it standing, so long as every call
  // came back and no person cut the helper short: a call with no answer, or the
  // person's stop, means it was cut off in the middle of its work, and what it
  // said before was not its last word. A last answer with nothing in it is no
  // report, and an earlier answer is never looked back for in its place.
  const turns = run.helper.turns;
  let closing = false;
  for (let at = turns.length - 1; at >= 0; at -= 1) {
    const turn = turns[at];
    if (turn.role === "assistant") {
      if (cleanseAssistantText(turn.text) === "") return null;
      return closing && run.helper.stopRecord === "stopped_by_person" ? null : turn;
    }
    if (turn.role === "user" || (turn.role === "tool" && turn.output === undefined)) return null;
    closing = true;
  }
  return null;
}

function helperStepCount(run) {
  return run.helper.turns.filter((turn) => turn.role === "tool" || turn.role === "tool_result").length;
}

/* What a helper did, counted for the strip under its brief (t-18702): every step once, the kinds in the
 * order each first came (a Map keeps it), how many failed. A row's kind is the one its line is drawn by
 * (`stepKindOf`), so the strip and the rows say the same words. */
function helperTally(run) {
  const kinds = new Map();
  let total = 0;
  let failed = 0;
  for (const turn of run.helper.turns) {
    if (turn.role !== "tool") continue;
    total += 1;
    const kind = stepKindOf(turn, run);
    kinds.set(kind, (kinds.get(kind) ?? 0) + 1);
    if (turn.outputError === true) failed += 1;
  }
  // The page holds the helper's last turns only (`HELPER_TURN_CAP`): once the
  // first it holds is not the file's first, the strip counts the recent steps,
  // and the head's whole count (the roster's) is a bigger number.
  const partial = run.helper.turns.length > 0 && run.helper.turns[0].seq > 0;
  return { total, failed, kinds, partial };
}

function paintReportDoor(list, card) {
  // What the report folds is what the rail stands for (t-22100).
  askTurnRail(list);
  const door = card.__door;
  const n = helperStepCount(card.__run);
  const open = list.classList.contains("is-unfolded");
  writeHidden(door, n === 0);
  writeAttribute(door, "aria-expanded", open ? "true" : "false");
  writeTextContent(door, open
    ? t("worker.reportClose", "한 일 {{n}}개 접기", { n })
    : t("worker.reportOpen", "한 일 {{n}}개 펼치기", { n }));
}

function reportNode(list, run, turn) {
  const card = document.createElement("section");
  card.className = "helper-report";
  card.setAttribute("aria-label", t("worker.report", "보고"));
  const label = document.createElement("p");
  label.className = "helper-report-label";
  label.textContent = t("worker.report", "보고");
  const said = document.createElement("div");
  said.className = "helper-said helper-report-said";
  paintAnswerProse(said, turn, run);
  const door = document.createElement("button");
  door.type = "button";
  door.className = "helper-report-door";
  door.dataset.keyboardOwner = "true";
  door.addEventListener("click", () => {
    writeClass(list, "is-unfolded", !list.classList.contains("is-unfolded"));
    paintReportDoor(list, card);
  });
  card.append(label, said, helperActionsNode(turn), door);
  card.__door = door;
  card.__run = run;
  card.__turn = turn;
  return card;
}

function dropHelperReport(list, card) {
  card.remove();
  card.__said?.classList.remove("is-said-above");
  list.__report = null;
  writeClass(list, "is-reported", false);
  writeClass(list, "is-unfolded", false);
}

/* Keeps the list's report as the run has it. Returns true when the report has
 * just been put up — the list then starts at its top, where the report is. */
function syncHelperReport(list, run) {
  const turn = helperReportOf(run);
  const held = list.__report ?? null;
  if (turn === null) {
    if (held) dropHelperReport(list, held);
    return false;
  }
  if (held?.__turn === turn) {
    paintReportDoor(list, held);
    return false;
  }
  if (!held && !chatFollows(list)) return false;
  if (held) dropHelperReport(list, held);
  const card = reportNode(list, run, turn);
  const anchor = [...list.children].find((child) => child.dataset.turn !== undefined && !child.classList.contains("is-user")) ?? null;
  list.insertBefore(card, anchor);
  list.__report = card;
  card.__said = list.querySelector(`:scope > .helper-turn.is-assistant[data-turn="${turn.seq}"]`);
  card.__said?.classList.add("is-said-above");
  writeClass(list, "is-reported", true);
  paintReportDoor(list, card);
  // The report is read from its top: the list goes there and stops following
  // its foot — unless it all fits, when there is no foot to be away from.
  list.scrollTop = 0;
  chatFollowState(list).away = chatFootGap(list) >= CHAT_FOLLOW.slack;
  paintFootDoor(list);
  return true;
}

/* ---- the turn rail (t-22100) --------------------------------------------------
 *
 * The approved conversation stands a rail of its rows at the list's left: a
 * tick for each row the list shows — the person's longer, the ones in the view
 * on the accent, a step that failed in the failure's ink — so a long
 * conversation is a length a person can see and go along. A press anywhere on
 * the rail goes to the row of the tick nearest it; the rail's handle takes the
 * keyboard (Up and Down a row, Page Up and Down a rail's worth, Home and End
 * the ends) and is announced as a slider over the rows. A long conversation
 * keeps in the page only the ticks the rail has room for — the stretch of them
 * around the rows in view — so 400 rows stand some seventy ticks, not 400.
 *
 * It is asked again when the rows change (`syncHelperTurns`), when the list
 * scrolls (once a frame) and when its box changes; it writes only what
 * changed, and never for a word that streams — a streaming row is no row of
 * the list's, and words move no row while the reader is up the list. Every
 * paint that wrote is counted (`__paints`), so that promise is a number a test
 * reads. The rows in view are found by halving the list, not by a watcher on
 * every row: an observer of 400 rows is asked about all of them on every
 * frame a stream changes the page. */
function turnRailNode(list) {
  const rail = document.createElement("div");
  rail.className = "chat-rail";
  const handle = document.createElement("div");
  handle.className = "chat-rail-handle";
  handle.setAttribute("role", "slider");
  handle.setAttribute("aria-orientation", "vertical");
  handle.setAttribute("aria-label", t("worker.railLabel", "대화의 행"));
  handle.setAttribute("aria-valuemin", "1");
  handle.tabIndex = 0;
  // Its keys are its own — the window's key sink (`rearmKeySink`) would
  // otherwise take them for the terminal.
  handle.dataset.keyboardOwner = "true";
  rail.appendChild(handle);
  rail.__list = list;
  rail.__paints = 0;
  list.__rail = rail;
  rail.addEventListener("click", (event) => {
    const tick = event.target instanceof Element ? event.target.closest(".chat-rail-tick") : null;
    const row = (tick ?? nearestRailTick(rail, event.clientY))?.__row;
    if (row?.isConnected) railJump(list, row);
  });
  handle.addEventListener("keydown", (event) => {
    const rows = railRows(list);
    if (rows.length === 0) return;
    const at = railAt(list, rows);
    const page = Math.max(1, railRoom(rail) - 1);
    const to = {
      ArrowUp: at - 1, ArrowDown: at + 1, PageUp: at - page, PageDown: at + page, Home: 0, End: rows.length - 1,
    }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    const index = Math.min(rows.length - 1, Math.max(0, to));
    list.__railAt = rows[index];
    railJump(list, rows[index]);
  });
  // The list's own scroll asks the rail once a frame; a press that left the
  // handle on a row keeps that row its value until the person moves the list.
  list.addEventListener("scroll", () => askTurnRail(list), { passive: true });
  for (const kind of ["wheel", "touchmove"]) list.addEventListener(kind, () => { list.__railAt = null; }, { passive: true });
  return rail;
}

/* The rows the rail stands for: the list's own rows that are shown — not a row
 * the Focus view folded away, and, while a finished helper's report stands
 * folded over what it did, only the person's (`is-reported`, shell.css). */
function railRows(list) {
  const folded = list.classList.contains("is-reported") && !list.classList.contains("is-unfolded");
  return [...list.querySelectorAll(":scope > [data-turn]")].filter((row) =>
    !row.hidden && !row.classList.contains("is-said-above") && (!folded || row.classList.contains("is-user")));
}

/* How many ticks the rail has room for: its height over a tick and the gap
 * after it, read off the rail's own tokens once per box. */
function railRoom(rail) {
  const height = rail.clientHeight;
  if (rail.__room?.height === height) return rail.__room.ticks;
  const style = getComputedStyle(rail);
  const gap = parseFloat(style.rowGap) || 0;
  const tick = parseFloat(style.getPropertyValue("--chat-turn-tick-h")) || 0;
  const inner = height - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
  const ticks = tick + gap > 0 ? Math.max(0, Math.floor((inner + gap) / (tick + gap))) : 0;
  rail.__room = { height, ticks };
  return ticks;
}

/* The rows in view, as a stretch of `rows`: the first whose box reaches below
 * the list's top and the last whose box starts above its foot, found by halving
 * (a person's row stuck at the top stands in its turn's first rows' place, and
 * is not looked at). `null` while the list has no box. */
function railInView(list, rows) {
  const view = list.getBoundingClientRect();
  if (view.height === 0 || rows.length === 0) return null;
  const plain = rows.filter((row) => !row.classList.contains("is-user"));
  const firstBelow = (top) => {
    let low = 0;
    let high = plain.length;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (plain[mid].getBoundingClientRect().bottom > top) high = mid;
      else low = mid + 1;
    }
    return low;
  };
  const from = plain.length > 0 ? firstBelow(view.top) : 0;
  const to = plain.length > 0 ? firstBelow(view.bottom) : 0;
  const first = plain[Math.min(from, plain.length - 1)];
  const last = plain[Math.max(0, Math.min(to, plain.length - 1))];
  let start = first ? rows.indexOf(first) : 0;
  let end = last ? rows.indexOf(last) : rows.length - 1;
  if (last && last.getBoundingClientRect().top >= view.bottom) end -= 1;
  // The person's row that heads the first row in view stands at the top.
  while (start > 0 && rows[start - 1].classList.contains("is-user")) start -= 1;
  if (plain.length === 0) end = rows.length - 1;
  return { start, end: Math.max(start, end) };
}

/* The row the handle stands on: the one a press or a key went to, while it is
 * still in view; else the first row in view. */
function railAt(list, rows) {
  const seen = railInView(list, rows);
  const held = list.__railAt ? rows.indexOf(list.__railAt) : -1;
  if (held >= 0 && seen && held >= seen.start && held <= seen.end) return held;
  return seen ? seen.start : rows.length - 1;
}

/* Ask the rail for a paint on the next frame — one a frame however often it is
 * asked. */
function askTurnRail(list) {
  const rail = list?.__rail;
  if (!rail || rail.__frame) return;
  rail.__frame = requestAnimationFrame(() => {
    rail.__frame = 0;
    if (rail.isConnected) paintTurnRail(rail);
  });
}

/* The rail as the list stands: the stretch of ticks it has room for, around
 * the rows in view, each tick worn as its row is. Writes only what changed. */
function paintTurnRail(rail) {
  const list = rail.__list;
  const rows = railRows(list);
  const room = railRoom(rail);
  const seen = railInView(list, rows) ?? { start: 0, end: -1 };
  const count = Math.min(rows.length, room);
  const middle = Math.floor((count - (seen.end - seen.start + 1)) / 2);
  const from = Math.max(0, Math.min(rows.length - count, seen.start - Math.max(0, middle)));
  let wrote = false;
  const handle = rail.firstElementChild;
  const ticks = rail.getElementsByClassName("chat-rail-tick");
  while (ticks.length > count) {
    ticks[ticks.length - 1].remove();
    wrote = true;
  }
  while (ticks.length < count) {
    const tick = document.createElement("i");
    tick.className = "chat-rail-tick";
    rail.appendChild(tick);
    wrote = true;
  }
  for (let at = 0; at < count; at += 1) {
    const tick = ticks[at];
    const index = from + at;
    const row = rows[index];
    tick.__row = row;
    const wear = `chat-rail-tick${row.classList.contains("is-user") ? " is-user" : ""}${row.classList.contains("is-failed") ? " is-failed" : ""}${index >= seen.start && index <= seen.end ? " is-current" : ""}`;
    if (tick.className !== wear) {
      tick.className = wear;
      wrote = true;
    }
  }
  const at = rows.length > 0 ? railAt(list, rows) : -1;
  const value = String(at + 1);
  if (handle.getAttribute("aria-valuemax") !== String(rows.length) || handle.getAttribute("aria-valuenow") !== value) {
    writeAttribute(handle, "aria-valuemax", String(rows.length));
    writeAttribute(handle, "aria-valuenow", value);
    writeAttribute(handle, "aria-valuetext", at < 0 ? "" : t("worker.railAt", "{{n}} / {{total}} · {{what}}", {
      n: at + 1, total: rows.length, what: railRowWords(rows[at]),
    }));
    wrote = true;
  }
  if (wrote) rail.__paints += 1;
}

/* What a row is, in a few words, for the handle's value: the person's words,
 * what a step or a thought was (not a shell step's lines), an answer's first
 * words. */
function railRowWords(row) {
  const words = row.querySelector(":scope > .helper-step-line > .helper-step-what") ?? row.querySelector(":scope > .helper-said");
  return stepFirstLine(words?.textContent ?? row.getAttribute("aria-label") ?? "");
}

/* The tick nearest a height on the rail, for a press between ticks. */
function nearestRailTick(rail, y) {
  let best = null;
  let gap = Number.POSITIVE_INFINITY;
  for (const tick of rail.getElementsByClassName("chat-rail-tick")) {
    const box = tick.getBoundingClientRect();
    const away = Math.abs(box.top + box.height / 2 - y);
    if (away < gap) {
      gap = away;
      best = tick;
    }
  }
  return best;
}

/* Go to `row`: it stands at the list's top — under the person's words of its
 * turn, which stick there — and the list stops following its foot unless the
 * row is the last one. */
function railJump(list, row) {
  const view = list.getBoundingClientRect();
  let header = null;
  if (!row.classList.contains("is-user")) {
    for (let at = row.previousElementSibling; at; at = at.previousElementSibling) {
      if (at.classList.contains("is-user")) {
        header = at;
        break;
      }
    }
  }
  const cover = header ? header.getBoundingClientRect().height : 0;
  const state = chatFollowState(list);
  state.intent = "up";
  state.at = Date.now();
  list.scrollTop += row.getBoundingClientRect().top - view.top - cover;
  state.away = chatFootGap(list) >= CHAT_FOLLOW.slack;
  paintFootDoor(list);
  askTurnRail(list);
}

/* ---- the conversation's state, over the composer (t-22100) --------------------
 *
 * The approved conversation keeps what a person would otherwise ask about
 * over the composer, in one flat stack joined to it: the agent's todo list
 * (「할 일 2/3」 and its items), the work going on in the background (the
 * helpers at work, `helperTasksOf`), the messages waiting for the turn to end
 * (the composer's own queue), and the checkout's branch and how many files
 * changed. Only facts the window already holds — the page's turns, the
 * helper roster, the queue, the worktree list and the last `git status` of
 * the checkout in front — and a fact the window does not hold is left out,
 * never guessed: a checkout other than the one in front has a branch here,
 * not a count. With nothing to say the stack is not there.
 *
 * Its button opens it to the items, one row each with its state, and the
 * choice is the conversation's (`run.stackOpen`): another tab and back, or
 * another conversation beside it, keeps each its own. It paints when the
 * page paints, when the composer's state moves and when the checkout's status
 * lands — never for a streamed word — and only when what it says changed;
 * every paint that wrote is counted (`__paints`). */
function chatStackNode(run) {
  const stack = document.createElement("section");
  stack.className = "chat-stack";
  stack.hidden = true;
  stack.setAttribute("aria-label", t("worker.stackLabel", "이 대화의 상태"));
  stack.__run = run;
  stack.__paints = 0;
  return stack;
}

/* The newest todo list the conversation's turns hold — the newest call of the
 * catalog's todo tool that did not fail — or none. */
function latestTodos(run) {
  for (let at = run.helper.turns.length - 1; at >= 0; at -= 1) {
    const turn = run.helper.turns[at];
    if (turn.role !== "tool" || turn.outputError === true) continue;
    const todos = todosOf(turn, run.agent);
    if (todos !== null) return todos;
  }
  return [];
}

/* The checkout the conversation works in: its branch from the window's
 * worktree list, and how many files changed when that checkout is the one the
 * window last asked git about (`scmStatusOf`) — `null` for a count the window
 * does not hold. No branch the window knows, no git line. */
function chatGitFacts(run) {
  const path = run.wire ? run.cwd : tabOfTerm(run.term)?.worktree ?? null;
  const worktree = worktreeAt(path);
  if (!worktree?.branch) return null;
  const changes = path === scmStatusOf ? (scmCapState?.total ?? scmEntries.length) : null;
  return { branch: worktree.branch, changes };
}

function chatStackFacts(run) {
  return {
    todos: latestTodos(run),
    helpers: helperTasksOf(run).map((task) => ({ key: task.key, label: helperTaskLabel(task) })),
    queued: run.queue?.length ?? 0,
    git: chatGitFacts(run),
    open: run.stackOpen === true,
  };
}

/* One item of the open stack: its box (the todo list's own checkbox, read out
 * as checked, mixed or empty) and its words. */
function chatStackItemNode(words, todo = null) {
  const item = document.createElement("li");
  item.className = todo ? `chat-stack-item${todo.status === "completed" ? " is-done" : ""}` : "chat-stack-item is-work";
  if (todo) {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "chat-stack-box";
    box.disabled = true;
    box.checked = todo.status === "completed";
    box.indeterminate = todo.status === "in_progress";
    item.appendChild(box);
  } else {
    const dot = document.createElement("span");
    dot.className = "chat-stack-work-dot";
    dot.setAttribute("aria-hidden", "true");
    item.appendChild(dot);
  }
  const said = document.createElement("span");
  said.className = "chat-stack-item-words";
  said.textContent = words;
  item.appendChild(said);
  return item;
}

function chatStackSpan(className, words) {
  const span = document.createElement("span");
  span.className = className;
  span.textContent = words;
  return span;
}

/* The stack as the facts stand — rebuilt only when they moved (it is a few
 * rows), the keyboard kept on its button across a rebuild. */
function paintChatStack(stack, run) {
  const facts = chatStackFacts(run);
  const said = JSON.stringify([facts, locale]);
  if (stack.__said === said) return;
  stack.__said = said;
  stack.__paints += 1;
  const { todos, helpers, queued, git, open } = facts;
  const shown = todos.length > 0 || helpers.length > 0 || queued > 0 || git !== null;
  writeHidden(stack, !shown);
  const kept = stack.contains(document.activeElement);
  if (!shown) {
    stack.replaceChildren();
    return;
  }
  const rows = [];
  if (todos.length > 0) {
    const done = todos.filter((todo) => todo.status === "completed").length;
    const row = document.createElement("div");
    row.className = "chat-stack-row is-todos";
    const box = document.createElement("span");
    box.className = `chat-stack-mark${done === todos.length ? " is-done" : done > 0 ? " is-going" : ""}`;
    box.setAttribute("aria-hidden", "true");
    row.append(
      box,
      chatStackSpan("chat-stack-todo-count", t("worker.stackTodos", "할 일 {{done}}/{{total}}", { done, total: todos.length })),
      chatStackSpan("chat-stack-todo-words", todos.map((todo) => todo.content).join(" · ")),
    );
    rows.push(row);
  }
  const factsRow = document.createElement("div");
  factsRow.className = "chat-stack-row is-facts";
  factsRow.append(
    chatStackSpan("chat-stack-work", helpers.length === 0
      ? t("worker.stackNoWork", "배경 작업 없음")
      : t("worker.stackWork", "배경 작업 {{n}}", { n: helpers.length })),
    chatStackSpan("chat-stack-sep", "·"),
    chatStackSpan("chat-stack-queue", t("worker.stackQueue", "대기 메시지 {{n}}", { n: queued })),
  );
  if (git) {
    const changes = git.changes === null ? "" : ` · ${t("worker.stackChanges", "변경 {{n}}", { n: git.changes })}`;
    factsRow.appendChild(chatStackSpan("chat-stack-git", `git ${git.branch}${changes}`));
  }
  rows.push(factsRow);
  if (todos.length > 0 || helpers.length > 0) {
    const items = document.createElement("ul");
    items.className = "chat-stack-items";
    items.id = stack.__itemsId ??= `chat-stack-items-${++chatStackSeq}`;
    items.hidden = !open;
    for (const todo of todos) items.appendChild(chatStackItemNode(todo.content, todo));
    for (const helper of helpers) items.appendChild(chatStackItemNode(helper.label));
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "chat-stack-toggle";
    toggle.dataset.keyboardOwner = "true";
    toggle.setAttribute("aria-controls", items.id);
    toggle.setAttribute("aria-expanded", open ? "true" : "false");
    toggle.setAttribute("aria-label", open ? t("worker.stackClose", "상태 접기") : t("worker.stackOpen", "상태 펼치기"));
    // One chevron, turned by the rule for its state (shell.css): up while the
    // items wait above, down once they stand.
    toggle.appendChild(iconNode("chevron"));
    toggle.addEventListener("click", () => {
      run.stackOpen = run.stackOpen !== true;
      paintChatStack(stack, run);
    });
    rows[0].appendChild(toggle);
    rows.unshift(items);
  }
  stack.replaceChildren(...rows);
  if (kept) stack.querySelector(".chat-stack-toggle")?.focus({ preventScroll: true });
}

let chatStackSeq = 0;

/* Every stack on screen, again — the checkout's status landed. */
function paintChatStacks() {
  for (const stack of document.querySelectorAll(".chat-stack")) {
    if (stack.__run) paintChatStack(stack, stack.__run);
  }
}
