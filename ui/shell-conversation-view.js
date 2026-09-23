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
 * The extension's `toolBody`: one bordered box under the call, a row per side
 * — IN what the call took, OUT what came back — each with its label in the
 * mono column. It stands when a side has more than the call line and the
 * result line already say; a side with nothing more stands hidden. Built
 * once, when the row first has something to put in it. */
function toolBodyNode() {
  const body = document.createElement("div");
  body.className = "helper-tool-body";
  for (const kind of ["input", "output"]) {
    const well = document.createElement("div");
    well.className = "helper-tool-well";
    well.hidden = true;
    const words = document.createElement("pre");
    words.className = `helper-fold-body helper-tool-${kind}`;
    well.appendChild(words);
    body.appendChild(well);
  }
  return body;
}

/* The body's two sides from the words they carry (`""` hides a side). */
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

/* The call line's target made the door, once — a poll that re-dresses a live
 * row never rebuilds it. A read's lines stand after it in the extension's
 * words: 「(11–60행)」 / 「(11행부터)」. */
function dressToolFile(row, turn, run) {
  const file = turn.tool?.file;
  if (!file?.path || row.__fileDoor) return;
  row.__fileDoor = true;
  const arg = row.querySelector(".helper-tool-arg");
  const place = toolFilePlace(file, run.agent);
  arg.classList.add("is-door");
  actsAsButton(arg, (event) => {
    event.stopPropagation();
    openPath(resolveDocPath(helperBase(run), file.path), {
      preview: true,
      line: place.line,
      search: place.line === undefined ? file.search : undefined,
    });
  });
  if (place.line === undefined) return;
  const where = document.createElement("span");
  where.className = "helper-tool-where";
  where.textContent = place.end !== undefined
    ? t("worker.readLines", "({{from}}–{{to}}행)", { from: place.line, to: place.end })
    : t("worker.readFrom", "({{from}}행부터)", { from: place.line });
  arg.after(where);
}

/* ---- the spinner's verb (t-6323 A4) -----------------------------------------
 *
 * The extension's spinner row (`Ke`, 2.1.280): while the turn is out the word
 * is one of the CLI's own verbs (`spinner_verbs` on its catalog row — Claude
 * Code's 84, the list its screen says) picked at random, picked again after
 * 2 s, 3 s more, 5 s more and every 5 s after (`cx`), and written `Verb...`;
 * each new verb is revealed by a sweep (`j75`): every 40 ms a window four
 * letters wide moves right — its lead is `▌`, then two of `.`/`_`/the letter,
 * then the letter — writing the new word over the old, across a field as wide
 * as the longest verb and its dots, which starts blank. A CLI with no verbs
 * keeps its one word. The numbers are the panel's own, held to its snapshot
 * by `the_conversation_wears_the_extensions_own_measures`. */
const STATUS_VERB = Object.freeze({ after: [2000, 3000, 5000], every: 5000, step: 40, tail: 3, suffix: "..." });

/* How long the verb stands after its `picks`-th pick. */
function statusVerbDelay(picks) {
  return STATUS_VERB.after[picks] ?? STATUS_VERB.every;
}

function randomOf(choices) {
  return choices[Math.floor(Math.random() * choices.length)];
}

/* One step of the sweep at `at`, over `text` toward `target` (both padded to
 * one width): the window's four letters, lead first. */
function statusRevealAdvance(text, target, at, pick) {
  let written = text;
  for (let back = 0; back <= STATUS_VERB.tail; back += 1) {
    const index = at - back;
    if (index < 0 || index >= target.length) continue;
    const letter = target[index];
    const glyph = letter === " " ? " " : back === STATUS_VERB.tail ? letter : back === 0 ? "▌" : pick([".", "_", letter]);
    written = written.slice(0, index) + glyph + written.slice(index + 1);
  }
  return written;
}

/* The text once the sweep from `from` to `to` has taken its steps 0…`at` —
 * the whole reveal as one pure answer (what the page draws frame by frame) —
 * over a field at least `width` wide. */
function statusRevealStep(from, to, at, pick = randomOf, width = 0) {
  width = Math.max(width, from.length, to.length);
  const target = to.padEnd(width, " ");
  let text = from.padEnd(width, " ");
  for (let step = 0; step <= at; step += 1) text = statusRevealAdvance(text, target, step, pick);
  return text;
}

/* Keep `word` turning through `verbs` while its line is out; once started it
 * runs on its own clock and stops when the line leaves the page or is hidden
 * (`stopStatusVerb`). */
function turnStatusVerb(word, verbs) {
  if (word.__verbs === verbs) return;
  stopStatusVerb(word);
  word.__verbs = verbs;
  const width = Math.max(...verbs.map((verb) => verb.length)) + STATUS_VERB.suffix.length;
  writeTextContent(word, "");
  let picks = 0;
  const pick = () => {
    if (!word.isConnected) {
      stopStatusVerb(word);
      return;
    }
    revealStatusVerb(word, `${randomOf(verbs)}${STATUS_VERB.suffix}`, width);
    word.__verbTimer = setTimeout(pick, statusVerbDelay(picks));
    picks += 1;
  };
  pick();
}

