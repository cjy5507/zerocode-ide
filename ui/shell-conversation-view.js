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
