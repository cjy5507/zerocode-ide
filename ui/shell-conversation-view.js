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