function stopStatusVerb(word) {
  clearTimeout(word.__verbTimer);
  if (word.__revealFrame) cancelAnimationFrame(word.__revealFrame);
  word.__verbTimer = null;
  word.__revealFrame = null;
  word.__verbs = null;
}

/* The sweep, one step per 40 ms on the frame clock, over a field `width`
 * wide; a page that asks for less motion — or one nobody can see — takes the
 * word at once. */
function revealStatusVerb(word, to, width) {
  if (word.__revealFrame) cancelAnimationFrame(word.__revealFrame);
  word.__revealFrame = null;
  if (document.hidden || motionReduced()) {
    writeTextContent(word, to);
    return;
  }
  width = Math.max(width, word.textContent.length, to.length);
  const target = to.padEnd(width, " ");
  let text = word.textContent.padEnd(width, " ");
  let at = 0;
  let last = -Infinity;
  const frame = (now) => {
    if (!word.isConnected) return;
    if (now - last < STATUS_VERB.step) {
      word.__revealFrame = requestAnimationFrame(frame);
      return;
    }
    last = now;
    if (at - STATUS_VERB.tail >= width) {
      word.__revealFrame = null;
      writeTextContent(word, to);
      return;
    }
    text = statusRevealAdvance(text, target, at, randomOf);
    writeTextContent(word, text);
    at += 1;
    word.__revealFrame = requestAnimationFrame(frame);
  };
  word.__revealFrame = requestAnimationFrame(frame);
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
 * (`g25`) new words keep it gliding. The extension grows no "jump to latest"
 * button, and neither does this page. The numbers, the keys and what keeps a
 * Space for itself are the panel's own, held to its snapshot by
 * `the_conversation_wears_the_extensions_own_measures`. */
const CHAT_FOLLOW = Object.freeze({ slack: 50, intent: 300, glide: 2000 });
const CHAT_FOLLOW_KEYS = Object.freeze({ up: new Set(["ArrowUp", "PageUp", "Home"]), down: new Set(["ArrowDown", "PageDown", "End"]) });
const CHAT_FOLLOW_CONTROL = "button, [role=\"button\"], input, textarea, [contenteditable]:not([contenteditable=\"false\"])";

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
    away: false, top: list.scrollTop, height: list.scrollHeight, intent: null, at: 0, touch: null, gliding: null,
  };
  noteChatScroll(list, list.__follow);
  return list.__follow;
}

/* One move of the list, read as the extension reads its `scroll`: up is
 * leaving — unless the list only shrank under a reader who never asked to go
 * up, or it stands at its very foot — and down into the last 50px is coming
 * back, unless the person was on the way up or the move only kept pace with
 * rows that grew above. */
function noteChatScroll(list, state) {
  const top = list.scrollTop;
  if (top === state.top) return;
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
      if (event.target instanceof Element && event.target.closest(CHAT_FOLLOW_CONTROL)) return;
      if (event.shiftKey) leave();
      else toward();
    } else if (CHAT_FOLLOW_KEYS.up.has(event.key)) leave();
    else if (CHAT_FOLLOW_KEYS.down.has(event.key)) toward();
  });
  list.addEventListener("scroll", () => noteChatScroll(list, state), { passive: true });
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

/* A todo call's row wears its list, once — its input never changes after the
 * call stood; an emptied list is a head alone. True when the row is a todo
 * row, so the generic body stays away. */
function dressTodoRow(row, turn, run) {
  if (row.__todos !== undefined) return row.__todos !== null;
  row.__todos = todosOf(turn, run.agent);
  if (row.__todos === null) return false;
  row.classList.add("is-todo");
  if (row.__todos.length > 0) row.appendChild(todoListNode(row.__todos));
  return true;
}

/* In the Focus view the newest todo list stands out of its fold; the one
 * before it goes back in. Asked once per paint, and it touches the page only
 * when the one that stands changes. */
function standLatestTodo(list, focus) {
  const newest = focus ? [...list.querySelectorAll(":scope > .helper-turn.is-todo:not(.is-failed)")].at(-1) : null;
  const latest = newest?.__todos?.length > 0 ? newest : null;
  const before = list.__standingTodo ?? null;
  if (before === latest) return;
  if (before) {
    before.__standing = false;
    before.classList.remove("is-standing");
    if (before.__group) writeHidden(before, !before.__group.__open);
  }
  list.__standingTodo = latest;
  if (!latest) return;
  latest.__standing = true;
  latest.classList.add("is-standing");
  writeHidden(latest, false);
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
