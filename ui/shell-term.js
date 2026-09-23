/* The one programmatic text-clipboard authority in this document.
 *
 * Tauri owns the OS boundary and its capability names only readText/writeText;
 * browser Clipboard APIs are deliberately not a fallback. Failures say only
 * which operation failed — never the clipboard contents or a native error. */
const clipboardText = Object.freeze({
  /* `null` when the read itself failed, `""` when there were no words.
   * `quiet` leaves the failure unsaid for a caller that has another road to
   * try first — the paste shortcut, which asks for a picture next. */
  async read({ quiet = false } = {}) {
    try {
      const value = await window.__TAURI__.clipboardManager.readText();
      return typeof value === "string" ? value : "";
    } catch {
      if (!quiet) showError(t("clipboard.readFailed", "클립보드의 텍스트를 읽지 못했습니다."));
      return null;
    }
  },
  async write(value) {
    try {
      await window.__TAURI__.clipboardManager.writeText(String(value));
      return true;
    } catch {
      showError(t("clipboard.writeFailed", "텍스트를 클립보드에 복사하지 못했습니다."));
      return false;
    }
  },
});

/* Orca's middle-click selection paste is not the ordinary clipboard.
 *
 * Linux owns a real PRIMARY selection, reached through the narrow native
 * commands below. macOS and other platforms keep a private buffer for this
 * window. The setting itself stores `null` as "follow the platform": enabled
 * on Linux/macOS, disabled on Windows. No selection crosses either boundary
 * after the exact Orca limits. */
const PRIMARY_SELECTION_MAX_CODE_UNITS = 65_536;
const PRIMARY_SELECTION_MAX_BYTES = 262_144;
const PRIMARY_SELECTION_CAPTURE_DELAY_MS = 100;
const PRIMARY_SELECTION_TARGET_TTL_MS = 750;
const primarySelectionEncoder = new TextEncoder();
let privatePrimarySelectionText = "";
let primarySelectionCaptureTimer = 0;
let primarySelectionNativeWrite = Promise.resolve();
let pendingPrimarySelectionTarget = null;

function boundedPrimarySelectionText(value) {
  if (typeof value !== "string" || value.length === 0) return null;
  if (value.length > PRIMARY_SELECTION_MAX_CODE_UNITS) return null;
  if (primarySelectionEncoder.encode(value).byteLength > PRIMARY_SELECTION_MAX_BYTES) return null;
  return value;
}

function primarySelectionPasteEnabled(prefs = editingPrefs) {
  if (prefs === null) return false;
  const explicit = prefs?.primary_selection_middle_click_paste;
  return typeof explicit === "boolean" ? explicit : !usesWindowsPlatform;
}

function rememberPrimarySelection(value) {
  if (!primarySelectionPasteEnabled()) return false;
  const text = boundedPrimarySelectionText(value);
  if (text === null) return false;
  privatePrimarySelectionText = text;
  if (usesLinuxPlatform) {
    primarySelectionNativeWrite = primarySelectionNativeWrite
      .then(() => invoke("write_primary_selection", { text }))
      // A compositor without PRIMARY still has the private-buffer road.
      .catch(() => {});
  }
  return true;
}

function selectedTextForPrimarySelection() {
  const active = document.activeElement;
  if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) {
    if (active instanceof HTMLInputElement && active.type === "password") return "";
    const start = active.selectionStart;
    const end = active.selectionEnd;
    if (typeof start === "number" && typeof end === "number" && end > start) {
      return active.value.slice(start, end);
    }
  }
  // A terminal's selection is the grid model's, not the document's, and it
  // is remembered by the view itself when its drag ends (`finishPointerGesture`
  // in `makeTermView`) — the text lives on the backend and arrives later.
  return window.getSelection()?.toString() ?? "";
}

function capturePrimarySelectionLater() {
  clearTimeout(primarySelectionCaptureTimer);
  primarySelectionCaptureTimer = setTimeout(() => {
    primarySelectionCaptureTimer = 0;
    rememberPrimarySelection(selectedTextForPrimarySelection());
  }, PRIMARY_SELECTION_CAPTURE_DELAY_MS);
}

function primaryEditableTarget(node) {
  const element = selectionElement(node);
  if (!element || element.closest?.(".terminal")) return null;
  const field = element.closest?.("input, textarea");
  if (field) {
    if (field.disabled || field.readOnly || typeof field.selectionStart !== "number") return null;
    return field;
  }
  const editable = element.closest?.("[contenteditable]");
  return editable?.isContentEditable ? editable : null;
}

function editorAtPrimaryTarget(target) {
  for (const held of editorViews.values()) {
    if (held.editor.contentDOM === target || held.editor.contentDOM.contains(target)) {
      return held.editor;
    }
  }
  return null;
}

function dispatchPrimarySelectionInput(target, text) {
  const init = { bubbles: true, inputType: "insertFromPaste", data: text };
  target.dispatchEvent(typeof InputEvent === "function"
    ? new InputEvent("input", init)
    : new Event("input", { bubbles: true }));
}

/* Capture the insertion point before the native read begins. A slow Linux
 * selection owner must not redirect its answer into whichever control became
 * focused while IPC was in flight. */
function capturePrimaryPasteDestination(target) {
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    const start = target.selectionStart;
    const end = target.selectionEnd;
    if (typeof start !== "number" || typeof end !== "number") return null;
    return (text) => {
      if (!target.isConnected || target.disabled || target.readOnly) return false;
      target.focus();
      target.setRangeText(text, start, end, "end");
      dispatchPrimarySelectionInput(target, text);
      return true;
    };
  }

  const editor = editorAtPrimaryTarget(target);
  if (editor) {
    const { from, to } = editor.state.selection.main;
    return (text) => {
      if (!editor.dom.isConnected) return false;
      editor.dispatch({
        changes: { from, to, insert: text },
        selection: { anchor: from + text.length },
        userEvent: "input.paste",
      });
      editor.focus();
      return true;
    };
  }

  const selection = window.getSelection();
  let range = null;
  if (selection?.rangeCount) {
    const candidate = selection.getRangeAt(0);
    if (target.contains(candidate.commonAncestorContainer)) range = candidate.cloneRange();
  }
  if (range === null) {
    range = document.createRange();
    range.selectNodeContents(target);
    range.collapse(false);
  }
  return (text) => {
    if (!target.isConnected || !target.isContentEditable) return false;
    try {
      const node = document.createTextNode(text);
      range.deleteContents();
      range.insertNode(node);
      range.setStartAfter(node);
      range.collapse(true);
      const current = window.getSelection();
      current?.removeAllRanges();
      current?.addRange(range);
      target.focus();
      dispatchPrimarySelectionInput(target, text);
      return true;
    } catch {
      return false;
    }
  };
}

async function readPrimarySelectionText() {
  if (!primarySelectionPasteEnabled()) return "";
  if (usesLinuxPlatform) {
    await primarySelectionNativeWrite;
    try {
      const nativeText = boundedPrimarySelectionText(await invoke("read_primary_selection"));
      if (nativeText !== null) return nativeText;
    } catch {
      // The private selection remains the exact fallback Orca uses when the
      // Linux compositor does not expose PRIMARY.
    }
  }
  return privatePrimarySelectionText;
}

async function pastePrimarySelectionInto(destination) {
  const text = await readPrimarySelectionText();
  return text !== "" && destination(text);
}

function clearPrivatePrimarySelection() {
  privatePrimarySelectionText = "";
  pendingPrimarySelectionTarget = null;
  clearTimeout(primarySelectionCaptureTimer);
  primarySelectionCaptureTimer = 0;
}

function installPrimarySelectionPaste() {
  document.addEventListener("selectionchange", capturePrimarySelectionLater, true);
  document.addEventListener("mouseup", capturePrimarySelectionLater, true);
  document.addEventListener("keyup", capturePrimarySelectionLater, true);
  document.addEventListener("select", capturePrimarySelectionLater, true);

  document.addEventListener("mousedown", (event) => {
    if (event.button !== 1) return;
    const target = primaryEditableTarget(event.target);
    if (target === null) return;
    const enabled = primarySelectionPasteEnabled();
    // Linux browsers have their own native middle-paste. Remember the target
    // even while disabled so the setting can suppress that second owner.
    if (!enabled && !usesLinuxPlatform) return;
    pendingPrimarySelectionTarget = {
      target,
      enabled,
      expiresAt: performance.now() + PRIMARY_SELECTION_TARGET_TTL_MS,
    };
  }, true);

  const suppressNativePaste = (event) => {
    const pending = pendingPrimarySelectionTarget;
    if (!pending || performance.now() > pending.expiresAt) return;
    if (event.type === "beforeinput" && event.inputType !== "insertFromPaste") return;
    event.preventDefault();
    event.stopPropagation();
  };
  document.addEventListener("beforeinput", suppressNativePaste, true);
  document.addEventListener("paste", suppressNativePaste, true);

  document.addEventListener("mouseup", (event) => {
    if (event.button !== 1) return;
    const pending = pendingPrimarySelectionTarget;
    pendingPrimarySelectionTarget = null;
    if (!pending || performance.now() > pending.expiresAt || !pending.target.isConnected) return;
    event.preventDefault();
    if (!pending.enabled) return;
    const destination = capturePrimaryPasteDestination(pending.target);
    if (destination) void pastePrimarySelectionInto(destination);
  }, true);

  document.addEventListener("auxclick", (event) => {
    if (event.button !== 1 || primaryEditableTarget(event.target) === null) return;
    if (primarySelectionPasteEnabled() || usesLinuxPlatform) event.preventDefault();
  }, true);
}

/* OSC 52 is a terminal side effect, not a screen frame. The Rust grid has
 * already validated, bounded, decoded and coalesced each terminal's request;
 * this final one-slot queue keeps writes ordered across terminals without
 * allowing a fast TUI to build an unbounded promise backlog. */
let pendingOsc52ClipboardText = null;
let osc52ClipboardFlushRunning = false;
let osc52ClipboardFlushScheduled = false;
let osc52BlockedNoticeShown = false;

async function flushOsc52ClipboardWrite() {
  if (osc52ClipboardFlushRunning) return;
  osc52ClipboardFlushRunning = true;
  while (pendingOsc52ClipboardText !== null) {
    const text = pendingOsc52ClipboardText;
    pendingOsc52ClipboardText = null;
    await clipboardText.write(text);
  }
  osc52ClipboardFlushRunning = false;
}

function scheduleOsc52ClipboardWrite() {
  if (osc52ClipboardFlushRunning || osc52ClipboardFlushScheduled) return;
  osc52ClipboardFlushScheduled = true;
  queueMicrotask(() => {
    osc52ClipboardFlushScheduled = false;
    void flushOsc52ClipboardWrite();
  });
}

function acceptOsc52ClipboardWrite(text) {
  // An event can race the initial settings snapshot. Orca treats an unknown
  // gate as blocked without surfacing it, so startup never grants permission
  // from a guessed default.
  if (termPrefs?.allow_osc52_clipboard !== true) {
    if (termPrefs !== null && !osc52BlockedNoticeShown) {
      osc52BlockedNoticeShown = true;
      showError(t(
        "settings.terminal.osc52Blocked",
        "터미널 프로그램의 클립보드 쓰기를 차단했습니다. 허용하려면 터미널 설정에서 OSC 52를 켜세요.",
      ));
    }
    return;
  }
  pendingOsc52ClipboardText = String(text);
  scheduleOsc52ClipboardWrite();
}

/* ---- whose selection a copy means ----
 *
 * A terminal's selection is not the document's. It is a model of the grid
 * (`makeTermSelection`, ui/shell-term-selection.js) held by the view that
 * painted it, and nothing inside a `.term` is browser-selectable at all
 * (`user-select: none`) — the browser's selection lived in Text nodes the
 * renderer rewrites on every frame and every scroll, and a scroll took it
 * away (「zo에서 선택 드래그 유지가 안 됨, 스크롤을 내리면…」, 2026-09-08).
 * So every road that used to ask `window.getSelection()` which terminal it
 * was in — ⌘C, the copy event, the pane menu, copy-on-select, the link
 * gesture's "did a selection stand" — asks here instead, by the terminal's
 * address key, and every view that exists is registered below.
 *
 * The text is asked for LAST, and from the backend: a selection dragged past
 * the top of the pane names history the window never held. */
const termSelectionViews = new Set();
const terminalSelectionByHost = new WeakMap();

function selectionElement(node) {
  if (!node) return null;
  return node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
}

/* The address key a keyboard target answers to — the spelling every view's
 * `owner.address().key` already uses. */
function terminalTargetKey(target) {
  if (!target) return null;
  return target.kind === "term" ? `term:${target.term}` : `lane:${target.id}`;
}

/* The view whose selection stands for `key`, or null. */
function terminalSelectionView(key) {
  if (!key) return null;
  for (const view of termSelectionViews) {
    if (view.selectionStands() && view.addressKey() === key) return view;
  }
  return null;
}

/* Whether a selection stands in the terminal `key` names. Synchronous, so a
 * menu can enable its copy row and Ctrl+C can decide between copy and SIGINT
 * without a round trip. */
function terminalSelectionStands(key) {
  return terminalSelectionView(key) !== null;
}

/* Copy the selection standing in the terminal `key` names, through the one
 * clipboard adapter. Answers whether anything was written. The text comes
 * from the backend (`term_lines`), which holds every line the selection can
 * name — see `selectionText` in `makeTermView`. */
async function copyTerminalSelection(key) {
  const view = terminalSelectionView(key);
  if (view === null) return false;
  const text = await view.selectionText();
  return text !== "" && clipboardText.write(text);
}

/* Whether a selection stands in the terminal whose host this is — the
 * link gesture's question (`canRequestTermLinkAction`): a click that ends a
 * selection is not a request to open what it landed on. */
function terminalSelectionStandsIn(host) {
  return host ? terminalSelectionByHost.get(host)?.selectionStands() === true : false;
}

/* Escape clears what stands in every visible terminal. A person clears a
 * selection with a click or with Escape and with nothing else — a frame, a
 * scroll or a focus move never does (the model's whole reason to exist). */
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const view of termSelectionViews) {
    if (!view.host.hidden) view.clearSelection();
  }
});

/* Both native paste events and programmatic clipboard reads enter through the
 * existing backend paste commands. Those commands own bracketed-paste and
 * control-sequence sanitizing; JavaScript never writes clipboard text to a PTY
 * directly. */
async function pasteTextAt(target, text) {
  if (!target || text === "") return false;
  hideTerminalPointerForInput(target);
  try {
    if (target.kind === "term") await invoke("term_paste", { term: target.term, text });
    else await invoke("paste_input", { id: target.id, text });
    return true;
  } catch {
    showError(t("clipboard.pasteFailed", "클립보드의 텍스트를 붙여넣지 못했습니다."));
    return false;
  }
}

/* ---- OS 파일 드롭 → 터미널 ----
 *
 * Tauri owns native file drags (`dragDropEnabled` is the webview default), so
 * a file dragged in from the OS never reaches an HTML5 drop handler — it
 * arrives as `tauri://drag-drop` carrying absolute paths and PHYSICAL
 * coordinates. The original routes such a drop to the terminal pane under the
 * cursor and writes the paths into its PTY (terminal-drop-path-writer.ts): a
 * safe image path travels as a bracketed paste so a TUI (Claude Code, Codex)
 * reads it as an attachment — mirroring its clipboard screenshot flow — and
 * every other path is shell-escaped with a trailing separating space, ready
 * to sit in a command line. Rust owns bracketed framing and sanitizing here
 * (`term_paste`/`paste_input` → encode_paste), so both lanes ride
 * `pasteTextAt`, the one paste door this file already has — terminal tabs,
 * the floating shell and lanes alike. */
const IMAGE_DROP_EXTENSIONS = new Set([
  ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".ico",
]);
/* The original's unsafe alphabet (terminal-drop-image-path.ts): a raw path
 * must survive the shell it may echo through and the TUI's file-existence
 * probe, so any quoting-relevant byte sends the image down the escaped lane
 * instead. Windows adds `^%` and forgives its own path separator. */
const RAW_IMAGE_DROP_UNSAFE = usesWindowsPlatform
  ? /["'`$;&|<>(){}[\]*?!#^%]/
  : /["'`$;&|<>(){}[\]*?!#\\]/;

function isImageDropPath(path) {
  const lastDot = path.lastIndexOf(".");
  if (lastDot === -1) return false;
  if (lastDot < Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))) return false;
  return IMAGE_DROP_EXTENSIONS.has(path.slice(lastDot).toLowerCase());
}

function canPasteImageDropPathRaw(path) {
  for (let i = 0; i < path.length; i += 1) {
    const code = path.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return false;
  }
  return !RAW_IMAGE_DROP_UNSAFE.test(path);
}

/* The original's shellEscapePath (pane-helpers.ts:66-76): calm bytes stay
 * bare; anything else rides quotes, with the POSIX spelling for a quote
 * inside quotes. */
function shellEscapeDropPath(path) {
  if (usesWindowsPlatform) {
    return /^[a-zA-Z0-9_./@:\\-]+$/.test(path) ? path : `"${path}"`;
  }
  if (/^[a-zA-Z0-9_./@:-]+$/.test(path)) return path;
  return `'${path.replace(/'/g, "'\\''")}'`;
}

function terminalDropTargetAt(x, y) {
  /* The whole stack under the point, not just its top: a toast or another
   * decoration hovering over a pane must not eat a drop aimed at the terminal
   * visibly under it. A dialog is different — it hides what it covers, so a
   * pane behind one is not a target. */
  for (const node of document.elementsFromPoint(x, y)) {
    if (node.closest?.('dialog, [role="dialog"]')) return null;
    const host = node.closest?.(".terminal");
    if (host) return terminalSelectionByHost.get(host)?.address() ?? null;
  }
  return null;
}

async function dropPathsIntoTerminal(target, paths) {
  for (let i = 0; i < paths.length; i += 1) {
    const path = paths[i];
    if (isImageDropPath(path) && canPasteImageDropPathRaw(path)) {
      if (!(await pasteTextAt(target, path))) return;
      /* An image payload carries no separator of its own: only a following
       * escaped path needs one — image after image is self-delimiting, and a
       * stray space between attachments would land in the TUI's input. */
      const next = paths[i + 1];
      if (next !== undefined && !(isImageDropPath(next) && canPasteImageDropPathRaw(next))) {
        if (!(await pasteTextAt(target, " "))) return;
      }
    } else if (!(await pasteTextAt(target, `${shellEscapeDropPath(path)} `))) {
      return;
    }
  }
}

listen("tauri://drag-drop", (event) => {
  const paths = event.payload?.paths;
  if (!Array.isArray(paths) || paths.length === 0) return;
  const scale = window.devicePixelRatio || 1;
  const position = event.payload.position ?? { x: 0, y: 0 };
  const x = position.x / scale;
  const y = position.y / scale;
  // 입력줄 위에 떨어지면 칩이 된다(t-2993, shell-attach.js) — 판에 붙는 것이
  // 아니라. 한 리스너가 과녁을 둘 아는 것이지 두 번째 리스너가 아니다.
  if (typeof sftpNativeDrop === "function" && sftpNativeDrop(paths, x, y)) return;
  const composer = composerAttachTargetAt(x, y);
  if (composer) {
    void composer.add(paths);
    return;
  }
  const target = terminalDropTargetAt(x, y);
  if (!target) return;
  void dropPathsIntoTerminal(target, paths);
});

async function pasteClipboardAt(target) {
  // Held before the read: focus/tab changes while the native clipboard call is
  // pending must not redirect text into the terminal that became active later.
  const captured = target ? { ...target } : null;
  if (!captured) return false;
  const text = await clipboardText.read({ quiet: true });
  if (text) return pasteTextAt(captured, text);
  // No words — or a clipboard the text road could not read at all, which is
  // what a clipboard holding only a picture looks like to it. The `paste`
  // event road sees the picture the webview was handed; this road is the
  // shortcut's, taken with the focus on the body after a click or a drag in
  // the pane, and it read text alone — so a screenshot pasted there went
  // nowhere. Ask the backend for the picture and paste its path: the same
  // temp file the event road would have written.
  const path = await invoke("save_clipboard_image").catch(() => null);
  if (typeof path === "string" && path.length > 0) return pasteTextAt(captured, path);
  if (text === null) showError(t("clipboard.readFailed", "클립보드의 텍스트를 읽지 못했습니다."));
  return false;
}

/* What was said into a keyed element, and how to say it again.
 *
 * Held as a way to PRODUCE the words rather than as the words, so a language
 * change can ask for them a second time and get them in the language now in
 * force. Keyed off the element itself: nothing else can name it, and an
 * element that goes away takes its entry with it. */
/* Held twice on purpose. The map is the lookup `applyLocale` does per node;
 * the set is how a language change finds them all — a `WeakMap` cannot be
 * walked, and the elements that need this most are exactly the ones with no
 * key in the markup to be found by. */
const spoken = new WeakMap();

/* The walkable half holds HANDLES on the elements, never the elements.
 *
 * This set is the one structure in the window that a repaint feeds and only a
 * LANGUAGE CHANGE drains, and almost every caller below is a list that redraws
 * itself: the worktree rows, the changed-file sections, the cleanup table, the
 * external-repository cards. Each pass builds fresh elements, says a sentence
 * into each, and `replaceChildren` throws the previous pass out of the
 * document — but a plain `Set` went on holding them, so every detached subtree
 * from every repaint since boot stayed reachable from here. Nobody switches
 * language in a working day, so in practice it never drained at all.
 *
 * A `WeakSet` cannot be walked, which is the whole reason this is not one. So:
 * a set of `WeakRef`s. The element is reachable from here only while something
 * ELSE still holds it — a repaint's leftovers are collectable the moment the
 * document lets go — and `applyLocale` derefs on its way past. `say`'s own
 * behaviour is untouched: nothing here decides an element is finished, which
 * is what a sweep on "is it still in the document" would have to decide, and
 * it would decide it wrongly for every caller that says its sentence BEFORE
 * appending the node (`addExternalCardAction`, `paintWorktreeBranchList`, …).
 *
 * `spokenRefs` is how `say(node, null)` and the sweep find the handle again —
 * a second `WeakRef` for the same element would be a second entry that no
 * road could ever remove. The registry is what drains the set without waiting
 * for a language change: the collector hands back the handle whose element is
 * gone, and the only thing left to do with it is drop it. */
const spokenNodes = new Set();
const spokenRefs = new WeakMap();
const spokenGone = new FinalizationRegistry((ref) => spokenNodes.delete(ref));

/* Say something into an element the markup gave a translation key.
 *
 * `applyLocale` redraws a keyed element FROM its key, so a sentence written
 * straight into one lasts exactly until the next language change — which puts
 * the ORIGINAL back over it. That is how the note under the terminal command
 * lost a save's answer, and how a failure the server sent back was wiped off
 * the screen entirely rather than translated.
 *
 * `produce` is recorded and `applyLocale` asks it instead of the key.
 * `say(node, null)` hands the element back to its key.
 *
 * Always written as text, never as markup: what is said this way can be what
 * a server sent back. The one sentence in this window that needs shape — the
 * stage's placeholder, with its `<kbd>` — is drawn by `paintStagePlaceholder`,
 * which `setLocale` calls for that reason.
 *
 * The text is written HERE rather than left to `applyLocale`. That worked
 * only for elements the markup had already given a key, because `applyLocale`
 * walks `[data-i18n]` and nothing else — so an error line with no key of its
 * own recorded its sentence and then displayed nothing at all. The elements
 * that most need this are precisely the keyless ones. */
function say(node, produce) {
  if (produce === null) {
    spoken.delete(node);
    forgetSpoken(node);
    return;
  }
  spoken.set(node, produce);
  // One handle per element, made once. Said into twice — which every repainted
  // row does — this must not leave a second entry behind, because the element
  // is the only name either entry has and dropping it drops one of them.
  if (!spokenRefs.has(node)) {
    const ref = new WeakRef(node);
    spokenRefs.set(node, ref);
    spokenNodes.add(ref);
    spokenGone.register(node, ref, node);
  }
  node.textContent = produce();
}

/* Take an element off the walkable half. Both halves of the handle go: the
 * set's entry, and the element's way back to it — kept together so a later
 * `say` into the same element makes a fresh handle rather than finding a stale
 * one that no longer stands in the set. */
function forgetSpoken(node) {
  const ref = spokenRefs.get(node);
  if (ref === undefined) return;
  spokenNodes.delete(ref);
  spokenRefs.delete(node);
  spokenGone.unregister(node);
}

/* The static markup, re-read in the language in force.
 *
 * Every translatable node carries its key AND its Korean, so the document is
 * readable as written and this only has to swap text when the language is not
 * the source. The Korean already in the node is the fallback, which means a
 * key nobody translated cannot blank an element. */
function applyLocale(root = document) {
  // The root counts as one of its own. `querySelectorAll` only ever answers
  // about descendants, so naming a single element — which is how the stage's
  // message is put back into the language in force — would match nothing and
  // silently do nothing at all.
  const tagged = (attribute) => {
    const found = [...root.querySelectorAll(`[${attribute}]`)];
    if (root.nodeType === 1 && root.hasAttribute(attribute)) found.unshift(root);
    return found;
  };
  // Something was said into this element that its key does not describe — a
  // save's answer, a project's name, a failure. `say` recorded a way to
  // produce those words, so they are said again here rather than overwritten
  // by the key's own sentence.
  const overridden = (node) => {
    const produce = spoken.get(node);
    if (!produce) return false;
    node.textContent = produce();
    return true;
  };
  const swap = (attribute, set) => {
    for (const node of tagged(attribute)) {
      const key = node.getAttribute(attribute);
      if (!key) continue;
      if (overridden(node)) continue;
      if (!node.dataset.i18nSource) node.dataset.i18nSource = set.read(node);
      set.write(node, t(key, node.dataset.i18nSource));
    }
  };
  swap("data-i18n", {
    read: (node) => node.textContent,
    write: (node, text) => {
      node.textContent = text;
    },
  });
  // A sentence with shape inside it — a `<kbd>` for the chord, `<code>` for
  // the programs it names. `textContent` would flatten those away, so this
  // one takes markup. The source is this file's own catalog and never
  // anything a person or a program supplied, which is the only reason
  // assigning `innerHTML` here is not a way in.
  for (const node of tagged("data-i18n-html")) {
    if (overridden(node)) continue;
    const key = node.getAttribute("data-i18n-html");
    if (!node.dataset.i18nSourceHtml) node.dataset.i18nSourceHtml = node.innerHTML;
    node.innerHTML = t(key, node.dataset.i18nSourceHtml);
  }
  for (const [attribute, name] of [
    // `data-tip`, not `title`: the tooltip this window draws is its own
    // surface (see the tooltip section), and the key that names the words is
    // unchanged — only the attribute they land in moved.
    ["data-i18n-title", "data-tip"],
    ["data-i18n-aria", "aria-label"],
    ["data-i18n-placeholder", "placeholder"],
  ]) {
    for (const node of tagged(attribute)) {
      const key = node.getAttribute(attribute);
      const store = `i18nSource${name.replace(/[^a-z]/g, "")}`;
      if (!node.dataset[store]) node.dataset[store] = node.getAttribute(name) ?? "";
      node.setAttribute(name, t(key, node.dataset[store]));
    }
  }
  // And everything `say` is holding, whether or not the markup gave it a key.
  // An element dropped from the document is dropped from here with it — the
  // collector drains the set on its own now, but a language change is already
  // walking every handle, and a detached element it finds on the way is one
  // the collector has simply not reached yet.
  for (const ref of spokenNodes) {
    const node = ref.deref();
    if (node === undefined) {
      spokenNodes.delete(ref);
      continue;
    }
    if (!node.isConnected) {
      spoken.delete(node);
      forgetSpoken(node);
      continue;
    }
    if (root === document || root === node || root.contains?.(node)) {
      node.textContent = spoken.get(node)();
    }
  }
}

/* Serde spellings (zerocode-core LaneState) → the prototype's visual grammar. */
const STATE_CLASS = {
  idle: "idle",
  streaming: "streaming",
  awaiting_permission: "waiting",
  blocked: "blocked",
  exited: "exited",
};
/* What an agent's own report is called in that same grammar.
 *
 * The hook road (`hookStates`, keyed by shell) and the lane registry are two
 * ledgers about the same thing — somebody working in this checkout — and the
 * workspace dot has to weigh them against each other. Said once here rather
 * than as a second rank table beside `WORKTREE_STATE_RANK`: two rankings of
 * "which of these is worse" is how a card ends up disagreeing with the dot
 * above it. */
/* `idle` is the state no agent ever sends — the backend OBSERVES it, when the
 * process that was holding a terminal stops holding it and no `Stop` ever came
 * (`zerocode_core::agent_exit`). It reads exactly like `done` here on purpose:
 * to a person looking at a workspace dot, "finished" and "left" are one
 * sentence — nothing is running in there. The difference between the two is a
 * notification, and that decision is made where notifications are. */
const HOOK_STATE_CLASS = { working: "streaming", "needs-attention": "waiting", paused: "paused", done: "idle", idle: "idle" };

/* 일이 흐르고 있다는 말들 — 두 원장 각각의 어휘로, 한 번씩만.
 *
 * 사이드바 카드와 레인 요약이 같은 물음("접어도 되는가")을 서로 다른 어휘로
 * 묻는다: 훅 원장은 working/needs-attention을, 레인 원장은 STATE_CLASS의
 * streaming/waiting/blocked를 말한다. 이 둘을 각 자리에서 배열 리터럴로
 * 다시 적으면 상태 하나가 늘 때 두 요약이 조용히 갈라진다 — 그래서 어휘 표
 * 옆에 이름으로 선다. */
const LIVE_HOOK_STATES = new Set(["working", "needs-attention"]);
const LIVE_LANE_CLASSES = new Set(["streaming", "waiting", "blocked"]);

/* Which state wins when several are in one worktree. The worst one, so a
 * checkout with an agent waiting on a person never reports the calm of the one
 * beside it.
 *
 * `exited` is not here: a lane that has ended says nothing about the checkout
 * it ended in — it falls to `empty`, which is the rank it deserves and also
 * the only spelling `worktreeStateLabel` can say out loud.
 *
 * `active` sits between "nothing here" and "an agent finished": a shell alive
 * in this checkout with nothing to report is Orca's `active` (see
 * `foldWorktreeState`), and any agent that HAS reported outranks it. */
const WORKTREE_STATE_RANK = { empty: 0, active: 1, idle: 2, paused: 3, streaming: 4, waiting: 5, blocked: 6 };

/* Our six, folded onto the five Orca's dot actually draws
 * (`getWorktreeStatus`, worktree-status-BKo1bx-D.js:404 — `permission >
 * working > active > inactive`, with `done` composed in from the agent
 * summaries by `resolveWorktreeStatus`).
 *
 * The finer vocabulary is kept UNDER this on purpose: `waiting` ("waiting for
 * you") and `blocked` ("needs permission") are one amber question mark to the
 * eye and two different sentences to a screen reader, and folding them in the
 * ledger rather than at the pixel would throw the sentence away. */
const WORKTREE_INDICATOR = {
  empty: "inactive",
  active: "active",
  idle: "done",
  paused: "paused",
  streaming: "working",
  waiting: "permission",
  blocked: "permission",
};

/* What a lane's state is called. A function rather than the map this was,
 * for the reason the map could not be: a `const` of `t()` calls resolves at
 * load, before a language has been chosen, and freezes the window in Korean.
 * The catalog has carried `session.idle` and its four siblings all along —
 * nothing read them, so four translations shipped that nothing could reach. */
function laneStateLabel(state) {
  switch (state) {
    case "idle":
      return t("session.idle", "대기");
    case "streaming":
      return t("session.streaming", "스트리밍");
    case "awaiting_permission":
      return t("session.awaiting", "권한 대기");
    case "blocked":
      return t("session.blocked", "막힘");
    case "exited":
      return t("session.exited", "종료됨");
    default:
      return state;
  }
}
const GLYPH = {
  zo: "ZO",
  claude: "CL",
  codex: "CX",
  cursor: "CU",
  droid: "DR",
  copilot: "CP",
  grok: "GK",
  kimi: "KM",
  devin: "DV",
  antigravity: "AG",
  opencode: "OC",
  "command-code": "CC",
};

/* ---- view bookkeeping ---- */

const lanes = new Map(); // lane id -> { lane, row }
const order = []; // rail order = insertion order, mirrors the registry
let focusedId = null;
let zoAvailable = false;

function slotClass(id) {
  return `lane-${(order.indexOf(id) % 5) + 1}`;
}

/* Give a row the three things the button it already behaves like has: a tab
 * stop, a role, and Enter/Space.
 *
 * These rows are laid out with grid and flex templates that a real `<button>`
 * fights, so they stay divs — but a div with only a click handler does not
 * exist for anybody navigating by keyboard. It cannot be reached and Enter
 * does nothing on it, which for this window means the file tree, the worktree
 * list, the thread list, the tabs and the source-control rows are all
 * mouse-only. One helper rather than a line in each builder, so the next row
 * somebody adds gets it by using the same call. */
function actsAsButton(node, run) {
  node.tabIndex = 0;
  node.setAttribute("role", "button");
  node.addEventListener("click", run);
  node.addEventListener("keydown", (event) => {
    // A real button nested in a composite row owns its own Enter/Space. Letting
    // that key bubble into the row would run both the action and the project
    // switch behind it.
    if (event.target !== node) return;
    if (event.key !== "Enter" && event.key !== " ") return;
    // Space scrolls the page and Enter would fall through to the window's
    // terminal routing; this row is what was activated, so it consumes both.
    event.preventDefault();
    run(event);
  });
}

/* One glyph from the sprite in index.html.
 *
 * Returns markup rather than a node so it composes into the `innerHTML` every
 * row in this file is already built with. Safe by construction: `name` is only
 * ever a literal from this file — no caller passes anything a person typed.
 *
 * `open` is for the disclosure chevron, which is one glyph rotated rather than
 * two that could drift apart. */
function icon(name, open = false) {
  const twist = name === "chevron" ? ` icon--twist${open ? " is-open" : ""}` : "";
  return `<svg class="icon${twist}" aria-hidden="true"><use href="#i-${name}"></use></svg>`;
}

/* Which face a file wears, measured whole from Orca's resolver
 * (file-type-icons-Uuu7rMFP.js: a 73-entry exact-name map, a 173-entry
 * extension map, four filename rules between them, three compound
 * extensions, 21 lucide glyphs). The artwork is lucide (MIT) — the one
 * kind of drawing in Orca we may copy; names and strings stay ours.
 * Sprite ids are `i-ft-*` in index.html, except the default, which is
 * the `i-file` already there (Orca's File path data is byte-identical). */
const FILE_ICON_BY_NAME = {
  ".babelrc": "ft-file-sliders", ".dockerignore": "ft-file-sliders", ".editorconfig": "ft-file-sliders",
  ".eslintrc": "ft-file-sliders", ".eslintrc.cjs": "ft-file-sliders", ".eslintrc.js": "ft-file-sliders",
  ".eslintrc.json": "ft-file-braces", ".eslintrc.yaml": "ft-file-sliders", ".eslintrc.yml": "ft-file-sliders",
  ".gitattributes": "ft-file-sliders", ".gitignore": "ft-file-sliders", ".npmrc": "ft-file-sliders",
  ".prettierrc": "ft-file-sliders", ".prettierrc.json": "ft-file-braces", ".prettierrc.yaml": "ft-file-sliders",
  ".prettierrc.yml": "ft-file-sliders", "agents.md": "ft-file-text", "authors": "ft-file-text",
  "bun.lock": "ft-file-box", "bun.lockb": "ft-file-box", "cargo.lock": "ft-file-box",
  "cargo.toml": "ft-file-box", "changelog": "ft-file-text", "changelog.md": "ft-file-text",
  "cmakelists.txt": "ft-file-cog", "codeowners": "ft-file-key", "components.json": "ft-file-sliders",
  "composer.json": "ft-file-box", "composer.lock": "ft-file-box", "contributing": "ft-file-text",
  "contributing.md": "ft-file-text", "copying": "ft-file-key", "dockerfile": "ft-file-cog",
  "gemfile": "ft-file-box", "go.mod": "ft-file-box", "go.sum": "ft-file-box",
  "license": "ft-file-key", "makefile": "ft-file-terminal", "meson.build": "ft-file-cog",
  "notice": "ft-file-key", "package-lock.json": "ft-file-box", "package.json": "ft-file-box",
  "pipfile": "ft-file-box", "pnpm-lock.yaml": "ft-file-box", "pnpm-workspace.yaml": "ft-file-box",
  "poetry.lock": "ft-file-box", "pom.xml": "ft-file-box", "postcss.config.cjs": "ft-file-sliders",
  "postcss.config.js": "ft-file-sliders", "postcss.config.mjs": "ft-file-sliders", "postcss.config.ts": "ft-file-sliders",
  "pyproject.toml": "ft-file-box", "readme": "ft-file-text", "readme.md": "ft-file-text",
  "requirements-dev.txt": "ft-file-box", "requirements.txt": "ft-file-box", "security": "ft-file-lock",
  "security.md": "ft-file-lock", "settings.gradle": "ft-file-cog", "settings.gradle.kts": "ft-file-cog",
  "tailwind.config.cjs": "ft-file-sliders", "tailwind.config.js": "ft-file-sliders", "tailwind.config.mjs": "ft-file-sliders",
  "tailwind.config.ts": "ft-file-sliders", "todo": "ft-file-text", "tsconfig.json": "ft-file-sliders",
  "vite.config.js": "ft-file-sliders", "vite.config.mjs": "ft-file-sliders", "vite.config.ts": "ft-file-sliders",
  "vitest.config.js": "ft-file-sliders", "vitest.config.mjs": "ft-file-sliders", "vitest.config.ts": "ft-file-sliders",
  "yarn.lock": "ft-file-box",
};

const FILE_ICON_BY_EXTENSION = {
  "7z": "ft-file-archive", "aac": "ft-file-music", "adoc": "ft-file-text",
  "ai": "ft-file-image", "asc": "ft-file-key", "astro": "ft-file-code",
  "avi": "ft-file-play", "avif": "ft-file-image", "bash": "ft-file-terminal",
  "bat": "ft-file-terminal", "blend": "ft-file-axis-3d", "bmp": "ft-file-image",
  "br": "ft-file-archive", "bz2": "ft-file-archive", "c": "ft-file-code",
  "cc": "ft-file-code", "cer": "ft-file-key", "cfg": "ft-file-sliders",
  "cjs": "ft-file-code", "clj": "ft-file-code", "cmd": "ft-file-terminal",
  "conf": "ft-file-sliders", "cpp": "ft-file-code", "crt": "ft-file-key",
  "cs": "ft-file-code", "css": "ft-file-type", "csv": "ft-file-spreadsheet",
  "cts": "ft-file-code", "cxx": "ft-file-code", "dart": "ft-file-code",
  "db": "ft-database", "diff": "ft-file-diff", "dmg": "ft-file-archive",
  "doc": "ft-file-text", "docx": "ft-file-text", "duckdb": "ft-database",
  "eot": "ft-file-type", "eps": "ft-file-image", "erl": "ft-file-code",
  "ex": "ft-file-code", "exs": "ft-file-code", "fbx": "ft-file-axis-3d",
  "fish": "ft-file-terminal", "flac": "ft-file-music", "fs": "ft-file-code",
  "fsx": "ft-file-code", "gif": "ft-file-image", "glb": "ft-file-axis-3d",
  "gltf": "ft-file-axis-3d", "go": "ft-file-code", "gpg": "ft-file-key",
  "gql": "ft-file-braces", "gradle": "ft-file-cog", "graphql": "ft-file-braces",
  "gz": "ft-file-archive", "h": "ft-file-code", "hcl": "ft-file-sliders",
  "heic": "ft-file-image", "hpp": "ft-file-code", "hrl": "ft-file-code",
  "hs": "ft-file-code", "htm": "ft-file-code", "html": "ft-file-code",
  "ico": "ft-file-image", "ini": "ft-file-sliders", "ipynb": "ft-file-chart-column",
  "iso": "ft-file-archive", "java": "ft-file-code", "jpeg": "ft-file-image",
  "jpg": "ft-file-image", "js": "ft-file-code", "json": "ft-file-braces",
  "json5": "ft-file-braces", "jsonc": "ft-file-braces", "jsx": "ft-file-code",
  "key": "ft-file-key", "kt": "ft-file-code", "kts": "ft-file-code",
  "less": "ft-file-type", "lock": "ft-file-lock", "log": "ft-file-text",
  "lua": "ft-file-code", "m4a": "ft-file-music", "m4v": "ft-file-play",
  "md": "ft-file-text", "mdx": "ft-file-text", "mjs": "ft-file-code",
  "mkv": "ft-file-play", "mmd": "ft-file-chart-column", "mov": "ft-file-play",
  "mp3": "ft-file-music", "mp4": "ft-file-play", "mpeg": "ft-file-play",
  "mpg": "ft-file-play", "mts": "ft-file-code", "nim": "ft-file-code",
  "nu": "ft-file-terminal", "obj": "ft-file-axis-3d", "ods": "ft-file-spreadsheet",
  "ogg": "ft-file-music", "opus": "ft-file-music", "otf": "ft-file-type",
  "p12": "ft-file-lock", "patch": "ft-file-diff", "pdf": "ft-file-text",
  "pem": "ft-file-key", "pfx": "ft-file-lock", "php": "ft-file-code",
  "pl": "ft-file-code", "pm": "ft-file-code", "png": "ft-file-image",
  "ppt": "ft-file-chart-column", "pptx": "ft-file-chart-column", "prisma": "ft-database",
  "properties": "ft-file-sliders", "proto": "ft-file-braces", "ps1": "ft-file-terminal",
  "psd": "ft-file-image", "pub": "ft-file-key", "py": "ft-file-code",
  "r": "ft-file-code", "rar": "ft-file-archive", "rb": "ft-file-code",
  "rst": "ft-file-text", "rs": "ft-file-code", "rtf": "ft-file-text",
  "sass": "ft-file-type", "scala": "ft-file-code", "scss": "ft-file-type",
  "sh": "ft-file-terminal", "sol": "ft-file-code", "sqlite": "ft-database",
  "sqlite3": "ft-database", "sql": "ft-database", "stl": "ft-file-axis-3d",
  "svelte": "ft-file-code", "svg": "ft-file-image", "swift": "ft-file-code",
  "tar": "ft-file-archive", "tar.bz2": "ft-file-archive", "tar.gz": "ft-file-archive",
  "tar.xz": "ft-file-archive", "tbz2": "ft-file-archive", "tex": "ft-file-text",
  "tf": "ft-file-sliders", "tfvars": "ft-file-sliders", "tgz": "ft-file-archive",
  "tif": "ft-file-image", "tiff": "ft-file-image", "toml": "ft-file-sliders",
  "ts": "ft-file-code", "tsx": "ft-file-code", "tsv": "ft-file-spreadsheet",
  "ttf": "ft-file-type", "txt": "ft-file-text", "txz": "ft-file-archive",
  "vb": "ft-file-code", "vue": "ft-file-code", "wav": "ft-file-music",
  "webm": "ft-file-play", "webp": "ft-file-image", "woff": "ft-file-type",
  "woff2": "ft-file-type", "xhtml": "ft-file-code", "xls": "ft-file-spreadsheet",
  "xlsx": "ft-file-spreadsheet", "xml": "ft-file-code", "xz": "ft-file-archive",
  "yaml": "ft-file-sliders", "yml": "ft-file-sliders", "zig": "ft-file-code",
  "zip": "ft-file-archive", "zsh": "ft-file-terminal",
};

const COMPOUND_EXTENSIONS = ["tar.bz2", "tar.gz", "tar.xz"];

/* Orca's lookup order (getFileTypeIcon, same file): exact lowercased
 * basename, its four prefix rules, then the extension — compound forms
 * first, a trailing dot yields none — and File when nothing bites. */
function fileTypeIcon(path) {
  const text = String(path ?? "");
  const slash = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  const base = (slash >= 0 ? text.slice(slash + 1) : text).toLowerCase();
  if (!base) return "file";
  const exact = FILE_ICON_BY_NAME[base];
  if (exact) return exact;
  if (base === "mobile emulator" || base === "simulator") return "ft-smartphone";
  if (base === ".env" || base.startsWith(".env.")) return "ft-file-lock";
  if (base === "dockerfile" || base.startsWith("dockerfile.")) return "ft-file-cog";
  if (base === "makefile" || base.startsWith("makefile.")) return "ft-file-terminal";
  const compound = COMPOUND_EXTENSIONS.find((ext) => base.endsWith(`.${ext}`));
  const dot = base.lastIndexOf(".");
  const ext = compound ?? (dot > 0 && dot < base.length - 1 ? base.slice(dot + 1) : "");
  return FILE_ICON_BY_EXTENSION[ext] ?? "file";
}

function laneBySession(session) {
  for (const entry of lanes.values()) {
    if (entry.lane.session_id === session) return entry;
  }
  return null;
}

function shortSession(id) {
  return id.length > 18 ? `…${id.slice(-14)}` : id;
}

/* Session ids carry their birth time (`session-<epoch-ms>-N`) — enough for
 * the relative timestamps a thread list lives on. */
function relTime(sessionId) {
  const match = /session-(\d{13})/.exec(sessionId ?? "");
  if (!match) return "";
  const minutes = Math.floor((Date.now() - Number(match[1])) / 60000);
  if (minutes < 1) return t("session.justNow", "방금");
  if (minutes < 60) return t("session.minutes", "{{n}}분", { n: minutes });
  if (minutes < 1440) return t("session.hours", "{{n}}시간", { n: Math.floor(minutes / 60) });
  return t("session.days", "{{n}}일", { n: Math.floor(minutes / 1440) });
}

/* ---- terminal views ----
 *
 * One factory, two instances: the stage (lane TUI) and the float (shell).
 * Each keeps its own paint buffer and cell metrics; both apply GridDelta by
 * the shift contract. */

function cssColor(color, fallback) {
  // Absent, not "default": the grid omits every style field nobody set, so an
  // unstyled cell arrives with no colour key at all.
  if (color == null || color === "default") return fallback;
  if (color.indexed !== undefined) {
    // Every indexed colour is parsed once in tokens.css and selected by a
    // class on the run. Building an rgb() string here put a fresh inline
    // style value through WebKit on every changed row, which is precisely the
    // frame-path retention this renderer must avoid.
    return `var(--term-${color.indexed})`;
  }
  if (color.rgb) return `rgb(${color.rgb[0]} ${color.rgb[1]} ${color.rgb[2]})`;
  return fallback;
}

/* Orca's `minimumContrastRatio` — 4.5 on a light screen, 3 on a dark one
 * (`resolveTerminalMinimumContrastRatio`,
 * terminal-shortcut-policy-BK9MulHK.js:17591), and xterm raises any colour
 * that lands closer to its background than that.
 *
 * Which is a thing a terminal needs and an application does not: the colours
 * are the PROGRAM's, chosen against a screen it cannot see, and "blue on
 * black" is a real pairing that real tools emit. Without this, `--term-4` on
 * `--term-screen-bg` is simply hard to read and there is nobody to complain
 * to — the program picked a legal colour and we drew exactly what it asked
 * for.
 *
 * The bar follows Orca's split — 4.5 when the screen is light, 3 when it is
 * dark, because a lit screen washes out low-contrast ink and a dark one does
 * not. Not because 3 is the bar this window would pick on its own, but because
 * its mandate is that the same program output paints the same pixels as Orca,
 * and a stricter bar is still a different picture. */
let termContrastBar = null;

function termMinContrast() {
  if (termContrastBar === null) {
    const screen = readColor("var(--term-screen-bg)");
    termContrastBar =
      screen !== null && contrastRatio([0, 0, 0], screen) > contrastRatio([255, 255, 255], screen)
        ? 4.5
        : 3;
  }
  return termContrastBar;
}

/* Resolved colours are cached, and the key carries the palette generation.
 *
 * The lift has to read the *actual* pixels behind `var(--term-4)`, which is a
 * `getComputedStyle` on the root — far too expensive to do per run, per frame.
 * Almost every screen uses a handful of distinct pairs, so a map keyed by the
 * pair is nearly always a hit. The generation is what makes it safe to cache
 * across a theme change instead of stale (see `forgetPalette`). */
let paletteGeneration = 0;
const contrastLift = new Map();
const indexedContrastLift = new Map();
/* And the tokens themselves, read once each per palette.
 *
 * The two maps above cache ANSWERS, which is not the same as caching the
 * reads that produce them: `nearestReadableIndex` walks all 256 palette
 * entries on every miss, so one cache miss was 258 `getComputedStyle` calls on
 * the root — each of which flushes style. Measured on a screen whose colour
 * changes every four columns (a syntax-highlighted diff, `ls --color`, a
 * dashboard TUI): **96 forced style reads per frame** averaged over the run,
 * and a worst frame of 27ms against a 2.3ms median — a visible hitch whenever
 * a colour the palette had not been asked about yet came on screen.
 *
 * A token's value cannot change without `forgetPalette`, which is what makes
 * this safe to hold: every road that rebinds `--term-*` — a treatment flip, a
 * theme, an override — ends there (`applyTheme`, `applyTerminalPalette`). */
const resolvedTokens = new Map();

function readColor(css) {
  if (typeof css !== "string") return null;
  let text = css.trim();
  // `var(--term-4)` — the only indirection cssColor produces. Read what the
  // treatment currently binds it to rather than keeping a second copy of the
  // palette here, which would be a copy to keep in step.
  const via = text.match(/^var\((--[a-z0-9-]+)\)$/i);
  if (via) {
    const held = resolvedTokens.get(via[1]);
    if (held === undefined) {
      text = getComputedStyle(document.documentElement).getPropertyValue(via[1]).trim();
      resolvedTokens.set(via[1], text);
    } else {
      text = held;
    }
    if (text === "") return null;
  }
  const hex = text.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const value = Number.parseInt(hex[1], 16);
    return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
  }
  const short = text.match(/^#([0-9a-f]{3})$/i);
  if (short) {
    return [0, 1, 2].map((at) => Number.parseInt(short[1][at].repeat(2), 16));
  }
  const rgb = text.match(/^rgba?\(\s*([0-9.]+)[\s,]+([0-9.]+)[\s,]+([0-9.]+)/i);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return null;
}

/* WCAG relative luminance, and the ratio built from two of them. Arithmetic
 * from the specification, so it is computed rather than tokenised. */
function luminance([r, g, b]) {
  const channel = (raw) => {
    const part = raw / 255;
    return part <= 0.03928 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrastRatio(ink, ground) {
  const a = luminance(ink);
  const b = luminance(ground);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/* The colour the program asked for, or the nearest one that can be read.
 *
 * Moved toward whichever pole is AWAY from the background, by bisection rather
 * than a fixed step — a fixed step either overshoots (a colour hauled to near
 * white loses the hue the program was using to mean something) or takes many
 * tries. Twelve halvings put it within 1/4096 of the least change that clears
 * the bar, which is the point: the smallest lift, not the safest colour.
 *
 * Returns the original string untouched when the pair already reads, which is
 * the overwhelmingly common case and keeps `var(--term-N)` bound to the token
 * instead of frozen to a literal. */
function readableOn(fg, bg) {
  if (fg == null) return fg;
  const key = `${paletteGeneration}\u0000${fg}\u0000${bg ?? ""}`;
  const held = contrastLift.get(key);
  if (held !== undefined) return held;
  const answer = computeReadable(fg, bg);
  contrastLift.set(key, answer);
  return answer;
}

function computeReadable(fg, bg) {
  const ink = readColor(fg);
  // A run with no background of its own sits on the screen, so that is what it
  // has to be legible against.
  const ground = readColor(bg ?? "var(--term-screen-bg)");
  if (ink === null || ground === null) return fg;
  if (contrastRatio(ink, ground) >= termMinContrast()) return fg;
  const toward = luminance(ground) > 0.5 ? [0, 0, 0] : [255, 255, 255];
  let low = 0;
  let high = 1;
  let best = toward;
  for (let step = 0; step < 12; step += 1) {
    const mid = (low + high) / 2;
    const tried = ink.map((part, at) => Math.round(part + (toward[at] - part) * mid));
    if (contrastRatio(tried, ground) >= termMinContrast()) {
      best = tried;
      high = mid;
    } else {
      low = mid;
    }
  }
  // A mid-grey background can be unreachable from either pole — neither black
  // nor white clears the bar against it. Then this is simply the most readable
  // colour available, which is what xterm settles for too.
  return `rgb(${best[0]} ${best[1]} ${best[2]})`;
}

/* A finite terminal colour remains finite after contrast correction.
 *
 * `readableOn` computes the exact smallest lift, but writing that rgb() onto
 * a span would turn an indexed colour into another per-frame inline value.
 * Pick the closest member of the already-declared xterm palette that clears
 * the same bar instead. True-colour input keeps the exact inline result; this
 * fold is only for the 256 indexed colours and defaults. */
function nearestReadableIndex(readable, bg) {
  const key = `${paletteGeneration}\u0000${readable}\u0000${bg ?? ""}`;
  const held = indexedContrastLift.get(key);
  if (held !== undefined) return held;
  const target = readColor(readable);
  const ground = readColor(bg ?? "var(--term-screen-bg)");
  if (target === null || ground === null) return 15;
  let best = 15;
  let bestDistance = Number.POSITIVE_INFINITY;
  let foundReadable = false;
  for (let index = 0; index < 256; index += 1) {
    const candidate = readColor(`var(--term-${index})`);
    if (candidate === null) continue;
    const reads = contrastRatio(candidate, ground) >= termMinContrast();
    if (foundReadable && !reads) continue;
    const distance = candidate.reduce((sum, part, at) => sum + (part - target[at]) ** 2, 0);
    if ((reads && !foundReadable) || distance < bestDistance) {
      foundReadable = reads;
      bestDistance = distance;
      best = index;
    }
  }
  indexedContrastLift.set(key, best);
  return best;
}

function finiteColorClass(color, fallback, role, ground = null) {
  const asked = cssColor(color, fallback);
  // RGB is the one unbounded terminal palette. It remains inline so a class
  // catalogue does not grow with program output.
  if (color?.rgb) {
    return { cls: "", inline: role === "fg" ? readableOn(asked, ground) : asked };
  }
  if (role === "fg") {
    const readable = readableOn(asked, ground);
    if (readable !== asked) return { cls: `term-fg-${nearestReadableIndex(readable, ground)}`, inline: "" };
  }
  if (color?.indexed !== undefined) return { cls: `term-${role}-${color.indexed}`, inline: "" };
  if (role === "fg" && fallback === "var(--term-bg-solid)") {
    return { cls: "term-fg-reverse-default", inline: "" };
  }
  if (role === "fg") return { cls: "term-fg-default", inline: "" };
  if (fallback === "var(--term-fg)") return { cls: "term-bg-default", inline: "" };
  return { cls: "", inline: "" };
}

/* Every view's painted colours, forgotten together.
 *
 * A treatment change rebinds `--term-*` (tokens.css binds a light block for
 * the palette as well as the chrome), and a lifted colour is a LITERAL — it
 * stopped tracking the token the moment it was lifted. So the cache is
 * dropped and every screen is asked to paint again from its model, rather
 * than leaving the lifted runs holding yesterday's palette until the program
 * inside happens to rewrite them. */
const paintedViews = new Set();

function forgetPalette() {
  paletteGeneration += 1;
  termContrastBar = null;
  contrastLift.clear();
  indexedContrastLift.clear();
  resolvedTokens.clear();
  for (const again of paintedViews) again();
}

/* Two cells wear the same run if their styles match. Compared field by field
 * because this is the hottest question in the window: it is asked once per
 * cell, and a 96×40 screen is nearly four thousand cells a frame.
 *
 * It used to be `JSON.stringify(style)` per cell, which is a serialisation and
 * an allocation each time — the single biggest reason the terminal crawled. */
function sameColor(a, b) {
  if (a === b) return true;
  if (a == null || b == null) return false;
  if (typeof a === "string" || typeof b === "string") return false;
  if (a.indexed !== undefined || b.indexed !== undefined) return a.indexed === b.indexed;
  if (a.rgb && b.rgb) {
    return a.rgb[0] === b.rgb[0] && a.rgb[1] === b.rgb[1] && a.rgb[2] === b.rgb[2];
  }
  return false;
}

/* The second column of a double-width glyph, as the grid marks it
 * (`CONTINUATION`, crates/zerocode-pty/src/grid.rs). A Korean syllable is
 * drawn across two terminal columns and the cell in front of it already
 * advances both, so this column carries no glyph of its own. */
const CONTINUATION = "\u0000";
/* How far a double-width glyph may grow toward its two columns: its font-size
 * may reach this fraction of the row height and no more. The row is the one
 * box a glyph must never push — a Hangul syllable drawn taller than the row
 * would sit on the row below. At 14px/1.15 this allows 1.09×; the rest of
 * the shortfall stays as tracking, split around the glyph so it sits
 * centered in its columns rather than left with a gap after it. */
const WIDE_GLYPH_ROW_FILL = 0.95;
/* The glyph a cell is measured with: the mono face's own advance, which a
 * narrow glyph's advance is compared against (`measure`, `glyphPitch`). */
const CELL_PROBE_GLYPH = "W";
/* Below this the mono face draws the character itself: every face in the
 * terminal stack carries printable ASCII, so those cells never need asking. */
const ASCII_END = 0x80;
/* A narrow glyph's advance may miss the cell by this much and still be the
 * cell's: a hundredth of a pixel cannot move a column a device pixel within
 * any row this window lays out. */
const PITCH_SLACK_PX = 0.01;

/* The step a narrow glyph's tracking is written in: WebKit lays inline boxes
 * out in 1/64px units, so a finer value would name classes the layout cannot
 * tell apart. */
const PITCH_QUANTUM_PX = 1 / 64;

/* One 2D context for every view's glyph advances. `measureText` shapes a
 * string through the same font cascade a run's text takes — including the
 * fallback face that draws what the mono stack lacks — and reads no layout,
 * which the paint path may not do. */
let glyphMeterContext = null;

function glyphMeter() {
  if (glyphMeterContext === null) glyphMeterContext = document.createElement("canvas").getContext("2d");
  return glyphMeterContext;
}

/* The tracking a narrow off-pitch glyph wears, as a CLASS per distinct
 * shortfall — the same road the wide glyph's correction takes, for the same
 * reason: a run names its dress and the value sits once, so a changed span is
 * told a class name rather than handed inline lengths, and a span a later run
 * reuses keeps nothing of it. The rules are written into one sheet the first
 * time a shortfall is met, shared by every view, never removed (a face has a
 * handful of fallback advances). */
const pitchClasses = new Map();
let pitchSheet = null;

function pitchClass(spread) {
  const units = Math.round(spread / PITCH_QUANTUM_PX);
  if (units === 0) return "";
  const held = pitchClasses.get(units);
  if (held !== undefined) return held;
  const name = `term-pitch-${units < 0 ? "n" : "p"}${Math.abs(units)}`;
  if (pitchSheet === null) {
    const holder = document.createElement("style");
    document.head.appendChild(holder);
    pitchSheet = holder.sheet;
  }
  const px = units * PITCH_QUANTUM_PX;
  // Half the tracking leads the glyph so it sits centred in its column; the
  // negative tail gives the lead back, so the next run starts on its column.
  pitchSheet.insertRule(
    `.term .${name} { letter-spacing: ${px}px; margin-left: ${px / 2}px; margin-right: ${-px / 2}px; }`,
    pitchSheet.cssRules.length,
  );
  pitchClasses.set(units, name);
  return name;
}

/* The text a visible cell contributes. A continuation owns no glyph, while
 * `zw` carries the scalars that belong to the cell without taking columns.
 * Both the full terminal renderer and the card preview use this one reading,
 * so a new grapheme form cannot be taught to one surface and forgotten by the
 * other. */
function cellGlyph(cell) {
  return cell.ch === CONTINUATION ? "" : cell.ch + (cell.zw ?? "");
}

/* What a cell with no style at all means.
 *
 * The grid omits every style field that is off (`CellStyle`,
 * crates/zerocode-pty/src/grid.rs), because spelling out seven defaults per
 * cell cost about ten times what the character did — a plain 80-column row
 * was 10KB of JSON on a channel that carries a frame every 16ms. So `style`
 * is simply absent on an unstyled cell, and every field read off this stands
 * in: absent is falsy, and `cssColor` reads absent as "whatever the theme
 * decides", which is what `Color::Default` already meant. */
const PLAIN_STYLE = Object.freeze({});

function sameStyle(a, b) {
  /* The same object is the same style, and that is the common case by a wide
   * margin: `expandPackedRow` hands every cell of a run the run's ONE style
   * object (`cells[col].style = style`, the packed wire's `[at, len, style]`),
   * so the comparison below is asked 13,200 times a frame at 220×60 and gets
   * a different answer only at a run boundary — one to three places in a row.
   * Without this line each of those asks eighteen property loads; with it,
   * one pointer compare. It is a fast path and not a replacement: the window's
   * own fixtures build a fresh object per cell (`{ ...dress }`), and the field
   * comparison below is what still answers for them. */
  if (a === b) return true;
  return (
    a.bold === b.bold &&
    a.dim === b.dim &&
    a.italic === b.italic &&
    a.underline === b.underline &&
    a.reverse === b.reverse &&
    // The two the grid learned with SGR 9 and SGR 5. A field added to the
    // style and not added HERE is worse than one that was never added: two
    // adjacent runs that differ only in it compare equal, merge, and the whole
    // merged run wears whatever the first cell had — so half a line silently
    // takes a strike it was never given.
    a.strike === b.strike &&
    a.blink === b.blink &&
    sameColor(a.fg, b.fg) &&
    sameColor(a.bg, b.bg)
  );
}

/* How much of the mouse each running program wants, by surface.
 *
 * Refreshed from every frame rather than asked for: the modes belong to the
 * program, it changes them while it runs, and a window that assumed them would
 * either go silent on a program waiting for clicks or chatter at one that
 * never asked. */
const mouseModes = new Map();

/* The shortest a scrollbar thumb may be drawn.
 *
 * A 5000-line scrollback showing 40 rows makes the thumb 0.8% of the track —
 * a line one pixel tall, which reports the position honestly and cannot be
 * grabbed. Every scrollbar has this floor; xterm calls it
 * `MINIMUM_SLIDER_SIZE`. */
const TERM_THUMB_MIN = 24;

/* How long a banked scroll may wait when no animation frame comes.
 *
 * The scroll flight flushes on rAF — once per painted frame — and this floor
 * exists only for the moments the compositor throttles frames (an occluded
 * or backgrounded window): two display frames, so a gesture still lands at a
 * cadence nobody can feel while never regressing to per-event commands. */
const SCROLL_FLIGHT_FLOOR_MS = 32;

/* What a terminal's ⌘F will look through, at most.
 *
 * The count the backend enforces (`TERM_SEARCH_MAX`) and the one the bar
 * prints. Named here as well because the window decides what "and more" means
 * on screen, and a number in only one of the two places is a number that
 * drifts. */
const TERM_SEARCH_MAX = 2000;

/* What counts as a link in terminal output.
 *
 * Two shapes, and they are deliberately narrow. A terminal is not a document —
 * everything on the screen is somebody's output, and a greedy pattern turns
 * ordinary prose into a minefield of accidental links. So: a real scheme for a
 * URL, and for a path either a leading `./`/`../`/`/` or something with a
 * slash and a file extension. `foo.rs` alone is NOT a link; `src/foo.rs` is.
 *
 * The trailing `(:line(:col)?)?` is what makes the feature worth having — a
 * compiler, a test runner and an agent all print `src/foo.rs:42`, and landing
 * on the line is the whole reason somebody clicks it. */
/* 경로의 몸통에서 빼는 두 유니코드 블록: Box Drawing(U+2500–257F)과 Block
 * Elements(U+2580–259F).
 *
 * 이 문자류는 거부 목록이고 거부 목록은 유니코드에 대해 **닫힐 수 없다** — Orca 는
 * 여기서 허용 목록을 쓴다(`LOCAL_PATH_REGEX`, terminal-links.ts:36). 그 차이가
 * 실제 결함으로 나왔다: TUI 의 테두리가 경로에 바로 붙으면(칸이 정확히 차서
 * 사이에 공백이 없다) `│/Users/dev/2026/zerocode│` 의 테두리까지 `href` 에
 * 삼켜지고, 열리지 않는 링크는 링크가 없는 것보다 나쁘다.
 *
 * 개별 글자를 두더지 잡듯 보태는 것이 아니라 **범주 하나**를 뺀다: 이 두 블록은
 * 유니코드가 스스로 「그리기」라고 이름 붙인 것이므로 파일 이름의 몸통이 될 수
 * 없다. 허용 목록으로 옮기는 것이 옳은 끝이지만 그건 Orca 처럼 공백 포함 경로의
 * 넓은 후보 길을 함께 들여야 한다(그러지 않으면 한글 이름의 경로를 잃는다) —
 * 별개의 조각으로 남긴다. */
const PATH_DRAWING_BLOCKS = "\\u2500-\\u259F";
/* 있는 경로만 링크가 된다.
 *
 * 원본은 후보마다 파일 시스템에 물어보고 **없으면 링크를 만들지 않는다**
 * (`pathExists: existsSync`, main/index.ts:611 → 후보를 버리는 자리는
 * terminal-link-handlers.ts:170-181). 그 필터가 원본의 넓은 추측을 안전하게
 * 만드는 것이고, 우리는 패턴이 맞으면 무조건 만들고 있었다 — **열리지 않는
 * 링크는 링크가 없는 것보다 나쁘다**: 사람의 클릭 한 번을 써서 아무것도 말하지
 * 않는다.
 *
 * `marksOf` 는 `paintRow` 안에서 도는 **동기** 손이라 물어볼 수가 없다. 그래서
 * 답을 캐시에서 동기로 읽고, 모르는 것은 물을 목록에 넣고, 답이 오면 **무언가
 * 바뀐 경우에만** 다시 그린다. 원본도 같은 모양이다 — `pathExistsCache` 와
 * async `provideLinks`, 링크가 한 박자 뒤에 선다.
 *
 * 예외 둘도 원본의 것이다:
 *   · 알려진 워크트리 뿌리는 **묻지 않고** 링크가 된다 — SSH 나 낡은 로컬 경로가
 *     「없다」로 읽혀도 눌릴 수 있어야 한다(:176-178).
 *   · 끝이 `/` 인 것은 뿌리가 아니면 경로로 세지 않는다(:166).
 *
 * 우리 층의 차이 하나: 상대 경로를 `activeWorktreePath` 하나에 대해 푼다(원본은
 * 판마다 `getPaneLinkCwd`). 캐시 키가 그렇게 푼 **절대 경로**이므로 그 단순화가
 * 캐시로 새지는 않지만, 판마다 cwd 를 알게 되는 날 키도 같이 넓혀야 한다. */
const termPathStands = new Map();
const termPathToAsk = new Set();
let termPathAsking = false;

/* 이 장부는 세션 내내 자라면 안 된다. 상한과 그 이유 둘 다 원본에서 왔다
 * (`terminal-path-exists-cache.ts:1`, 그리고 `:47-48` 의 주석: 긴 세션의 터미널
 * 출력은 **유일한 경로를 무한히** 낼 수 있다 — 에이전트 하나가 파일 트리를
 * 훑으면 한 화면이 수백 개를 남긴다). 잘라내는 방식도 같다: 읽힌 것은 뒤로,
 * 새 항이 들어올 때 앞을 버린다(`:31-36`, `:44-57`). `Map` 의 순회 순서가
 * 삽입 순서라는 것이 여기서 유일한 장부이고, 그래서 나란한 큐가 필요 없다. */
const TERM_PATH_CACHE_MAX = 1024;

/* 판정을 읽는다 — 그리고 읽힌 것을 최근으로 올린다. */
function termPathHeld(absolute) {
  const held = termPathStands.get(absolute);
  if (held !== undefined) {
    termPathStands.delete(absolute);
    termPathStands.set(absolute, held);
  }
  return held;
}

/* 판정을 적는다 — 자리가 없으면 가장 오래 안 읽힌 것을 버리고. */
function termPathNote(absolute, stands) {
  // 이미 있던 항은 자리를 차지하지 않는다(지우고 다시 넣어 뒤로 보낼 뿐).
  if (!termPathStands.delete(absolute)) {
    while (termPathStands.size >= TERM_PATH_CACHE_MAX) {
      const oldest = termPathStands.keys().next().value;
      if (oldest === undefined) break;
      termPathStands.delete(oldest);
    }
  }
  termPathStands.set(absolute, stands);
}

function askTermPathStands(absolute) {
  termPathToAsk.add(absolute);
  if (termPathAsking) return;
  termPathAsking = true;
  // 한 프레임의 새 경로들을 한 번에 묻는다: 행마다 왕복하면 페인트마다 왕복이다.
  queueMicrotask(async () => {
    termPathAsking = false;
    const asked = [...termPathToAsk];
    termPathToAsk.clear();
    if (asked.length === 0) return;
    let answers;
    try {
      answers = await invoke("paths_exist", { paths: asked });
    } catch {
      return;
    }
    let moved = false;
    asked.forEach((path, at) => {
      const said = answers?.[at];
      /* 답이 없는 것은 「없다」가 아니다.
       *
       * 한 번의 물음에는 상한이 있고(Rust `paths_exist`), 상한이 물리면 답은
       * 물음보다 **짧게** 온다. 그때 없는 자리를 `false` 로 적으면 그 경로는
       * 캐시가 살아 있는 동안 다시 물어보지도 않으므로 **있는 파일이 링크를
       * 영구히 잃는다**. 적지 않으면 다음 페인트가 다시 묻는다. */
      if (typeof said !== "boolean") return;
      const before = termPathStands.get(path);
      termPathNote(path, said);
      /* 답이 **화면을** 바꾼 것만 그리기를 부른다.
       *
       * 「판정을 바꾼 것」이 아니다. 처음 묻는 경로의 `before` 는 `undefined`
       * 이지만, 화면은 그 동안 이미 「링크 아님」으로 서 있다 — 모르는 동안
       * `termLinkStands` 가 돌려준 답이 바로 그것이다(:1462, `return false`).
       * 그러니 첫 답이 「없다」이면 바뀐 것은 없고, 그릴 것도 없다.
       *
       * `undefined !== false` 를 변화로 읽던 동안의 값: 경로처럼 생겼지만
       * 없는 낱말이 프레임에 하나만 섞여도 아래의 `relink()` 가 **모든 뷰의
       * 전 행**을 다시 그렸다. 그런 낱말은 드물지 않다 — diff 의 `a/…`·`b/…`,
       * 남의 워크트리 기준으로 인쇄된 상대 경로, 빌드 로그가 부르는 이미
       * 지워진 산출물. 세 행이 바뀐 프레임이 예순 행을 그리는 값이었고,
       * 그것이 흐르는 출력 내내 프레임마다였다. */
      if (before === undefined ? said : before !== said) moved = true;
    });
    // 바뀐 것이 없으면 그리지 않는다 — 이 손은 페인트 경로에 매달려 있다.
    if (!moved) return;
    for (const view of termViews.values()) view.relink();
  });
}

/* 이 주소가 링크로 설 수 있는가. `marksOf` 가 동기로 읽는 답. */
function termLinkStands(href) {
  // URL 은 파일 시스템의 것이 아니다.
  if (/^https?:\/\//.test(href)) return true;
  const named = href.match(/^(.*?)(?::(\d+))?(?::(\d+))?$/);
  const path = named ? named[1] : href;
  const absolute = path.startsWith("/") ? path : `${activeWorktreePath}/${path}`;
  /* 뿌리를 찾기 전에 끝의 구분자를 걷는다 — 원본도 그렇게 한다
   * (`normalizeWorktreeRootPathForTerminalLink`, terminal-worktree-path-link.ts:27-41;
   * `/` 자체와 드라이브 뿌리는 남긴다). 걷지 않으면 사람이 방금 `cd` 한 그
   * 워크스페이스가 `…/zerocode/` 로 인쇄되었을 때 뿌리로 읽히지 않아 바로 아래의
   * 끝-슬래시 거절에 걸린다 — 가장 눌릴 만한 경로가 링크가 아니게 된다. */
  if (worktreeAt(absolute.replace(/(?!^)[/\\]+$/, ""))) return true;
  if (/[/\\]$/.test(path)) return false;
  const held = termPathHeld(absolute);
  if (held !== undefined) return held;
  askTermPathStands(absolute);
  return false;
}

/* A URL ends where the text stops being a URL. Its body is the URL alphabet
 * (RFC 3986 unreserved, sub-delims, `:/?#[]@%`; quotes and the backtick left
 * out), so a glyph outside it — a Korean particle glued after the address,
 * `…/stage)을` — is never part of the match. What that body still takes at
 * the end is the sentence's, not the URL's: trailing punctuation, and a
 * closing bracket the URL did not open itself (`Foo_(bar)` keeps its paren;
 * a URL written inside parentheses gives the paren back). */
const URL_TAIL_PUNCTUATION = ".,;:!?";
const URL_CLOSERS = { ")": "(", "]": "[", "}": "{" };
function trimUrlTail(url) {
  let end = url.length;
  while (end > 0) {
    const last = url[end - 1];
    if (URL_TAIL_PUNCTUATION.includes(last)) {
      end -= 1;
      continue;
    }
    const opener = URL_CLOSERS[last];
    if (opener === undefined) break;
    const head = url.slice(0, end - 1);
    const opened = head.split(opener).length - 1;
    const closed = head.split(last).length - 1;
    if (closed < opened) break;
    end -= 1;
  }
  return url.slice(0, end);
}

const TERM_LINK_PATTERN = new RegExp(
  `(https?://[A-Za-z0-9\\-._~:/?#\\[\\]@!$&*+,;=%()]+)` +
    `|((?:\\.{1,2}/|/|~/)[^\\s:<>"'\\x60|${PATH_DRAWING_BLOCKS}]+` +
    `|[\\w.-]+(?:/[\\w.-]+)+\\.\\w+)(?::(\\d+))?(?::(\\d+))?`,
  "g",
);

function noteMouseModes(key, delta) {
  if (delta.mouse_tracking === undefined) return;
  mouseModes.set(key, {
    tracking: delta.mouse_tracking,
    sgr: delta.mouse_sgr,
    // Whether saying "you have the keyboard" is worth a message at all.
    // Almost no shell asks for `?1004`, so almost every tab switch should cost
    // nothing — and a window that had to ASK would spend a round trip per
    // switch to be told no. Same frame, same reason as the pointer's modes.
    focus: delta.focus_reporting === true,
  });
}

/* Bumped whenever the browser finishes loading faces.
 *
 * A view measured before the fallback face for Hangul arrived measured a face
 * that is no longer the one drawing, and the metrics it took are wrong by
 * whatever the two differ by. That is the reason the measurement below cannot
 * simply be cached on the font the stylesheet asks for: the stylesheet's
 * answer does not change when the loader's does. */
let fontGeneration = 0;

if (document.fonts) {
  document.fonts.addEventListener?.("loadingdone", () => {
    fontGeneration += 1;
  });
}

/* 어떤 손이 어느 토큰을 밀든 — 뒤늦게 도착한 설정 스냅샷, 확대, 테마 — 줄의
 * `calc(font × leading)`는 즉시 따라가고 눈금(cell)은 재지 않은 채 스냅샷으로
 * 남는다. 그 어긋남은 행마다 쌓여 긴 판의 맨 아래에서 반 행이 되고("노란
 * 커서가 제 줄 위에 떠 있다"), 같은 눈금으로 센 pty 행수는 상자보다 커져
 * 마지막 줄이 크롬 밑으로 미끄러진다("하단 숫자가 가려짐" — 둘 다 라이브
 * 보고 2026-08-25, 세 번째 드리프트). 그래서 루트 스타일이 움직였다는 사실
 * 자체가 재측정을 예약한다: 폭풍 경로는 아무것도 읽지 않고, 관찰자는 토큰이
 * 실제로 움직인 프레임에만 한 번 울며, 재측정 자체는 metricsKey가 같으면
 * 공짜로 돌아온다. */
{
  let owedRemeasure = false;
  new MutationObserver(() => {
    if (owedRemeasure) return;
    owedRemeasure = true;
    requestAnimationFrame(() => {
      owedRemeasure = false;
      resizeAllTerminals();
    });
  }).observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
}

/* A packed wire row, back into cells.
 *
 * The backend sends `{index, text, runs, zw}` — the row's text once, style
 * runs only where ink was set (grid.rs `WireRow`) — because one JSON object
 * per cell was thousands of allocations per tick on the very thread that
 * owes the next keystroke. `[...text]` walks CODE POINTS, which is exactly
 * one per column: the continuation NUL included, surrogate pairs whole, so
 * no width logic lives here. The cell spelling still passes through
 * untouched (`row.cells ??` at the one consumer) — the harness's fixtures
 * and any older sender keep meaning what they meant. */
function expandPackedRow(row) {
  const cells = [];
  for (const ch of row.text) cells.push({ ch });
  if (row.runs) {
    for (const [at, len, style] of row.runs) {
      const end = Math.min(at + len, cells.length);
      for (let col = at; col < end; col += 1) cells[col].style = style;
    }
  }
  if (row.zw) {
    for (const [col, riders] of row.zw) {
      if (cells[col]) cells[col].zw = riders;
    }
  }
  return cells;
}

/* A person's answer for one terminal's fold. Marker metadata is only the
 * initial state; this memory keeps an opened cell open across later frames and
 * when its rows move into scrollback. */
const foldCollapsed = new Map();

function makeTermView(host, pre, caret, owner = { address: () => null }) {
  /* `spread` is what a double-width glyph is short by.
   *
   * A CJK syllable takes two terminal columns, but the mono stack has no
   * Hangul, so Korean falls back to a face with an advance of its own —
   * measured on this machine at 10.38px against a 7.225px Latin cell, which
   * is 1.44 columns, not 2. Left alone a Korean line draws narrower than the
   * grid reserved for it and everything after it slides left of its column.
   * The glyph is first grown toward its two columns as far as the row height
   * allows (`wideScale`, see `WIDE_GLYPH_ROW_FILL`), and what is still short
   * is closed with `letter-spacing` — one property on a run rather than a box
   * around every cell — with half of it leading the run, so each syllable sits
   * centered in its columns instead of left with the whole gap after it
   * (2026-09-10: Korean in every pane read as spaced-out letters, 자 체 는). */
  const cell = { width: 8, height: 18, spread: 0, wideScale: 1 };
  /* The tracking class of each narrow glyph a FALLBACK face draws off the
   * cell's pitch ("" on pitch), for the metrics `cell` was last measured under.
   *
   * The same shortfall `spread` answers for double-width glyphs, per glyph,
   * because a narrow one's fallback face is the OS's answer per character.
   * Claude Code draws ⎿ under every tool result and ⠋ in its spinner, and
   * neither is in SF Mono: WebKit drew ⎿ 4.58px wider than a 9.27px cell and ⠋
   * 0.98px wider, so every glyph after either on the row stood off its column
   * (+0.51 cell after ⎿ in the live window, 2026-09-17; Terminal.app 0.00). */
  const glyphSpreads = new Map();
  /* The CSS font a run of this screen is shaped in, as `measureText` takes it;
   * empty until a measurement has landed, when there is no cell to compare to. */
  let glyphFont = "";
  /* Where the caret last stood, so a frame that did not move it does not
   * rewind the blink — a shell printing output under a still cursor should
   * keep blinking like an idle one. */
  const caretAt = [-1, -1];
  /* The keystroke (`typedSerial`) this caret last held still for. */
  let caretHeldFor = 0;
  const rows = [];
  const nodes = [];
  /* What each row's spans currently SAY, parallel to `nodes`.
   *
   * The renderer's memory of the DOM, so a repaint can tell which runs are
   * already right and leave them — and leaving them is what lets a selection
   * survive a frame (see `paintRow`). Reading it back off the DOM instead
   * would mean a `getComputedStyle` per span per frame, which is the cost this
   * exists to avoid. A row with no entry has never been painted, and every
   * road that invalidates the DOM has to clear its entry here too or the next
   * frame will believe spans it no longer has. */
  const painted = [];
  /* ---- the selection ----
   *
   * The model (ui/shell-term-selection.js): absolute lines and cell
   * boundaries, nothing of the DOM. Beside it, what each visible row was
   * last painted WITH — the selection's columns on it, as one number — so a
   * change of the model repaints only the rows it moved; parallel to
   * `painted` and moved with it on a shift. And the drag in progress, when
   * there is one. The hands are below, under "the selection, on this
   * screen". */
  const selection = makeTermSelection();
  const paintedSelection = [];
  let selectDrag = null;
  /* How many columns the last frame said the screen has. */
  let screenCols = 0;
  // Composed fold frames name each slot's original buffer line. Ordinary
  // frames omit the map and use viewTop + row, including older snapshots.
  let rowLines = null;
  /* WHICH rows went stale while this view was hidden.
   *
   * A set rather than the one boolean this used to be. The flag was honest —
   * frames did arrive — but it could only answer "everything", so coming back
   * to a terminal where a single status line had been spinning rebuilt all
   * fifty rows: a thousand-odd spans torn down and made again for one row of
   * change. That repaint IS what a tab switch felt like (measured: 1,250 of
   * the 1,263 elements one switch built).
   *
   * `behindAll` is kept beside it for the cases where naming rows would be a
   * lie — a full frame, a scroll, a grid that changed height — because after
   * any of those row *n* means something else than it did. */
  const behindRows = new Set();
  let behindAll = false;
  /* How far the model's rows have moved since the DOM last followed them —
   * positive up (output scrolled, or the view moved toward live), negative
   * down (the view moved back into history). Frames only ADD to it; a paint
   * moves the row nodes by the net amount once (`shiftNodes`), and the rows the
   * frames exposed are already named in `behindRows`, moved along with it.
   * So several frames painted together cost one node move, not one each. */
  let behindShift = 0;
  /* The caret the last frame asked for, painted with the rows. */
  let caretWanted = null;
  // A top-layer control may temporarily own the compositor. Frames still
  // update `rows` while paused; only DOM writes wait, and one repaint catches
  // up when the control leaves. This keeps a streaming terminal from
  // invalidating the pixels underneath the create palette every few ms.
  let paintPaused = false;
  const foldSummaries = new Map();
  const hiddenBefore = [];
  let foldsShown = false;

  /* Where this screen sits in its own history, as the last frame reported it.
   *
   * `viewTop` is the absolute line number of display row 0 — the coordinate
   * `TermHit` speaks and the one a search jump needs. Derived once here rather
   * than at each use, because "which of history and screen is this row in" is
   * exactly the arithmetic that goes wrong when it is written twice. */
  let viewOffset = 0;
  let scrollbackLen = 0;
  let scrollbackTrimmed = null;
  let viewTop = 0;
  let altScreen = false;

  /* ---- what ⌘F found ----
   *
   * The hits themselves in the order the backend reported them (oldest line
   * first, so stepping forward walks DOWN the history the way reading does),
   * and the same hits keyed by line for the painter — which asks "what is on
   * this row" once per row per frame and must not scan the whole list to
   * answer. `hitAt` is which one is current; it is an index into `hits` rather
   * than a copy of one, so "which is current" has a single answer. */
  let hits = [];
  let hitsByLine = new Map();
  let hitAt = -1;

  function hitsOn(line) {
    const here = hitsByLine.get(line);
    if (here === undefined) return [];
    return here;
  }

  /* What the metrics in `cell` were taken under, so they are not taken again
   * under the same thing. Empty until a measurement has actually landed. */
  let measuredUnder = "";

  /* Everything that can move a cell box, as one string.
   *
   * A cell is a glyph in a face at a size — it does not follow the window, the
   * split, or which tab is in front. `measure` was called from the stage
   * repaint anyway, so switching tabs inserted a probe into a live screen and
   * took three forced layouts of the thousand-odd spans already in it, to
   * arrive at the numbers it already had. Reading the computed font is a style
   * read; the probe is a layout. */
  function metricsKey() {
    const style = getComputedStyle(pre);
    return [
      style.fontFamily,
      style.fontSize,
      style.fontWeight,
      style.letterSpacing,
      style.lineHeight,
      window.devicePixelRatio,
      fontGeneration,
    ].join("|");
  }

  /* Whether a double-width run needs the wide treatment at all: a face whose
   * Hangul already spans two columns (a CJK mono font in the stack) needs
   * neither growing nor tracking. */
  function wideAdjusted() {
    return cell.spread !== 0 || cell.wideScale !== 1;
  }

  function measure() {
    const under = metricsKey();
    if (under === measuredUnder) return;
    // The probe is built exactly like a painted row — a block .term-row with
    // an inline run inside — so the measured box is the box the renderer
    // will actually lay out. An inline-only probe reads the line box (~16px)
    // instead of the row's own height (18.2px), and that 2px lie compounds
    // into a pty sized taller than the screen.
    const probe = document.createElement("div");
    probe.className = "term-row";
    const run = document.createElement("span");
    run.textContent = CELL_PROBE_GLYPH.repeat(20);
    probe.appendChild(run);
    // The same measurement for a glyph the grid counts as two columns. It is
    // taken here rather than assumed, because which face draws Hangul is the
    // OS's answer to a stack with no Hangul in it, and that answer decides
    // how short the glyph falls.
    const wide = document.createElement("span");
    wide.textContent = "가".repeat(20);
    probe.appendChild(wide);
    // Inline-block, so the two runs have a computed width to read below —
    // an inline's computed width is "auto", and a RECT is the one measure a
    // cell must never be taken from: the float's `surface-enter` animation
    // holds scale(0.95) on its first frame, and a rect read through that
    // transform recorded cells 5% short FOREVER (metricsKey cannot see a
    // transform). 0.81px per row of caret drift and a pty two rows taller
    // than its box, both live-reported (Image #18, #22). Computed style is
    // the layout's own number, before any transform touches it.
    run.style.display = "inline-block";
    wide.style.display = "inline-block";
    // Sixteen stacked rows, not one: a computed string is the used value
    // rounded to four decimals, and one row's 16.09375px comes back 16.0938 —
    // enough of a lie to flip a floor division exactly on a row boundary
    // (the wheel turns eight rows and lands seven). Sixteen of a 1/64px-grid
    // height sum to a quarter-pixel grid, which four decimals carry exactly,
    // and dividing back recovers the row to the layout's own precision.
    const stack = document.createElement("div");
    stack.appendChild(probe);
    for (let extra = 1; extra < 16; extra += 1) {
      const filler = document.createElement("div");
      filler.className = "term-row";
      stack.appendChild(filler);
    }
    pre.appendChild(stack);
    const stackHeight = parseFloat(getComputedStyle(stack).height);
    const runWidth = parseFloat(getComputedStyle(run).width);
    const wideWidth = parseFloat(getComputedStyle(wide).width);
    const shaped = getComputedStyle(pre);
    if (stackHeight > 0 && runWidth > 0) {
      cell.width = runWidth / 20;
      cell.height = stackHeight / 16;
      // Recorded only when a measurement actually landed. A hidden host lays
      // nothing out and answers zero, and remembering THAT would leave a view
      // first measured behind another tab holding the default 8×18 forever.
      measuredUnder = under;
      // Every glyph's pitch was an answer about the old cell and the old face.
      glyphFont = `${shaped.fontStyle} ${shaped.fontWeight} ${shaped.fontSize} ${shaped.fontFamily}`;
      glyphSpreads.clear();
    }
    if (wideWidth > 0) {
      // Grow the glyph toward its two columns, capped by the row; then measure
      // the grown glyph rather than assuming its advance scales linearly — a
      // face's advance at another size is the face's answer, not arithmetic.
      const fontPx = parseFloat(shaped.fontSize) || cell.height;
      const scale = Math.min(
        (2 * cell.width) / (wideWidth / 20),
        (cell.height * WIDE_GLYPH_ROW_FILL) / fontPx,
      );
      wide.style.fontSize = `${scale}em`;
      const grownWidth = parseFloat(getComputedStyle(wide).width);
      const spread = 2 * cell.width - (grownWidth > 0 ? grownWidth : wideWidth * scale) / 20;
      if (spread !== cell.spread || scale !== cell.wideScale) {
        cell.spread = spread;
        cell.wideScale = scale;
        const vars = [
          ["--term-cell-spread", spread === 0 ? "" : `${spread}px`],
          ["--term-wide-scale", scale === 1 ? "" : `${scale}em`],
          ["--term-wide-lead", spread === 0 ? "" : `${spread / 2}px`],
        ];
        for (const [name, value] of vars) {
          if (value) pre.style.setProperty(name, value);
          else pre.style.removeProperty(name);
        }
      }
    }
    stack.remove();
  }

  /* The class that tracks `glyph` back onto the cell's pitch: how far its own
   * advance falls short of one cell (negative when the face draws it wider),
   * or "" for everything the mono face draws.
   *
   * Asked of the cascade through `measureText`, relative to the probe glyph in
   * the same font, so the answer is in the run's units whatever the canvas
   * rounds to; remembered until the metrics change. */
  function glyphPitch(glyph) {
    if (glyph.length === 1 && glyph.charCodeAt(0) < ASCII_END) return "";
    const held = glyphSpreads.get(glyph);
    if (held !== undefined) return held;
    if (glyphFont === "") return "";
    const meter = glyphMeter();
    meter.font = glyphFont;
    const pitch = meter.measureText(CELL_PROBE_GLYPH).width;
    const advance = meter.measureText(glyph).width;
    const spread = pitch > 0 ? cell.width * (1 - advance / pitch) : 0;
    const answer = Math.abs(spread) < PITCH_SLACK_PX ? "" : pitchClass(spread);
    glyphSpreads.set(glyph, answer);
    return answer;
  }

  /* The model's row count follows the frame; the DOM's follows the model at
   * the next paint (`followRowCount`). */
  function setSize(rowCount) {
    while (rows.length > rowCount) rows.pop();
    while (rows.length < rowCount) rows.push([]);
  }

  function followRowCount() {
    while (nodes.length > rows.length) {
      nodes.pop().remove();
      painted.pop();
      paintedSelection.pop();
    }
    while (nodes.length < rows.length) {
      const node = document.createElement("div");
      node.className = "term-row";
      pre.appendChild(node);
      nodes.push(node);
      // A fresh node has no spans, and saying so is what makes the first paint
      // build them rather than believe in ones that were never made.
      painted.push(undefined);
      paintedSelection.push(undefined);
    }
  }

  /* The runs a row's cells make, as plain data — no DOM.
   *
   * Split out from the painting so the two questions can be asked separately:
   * "what should this row look like" is cheap and allocation-only, and "what
   * does the DOM have to be told" is the expensive one. `paintRow` compares
   * the answers.
   *
   * The two things this must not do, both of which it used to: ask
   * `JSON.stringify` whether a cell matches the run (a serialisation per
   * cell), and `append` each character (a Text node per character). A busy TUI
   * repaints continuously, so per-cell costs are the whole budget. */
  function runsOf(cells, marks, selected = null) {
    const runs = [];
    let run = null;
    let runStyle = null;
    // Marks are sorted and never overlap, so the scan walks them with the
    // cells rather than searching for each cell's mark — the row is the hot
    // path and a lookup per cell is a lookup per cell.
    let markAt = 0;
    // The selection's columns on this row, `[from, to)`. A second dimension
    // beside the marks rather than a mark: it may cross a hit or a link, and
    // neither of them gives way — the run breaks at both edges and wears both.
    const selFrom = selected === null ? -1 : selected.from;
    const selTo = selected === null ? -1 : selected.to;

    for (let i = 0; i < cells.length; i += 1) {
      while (markAt < marks.length && marks[markAt].to <= i) markAt += 1;
      const mark = markAt < marks.length && marks[markAt].from <= i ? marks[markAt] : null;
      const sel = i >= selFrom && i < selTo;
      const c = cells[i];
      // Painting this would draw the glyph in front of it a second time, and
      // the row would overrun its right edge by one column per CJK character
      // on it — the same clipping the grid's own column count just stopped,
      // arriving from the other side.
      if (c.ch === CONTINUATION) continue;
      // A combining accent or an emoji joiner rides in the cell it modifies
      // rather than taking a column of its own, so it is drawn with it — an
      // `e` whose accent was left behind is just an `e`, and nothing says so.
      // `cellGlyph` is the shared form of `c.zw ? c.ch + c.zw : c.ch`;
      // keeping the full-cell reading here makes the renderer's contract
      // explicit while the preview reuses the same implementation.
      const glyph = cellGlyph(c);
      const s = c.style ?? PLAIN_STYLE;
      // The grid already claimed the column behind a wide glyph, so its own
      // mark answers this — the view keeps no width table of its own.
      const wide = cells[i + 1] !== undefined && cells[i + 1].ch === CONTINUATION;
      // And a narrow glyph drawn off the cell's pitch carries its own tracking,
      // so a run holds glyphs of one pitch only.
      const pitch = wide ? "" : glyphPitch(glyph);
      // A run also breaks where a mark starts or ends: a search hit and a link
      // are drawn on the run, so a run that straddles either edge would paint
      // the highlight over text that is not part of the match.
      if (run !== null && run.wide === wide && run.pitch === pitch && run.mark === mark && run.sel === sel && sameStyle(s, runStyle)) {
        run.text += glyph;
        // Where this run ends, in columns — kept because a selection asks
        // "which cell is under the pointer" and the answer has to survive the
        // fact that a wide glyph is one character across two columns.
        run.to = i + (wide ? 2 : 1);
        continue;
      }
      runStyle = s;
      let fgSpec = s.fg;
      let bgSpec = s.bg;
      let fgFall = "var(--term-fg)";
      let bgFall = null;
      if (s.reverse) {
        [fgSpec, bgSpec] = [bgSpec, fgSpec];
        fgFall = "var(--term-bg-solid)";
        bgFall = "var(--term-fg)";
      }
      // xterm settles reverse first and only then lifts bold ink into its
      // bright variant, and only across the 16-colour palette's lower half — so
      // a reversed cell's promoted colour is the one that was its ground.
      if (s.bold && fgSpec?.indexed !== undefined && fgSpec.indexed < 8) {
        fgSpec = { indexed: fgSpec.indexed + 8 };
      }
      const bgAsked = cssColor(bgSpec, bgFall);
      const bgPaint = finiteColorClass(bgSpec, bgFall, "bg");
      // Legibility is decided on the pair, after reverse has had its say —
      // reverse video changes which colour is the ground, so asking before it
      // would be asking about a pairing that never reaches the screen.
      const fgPaint = finiteColorClass(fgSpec, fgFall, "fg", bgAsked);
      let classes = "";
      if (s.bold) classes += "bold ";
      if (s.dim) classes += "dim ";
      if (s.italic) classes += "italic ";
      if (s.underline) classes += "underline ";
      // SGR 9 and SGR 5, which this grid had no fields for at all. A TUI
      // draws a removed line or a finished todo struck through, and without
      // it those lines read as ordinary text saying the opposite.
      if (s.strike) classes += "strike ";
      if (s.blink) classes += "blink ";
      if (mark) classes += `${mark.cls} `;
      if (fgPaint.cls) classes += `${fgPaint.cls} `;
      if (bgPaint.cls) classes += `${bgPaint.cls} `;
      if (wide && wideAdjusted()) classes += "term-wide ";
      if (pitch) classes += `${pitch} `;
      // The selection is painted on the run, in the theme's two selection
      // tokens, inline — inline is what beats a palette class and a program's
      // own RGB alike, and the selection has to win over both to be seen.
      if (sel) classes += "term-selected ";
      run = {
        text: glyph,
        wide,
        mark,
        sel,
        fg: sel ? "var(--term-selection-fg)" : fgPaint.inline,
        bg: sel ? "var(--term-selection-bg)" : bgPaint.inline,
        href: mark?.href ?? "",
        cls: classes.trim(),
        // A run of double-width glyphs is drawn short by its fallback face, so
        // it carries the measured shortfall as tracking. One property on the
        // run rather than a box per cell: each syllable still lands on its own
        // column, and the next run starts on the column the grid says it does.
        ls: wide && wideAdjusted(),
        // The same, for a narrow glyph whose fallback face misses the cell
        // (`glyphPitch`); it is dressed by its class, so `cls` compares it.
        pitch,
        from: i,
        to: i + (wide ? 2 : 1),
      };
      runs.push(run);
    }
    return runs;
  }

  function sameRun(a, b) {
    return (
      a.text === b.text &&
      a.fg === b.fg &&
      a.bg === b.bg &&
      a.cls === b.cls &&
      a.ls === b.ls &&
      a.href === b.href
    );
  }

  /* Write the FIELDS that moved, not the run.
   *
   * `was` is what this very span currently wears — `painted[index][at]`, the
   * renderer's memory of the DOM. `paintRow` has already decided the two runs
   * differ *somewhere*; this decides where, because a terminal's runs
   * overwhelmingly differ in their text alone. A status line whose number
   * ticked, a log line printed under the one before it in the same colour: the
   * glyphs are new and the dress is identical, and writing `style.color` with
   * the string it already holds is a style attribute mutation, a style recalc
   * for that element, and a paint invalidation — for nothing.
   *
   * `className` was already asked this way and is left as it was: the DOM's
   * own answer is right there and cannot go stale. */
  function dressRun(span, run, was) {
    if (was === undefined || was.text !== run.text) {
      if (span.firstChild === null) span.appendChild(document.createTextNode(run.text));
      else span.firstChild.data = run.text;
    }
    if (was === undefined || was.fg !== run.fg) {
      if (run.fg) span.style.color = run.fg;
      else if (span.style.color) span.style.removeProperty("color");
    }
    // Both ways round, like the background below: a span reused by a run that
    // is not a link would otherwise stay clickable, pointing at wherever the
    // previous occupant pointed.
    if (was === undefined || was.href !== run.href) {
      if (run.href) {
        span.dataset.href = run.href;
        // The tooltip says what the modifier is, because a link that only
        // reveals itself when you happen to be holding a key is a link nobody
        // finds. Orca writes the same sentence into its own hover
        // (`defaultLinkTooltipText`, OnboardingInline…:12997).
        span.dataset.tip = t("term.linkOpen", "{{href}} — {{modifier}}+클릭으로 열기", {
          href: run.href,
          modifier: primaryModifierLabel(),
        });
      } else {
        delete span.dataset.href;
        delete span.dataset.tip;
      }
    }
    // Written both ways round: a span reused by a run with no background of
    // its own would otherwise keep the one the previous occupant set.
    if (was === undefined || was.bg !== run.bg) {
      if (run.bg) span.style.background = run.bg;
      else if (span.style.background) span.style.removeProperty("background");
    }
    if (span.className !== run.cls) span.className = run.cls;
  }

  /* Paint a row by DIFFERENCE — leave every span the frame did not change.
   *
   * This used to open with `node.replaceChildren()`. A frame that changes one
   * run of one row writes one Text node's data and nothing else, which is
   * strictly less work than rebuilding the row — the same reason the
   * tab-switch repaint was made incremental — and it is the shape WebKit's
   * retention path rewards: a Text node replaced is a Text node kept
   * (docs/measurements/rss-growth-verdict-20260831.md).
   *
   * The selection no longer depends on it. It used to — the browser's
   * selection was anchored in these very Text nodes, and leaving them alone
   * was what let it survive a frame — but a scroll moved every row and took
   * it away regardless. The selection is a model of the grid now
   * (`selection`, below): every paint asks it which columns of this row are
   * selected and dresses the runs accordingly, so a frame, a scroll and a
   * repaint all draw the selection where it IS rather than where a Text
   * node used to be. */
  /* What is drawn ON TOP of a row's own styling: search hits and links.
   *
   * Sorted, non-overlapping, in columns. Hits win where the two collide — a
   * person who just searched is looking for the hit, and a run cannot wear
   * both treatments without one of them being a lie.
   *
   * The link scan is a regular expression over the row's text, and this is the
   * one place in the renderer where that is affordable: `paintRow` runs for
   * rows that CHANGED, so an idle screen scans nothing and a busy one scans
   * only what moved.
   *
   * Links are scanned on EVERY screen, including the alternate one and while
   * a program is tracking the mouse. This file used to skip both, reasoning
   * that a link drawn inside a full-screen program cannot be clicked because
   * the program owns the pointer — and the report that disproved it is the
   * ordinary one: an agent TUI prints `http://localhost:5173/` and nobody
   * could click it ("orca처럼 url 클릭하면 보여야함"). Orca keeps its link
   * provider live there and keeps the ACTIVATING press from the child
   * instead (terminal-link-pty-mouse-suppression.ts), which is the rule the
   * press road below now carries. Cost is unchanged: the scan runs per
   * PAINTED row, which is what it always did. */
  /* 소프트 랩으로 갈린 줄을 한 논리 줄로 잇는다.
   *
   * 화면의 행은 「보이는 줄」이고 경로는 「논리 줄」에 산다. 판이 좁아 경로가
   * 오른쪽 끝에서 갈리면 두 행 어디에도 온전한 경로가 없어 **밑줄이 아예 생기지
   * 않는다**(사용자 보고: 「밑줄도 안 생긴다」). Rust 쪽 `flatten_logical`
   * (zerocode-pty grid.rs)이 검색을 위해 이미 같은 일을 하고 있었는데 그 사실이
   * 그리드를 떠나지 않았고, 이제 행마다 `wrap` 으로 떠난다.
   *
   * 양쪽으로 걷는다. 장부는 「나는 앞 행에서 이어졌다」만 말하므로, 줄의 머리인
   * 행은 제가 아래로 이어진다는 것을 **다음 행에게 물어야** 안다 — 묻지 않으면
   * 머리 행은 잘린 경로에 밑줄을 긋는다(그건 없는 것보다 나쁘다: 열리지 않는
   * 링크다).
   *
   * 돌려주는 것은 글자마다 어느 행 어느 칸에서 왔는지다. 넓은 글자의 이음
   * 칸을 건너뛴 뒤에도 열로 되돌아갈 수 있어야 하고, 이어 붙인 다른 행의
   * 글자에 이 행이 밑줄을 그어서는 안 된다.
   *
   * 조립과 링크 스캔은 **줄에 한 번**이면 된다. 행마다 불리되 행이 속한 논리
   * 줄 전체를 다시 잇고 정규식을 처음부터 돌리던 것을, 줄의 머리 행을 열쇠로
   * 붙들어 둔다 — 긴 경로를 찍는 에이전트 판은 한 줄이 k 행에 걸쳐 있고,
   * 전체 다시 그리기(full 프레임, 리사이즈, 숨김 뒤 복귀)는 그 줄의 행 수만큼
   * 같은 글자를 잇고 훑었다. 무엇이 이 기억을 낡게 하는가는 배열의 **동일성**으로
   * 판정한다: `apply` 는 바뀜 행을 새 배열로 갈아 끼우고 `wrap` 은 그 배열의
   * 속성이므로, 줄에 속한 배열들이 그대로이고 줄의 아랫 끝이 여전히 끝이면 조립도
   * 그대로다. 어느 하나라도 갈렸으면 다시 만들고, 지우는 시점은 따로 셀 것이
   * 없다. `termLinkStands` 의 답은 여기 두지 않는다 — 백엔드에 물어 뒤에 바뀌는
   * 것이라 행을 그릴 때마다 새로 묻는다. */
  const logicalLines = new Map();

  function logicalLineAt(index, cells) {
    // 넘겨받은 배열이 장부의 그 행이 아닐 때는 기억을 지나친다 — 밑줄의 열이
    // 그려지는 그 배열을 가리켜야 한다는 계약이 기억보다 앞서며, 그런 배열을
    // 기억해 둘 법은 없다.
    if (cells !== rows[index]) return assembleLogicalLine(index, cells);
    let first = index;
    while (first > 0 && rows[first]?.wrap != null) first -= 1;
    const held = logicalLines.get(first);
    if (held !== undefined && stillTheSameLine(held, first)) return held;
    const made = assembleLogicalLine(index, cells);
    logicalLines.set(first, made);
    return made;
  }

  function stillTheSameLine(held, first) {
    const { lines } = held;
    for (let at = 0; at < lines.length; at += 1) {
      if (rows[first + at] !== lines[at]) return false;
    }
    const after = first + lines.length;
    return after >= rows.length || rows[after]?.wrap == null;
  }

  function assembleLogicalLine(index, cells) {
    let first = index;
    while (first > 0 && rows[first]?.wrap != null) first -= 1;
    let last = index;
    while (last + 1 < rows.length && rows[last + 1]?.wrap != null) last += 1;
    let text = "";
    const rowAt = [];
    const colAt = [];
    const lines = [];
    for (let at = first; at <= last; at += 1) {
      // 지금 그리는 행만은 넘겨받은 배열로 읽는다. 밑줄의 열은 **그려지는 그
      // 배열**을 가리켜야 하고, 한 행에 닿는 길을 둘 두면 언젠가 갈라진다.
      const line = at === index ? cells : rows[at];
      lines.push(rows[at]);
      if (!line) continue;
      // 앞 행에서 셈에 든 칸까지만 붙인다. 넓은 글자가 오른쪽 끝에서 한 칸을
      // 남기고 넘어갔다면 그 칸은 공백이 아니라 아예 쓰이지 않은 자리이므로,
      // 공백으로 읽으면 경로가 거기서 끊긴다 — Rust 쪽 장부가 세는 수가 바로
      // 그 구분이다.
      const upTo = at === last ? line.length : (rows[at + 1]?.wrap ?? line.length);
      for (let col = 0; col < Math.min(upTo, line.length); col += 1) {
        if (line[col].ch === CONTINUATION) continue;
        text += line[col].ch;
        rowAt.push(at);
        colAt.push(col);
      }
    }
    // 그 줄의 주소들, 찾은 순서대로. 어느 행에 얼마만큼 떨어지는가는 읽는 쪽이
    // `rowAt` 로 가른다 — 그 답은 행마다 다르고, 이것은 줄에 하나다.
    const links = [];
    TERM_LINK_PATTERN.lastIndex = 0;
    let found = TERM_LINK_PATTERN.exec(text);
    while (found !== null) {
      // The URL alternative gives its sentence tail back; a path keeps its match.
      const href = found[1] === undefined ? found[0] : trimUrlTail(found[0]);
      links.push({
        head: found.index,
        tail: found.index + href.length - 1,
        href,
        dev: found[1] !== undefined,
      });
      found = TERM_LINK_PATTERN.exec(text);
    }
    return { text, rowAt, colAt, lines, links };
  }

  function marksOf(index, cells) {
    const marks = [];
    const line = absoluteLine(index);
    for (const hit of hitsOn(line)) {
      marks.push({
        from: hit.col,
        to: hit.col + hit.len,
        cls: hit.on ? "term-hit term-hit-on" : "term-hit",
        href: "",
      });
    }
    {
      // Built from the cells rather than from the painted DOM, because the
      // mapping back to columns has to survive wide glyphs — the same reason
      // the selection's word rule reads cells (`termWordSpan`).
      const { rowAt, colAt, links } = logicalLineAt(index, cells);
      for (const link of links) {
        const { head, tail, href } = link;
        // 개발 서버 주소는 줄 하나에 하나다. 이어 붙인 줄은 그 줄에 속한 행마다
        // 다시 훑리므로, 그 말은 매치가 **시작한** 행에서만 한다.
        if (link.dev && rowAt[head] === index) {
          notePotentialDevServerLink(href, owner.address());
        }
        // 이 행에 떨어진 몫만 밑줄이 된다 — 머리가 위에 있으면 그 몫은 위 행이
        // 이미 그었고, 여기서 다시 그으면 열이 어긋난다. `href` 는 몫이 아니라
        // **온전한 경로**다: 어느 조각을 눌러도 같은 것이 열려야 한다.
        let from;
        let to;
        for (let at = head; at <= tail; at += 1) {
          if (rowAt[at] !== index) continue;
          if (from === undefined) from = colAt[at];
          to = colAt[at] + 1;
        }
        // A hit already claims these columns: leave it be. And a path that is
        // not there does not become a link at all — the filter the original
        // buys its guessing with (`termLinkStands`).
        if (from !== undefined && termLinkStands(href) &&
          !marks.some((mark) => mark.from < to && from < mark.to)) {
          marks.push({ from, to, cls: "term-link", href });
        }
      }
    }
    marks.sort((one, two) => one.from - two.from);
    return marks;
  }

  function paintRow(index) {
    const cells = rows[index];
    const node = nodes[index];
    if (!node) return;
    const selected = selectionOn(index);
    const runs = runsOf(cells, marksOf(index, cells), selected);
    paintedSelection[index] = selectionKey(selected);
    const was = painted[index];
    const kids = node.childNodes;
    for (let at = 0; at < runs.length; at += 1) {
      const span = kids[at];
      // Untouched: same text, same dress, and a node already standing there.
      // Skipping the write is the whole point — an assignment to
      // `textContent`, even of an identical string, replaces the Text node and
      // takes the selection down with it.
      if (span !== undefined && was !== undefined && was[at] !== undefined && sameRun(was[at], runs[at])) {
        continue;
      }
      if (span === undefined) {
        const made = document.createElement("span");
        dressRun(made, runs[at], undefined);
        node.appendChild(made);
        continue;
      }
      // What this span is currently wearing, which is what decides how little
      // has to be written to it. `undefined` where the row has never been
      // painted, and a span standing there with nothing remembered about it
      // has to be dressed whole.
      dressRun(span, runs[at], was?.[at]);
    }
    // Whatever the row used to be longer by.
    while (kids.length > runs.length) node.lastChild.remove();
    painted[index] = runs;
  }

  /* Move the nodes whose rows scrolled off down to the bottom, so `nodes[i]`
   * still draws `rows[i]`.
   *
   * The shift contract does not resend the rows that moved — only the exposed
   * bottom — so the consumer owes the shift. This side was doing it to the
   * MODEL and not to the DOM, and the repaint that was supposed to cover the
   * difference could not run (see `repaint`). A screen that had been sitting
   * still and then printed one line drew the line it printed and kept every
   * row above it one position too low: `AAA/BBB/CCC` + `DDD` came out
   * `AAA/BBB/DDD` where the shell had `BBB/CCC/DDD`. It self-healed only
   * while output was fast enough that every row was still dirty and got
   * resent anyway, which is why it reads as a terminal that goes wrong when
   * it is quiet.
   *
   * Moving n elements rather than repainting the screen is the point: a shell
   * printing at speed does this on every frame. */
  function shiftNodes(count) {
    // Sound only while the DOM's order still matches the array's: the move is
    // an `appendChild`, which lands at the very END, so a node the same breath
    // added below would be jumped over. The contract says that cannot happen —
    // a size change arrives as a FULL frame and a full frame never carries a
    // shift — and this is the O(1) way to keep believing it rather than
    // assuming it. If the two ever disagree, the whole screen is owed.
    if (nodes.length > 0 && pre.lastElementChild !== nodes.at(-1)) {
      behindAll = true;
      repaint();
      return;
    }
    for (let at = 0; at < count; at += 1) {
      const node = nodes.shift();
      pre.appendChild(node);
      nodes.push(node);
      // The spans travel with the row node, so their dress memory must travel
      // with it too. The exposed bottom row then updates those Text nodes and
      // palette classes in place. Emptying the node here discarded one row of
      // spans on nearly every streaming frame — the exact WebKit retention
      // path `dressRun` avoids.
      const remembered = painted.shift();
      painted.push(remembered);
      paintedSelection.push(paintedSelection.shift());
    }
  }

  /* The mirror of `shiftNodes`, for a view that moved back into history: the
   * bottom rows leave and the same nodes stand in at the top, so `nodes[i]`
   * still draws `rows[i]` and the exposed top rows are painted in place. The
   * dress memory travels with the nodes for the reason `shiftNodes` gives. */
  function shiftNodesBack(count) {
    if (nodes.length > 0 && pre.firstElementChild !== nodes[0]) {
      behindAll = true;
      repaint();
      return;
    }
    for (let at = 0; at < count; at += 1) {
      const node = nodes.pop();
      pre.insertBefore(node, pre.firstElementChild);
      nodes.unshift(node);
      painted.unshift(painted.pop());
      paintedSelection.unshift(paintedSelection.pop());
    }
  }

  /* Pay what the DOM owes the model: the net row move, then the rows.
   *
   * The rows it owes, not every row. A view keeps its model current the whole
   * time, painted or not, so the DOM is owed exactly what moved — which for a
   * terminal nobody wrote to is nothing at all, for the common one (an agent's
   * spinner, a status line) is one row, and for several frames painted together
   * is ONE node move plus the rows they exposed. */
  function repaint() {
    // A row count the DOM has not followed yet, under a move, leaves no node
    // where the move assumes one: the whole screen is owed instead.
    if (nodes.length !== rows.length) {
      if (behindShift !== 0) behindAll = true;
      followRowCount();
    }
    if (behindAll) {
      behindAll = false;
      behindShift = 0;
      behindRows.clear();
      for (const index of rows.keys()) paintRow(index);
      applyFolds();
      return;
    }
    if (behindShift !== 0) {
      const shift = behindShift;
      behindShift = 0;
      if (shift > 0) shiftNodes(shift);
      else shiftNodesBack(-shift);
    }
    if (behindRows.size === 0) return;
    const stale = [...behindRows];
    behindRows.clear();
    // A row named before the screen shrank is a row that is no longer there.
    for (const index of stale) if (index < rows.length) paintRow(index);
    applyFolds();
  }

  function setPaintPaused(paused) {
    if (paintPaused === paused) return;
    paintPaused = paused;
    if (!paintPaused && !host.hidden) repaint();
  }

  /* Put a late "does that path exist" answer on the screen.
   *
   * `repaint` alone cannot do it. That hand chases rows the MODEL ran ahead of
   * (`behindRows`); what moved here is not the model but the verdict — the same
   * cells now carry a link where a moment ago they did not. With no row owed,
   * `repaint` returns at its own first line and the mark never lands. Measured:
   * the answer arrived, the cache held `true`, and the screen stayed bare.
   *
   * So the whole screen is owed, and a painted view pays it now: the selection
   * is a model the paint reads (`selectionOn`), so a mark landing on the run it
   * sits in redraws the run wearing both, and takes nothing away. A hidden or
   * paused view pays it when it shows. */
  function relink() {
    behindAll = true;
    if (!host.hidden && !paintPaused) repaint();
  }

  /* The model's rows moved by `count` (negative: down) — owed to the DOM as
   * one net move, with the rows already owed moving along with it. A move as
   * large as the screen is not a move at all: every row is owed. */
  function oweShift(count) {
    if (behindAll) return;
    behindShift += count;
    if (Math.abs(behindShift) >= rows.length) {
      behindAll = true;
      behindShift = 0;
      behindRows.clear();
      return;
    }
    if (behindRows.size === 0) return;
    const moved = [...behindRows];
    behindRows.clear();
    for (const index of moved) {
      const to = index - count;
      if (to >= 0 && to < rows.length) behindRows.add(to);
    }
  }

  /* One frame into the MODEL — rows, position, history, caret wanted — and
   * what the DOM now owes it, written down. Nothing here touches the DOM, so
   * any number of frames can be taken before one paint (`applyFrames`). */
  function update(delta) {
    const [rowCount, colCount] = delta.size;
    setSize(rowCount);
    screenCols = colCount ?? 0;
    rowLines = delta.row_lines ?? null;
    // The buffer origin is state, independent of DOM row shifts. Full/fold
    // frames, repeated snapshots and a reveal after missed frames all carry
    // the same cumulative count. The first frame establishes this view's
    // origin; old/replayed frames without it leave the known origin alone.
    const trimmed = delta.scrollback_trimmed ?? scrollbackTrimmed;
    if (trimmed !== null && scrollbackTrimmed !== null) {
      selection.shift(trimmed - scrollbackTrimmed);
    }
    scrollbackTrimmed = trimmed;

    /* Where this frame sits in history, before anything is painted.
     *
     * Before, because `paintRow` reads `viewTop` to decide which search hits
     * and which links belong to a row — a row painted against the PREVIOUS
     * frame's position highlights the line that used to be there. `?? 0` so a
     * frame from an older backend, or the board's replayed snapshots, still
     * paint: the fields are an extension, and an extension nobody sent means
     * "live screen, no history", which is exactly what those frames are.
     *
     * Tracked on hidden views too. A tab in the background keeps its model
     * current the whole time it is away, and its scrollbar is part of that
     * model — coming back to a stale thumb would be the same class of bug as
     * coming back to a stale row. */
    viewOffset = delta.view_offset ?? 0;
    scrollbackLen = delta.scrollback_len ?? 0;
    viewTop = scrollbackLen - viewOffset;
    altScreen = delta.alt_screen === true;
    for (const meta of delta.folds ?? []) {
      foldSummaries.set(meta.id, {
        collapsed: meta.collapsed === true,
        summary: meta.summary ?? "",
      });
    }

    /* The shift contract, applied to the model and owed to the DOM.
     *
     * The rows that moved are NOT resent — only the exposed edge — so the DOM
     * owes a node move (`oweShift`), never a repaint of the rows that merely
     * moved. Moving every node is moving none of them: a move as large as the
     * screen owes every row instead, which is also what a whole page scrolled
     * in one frame costs (one drained round carries thousands of lines). */
    if (delta.full) {
      for (const index of rows.keys()) rows[index] = [];
      // Every row was just replaced, so naming rows would be a lie.
      behindAll = true;
      behindShift = 0;
      behindRows.clear();
    } else if (delta.scrolled_lines > 0) {
      // The shift: rows moved up; only the exposed bottom is resent.
      rows.splice(0, delta.scrolled_lines);
      while (rows.length < rowCount) rows.push([]);
      oweShift(Math.min(delta.scrolled_lines, rowCount));
    } else if (delta.view_shift) {
      // The view moved over unchanged history (grid.rs, the view shift): the
      // rows a person is reading move as one block and only the exposed edge
      // is resent. Positive is toward the live screen — the rows move up, the
      // motion an output shift makes; negative is back into history — the
      // rows move down and the top is exposed. Repainting the whole window a
      // notch at a time was the scroll stutter (12.8 ms a notch at 60×220,
      // against 0.5 ms for this).
      const count = Math.min(Math.abs(delta.view_shift), rows.length);
      if (delta.view_shift > 0) {
        rows.splice(0, count);
        while (rows.length < rowCount) rows.push([]);
        oweShift(count);
      } else {
        rows.splice(rows.length - count, count);
        while (rows.length < rowCount) rows.unshift([]);
        oweShift(-count);
      }
    }

    for (const row of delta.rows) {
      rows[row.index] = row.cells ?? expandPackedRow(row);
      /* 「앞 행에서 몇 칸이 셈에 들었나」를 셀 배열 자신에 얹는다.
       *
       * 나란한 두 번째 배열이 아니라 배열의 속성인 이유: `rows.splice`가 행을
       * 위로 밀 때 장부가 행과 **함께** 움직여야 한다. 따로 두면 스크롤 한 번에
       * 어긋나고, 어긋난 장부는 없는 것보다 나쁘다 — 엉뚱한 두 행을 한 줄로
       * 이어 붙인다. 값이 없으면 「제 줄을 스스로 시작한다」는 뜻이고, 그것이
       * 이 필드를 보내지 않는 옛 백엔드의 프레임에도 맞는 답이다
       * (zerocode-pty grid.rs 의 `WireRow::wrap`). */
      rows[row.index].wrap = row.wrap ?? null;
      rows[row.index].fold = row.fold ?? null;
      /* 갈아 끼운 다음에 그린다 — 한 행씩 바꾸며 그리지 않고.
       *
       * 링크의 밑줄은 행이 속한 논리 줄 전체를 읽어 결정된다. 같은 줄의 두 행이 한
       * 프레임에 온 것을 한 행씩 갈면서 그리면, 앞 행은 뒷 행의 지나간 글자를 보고
       * 밑줄을 그어 다음 프레임까지 어긋난 채로 서 있고, 그 줄의 조립은 두 번 일어난다.
       * 그래서 여기서는 빚만 적고, 새 줄이 전부 자리를 잡은 뒤 한 번에 그린다. */
      if (!behindAll) behindRows.add(row.index);
    }
    caretWanted = { cursor: delta.cursor, visible: delta.cursor_visible !== false };
  }

  /* What the model owes the DOM, paid once — the rows, the folds, the caret,
   * the rail and the selection. A hidden view, or one whose compositor a
   * top-layer control has taken, keeps the debt: painting hidden terminals is
   * work on the very thread that has to deliver the next keystroke, and with a
   * few tabs open it is felt as typing lag. */
  function paint() {
    if (host.hidden || paintPaused) return;
    repaint();
    applyFolds();
    if (caretWanted === null) return;
    const [cursorRow, cursorCol] = caretWanted.cursor;
    // A caret that moved is a caret somebody is typing under, and every real
    // terminal holds it SOLID through a keystroke — the blink is an idle
    // gesture. Left alone, the CSS animation keeps its own clock, so the
    // letter can land in the caret's dark phase and read as a missed beat.
    // Rewinding the running animation costs no layout; restarting the
    // animation property would.
    if (cursorRow !== caretAt[0] || cursorCol !== caretAt[1]) {
      caretAt[0] = cursorRow;
      caretAt[1] = cursorCol;
      // Once per keystroke, not once per move. `getAnimations()` flushes
      // style before it answers, and a streaming program moves the cursor on
      // every frame — so this line was a forced style pass per frame over
      // the rows the same frame had just dirtied, for a caret nobody was
      // typing under (measured: 120 recalcs in a 120-frame burst, 0 with the
      // guard). The first move after a key is the echo that key is waiting on.
      if (caretHeldFor !== typedSerial) {
        caretHeldFor = typedSerial;
        for (const beat of caret.getAnimations?.() ?? []) beat.currentTime = 0;
      }
    }
    // cursorRow × cell.height, and the cell is TRUSTED here — the paint path
    // reads no layout (the storm gate counts every such read). What keeps the
    // trust honest is the root-style observer beside `fontGeneration`: any
    // token move that would let the live rows' calc drift from this snapshot
    // schedules a re-measure off the storm path (세 번째 캐럿 드리프트,
    // 라이브 보고 2026-08-25 "노란 커서가 제 줄 위에 떠 있다").
    const caretRow = cursorRow - (hiddenBefore[cursorRow] ?? 0);
    caret.style.transform = `translate(${cursorCol * cell.width}px, ${caretRow * cell.height}px)`;
    caret.style.height = `${cell.height}px`;
    // A block, and exactly one cell of it — Orca's `cursorStyle: "block"`.
    // Written from the MEASURED width for the same reason the pty is sized
    // from it: the stylesheet knows the font size, not what the face that
    // actually drew answered, and a caret a fraction of a column off is
    // visible on every prompt.
    caret.style.width = `${cell.width}px`;
    const cursorCell = rows[cursorRow]?.[cursorCol];
    const cursorGlyph = cursorCell?.ch === CONTINUATION
      ? ""
      : `${cursorCell?.ch ?? " "}${cursorCell?.zw ?? ""}`;
    if (caret.textContent !== cursorGlyph) caret.textContent = cursorGlyph;
    const cursorStyle = cursorCell?.style ?? PLAIN_STYLE;
    caret.classList.toggle("bold", cursorStyle.bold === true);
    caret.classList.toggle("italic", cursorStyle.italic === true);
    // A TUI hides the caret while it redraws and asks for it back on its own
    // prompt (`CSI ?25l`/`h`). Drawing one anyway puts a caret in the middle
    // of an interface that deliberately had none. Absent means visible, which
    // is what a terminal does before anybody says otherwise.
    caret.hidden = !caretWanted.visible || nodes[cursorRow]?.hidden === true;
    paintRail();
    // A drag held past an edge grows with every frame the auto-scroll lands:
    // the far end is the edge row of the screen that is NOW on view. Then the
    // rows whose selection moved are painted — the frame's own paint already
    // drew the ones it touched, so this finds only what it did not.
    if (selectDrag !== null && selectDrag.edge !== 0) {
      extendToEdge();
    }
    // A shift can recycle row nodes without repainting them. On the
    // alternate screen their text moves while the selected coordinates stay
    // put, so settle the selection on those rows too.
    repaintSelection();
  }

  /* One frame, painted. */
  function apply(delta) {
    update(delta);
    paint();
  }

  /* Several frames of one screen — what one pull brought — painted ONCE.
   * Every frame reaches the model in order; the DOM is told only the sum, so a
   * frame no person could have seen costs no DOM work (design rule 7). */
  function applyFrames(frames) {
    for (const delta of frames) update(delta);
    paint();
  }

  function reset() {
    rows.length = 0;
    logicalLines.clear();
    nodes.forEach((node) => node.remove());
    nodes.length = 0;
    painted.length = 0;
    paintedSelection.length = 0;
    // The float reuses one view across shells: what was selected in the last
    // one names lines of a buffer that is gone.
    selection.clear();
    rowLines = null;
    endSelectDrag();
    foldSummaries.clear();
    hiddenBefore.length = 0;
    foldsShown = false;
    for (const handle of foldHandles) handle.hidden = true;
    viewOffset = 0;
    scrollbackLen = 0;
    scrollbackTrimmed = null;
    viewTop = 0;
    rail.hidden = true;
    toLive.hidden = true;
    // The debt goes with the rows it was owed on: a view reset and reused —
    // the float, on its way to another shell — would otherwise be told to
    // repaint rows that no longer exist.
    behindRows.clear();
    behindAll = false;
    behindShift = 0;
    caretWanted = null;
    caret.hidden = true;
    // The float reuses one view across shells — the next occupant's first
    // frame must read as a move, not as the old caret standing still.
    caretAt[0] = -1;
    caretAt[1] = -1;
  }

  /* ---- the scrollbar ----
   *
   * An overlay, hand-drawn, because this surface is not a scroller: `.term`
   * is `overflow: hidden` and the history lives in Rust. That is also why
   * there was no scrollbar to begin with — not a piece of UI nobody wrote,
   * but a frame that carried no position to draw one from. The two fields
   * (`view_offset`, `scrollback_len`) are what made it possible at all.
   *
   * Built here rather than in markup so all three screens get one from a
   * single door — the lane's, the float's, and every terminal tab's, which are
   * made at runtime. It lives in `host` and never in `pre`: `pre` is where the
   * row nodes go and `setSize` owns that list.
   *
   * Absolutely positioned, and that is load-bearing. `gridSize` measures the
   * host minus its stylesheet padding to decide how many rows the pty gets, so
   * anything in the flow here would be counted as screen and the bottom row
   * would sit behind it. */
  const rail = document.createElement("div");
  rail.className = "term-rail";
  rail.hidden = true;
  const thumb = document.createElement("div");
  thumb.className = "term-thumb";
  rail.appendChild(thumb);
  /* The way back to live. Shown only while there is a "back" to come from —
   * a control that is always there is a control that says nothing. */
  const toLive = document.createElement("button");
  toLive.type = "button";
  toLive.className = "term-to-live";
  toLive.hidden = true;
  host.append(rail, toLive);

  /* Fold handles live outside row nodes so row-run reconciliation and text
   * selection remain untouched. Their position uses measured grid cells and
   * therefore adds no layout read to the frame path. */
  const foldLayer = document.createElement("div");
  foldLayer.className = "term-folds";
  host.appendChild(foldLayer);
  const foldHandles = [];

  function foldKey(id) {
    return `${owner.address()?.key ?? "view"}\u0000${id}`;
  }

  function foldIsCollapsed(id) {
    const answered = foldCollapsed.get(foldKey(id));
    if (answered !== undefined) return answered;
    return foldSummaries.get(id)?.collapsed === true;
  }

  function toggleFold(id) {
    const collapsed = !foldIsCollapsed(id);
    foldCollapsed.set(foldKey(id), collapsed);
    applyFolds();
    // The same answer goes to the grid: the live screen is hidden here, but a
    // scrolled frame is composed there, and only the grid can pull the rows
    // that follow a hidden body into its vacated slots.
    const address = owner.address();
    if (address === null) return;
    if (address.kind === "term") {
      invoke("term_fold", { term: address.term, id, collapsed }).catch(() => {});
    } else {
      invoke("lane_fold", { id: address.id, fold: id, collapsed }).catch(() => {});
    }
  }

  function dressFoldHandle(slot, visualRow, id, collapsed) {
    let node = foldHandles[slot];
    if (node === undefined) {
      node = document.createElement("span");
      node.className = "term-fold";
      actsAsButton(node, () => toggleFold(Number(node.dataset.foldId)));
      foldLayer.appendChild(node);
      foldHandles[slot] = node;
    }
    node.hidden = false;
    node.dataset.foldId = String(id);
    const action = collapsed
      ? t("term.foldExpand", "펼치기")
      : t("term.foldCollapse", "접기");
    node.dataset.tip = action;
    const summary = foldSummaries.get(id)?.summary ?? "";
    node.setAttribute("aria-label", summary ? `${summary} — ${action}` : action);
    node.setAttribute("aria-expanded", collapsed ? "false" : "true");
    node.textContent = collapsed ? "▸" : "▾";
    node.style.transform = `translateY(${visualRow * cell.height}px)`;
    node.style.width = `${cell.width}px`;
    node.style.height = `${cell.height}px`;
  }

  function applyFolds() {
    const present = rows.some((row) => row?.fold);
    if (!present && !foldsShown) return;
    foldsShown = present;
    let hidden = 0;
    let handles = 0;
    for (let index = 0; index < rows.length; index += 1) {
      const fold = rows[index]?.fold ?? null;
      const node = nodes[index];
      hiddenBefore[index] = hidden;
      if (node === undefined) continue;
      const collapsed = fold !== null && foldIsCollapsed(fold.id);
      /* A scrolled view is an absolute row-for-slot frame: hiding a body here
       * cannot pull replacement rows into the vacated slots. So the grid
       * composes scrollback without collapsed bodies (`term_fold`), and this
       * side hides only on the live screen. The handle still tells the truth
       * about the answer in both places. */
      const hide =
        viewOffset === 0 &&
        ((fold?.role === "body" && collapsed) || (fold?.role === "teaser" && !collapsed));
      node.hidden = hide;
      if (hide) {
        hidden += 1;
      } else if (fold?.role === "header") {
        dressFoldHandle(handles, index - hidden, fold.id, collapsed);
        handles += 1;
      }
    }
    hiddenBefore.length = rows.length;
    for (let at = handles; at < foldHandles.length; at += 1) {
      foldHandles[at].hidden = true;
    }
  }

  pre.addEventListener("click", (event) => {
    if (programHasMouse()) return;
    // A click that ended a drag is the drag's, not the fold's.
    if (selection.stands()) return;
    const row = rowUnder(event);
    const fold = row === null ? null : rows[row.index]?.fold ?? null;
    if (fold?.role === "header") toggleFold(fold.id);
  });

  /* How tall the track is — WATCHED, never measured from inside a frame.
   *
   * `paintRail` runs at the tail of every `apply`, and the line it used to
   * open with (`rail.getBoundingClientRect().height`) is a forced layout of a
   * screen whose thirty changed rows were dirtied two statements earlier. It
   * is the single most expensive thing a streaming terminal did in this
   * window, and nothing about it is visible in the code: the read looks like
   * arithmetic. Measured on the burst rig (`ui/tests/window.mjs`, 300 frames
   * of 20–40 changed rows): 883ms of `apply` with the read, 282ms with the
   * rail short-circuited — two thirds of the cost of a watched shell was one
   * number that only moves when somebody drags a splitter.
   *
   * A `ResizeObserver` answers the same question from *after* layout, where
   * the box has already been computed, so asking costs nothing. The value is
   * seeded lazily rather than assumed: an observation lands a frame after the
   * rail is revealed, and a thumb drawn against zero is a thumb in the wrong
   * place — so the one frame that reveals the track still pays for it, once.
   */
  let trackHeight = 0;
  const trackWatch = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const box = entry.borderBoxSize?.[0];
      trackHeight = box ? box.blockSize : entry.contentRect.height;
    }
  });
  trackWatch.observe(rail);

  function railTrack() {
    if (trackHeight === 0) trackHeight = rail.getBoundingClientRect().height;
    return trackHeight;
  }

  /* One scroll command per painted frame, never one per DOM event.
   *
   * A trackpad fling speaks a hundred wheel events a second and a thumb drag
   * one mousemove per pixel, and each used to cross to the backend as its
   * own command — a hundred lock-takes a second against the pump that is
   * busy producing the very frames the scroll asks for, felt exactly as
   * 스크롤 잔랙 (reported). Worse, the drag computed every step against a
   * `viewOffset` that only advances when a frame lands, so the moves between
   * two frames re-sent the same distance and the view rubber-banded.
   *
   * The bank holds both intents: RELATIVE rows sum (a wheel is additive),
   * and an ABSOLUTE target keeps only its last word (a drag names a place,
   * not a distance) — converted to lines at flush time against the freshest
   * offset, once per frame, so staleness is bounded by one frame and
   * self-corrects as frames land. The timeout floor mirrors the agent-paint
   * flusher above it: rAF alone stalls when the window is throttled, and a
   * banked scroll that waits for a repaint nobody scheduled would arrive
   * after the finger has stopped. */
  let pendingLines = 0;
  let pendingOffset = null;
  let scrollFlightFrame = null;
  let scrollFlightTimer = null;
  function flushScroll() {
    if (scrollFlightFrame !== null) cancelAnimationFrame(scrollFlightFrame);
    if (scrollFlightTimer !== null) clearTimeout(scrollFlightTimer);
    scrollFlightFrame = null;
    scrollFlightTimer = null;
    let lines = pendingLines;
    pendingLines = 0;
    if (pendingOffset !== null) {
      lines += pendingOffset - viewOffset;
      pendingOffset = null;
    }
    const address = owner.address();
    if (address === null || lines === 0) return;
    if (address.kind === "term") {
      invoke("term_scroll", { term: address.term, lines }).catch(() => {});
    } else {
      invoke("lane_scroll", { id: address.id, lines }).catch(() => {});
    }
  }
  function scheduleScroll() {
    if (scrollFlightFrame !== null || scrollFlightTimer !== null) return;
    scrollFlightFrame = requestAnimationFrame(flushScroll);
    scrollFlightTimer = setTimeout(flushScroll, SCROLL_FLIGHT_FLOOR_MS);
  }
  function scrollTo(lines) {
    if (lines === 0) return;
    pendingLines += lines;
    scheduleScroll();
  }
  /* The thumb's road: a place, not a distance. */
  function scrollToOffset(offset) {
    pendingOffset = offset;
    scheduleScroll();
  }

  /* Draw the thumb from the numbers the last frame carried.
   *
   * The whole of a scrollbar is three quantities: how much there is, how much
   * of it is showing, and where the showing part starts. History plus screen
   * is the first, the row count is the second, and `scrollback_len -
   * view_offset` is the third — the top line of what is on screen. */
  function paintRail() {
    const total = scrollbackLen + rows.length;
    // Nothing behind the screen, or a full-screen program that owns its own
    // surface and has no history at all: no bar, because there is no distance
    // to report and a permanent empty track is furniture.
    // Written only when it changes: this runs at the tail of every frame, and
    // an attribute set to the value it already holds is still a mutation
    // record and a style invalidation — the freeze probe counted the two
    // `hidden` writes below at 120 a second on a streaming shell with no
    // history at all.
    const bare = scrollbackLen === 0 || altScreen || rows.length === 0;
    if (rail.hidden !== bare) rail.hidden = bare;
    if (bare) {
      if (!toLive.hidden) toLive.hidden = true;
      return;
    }
    const track = railTrack();
    const share = rows.length / total;
    // A floor, or a deep scrollback makes the thumb a line nobody can grab.
    // Orca's xterm does the same (`MINIMUM_SLIDER_SIZE`).
    const height = Math.max(TERM_THUMB_MIN, track * share);
    const room = Math.max(0, track - height);
    // 1 is the bottom, which is where the live screen is.
    const at = scrollbackLen === 0 ? 1 : (scrollbackLen - viewOffset) / scrollbackLen;
    thumb.style.height = `${height}px`;
    thumb.style.transform = `translateY(${room * at}px)`;
    rail.classList.toggle("is-back", viewOffset > 0);
    const live = viewOffset === 0;
    if (toLive.hidden !== live) toLive.hidden = live;
  }

  /* Dragging the thumb is the same scroll the wheel does, in the same units.
   *
   * The gesture is measured against the track and turned into LINES here,
   * because the backend's one door for moving a view takes lines — a second
   * door taking pixels would be a second place the clamping rules live. */
  let dragFrom = null;
  thumb.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    // The screen underneath must not also answer this: a drag on the bar is
    // not a selection, and it is not a click for a program that is tracking.
    event.preventDefault();
    event.stopPropagation();
    dragFrom = { y: event.clientY, offset: viewOffset };
    rail.classList.add("is-held");
  });
  window.addEventListener("mousemove", (event) => {
    if (dragFrom === null) return;
    // The same watched number: a drag is a mousemove per pixel, and measuring
    // the track under the pointer would force a layout on every one of them
    // while the screen behind it is still streaming.
    const track = railTrack();
    const total = scrollbackLen + rows.length;
    if (track <= 0 || total === 0) return;
    const height = Math.max(TERM_THUMB_MIN, track * (rows.length / total));
    const room = Math.max(1, track - height);
    // Down the track is toward the live screen, which is a SMALLER offset.
    const moved = ((event.clientY - dragFrom.y) / room) * scrollbackLen;
    const wanted = Math.round(dragFrom.offset - moved);
    const clamped = Math.min(scrollbackLen, Math.max(0, wanted));
    scrollToOffset(clamped);
  });
  window.addEventListener("mouseup", () => {
    if (dragFrom === null) return;
    dragFrom = null;
    rail.classList.remove("is-held");
  });

  /* A click on the empty track pages toward it, which is what every scrollbar
   * in every application does and the reason a track is clickable at all. */
  rail.addEventListener("mousedown", (event) => {
    if (event.button !== 0 || event.target !== rail) return;
    event.preventDefault();
    event.stopPropagation();
    const box = rail.getBoundingClientRect();
    const above = event.clientY < thumb.getBoundingClientRect().top;
    scrollTo(above ? rows.length : -rows.length);
    void box;
  });

  toLive.addEventListener("mousedown", (event) => {
    event.preventDefault();
    event.stopPropagation();
  });
  toLive.addEventListener("click", () => {
    // Back to zero in one move rather than a scroll of the right size: the
    // control means "live", and a computed distance would be a second opinion
    // about where live is.
    scrollToOffset(0);
  });

  /* ---- ⌘F, in the terminal ----
   *
   * ⌘F used to return immediately unless the tab was a file, so in a terminal
   * it did nothing at all — which is worse than an absent feature, because the
   * key is a key everybody presses and silence reads as broken.
   *
   * The bar is the file finder's, by class: `.file-find` and its children
   * already carry every rule for the row, the field, the count and the
   * buttons, so this is the same bar with a different engine behind it and no
   * second stylesheet to keep in step. What it does NOT carry is replace —
   * there is nothing to write back into a terminal's history.
   *
   * Built here, per screen, so the lane, the float and every terminal tab get
   * one from one door. */
  const find = document.createElement("div");
  find.className = "file-find term-find";
  find.hidden = true;
  const findField = document.createElement("input");
  findField.type = "search";
  findField.className = "file-find-field";
  findField.spellcheck = false;
  const findCount = document.createElement("span");
  findCount.className = "file-find-count";
  const findCase = document.createElement("button");
  findCase.type = "button";
  findCase.className = "file-find-case";
  findCase.textContent = "Aa";
  findCase.setAttribute("aria-pressed", "false");
  const findPrev = document.createElement("button");
  findPrev.type = "button";
  findPrev.className = "file-find-step";
  findPrev.textContent = "↑";
  const findNext = document.createElement("button");
  findNext.type = "button";
  findNext.className = "file-find-step";
  findNext.textContent = "↓";
  const findShut = document.createElement("button");
  findShut.type = "button";
  findShut.className = "file-find-close";
  findShut.textContent = "×";
  find.append(findField, findCount, findCase, findPrev, findNext, findShut);
  host.appendChild(find);

  let findCaseOn = false;

  /* The words on the bar, in the language in force.
   *
   * Written through `say` for the reason every keyed element is: `applyLocale`
   * walks the markup and rewrites from the key, so a bare `textContent` would
   * be erased at the next language change. These nodes are built at runtime
   * and carry no key, so they are re-labelled from here whenever the bar is
   * painted — which includes every locale change, because `applyLocale` is
   * followed by a repaint of what is open. */
  function labelFind() {
    findField.placeholder = t("term.find", "터미널에서 찾기");
    findCase.setAttribute("aria-label", t("file.findCase", "대소문자 구분"));
    findPrev.setAttribute("aria-label", t("file.findPrev", "이전 일치"));
    findNext.setAttribute("aria-label", t("file.findNext", "다음 일치"));
    findShut.setAttribute("aria-label", t("file.findClose", "찾기 닫기"));
  }

  /* Index the hits by line and repaint, so the highlight follows the list.
   *
   * Every road that changes what is found ends here rather than each doing its
   * own bookkeeping — a search, a step, a case toggle, a close. */
  function settleHits() {
    hitsByLine = new Map();
    for (const [at, hit] of hits.entries()) {
      const here = hitsByLine.get(hit.line) ?? [];
      here.push({ col: hit.col, len: hit.len, on: at === hitAt });
      hitsByLine.set(hit.line, here);
    }
    findCount.textContent =
      hits.length === 0
        ? findField.value === ""
          ? ""
          : t("file.findNone", "일치 없음")
        : t("file.findHits", "{{at}}/{{count}}", {
            at: hitAt + 1,
            count: hits.length >= TERM_SEARCH_MAX ? `${TERM_SEARCH_MAX}+` : hits.length,
          });
    // Every row's marks may have changed, and the run cache compares the
    // classes a mark puts on a run — so a repaint from the model is what makes
    // the highlight appear, and it repaints only the runs that differ.
    behindAll = true;
    repaint();
  }

  async function runTermFind(step) {
    const address = owner.address();
    if (address === null || address.kind !== "term") return;
    const query = findField.value;
    if (query === "") {
      hits = [];
      hitAt = -1;
      settleHits();
      return;
    }
    let found;
    try {
      found = await invoke("term_search", { term: address.term, query, caseSensitive: findCaseOn });
    } catch {
      return;
    }
    hits = found ?? [];
    if (hits.length === 0) {
      hitAt = -1;
      settleHits();
      return;
    }
    if (step === 0) {
      // Opening the bar lands on the LAST hit, not the first: a terminal is
      // read from the bottom, and the most recent occurrence is the one
      // somebody searching a log is nearly always after.
      hitAt = hits.length - 1;
    } else {
      hitAt = (hitAt + step + hits.length) % hits.length;
    }
    await showHit();
  }

  /* Put the current hit on screen. The grid answers where the view landed, so
   * the thumb is right on the same frame rather than one behind. */
  async function showHit() {
    const address = owner.address();
    const hit = hits[hitAt];
    if (address === null || address.kind !== "term" || hit === undefined) {
      settleHits();
      return;
    }
    try {
      const landed = await invoke("term_view_to_line", { term: address.term, line: hit.line });
      if (typeof landed === "number") {
        viewOffset = landed;
        viewTop = scrollbackLen - viewOffset;
      }
    } catch {
      // The jump failing is not a reason to lose the list — the hits are
      // still true, they are simply not on screen.
    }
    settleHits();
    paintRail();
  }

  function openTermFind() {
    find.hidden = false;
    labelFind();
    findField.focus();
    findField.select();
    void runTermFind(0);
  }

  function closeTermFind() {
    find.hidden = true;
    hits = [];
    hitAt = -1;
    settleHits();
    keySink.focus();
  }

  // The bar's own keys never reach the terminal: a person typing a query is
  // not typing at the shell. Without this the query would also be entered at
  // the prompt, because the window's key sink is what a terminal listens to.
  find.addEventListener("mousedown", (event) => event.stopPropagation());
  find.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Escape") {
      closeTermFind();
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      void runTermFind(event.shiftKey ? -1 : 1);
    }
  });
  findField.addEventListener("input", () => {
    void runTermFind(0);
  });
  findCase.addEventListener("click", () => {
    findCaseOn = !findCaseOn;
    findCase.setAttribute("aria-pressed", String(findCaseOn));
    void runTermFind(0);
  });
  findPrev.addEventListener("click", () => void runTermFind(-1));
  findNext.addEventListener("click", () => void runTermFind(1));
  findShut.addEventListener("click", () => closeTermFind());

  function gridSize() {
    // Computed values, not a rect: the float measures its grid while its
    // `surface-enter` frame still wears scale(0.95), and a rect read through
    // that transform sized the pty against a box that does not exist — the
    // same lie `measure` used to record through its probe. Under the
    // window's border-box sizing the computed width IS the border box, so
    // the stylesheet's padding comes off it exactly as it came off the rect
    // — all from the one style object this function was already paying for.
    const style = getComputedStyle(host);
    const width = parseFloat(style.width);
    const height = parseFloat(style.height);
    // `blind` marks the guess: a host that lays nothing out — display:none
    // somewhere above it — computes no width, and the numbers below are a
    // default, not a measurement. A caller SIZING something new wants the
    // default; a caller about to TELL a pty its room must not repeat a guess
    // (`resizeTermTab` refuses on it).
    if (!Number.isFinite(width) || width <= 0) return { rows: 24, cols: 96, blind: true };
    const grid = gridFor(width, height, style);
    placeGrid(grid);
    return grid;
  }

  /* Balance the slack. The box holds whole cells only, and what is left past
   * the last one — up to a cell's width, up to a row's height — used to sit
   * entirely at the right and the bottom, where the history rail stood in it.
   * A full-screen program draws rules the whole width, so once the rail was
   * gone that strip read as a ragged margin (라이브 보고 2026-09-15 "여백이
   * 생겨서"). Half of it now goes to each edge and the grid sits centred, the
   * way Ghostty balances its padding. Two custom properties on the host, read
   * by the stylesheet for the grid's margin and for every overlay anchored at
   * the grid's origin (caret, preedit, fold handles); the pointer-to-cell
   * arithmetic reads the grid's own box, so it moves with it. Written from
   * the same measurement that sizes the pty — the one moment the box and the
   * grid are both known — and guarded, since that measurement is frequent. */
  function placeGrid(grid) {
    // Whole pixels: half a fractional slack would put the grid's origin
    // between pixels — glyphs blurred, and a pointer (whose coordinates
    // arrive as integers) landing one cell short at a cell's left edge. The
    // odd pixel stays at the right and the bottom, where nobody can see it.
    writeCustomProperty(host, "--term-inset-x", `${Math.floor(grid.slackX / 2)}px`);
    writeCustomProperty(host, "--term-inset-y", `${Math.floor(grid.slackY / 2)}px`);
  }

  /* The grid a box of `width`×`height` CSS pixels holds under this view's cell
   * size and the stylesheet's terminal padding — the ruler `spawnGrid` reads
   * for a pane that does not exist yet. */
  function gridFor(width, height, style = getComputedStyle(host)) {
    const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    // Clamped: a layout regression must never be able to drive the pty to
    // absurd dimensions again (the 1088-row incident).
    const cols = Math.min(400, Math.max(20, Math.floor((width - padX) / cell.width)));
    const rows = Math.min(200, Math.max(5, Math.floor((height - padY) / cell.height)));
    // What the box holds past the last whole cell, on each axis — the strip
    // `placeGrid` balances onto both edges. Never negative: a box narrower
    // than the clamp's floor owes nothing.
    const slack = (room, count, size) => Math.max(0, Math.round((room - count * size) * 100) / 100);
    return {
      cols,
      rows,
      slackX: slack(width - padX, cols, cell.width),
      slackY: slack(height - padY, rows, cell.height),
    };
  }

  /* Tell the program what the pointer did, if it asked to be told.
   *
   * Cells, not pixels: the child speaks in cells and this side is the only one
   * that knows the font metrics. The tracking level is checked HERE and not
   * only in the backend — a pointer crossing an idle terminal would otherwise
   * cost a message per frame of travel to be thrown away at the other end.
   *
   * The bytes are still the backend's to write. This decides whether there is
   * anything worth sending, never what it says. */
  let holding = 3;
  let pressOwnedByProgram = false;
  // Shift reserves a gesture for the terminal even while a program is tracking
  // the mouse — the xterm convention this window already keeps for the wheel
  // (`tuiOwnsWheel = !event.shiftKey && programHasMouse()`). Without it a
  // full-screen TUI like Claude Code or Codex owns every drag, so there is no
  // way to select its output to copy ("드래그 복사가 안돼", 2026-09-02, agent
  // pane). Held for the whole gesture: a shift released mid-drag must not hand
  // the rest of the drag back to the program.
  let selectingLocally = false;
  let rightPressOwnedByProgram = false;
  let middlePressTarget = null;
  let copySelectionTimer = 0;
  /* ---- a link gesture is this window's, not the child's ----
   *
   * While a program tracks the mouse, a press that lands ON a link is held
   * rather than sent: the reports queue here, and at the end of the gesture
   * they are dropped if the link road took it and forwarded — in order —
   * if it did not. That second half is what keeps a drag that merely STARTED
   * on a URL working inside a TUI, and it is Orca's own shape (deferral plus
   * `claimAction`, terminal-link-pty-mouse-suppression.ts); a press that is
   * simply swallowed would be a mouse-aware program that silently misses
   * gestures. `null` means nothing is being held, which is every gesture
   * that did not begin on a link. */
  let deferredMouse = null;
  let linkPressClaimed = false;

  function flushDeferredMouse() {
    const held = deferredMouse;
    const claimed = linkPressClaimed;
    deferredMouse = null;
    linkPressClaimed = false;
    if (held === null || claimed) return;
    for (const one of held) one();
  }

  function copyOwnedSelection(expectedKey, localSelect) {
    copySelectionTimer = 0;
    if (termPrefs?.copy_on_select !== true || (programHasMouse() && !localSelect)) return;
    if (owner.address()?.key !== expectedKey || !selection.stands()) return;
    void selectionText().then((text) => {
      if (text !== "") void clipboardText.write(text);
    });
  }

  function queueOwnedSelectionCopy(localSelect = false) {
    const expectedKey = owner.address()?.key;
    if (!expectedKey) return;
    clearTimeout(copySelectionTimer);
    copySelectionTimer = setTimeout(() => copyOwnedSelection(expectedKey, localSelect));
  }

  /* The grid row a visual row stands for while a collapsed fold has taken
   * body rows out of the flow: the hidden rows above the pointer are added
   * back, so the row the program is told is the row it numbers. With nothing
   * hidden the answer is the visual row itself. */
  function gridRowOf(visual) {
    let seen = 0;
    for (let index = 0; index < nodes.length; index += 1) {
      if (nodes[index]?.hidden) continue;
      if (seen === visual) return index;
      seen += 1;
    }
    // Past the painted rows: count the hidden ones above and carry on.
    return visual + nodes.filter((node) => node?.hidden).length;
  }

  function cellUnder(event) {
    const box = pre.getBoundingClientRect();
    const grid = gridSize();
    if (grid.rows === 0 || grid.cols === 0 || box.width === 0 || box.height === 0) return null;
    const col = Math.floor(((event.clientX - box.left) / box.width) * grid.cols);
    /* A share of the box, as before, unless a fold has hidden rows: then the
     * pre is shorter than the grid and a share of it names a row that drifts
     * further from the pointer the lower it sits, so the row is read off the
     * measured cell height and mapped back through the hidden rows. */
    const folded = nodes.some((node) => node?.hidden);
    const row = folded && cell.height > 0
      ? gridRowOf(Math.floor((event.clientY - box.top) / cell.height))
      : Math.floor(((event.clientY - box.top) / box.height) * grid.rows);
    if (row < 0 || col < 0 || row >= grid.rows || col >= grid.cols) return null;
    return { row, col };
  }

  function report(event, kind, button) {
    // Shift held the pointer for the terminal at press; the whole gesture is a
    // local selection, reported to no program (the wheel makes the same call
    // the other way, refusing to forward a shifted notch).
    if (selectingLocally) return false;
    const address = owner.address();
    if (address === null) return false;
    const held = mouseModes.get(address.key);
    if (!held || held.tracking === "off") return false;
    if (kind === "move") {
      if (held.tracking === "click") return false;
      // A hover with nothing held is only worth saying to the one level that
      // asked about hovers.
      if (button === 3 && held.tracking !== "motion") return false;
    }
    const cell = cellUnder(event);
    if (cell === null) return false;
    const payload = {
      row: cell.row,
      col: cell.col,
      button,
      kind,
      shift: event.shiftKey,
      alt: event.altKey,
      ctrl: event.ctrlKey,
    };
    const send = () => {
      if (address.kind === "term") {
        invoke("term_mouse", { term: address.term, event: payload }).catch(() => {});
      } else {
        invoke("mouse_input", { id: address.id, event: payload }).catch(() => {});
      }
    };
    // Undecided link gesture: hold this report until the end of the gesture
    // decides whose it was. Answered `true` either way — the child is not
    // getting it NOW, and the browser must not start a selection underneath.
    if (deferredMouse !== null) deferredMouse.push(send);
    else send();
    return true;
  }

  host.addEventListener("mousedown", (event) => {
    if (event.button > 2) return;
    holding = event.button;
    // A left press on a link starts an undecided gesture (see `deferredMouse`).
    // Left only: a link is opened with the primary button, and holding the
    // other two would keep a context menu or a paste from the program that
    // asked for them.
    flushDeferredMouse();
    if (event.button === 0 && event.target?.closest?.("[data-href]")) deferredMouse = [];
    // Ownership belongs to the gesture at its press, not to whichever mouse
    // mode the next frame happens to report. A TUI can turn tracking off while
    // handling this click; that must not give the same click to clipboard UI.
    // Shift makes this the terminal's own gesture, whatever the program asked
    // for — so a left drag selects text and a right press still opens the menu.
    selectingLocally = event.shiftKey && programHasMouse();
    pressOwnedByProgram = !event.shiftKey && programHasMouse();
    rightPressOwnedByProgram = event.button === 2 && pressOwnedByProgram;
    middlePressTarget = event.button === 1 && !pressOwnedByProgram && primarySelectionPasteEnabled()
      ? owner.address()
      : null;
    // Prevented only when it was actually sent: a program that is tracking the
    // mouse owns the drag, and a press it received must not also start a
    // selection under it — two things answering one gesture.
    if (report(event, "press", event.button)) event.preventDefault();
    // The terminal's own left press is a selection gesture — a click, a
    // shift+click, a double or a triple — whatever the program asked for or
    // did not. Nothing is prevented for it: the browser has no selection of
    // its own to start here (`user-select: none`), and a prevented press
    // would keep focus from following the click.
    if (event.button === 0 && !pressOwnedByProgram) pressSelection(event);
  });
  function movePointer(event) {
    host.classList.remove("is-pointer-hidden");
    report(event, "move", holding);
  }
  host.addEventListener("mousemove", movePointer);
  // On the window, not the host: a drag that leaves the screen by an edge is
  // still this screen's drag, and it is exactly the one that has to keep
  // arriving for the auto-scroll to run.
  window.addEventListener("mousemove", dragSelection);
  function finishPointerGesture(event) {
    if (event.button > 2) return;
    // The selection drag ends with the gesture. What stands is remembered
    // for the middle-click paste — from the backend, like every copy.
    if (selectDrag !== null) {
      endSelectDrag();
      if (selection.stands() && primarySelectionPasteEnabled()) {
        void selectionText().then((text) => rememberPrimarySelection(text));
      }
    }
    const was = holding;
    const wasOwnedByProgram = pressOwnedByProgram;
    const wasLocalSelect = selectingLocally;
    const middleTarget = middlePressTarget;
    holding = 3;
    pressOwnedByProgram = false;
    middlePressTarget = null;
    if (was === 3) {
      selectingLocally = false;
      flushDeferredMouse();
      return;
    }
    // The release belongs to the same gesture as its press: a shift selection
    // ends without a word to the program, so the flag drops only after the
    // report has been refused.
    report(event, "release", was);
    selectingLocally = false;
    // After the click, which is the only thing that can still claim this
    // gesture — the browser fires it once mouseup has been handled, and
    // Orca's own fallback flush is this same next-tick (`setTimeout(…, 0)`).
    if (deferredMouse !== null) setTimeout(flushDeferredMouse);
    // A shift selection over a tracking program is still a selection the person
    // can auto-copy: the program never saw the drag, so `copy_on_select` owes
    // the same clipboard write it would on a bare pane.
    if (was === 0 && !wasOwnedByProgram && (!programHasMouse() || wasLocalSelect)) {
      queueOwnedSelectionCopy(wasLocalSelect);
    }
    if (was === 1 && !wasOwnedByProgram && !programHasMouse() && middleTarget) {
      event.preventDefault();
      event.stopPropagation();
      void readPrimarySelectionText().then((text) => pasteTextAt(middleTarget, text));
    }
  }
  window.addEventListener("mouseup", finishPointerGesture);
  host.addEventListener("auxclick", (event) => {
    if (event.button !== 1 || programHasMouse() || !primarySelectionPasteEnabled()) return;
    event.preventDefault();
  });
  host.addEventListener("contextmenu", (event) => {
    // A keyboard context-menu request and a middle click are not this setting.
    if (event.button !== 2) return;
    const wasOwnedByProgram = rightPressOwnedByProgram;
    rightPressOwnedByProgram = false;
    // Only the optional CLIPBOARD gesture stays one-owner: a right press a
    // mouse-tracking program already received must not also paste. The MENU
    // is not a second owner — Orca opens its pane menu with no tracking
    // check at all (use-terminal-context-menu-trigger.ts, openContextMenu),
    // and swallowing it here left a claude pane, whose TUI keeps the mouse,
    // with no door to copy or split while a codex pane had one ("claude
    // 창에선 오른쪽 마우스 팝업이 안뜸 codex cli창에서는뜸"). Everything
    // that is not the paste gesture falls through to the document's menu
    // handler; Ctrl+right-click keeps the menu reachable even with the
    // paste gesture on — Orca's own escape hatch (`!event.ctrlKey`).
    if (termPrefs?.right_click_paste !== true || event.ctrlKey) return;
    if (wasOwnedByProgram || programHasMouse()) return;
    const address = owner.address();
    if (!address) return;
    event.preventDefault();
    event.stopPropagation();
    void pasteClipboardAt(address);
  });
  // Wheel travel still short of one row. A trackpad speaks in single pixels
  // across dozens of events, and a handler that sends "at least one line" per
  // event turns a slow two-finger drag into a sprint — the remainder is kept
  // here until it adds up to a row.
  let wheelHeld = 0;
  let wheelHeldRoad = null;
  host.addEventListener("wheel", (event) => {
    const address = owner.address();
    if (address === null) return;
    event.preventDefault();
    // Pixels to whole rows, ONCE, for both roads. A notch is a row, and it is
    // the same row whether it is spent on this side's history or handed to a
    // program that asked for the wheel.
    //
    // The tracking road used to skip this and post exactly one notch per DOM
    // event, which is the same gesture measured two different ways: one turn
    // of a mouse wheel moved this terminal's history six rows and a TUI
    // inside it one — so scrolling back through an agent's transcript took
    // six times the spinning it takes in Orca, whose xterm divides once and
    // spends the result either way (`_consumeWheelEvent`,
    // I18nProvider-4EBrmTGg.js). Negated below because wheel-up is negative
    // deltaY and means BACK.
    //
    // Sensitivity multiplies the PIXELS, above the accumulator, so it changes
    // how far a gesture goes without changing the fact that sub-row travel is
    // banked — scaling the whole rows afterwards instead would quantise every
    // notch to a multiple of five and make a trackpad unusable at speed.
    // Both numbers are Orca's own options and it holds them at their xterm
    // defaults: `scrollSensitivity: 1.15` and `fastScrollSensitivity: 5`,
    // the latter on Alt (terminal-shortcut-policy-BK9MulHK.js:3809, :1848).
    // Alt is a modifier no terminal has a use for otherwise, which is why
    // xterm chose it, and a 5000-row scrollback is five times as far to
    // travel without it.
    // History uses the person's normal sensitivity and Alt adds the configured
    // fast multiplier. A mouse-tracking TUI has its own factor: changing
    // history speed must not also change how many wheel reports the program
    // receives.
    const tuiOwnsWheel = !event.shiftKey && programHasMouse();
    const wheelRoad = tuiOwnsWheel ? "tui" : event.altKey ? "fast" : "normal";
    if (wheelRoad !== wheelHeldRoad) {
      wheelHeld = 0;
      wheelHeldRoad = wheelRoad;
    }
    const sensitivity = tuiOwnsWheel
      ? termPrefs.tui_scroll_sensitivity
      : termPrefs.sensitivity * (event.altKey ? termPrefs.fast_scroll_sensitivity : 1);
    const pixels = event.deltaMode === 1 ? event.deltaY * cell.height : event.deltaY;
    wheelHeld += pixels * sensitivity;
    const steps = Math.trunc(wheelHeld / cell.height);
    if (steps === 0) return;
    wheelHeld -= steps * cell.height;
    // The program's wheel, if it asked for one — 64 up, 65 down, xterm's own
    // numbering, so nothing is translated twice. Never with shift held: shift
    // is the terminal's own key, and xterm refuses to forward a shifted wheel
    // to a tracking program at all (`_consumeWheelEvent` answers 0 for
    // `shiftKey`). It is the only way to reach history while a full-screen
    // TUI owns the pointer, and without it this window had none.
    if (tuiOwnsWheel) {
      let sent = false;
      for (let notch = Math.abs(steps); notch > 0; notch -= 1) {
        if (!report(event, "press", steps < 0 ? 64 : 65)) break;
        sent = true;
      }
      if (sent) return;
    }
    // Nothing is tracking the mouse — or shift said this notch is ours — so
    // the wheel means what it means everywhere else: scroll. Every rule lives
    // on the other side: the grid's history clamping and content anchoring,
    // and the alternate screen's arrow-key fallback. Through the flight, so a
    // fling is one command a frame rather than one per event.
    scrollTo(-steps);
  }, { passive: false });

  /* ---- the selection, on this screen ----
   *
   * The model is `selection` (ui/shell-term-selection.js); this is the half
   * that knows the DOM and the pointer: which cell an event is on, which
   * columns of a painted row the model names, when to paint again, and the
   * drag with its auto-scroll. Word and line gestures live here too, off the
   * CELL MODEL rather than the DOM — a wide glyph is one character standing
   * in two columns, and a run boundary is a change of colour, neither of
   * which the text in a span records. Orca has both from xterm, and its word
   * rule is an OPTION rather than the browser's default (`wordSeparator`)
   * because a shell's idea of a word is not prose's. */

  /* Is a program holding the mouse? Then a plain gesture is its, not ours. */
  function programHasMouse() {
    const address = owner.address();
    if (address === null) return false;
    const held = mouseModes.get(address.key);
    return held !== undefined && held.tracking !== "off";
  }

  /* Which row a pointer event landed in, and which column of it. */
  function rowUnder(event) {
    const node = event.target?.closest?.(".term-row");
    if (!node) return null;
    const index = nodes.indexOf(node);
    if (index < 0) return null;
    const box = node.getBoundingClientRect();
    if (cell.width <= 0) return null;
    return { node, index, col: Math.floor((event.clientX - box.left) / cell.width) };
  }

  // Fold composition can skip a body or fill live slots from history. Empty
  // slots after a short frame continue past its final line; the backend
  // bounds such a selection at the end of the buffer when copying it.
  function absoluteLine(index) {
    if (!rowLines?.length) return viewTop + index;
    return rowLines[index] ?? rowLines[rowLines.length - 1] + index - rowLines.length + 1;
  }

  /* The columns of visual row `index` the selection covers, `[from, to)` in
   * cells, or null. The start snaps to a wide glyph's head when the boundary
   * fell on its second column — half a syllable is not a thing a person can
   * have meant, and the backend copies by the same rule (`text_between`). */
  function selectionOn(index) {
    const cells = rows[index];
    if (cells === undefined) return null;
    const held = selection.columnsOn(absoluteLine(index), cells.length);
    if (held === null) return null;
    if (held.from > 0 && cells[held.from]?.ch === CONTINUATION) {
      return { from: held.from - 1, to: held.to };
    }
    return held;
  }

  /* One number for "which columns", so a row's memory is one compare. */
  function selectionKey(columns) {
    return columns === null ? undefined : columns.from * 4096 + columns.to;
  }

  /* Paint the rows whose selection moved — and only those. Asked after every
   * change of the model: a drag moves one or two rows a mousemove, a clear
   * touches exactly the rows that were painted, and a screen with nothing
   * selected and nothing painted compares its rows and writes nothing. A
   * hidden view owes the rows to the repaint that shows it. */
  function repaintSelection() {
    const painting = !host.hidden && !paintPaused;
    for (let index = 0; index < nodes.length; index += 1) {
      if (selectionKey(selectionOn(index)) === paintedSelection[index]) continue;
      if (painting) paintRow(index);
      else behindRows.add(index);
    }
  }

  function clearSelection() {
    endSelectDrag();
    if (!selection.anchored()) return;
    selection.clear();
    repaintSelection();
  }

  /* The text the selection names, from the backend — which holds every line
   * of it, on screen or not (`term_lines`, `lane_lines`; the rules of a
   * copied line are `TerminalGrid::text_between`'s). Empty when nothing
   * stands, or the shell is gone. */
  async function selectionText() {
    const held = selection.range();
    const address = owner.address();
    if (held === null || address === null) return "";
    const ends = { from: held.start, to: held.end };
    try {
      const text = address.kind === "term"
        ? await invoke("term_lines", { term: address.term, ...ends })
        : await invoke("lane_lines", { id: address.id, ...ends });
      return typeof text === "string" ? text : "";
    } catch {
      return "";
    }
  }

  /* Where a pointer event lands on this screen, in the model's terms: the
   * visual row and its absolute line, the nearest cell boundary and the cell
   * itself, and whether the pointer is past the top or bottom edge. `box` is
   * the screen's rect, read ONCE at the press (`selectDrag.box`) and reused
   * for the whole drag — a mousemove per pixel must not be a layout per
   * pixel. The cell size is the renderer's own measurement, the same one the
   * caret is placed with. Rows a collapsed fold hides are stepped over the
   * way `cellUnder` does it. */
  function pointUnder(event, box) {
    const point = termPointOnGrid(
      event.clientX - box.left,
      event.clientY - box.top,
      cell.width,
      cell.height,
      screenCols,
      nodes.length,
    );
    if (point === null) return null;
    const folded = nodes.some((node) => node?.hidden);
    const row = Math.min(nodes.length - 1, folded ? gridRowOf(point.row) : point.row);
    return { ...point, row, line: absoluteLine(row) };
  }

  /* The span the model takes for a point in the gesture's mode: the point
   * itself, the word under it, or the whole row. */
  function spanAt(point, mode) {
    const at = { line: point.line, col: point.col };
    if (mode === "cell") return { from: at, to: at };
    const cells = rows[point.row] ?? [];
    if (mode === "line") {
      return {
        from: { line: point.line, col: 0 },
        to: { line: point.line, col: Math.max(cells.length, screenCols) },
      };
    }
    const word = termWordSpan(cells, point.cell, termPrefs?.word_separators ?? " ", CONTINUATION);
    if (word === null) return { from: at, to: at };
    return {
      from: { line: point.line, col: word.from },
      to: { line: point.line, col: word.to },
    };
  }

  const clicks = makeTermClickCounter();

  /* A left press the terminal owns. The click count decides the mode, and
   * shift+click extends what stands — xterm's incremental click — unless a
   * program holds the mouse, where shift is the key that RESERVES the gesture
   * for the terminal and a fresh selection is what it starts. A plain click
   * is a press whose span is a point: that is how a click clears. */
  function pressSelection(event) {
    endSelectDrag();
    const box = pre.getBoundingClientRect();
    const point = pointUnder(event, box);
    if (point === null) return;
    const count = clicks.count(event.clientX, event.clientY, performance.now());
    const mode = count === 3 ? "line" : count === 2 ? "word" : "cell";
    if (event.shiftKey && !programHasMouse() && selection.anchored()) {
      selection.extend(spanAt(point, mode));
    } else {
      selection.begin(spanAt(point, mode));
    }
    selectDrag = { box, mode, edge: 0, distance: 0, timer: 0 };
    window.addEventListener("blur", endSelectDrag);
    document.addEventListener("visibilitychange", selectDragActive);
    repaintSelection();
  }

  function dragSelection(event) {
    if ((event.buttons & TERM_SELECT_PRIMARY_BUTTON_MASK) === 0) endSelectDrag();
    if (!selectDragActive()) return;
    const point = pointUnder(event, selectDrag.box);
    if (point === null) return;
    selection.extend(spanAt(point, selectDrag.mode));
    repaintSelection();
    // Past an edge: how far, for the speed, and which, for the direction.
    // The bottom edge is the last painted row's, not the box's — the screen
    // may stand shorter than the box that holds it.
    const gridBottom = selectDrag.box.top + nodes.length * cell.height;
    selectDrag.distance = point.edge < 0
      ? selectDrag.box.top - event.clientY
      : point.edge > 0 ? event.clientY - gridBottom : 0;
    if (point.edge === selectDrag.edge) return;
    selectDrag.edge = point.edge;
    clearTimeout(selectDrag.timer);
    selectDrag.timer = 0;
    if (point.edge !== 0) {
      dragScrollTick();
    }
  }

  /* One tick of the auto-scroll: the view moves by the table's rows and the
   * far end of the selection goes to the edge of the screen. The frame the
   * scroll lands on carries the new `viewTop`, and `apply` extends again —
   * which is what makes the selection GROW rather than wait for the next
   * tick. The alternate screen has no history, and `scrollTo` there is arrow
   * keys typed at the program, which is not what a drag past the edge means:
   * the selection still reaches the edge row, and stays there. */
  function dragScrollTick() {
    if (!selectDragActive() || selectDrag.edge === 0) return;
    const step = termSelectDragRows(selectDrag.distance);
    if (!altScreen && scrollbackLen > 0) scrollTo(selectDrag.edge < 0 ? step : -step);
    extendToEdge();
    repaintSelection();
    // Only a live edge drag schedules another tick. This is gesture work,
    // with the same lifetime as the held pointer, rather than an idle poller.
    selectDrag.timer = setTimeout(dragScrollTick, TERM_SELECT_DRAG_SCROLL.intervalMs);
  }

  function extendToEdge() {
    if (!selectDragActive() || selectDrag.edge === 0 || nodes.length === 0) return;
    const last = nodes.length - 1;
    const point = selectDrag.edge < 0
      ? { row: 0, line: absoluteLine(0), col: 0, cell: 0 }
      : { row: last, line: absoluteLine(last), col: screenCols, cell: Math.max(0, screenCols - 1) };
    selection.extend(spanAt(point, selectDrag.mode));
  }

  // Visibility can change without another pointer event. Check at both the
  // timer and frame boundaries so a late scroll response cannot grow a
  // cancelled gesture. These state reads do not force layout.
  function selectDragActive() {
    if (host.hidden || !host.isConnected || document.hidden) endSelectDrag();
    return selectDrag !== null;
  }

  function endSelectDrag() {
    if (selectDrag === null) return;
    clearTimeout(selectDrag.timer);
    window.removeEventListener("blur", endSelectDrag);
    document.removeEventListener("visibilitychange", selectDragActive);
    selectDrag = null;
  }

  /* ⌘+click opens what the pointer is on.
   *
   * ⌘ and not a plain click for a PATH, which is Orca's rule and the only one
   * that can work: everything on this screen is somebody's output, and a bare
   * click has to stay available for placing a selection.
   *
   * A program holding the mouse no longer takes this gesture away. It used to,
   * on the reasoning that a link inside a full-screen program cannot be
   * clicked — and the ordinary case disproves it: an agent TUI prints a
   * localhost URL and that URL is exactly what a person reaches for. Orca
   * keeps the link live and keeps the PRESS from the child instead; the press
   * road above holds those reports and drops them only when this handler
   * claims the gesture, so a drag that merely started on a URL still reaches
   * the program whole.
   *
   * A URL follows the one Browser Link Routing preference shared with
   * Markdown and the editor. A path always stays in this window — and when the
   * output named a line, goes to that line, which is the whole reason a person
   * clicks `src/foo.rs:42` rather than reading it. */
  host.addEventListener("click", (event) => {
    const link = event.target?.closest?.("[data-href]");
    // The node first — same answer, cheaper — then what the press remembered,
    // which survives a repaint that took the node away.
    const href = link?.dataset.href || pressedLinkHref();
    if (!href) return;
    // 실측 isTerminalOwnedLinkGesture: 직행도 액션도 아닌 클릭 — 맥
    // Ctrl+클릭(컨텍스트 메뉴의 것), Alt/Shift 단독, ⌘Alt — 은 터미널의
    // 것이 아니다. preventDefault 없이 통째로 시스템에 양보한다
    // (terminal-web-link-click.ts:30-32).
    if (!terminalOwnsLinkGesture(event)) return;
    // 이 링크가 말해진 기계 — SSH 판의 링크는 목적지 표가 시스템 문
    // 하나로 준다(발주서 link-source-owner.md).
    const sshSource = termLinkSpokenOverSsh(owner.address()?.term);
    if (!terminalLinkDirectActivation(event)) {
      // 수식키 없는 일반 클릭도 URL 위에서는 제스처다 — 두 목적지가 행으로
      // 서는 액션 팝오버. 단 문지기가 먼저다: 4px 넘게 긁었거나 눌렀을 때
      // 선택이 서 있던 손(그 클릭은 선택의 끝), blur 뒤의 손은 아니다.
      if (!canRequestTermLinkAction(host)) return;
      const url = canonicalHttpLink(href);
      if (url === null) {
        // 경로도 무수식 팝오버다 — 원본 handleTerminalFileLink가 URL과
        // 같은 문지기 뒤에서 requestTerminalLinkAction을 부른다(:55).
        // 팝오버 설정이 꺼져 있으면 URL처럼 아무 문도 열지 않는다.
        if (browserPrefs.terminal_link_action_popover === false) return;
        event.preventDefault();
        linkPressClaimed = true;
        openFileLinkMenu(href, event.clientX ?? 0, event.clientY ?? 0);
        return;
      }
      event.preventDefault();
      linkPressClaimed = true;
      routeHttpLink(href, event, { chooser: true, sshSource });
      return;
    }
    event.preventDefault();
    linkPressClaimed = true;
    // ⌘클릭은 표의 primary로 직행한다 — 설정이 정한 그 목적지.
    if (routeHttpLink(href, event, { table: true, sshSource })) return;
    const named = href.match(/^(.*?)(?::(\d+))?(?::(\d+))?$/);
    const path = named ? named[1] : href;
    const line = named?.[2] ? Number(named[2]) : undefined;
    // ⇧⌘의 경로는 시스템 기본 앱으로 — 원본 직행의 openWithSystemDefault가
    // shiftKey 그 자체다(terminal-file-link-actions.ts:32-38).
    if (event.shiftKey) {
      const absolute = path.startsWith("/") ? path : `${activeWorktreePath}/${path}`;
      void invoke("fs_open_default", { path: absolute }).catch((error) => showError(String(error)));
      return;
    }
    void openTermLink(path, line);
  });

  /* Repaint from the model, from the top.
   *
   * For a treatment change: the runs a lifted colour produced are literals and
   * have to be computed again (`forgetPalette`). Registered rather than
   * exported so every view that exists is reached, including the terminal
   * tabs' screens, which are made at runtime and would otherwise each need
   * remembering at their own call site. */
  paintedViews.add(refresh);

  function refresh() {
    behindAll = true;
    repaint();
  }

  function release() {
    paintedViews.delete(refresh);
    clearTimeout(copySelectionTimer);
    holding = 3;
    pressOwnedByProgram = false;
    rightPressOwnedByProgram = false;
    middlePressTarget = null;
    host.classList.remove("is-pointer-hidden");
    host.removeEventListener("mousemove", movePointer);
    window.removeEventListener("mousemove", dragSelection);
    window.removeEventListener("mouseup", finishPointerGesture);
    endSelectDrag();
    termSelectionViews.delete(view);
    terminalSelectionByHost.delete(host);
    // The track watch holds the rail, and the rail holds the host this view is
    // about to have taken out of the document. A screen given back has to give
    // its observer back too, or a closed shell keeps a live observation over a
    // node nobody can see.
    trackWatch.disconnect();
  }

  // `pre` rides out so a caller can read what was actually painted. Nothing in
  // this window copies it any more — the board's peek used to, and clipped for
  // it; it borrows the whole host now — but the screen's own text is how the
  // paint is checked from outside, and a renderer nobody can inspect is a
  // renderer nobody can test.
  const view = {
    measure,
    apply,
    applyFrames,
    repaint,
    setPaintPaused,
    relink,
    reset,
    gridSize,
    gridFor,
    refresh,
    release,
    host,
    pre,
    caret,
    // The measured cell, so a driver of this screen can aim at a column.
    cell,
    // The selection's doors: the model itself for a reader, and the three
    // questions the window asks by address key (see `terminalSelectionView`).
    selection,
    selectionStands: () => selection.stands(),
    selectionText,
    clearSelection,
    address: () => owner.address(),
    addressKey: () => owner.address()?.key ?? null,
    openFind: openTermFind,
    closeFind: closeTermFind,
    stepFind: (step) => void runTermFind(step),
    findOpen: () => !find.hidden,
    labelFind,
    paintRail,
  };
  termSelectionViews.add(view);
  terminalSelectionByHost.set(host, view);
  return view;
}

/* A path a terminal printed, opened where it was pointed.
 *
 * The path is handed over EXACTLY as the output wrote it. Resolving it here
 * would be a second copy of `resolve_in_project`, which is the backend's
 * function and is not only a join — it is also the guard that keeps a path
 * from leaving the checkout. A terminal is a place arbitrary text arrives, so
 * that guard is the whole reason a printed path is safe to click: `../../etc/
 * passwd` gets the same refusal here as it would from the tree.
 *
 * The line number is the half worth having. A compiler, a test runner and an
 * agent all print `src/foo.rs:42`, and landing on line 42 is why somebody
 * clicks it rather than reading it. */
async function openTermLink(path, line) {
  await openPath(path.replace(/^\.\//, ""), { preview: true, line });
}

/* Whether the modifier that opens a link is down, as a class on the body.
 *
 * A terminal is nothing but somebody's output, so links cannot advertise
 * themselves the way a document's do — a screen of permanently underlined
 * paths reads as a web page, and most of them are not being clicked. They
 * light up while ⌘ is held and are ordinary text the rest of the time, which
 * is Orca's rule and xterm's before it.
 *
 * On `blur` as well, because a window that loses focus mid-chord (⌘Tab is
 * exactly that chord) never sees the keyup and would keep every path in the
 * window underlined until the next time somebody pressed and released ⌘. */
function noteModifier(event) {
  document.body.classList.toggle("mod-held", hasPrimaryModifier(event));
}

// On `document`, deliberately. The window's own key router is the single
// keydown listener registered on `window` further down; a second one there
// would read — to a person and to the gate that pins that router — as a rival
// for the same job. This one decides nothing and swallows nothing: it records
// whether a modifier is down, and nothing else.
document.addEventListener("keydown", noteModifier);
document.addEventListener("keyup", noteModifier);
window.addEventListener("blur", () => document.body.classList.remove("mod-held"));

/* ---- focus reporting (`CSI ?1004`) ----
 *
 * The mode was parsed and stored from the day the grid was written and the
 * bytes were never sent — a field read by nothing, which is worse than no
 * support: the program's request is accepted and then never answered. An agent
 * TUI asks for this in its opening burst (1-b-2 counted it) and uses it to
 * stop drawing a caret it does not own.
 *
 * Whether to actually speak is the GRID's decision, next to the mode it is
 * about — this side only says which shell has the keyboard and when. The last
 * shell told is remembered so a repeated answer is not sent: a program that
 * hears "focused" twice with no "blurred" between has been told something
 * untrue about the window.
 */
let focusedTerm = null;

function tellFocus(term, focused) {
  if (term === null || term === undefined) return;
  // Nobody asked: say nothing. This is what keeps a tab switch free — the
  // budget tests hold the whole switch to zero round trips, and a message per
  // switch for a shell that never enabled the mode is precisely the kind of
  // cost that put them there.
  if (mouseModes.get(`term:${term}`)?.focus !== true) return;
  invoke("term_focus", { term, focused }).catch(() => {});
}

function noteTermFocus() {
  // Which shell has the keyboard: the active pane of the terminal tab in
  // front, or the floating shell when it is up and over everything.
  const tab = currentTab();
  let wanted = null;
  if (!termFloat.hidden) wanted = FLOAT_TERM;
  else if (tab?.kind === "term") wanted = activePaneOf(tab);
  if (!document.hasFocus()) wanted = null;
  if (wanted === focusedTerm) return;
  tellFocus(focusedTerm, false);
  focusedTerm = wanted;
  tellFocus(focusedTerm, true);
}

window.addEventListener("focus", noteTermFocus);
window.addEventListener("blur", noteTermFocus);

/* Copy, from the document: the terminal's selection when that is what stands.
 *
 * A terminal's selection is the grid model's, and ⌘C on a terminal never
 * arrives here — the keydown road copies it through Tauri
 * (`terminalClipboardShortcut`). This is the browser's own copy (an Edit
 * menu, a `copy` event somebody dispatched), and it means the terminal only
 * when the document has no selection of its own: a doc tab, an input or the
 * sidebar with text selected is somebody else's copy and is left exactly
 * alone. The terminal the keyboard is talking to comes first, which is what a
 * copy means; failing that, any visible screen with a selection standing.
 *
 * The event is answered and the text arrives later — it is on the backend,
 * with the history the window never held — so it goes out through the one
 * clipboard adapter rather than the event's own `clipboardData`. The rules of
 * a copied line (the grid's padding left behind, a newline per row) are the
 * backend's, in one place (`TerminalGrid::text_between`). */
document.addEventListener("copy", (event) => {
  const own = window.getSelection();
  if (own !== null && !own.isCollapsed) return;
  const active = document.activeElement;
  if (
    (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) &&
    active !== keySink
  ) return;
  const view = terminalSelectionView(terminalTargetKey(keyboardTarget())) ??
    [...termSelectionViews].find((one) => !one.host.hidden && one.selectionStands()) ??
    null;
  if (view === null) return;
  event.preventDefault();
  void view.selectionText().then((text) => text !== "" && clipboardText.write(text));
});

const stageView = makeTermView(el("lane-terminal"), el("lane-term"), el("lane-caret"), {
  // The lane surface is one screen showing whichever lane is focused, so who
  // a click belongs to is a question with a different answer minute to minute.
  address: () =>
    focusedId === null ? null : { kind: "lane", id: focusedId, key: `lane:${focusedId}` },
});
const floatView = makeTermView(el("terminal"), el("term"), el("caret"), {
  // This view is built before the `FLOAT_TERM` declaration below. Its value is
  // the backend's fixed floating-shell id (0); the address closure can name
  // the constant lazily, while this eager seed key must use the value itself.
  term: 0,
  address: () => ({ kind: "term", term: FLOAT_TERM, key: `term:${FLOAT_TERM}` }),
});

/* ---- permission modal (F6) ---- */

/* A queue, not a stack: prompts are answered in arrival order and there is
 * exactly one modal on screen. Every answer goes through permission.respond —
 * no code path answers by itself (제품 원칙 3).
 *
 * Two agents stopping at once is the ordinary case, not the corner: five
 * lanes are five programs asking for their own permissions on their own
 * clocks. What the queue owes them is that the second question WAITS rather
 * than replacing the first, that answering the first delivers to the first,
 * and that a question whose session has gone leaves instead of standing in
 * front of everybody else's (`withdrawPermissions`). */
const permissionQueue = [];
let activePrompt = null;

/* Already on the road?
 *
 * The `prompt_id` is the identity, because it is what the server retires on
 * the answer. The backend allows one subscriber per session, but a lane
 * closed and re-attached hydrates again — and a second modal carrying an id
 * the first one is about to retire is a question that can only fail to be
 * answered. */
function alreadyRaised(frame) {
  const id = frame.prompt_id;
  if (id === undefined || id === null) return false;
  return (
    activePrompt?.frame?.prompt_id === id ||
    permissionQueue.some((queued) => queued.frame?.prompt_id === id)
  );
}

function raisePermission(session, frame) {
  if (alreadyRaised(frame)) return;
  permissionQueue.push({ session, frame });
  const entry = laneBySession(session);
  if (entry) {
    invoke("gate_lane", { id: entry.lane.id, gate: "awaiting_permission" }).catch(() => {});
  }
  // Behind a question already on screen: the count says so and nothing else
  // moves. A full repaint here would rebuild the buttons an answer in flight
  // was pressed on, which is how one answer becomes two.
  if (activePrompt) paintPermissionCount();
  else showNextPermission();
}

function showNextPermission() {
  activePrompt = permissionQueue.shift() ?? null;
  paintPermission();
}

/* How many questions are behind the one on screen.
 *
 * Its own function because it is written from two places — the repaint, and
 * a prompt arriving while the modal already stands — and because the second
 * of those must touch nothing else on the dialog. */
function paintPermissionCount() {
  const waiting = permissionQueue.length;
  const more = el("perm-more");
  more.hidden = waiting === 0;
  more.textContent =
    waiting === 0 ? "" : t("ask.more", "{{count}}개 더 대기 중", { count: waiting });
}

/* A session that ended takes its questions with it.
 *
 * The prompt id died with the session, so `respond_permission` can now only
 * fail: the modal would stand there refusing every press, in front of the
 * questions behind it that CAN still be answered. Nothing is hidden by this
 * — the lane is gated `blocked` on the same event and keeps saying so on its
 * row (제품 원칙 1). What leaves is the door that no longer opens. */
function withdrawPermissions(session) {
  for (let at = permissionQueue.length - 1; at >= 0; at -= 1) {
    if (permissionQueue[at].session === session) permissionQueue.splice(at, 1);
  }
  if (activePrompt?.session === session) showNextPermission();
  else if (activePrompt) paintPermissionCount();
}

/* Retire exactly one prompt after the pane wins the shared prompt race.
 *
 * A `prompt_resolved` frame means its id can no longer accept an IDE answer.
 * The session remains alive and may already be asking another question, so
 * withdrawing the whole session here would hide valid work. */
function withdrawPrompt(session, promptId) {
  if (promptId === undefined || promptId === null) return;
  const mine = (queued) =>
    queued.session === session && queued.frame?.prompt_id === promptId;
  for (let at = permissionQueue.length - 1; at >= 0; at -= 1) {
    if (mine(permissionQueue[at])) permissionQueue.splice(at, 1);
  }
  if (activePrompt && mine(activePrompt)) showNextPermission();
  else if (activePrompt) paintPermissionCount();

  const entry = laneBySession(session);
  const waiting =
    activePrompt?.session === session ||
    permissionQueue.some((queued) => queued.session === session);
  if (entry && !waiting) {
    invoke("gate_lane", { id: entry.lane.id, gate: "idle" }).catch(() => {});
  }
}

/* Draw whichever prompt is current, in the language now in force.
 *
 * Split from taking the next one off the queue so a language change can
 * redraw the modal without consuming anything. The `shift` belongs to
 * `showNextPermission` and to nowhere else: a second one here would answer a
 * permission by discarding it, which is the one failure this window must
 * never have (제품 원칙 3). */
function paintPermission() {
  if (!activePrompt) {
    // `closing` is a no-op on a surface that is already hidden, so the
    // repaint path a language change takes does not animate a modal nobody
    // opened.
    hideModal(permScrim, { animated: true });
    return;
  }
  const { session, frame } = activePrompt;
  const entry = laneBySession(session);
  el("perm-agent").textContent = `ZO · ${shortSession(session)}`;
  el("perm-agent").className = `perm-agent ${entry ? slotClass(entry.lane.id) : ""}`;
  el("perm-cmd").textContent = `${frame.tool_name ?? ""}`;
  el("perm-why").textContent = [frame.reasoning, frame.audit_hint]
    .filter(Boolean)
    .join(" — ");
  const actions = el("perm-actions");
  actions.replaceChildren();
  const choices =
    Array.isArray(frame.choices) && frame.choices.length > 0
      ? frame.choices
      : [
          { label: t("perm.allow", "허용"), decision: "allow_once" },
          { label: t("perm.deny", "거부"), decision: "deny" },
        ];
  for (const choice of choices) {
    const button = document.createElement("button");
    button.type = "button";
    const halting = choice.decision?.startsWith("deny");
    button.className = `btn${choice.decision === "allow_once" ? " btn--primary" : ""}${
      halting ? " btn--halt" : ""
    }`;
    button.textContent = choice.label ?? choice.decision;
    button.addEventListener("click", () => answerPermission(choice.decision));
    actions.appendChild(button);
  }
  paintPermissionCount();
  showModal(permScrim, { animated: true });
}

/* The modal advances ONLY on a delivered answer. A failed permission.respond
 * keeps the prompt on screen with the buttons re-enabled and the reason
 * shown — closing it would leave the server's prompt pending with the person
 * believing they answered (제품 원칙 1: 막힌 레인은 조용할 수 없다). */
async function answerPermission(decision) {
  if (!activePrompt) return;
  // WHICH question this press answers, held rather than re-read: the await
  // below is a hole in time, and a session ending inside it moves the modal
  // on. Everything after the await asks whether the screen is still showing
  // the question that was pressed.
  const answered = activePrompt;
  const { session, frame } = answered;
  const buttons = [...el("perm-actions").querySelectorAll("button")];
  for (const button of buttons) button.disabled = true;
  try {
    await invoke("respond_permission", { session, promptId: frame.prompt_id, decision });
  } catch (error) {
    if (activePrompt !== answered) return;
    el("perm-why").textContent = t("perm.failed", "응답이 서버에 닿지 못했습니다 — 다시 시도하세요. ({{error}})", { error });
    for (const button of buttons) button.disabled = false;
    return;
  }
  const entry = laneBySession(session);
  if (entry && !permissionQueue.some((queued) => queued.session === session)) {
    invoke("gate_lane", { id: entry.lane.id, gate: "idle" }).catch(() => {});
  }
  // Advance only if this is still the question on screen. A withdrawal that
  // landed during the await already advanced, and a second shift here would
  // take the NEXT agent's question off the queue without ever showing it —
  // an answer nobody was asked for, and a question nobody sees.
  if (activePrompt === answered) showNextPermission();
}

/* ---- threads ---- */

/* What a lane is called wherever it is listed: what the running program says
 * it is doing, else the session it is attached to. */
function laneTitle(lane) {
  return lane.title
    || (lane.session_id ? shortSession(lane.session_id) : t("sidebar.newSession", "새 세션"));
}

function buildRow(lane) {
  const row = document.createElement("div");
  row.className = "row";
  row.innerHTML =
    '<span class="tick"></span><span class="who"></span>' +
    '<span class="what"></span>' +
    '<span class="when"><button class="close">×</button></span>' +
    '<span class="how"></span>';
  actsAsButton(row, () => focusLane(lane.id));
  row.querySelector(".close").addEventListener("click", (event) => {
    event.stopPropagation();
    closeLane(lane.id);
  });
  // The row stays detached until `refreshWorktrees` hangs it from the
  // workspace it actually runs in. Orca has no global session ledger in the
  // sidebar (`WorktreeCard-D1AQ61B3.js:2060-2580`).
  return row;
}

function styleLane(entry) {
  const { lane, row } = entry;
  const stateClass = STATE_CLASS[lane.state] ?? "idle";
  const slot = slotClass(lane.id);
  const active = lane.id === focusedId;
  const title = laneTitle(lane);


  row.className = `row ${slot} is-${stateClass}${active ? " is-active" : ""}`;
  row.querySelector(".who").textContent = lane.agent;
  row.querySelector(".what").textContent = title;
  const age = relTime(lane.session_id);
  row.querySelector(".how").textContent =
    `${laneStateLabel(lane.state)}${age ? ` · ${age}` : ""}`;
  // Written on every repaint rather than once where the row is built: this is
  // what a language change reaches, and a title set in the builder keeps the
  // language the row happened to be created in.
  const closeChord = optionalShortcutLabel("tab.close");
  row.querySelector(".close").dataset.tip = closeChord === null
    ? t("session.close", "레인 닫기")
    : t("session.closeChord", "레인 닫기 ({{chord}})", { chord: closeChord });
}

function refreshChrome() {
  for (const entry of lanes.values()) styleLane(entry);
  if (lanes.has(focusedId)) stage.className = `stage ${slotClass(focusedId)}`;
  // The lane's tab carries what a lane header used to: which agent, which
  // session, and whether it is waiting for someone.
  renderTabs();

  const waiting = order.filter((id) => {
    const held = lanes.get(id);
    return held && ["awaiting_permission", "blocked"].includes(held.lane.state);
  });
  attention.hidden = waiting.length === 0;
  if (waiting.length > 0) {
    attention.className = `attention ${slotClass(waiting[0])}`;
    attentionCount.textContent = t("session.waiting", "{{count}}개 대기", { count: waiting.length });
  }
}

function upsertLane(lane) {
  let entry = lanes.get(lane.id);
  if (!entry) {
    // Stamped at INGESTION, on the state change — the same discipline the
    // hook loop applies to `PaneState.at`. Stamping at render would say
    // "방금" about everything forever; stamping only at open would let a
    // lane's card age while the lane kept talking.
    entry = { lane, row: buildRow(lane), changedAt: Date.now() };
    lanes.set(lane.id, entry);
    order.push(lane.id);
  } else {
    if (entry.lane.state !== lane.state) entry.changedAt = Date.now();
    entry.lane = lane;
  }
  refreshChrome();
}

function resizeStageLane() {
  if (!focusedId) return;
  // A guess is not a telling — the same refusal `resizeTermTab` makes, and
  // for the same reason: a host that lays nothing out (a settings page over
  // the stage, a lane focused before its surface is on screen) computes no
  // width, and `gridSize` answers the 96×24 default. Sent, that default is
  // the size the agent's TUI then draws itself at, and nothing moves it back
  // until the box happens to change again.
  const { rows, cols, blind } = stageView.gridSize();
  if (blind) return;
  invoke("resize_lane", { id: focusedId, rows, cols }).catch(() => {});
}

async function focusLane(id) {
  const owner = lanes.get(id)?.lane.worktree_id;
  if (owner && owner !== activeWorktreePath && !(await activateWorktree(owner))) return;
  setFocused(id);
}

function setFocused(id) {
  const changed = focusedId !== id;
  const entry = lanes.get(id);
  // The view's focus moves BEFORE the backend is asked, or the full frame
  // the backend emits on focus arrives while the listener still filters on
  // the old id and the stage sits empty until the TUI happens to repaint.
  focusedId = id;
  // The lane's tab is created once and reused: staging another lane changes
  // what this tab shows, it does not add one. Lanes are their own axis.
  openTab({
    id: LANE_TAB,
    kind: "lane",
    worktree: entry?.lane.worktree_id ?? activeWorktreePath,
  });
  if (changed) stageView.reset();
  // Re-focusing is idempotent on the backend and always answers with a full
  // frame — the one paint that makes the stage correct from nothing.
  invoke("focus_lane", { id })
    .then(() => resizeStageLane())
    .catch(showError);
  refreshChrome();
  if (termFloat.hidden) keySink.focus();
  if (entry?.lane.session_id) {
    invoke("session_info", { session: entry.lane.session_id })
      .then((info) => {
        el("sb-model").textContent = [info.model, info.permission_mode]
          .filter(Boolean)
          .join(" · ");
      })
      .catch(() => {});
  }
}

function removeLane(id) {
  const entry = lanes.get(id);
  if (!entry) return;
  entry.row.remove();
  lanes.delete(id);
  // 이 레인에게 묻던 질문도 함께 간다 — 닫힌 레인의 프롬프트는 답할 수 없고,
  // 그것이 모달을 붙들고 서 있으면 뒤에 선 다른 에이전트의 질문이 보이지 않는다.
  if (entry.lane.session_id) withdrawPermissions(entry.lane.session_id);
  // 이 레인의 TUI가 마우스를 어디까지 원했는지도 함께 간다 — 레인 하나가
  // 남기는 유일한 창 쪽 흔적이고, 프로젝트를 옮길 때마다 모든 레인이 이 문을
  // 지나므로 두고 가면 그 자국만 세션 내내 쌓인다.
  mouseModes.delete(`lane:${id}`);
  const at = order.indexOf(id);
  if (at >= 0) order.splice(at, 1);
  if (lanes.size === 0) {
    focusedId = null;
    stageView.reset();
    // No lane left for the tab to show, so the tab goes too.
    dropTab(LANE_TAB);
  }
  updateStage();
  refreshChrome();
}

/* ---- the stage is a tab host ----
 *
 * Orca's centre is a split tree and each leaf carries its own strip of tabs
 * (docs/reverse/orca-ui-inventory.md 1-e). This is that leaf, singular until
 * splitting lands. One tab is the staged lane's terminal — created once,
 * because the registry serialises exactly one focused lane and lanes are
 * their own axis — and the rest are files. */

const LANE_TAB = "lane";

/* The floating panel's shell keeps the id it always had; ⌘T takes the next
 * (`FLOAT_TERM`, crates/zerocode-shell/src/main.rs). Every terminal on screen
 * is its own process, so every frame and every keystroke is addressed. */
const FLOAT_TERM = 0;
let floatTermRunning = false;
let floatTermGeneration = 0;

const tabs = [];
let activeTabId = null;
const activeTabByWorktree = new Map();

/* The leaves of the stage. Orca's centre is a split tree and each leaf
 * carries its own tab bar (1-e); this is that tree — see the stage tree
 * block below for the nodes, and `renderStage` for how they reach the DOM. */
/* The group being looked at: where a new tab lands and which strip's active
 * tab `currentTab` means. */
let focusedPane = 0;

/* Each group's element and strip, made on first need and kept while its
 * views are worth keeping. Group 0 is the markup's own pane — it holds the
 * originals the other groups' views are cloned from, so its entry is never
 * deleted even when its leaf folds away. */
const groups = new Map([[0, { el: el("pane-0"), strip: el("tabstrip") }]]);

function groupOf(id) {
  let held = groups.get(id);
  if (held) return held;
  const box = document.createElement("div");
  box.className = "pane";
  const strip = document.createElement("div");
  strip.className = "tabstrip";
  strip.setAttribute("role", "tablist");
  strip.setAttribute("aria-label", t("tab.label", "탭"));
  box.appendChild(strip);
  held = { el: box, strip };
  groups.set(id, held);
  return held;
}

function paneTabs(group) {
  bringStrandedTabsHome();
  return tabs.filter(
    (tab) => tab.pane === group && tab.worktree === activeWorktreePath,
  );
}

/* A tab of this checkout whose group the tree does not hold is brought to
 * the first leaf, the moment anyone asks the stage for its tabs.
 *
 * Two roads made such a tab: a worker seated for a checkout that was not in
 * front took its group from the tree that was (`openTab` no longer does),
 * and a split folding while its group still held another checkout's tabs
 * left them pointing at a leaf that was gone (`collapseStageGroup` no longer
 * does). Closing both roads does not close the class — a tab is a record and
 * a tree is a record, and nothing else ties them — so the answer to "which
 * tabs are in this group" first makes every tab of this checkout answerable.
 * The sidebar lists what `tabs` holds; without this the strip and the stage
 * could disagree with it forever (live report 2026-08-30, "zerocode-cli의
 * 워커를 클릭해도 보이지 않음"). */
function bringStrandedTabsHome() {
  const live = stageGroups();
  const home = live[0] ?? 0;
  for (const tab of tabs) {
    if (tab.worktree !== activeWorktreePath || live.includes(tab.pane)) continue;
    tab.pane = home;
  }
}

/* Where a tab dropped at this point belongs, by Orca's own geometry:
 * `resolveDropZone` insets a leaf by 10% on every side, and inside that inset
 * a drop means "this leaf" rather than a split (`rename-file-*.js`). Outside
 * it the horizontal thirds decide first, and what is left over splits on the
 * halfway line — which is the whole function, both axes, as measured. */
function dropZone(rect, x, y) {
  const localX = x - rect.left;
  const localY = y - rect.top;
  const insetX = rect.width * 0.1;
  const insetY = rect.height * 0.1;
  if (
    localX > insetX &&
    localX < rect.width - insetX &&
    localY > insetY &&
    localY < rect.height - insetY
  ) {
    return "center";
  }
  if (localX < rect.width / 3) return "left";
  if (localX > (rect.width / 3) * 2) return "right";
  return localY < rect.height / 2 ? "up" : "down";
}

/* ---- the stage's own split tree ----
 *
 * Orca's editor centre, whole this time: a leaf names a tab GROUP and a
 * split carries a direction and a ratio — new splits are born even
 * (`buildSplitNode`, I18nProvider-4EBrmTGg.js:57391, `ratio: 0.5`). The zone
 * a tab is dropped in picks both the axis and the side: `left`/`right`
 * divide side by side, `up`/`down` stack, and `left`/`up` put the NEW group
 * first (:57706-57710). One tree per worktree, because the groups hold that
 * worktree's tabs.
 *
 * The same node shape as a terminal tab's pane tree above, deliberately —
 * one vocabulary for one idea — but its own functions: a stage leaf is a
 * GROUP that owns a strip of tabs, splits carry Orca's measured default
 * ratio eagerly, and removing a leaf must say who inherits focus. */

const SPLIT_DIRECTION = {
  left: "horizontal",
  right: "horizontal",
  up: "vertical",
  down: "vertical",
};

const SPLIT_POSITION = { left: "first", up: "first", right: "second", down: "second" };

/* Orca's clamp for a group divider (`MIN_RATIO`/`MAX_RATIO`,
 * Terminal-DK7UpPWQ.js:3710-3711). The terminal panes' 0.12 stays theirs —
 * two measured numbers, two controls. */
const STAGE_MIN_RATIO = 0.15;

/* Group 0 is the primordial leaf — the markup's own pane, holding the
 * document view templates. Every later group takes the next number. */
let nextGroupId = 1;

const stageTrees = new Map();

function stageLeaf(group) {
  return { type: "leaf", group };
}

function stageTree() {
  let held = stageTrees.get(activeWorktreePath);
  if (!held) {
    held = stageLeaf(0);
    stageTrees.set(activeWorktreePath, held);
  }
  return held;
}

function setStageTree(node) {
  stageTrees.set(activeWorktreePath, node);
  // A new split changes how much of the workbench the right inspector may
  // safely occupy. Recompute after this turn so startup can finish declaring
  // the panel-width machinery before the first tree is restored.
  queueMicrotask(refreshResponsivePanelWidths);
}

/* Every group in this worktree's tree, left to right and top to bottom. */
function stageGroups(node = stageTree(), out = []) {
  if (node.type === "leaf") {
    out.push(node.group);
    return out;
  }
  stageGroups(node.first, out);
  stageGroups(node.second, out);
  return out;
}

/* The target leaf becomes a split holding it and the new group, on the
 * measured side (`buildSplitNode` + `replaceLeaf`, :57391/:57403). */
function splitStageLeaf(node, target, group, direction, position) {
  if (node.type === "leaf") {
    if (node.group !== target) return node;
    const added = stageLeaf(group);
    return {
      type: "split",
      direction,
      ratio: 0.5,
      first: position === "first" ? added : node,
      second: position === "second" ? added : node,
    };
  }
  return {
    ...node,
    first: splitStageLeaf(node.first, target, group, direction, position),
    second: splitStageLeaf(node.second, target, group, direction, position),
  };
}

/* A split that loses a child is the other child (`removeLeaf`, :57466). */
function dropStageLeaf(node, group) {
  if (node.type === "leaf") return node.group === group ? null : node;
  const first = dropStageLeaf(node.first, group);
  const second = dropStageLeaf(node.second, group);
  if (first === null) return second;
  if (second === null) return first;
  return { ...node, first, second };
}

/* Who inherits focus when a group folds — its layout sibling, the nearest
 * leaf of the sibling subtree (`findSiblingGroupId`, :57454). */
function stageSibling(node, group) {
  if (node.type === "leaf") return null;
  if (node.first.type === "leaf" && node.first.group === group) {
    return stageGroups(node.second)[0];
  }
  if (node.second.type === "leaf" && node.second.group === group) {
    return stageGroups(node.first)[0];
  }
  return stageSibling(node.first, group) ?? stageSibling(node.second, group);
}

/* The group already sitting on the side a split would create
 * (`getDirectLayoutSiblingOnSplitSide`/`findLayoutSiblingOnSplitSide`,
 * :57096/:57116) — what makes dropping a group's only tab right beside
 * itself a no-op instead of a shuffle. */
function stageSiblingOnSide(node, target, zone) {
  if (node.type === "leaf") return null;
  const { first, second, direction } = node;
  if (first.type === "leaf" && first.group === target && second.type === "leaf") {
    if (direction === "horizontal" && zone === "right") return second.group;
    if (direction === "vertical" && zone === "down") return second.group;
  }
  if (second.type === "leaf" && second.group === target && first.type === "leaf") {
    if (direction === "horizontal" && zone === "left") return first.group;
    if (direction === "vertical" && zone === "up") return first.group;
  }
  return stageSiblingOnSide(first, target, zone) ?? stageSiblingOnSide(second, target, zone);
}

/* Orca's whole no-op table (`isPaneColumnSplitDropNoOp`, :57126): a lone tab
 * dropped on its own group's edge, or where the split would land it back
 * beside its own group, changes nothing and so does nothing. */
function splitDropIsNoOp(sourceGroup, sourceCount, targetGroup, zone) {
  if (sourceGroup === targetGroup && sourceCount <= 1) return true;
  if (sourceCount !== 1) return false;
  return stageSiblingOnSide(stageTree(), targetGroup, zone) === sourceGroup;
}

/* The tree, onto the screen. Orca's `SplitNode` (Terminal-DK7UpPWQ.js:3794):
 * a split is a flex box in the node's direction, its children sized
 * `flex: ratio 1 0%` and `flex: (1-ratio) 1 0%`, with the resize handle
 * between them. Containers are rebuilt per render — they are three divs —
 * while the group elements persist and are MOVED, which is what keeps every
 * open view's scroll position across a re-split. */
let stageRootEl = null;

function stageNodeEl(node) {
  if (node.type === "leaf") return groupOf(node.group).el;
  const box = document.createElement("div");
  box.className = `stage-split is-${node.direction}`;
  const first = stageNodeEl(node.first);
  const second = stageNodeEl(node.second);
  const share = node.ratio ?? 0.5;
  first.style.flex = `${share} 1 0%`;
  second.style.flex = `${1 - share} 1 0%`;
  const grip = document.createElement("div");
  grip.className = "stage-grip";
  grip.tabIndex = 0;
  grip.setAttribute("role", "separator");
  grip.setAttribute(
    "aria-orientation",
    node.direction === "horizontal" ? "vertical" : "horizontal",
  );
  grip.setAttribute("aria-label", t("terminal.paneResize", "페인 크기 조절"));
  wireStageGrip(node, box, first, second, grip);
  box.append(first, grip, second);
  return box;
}

/* The tree the containers on screen were built from.
 *
 * Compared by IDENTITY, not by shape: every road that changes the tree —
 * `splitStageLeaf`, `dropStageLeaf`, moving to another worktree's tree —
 * hands `setStageTree` a NEW object, and the one road that changes a split
 * without changing the tree (`wireStageGrip`'s drag) mutates `node.ratio` in
 * place and writes the flex itself. So a tree that is the same object is a
 * tree whose containers are already right, down to the divider. */
let stageRenderedTree = null;

function renderStage() {
  // The pop-out has no stage — it reveals the board alone and never builds a
  // tab strip. It reached this function only through `renderTabs`, which
  // refuses for that surface; `updateStage` now calls it directly, so the
  // refusal has to be stated here rather than inherited from one caller.
  if (isPopout) return;
  const tree = stageTree();
  // Rebuilding the containers re-parents every group element, and a group
  // element holds a terminal's whole screen — thousands of nodes that the
  // browser must lay out again from nothing the moment they are detached.
  // This ran on EVERY render, so a split stage paid a full relayout of both
  // halves to switch a tab (measured: 7ms → 18ms to paint, one dropped
  // frame, on nothing but a click on the strip).
  if (stageRenderedTree === tree && stageRootEl !== null && stageRootEl.parentElement === stage) {
    return;
  }
  const built = stageNodeEl(tree);
  // A leaf that was lately inside a split still wears that split's share.
  if (built.classList.contains("pane")) built.style.flex = "";
  if (stageRootEl && stageRootEl.parentElement === stage && stageRootEl !== built) {
    stage.replaceChild(built, stageRootEl);
  } else if (built.parentElement !== stage) {
    stage.insertBefore(built, stage.firstChild);
  }
  stageRootEl = built;
  stageRenderedTree = tree;
}

/* Drag the divider between two groups — Orca's `ResizeHandle`
 * (Terminal-DK7UpPWQ.js:3712): the ratio is the pointer's place inside the
 * split's own box, clamped to the measured 0.15–0.85, applied live. Arrow
 * keys walk it too, the same reason the terminal grips answer them. */
function wireStageGrip(node, box, first, second, grip) {
  const horizontal = node.direction === "horizontal";
  const setRatio = (value) => {
    node.ratio = Math.min(1 - STAGE_MIN_RATIO, Math.max(STAGE_MIN_RATIO, value));
    first.style.flex = `${node.ratio} 1 0%`;
    second.style.flex = `${1 - node.ratio} 1 0%`;
    // Terminals inside either side read their room from the grid — a shell
    // painting for the old width shows columns that are not there.
    for (const [term, view] of termViews) {
      if (view.host.hidden || !box.contains(view.host)) continue;
      view.measure();
      resizeTermTab(term);
    }
  };
  grip.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    const moved = (moving) => {
      if (!grip.hasPointerCapture(event.pointerId)) return;
      const rect = box.getBoundingClientRect();
      setRatio(
        horizontal
          ? (moving.clientX - rect.left) / rect.width
          : (moving.clientY - rect.top) / rect.height,
      );
    };
    const ended = () => {
      grip.removeEventListener("pointermove", moved);
      grip.removeEventListener("pointerup", ended);
      grip.removeEventListener("pointercancel", ended);
      // Where the divider was parked is part of what the stage looks like.
      persistStageLayouts();
    };
    grip.addEventListener("pointermove", moved);
    grip.addEventListener("pointerup", ended);
    grip.addEventListener("pointercancel", ended);
  });
  grip.addEventListener("keydown", (event) => {
    const towardFirst = horizontal ? "ArrowLeft" : "ArrowUp";
    const towardSecond = horizontal ? "ArrowRight" : "ArrowDown";
    if (event.key !== towardFirst && event.key !== towardSecond) return;
    event.preventDefault();
    setRatio((node.ratio ?? 0.5) + (event.key === towardFirst ? -0.05 : 0.05));
  });
}

/* A group whose last tab left folds out of the tree, and its layout sibling
 * inherits the eye (`collapseGroupLayout`, I18nProvider:57490). The last
 * group standing never folds — it IS the stage. Group 0's entry stays in the
 * map either way; it holds the view templates the clones come from. */
function collapseStageGroup(group) {
  if (paneTabs(group).length > 0) return false;
  // Group 0 never LEAVES, even empty: its element IS the markup's #pane-0,
  // the chassis every document-view template lives in — detaching it kills
  // every later getElementById. But an empty chassis beside a split is not
  // an honest picture either: it stood as a dead half-screen (live report
  // 2026-08-14 #65, after a restart restored the split without the tab that
  // owned the chassis). So the chassis folds the OTHER way: its stage
  // sibling's tabs come home to pane 0, and the sibling — now empty — folds
  // through the ordinary door below.
  if (group === 0) {
    const tree = stageTree();
    if (stageGroups(tree).length < 2 || !stageGroups(tree).includes(0)) return false;
    const sibling = stageSibling(tree, 0);
    if (sibling === null || sibling === undefined) return false;
    for (const one of tabs) {
      if (one.pane === sibling) one.pane = 0;
    }
    // The looked-at order moves with them — where the eye lands on close
    // is that order's whole job.
    const carried = groupRecents.get(sibling) ?? [];
    if (carried.length > 0) {
      const held = (groupRecents.get(0) ?? []).filter((id) => !carried.includes(id));
      groupRecents.set(0, [...held, ...carried]);
      groupRecents.delete(sibling);
    }
    if (focusedPane === sibling) focusedPane = 0;
    return collapseStageGroup(sibling);
  }
  const tree = stageTree();
  if (stageGroups(tree).length < 2 || !stageGroups(tree).includes(group)) return false;
  const sibling = stageSibling(tree, group);
  setStageTree(dropStageLeaf(tree, group));
  if (focusedPane === group) focusedPane = sibling ?? stageGroups()[0];
  // "Empty" above was judged by this checkout's tabs, which is all the strip
  // shows — but a tab of ANOTHER checkout can sit in this group (seated while
  // this one was in front), and a leaf that folds with it inside strands it:
  // listed by the sidebar, drawn by nothing. It comes home the way the
  // chassis branch brings its sibling's tabs home.
  const rehomed = sibling ?? stageGroups()[0] ?? 0;
  for (const one of tabs) {
    if (one.pane === group) one.pane = rehomed;
  }
  const held = groups.get(group);
  if (held && group !== 0) {
    // Before the element goes: an EditorView removed from the document without
    // `destroy()` keeps its DOM listeners and its measure loop, which is the
    // leak `dropTermView` exists to prevent one surface over. The diff's merge
    // view holds two of those editors and a measure loop of its own.
    dropEditorView(group);
    dropDiffView(group);
    // The browser watcher holds the leaf's body STRONGLY — an unobserved
    // ResizeObserver target is how a folded leaf leaks its whole surface
    // (review finding 11). The tab reference goes with it.
    if (held.browserView) {
      const body = held.browserView.querySelector(".browser-body");
      if (body) browserBodyWatch.unobserve(body);
      held.browserView._browserTab = null;
    }
    if (held.emulatorView) {
      held.emulatorView._emulatorResizeObserver?.disconnect();
      held.emulatorView._emulatorResizeObserver = null;
      held.emulatorView._emulatorTab = null;
    }
    held.el.remove();
    groups.delete(group);
  }
  return true;
}

/* Give a screen the keyboard when it is clicked.
 *
 * Keystrokes are composed in the sink, because Korean needs a real field to
 * compose in — so a terminal screen that takes a click without passing focus
 * on is a terminal you cannot type into. One function rather than a listener
 * written at each site: the lane's screen and the floating one had theirs
 * from the start, the tab terminals are built at runtime and were missed,
 * and clicking the centre of the window then typing did nothing. */
/* 미러가 그룹의 앞 탭일 때, 창의 키 입력을 그 기기로 흘린다 — 화면을 찍어
 * 키보드를 미러로 가져온 다음의 타이핑이 여기로 온다. */
document.addEventListener("keydown", (event) => {
  const active = activeTabIn(focusedPane);
  if (active?.kind !== "emulator") return;
  const host = groupOf(active.pane)?.emulatorView;
  host?._emulatorKeydown?.(event);
});

function focusesTheKeyboard(host) {
  // Twice, and neither one prevents the default.
  //
  // Preventing the mousedown put focus on the sink but cancelled the
  // drag-selection the browser was about to start, so output could not be
  // copied. Only handling mouseup left a window that could not be typed into
  // whenever the selection check misread the click. So: take focus on the way
  // down, let the browser do whatever it does with the click, and take it
  // back on the way up unless the person actually selected something.
  host.addEventListener("mousedown", () => keySink.focus());
  host.addEventListener("mouseup", () => {
    // Text they just dragged out is text they mean to copy; focusing would
    // collapse it. Anything else — including a plain click — ends here.
    if (window.getSelection()?.toString()) return;
    keySink.focus();
  });
}

/* The terminal tabs' own screens, by shell id.
 *
 * A view per tab rather than one repainted from whichever is active: these
 * are separate processes running at the same time, and a single buffer would
 * make switching tabs redraw the wrong scrollback until the next frame
 * arrived. Orca's terminal tabs are separate processes for the same reason
 * (docs/reverse/orca-ui-inventory.md 1-e). */
const termViews = new Map();

/* What each shell's program says it is doing — the `OSC 0`/`OSC 2` word the
 * grid already parses and used to drop on the floor here. The tab strip
 * reads it so a tab says `vim grid.rs` or the agent's own status line rather
 * than `터미널 8`; a name the person typed on the pane still wins, because a
 * word somebody chose beats a word a program emits. Live shells carry it on
 * their frames, hidden shells get `term:title`, and a reveal seeds it from
 * the snapshot — three roads into one map. */
const termTitles = new Map();

function noteTermTitle(term, title) {
  const said = String(title ?? "").trim();
  if (!said || termTitles.get(term) === said) return;
  termTitles.set(term, said);
  // The strip and the card's session row (1-g68b) both wear this word, and
  // neither is drawn here. This is the FRAME path: an agent CLI renames its
  // terminal with its current step (OSC 0/2) while it streams, and drawing
  // the strip from inside the frame handler ran `renderTabs` — the running
  // count, the machine figures, the stage, every group's strip — once per
  // renaming frame, synchronously, on the thread that owes the next
  // keystroke (t-161, 전체 창 간헐 깜박임). The agent-paint beat draws both
  // surfaces once per frame and the last word wins, which is all a name
  // needs. Once both leaves already exist, a title-only frame can write those
  // two words in place. A missing leaf — including a shell with no agent row
  // yet — keeps this original full-paint road.
  if (!writeTermTitleLeaves(term)) scheduleAgentPaint(["tabs", "cards"]);
}

function termTabId(term) {
  return `term:${term}`;
}

/* A terminal tab's screen, made on first need and kept until the tab closes. */
function termView(term) {
  const held = termViews.get(term);
  if (held) return held;
  // The hour this shell started, for the row's clock — first paint, because it
  // is the earliest moment this window can honestly claim to know the shell.
  if (!paneBorn.has(term)) paneBorn.set(term, Date.now());
  const host = document.createElement("section");
  host.className = "terminal terminal--stage";
  host.setAttribute("aria-label", t("terminal.label", "터미널"));
  host.hidden = true;
  const pre = document.createElement("pre");
  pre.className = "term";
  const caret = document.createElement("div");
  caret.className = "term-caret";
  caret.hidden = true;
  host.append(pre, caret);
  stage.appendChild(host);
  focusesTheKeyboard(host);
  const view = makeTermView(host, pre, caret, {
    term,
    address: () => ({ kind: "term", term, key: `term:${term}` }),
  });
  termViews.set(term, view);
  return view;
}

/* ---- 어느 셸을 실제로 읽고 있는가 (1-ep) --------------------------------
 *
 * "에이전트 여럿이 돌 때 타이핑이 너무 심하게 밀린다"의 절반이 여기 있었다.
 * 펌프는 변화가 있는 **모든** 셸의 프레임을 표시 주기로 내보냈고, 이 창은
 * 그것을 전부 — 아무도 보고 있지 않은 탭의 것까지 — 다음 키 입력을 나를 바로
 * 그 스레드 위에서 역직렬화한 다음에야 `host.hidden`을 보고 버렸다. 값을 다
 * 치르고 나서 버린 것이다. 배경 에이전트 넷이면 초당 240장이 그 길을 지난다.
 *
 * 그래서 창이 먼저 말한다: 지금 읽고 있는 셸은 이것들이다. 늘렸다 줄였다 하는
 * 셈이 아니라 **매번 현재 집합 전체**를 말한다 — 셈은 한 번 어긋나면 영영
 * 어긋나고, 그 어긋남의 증상이 "보고 있는데 안 그려지는 터미널"이다. 멱등하니
 * 의심스러우면 그냥 한 번 더 부르면 된다. */

/* 한 셸의 화면. 떠 있는 패널만 자기 뷰를 따로 들고 있고, 나머지는 탭의 것이다. */
function viewOfTerm(term) {
  return term === FLOAT_TERM ? floatView : (termViews.get(term) ?? null);
}

/* 지금 이 창에 실제로 그려지고 있는 셸들.
 *
 * `host.hidden` 하나로 답한다. 무대의 탭도, 나뉜 판도, 보드의 미리보기도 전부
 * 그 플래그로 보이고 숨기 때문이다 — 미리보기는 판의 호스트를 **빌려** 가므로
 * 여기서 따로 셀 것이 없고, 그것이 빌리기를 택한 값이다. */
function readingTerms() {
  const here = new Set();
  for (const [term, view] of termViews) if (termShown(view)) here.add(term);
  if (!termFloat.hidden) here.add(FLOAT_TERM);
  return here;
}

/* 이 창이 마지막으로 선언한 집합, 그리고 그 선언들을 한 줄로 세우는 꼬리. */
let watchedTerms = new Set();
let watchTail = Promise.resolve(null);

/* ---- 프레임은 화면이 당겨 간다 --------------------------------------------
 *
 * docs/design/terminal-display-paced-frames-20260917.md. 백엔드는 예전에 화면이
 * 받을 준비가 됐는지 묻지 않고 16 ms마다 프레임을 밀어 보냈고, 이 창은 받는 즉시
 * DOM을 고쳤다. 한 프레임을 그리는 값이 그 박자보다 큰 기계에서는 그 차이가 줄이
 * 되고, 줄은 지연과 「지난 화면의 재생」이 됐다 — 사람이 한 번도 못 본 화면을
 * 차례로 그리는 일이다(M2: 스로틀 6배에서 적용 682번 중 159번이 그려지기 전에
 * 덮였다).
 *
 * 이제 창이 그릴 차례에 그때까지 쌓인 변경을 **한 번에** 당겨 간다(`term_pull`).
 * 그리드가 마지막으로 가져간 뒤의 변경을 한 장으로 접어 두므로, 늦게 가져가는
 * 것이 곧 합치기다. 박자는 셋으로 정해진다:
 *
 *   - 조용하던 창이 알림(`term:dirty:<라벨>`)을 받으면 **곧바로** 당긴다 — 키
 *     에코가 화면 프레임을 기다리지 않는다.
 *   - 당김이 프레임을 가져왔으면 다음 화면 프레임(rAF)에서 또 당긴다. 알림 없이
 *     화면의 박자로 — 흐르는 출력이 이 길이다.
 *   - 당김이 비었으면 식는다. 백엔드도 빈 당김을 보고 이 창을 조용한 것으로 적고,
 *     다음 변경에서 다시 알린다. 조용한 창은 IPC 0이다.
 *   - 흐르는 중에 사람이 키를 치면, 그 키의 에코는 **다음 화면 프레임을 기다리지
 *     않는다.** 백엔드의 펌프는 키를 쓴 뒤 그 답을 추격하고(`CHASE_INTERVAL`),
 *     셸이 답한 출력을 파싱한 라운드에서 흐르는 창에도 알린다(`Look::Chase`).
 *     이 창이 마지막으로 알림에 당긴 뒤 키가 pty로 갔으면(`typedSerial`), 그
 *     알림은 에코다 — 곧바로 당긴다.
 *     그렇지 않으면 흐르는 화면은 알림을 기다리지 않고 제 화면 프레임에 온다.
 *     키 없이 흐르는 출력에는 알림 당김이 없으니 화면 프레임 하나에 당김
 *     하나(I4)는 그대로다.
 *
 * 한 창에 진행 중인 당김은 하나뿐이다 — 그것이 배압이다. 가려진 창에서는 rAF가
 * 돌지 않으므로 당김이 멈추고, 백엔드의 몫은 한도에서 스냅샷 빚으로 접힌다. */
const termPull = {
  flying: false,
  // A notice, or a declaration waiting on its floor, arrived mid-flight.
  noticed: false,
  // The display frame a flowing screen's next pull waits for (0: none).
  frame: 0,
  // Whoever is waiting for the NEXT pull's answer (a declaration's floor).
  waiters: [],
  // The last pull failed after the backend may have taken frames for it: the
  // next one asks for every screen whole instead of trusting a model with a
  // hole in it.
  resync: false,
  // How many keys had gone to a pty (`typedSerial`) when this window last
  // pulled for a notice.
  answered: 0,
};
const TERM_PULL_DECODER = new TextDecoder();

/* Whether a key has gone to a pty since this window last pulled for a notice —
 * so a notice now is the pump telling it that key's answer is there. */
function termEchoOwed() {
  return typedSerial !== termPull.answered;
}

/* A pull a notice asked for. */
function pullForNotice() {
  termPull.answered = typedSerial;
  void pullTermFrames();
}

/* The backend says this window's terminals have frames. */
function noteTermFramesOwed() {
  if (termPull.flying) {
    termPull.noticed = true;
    return;
  }
  if (termPull.frame !== 0) {
    // A flowing screen is already coming on its next display frame — and the
    // echo of a key typed over it is not made to wait for that frame: the
    // answer it brings would paint a frame later still.
    if (!termEchoOwed()) return;
    cancelAnimationFrame(termPull.frame);
    termPull.frame = 0;
  }
  pullForNotice();
}

/* The answer of the next pull that starts after this call — for a declaration,
 * whose new shells are paid their floor by exactly that pull. It does not wait
 * for a display frame: a hidden window has none, and a declaration made there
 * must still settle. */
function awaitTermPull() {
  return new Promise((resolve) => {
    termPull.waiters.push(resolve);
    if (termPull.flying) {
      termPull.noticed = true;
      return;
    }
    if (termPull.frame !== 0) {
      cancelAnimationFrame(termPull.frame);
      termPull.frame = 0;
    }
    void pullTermFrames();
  });
}

async function pullTermFrames() {
  termPull.flying = true;
  termPull.noticed = false;
  const waiters = termPull.waiters.splice(0);
  const retrying = termPull.resync;
  let answer = null;
  try {
    const bytes = await invoke("term_pull", { resync: termPull.resync });
    answer = JSON.parse(TERM_PULL_DECODER.decode(bytes));
    termPull.resync = false;
  } catch {
    termPull.resync = true;
  }
  const failed = answer === null;
  const brought = !failed && applyPulledScreens(answer);
  termPull.flying = false;
  for (const resolve of waiters) resolve(answer);
  // A declaration that came while this pull was in the air waits for the
  // next answer, and it cannot wait for a display frame: a hidden window has
  // none, and every later declaration queues behind this one. Its waiters
  // leave with that answer whatever it is, so this cannot loop.
  if (termPull.waiters.length > 0) {
    void pullTermFrames();
    return;
  }
  // A key's echo told while this pull was in the air is pulled at once, for
  // the reason `noteTermFramesOwed` gives.
  if (termPull.noticed && termEchoOwed()) {
    pullForNotice();
    return;
  }
  // Frames came, so more are likely: the next display frame asks. A failure
  // asks there too, once — the screen may hold a hole only the resync fills,
  // and a still shell sends no notice that would bring it. The retry of a
  // failure does not retry itself: a backend that keeps refusing waits for
  // its next notice instead of becoming a loop.
  if (brought || (failed && (!retrying || termPull.noticed))) {
    termPull.frame = requestAnimationFrame(() => {
      termPull.frame = 0;
      void pullTermFrames();
    });
  } else if (termPull.noticed) {
    pullForNotice();
  }
}

/* One pull's screens, each painted once however many frames it brought. A
 * shell this window stopped reading while the pull was in the air is skipped:
 * its next reveal is owed a floor of its own. */
function applyPulledScreens(answer) {
  const screens = Array.isArray(answer?.screens) ? answer.screens : [];
  for (const { term, frames } of screens) {
    if (!watchedTerms.has(term) || !Array.isArray(frames) || frames.length === 0) continue;
    applyTermFrames(term, frames);
  }
  return screens.length > 0;
}

/* 읽고 있는 것을 백엔드에 말하고, 새로 읽기 시작한 화면에 바닥을 깐다.
 *
 * 선언은 백엔드에서 새로 든 셸마다 이 창을 「스냅샷을 빚진 독자」로 세우고, 그
 * 다음 당김이 그 빚을 갚는다 — 다른 창이 받은 마지막 델타 바로 뒤를 기준선으로
 * 하는 화면 전체(설계 규칙 3). 그래서 새로 읽기 시작한 셸의 첫 적용은 언제나 전체
 * 프레임이고(I7), 선언과 바닥 사이에 난 변경은 바닥 뒤의 델타로 이어진다.
 *
 * 새 셸이 있었으면 그 당김의 답(`{ screens, missing }`)을 돌려준다. 보드의
 * 미리보기가 그 답의 `missing`으로 "라이브 터미널 없음"을 말한다.
 *
 * 한 줄로 늘어세운다: 탭을 빠르게 넘기면 이 문이 한 왕복 안에 두 번 불릴 수
 * 있고, 두 선언이 순서를 바꿔 도착하면 백엔드는 지나간 집합을 들고 남는다 —
 * 그 값이 곧 보고 있는데 안 그려지는 터미널이다. */
function syncWatchedTerms() {
  // What the stage shows was just decided, and a placed worker's pane on it
  // is one a person may now be reading (t-6342) — a check of a map that is
  // empty on every switch but the few after a summons, and no IPC of its own.
  notePlacedWorkersSeen();
  watchTail = watchTail.then(declareWatchedTerms, declareWatchedTerms);
  return watchTail;
}

async function declareWatchedTerms() {
  const next = readingTerms();
  const fresh = [...next].filter((term) => !watchedTerms.has(term));
  // 같은 집합이면 할 말이 없다 — 새로 든 것이 없고 크기가 같으면 두 집합은
  // 같다. 무대 갱신은 아무것도 움직이지 않은 채로도 자주 불린다.
  if (fresh.length === 0 && next.size === watchedTerms.size) return null;
  watchedTerms = next;
  // The black box hears this door on the OTHER side: `set_watched_terms`
  // writes every arrived declaration down, which is the whole forensic
  // record of "보고 있는데 안 그려지는 터미널". No second log line from here —
  // the switch path's IPC budget is exactly six and every one is load-bearing.
  await invoke("set_watched_terms", { terms: [...next] }).catch(() => {});
  // 선언이 닿은 뒤의 당김이라야 새 셸의 바닥을 받는다.
  const floor = fresh.length > 0 ? await awaitTermPull() : null;
  // 전속력으로 올라간 화면은 미리보기 집합에서 내려와야 한다 — 한 화면이
  // 델타와 스냅샷을 번갈아 받으면 그 둘 사이에서 찢어진다. 반대도 같다:
  // 무대에서 내려간 화면은 그 카드가 서 있다면 다시 미리보기로 내려간다.
  void syncPreviewedTerms();
  return floor;
}

/* The notice comes under this window's own name: a window that never
 * declared a shell holds no listener that could be told about it, and Tauri
 * skips a webview with no listener for a name before it evaluates anything
 * (`dirty_event_for`, pane_runtime.rs). */
listen(`term:dirty:${WINDOW_LABEL}`, noteTermFramesOwed);

/* ---- 두 번째 티어: 보드가 한꺼번에 보여 주는 화면들 ----
 *
 * 위의 집합은 정의상 **무대가 드러낸 것**이고, 무대는 한 번에 한 탭의 판만
 * 드러낸다 — 그래서 에이전트가 네이든 여덟이든 그려지는 것은 언제나 하나였다.
 * 블랙박스가 그대로 적었다: `watch main: [3]` → `[32]` → `[33]` → `[2]`,
 * 그 사이마다 나머지 셀 셋의 프레임은 펌프에서 멈춰 섬다.
 *
 * 이쪽은 그 반대의 질문에 답한다: 지금 보드에 카드로 서 있는 셸은 전부.
 * 받는 것은 몇 줄짜리 꼬리를 느린 박자로(`term:preview`), 그래서 여덟 장을
 * 동시에 들고 있을 수 있다. 선언의 문법은 위와 글자 그대로 같다 — 집합 전체를
 * 매번, 꼬리 하나로 줄 세워서. */

/* 지금 보드가 그리고 있는 미니 화면들, 그 카드가 서 있는 동안만.
 *
 * 그려지는 것에서 유도한다 — `readingTerms`가 `termShown`에서 유도하는 것과
 * 같은 규율이다. 보드는 다시 그릴 때마다 카드 노드를 새로 짓으므로, 문서에서
 * 떨어진 노드는 이 집합에서도 떨어져야 한다. 그러지 않으면 카드 한 장을 열어
 * 본 적 있는 셀 전부가 영영 미리보기로 남는다. */
const previewScreens = new Map();

function previewingTerms() {
  const here = new Set();
  for (const [term, node] of previewScreens) {
    if (!node.isConnected) {
      previewScreens.delete(term);
      continue;
    }
    // 전속력으로 읽히는 화면은 이 티어를 받지 않는다. 백엔드도 같은 판정을
    // 한 번 더 하지만(두 창이 각자 선언하므로 그쪽이 최종이다), 여기서 먼저
    // 빼는 것은 왕복 하나를 아끼는 일이다.
    if (watchedTerms.has(term)) continue;
    here.add(term);
  }
  return here;
}

let previewedTerms = new Set();
let previewTail = Promise.resolve();

function syncPreviewedTerms() {
  previewTail = previewTail.then(declarePreviewedTerms, declarePreviewedTerms);
  return previewTail;
}

async function declarePreviewedTerms() {
  const next = previewingTerms();
  // 같은 집합이면 할 말이 없다 — `declareWatchedTerms`와 같은 비교다.
  const fresh = [...next].filter((term) => !previewedTerms.has(term));
  if (fresh.length === 0 && next.size === previewedTerms.size) return;
  previewedTerms = next;
  await invoke("set_previewed_terms", { terms: [...next] }).catch(() => {});
}

/* 각 셸이 마지막으로 보낸 꼬리 — 카드가 다시 지어질 때 곧바로 채울 수 있게.
 *
 * 보드는 훅 이벤트마다 카드 노드를 새로 짓는다. 모델이 없으면 그때마다 카드가
 * 비었다가 다음 박자(최대 한 박)에 다시 차오르고, 그 깜빡임이 곧 "안 보인다"의
 * 두 번째 판본이다. */
const previewRows = new Map();

/* 꼬리 한 줄의 글자.
 *
 * 백엔드가 보내는 것은 `grid.rs`의 packed row이므로 여기서 푸는 것은 `text`
 * 하나뿐이고, 연속 칸(앞 글자가 이미 두 칸을 먹은 그림자)은 그리는 쪽이
 * 건너뛴다 — 화면 렌더러의 규칙 그대로다. `zw`에 실린 나머지 스칼라는
 * 그 셀의 글자에 붙인다. 그래야 조합형 한글·악센트·이모지 ZWJ가 첫
 * 스칼라만 남아 네모나거나 의미가 바뀌지 않는다. 셀 철자도 읽는다:
 * 하네스의 픽스처와 옛 보낸이가 그 모양이다.
 *
 * **색은 오지 않는다.** 색을 정하는 기계는 뷰 안에 산다(`runsOf` — 굵은 잉크의
 * 밝은 승격, 화면이 정하는 대비 바). 그것을 카드에 복사하면 같은 답을 내는 곳이
 * 둘이 되고, 둘은 반드시 갈라진다. 카드가 답하는 질문은 "이 에이전트가 지금
 * 무엇을 하고 있나"이고 그 답은 낱말이다. */
function previewLineText(row) {
  // Use the same packed-row decoder as the full terminal view. Keeping one
  // decoder matters here: `text` is one code point per COLUMN, while `zw`
  // carries scalars that own no column, and the two forms must agree about
  // both. The card is low-rate, so paying for the small cell array buys one
  // source of truth instead of a second byte/column interpretation.
  const cells = row.cells ?? expandPackedRow(row);
  const said = cells.map(cellGlyph).join("");
  return said.trimEnd();
}

/* 한 카드의 미니 화면을 지금 아는 것으로 채운다.
 *
 * 문서에 붙었는지는 **묻지 않는다.** 카드는 자기 열에 들어가기 전에 조립되므로,
 * 짓는 자리에서 부를 때 그 노드는 아직 떠 있다 — 거기서 물러서면 다시 그릴
 * 때마다 카드가 빈 채로 태어나고 다음 박자에야 차오른다(측정: 보드는 훅
 * 이벤트마다 다시 그려진다). 서 있는지를 묻는 곳은 선언 쪽이다. */
function paintPreviewScreen(term) {
  const host = previewScreens.get(term);
  if (!host) return;
  const rows = previewRows.get(term) ?? [];
  const said = rows.map(previewLineText);
  // 꼬리의 빈 줄은 카드의 높이만 먹는다. 백엔드가 이미 화면 아래의 빈 바닥을
  // 잘라 보내므로 여기 남는 것은 출력 사이의 빈 줄뿐이고, 그건 그대로 둔다 —
  // 자르는 것은 끝의 것뿐이다.
  while (said.length > 0 && said.at(-1) === "") said.pop();
  host.textContent = said.join("\n");
  host.hidden = said.length === 0;
}

/* 백엔드가 이 셸의 꼬리를 보냈다.
 *
 * 당김 프레임(`term_pull`)의 델타 경로를 타지 않는다. 이것은 **스냅샷**이고 —
 * 델타는 가져가면서 비우므로 느린 박자로 델타를 보낼 방법은 없다 — 받는 쪽도
 * 누적하지 않고 통째로 갈아 끼운다. */
listen("term:preview", (event) => {
  const { term, rows } = event.payload ?? {};
  if (typeof term !== "number" || !Array.isArray(rows)) return;
  previewRows.set(term, rows);
  paintPreviewScreen(term);
});

/* Ask the board what is left, once, however many shells just went.
 *
 * A close is one shell, but the roads that close them close them in LOOPS:
 * switching projects, removing a checkout, sweeping the idle ones. Each pass
 * through `dropTermView` used to send `pane_agents` and `board_columns` on its
 * own, so a click that ended eight shells spent sixteen round trips deciding
 * a badge, and the first fifteen answers were about a machine that had already
 * changed.
 *
 * 한 박자를 따로 두지 않고 `scheduleAgentPaint`를 지난다. 그리는 것이 정확히
 * 같은 세 표면이고, 창에 코얼레서가 둘이면 한 이벤트가 두 박자에 나뉘어 같은
 * 카드를 두 번 그리게 된다. 닫기 루프는 여전히 동기이므로 무리는 여전히 한
 * 번만 묻는다 — 박자가 마이크로태스크에서 프레임으로 넓어졌을 뿐이고, 그
 * 사이에 도착한 소식까지 같은 그림에 담기는 쪽이 낫다.
 *
 * 카드 행은 이 문이 방금 비운 표에서 그려진다 — 이것이 없으면 끝난 셸의 행이
 * (그리고 그 헬퍼들의 행이) 다음에 누가 말할 때까지 서 있는다. */
function askBoardAfterClose() {
  scheduleAgentPaint(["cards", "badge", "board"]);
}

function dropTermView(term) {
  // Dropped whether or not there is a view: what an agent last reported belongs
  // to the shell, and a shell that ended has stopped reporting. Left behind, the
  // next terminal to take this id would inherit a stranger's badge.
  hookStates.delete(term);
  // The model it was on, when it last moved, and when it began — same owner,
  // same ending.
  paneModels.delete(term);
  panePermissionModes.delete(term);
  paneAsks.delete(term);
  hookStamps.delete(term);
  paneBorn.delete(term);
  // The words it was on, too — the next shell to wear this id would open on a
  // stranger's sentence.
  panePrompts.delete(term);
  paneSaid.delete(term);
  // And the conversation page it wore, if any — same owner, same ending.
  forgetPaneChat(term);
  // The conversation goes with it. Not because the id went stale — the vendor
  // still has it — but because this window's handle on it was the pane, and a
  // menu offering to reopen a conversation from a tab that is gone is a row
  // attached to nothing.
  paneSessions.delete(term);
  paneAutonomy.delete(term);
  // And which agent held it — the next shell to wear this id is a stranger.
  paneAgents.delete(term);
  restoredWorkers.delete(term);
  // And what was seen of it: the next terminal to take this id must not
  // inherit a stranger's acknowledgement.
  acknowledgedPanes.delete(`term:${term}`);
  // And the helpers that were running inside it. They lived in the agent this
  // shell held, so the shell ending is their ending too — and no stop event is
  // coming for them, because the process that would have sent one is the one
  // that just died. The backend drops the same entry in `close_term`; this is
  // the road a shell that exited on its own takes.
  paneSubagents.delete(term);
  // And what this shell and those helpers were doing. Keyed by card name, so
  // the shell's own card goes and every helper's card under it goes with it —
  // nothing will ever name those keys again, which is what makes them a leak.
  paneActivities.delete(`term:${term}`);
  for (const card of [...paneActivities.keys()]) {
    if (card.startsWith(`sub:${term}:`)) paneActivities.delete(card);
  }
  // 보드는 hook:agent에만 다시 그려지는데, 닫힘은 hook 이벤트가 아니다 —
  // 여기서 말하지 않으면 닫힌 판의 카드가 "작업중"인 채로 남는다. 모든 닫힘
  // 경로(탭 닫기·자연 종료·워크스페이스 삭제 일소)가 이 문을 지나므로,
  // 갱신은 여기 한 곳이면 전부다. **한 박자 뒤에** 묻는다: 프로젝트 전환과
  // 워크스페이스 일소는 이 문을 셸 수만큼 지나가고, 그때마다 두 커맨드를
  // 각각 보내면 여덟 셸을 닫는 클릭 하나가 왕복 열여섯 번이 된다. 마지막
  // 답만이 맞는 답이므로, 한 무리는 한 번만 묻는다.
  askBoardAfterClose();
  // A shell that has gone has nothing left to want: its bell must not outlive
  // it as a badge on a tab whose other pane is perfectly quiet.
  bellRang.delete(term);
  // 누가 이 판을 열었는지도 — **이 판 자신의 항목만**. 이 판이 시작한 판들은
  // 자기 항목을 그대로 들고 뿌리가 된다. 백엔드의 `close_term`이 같은 자리에서
  // 같은 이유로 같은 선택을 하고, 그 이유가 여기서도 그대로 옳다: 계보 규칙은
  // 이미 "없는 부모를 가리키는 판은 뿌리"라고 답하므로, 여기서 자식들을 훑어
  // 지우는 것은 같은 답을 두 군데에서 내는 일이고 그중 한 곳은 시험이 없다.
  // 새지도 않는다 — 항목은 전부 자기 term으로 키를 잡으므로, 자식의 선은
  // 자식이 끝날 때 이 줄이 가져간다.
  paneParents.delete(term);
  workerListTeams.delete(term);
  // 이 판이 어느 헬퍼였는지도(t-3024) — 같은 키, 같은 끝.
  paneHelpers.delete(term);
  agentClock.sync();
  // 떼어 둔 목록에서도. 끝난 셸은 다시 붙을 곳이 없고, 남겨 두면 카드가 문
  // 없는 행을 그린다(사용자 계약의 마지막 절: "종료되면 자동 에이전트 종료").
  detachedAgents.delete(term);
  // 자리 판정의 라벨을 기다리던 판이었다면 그것도 — 끝난 판은 옮길 곳이 없다.
  forgetPlacedWorker(term);
  // 그리고 이 셸의 카드 화면도. **여기지 `dropTermScreen`이 아니다** — 저쪽은
  // 미리보기가 빌려 간 화면을 돌려주는 길이기도 하고, 그때 그 에이전트는 여전히
  // 살아 있어서 카드도 서 있다. 거기서 지우면 들여다본 카드는 닫는 순간 두 번째
  // 티어로 돌아오지 못하고, 다음 보드 그리기까지 얼어붙는다.
  previewScreens.delete(term);
  previewRows.delete(term);
  previewedTerms.delete(term);
  dropTermScreen(term);
}

/* 화면만 버린다 — 셸에 대한 기억은 건드리지 않는다.
 *
 * `dropTermView`의 꼬리이자, 보드의 미리보기가 홀로 만들어 낸 뷰를 되돌리는
 * 문이기도 하다. 그 둘은 같은 일이 아니다: 셸이 끝나면 그 셸에 대해 아는 것이
 * 전부 함께 가야 하지만, 미리보기를 닫는 것은 화면 하나를 그만 그리는 일일
 * 뿐이고 — 그 에이전트는 여전히 살아 있다 — 거기서 훅 상태나 대화 번호까지
 * 지우면 팝아웃은 방금 들여다본 카드를 잊는다. */
function dropTermScreen(term) {
  // 화면에 딸린 사실들부터. 넷 다 그리기에 대한 것이지 에이전트에 대한 것이
  // 아니다 — 프로그램이 마우스를 어디까지 원했는지, 마지막으로 일러 준 격자가
  // 무엇인지, 바닥에 깔아 준 스냅샷, 그리고 프로그램이 스스로를 뭐라고
  // 불렀는지. 화면이 없어지면 넷 다 아무것도 가리키지 않는다.
  //
  // 뷰가 있든 없든 지운다: `term:title`은 화면이 만들어지기 전에도 오고,
  // 조기 반환 뒤에 두면 한 번도 드러난 적 없는 셸의 이름이 남는다.
  //
  // 이것들이 `dropTermView`가 아니라 여기 있는 이유는 보드의 미리보기다.
  // 미리보기는 탭이 없는 셸의 화면을 만들어 빌려 가고, 닫으면서 그 화면을
  // 돌려준다 — 그때 이 넷도 함께 가야 한다. 훅 상태·대화 번호·확인 시각은
  // 반대로 남아야 한다: 그 에이전트는 여전히 돌고 있다.
  mouseModes.delete(`term:${term}`);
  termGrids.delete(term);
  termTitles.delete(term);
  const view = termViews.get(term);
  if (!view) return;
  view.reset();
  // Off the treatment-change list as well, or a closed shell's screen keeps a
  // repaint closure alive over a host that has left the document.
  view.release();
  view.host.remove();
  // A composition can be live while its terminal dies (an agent killing the
  // pane mid-syllable). The preedit box rides the host, and a detached host
  // holding it would keep the overlay — and the moved key sink — forever.
  if (preeditBox.parentNode === view.host) clearPreedit();
  termViews.delete(term);
  // A per-pane zoom dies with its pane — a bounded map is the rule
  // (북극성: a keyed collection needs its deleting door).
  termFontOverrides.delete(term);
  // 그리고 더 이상 읽지 않는다고 말한다. 이 문은 화면을 버리지만 무대를 다시
  // 그리지는 않으므로(그 일은 부르는 쪽의 것이다), 선언을 여기서 하지 않으면
  // 죽은 셸의 번호가 감시 집합에 남는다. 기억은 **즉시** 지운다: 같은 번호로
  // 새 뷰가 곧바로 만들어질 수 있고(닫힌 판을 카드에서 다시 여는 길이 그렇다),
  // 그때 "집합이 그대로군"으로 읽히면 갓 태어난 빈 화면이 바닥을 못 받는다.
  watchedTerms.delete(term);
  void syncWatchedTerms();
  // 그리고 이 셸이 어느 티어에 속하는지도 바뀌었다. 미리보기가 빌려 간 화면을
  // 돌려주는 길이 여기이고, 그 판은 방금까지 전속력이었다 — 카드가 여전히 서
  // 있다면 이제 두 번째 티어로 내려온다. 등록 자체를 지우는 것은 셸이 끝났을
  // 때의 일이고, 그것은 `dropTermView`가 한다.
  void syncPreviewedTerms();
}

/* ---- the panes inside one terminal tab ----
 *
 * A terminal tab is not one shell. Orca stores a layout per tab id
 * (`terminalLayoutsByTabId`) and that layout is a tree of two node kinds:
 * `{type:"leaf", leafId}` and `{type:"split", direction, first, second,
 * ratio?}`, with the pane that was there as `first` and the new one as
 * `second`. `direction: "vertical"` divides left from right, `"horizontal"`
 * top from bottom; those are the two words its split commands pass
 * (file-preview-BNViqF3u.js:3480-3516).
 *
 * **`ratio` is absent on a new split, and absent means even.** The live split
 * gives both children plain `flex: 1 1 0%` and writes an explicit ratio only
 * when one is handed to it (`wrapInSplit`,
 * terminal-shortcut-policy-BK9MulHK.js:17580-17584), and the serializer drops
 * the key again whenever the two sides are within .005 of even
 * (`serializePaneTree`, store-BgJxB0hr.js:18007-18021). So a number here means
 * "somebody moved this divider" — which is exactly the thing worth keeping
 * when a layout is written to disk, and the reason not to write .5 eagerly.
 * (An earlier reading of this took `ratio: .5` from `addSplitLeafToLayout`
 * (App-BaqTRjaA.js:3533). That is a pane-tree writer, but it serves the
 * PROGRAMMATIC path — a hook or agent asking for a terminal that happens to
 * be a split (`onCreateTerminal`, :4144) — not ⌘D.)
 *
 * Our leaf id IS the shell id. A leaf owns exactly one pty, so a separate
 * identifier would only be a second name for the same thing — and every
 * frame, key and resize this window already routes is addressed by shell id.
 *
 * Sizes live on the node rather than in a table beside it: a tree that
 * carries its own proportions can be pruned without a second structure
 * going stale. */

const PANE_MIN_RATIO = 0.12;
/* A resize burst may move through many widths before its final pane mode;
 * wait one short frame-scale beat so only the settled answer is rendered. */
const SIDEBAR_REFLOW_DEBOUNCE_MS = 80;

function paneLeaf(term) {
  return { type: "leaf", term };
}

/* How much of a split the first side takes. Absent is even — see the note
 * above on why a new split carries no number at all. */
function paneRatio(node) {
  return node.ratio ?? 0.5;
}

/* Every shell in this layout, left to right and top to bottom — which is the
 * order `terminal.focusNextPane` walks. */
function paneLeaves(node, out = []) {
  if (!node) return out;
  if (node.type === "leaf") {
    out.push(node.term);
    return out;
  }
  paneLeaves(node.first, out);
  paneLeaves(node.second, out);
  return out;
}

/* Put `newTerm` beside `sourceTerm`, turning that leaf into a split. Orca
 * walks the tree for the source and only falls back to dividing the whole
 * root when it is not there (`addSplitLeafToLayout`) — the fallback matters,
 * because a split requested while the active pane is already gone must still
 * produce a pane rather than nothing. */
function splitPaneAt(node, sourceTerm, newTerm, direction) {
  if (node.type === "leaf") {
    if (node.term !== sourceTerm) return null;
    return {
      type: "split",
      direction,
      first: node,
      second: paneLeaf(newTerm),
    };
  }
  const first = splitPaneAt(node.first, sourceTerm, newTerm, direction);
  if (first) return { ...node, first };
  const second = splitPaneAt(node.second, sourceTerm, newTerm, direction);
  if (second) return { ...node, second };
  return null;
}

function addPane(layout, sourceTerm, newTerm, direction) {
  return (
    splitPaneAt(layout, sourceTerm, newTerm, direction) ?? {
      type: "split",
      direction,
      first: layout,
      second: paneLeaf(newTerm),
    }
  );
}

/* How much of its parent one node deserves when the sizes are evened out.
 *
 * Not one share each — Orca weights a child by how many leaves it holds IN
 * THE PARENT'S OWN DIRECTION (`getEqualizeWeight`, index-ftls8Hg_.js:36154).
 * A row of [A | (B over C)] evens to A:1, the column:1, because the column is
 * one thing as far as the row is concerned; but [A | (B | C)] — a row holding
 * a row — evens to 1:2, so A, B and C end up the same width. Splitting equally
 * at every node instead gives A half the window and B and C a quarter each,
 * which is not what "equalize" means to somebody looking at three columns. */
function paneWeight(node, direction) {
  if (node.type !== "split" || node.direction !== direction) return 1;
  return Math.max(
    1,
    paneWeight(node.first, direction) + paneWeight(node.second, direction),
  );
}

/* Even out every split in this layout, and say whether anything moved.
 *
 * Writes the ratio each node already carries rather than deleting it: absent
 * means even, but "even" for a lopsided tree is not 0.5, and clearing the
 * number would hand this node's children back the wrong proportion. */
function equalizePanes(node, geometry = null, gap = 0) {
  if (!node || node.type !== "split") return false;
  const first = paneWeight(node.first, node.direction);
  const second = paneWeight(node.second, node.direction);
  const axis = node.direction === "vertical" ? "width" : "height";
  const span = geometry?.[axis] || 0;
  const unit = span > gap * (first + second - 1)
    ? (span - gap * (first + second - 1)) / (first + second)
    : null;
  const firstSpan = unit === null ? null : first * unit + (first - 1) * gap;
  const share = firstSpan === null ? first / (first + second) : firstSpan / (span - gap);
  let moved = false;
  if (paneRatio(node) !== share) {
    node.ratio = share;
    moved = true;
  }
  // Both sides, always — `||` would stop walking the second the first moved.
  const inFirst = equalizePanes(node.first, firstSpan === null ? null : { ...geometry, [axis]: firstSpan }, gap);
  const inSecond = equalizePanes(node.second, firstSpan === null ? null : { ...geometry, [axis]: span - gap - firstSpan }, gap);
  return moved || inFirst || inSecond;
}

/* Take the leaves that are gone. A split that loses one child does not become
 * a split with a hole in it — it collapses to the child that is left
 * (`prunePaneLayout`, web-session-tabs-sync-DbcRbUBD.js:96-108). */
function prunePanes(node, keep) {
  if (!node) return null;
  if (node.type === "leaf") return keep.has(node.term) ? node : null;
  const first = prunePanes(node.first, keep);
  const second = prunePanes(node.second, keep);
  if (!first) return second;
  if (!second) return first;
  return { ...node, first, second };
}

/* The element a terminal tab draws into: one host per tab, holding the tree.
 *
 * Per tab rather than per shell, because it is the tab that `updateStage`
 * moves between leaves of the stage. The shells' own screens are moved into
 * this host's slots and keep their scrollback — the same reason `updateStage`
 * moves a surface rather than drawing a second one. */
const paneHosts = new Map();

function paneHost(tab) {
  const held = paneHosts.get(tab.id);
  if (held) return held;
  const host = document.createElement("div");
  host.className = "panes";
  host.hidden = true;
  stage.appendChild(host);
  paneHosts.set(tab.id, host);
  // 반응형: the choice between the split renderer and the wrapped grid is a
  // fact about the host's WIDTH, and the width moves without any pane road
  // running — a window drag, a side panel folding. Debounced the way the
  // float's observer is, and re-rendered only when the answer actually
  // flips, so a resize inside one mode costs nothing but the measure the
  // stage already pays.
  let reflowTimer = null;
  new ResizeObserver(() => {
    if (host.hidden) return;
    clearTimeout(reflowTimer);
    reflowTimer = setTimeout(() => {
      if (host.hidden || !tabs.includes(tab)) return;
      if (paneModeOf(tab, host) !== host.dataset.paneMode) {
        renderPanes(tab);
      } else {
        for (const term of paneLeaves(tab.layout)) {
          const view = termViews.get(term);
          if (!view || view.host.hidden) continue;
          view.measure();
          resizeTermTab(term);
        }
      }
    }, SIDEBAR_REFLOW_DEBOUNCE_MS);
  }).observe(host);
  return host;
}

/* Which renderer this tab's panes get, for the room the host has NOW.
 *
 * `grid` only when the row would overflow — a chain whose panes still fit
 * keeps the split renderer, its measured divider and its drag. A hidden or
 * unmeasured host answers `split`: the observer above re-asks the moment
 * the stage gives it real room. */
function paneModeOf(tab, host) {
  if (expandedPaneOf(tab) !== null) return "split";
  const chain = uniformVerticalChainLeaves(tab.layout);
  if (!chain || chain.length < 2) return "split";
  const width = host.clientWidth;
  if (width <= 0) return "split";
  return chain.length * PANE_MIN_GRID_WIDTH > width ? "grid" : "split";
}

function dropPaneHost(id) {
  paneHosts.get(id)?.remove();
  paneHosts.delete(id);
}

/* Which pane a key, a split or a close is about. Orca reads the pane the
 * pointer last put focus in (`manager.getActivePane()`), falling back to the
 * first — so a tab whose active pane has exited still answers. */
function activePaneOf(tab) {
  const leaves = paneLeaves(tab.layout);
  return leaves.includes(tab.activePane) ? tab.activePane : (leaves[0] ?? null);
}

function setActivePane(tab, term) {
  if (tab.activePane === term) return;
  tab.activePane = term;
  paintActivePane(tab);
  paintViewToggle();
  // The sidebar's focused-row fill follows the stage — same beat as a tab
  // switch, same guard.
  scheduleAgentPaint(["cards"]);
  // The active pane is part of what the file remembers — a restored tab whose
  // keyboard lands in a different pane than it left is a restore that moved
  // somebody's hands.
  persistPaneLayouts(tab.worktree);
}

/* Which slot wears the active mark. Repainted on its own rather than through
 * `renderPanes`: rebuilding the tree on every click would move the shells'
 * screens through the DOM, and a moved screen is a repainted one. */
function paintActivePane(tab) {
  const host = paneHosts.get(tab.id);
  if (!host) return;
  const active = activePaneOf(tab);
  for (const slot of host.querySelectorAll(".pane-slot")) {
    slot.classList.toggle("is-active", Number(slot.dataset.term) === active);
  }
  // The caret's outline and the pane's dimming both already say which pane has
  // the keyboard; a program that asked for `?1004` is owed the same news.
  noteTermFocus();
}

/* One pane's name: the title a person gave it, else the name it was BORN
 * with — Orca's per-leaf `paneTitle ?? tabTitle` (index-ftls8Hg_.js:20195).
 *
 * Both live on the TAB, keyed by shell id, rather than on the leaf: the tree
 * is rebuilt on every split and prune, and a name attached to a node would be
 * lost the moment its parent collapsed. Each map is absent until its first
 * entry, so a tab that was never renamed carries no object at all.
 *
 * Two maps, because they are two different facts. `paneTitles` is what a
 * person typed into the bar, and the bar's X takes it back. `paneNames` is the
 * launch's word for a shell seated in a division — ZO, Codex, an action's
 * name — the title the tab would have worn had the shell been a tab. Folding
 * the second into the first made the X on every agent pane born by the
 * strip's ＋ mean "제목 삭제" first and "창 닫기" only second ("cli끌때 2번
 * x를 눌러야 꺼짐 2개 켜지는거같음 — 새터미널 열때", 2026-09-06).
 *
 * Orca shows the bar only for a pane that HAS a title or is being renamed
 * (`syncSessionRestoredBannerTitleSpace`, :73742) — the header is not chrome
 * this window wears by default, it is the name appearing. */
function paneTitleOf(tab, term) {
  return tab.paneTitles?.[term] ?? tab.paneNames?.[term] ?? "";
}

/* Whether the name on the bar is a PERSON's — the only kind the X removes. */
function paneTitleGiven(tab, term) {
  return Boolean(tab.paneTitles?.[term]);
}

function setPaneTitle(tab, term, words) {
  const said = words.trim();
  if (!said) {
    clearPaneTitle(tab, term);
    return;
  }
  tab.paneTitles = { ...(tab.paneTitles ?? {}), [term]: said };
  renderPanes(tab);
  renderTabs();
}

function clearPaneTitle(tab, term) {
  if (!tab.paneTitles?.[term]) return;
  const rest = { ...tab.paneTitles };
  delete rest[term];
  // Back to absent when the last one goes, so "has this tab any titles" stays
  // one question rather than "an object with no keys" as well.
  tab.paneTitles = Object.keys(rest).length > 0 ? rest : undefined;
  renderPanes(tab);
  renderTabs();
}

/* The bar itself: 24px, 13px mono, the name ellipsised, clicking it renames.
 * Orca's measured `.pane-title-bar` (I18nProvider-DPbv01TU.css:21152) with its
 * own `--orca-pane-title-height: 24px`.
 *
 * And the bar is no longer only the name's. Orca's `TerminalPaneHeaderOverlay`
 * draws it for EVERY pane of the active terminal tab (`showAlwaysOnHeaders:
 * isActive && terminalContentVisible`) and ends it with an actions cluster —
 * continue-in-new-session on the active agent pane, split-right on every
 * pane, and an X that removes a title a person gave or, on a split tab,
 * closes the pane — a name the pane was BORN with is not a title, so the X
 * over it is the close. Untitled, the bar is CHROMELESS (`data-chromeless`):
 * no strip, no layout shift, only the icons floating in the pane's top-right
 * corner. */
function paneTitleBar(tab, term) {
  const bar = document.createElement("div");
  bar.className = "pane-title-bar";
  const title = paneTitleOf(tab, term);
  if (!title) bar.dataset.chromeless = "";
  if (title) {
    const said = document.createElement("button");
    said.className = "pane-title-text";
    said.type = "button";
    said.textContent = title;
    said.dataset.tip = title;
    said.addEventListener("click", (event) => {
      event.stopPropagation();
      startPaneRename(tab, term);
    });
    bar.appendChild(said);
  }
  bar.appendChild(paneTitleActions(tab, term, paneTitleGiven(tab, term)));
  return bar;
}

/* The cluster at the bar's end — Orca's `pane-title-actions ml-auto`
 * (`TerminalPaneHeaderOverlay`, OnboardingInlineCommandTerminal-D_M5zIRV.js
 * :2815-2900): ghost icon buttons, tooltips below, every click stopped so the
 * pane underneath is not also told. Each button acts on ITS pane and says so
 * by activating it first — the mousedown that landed here already pointed at
 * this pane, and a split that lands on a different pane than the icon sits in
 * is a button that lies. */
function paneTitleActions(tab, term, given) {
  const acts = document.createElement("span");
  acts.className = "pane-title-acts";
  const act = (glyph, said, run, className = "") => {
    const button = document.createElement("button");
    button.className = `pane-title-act${className ? ` ${className}` : ""}`;
    button.type = "button";
    button.innerHTML = icon(glyph);
    button.dataset.tip = said;
    button.setAttribute("aria-label", said);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      run();
    });
    acts.appendChild(button);
    return button;
  };
  // Continue in a new session — only where a conversation lives, and drawn
  // only on the active pane (Orca's `isActivePane` gate rides on CSS here,
  // through the same `.is-active` mark the caret already follows).
  if (paneHoldsAgent(tab, term)) {
    act("message-plus", t("continue.continueInNewSession", "새 세션에서 계속…"), () => {
      setActivePane(tab, term);
      void openContinueDialog(tab, term);
    }, "pane-title-continue");
  }
  act("split-v", t("terminal.splitRight", "터미널 오른쪽으로 분할"), () => {
    setActivePane(tab, term);
    void splitActivePane("vertical");
  });
  // `given` is a person's title (Orca's X takes that back); the name a pane
  // was born with is the tab's word for it, and over that the X is the close.
  if (given) {
    act("x", t("terminal.removeTitle", "제목 삭제"), () => clearPaneTitle(tab, term));
  } else if (paneLeaves(tab.layout).length > 1) {
    act("x", t("terminal.closePane", "창 닫기"), () => {
      setActivePane(tab, term);
      closeActivePane();
    });
  }
  return acts;
}

/* Rename in place. Orca edits in the bar rather than in a dialog, and its
 * three keys are Enter and Tab to commit, Escape to abandon, with a blur
 * committing too (`pane-title-input`, :38872-38899). */
function startPaneRename(tab, term) {
  const host = paneHosts.get(tab.id);
  const slot = host?.querySelector(`.pane-slot[data-term="${term}"]`);
  if (!slot) return;
  let bar = slot.querySelector(".pane-title-bar");
  if (!bar) {
    bar = paneTitleBar(tab, term);
    slot.prepend(bar);
  }
  // Editing takes the strip even on an untitled pane — a field cannot float
  // chromeless — and `finish` re-renders, which puts the overlay back.
  delete bar.dataset.chromeless;
  slot.classList.add("has-title");
  bar.replaceChildren();
  const field = document.createElement("input");
  field.className = "pane-title-input";
  field.type = "text";
  field.value = paneTitleOf(tab, term);
  field.placeholder = t("terminal.paneTitle", "페인 이름");
  field.setAttribute("aria-label", t("terminal.paneTitle", "페인 이름"));
  bar.appendChild(field);
  // Guarded, because commit-on-blur and commit-on-Enter both fire when Enter
  // moves focus off the field — and the second one would write over a title
  // the first already replaced.
  let done = false;
  const finish = (keep) => {
    if (done) return;
    done = true;
    if (keep) setPaneTitle(tab, term, field.value);
    else renderPanes(tab);
  };
  field.addEventListener("keydown", (event) => {
    // A Korean syllable is still being composed; Enter here ends the
    // composition, not the rename.
    if (event.isComposing) return;
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      finish(true);
    } else if (event.key === "Escape") {
      event.preventDefault();
      finish(false);
    }
  });
  field.addEventListener("blur", () => finish(true));
  field.focus();
  field.select();
  resizeTermTab(term);
}

/* One pane's slot, identically on both roads — the split tree and the
 * responsive grid below. One builder, because the slot carries behaviour
 * (the pointer's active mark, focus-follow, the title bar) and a second
 * copy is how the grid's panes would come to answer clicks differently. */
function paneSlotEl(tab, term) {
  const slot = document.createElement("div");
  slot.className = "pane-slot";
  slot.dataset.term = String(term);
  // The pointer decides which pane is active, the way Orca's does. On the
  // way down, so that the split or close a person reaches for straight
  // after the click is about the pane they just pointed at.
  slot.addEventListener("mousedown", () => setActivePane(tab, term));
  slot.addEventListener("pointerenter", () => {
    if (!termPrefs.focus_follows_mouse) return;
    setActivePane(tab, term);
    // Moving only the active mark leaves typing in whichever input had it.
    // Focus-follow means the pointed terminal owns the keyboard as well.
    keySink.focus();
  });
  // Every pane of a terminal tab wears the bar now — Orca's
  // `showAlwaysOnHeaders` — but only a TITLED one takes layout room;
  // untitled, the bar is a chromeless overlay and the grid keeps its rows.
  if (paneTitleOf(tab, term)) slot.classList.add("has-title");
  slot.appendChild(paneTitleBar(tab, term));
  slot.appendChild(termView(term).host);
  return slot;
}

/* The leaves of a layout that is ONE vertical row, or `null`.
 *
 * The responsive grid can only stand in for a shape it can reproduce: a
 * single-direction chain of even shares — which is exactly what every ⌘D
 * now leaves behind. A tree somebody built by hand (a horizontal cut, a
 * mixed nest) states an arrangement, and reflowing it would redraw their
 * intent, so those always keep the split renderer. */
function uniformVerticalChainLeaves(node, out = []) {
  if (!node) return null;
  if (node.type === "leaf") {
    out.push(node.term);
    return out;
  }
  if (node.direction !== "vertical") return null;
  if (!uniformVerticalChainLeaves(node.first, out)) return null;
  return uniformVerticalChainLeaves(node.second, out);
}

/* How narrow a pane may be drawn before the row stops being a row.
 *
 * 300px is about 36 mono columns at the default face — below that a shell
 * shows fragments of its own prompt. The number the reflow decision and the
 * grid's `minmax` share, so the mode that is chosen and the wrap the grid
 * then performs cannot disagree. */
const PANE_MIN_GRID_WIDTH = 300;

/* 반응형: the row wraps when the screen is small — four panes become 2×2
 * instead of four slivers ("작은 화면에서는 4분활등 화면에 맞게"). CSS's own
 * auto-fit does the arranging; this just lays the slots flat in pane order,
 * so ⌘] and the eye still walk the same list. The gaps wear the terminal
 * ground the divider gutter wears; the 3px bar itself belongs to the split
 * renderer and its grips (a recorded trade — a wrapped grid has no single
 * axis for a divider to own). */
function paneGridEl(tab, leaves) {
  const grid = document.createElement("div");
  grid.className = "panes-grid";
  grid.style.setProperty("--pane-min-width", `${PANE_MIN_GRID_WIDTH}px`);
  for (const term of leaves) grid.appendChild(paneSlotEl(tab, term));
  return grid;
}

function paneNodeEl(tab, node) {
  if (node.type === "leaf") return paneSlotEl(tab, node.term);
  const box = document.createElement("div");
  box.className = `pane-split is-${node.direction}`;
  const first = paneNodeEl(tab, node.first);
  const second = paneNodeEl(tab, node.second);
  // Expanded: everything that is not on the way to the chosen pane goes away,
  // and the surviving side takes the room. Orca hides siblings and gives the
  // ancestor `flex: 1 1 auto` (`applyExpandedLayoutTo`, :36237) — a hide
  // rather than a re-parent, so the shells keep their scrollback and come back
  // to the sizes they had.
  const grown = expandedPaneOf(tab);
  if (grown !== null) {
    const inFirst = paneLeaves(node.first).includes(grown);
    const inSecond = paneLeaves(node.second).includes(grown);
    if (inFirst || inSecond) {
      const kept = inFirst ? first : second;
      const gone = inFirst ? second : first;
      gone.hidden = true;
      grownSolo(kept);
      box.append(first, second);
      return box;
    }
  }
  // Grow proportionally from a zero basis rather than by percentage width:
  // the grip between them takes real space, and percentages of the container
  // would add up to more than the container once it does.
  const share = paneRatio(node);
  first.style.flex = `${share} 1 0`;
  second.style.flex = `${1 - share} 1 0`;
  const grip = document.createElement("div");
  grip.className = "pane-grip";
  grip.tabIndex = 0;
  grip.setAttribute("role", "separator");
  grip.setAttribute(
    "aria-orientation",
    node.direction === "vertical" ? "vertical" : "horizontal",
  );
  grip.setAttribute("aria-label", t("terminal.paneResize", "페인 크기 조절"));
  wirePaneGrip(tab, node, box, first, second, grip);
  box.append(first, grip, second);
  return box;
}

/* Drag the divider, and move it with the arrow keys.
 *
 * Both, because this is the one control on a terminal pane that has no other
 * way in — the same finding that put arrow keys on the side columns' grips.
 * The ratio is clamped so neither side can be dragged to nothing: a pane with
 * no columns left is a shell that cannot be read, and the drag that produced
 * it cannot be undone by aiming at a divider that is no longer there. */
function wirePaneGrip(tab, node, box, first, second, grip) {
  const vertical = node.direction === "vertical";

  const setRatio = (value) => {
    node.ratio = Math.min(1 - PANE_MIN_RATIO, Math.max(PANE_MIN_RATIO, value));
    first.style.flex = `${node.ratio} 1 0`;
    second.style.flex = `${1 - node.ratio} 1 0`;
    for (const term of paneLeaves(node)) {
      const view = termViews.get(term);
      if (!view || view.host.hidden) continue;
      view.measure();
      resizeTermTab(term);
    }
  };

  grip.addEventListener("mousedown", (event) => {
    event.preventDefault();
    const bounds = box.getBoundingClientRect();
    const span = vertical ? bounds.width : bounds.height;
    if (span <= 0) return;
    const move = (moved) => {
      const along = vertical ? moved.clientX - bounds.left : moved.clientY - bounds.top;
      setRatio(along / span);
    };
    const stop = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", stop);
      grip.classList.remove("is-dragging");
      document.body.classList.remove("is-dragging-pane");
      // Once, where the divider lands — not per mousemove, which would write
      // the file at pointer speed.
      persistPaneLayouts(tab.worktree);
    };
    // The strip wears the mark itself, so the CSS that reaches for the strong
    // colour under `:hover` keeps reaching for it all drag long — Orca's
    // divider takes `is-dragging` on pointerdown and sheds it when the drag
    // ends (`attachDividerDrag`,
    // src/renderer/src/lib/pane-manager/pane-divider-drag.ts:205/:156).
    grip.classList.add("is-dragging");
    document.body.classList.add("is-dragging-pane");
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", stop);
  });

  grip.addEventListener("keydown", (event) => {
    const back = vertical ? "ArrowLeft" : "ArrowUp";
    const on = vertical ? "ArrowRight" : "ArrowDown";
    if (event.key !== back && event.key !== on) return;
    event.preventDefault();
    setRatio(paneRatio(node) + (event.key === on ? 0.02 : -0.02));
    persistPaneLayouts(tab.worktree);
  });
}

/* The one surviving side of an expanded split. `1 1 auto` rather than a ratio:
 * it is the only child left visible, and a ratio would still be describing a
 * division that is not on screen. */
function grownSolo(node) {
  node.style.flex = "1 1 auto";
}

/* The pane filling this tab, or null.
 *
 * One reading rather than four: the mark is ABSENT on a tab that has never
 * expanded anything and NULL on one that collapsed, and every road that asks
 * has to treat those the same. Spelling that test out at each caller is how
 * one of them ends up asking `!= null` and another `!== null`. */
function expandedPaneOf(tab) {
  return tab?.expandedPane ?? null;
}

/* Can this tab expand a pane at all? Orca asks the same question — more than
 * one pane — and hides the row rather than disabling it (`canExpandPane:
 * contextMenu.paneCount > 1`, OnboardingInlineCommandTerminal-Caul4Dbw.js
 * @1232066). */
function canExpandPane(tab) {
  return paneLeaves(tab?.layout).length > 1;
}

/* Can it even the splits out right now? Orca asks TWO things here, not one:
 * `canEqualizePaneSizes: contextMenu.paneCount > 1 && expandedPaneId === null`
 * (same object, @1232066). The second half is what keeps the verb honest —
 * while one pane fills the tab the ratios are not on screen, so evening them
 * would rewrite, unseen, the sizes the person gets back when they collapse.
 * Orca refuses on the key road for the same reason (`if
 * (expandedPaneIdRef.current !== null) return`, @76900). */
function canEqualizePanes(tab) {
  return canExpandPane(tab) && expandedPaneOf(tab) === null;
}

/* Even out this tab's splits, and say whether the screen changed. */
function equalizeActivePanes(tab, { focus = true } = {}) {
  if (!canEqualizePanes(tab)) return false;
  const host = paneHosts.get(tab.id);
  const box = host?.firstElementChild?.getBoundingClientRect();
  const grip = host?.querySelector(".pane-grip");
  const gripBox = grip?.getBoundingClientRect();
  const gap = gripBox ? gripBox[grip.parentElement.classList.contains("is-vertical") ? "width" : "height"] : 0;
  if (!equalizePanes(tab.layout, box, gap)) return false;
  renderPanes(tab);
  if (focus) keySink.focus();
  return true;
}

// Coalesce a burst of spawns/exits; use the same equalize action as the menu.
const pendingPaneAlignment = new Set();
function schedulePaneAlignment(tab) {
  if (pendingPaneAlignment.has(tab.id)) return;
  pendingPaneAlignment.add(tab.id);
  requestAnimationFrame(() => {
    pendingPaneAlignment.delete(tab.id);
    if (!tabs.includes(tab)) return;
    equalizeActivePanes(tab, { focus: false });
    persistPaneLayouts(tab.worktree);
  });
}

/* Grow this pane over its siblings, or let them back.
 *
 * A flag on the tab and nothing else — the tree is not rewritten, which is
 * what lets a collapse hand back the exact ratios rather than a
 * reconstruction of them. Orca hides the siblings from a style snapshot and
 * replays it (`applyExpandedLayoutTo`/`restoreExpandedLayoutFrom`, @50650).
 *
 * The pane that grows takes the keyboard, the way Orca's does
 * (`manager.setActivePane(paneId, { focus: true })`, @53888). Without that, a
 * pane expanded from ANOTHER pane's menu leaves the active mark on a slot that
 * is now hidden, and the next keystroke lands in a shell nobody can see. */
function togglePaneExpanded(tab, term) {
  if (!canExpandPane(tab)) return;
  const growing = expandedPaneOf(tab) !== term;
  tab.expandedPane = growing ? term : null;
  if (growing) tab.activePane = term;
  renderPanes(tab);
  keySink.focus();
}

function renderPanes(tab) {
  const host = paneHost(tab);
  // An expanded pane whose shell has gone leaves the layout expanded around
  // nothing, so every other pane stays hidden. Orca clears the mark in the
  // same situation (`syncExpandedLayout`: the pane is not among the panes).
  const grown = expandedPaneOf(tab);
  if (grown !== null && !paneLeaves(tab.layout).includes(grown)) {
    tab.expandedPane = null;
  }
  // 반응형: a uniform row that would overflow the room wraps as a grid —
  // four panes become 2×2 on a small screen instead of four slivers. A row
  // that still fits keeps the split renderer, its measured divider and its
  // drag; so does every hand-built shape (`paneModeOf`).
  const mode = paneModeOf(tab, host);
  host.replaceChildren(
    mode === "grid"
      ? paneGridEl(tab, uniformVerticalChainLeaves(tab.layout))
      : paneNodeEl(tab, tab.layout),
  );
  paintActivePane(tab);
  // 반응형: which renderer stood is stamped on the host, so the observer can
  // tell a resize that flips the answer from one that does not.
  host.dataset.paneMode = mode;
  // Every visible shell measures again: what changed is how much room each of
  // them has, and a grid sized for the old room paints for columns that are
  // not there.
  for (const term of paneLeaves(tab.layout)) {
    const view = termViews.get(term);
    if (!view || view.host.hidden) continue;
    view.measure();
    resizeTermTab(term);
  }
  // Every road that changes the shape ends here — split, prune, title,
  // expand, equalize — so this is the one place the file learns about all of
  // them. Orca persists a snapshot at each of those verbs individually
  // (`persistLayoutSnapshot`); ours has a single door.
  persistPaneLayouts(tab.worktree);
}

/* ---- the layout, across the window closing ----
 *
 * Orca serialises each tab's pane tree into its session store and rebuilds it
 * on the way back up (`serializeTerminalLayout`/`replayTerminalLayout`,
 * I18nProvider-4EBrmTGg.js:46140, :46198). Ours goes to the backend raw — the
 * two rules about what a ratio stores (even splits store nothing, uneven ones
 * store three decimals) are Rust's (`pane_layout.rs`), so this file never
 * holds a second copy of them.
 *
 * Leaves are named by POSITION, not by shell id: the ids are pty numbers that
 * die with the process, so the stored tree says "the second pane was called
 * build" and the window maps that onto the fresh shells it spawns. Agent tabs
 * are not stored — a restored agent would be a plain shell wearing an agent's
 * name, and waking the agent itself is the resume path's job, not this one's. */

/* Suppresses the persist calls that restoring itself fires: every mount goes
 * through `renderPanes` and `openTab`, and a restore that saved each
 * half-built step would write the file it is still reading. */
let restoringPanes = false;

function paneNodeRecord(node) {
  if (node.type === "leaf") return { type: "leaf" };
  return {
    type: "split",
    direction: node.direction,
    first: paneNodeRecord(node.first),
    second: paneNodeRecord(node.second),
    ...(node.ratio !== undefined && { ratio: node.ratio }),
  };
}

function tabLayoutRecord(tab) {
  // A tab still asleep goes back exactly as it came. It has no shells to read
  // — that is what asleep MEANS — so anything built from it here would be a
  // record of an empty tab, and saving one would close, on disk, a tab that is
  // sitting on the strip waiting to be opened. The name the file gave it rides
  // along inside the record, which is how the backend keeps its stored screens
  // through a save that could not carry them.
  if (tab.asleep) return { ...tab.asleep, focused: tab.id === activeTabId };
  const leaves = paneLeaves(tab.layout);
  const titles = {};
  const names = {};
  const agents = {};
  const running = {};
  for (const [at, term] of leaves.entries()) {
    // A person's title and the launch's word travel apart — `paneTitleOf`'s
    // two halves — so a restart cannot promote the second into the first. A
    // pane with neither is remembered by the words that started it.
    if (paneTitleGiven(tab, term)) titles[at] = tab.paneTitles[term];
    const born = tab.paneNames?.[term] || panePrompts.get(term)?.trim();
    if (born) names[at] = born;
    // A conversation asleep in this leaf. Only the ones the BACKEND called
    // resumable — it owns the table of vendors that offer a way back, and a
    // stored wake that fails restores as an error where a shell should be.
    const held = paneSessions.get(term);
    if (held?.resumable && held.session) {
      agents[at] = {
        agent: held.agent,
        key: held.session.key,
        id: held.session.id,
        ...(held.session.transcript_path && { transcript_path: held.session.transcript_path }),
        // Mid-turn RIGHT NOW — in the same vocabulary the board reads. A pane at
        // `needs-attention` is waiting on the person, and a restart must
        // hand it back to them, not answer for them. The wake path reads
        // this to resume with a continue nudge instead of an empty composer.
        ...(isMidTurn(hookStates.get(term)) && { interrupted: true }),
      };
    }
    // And the PROGRAM that was in this leaf, conversation or not — a
    // different question, and the only one `zo` can answer: it wires no hook,
    // so it never reports a session and a pane running it was stored as a
    // bare shell. A stored set of bare shells is the shape that steps aside
    // for the default agent, which is how a workspace that was running zo
    // opened claude instead ("실행되고있던 에이전트가 zo였는데 claude가 열림").
    const launched = held?.agent ?? paneAgents.get(term);
    if (launched) running[at] = launched;
  }
  // The shells themselves, by position — Orca's `ptyIdsByLeafId`. What the
  // backend's exit capture follows to each leaf's grid; dead numbers to any
  // later session, and cleared by that capture on the way out.
  const terms = Object.fromEntries(leaves.map((term, at) => [at, term]));
  return {
    // The name the file gave this tab, kept from the record it woke out of —
    // a tab that was restored keeps its stored screens addressed by it.
    ...(tab.storedId != null && { id: tab.storedId }),
    root: paneNodeRecord(tab.layout),
    ...(Object.keys(titles).length > 0 && { titles }),
    ...(Object.keys(names).length > 0 && { names }),
    ...(Object.keys(agents).length > 0 && { agents }),
    ...(Object.keys(running).length > 0 && { running }),
    active: Math.max(0, leaves.indexOf(activePaneOf(tab))),
    // The grown pane travels too — Orca's `serializeTerminalLayout` carries an
    // `expandedLeafId` beside the tree and the active leaf
    // (store-BgJxB0hr.js@743641), so a window that closed with one pane filling
    // a tab opens on the same one pane.
    ...(leaves.includes(expandedPaneOf(tab)) && {
      expanded: leaves.indexOf(expandedPaneOf(tab)),
    }),
    ...(tab.pinned && { pinned: true }),
    // Which tab the eye was on — Orca's `activeTabId`, kept in the same
    // record as the set. It is what decides, on the way back, which single
    // tab of the set starts a shell.
    focused: tab.id === activeTabId,
    terms,
  };
}

/* The whole worktree's set, replacing what was stored — the same shape as the
 * file watch list, for the same reason: a set sent whole cannot drift from
 * what is actually open. An empty set is an instruction too; it is how a
 * closed tab stays closed on the next visit.
 *
 * An agent tab travels only when a conversation in it can be WOKEN — one of
 * its leaves holds a session the backend called resumable. Without that it
 * stays out: restoring it would put a plain shell on screen wearing an
 * agent's name. With it, the restore runs the agent's own resume command in
 * that leaf — Orca's cold restore, from its sleeping record
 * (index-ftls8Hg_.js:95637). */
function persistPaneLayouts(worktree) {
  if (restoringPanes || !worktree) return;
  const layouts = tabs
    .filter(
      (tab) =>
        tab.kind === "term" &&
        tab.worktree === worktree &&
        // Orchestration owns this terminal's lifetime in its ledger. Saving
        // it in the generic pane layout as well gives one shell two owners;
        // after a restart the layout resurrects the old worker and the ledger
        // opens its replacement beside it.
        !tab.ledgerManaged &&
        // SSH terminals belong to this window's visible workspace, but their
        // session executes on a remote host. Restoring one as an ordinary local
        // shell would silently change what the tab does after a restart.
        !tab.remoteHost &&
        // A tab still asleep is judged by the record it came from, which was
        // judged by this same rule when it was written. Asking the live
        // sessions about it would drop every restored agent tab the person
        // has not opened yet — the exact tabs this file exists to keep.
        (tab.asleep ||
          !tab.agent ||
          paneLeaves(tab.layout).some(
            (term) => paneSessions.get(term)?.resumable || paneAgents.has(term),
          )),
    )
    .map(tabLayoutRecord);
  invoke("save_pane_layouts", { worktree, layouts }).catch(() => {});
}

/* ---- the stage itself, across the window closing ----
 *
 * Orca persists its editor centre whole — tabs, groups, split tree, focus
 * (`unifiedTabs`/`tabGroups`/`tabGroupLayout`) — and validates on the way
 * back (`hydrateUnifiedFormat`, I18nProvider:57156). Ours goes to the
 * backend raw, and the rules live there (`stage_layout.rs`): which kinds
 * come back, how the tree folds around a group that kept nothing, where
 * focus falls. This side only says what the stage looks like NOW. */

/* The kinds worth writing down. The transient kinds — diff, imagediff —
 * are views of a moment and the backend refuses them anyway
 * (`isTransientEditorContentType`, :50810); not sending them keeps the
 * file's churn down to what can actually return. */
const STAGE_STORED_KINDS = new Set(
  ["file", "image", "mdview", "csv", "ipynb", "board", "changes", "vault", "knowledge", "skills", "artifacts", "jev"],
);

/* Suppresses the persist calls that restoring itself fires — the same
 * reason `restoringPanes` exists, for the same shape of loop. */
let restoringStage = false;

/* ---- which tab the eye was on last, per group ----
 *
 * Orca's `recentTabIds` (I18nProvider:50772-50806): every group keeps the
 * order its tabs were LOOKED AT, last is most recent, and that order — not
 * the strip's — decides where the eye lands when the active tab closes.
 * The list survives the restart with the stage (`StageGroup.recent`). */
const groupRecents = new Map();
let ctrlTabOrderMode = "mru";
let terminalShortcutPolicy = "orca-first";
let computerAwakeMode = "off";
let tabSwitcher = null;

/* Orca's `pushRecentTabId`: a repeat of the current holder changes nothing;
 * otherwise the tab moves to the end. */
function pushRecentTab(group, id) {
  const held = groupRecents.get(group) ?? [];
  if (held.at(-1) === id) return;
  const moved = held.filter((one) => one !== id);
  moved.push(id);
  groupRecents.set(group, moved);
}

function dropRecentTab(group, id) {
  const held = groupRecents.get(group);
  if (!held) return;
  const left = held.filter((one) => one !== id);
  if (left.length === 0) groupRecents.delete(group);
  else groupRecents.set(group, left);
}

/* Orca's `pickNextActiveTab` (:50798): the most recently seen sibling that
 * is not the closing tab; with no recency to go on, the neighbour — next
 * first, then previous (`pickNeighbor`, :50759). */
function pickNextActiveTab(siblings, group, closingId) {
  const order = siblings.map((held) => held.id);
  const recent = (groupRecents.get(group) ?? []).filter((id) => order.includes(id));
  for (let at = recent.length - 1; at >= 0; at -= 1) {
    if (recent[at] !== closingId) return recent[at];
  }
  const at = order.indexOf(closingId);
  if (at === -1) return order.at(-1) ?? null;
  return order[at + 1] ?? order[at - 1] ?? null;
}

function normalizeCtrlTabOrderMode(value) {
  return value === "sequential" ? "sequential" : "mru";
}

function normalizeTerminalShortcutPolicy(value) {
  return value === "terminal-first" ? "terminal-first" : "orca-first";
}

/* Orca's three words and no others (`normalizeComputerAwakeMode`) — an
 * unknown value is the answer of somebody who never chose, which is off. */
function normalizeComputerAwakeMode(value) {
  return value === "on" || value === "auto" ? value : "off";
}

function buildTabSwitcherModel() {
  const group = currentTab()?.pane ?? focusedPane;
  const strip = paneTabs(group);
  if (strip.length <= 1) return null;
  const active = activeTabIn(group);
  if (ctrlTabOrderMode === "sequential") {
    return { items: strip, activeIndex: strip.findIndex((tab) => tab.id === active?.id) };
  }

  const byId = new Map(strip.map((tab) => [tab.id, tab]));
  const items = [];
  const seen = new Set();
  const add = (tab) => {
    if (!tab || seen.has(tab.id)) return;
    seen.add(tab.id);
    items.push(tab);
  };
  const recent = groupRecents.get(group) ?? [];
  for (let at = recent.length - 1; at >= 0; at -= 1) add(byId.get(recent[at]));
  for (const tab of strip) add(tab);
  const activeIndex = items.findIndex((tab) => tab.id === active?.id);
  if (activeIndex > 0) items.unshift(items.splice(activeIndex, 1)[0]);
  return { items, activeIndex: active ? 0 : -1 };
}

function nextTabSwitcherIndex(count, current, direction) {
  if (count <= 0) return -1;
  if (current < 0) return direction > 0 ? 0 : count - 1;
  return (current + direction + count) % count;
}

function paintTabSwitcher() {
  const host = el("tab-switcher");
  const list = el("tab-switcher-list");
  const options = list;
  host.hidden = !tabSwitcher;
  options.replaceChildren();
  if (!tabSwitcher) {
    list.removeAttribute("aria-activedescendant");
    return;
  }
  tabSwitcher.items.forEach((tab, index) => {
    const option = document.createElement("div");
    option.className = "tab-switcher-option";
    option.id = `tab-switcher-option-${index}`;
    option.dataset.tab = tab.id;
    option.setAttribute("role", "option");
    option.setAttribute("aria-selected", String(index === tabSwitcher.selectedIndex));
    const label = document.createElement("span");
    label.className = "tab-switcher-label";
    label.textContent = tabLabel(tab);
    option.appendChild(label);
    if (isDirty(tab)) {
      const dirty = document.createElement("span");
      dirty.className = "tab-switcher-dirty";
      dirty.setAttribute("aria-hidden", "true");
      option.appendChild(dirty);
    }
    options.appendChild(option);
  });
  list.setAttribute("aria-activedescendant", `tab-switcher-option-${tabSwitcher.selectedIndex}`);
}

function tabSwitcherAvailable() {
  if (isPopout) return false;
  for (const id of ["settings-view", "task-view", "auto-view", "space-view", "qo-scrim"]) {
    if (!el(id).hidden) return false;
  }
  return document.querySelector(".scrim:not([hidden])") === null;
}

function openOrAdvanceTabSwitcher(direction) {
  if (!tabSwitcherAvailable()) return false;
  const model = buildTabSwitcherModel();
  if (!model) return false;
  const selectedId = tabSwitcher?.items[tabSwitcher.selectedIndex]?.id ?? null;
  const currentIndex = selectedId === null
    ? model.activeIndex
    : model.items.findIndex((tab) => tab.id === selectedId);
  tabSwitcher = {
    items: model.items,
    selectedIndex: nextTabSwitcherIndex(model.items.length, currentIndex, direction),
  };
  paintTabSwitcher();
  return true;
}

function cancelTabSwitcher() {
  if (!tabSwitcher) return;
  tabSwitcher = null;
  paintTabSwitcher();
}

function commitTabSwitcher() {
  const selected = tabSwitcher?.items[tabSwitcher.selectedIndex];
  tabSwitcher = null;
  paintTabSwitcher();
  if (selected && tabs.some((tab) => tab.id === selected.id)) setActiveTab(selected.id);
}

/* ---- where the eye was IN each document ----
 *
 * Orca's scroll and cursor caches (scroll-cache module: two LRU maps,
 * `CACHE_MAX_ENTRIES = 20`, restored when the editor mounts). Deliberately
 * NOT persisted — Orca keeps these for the run, not the session — so a
 * restart starts every file at its top, and only tab-switching within one
 * sitting comes back to the same line. */
const SCROLL_KEEP = 20;
const scrollCache = new Map();

function cacheScroll(key, held) {
  scrollCache.delete(key);
  scrollCache.set(key, held);
  if (scrollCache.size > SCROLL_KEEP) {
    scrollCache.delete(scrollCache.keys().next().value);
  }
}

function scrollKeyOf(tab) {
  return `${tab.worktree ?? ""}${tab.path ?? tab.id}`;
}

function stageNodeRecord(node) {
  if (node.type === "leaf") return { type: "leaf" };
  return {
    type: "split",
    direction: node.direction,
    first: stageNodeRecord(node.first),
    second: stageNodeRecord(node.second),
    ...(node.ratio !== undefined && { ratio: node.ratio }),
  };
}

const STAGE_DRAFT_SAVE_MS = 300;
const pendingStageLayouts = new Map();
const stageLayoutTimers = new Map();
let savingStageLayouts = false;

async function flushStageLayouts() {
  if (savingStageLayouts) return;
  savingStageLayouts = true;
  try {
    for (;;) {
      const next = [...pendingStageLayouts].find(([worktree]) => !stageLayoutTimers.has(worktree));
      if (!next) break;
      const [worktree, layout] = next;
      pendingStageLayouts.delete(worktree);
      try {
        await invoke("save_stage_layouts", { worktree, layout });
      } catch (error) {
        showError(error);
      }
    }
  } finally {
    savingStageLayouts = false;
  }
}

function persistStageLayouts({ deferred = false, worktree = activeWorktreePath } = {}) {
  if (restoringStage || restoringPanes || !worktree) return;
  const inFront = worktree === activeWorktreePath;
  const tree = inFront ? stageTree() : stageTrees.get(worktree);
  if (!tree) return;
  const held = stageGroups(tree);
  const active = tabs.find((tab) => tab.id === activeTabByWorktree.get(worktree));
  const groupsRecord = held.map((group) => {
    const owned = inFront ? paneTabs(group) : tabs.filter((tab) => tab.pane === group && tab.worktree === worktree);
    const docs = owned.filter((tab) => STAGE_STORED_KINDS.has(tab.kind));
    // The recency order, as indices into the tabs being written — only the
    // documents; a terminal in the order has no seat in this record.
    const recent = (groupRecents.get(group) ?? [])
      .map((id) => docs.findIndex((tab) => tab.id === id))
      .filter((at) => at >= 0);
    return {
      tabs: docs.map((tab) => ({
        kind: tab.kind,
        ...(tab.path && { path: tab.path }),
        ...(tab.preview && { preview: true }),
        ...(tab.pinned && { pinned: true }),
        // The unsaved typing rides the record the way Orca's
        // `dirtyDraftContent` does — written only while dirty, so a saved
        // file's record stays one line.
        ...((isDirty(tab) || (tab.kind === "file" && tab.gone)) && { draft: tab.draft }),
      })),
      active: Math.max(0, inFront ? docs.indexOf(activeTabIn(group)) : docs.includes(active) ? docs.indexOf(active) : recent.at(-1) ?? 0),
      ...(recent.length > 0 && { recent }),
    };
  });
  const layout = groupsRecord.some((group) => group.tabs.length > 0)
    ? {
        root: stageNodeRecord(tree),
        groups: groupsRecord,
        focused: Math.max(0, held.indexOf(inFront ? focusedPane : active?.pane)),
      }
    : null;
  pendingStageLayouts.set(worktree, layout);
  if (deferred) {
    // Keep the first deadline: continuous typing must still reach the disk.
    if (!stageLayoutTimers.has(worktree)) {
      stageLayoutTimers.set(worktree, window.setTimeout(() => {
        stageLayoutTimers.delete(worktree);
        void flushStageLayouts();
      }, STAGE_DRAFT_SAVE_MS));
    }
  } else {
    window.clearTimeout(stageLayoutTimers.get(worktree));
    stageLayoutTimers.delete(worktree);
    void flushStageLayouts();
  }
}

/* One stored tab back through the door its kind opens — the same doors a
 * person walks, so the read, the paint and the watch list all happen the
 * way they always do. */
async function reopenStageTab(tab) {
  const opts = tab.preview ? { preview: true } : {};
  if (tab.kind === "file") {
    return openFile(tab.path, { ...opts, allowMissing: typeof tab.draft === "string" });
  }
  if (tab.kind === "image") return openImage(tab.path, opts);
  if (tab.kind === "mdview") return openMarkdownPreview(tab.path, opts);
  if (tab.kind === "csv") return openCsv(tab.path, opts);
  if (tab.kind === "ipynb") return openIpynb(tab.path, opts);
  if (tab.kind === "board") return openBoard();
  if (tab.kind === "changes") {
    // 표면대로 다시 — 커밋된 비교 탭이 작업트리 변경으로 둔갑해 돌아오면
    // 저장된 것과 열린 것이 다른 문서다.
    if (tab.source === "committed") return openCommittedChanges();
    if (tab.source === "commit") return openCommitChanges(tab.commit, tab.subject ?? "");
    return openChangesArea(tab.area ?? null);
  }
  if (tab.kind === "vault") return openVault();
  if (tab.kind === "knowledge") return openKnowledgeGraph();
  if (tab.kind === "skills") return openSkillsView();
  if (tab.kind === "artifacts") return openArtifacts({ fresh: true });
  if (tab.kind === "jev") return openJevView();
}

/* The documents this worktree had when it was last looked at, back in their
 * groups, under the same tree. Answers whether anything mounted. */
async function restoreStageLayout(worktree) {
  let record;
  try {
    record = await invoke("stage_layouts", { worktree });
  } catch {
    return false;
  }
  if (!record || !Array.isArray(record.groups) || record.groups.length === 0) return false;
  restoringStage = true;
  try {
    // Fresh group ids for the stored ordinals — except the first, which is
    // the markup's own pane and every tree's natural first leaf.
    const ids = record.groups.map((unused, at) => {
      if (at === 0) return 0;
      const id = nextGroupId;
      nextGroupId += 1;
      return id;
    });
    stageTrees.set(worktree, fillStageLeaves(record.root, [...ids]));
    let focusTab = null;
    for (const [at, group] of record.groups.entries()) {
      for (const tab of group.tabs ?? []) {
        focusedPane = ids[at];
        await reopenStageTab(tab);
        // The door may have REUSED a tab that already existed — the board
        // is one tab, and a file can be open under another worktree's
        // group — and a reused tab keeps the group it had. The record says
        // where it lives now.
        const opened = tabs.find(
          (held) =>
            held.worktree === worktree &&
            held.kind === tab.kind &&
            (tab.path === undefined || held.path === tab.path),
        );
        if (opened) {
          opened.pane = ids[at];
          if (tab.pinned) opened.pinned = true;
          // The unsaved typing comes back INTO the field it left. Dirty
          // follows from the comparison — and if the disk caught up with
          // the draft while the window was closed, the same comparison
          // makes it clean again, which is the honest answer.
          if (typeof tab.draft === "string" && opened.kind === "file") {
            opened.draft = tab.draft;
            if (opened.gone || isDirty(opened)) opened.raw = true;
            opened.saved = false;
          }
        }
      }
      // The recency order, back onto the reopened tabs: the record's
      // indices name documents, and the door above just gave each one an
      // id. Restored per group so the first close after a restart already
      // knows where the eye had been.
      const docs = paneTabs(ids[at]);
      const recent = (group.recent ?? [])
        .map((index) => docs[index]?.id)
        .filter((id) => id !== undefined);
      if (recent.length > 0) groupRecents.set(ids[at], recent);
      // Which of the group's tabs had the eye, remembered for the group the
      // FOCUS comes back to — the single active-tab register is the
      // window's, so the other groups fall back to their last tab.
      if (at === record.focused) {
        focusTab = docs[group.active] ?? docs[docs.length - 1] ?? null;
      }
    }
    focusedPane = ids[record.focused] ?? ids[0];
    if (focusTab) setActiveTab(focusTab.id);
  } finally {
    restoringStage = false;
  }
  return true;
}

/* A stored tree with the minted group ids in its leaves, in walking order —
 * the stage twin of `fillLeaves`. */
function fillStageLeaves(node, ids) {
  if (!node || node.type !== "split") {
    const id = ids.shift();
    return id === undefined ? stageLeaf(0) : stageLeaf(id);
  }
  const first = fillStageLeaves(node.first, ids);
  const second = fillStageLeaves(node.second, ids);
  return {
    type: "split",
    direction: node.direction,
    ratio: node.ratio,
    first,
    second,
  };
}

/* The first terminal of a workspace that stored nothing, when the
 * orchestration ledger last seated an agent here. Answers whether the seat
 * was taken — launched, or refused out loud — so the caller knows not to
 * open a second thing in its place.
 *
 * A worker's tab is the ledger's to persist, never the pane layout's
 * (`persistPaneLayouts` skips `ledgerManaged`), so a checkout cut for a Codex
 * worker restores as an EMPTY workspace — and an empty workspace opened the
 * default agent, which is how a Codex worktree came back wearing Claude after
 * a restart ("원래는 codex가 작업중이였는데 재시작하면 claude로 바껴있는"). The
 * ledger still knows who was there, released or not; asked once, here, on the
 * one road that would otherwise guess. A checkout no worker was cut for
 * answers null — so does a runtime that is down — and the caller takes the
 * door it always took (`openTermTab`). */
async function openLedgerSeatedAgent() {
  // Restore needs current ownership, even between board snapshot publications.
  // The existing asynchronous restore command returns the seat and agent together.
  let seated = null;
  try {
    seated = await invoke("worktree_last_agent", { worktree: activeWorktreePath });
  } catch {
    seated = null;
  }
  if (!seated?.agent) return false;
  if (Number.isInteger(seated.term)) {
    if (!detachedAgents.has(seated.term)) {
      const tab = seatLedgerManagedTerm(seated.term, activeWorktreePath, seated.agent);
      if (tab) setActiveTab(tab.id);
    }
    return true;
  }
  if (seated.sleeping === true) {
    restoringWorkers.set(activeWorktreePath, seated.agent);
    paintWorktreeAgents();
    // Reserved for the ledger's worker restoration. Opening a generic agent
    // here gives this checkout two owners when its coordinator mounts later.
    return true;
  }
  if (agentRows.length === 0) await refreshAgents();
  const chosen = installedAgents().find((row) => row.id === seated.agent);
  if (!chosen) return false;
  try {
    const term = await launchAgentTab({ agent: chosen.id, prompt: "", ...spawnGrid() });
    mountTermTab(term, { agent: chosen.name }, {});
  } catch (error) {
    // Reported, and still the answer: a launch that failed is not a reason
    // to open something else in its place — the person has been told.
    showError(error);
  }
  return true;
}

function recordLeafCount(node) {
  if (!node || node.type !== "split") return 1;
  return recordLeafCount(node.first) + recordLeafCount(node.second);
}

/* A stored tree with fresh shells in its leaves, in walking order. `terms` can
 * run short — a spawn failed — and the tree collapses around the missing leaf
 * exactly as `prunePanes` collapses around a closed one. */
function fillLeaves(node, terms) {
  if (!node || node.type !== "split") {
    const term = terms.shift();
    return term === undefined ? null : paneLeaf(term);
  }
  const first = fillLeaves(node.first, terms);
  const second = fillLeaves(node.second, terms);
  if (!first) return second;
  if (!second) return first;
  return {
    type: "split",
    direction: node.direction === "horizontal" ? "horizontal" : "vertical",
    first,
    second,
    ...(typeof node.ratio === "number" && { ratio: node.ratio }),
  };
}

/* Calculate the expected pixel bounds of leaf `ordinal` in a split tree. */
function leafBoxInTree(node, ordinal, room, grip) {
  if (!node || node.type !== "split") return room;
  const firstLeaves = recordLeafCount(node.first);
  const vertical = node.direction !== "horizontal";
  const share = paneRatio(node);
  if (ordinal < firstLeaves) {
    const nextRoom = vertical
      ? { width: Math.max(0, (room.width - grip) * share), height: room.height }
      : { width: room.width, height: Math.max(0, (room.height - grip) * share) };
    return leafBoxInTree(node.first, ordinal, nextRoom, grip);
  }
  const nextRoom = vertical
    ? { width: Math.max(0, (room.width - grip) * (1 - share)), height: room.height }
    : { width: room.width, height: Math.max(0, (room.height - grip) * (1 - share)) };
  return leafBoxInTree(node.second, ordinal - firstLeaves, nextRoom, grip);
}

/* The expected room (width, height) of the stage when pane hosts are not yet laid out,
 * taking into account whether the left sidebar and right aside are folded or open. */
function stageExpectedRoom() {
  const workbench = el("workbench");
  const width = workbench?.getBoundingClientRect()?.width || window.innerWidth;
  const height = workbench?.getBoundingClientRect()?.height || Math.max(200, window.innerHeight - 80);
  const measuredSidebar = workbench?.querySelector(".threads")?.getBoundingClientRect()?.width ?? 0;
  const isSidebarFolded = !!(folded.sidebar || workbench?.classList?.contains("is-sidebar-folded"));
  const sidebar = isSidebarFolded ? 0 : (measuredSidebar > 0 ? measuredSidebar : panelWidths.sidebar);
  const isAsideFolded = !!(folded.aside || workbench?.classList?.contains("is-aside-folded"));
  const aside = isAsideFolded
    ? 0
    : (typeof renderedPanelWidth === "function" ? renderedPanelWidth("aside") : panelWidths.aside);
  return {
    width: Math.max(200, Math.floor(width - sidebar - aside)),
    height: Math.max(100, Math.floor(height)),
  };
}

/* Calculate the initial grid (rows, cols) a restored split leaf will have at birth. */
function storedLeafGrid(root, ordinal, tab) {
  const fallback = { ...DEFAULT_TERMINAL_GRID };
  if (isPopout) return fallback;
  const ruler = [...termViews.values()].find((view) => termShown(view) && !view.gridSize().blind);
  if (!ruler) return fallback;
  const grip =
    parseFloat(
      getComputedStyle(document.documentElement).getPropertyValue("--term-divider-hit-size"),
    ) || 0;
  const host = tab ? paneHosts.get(tab.id) : null;
  const box = host?.getBoundingClientRect() ??
    [...paneHosts.values()].find((one) => !one.hidden)?.getBoundingClientRect();
  const roomBase = (box && box.width > 0 && box.height > 0)
    ? { width: box.width, height: box.height }
    : stageExpectedRoom();
  const room = leafBoxInTree(root, ordinal, roomBase, grip);
  if (!(room.width > 0 && room.height > 0)) return fallback;
  const { rows, cols } = ruler.gridFor(room.width, room.height);
  return { rows, cols };
}

/* Wake one conversation, and hear whether this door is the one that opened it.
 *
 * `resume_session` answers `{ term, standing }`: the pane it just opened, or —
 * `standing`, with nothing spawned — the live pane already holding this
 * conversation, or the wake already opening it. That judgment is the
 * backend's, made once on the road every door takes (`conversation_wake.rs`)
 * and keyed by the conversation spelled one way. This side keeps no copy of
 * "is it already open": the copies it kept were per door, and a door could not
 * see another door's wake in flight — a sidebar row resumed the zo session
 * its own activation was still waking, and the second `zo --resume` died at
 * the first one's writer lease (2026-09-16, five times that day).
 *
 * A pane this door opened is written down straight away, the way the backend
 * records its own copy: the agent's first event may be minutes off, and a
 * persist before it would drop the tab as an agent tab with no way back. */
async function wakeConversation(agent, session, grid, interrupted = false, restore = null) {
  const woke = await invoke("resume_session", { agent, session, ...grid, interrupted, restore });
  if (!woke.standing) paneSessions.set(woke.term, { agent, session, resumable: true });
  return woke;
}

/* One leaf's shell, woken or fresh. A leaf holding a sleeping conversation
 * gets the agent's own resume command (`resume_session` — validated argv,
 * launch token, the session pre-seeded so the pane IS that conversation);
 * a wake that refuses falls back to a plain shell rather than a hole, which
 * is Orca's fallback too (`startFreshSpawn(null)` when the plan fails). */
async function spawnStoredLeaf(wake, launched, grid = null, restore = null) {
  // A wake that refused, kept for whatever pane stands in its place. Losing
  // it is how a single bad morning erased four workspaces: the fallback pane
  // holds no session, so the next persist writes a record with no `agents`,
  // and the conversation that was only ever ASLEEP is gone from disk. The
  // refusal is usually about the machine and not the conversation — a login
  // that did not materialise, an agent mid-upgrade — and it is not this
  // side's place to answer that by forgetting.
  let refused = null;
  // A record naming a conversation that is already standing. Such a leaf is a
  // plain shell and nothing else — not the program that was here, which would
  // be the very second process the judge exists to prevent.
  let duplicate = false;
  const targetGrid = grid ?? spawnGrid();
  const rows = targetGrid.rows;
  const cols = targetGrid.cols;
  if (wake?.agent) {
    const session = {
      key: wake.key,
      id: wake.id,
      ...(wake.transcript_path && { transcript_path: wake.transcript_path }),
    };
    try {
      // The record's word, not this window's guess: the LAST persist saw the
      // pane mid-turn, so the resume carries the continue nudge and the cut
      // turn restarts without anybody typing "go on".
      const woke = await wakeConversation(wake.agent, session, { rows, cols }, !!wake.interrupted, restore);
      if (!woke.standing) return { term: woke.term, woke: true, agent: wake.agent };
      // One conversation, one process. Two records naming the same session used
      // to resume it twice on every restart: the second `zo --resume` met the
      // first's writer lease and never published its channel, or the two
      // launches raced on the keychain and the second was refused there — and
      // either way the fallback below then opened a NEW conversation in the
      // pane the person was looking at, while the real one stood behind
      // another tab ("zo는 resume으로 이어서 하지 않았어 새 릴리즈로 시작되고",
      // 2026-09-13). A duplicate leaf opens as a plain shell and carries
      // nothing: the conversation is standing in the pane the answer names, and
      // a record that named it twice is what put two resumes on one transcript
      // to begin with.
      duplicate = true;
      console.warn(`[restore] ${wake.agent} ${wake.id} is already standing in term ${woke.term}; this leaf opens as a plain shell`);
    } catch (error) {
      // The agent left the PATH, or the vendor dropped the session. The layout
      // still deserves its pane — and the conversation still deserves its line
      // in the file.
      // `carriedOnly`, because the pane that ends up holding this record is NOT
      // the conversation — it is a plain shell carrying its line so the next
      // persist cannot forget it. The sidebar's doors read that word and still
      // offer to open the conversation, rather than walking somebody to a shell
      // wearing its name.
      refused = { agent: wake.agent, session, resumable: true, carriedOnly: true };
      // Said, not merely logged. A refusal is almost always about the machine
      // — an expired login, an agent mid-upgrade — and the person who reads the
      // pane below sees a plain shell where their conversation was. This used
      // to stop at `console.warn`, so "이어서" simply did not happen and the
      // next investigation had nothing to start from.
      showError(
        t("session.wakeRefused", "{{agent}} 대화를 이어서 열지 못했습니다 — {{reason}}", {
          agent: agentName(wake.agent),
          reason: String(error),
        }),
      );
    }
  }
  // No conversation to re-enter, but a program that was here — started fresh,
  // because there is nothing to resume and a fresh one is what the pane held
  // a moment before the window closed. This is the nearest this side can
  // stand to Orca, whose restored tabs ARE the running programs, reattached;
  // a plain shell in their place is a pane that lost its agent.
  //
  // Never over a wake that REFUSED. A fresh agent where a conversation was
  // reads as "the resume did not work, it started over" — and the fresh
  // agent's first report then replaced the carried conversation in this
  // pane's record, so the file forgot it too. The refused conversation gets
  // a plain shell carrying its line, the toast above says why, and the
  // sidebar's door reopens it once whatever refused it is gone.
  if (launched && !refused && !duplicate) {
    try {
      const term = await launchAgentTab({ agent: launched, prompt: "", rows, cols, restore });
      return { term, woke: false, agent: launched };
    } catch {
      // The agent is not on this machine any more. Same reading as a wake
      // that refused: the layout still deserves its pane.
    }
  }
  try {
    const term = await invoke("open_term_tab", { rows, cols, plain: true, restore });
    // The pane is a plain shell and is not going to pretend otherwise — the
    // tab keeps no agent's name over a shell nobody can talk to. But the
    // conversation it stood in for rides along, so the next window asks for
    // it again instead of writing the file as though it never existed.
    if (refused) paneSessions.set(term, refused);
    return { term, woke: false, agent: null };
  } catch (error) {
    showError(error);
    return null;
  }
}

/* Start this tab's shells and put them into it — the moment a restored tab
 * stops being a name on the strip and becomes terminals.
 *
 * This is Orca's `connect()` at the moment a pane renders (store-BgJxB0hr.js:
 * 945920), and it is the ONLY road a stored tab takes to a shell: no session
 * of ours outlives the window, so there is never a live pty to reattach to and
 * every restored leaf is a fresh spawn. What it must never be is a painted
 * corpse — the stored screen goes back onto a REAL shell below, so the scroll
 * back a person recognises is sitting in a terminal they can type into.
 *
 * Answers whether the tab woke. It does not, only when every spawn failed. */
async function mountStoredLayout(record, worktree, tab) {
  const wanted = recordLeafCount(record.root);
  const terms = [];
  let wokeAgent = null;
  let ranAgent = null;
  for (let at = 0; at < wanted; at += 1) {
    const wake = record.agents?.[at];
    const grid = storedLeafGrid(record.root, at, tab);
    // What this leaf's screen held when the window closed rides INTO its
    // spawn: the backend feeds it to the new terminal before that terminal is
    // held, so before the program's first byte is parsed. A replay sent after
    // the spawn answered came after a resumed zo had switched bracketed paste
    // on, and the replay's mode reset switched it off again for the pane's
    // whole life — a multi-line paste then submitted at its first line break
    // (2026-09-17). Addressed by the name the file gave this tab: a record
    // from a file written before those names existed has no screens this
    // window can find, and asks for none rather than guessing at a position.
    const restore = record.id != null ? { worktree, id: record.id, ordinal: at } : null;
    const spawned = await spawnStoredLeaf(wake, record.running?.[at], grid, restore);
    if (!spawned) break;
    terms.push(spawned.term);
    if (spawned.woke && !wokeAgent) wokeAgent = wake.agent;
    else if (!spawned.woke && spawned.agent && !ranAgent) ranAgent = spawned.agent;
  }
  if (terms.length === 0) return false;
  let layout = fillLeaves(record.root, [...terms]);
  // A resume can die between its spawn and this assignment — a dead
  // conversation's `--resume` exits about a second in — and the exit prune
  // finds no tab holding the term yet, so it parks the id in `exitedUnheld`
  // and the pruning happens HERE, before the layout ever stands. Without
  // this the pane mounted already dead and no later event would ever clear
  // it: the standing empty pane of three husk hunts (블랙박스 2026-08-18,
  // terms 3·4 "ended code Some(1)" two seconds after birth). A leaf that
  // died may shift a stored title onto its neighbour below — the dead pane's
  // name has nowhere truer to go.
  const alive = terms.filter((term) => !exitedUnheld.delete(term));
  if (alive.length === 0) return false;
  if (alive.length < terms.length) {
    layout = prunePanes(layout, new Set(alive));
    if (!layout) return false;
  }
  const leaves = paneLeaves(layout);
  // Ordinals back into shell ids. The backend already bounded every ordinal
  // against the tree, so a miss here is a spawn that failed above.
  const titles = leafWords(record.titles, leaves);
  const names = leafWords(record.names, leaves);
  tab.term = leaves[0];
  tab.layout = layout;
  tab.activePane = leaves[record.active ?? 0] ?? leaves[0];
  if (Object.keys(titles).length > 0) tab.paneTitles = titles;
  if (Object.keys(names).length > 0) tab.paneNames = names;
  if (record.expanded != null && leaves[record.expanded] !== undefined) {
    tab.expandedPane = leaves[record.expanded];
  }
  // A woken tab is an agent tab again, in the reopened-conversation spelling
  // the resume path already uses — and a tab whose conversation REFUSED to
  // wake is a plain shell, named as one. Wearing the agent's name over a bare
  // shell is the impostor the stored set is filtered to prevent, and it must
  // not come back here on the one road that can find out for certain.
  if (wokeAgent) {
    tab.resumed = true;
    tab.agent = agentName(wokeAgent);
  } else if (ranAgent) {
    // A program started fresh is not a conversation continued, so it must not
    // borrow the resumed wording. It is a REAL agent though — the one this
    // pane was running — so wearing its name is not the impostor the stored
    // set is filtered to prevent. That impostor is a bare shell in a name it
    // never earned, and this is the agent itself.
    tab.agent = agentName(ranAgent);
  } else {
    delete tab.agent;
  }
  // Asleep no longer — cleared before the paint, because everything below
  // reads the tab and a tab that still claimed to be asleep would be sent
  // straight back here by the stage.
  delete tab.asleep;
  renderPanes(tab);
  renderTabs();
  updateStage();
  paintRunningCount();
  // A restored coordinator's provider session and fresh team now both exist.
  // Native code decides which (if any) leaf is the team's actual leader and
  // which run that session is bound to; paths and run ids never cross here.
  for (const leader of leaves) {
    void invoke("restore_orchestration_workers", { leader }).catch(() => {});
  }
  if (tab.id === activeTabId && termFloat.hidden) keySink.focus();
  return true;
}

/* Shells that ENDED while no tab yet held them.
 *
 * The exit prune finds a pane by the tab that holds it, and a restore
 * assigns its layout only after every leaf has spawned — so a shell that
 * dies inside that window (a dead conversation's `--resume` exits about a
 * second in) has no holder to prune from. Its id parks here and the mount
 * consumes it. Ids are never reused (`next_term`'s own rule), so an entry
 * is a fact about one shell forever, and `delete`-on-read keeps the set at
 * the handful a single restore can strand. */
const exitedUnheld = new Set();

/* The wake in flight for a tab, by tab id.
 *
 * A wake is several round trips long and the stage repaints inside it, so a
 * second caller must not start a second set of shells for the same tab. It
 * must not be DROPPED either, which is what a bare "already going, give up"
 * did: the restore and the stage can both reach the same tab inside one turn,
 * and the one that arrived second simply returned — to a caller that had every
 * reason to believe the tab was now awake. A promise rather than a flag, so
 * the second caller joins the first and learns the same answer. */
const waking = new Map();

async function wakeStoredTab(tab) {
  // Not during a restore: the loop there puts the whole set on the strip
  // before deciding which one is on screen, and each tab landing repaints the
  // stage. Waking from those repaints would spawn every tab in the set —
  // which is the eager open this slice exists to stop.
  if (!tab.asleep || tab.wakeRefused || restoringPanes) return;
  const held = waking.get(tab.id);
  if (held) {
    await held;
    return;
  }
  const going = (async () => {
    const woke = await mountStoredLayout(tab.asleep, tab.worktree, tab);
    // Every spawn refused — no shell could be started at all. The tab keeps
    // its record and stays asleep, but it stops trying: the stage repaints
    // for reasons that have nothing to do with this tab, and a spawn that
    // fails once fails on each of them, which is one error message per
    // repaint. Picking the tab again lifts the mark (`setActiveTab`), so the
    // person's own gesture is what tries again.
    if (!woke) {
      tab.wakeRefused = true;
      return;
    }
    // The file learns this tab's shells now: the exit capture finds a leaf's
    // screen through the pty ids in the record, and a tab that woke without
    // telling the file would be captured as though it had never opened.
    persistPaneLayouts(tab.worktree);
  })();
  waking.set(tab.id, going);
  try {
    await going;
  } finally {
    waking.delete(tab.id);
  }
}

/* The eager wakes a restore started behind the tab in front, by worktree —
 * the loop's promise, held until it settles. */
const eagerWakes = new Map();

/* Every stored wake this workspace has in flight, settled: the tab in front's
 * (`waking`) and the eager loop behind it (`eagerWakes`).
 *
 * Asked by a door that has just opened a workspace to reach one conversation
 * in it. Opening a workspace IS its restore, and the conversation the door
 * names may be one of the panes that restore is waking; asked before those
 * land, the backend's answer names a pane no tab holds yet, and the door has
 * nothing to go to. */
async function storedWakesSettled(worktree) {
  await Promise.allSettled([
    eagerWakes.get(worktree),
    ...tabs.filter((tab) => tab.worktree === worktree).map((tab) => waking.get(tab.id)),
  ]);
}

/* A stored tab on the strip with no shells in it yet.
 *
 * Orca's tab records live in its session store and hold no pty at all until a
 * pane renders — which is why opening a project there shows every tab a
 * workspace had while starting only the shells of the one in front of you
 * (§11: "PTY 스폰은 페인이 실제로 렌더될 때"). Ours are the same object the
 * strip already draws, minus the two things a shell provides: no `term`, and a
 * `layout` of nothing. `asleep` holds the record they will be built from. */
let asleepTabs = 0;

function openStoredTab(record, worktree) {
  const tab = {
    // Not `termTabId` — there is no shell to name it after yet, and the id
    // has to be stable across the wake that gives it one.
    id: `term:asleep:${(asleepTabs += 1)}`,
    kind: "term",
    worktree,
    layout: null,
    activePane: null,
    asleep: record,
    // The name the file knows it by, kept for every later save.
    ...(record.id != null && { storedId: record.id }),
    ...(record.pinned && { pinned: true }),
  };
  // Never focused here. Which tab is looked at — and so which one starts its
  // shells — is the restore's own decision, made once the whole set is up.
  //
  // What comes back is the tab on the STRIP, not the object built above:
  // `openTab` seats a copy, and the restore mutates what it is handed.
  return openTab(tab, { focus: false });
}

/* A record's words by leaf ordinal, back onto shell ids — one reader for the
 * person's titles and the born names, so the two cannot be bounded apart. */
function leafWords(byOrdinal, leaves) {
  const words = {};
  for (const [index, said] of Object.entries(byOrdinal ?? {})) {
    const term = leaves[Number(index)];
    if (term !== undefined && typeof said === "string" && said !== "") words[term] = said;
  }
  return words;
}

/* What a tab that has not woken yet is called. Its own stored name when it
 * has one — a person's title first, else the name its one pane was born with
 * — else the agent asleep in it, else the plain word — never the numbered
 * spelling, which names a tab after the shell it holds and this one holds
 * none. */
function storedTabLabel(record) {
  const leaves = recordLeafCount(record.root);
  const named = leaves === 1 ? record.titles?.["0"] ?? record.names?.["0"] : undefined;
  if (named) return named;
  for (let at = 0; at < leaves; at += 1) {
    const agent = record.agents?.[at]?.agent;
    if (agent) return agentName(agent);
  }
  return t("terminal.label", "터미널");
}

/* Does this stored set hold a CONVERSATION to come back to?
 *
 * The question decides which of two things a workspace opens with, so it is
 * asked of the records alone — no stage, no shells, nothing spawned. What it
 * reads is exactly what `mountStoredLayout` reads: a wake at a leaf's ordinal
 * in the record's `agents` map, bounded by the tree's own leaves because an
 * ordinal past the last leaf is one the mount never looks at. The test on the
 * wake is `spawnStoredLeaf`'s own, so this answer cannot disagree with what
 * the restore would actually do with the same record — a leaf that would be
 * resumed counts, and one that would come up as a plain shell does not. */
function storedLayoutsHoldProgram(records) {
  if (!Array.isArray(records)) return false;
  return records.some((record) => {
    const wakes = record?.agents;
    const ran = record?.running;
    if (!wakes && !ran) return false;
    const leaves = recordLeafCount(record.root);
    for (let at = 0; at < leaves; at += 1) {
      if (wakes?.[at]?.agent || ran?.[at]) return true;
    }
    return false;
  });
}

/* What this worktree stored, as records — the read on its own, because the
 * caller has to LOOK at them before deciding whether to mount them, and one
 * activation has no business asking the backend the same question twice. */
async function storedPaneLayouts(worktree) {
  let records;
  try {
    records = await invoke("pane_layouts", { worktree });
  } catch {
    return [];
  }
  return Array.isArray(records) ? records : [];
}

/* The tabs this worktree had when it was last looked at, back on the strip.
 *
 * Every one of them comes back ASLEEP — the whole set is drawn, and not one
 * shell is started here. The set's own record says which tab the eye was on;
 * that tab is handed the stage, and the stage starting its shells is the only
 * spawn a restore performs (`wakeStoredTab`, through `updateStage`). A
 * workspace with nine stored tabs therefore opens nine tabs and one shell,
 * which is what Orca does and what makes opening a project cheap.
 *
 * Answers whether anything came back, so the caller knows whether the plain
 * first terminal is still needed. Takes the records when the caller has
 * already read them; asks for itself when nobody has. */
async function restoreWorktreeLayouts(worktree, stored) {
  // Only entries that are records. The backend hands over a typed list, so
  // this costs nothing — but it runs at boot, and a file somebody edited must
  // not be able to stop the window on its way up.
  const records = (stored ?? (await storedPaneLayouts(worktree))).filter(Boolean);
  if (records.length === 0) return false;
  restoringPanes = true;
  let made = [];
  try {
    made = records.map((record) => openStoredTab(record, worktree));
  } finally {
    restoringPanes = false;
  }
  // Which tab the eye was on. Nothing in the file says so only when the file
  // was written before this window kept the answer — the last tab is where
  // the restore used to leave somebody, and it stays the fallback.
  const looked = records.findIndex((record) => record?.focused);
  const eye = made[looked >= 0 ? looked : made.length - 1];
  setActiveTab(eye.id);
  // A tab that held a CONVERSATION does not stay asleep.
  //
  // The lazy rule above is Orca's, and copying it produced a result Orca does
  // not have: Orca's restored tabs are the running programs themselves,
  // reattached, so every agent that was working is working the moment the
  // window is back. Ours re-spawns from a record, and only the tab the eye was
  // on was ever re-spawned — so an agent that was not in front simply never
  // came back. Reported exactly that way, twice: "lotto세션은 코덱스가 켜있지
  // 않음", then "claude는 복구되도 자동으로 뜨는데 codex는 안떠". Claude was
  // the tab the eye was on. That is the whole of the asymmetry.
  //
  // Bounded on purpose. Only this workspace, only the tabs holding a
  // conversation — a stored plain shell is a shape and stays lazy, which is
  // what keeps opening a project of nine terminals cheap. One at a time and
  // off the critical path, so the stage settles at the same moment it did
  // before and the spawns arrive behind it.
  const eager = (async () => {
    for (const [at, record] of records.entries()) {
      const tab = made[at];
      if (!tab || tab.id === eye.id || !storedLayoutsHoldProgram([record])) continue;
      try {
        await wakeStoredTab(tab);
      } catch (error) {
        // A wake that throws must not take the wakes behind it. Each stored
        // conversation is its own pane and its own process; one that cannot
        // start is one tab's problem, and swallowing it here silently is how
        // the last one of these went unnoticed.
        showError(error);
      }
    }
  })();
  // Held until it settles, for a door that opened this workspace to reach one
  // of these conversations (`storedWakesSettled`).
  eagerWakes.set(worktree, eager);
  void eager.finally(() => {
    if (eagerWakes.get(worktree) === eager) eagerWakes.delete(worktree);
  });
  return true;
}

/* Say the terminals' names again, in the language now in force.
 *
 * These hosts are built here rather than written in the markup, so
 * `applyLocale` has never seen them and never will — it walks the document for
 * `data-i18n-*`, and nothing puts those on a node this file creates. */
function relabelTerminalViews() {
  for (const view of termViews.values()) {
    view.host.setAttribute("aria-label", t("terminal.label", "터미널"));
  }
  // The stage's own furniture, which nothing else reaches. A strip lives as
  // long as its group and the split containers are no longer torn down on
  // every render, so a language change has to arrive here or these two keep
  // the words they were born with. (The strip never had this at all — it has
  // been built once per group since groups existed.)
  for (const held of groups.values()) {
    held.strip.setAttribute("aria-label", t("tab.label", "탭"));
  }
  for (const grip of stage.querySelectorAll(".stage-grip")) {
    grip.setAttribute("aria-label", t("terminal.paneResize", "페인 크기 조절"));
  }
}

/* ---- where the next terminal goes ----
 *
 * Orca lets the ASKER request a division: a launch can carry
 * `splitFromLeafId` with a `splitDirection`, and the layout writer takes
 * `splitDirection ?? "horizontal"` (`addSplitLeafToLayout`,
 * index-ftls8Hg_.js:725832). Everything that does not ask gets a tab.
 *
 * We go one step past that, on the product owner's decision: a terminal
 * opened in the workspace already in front of somebody DIVIDES the stage
 * rather than hiding what is on it ("orca처럼 여러 창이 열리면서 보여야 함 —
 * 터미널이 여러 개 열리면 화면에 맞게 분할됨"). A strip of tabs shows one
 * shell at a time, and the reason to open a second one beside a working agent
 * is almost always to watch both at once.
 *
 * Three boundaries, and each of them is a thing somebody would otherwise
 * cross without meaning to:
 *
 *   - **Four panes is the cap.** 2×2 is the last division where a terminal
 *     still holds a readable number of columns at the sizes this window runs
 *     at; a fifth would halve one of them again. Past the cap this answers
 *     exactly what it answered before this slice existed — a tab — so the
 *     strip is still where terminals five and up live.
 *   - **The longer side is the one that is cut.** A pane wider than it is
 *     tall divides left/right, a taller one top/bottom. That is what "화면에
 *     맞게" means: the halves stay as square as the room allows instead of
 *     one dimension being shaved over and over. Ties divide downwards —
 *     a square pane has no longer side, and one of the two has to be the
 *     answer.
 *   - **Same workspace only.** The stage draws one checkout's tabs
 *     (`paneTabs`), so a shell belonging to another checkout cannot be a pane
 *     of a tab belonging to this one. A schedule that fired in a workspace
 *     nobody is looking at still gets its own background tab.
 *
 * No DOM in here. The measurements arrive as numbers, so the whole table can
 * be asserted directly rather than inferred from a rendered layout. */
const TILE_PANE_CAP = 4;

function tilePlacement({ sameWorktree, activeIsTermTab, paneCount, paneWidth, paneHeight }) {
  if (!sameWorktree || !activeIsTermTab || paneCount >= TILE_PANE_CAP) return { tab: true };
  return paneWidth > paneHeight ? { split: "right" } : { split: "down" };
}

/* The pane a division would cut: how many that tab holds, and how much room
 * the one with the keyboard has right now.
 *
 * The only measuring on this road, kept out of the rule above so the rule
 * stays a table. A slot that is not on the stage measures zero, which the
 * rule reads as "no longer side" and answers with a division downwards — the
 * honest answer about a shape nobody can see. */
function activePaneShape(tab) {
  if (tab?.kind !== "term") return { paneCount: 0, paneWidth: 0, paneHeight: 0 };
  const box = paneHosts
    .get(tab.id)
    ?.querySelector(`.pane-slot[data-term="${activePaneOf(tab)}"]`)
    ?.getBoundingClientRect();
  return {
    paneCount: paneLeaves(tab.layout).length,
    paneWidth: box?.width ?? 0,
    paneHeight: box?.height ?? 0,
  };
}

/* Sit the new shell beside the one in front of somebody.
 *
 * `focus` keeps the promise it keeps on a tab: pressed for, the new pane
 * takes the keyboard the way ⌘D's does; fired by a schedule, the pane appears
 * and the hands stay exactly where they were. That asymmetry is the whole
 * reason a background mount is worth having.
 *
 * `source` is which pane gets cut. It defaults to the one with the keyboard,
 * which is what every by-hand road means by "here" — and it is a PARAMETER
 * because an orchestrating agent names the pane it wants divided, and that
 * pane is very often not the one somebody is standing in. One door either
 * way: two functions that both call `addPane` is how the pane titles, the
 * running count and the repaint come to be done in one of them only.
 *
 * The name the caller gave this shell goes on the PANE, as the name it was
 * BORN with (`paneNames`) and not as a title a person set. The tab already
 * has a name and the strip has one slot for it — `tabLabel`'s own rule — so
 * the second answer belongs in the pane's own bar, which is where Orca
 * resolves it per leaf (`paneTitle ?? tabTitle`, index-ftls8Hg_.js:20195).
 *
 * Every division re-evens the whole tree. Orca keeps halving the pane that
 * was split (`addSplitLeafToLayout` ratio 0.5, and the live manager's two
 * `flex 1 1 0%` children — pane-tree-ops.ts:222), so five splits leave a
 * 1/64 sliver; the person named that broken twice ("재대로 안쪼개짐",
 * "화면 분활이 갯수에 따라 등분되야함..정확한 크기로"). So the equalize the
 * balance action already runs is run here too, and N panes are each 1/N —
 * a recorded better-than-Orca deviation, not a drift. */
function tileTermPane(tab, term, extra, side, focus, source = activePaneOf(tab)) {
  tab.layout = addPane(
    tab.layout,
    source,
    term,
    side === "right" ? "vertical" : "horizontal",
  );
  equalizePanes(tab.layout);
  const named = (extra.title ?? extra.agent)?.trim();
  if (named) tab.paneNames = { ...(tab.paneNames ?? {}), [term]: named };
  if (focus) tab.activePane = term;
  renderPanes(tab);
  updateStage();
  paintRunningCount();
  if (focus) keySink.focus();
  return tab;
}

/* Put a tab around a shell the backend already opened — or a pane, when the
 * stage can be divided instead. One builder for every road that ends in a
 * terminal — ⌘T, an agent launched from the notes menu, a quick command —
 * because three copies of this object is how one of them comes to forget a
 * field, and because a placement rule spread across nine call sites is nine
 * chances to disagree about it.
 *
 * `placement: "tab"` is how the roads that must never divide say so: a
 * repository's declared `defaultTabs:` (the file said TABS), and a
 * conversation being resumed — from the menu or carried in from the vault —
 * because going back into one is a destination, not company for the shell
 * beside it. Said out loud at those call sites rather than inferred from
 * `restoringPanes`, which `mountRunTab` also raises for an entirely different
 * reason — and an automation run IS something to divide the stage for.
 *
 * A layout being restored does not come through here at all any more. It puts
 * its own tabs on the strip asleep and fills them when they are looked at
 * (`openStoredTab`/`mountStoredLayout`), which is what keeps a restored set
 * the shape somebody left rather than a set folded into the tab before it.
 *
 * One pane to start, which is what a tree of a single leaf is. The measure
 * happens only once the host is on screen: a surface that is still hidden
 * measures zero and would size the shell to nothing — which is also why a
 * background mount costs nothing here and picks its size up from
 * `updateStage` the first time it is looked at. */
/* Where a new shell goes, by the rule: the tab the new shell would be born
 * beside — the one the FOCUSED group is showing, not the globally active one;
 * the strip's `＋` sets that group before asking, and it means "another
 * terminal here" — unless the road said `tab`, and the division
 * `tilePlacement` picks for it. Asked twice per shell, by `spawnGrid` before
 * the pty exists to size it and by `mountTermTab` after to seat it, so the
 * two can never disagree. */
function plannedPlacement(placement, worktree) {
  const host = placement === "tab" ? null : activeTabIn(focusedPane);
  const shape = activePaneShape(host);
  const where = tilePlacement({
    sameWorktree: host !== null && host.worktree === worktree,
    activeIsTermTab: host?.kind === "term",
    ...shape,
  });
  return { host, shape, where };
}

function mountTermTab(term, extra = {}, { focus = true, placement = "auto" } = {}) {
  const worktree = extra.worktree ?? activeWorktreePath;
  const { host, where } = plannedPlacement(placement, worktree);
  if (where.split) return tileTermPane(host, term, extra, where.split, focus);
  // A terminal asked for while a BROWSER is in front stands beside it — the
  // stage's own split, because a shell cannot tile INTO a native page the way
  // it tiles into another shell (live report 2026-08-14: + over NAVER opened
  // a far-away tab). Doc tabs keep Orca's tab road.
  const beside =
    host?.kind === "browser" && host.worktree === worktree ? standBesideFocused() : null;
  const tab = {
    id: termTabId(term),
    kind: "term",
    term,
    worktree: activeWorktreePath,
    layout: paneLeaf(term),
    activePane: term,
    ...(beside === null ? {} : { pane: beside }),
    ...extra,
  };
  renderPanes(tab);
  openTab(tab, { focus });
  if (beside !== null) persistStageLayouts();
  termViews.get(term)?.measure();
  resizeTermTab(term);
  return tab;
}

/* Place the backend-owned setup terminal without ever receiving its command.
 * New Tab is deliberately background-only. A split joins an existing terminal
 * tab when there is one; a lane/browser surface gets a stage neighbour with
 * the same visible axis. A workspace created in the background has no visible
 * host to split, so its Setup tab stays background until that workspace is
 * opened. */
function mountSetupTerminal(worktree) {
  const setup = worktree?.setup_terminal;
  if (!setup || !Number.isInteger(setup.term)) return null;
  const mode = normalizedSetupScriptLaunchMode(setup.launch_mode);
  const extra = { title: "Setup", worktree: worktree.path };
  const inFront = activeWorktreePath === worktree.path
    ? activeTabIn(focusedPane)
    : null;
  if (mode !== "new-tab" && inFront?.kind === "term") {
    return tileTermPane(
      inFront,
      setup.term,
      extra,
      mode === "split-vertical" ? "right" : "down",
      false,
    );
  }
  if (mode !== "new-tab" && inFront) {
    const pane = splitStageBesideFocused(
      mode === "split-vertical" ? "horizontal" : "vertical",
    );
    if (pane !== null) {
      const tab = mountTermTab(
        setup.term,
        { ...extra, pane },
        { focus: false, placement: "tab" },
      );
      persistStageLayouts();
      return tab;
    }
  }
  return mountTermTab(setup.term, extra, { focus: false, placement: "tab" });
}

/* ⌘T. Orca's centre opens terminals as tabs and each is its own shell; ours
 * is the same, in the checkout being looked at — divided into the stage
 * beside the terminal already there, when `mountTermTab`'s rule says so.
 *
 * `placement` is passed rather than decided here: the only caller that wants
 * a tab no matter what is a repository's declared `defaultTabs:`, and it says
 * so itself. */
/* 지금 워크스페이스에 서 있는 에이전트 탭 수.
 *
 * 탭과 그 안의 페인을 함께 센다: 두 번째 에이전트는 새 탭으로 오기도 하고 같은
 * 탭의 분할로 오기도 하는데, 어느 쪽이든 나란히 도는 두 에이전트다. */
function agentTabsHere() {
  return tabs.filter(
    (tab) => tab.kind === "term" && tab.agent && tab.worktree === activeWorktreePath,
  ).length;
}

async function openTermTab({ placement, door = "agent" } = {}) {
  // WHICH DOOR asked, because the original has three and they do not agree.
  //
  //   "terminal"  ⌘T, the 명령 실행 button, the ＋ palette's 새 터미널 row,
  //               the empty stage's invitation. The original's
  //               `tab.newTerminal` (`shared/keybindings.ts:540-547`) runs
  //               `handleNewTab()` → `openNewTerminalTabInActiveWorkspace`
  //               → `createTab(worktreeId, groupId)` with no `launchAgent`
  //               and no shell override (`store/slices/terminals.ts:1501-1553`).
  //               NOTHING on that path reads `settings.defaultTuiAgent` —
  //               `pickTuiAgent` has four callers and this is not one of them.
  //               Ours launched claude here, and the button even said 셸 on
  //               its face while doing it. That is the report.
  //   "command"   a tab a COMMAND is about to be typed into (`defaultTabs:`).
  //               The original queues the text onto a plain `createTab`
  //               (`lib/run-quick-command-in-new-tab.ts:85-92`), and a bare
  //               shell is the only thing a shell command can be typed into.
  //   "agent"     activation seating this workspace's first terminal. THIS is
  //               where the default agent belongs, and it is why the branch
  //               below stays rather than being deleted.
  //
  // 자동 keeps this window's documented divergence on the two non-agent
  // doors: the terminal command setting for "terminal", a bare shell for
  // "command". The agent list may not have loaded yet when this is the first
  // thing pressed — ask once, the way the notes menu already does.
  if (door === "agent" && defaultAgentChosen()) {
    if (agentRows.length === 0) await refreshAgents();
    const chosen = installedAgents().find((row) => row.id === defaultAgentId());
    if (chosen) {
      // 이 워크스페이스에 이미 에이전트가 서 있었나 — **띄우기 전에** 센다.
      // 이 창은 부팅·프로젝트 전환·워크스페이스 활성화마다 첫 에이전트를 스스로
      // 띄우므로, "첫 에이전트를 실행했다"는 아무도 누르지 않아도 참이다.
      // 사람이 한 일인 것은 **둘째**부터다.
      const already = agentTabsHere();
      try {
        const term = await launchAgentTab({
          agent: chosen.id,
          prompt: "",
          ...spawnGrid({ placement }),
        });
        mountTermTab(term, { agent: chosen.name }, { placement });
        if (already > 0) markFirstRun("ran_second_agent");
        return term;
      } catch (error) {
        showError(error);
        return null;
      }
    }
  }
  let term;
  try {
    term = await invoke("open_term_tab", {
      ...spawnGrid({ placement }),
      // A tab that is about to be TYPED INTO is a bare shell whatever the
      // terminal command says; the other two doors keep asking the agent
      // setting only in the one place the original does.
      plain: door === "command" || defaultAgent.kind === "blank",
    });
  } catch (error) {
    showError(error);
    return null;
  }
  mountTermTab(term, {}, { placement });
  return term;
}

/* ⌘D / ⌘⇧D. Divide the pane in front of you and start a shell in the new
 * half — Orca's `terminal.splitRight`/`terminal.splitDown`, whose measured
 * defaults on macOS are exactly these two chords
 * (plugin-manifest-Dq3wpxrr.js:5676-5710).
 *
 * The new shell starts where this tab's shells start, which is the checkout
 * being looked at. Orca inherits the pane's own working directory; we cannot
 * read a running child's cwd, and the checkout is the honest answer to the
 * same question rather than a guess at a different one. */
async function splitActivePane(direction) {
  const tab = currentTab();
  if (tab?.kind !== "term") return;
  const source = activePaneOf(tab);
  if (source === null) return;
  let term;
  try {
    // Always a plain shell. Orca's split — header button, context menu and
    // keyboard alike — goes through `splitTerminalPaneWithInheritedCwd`,
    // which hands `manager.splitPane(pane.id, direction, { cwd })` a working
    // directory and nothing else (OnboardingInlineCommandTerminal-
    // D_M5zIRV.js): no command, no agent. The new-terminal command belongs
    // to NEW TABS; a split is more room beside work already running, and
    // starting claude in it is how "오른쪽 claude 실행이 아니고 터미널이
    // 실행되게 해야함" got reported.
    term = await invoke("open_term_tab", {
      ...spawnGrid({ divide: { tab, source, direction } }),
      plain: true,
    });
  } catch (error) {
    showError(error);
    return;
  }
  tab.layout = addPane(tab.layout, source, term, direction);
  // 갯수에 따라 등분, 정확한 크기로 — the same re-even `tileTermPane` runs,
  // for the same reported reason. The two roads must not disagree about what
  // a division leaves behind.
  equalizePanes(tab.layout);
  // The new pane takes the keyboard, the way Orca's does
  // (`addSplitLeafToLayout` activates the new leaf unless told otherwise).
  tab.activePane = term;
  renderPanes(tab);
  updateStage();
  paintRunningCount();
  keySink.focus();
}

/* ⌘W inside a terminal closes the pane, not the tab.
 *
 * Both actions default to the same chord and Orca lets the terminal answer
 * first — its handler stops the event before the tab strip sees it
 * (`closeActivePane`, OnboardingInlineCommandTerminal-Caul4Dbw.js:1510-1517).
 * With one pane left there is nothing to close but the tab, so the two
 * meanings agree there and the chord keeps working the way it always has. */
/* 떼어 둔 에이전트 — 판은 닫혔고 프로세스는 돈다. term → 그 워크트리 경로.
 *
 * 사용자 계약(2026-08-21): "페인 닫기 해도 실제 분할 화면만 닫는 거고 …
 * 그걸 화면에서 닫더라도 실제로 프로젝트에서 닫는 게 아니면 돌아야 함."
 * 화면을 닫는 일과 세션을 끝내는 일이 한 동작이어서, 분할 하나를 정리하는
 * 손짓이 돌고 있는 에이전트를 죽이고 있었다. 이제 그 둘은 다른 일이다:
 * 화면만 돌려주고(`dropTermScreen` — 보드 미리보기가 이미 그 문을 쓴다),
 * 셸에 대한 기억은 그대로 두고, 그 줄이 워크트리 카드에 남아 **유일한 문**이
 * 된다. 끝내는 문은 따로 있다(`end_terminal_session`).
 *
 * 평범한 셸은 예전처럼 끝난다. 아무도 다시 찾지 않을 셸을 떼어 두는 것은
 * 기능이 아니라 새는 것이고, 사람이 이 계약으로 지키려는 것은 **에이전트**다. */
const detachedAgents = new Map();

/* 이 판에 에이전트가 서 있는가 — 넷 중 하나라도 말하면 그렇다. 목록이 행을
 * 세우는 조건과 같은 문장이어야 한다(`worktreeAgentRows`): 여기서 떼어 뒀는데
 * 거기서 안 세우면 그 에이전트는 문 없이 도는 것이 된다. */
function termHoldsAgent(term) {
  return (
    paneAgents.has(term) ||
    paneSessions.has(term) ||
    hookStates.has(term) ||
    paneSubagents.has(term)
  );
}

/* 떼어 둔 그 에이전트가 아직 **도는** 중인가 — ＋ 옆 숫자와 그 목록의 술어.
 *
 * 떼어 두기 자체의 조건이 아니다: 닫기는 done인 판도 떼어 둔다(대화가 살아
 * 있고 사람이 다시 붙을 수 있다 — `closing_a_split_lets_the_agent_in_it_
 * keep_running`의 네 절). 하지만 "판 없이 **도는** 에이전트"라는 숫자가
 * done을 세면, 끝난 셸은 exited가 영영 없으므로 그 수가 재시작을 지나서도
 * 남는 유령이 된다("핀 없이 도는 표시 버그", 2026-08-25 — 2가 계속 서
 * 있었다). 도는 것은: 훅이 산 상태를 말하거나(어휘는 `LIVE_HOOK_STATES`
 * 한 벌), 헬퍼가 붙어 있거나, 아직 아무 말도 못 한 갓 뜬 판이다 — 마지막
 * 절은 `worktreeAgentRows`가 훅 없는 vendor에 내리는 그 판정과 같다. */
function detachedStillWorking(term) {
  if (LIVE_HOOK_STATES.has(hookStates.get(term)) || paneSubagents.has(term)) return true;
  return !hookStates.has(term) && (paneAgents.has(term) || paneSessions.has(term));
}

function closePaneLeaf(tab, going) {
  const staying = paneLeaves(tab.layout).filter((term) => term !== going);
  if (staying.length === 0) return;
  tab.layout = prunePanes(tab.layout, new Set(staying));
  schedulePaneAlignment(tab);
  tab.activePane = staying[0];
  if (termHoldsAgent(going)) {
    detachedAgents.set(going, tab.worktree);
    dropTermScreen(going);
    // The person closed a tiled worker's pane: it went to the background.
    noteWorkerRoomChange(going, "background");
  } else {
    invoke("close_term", { term: going }).catch(() => {});
    dropTermView(going);
  }
  renderPanes(tab);
  updateStage();
  paintRunningCount();
  keySink.focus();
}

function closeActivePane() {
  const tab = currentTab();
  if (tab?.kind !== "term") return false;
  const leaves = paneLeaves(tab.layout);
  if (leaves.length < 2) return false;
  const going = activePaneOf(tab);
  // The close acts on the RECORDED pane after its foreground query/dialog —
  // never whichever pane focus wandered to while those async answers ran.
  if (confirmCloseRunning) {
    const key = `pane:${going}`;
    if (closeChecks.has(key)) return true;
    closeChecks.add(key);
    void terminalCloseNeedsConfirmation([going]).then(async (needsConfirmation) => {
      closeChecks.delete(key);
      if (!paneLeaves(tab.layout).includes(going)) return;
      if (needsConfirmation && !(await askAboutRunning(tab, [going]))) return;
      if (paneLeaves(tab.layout).includes(going)) closePaneLeaf(tab, going);
    });
    return true;
  }
  closePaneLeaf(tab, going);
  return true;
}

/* ⌘] / ⌘[ — walk the panes in the order they are drawn, and wrap.
 *
 * A tab with a pane expanded COLLAPSES first. Orca clears the mark, restores
 * the sizes and persists before it moves the active pane at all
 * (OnboardingInlineCommandTerminal-Caul4Dbw.js@76900), and the reason is that
 * the pane being walked to is hidden: moving the mark alone would hand the
 * keyboard to a shell that is not on screen. Collapsing is the honest answer
 * to "show me the next pane". */
function focusPane(step) {
  const tab = currentTab();
  if (tab?.kind !== "term") return;
  const leaves = paneLeaves(tab.layout);
  if (leaves.length < 2) return;
  const at = leaves.indexOf(activePaneOf(tab));
  const wanted = leaves[(at + step + leaves.length) % leaves.length];
  if (expandedPaneOf(tab) === null) {
    setActivePane(tab, wanted);
  } else {
    // One paint for both changes — `renderPanes` marks the active pane and
    // writes the layout itself, so a `setActivePane` here would only be a
    // second save of the same thing.
    tab.expandedPane = null;
    tab.activePane = wanted;
    renderPanes(tab);
  }
  keySink.focus();
}

/* Whether a shell's screen is on the stage right now.
 *
 * Still `host.hidden`, and that matters: a view paints nothing while its host
 * is hidden, which is what keeps background tabs off the thread that has to
 * deliver the next keystroke. `updateStage` is what sets the flag — a screen
 * inside a split is not hidden by its own tab going away, it is hidden
 * because the stage walked the panes of the tab that went and said so.
 *
 * And it has to still be IN the document. The flag is only ever written by the
 * stage's walk over the pane slots it can find, so a host whose slot was
 * detached is never visited again and keeps whatever it was left with — a
 * screen stuck at `hidden === false` forever. `readingTerms` derives the watch
 * set straight from this answer, and `previewingTerms` skips everything the
 * watch set holds, so one stranded phantom silently takes a live board card
 * out of the preview declaration: the card is on screen and the backend was
 * never told to feed it. That is the whole of the board-preview failure, and
 * it is the same discipline `previewingTerms` already applies to its own
 * nodes. Two other callers were quietly paying for it as well — a resize
 * against a detached node's zero grid, and a repaint of hosts nobody can see. */
function termShown(view) {
  return !view.host.hidden && view.host.isConnected;
}

/* The grid each shell was last TOLD it had — corrected by what it says it
 * WEARS.
 *
 * Everything that can change how much room a screen has ends in
 * `resizeTermTab` — the stage repaint, both grips, a pane split, a window
 * resize — and a tab switch is one of them even though it moves no divider at
 * all. So switching tabs sent a resize per visible shell, every time, for a
 * grid that had not changed by one column.
 *
 * That is not free on either side: it is an IPC round trip, it takes the lock
 * the frame pump reads every shell under, and `with_terminal` wakes the pump
 * out of its idle nap to service a message that changes nothing. The backend
 * refuses an unchanged size too (`PtyLane::resize`) — belt and braces, because
 * the cheapest round trip is the one that was never sent.
 *
 * Told is not wearing, though. A telling that never landed — an invoke lost
 * to a closing sibling's race, a refusal nobody caught — left this map
 * carrying the answer the pty never got, and the dedupe then blocked every
 * honest retelling of the same numbers forever: the survivor of a pane close
 * stayed cut at its old width until a window drag happened to move the
 * numbers (live report 2026-08-24, "옆에 터미널을 닫으면 짤려있어").
 * Every frame already carries the grid it was drawn on (`GridDelta::size`),
 * so `applyTermFrames` folds the pty's own report back into this map — the cache
 * tracks the shell, not this window's memory of itself. */
const termGrids = new Map();

/* The grid a shell spawned now will have once it is mounted — worked out
 * BEFORE the pty exists, so it is born at its real size instead of at 24×96
 * and told the truth a frame later. A program that draws at birth paints for
 * the size it is given: a resumed zo replayed its whole transcript at 96
 * columns and then had to rebuild it at the real width, and before it could,
 * a pane that grew under it left the transcript at the top and the composer
 * on the new floor with nothing between them ("빈공백 아직도", 2026-09-02).
 *
 * The room follows `mountTermTab`'s own rule: a tab takes a whole pane host,
 * a division takes half of the pane it cuts, less the grip between them, along
 * the side `tilePlacement` would pick. Pixels become rows and columns through
 * a laid-out view's cell size — the font is one per window. When nothing is
 * laid out yet (boot, a stage nobody can see) the default stands, as it always
 * did, and `resizeTermTab` still says the exact size once the pane is up. */
function spawnGrid({ placement = "auto", worktree = activeWorktreePath, divide = null } = {}) {
  const fallback = { ...DEFAULT_TERMINAL_GRID };
  if (isPopout) return fallback;
  const ruler = [...termViews.values()].find((view) => termShown(view) && !view.gridSize().blind);
  if (!ruler) return fallback;
  let where;
  let shape;
  if (divide) {
    // ⌘D and the orchestrator name the pane they cut and the direction; the
    // room is that pane's, halved the way `addPane` halves it.
    const box = paneHosts
      .get(divide.tab.id)
      ?.querySelector(`.pane-slot[data-term="${divide.source}"]`)
      ?.getBoundingClientRect();
    shape = { paneWidth: box?.width ?? 0, paneHeight: box?.height ?? 0 };
    where = { split: divide.direction === "vertical" ? "right" : "down" };
  } else {
    ({ shape, where } = plannedPlacement(placement, worktree));
  }
  let room;
  if (where.split) {
    const grip =
      parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue("--term-divider-hit-size"),
      ) || 0;
    const expected = stageExpectedRoom();
    const baseWidth = shape.paneWidth > 0 ? shape.paneWidth : expected.width;
    const baseHeight = shape.paneHeight > 0 ? shape.paneHeight : expected.height;
    room =
      where.split === "right"
        ? { width: Math.max(0, (baseWidth - grip) / 2), height: baseHeight }
        : { width: baseWidth, height: Math.max(0, (baseHeight - grip) / 2) };
  } else {
    const box = [...paneHosts.values()].find((one) => !one.hidden)?.getBoundingClientRect();
    room = (box && box.width > 0 && box.height > 0)
      ? { width: box.width, height: box.height }
      : stageExpectedRoom();
  }
  if (!(room.width > 0 && room.height > 0)) return fallback;
  const { rows, cols } = ruler.gridFor(room.width, room.height);
  return { rows, cols };
}

function resizeTermTab(term) {
  // The pop-out is a READER of these screens, never their owner. It runs
  // this same document, so its board previews hold term views too — and its
  // window-resize road walked every one of them and resized the REAL pty to
  // a preview card's grid. That is the whole of "팝아웃으로 열었다가
  // 들어오면 깨지는" — the shell came back squeezed to a card and nothing in
  // the main window had changed size, so nothing resized it back.
  if (isPopout) return;
  const view = termViews.get(term);
  if (!view || !termShown(view)) return;
  // Shown by its own flag, hidden by an ancestor's: an expansion hides the
  // branch it grew past on the SLOT (`gone.hidden`, `paneNodeEl`), and the
  // screen inside it keeps `hidden === false` while laying out nothing.
  // `gridSize` answers its guess for a box that measures no width, and that
  // guess rode to the pty as a real resize — 96×24 to a pane nobody could
  // see. A guess is not a telling.
  const { rows, cols, blind } = view.gridSize();
  if (blind) return;
  const last = termGrids.get(term);
  if (last !== undefined && last.rows === rows && last.cols === cols) return;
  termGrids.set(term, { rows, cols });
  // Forgotten on refusal: a size this window believes was delivered and was
  // not is a shell that never hears about the next one either.
  invoke("term_resize", { term, rows, cols }).catch(() => termGrids.delete(term));
}

function currentTab() {
  return tabs.find((tab) => tab.id === activeTabId) ?? null;
}

/* The glyph a tab wears, by what it holds. A lane is a terminal too — it is
 * the agent's TUI, unwrapped — so it wears the same one. */
const TAB_GLYPH = {
  term: "terminal",
  lane: "terminal",
  file: "file",
  image: "image",
  // 읽으라고 그린 판이니 책 — 같은 파일의 원본 탭과 한눈에 갈린다(1-g36).
  mdview: "book",
  // 표는 칸으로 서 있는 것 — 보드가 쓰는 그 글리프다(1-g39).
  csv: "columns",
  // 노트북도 읽는 판이다 — 미리보기와 같은 책(1-g40). 두 탭은 이름으로 갈린다.
  ipynb: "book",
  diff: "diff",
  imagediff: "diff",
  changes: "diff",
  board: "columns",
  // 장부도 칸으로 선다 — 표와 보드가 쓰는 그 글리프다.
  tokens: "columns",
  skills: "book",
  // The sprite has no globe; `external` is the outward-facing glyph it does
  // have, and a browser tab is the one tab that faces outward.
  browser: "external",
  emulator: "phone",
};

function tabLabel(tab) {
  // 표와 노트북은 그 파일 자체이지 파생된 판이 아니다 — 이름만 단다(1-g39·1-g40).
  if (
    tab.kind === "file" ||
    tab.kind === "image" ||
    tab.kind === "csv" ||
    tab.kind === "ipynb"
  ) {
    return basename(tab.path);
  }
  // 같은 파일의 원본 탭과 나란히 서므로 이름만으로는 두 탭이 한 탭으로 읽힌다
  // — 변경 탭이 쓰는 그 접미사 방식 그대로(1-g36).
  if (tab.kind === "mdview") {
    return t("mdview.tab", "{{name}} · 미리보기", { name: basename(tab.path) });
  }
  if (tab.kind === "diff" || tab.kind === "imagediff") {
    // A commit's diff is pinned in time and wears the hash instead of
    // "변경" — two tabs on the same path at different commits must not
    // read as the same tab twice.
    if (tab.commit) {
      return t("tab.atCommit", "{{name}} · {{hash}}", {
        name: basename(tab.path),
        hash: tab.commit.slice(0, 7),
      });
    }
    return t("tab.changed", "{{name}} · 변경", { name: basename(tab.path) });
  }
  if (tab.kind === "board") return t("board.tab", "작업 상황판");
  if (tab.kind === "knowledge") return t("knowledge.tab", "지식 그래프");
  if (tab.kind === "skills") return skillText("title");
  if (tab.kind === "artifacts") return t("artifacts.tab", "아티팩트");
  if (tab.kind === "jev") return t("jev.tab", "Jev 대시보드");
  if (tab.kind === "tokens") return t("tokens.tab", "토큰 사용량");
  if (tab.kind === "emulator") {
    return tab.deviceName || t("emulator.tab", "모바일 에뮬레이터");
  }
  // Orca's browser tab is born "New Browser Tab", wears the page's own title
  // as soon as one arrives, and falls back to the address between the two.
  if (tab.kind === "browser") {
    if (tab.pageTitle) return tab.pageTitle;
    if (tab.url && tab.url !== BROWSER_BLANK_URL) return tab.url;
    return t("browser.tab", "새 브라우저 탭");
  }
  if (tab.kind === "worker") {
    const said =
      tab.worker.status === "running" ? t("worker.running", "실행 중")
      : tab.worker.status === "failed" ? t("worker.failed", "실패")
      : tab.worker.status === "orphaned" ? t("worker.orphaned", "부모 종료")
      : t("worker.done", "완료");
    return `${tab.worker.name} · ${said}`;
  }
  if (tab.kind === "changes") {
    if (tab.source === "committed") {
      return t("changes.committedTab", "커밋됨 · {{base}}", {
        base: tab.baseRef || "base",
      });
    }
    // 영역 문과 커밋 문은 표면의 이름을 그대로 문패로 — 원본의 라벨이 탭
    // 라벨이다(openAllDiffs/openCommitAllDiffs).
    if (tab.source === "commit" || tab.area) return changesSurfaceLabel(tab);
    return t("changes.tab", "모든 변경");
  }
  // A terminal born as an agent wears the agent's name — Orca titles a
  // launched tab after what it launched, not after the shell underneath.
  if (tab.kind === "term") {
    // Restored and not opened yet: the record is all there is to go on, and
    // it is enough — this is the name the strip carried before the restart.
    if (tab.asleep) return storedTabLabel(tab.asleep);
    // A named pane names the tab, but only while the tab IS that pane. Orca
    // resolves a tab's title as `paneTitle ?? tabTitle` per leaf
    // (index-ftls8Hg_.js:20195); with two panes there are two answers and the
    // strip has one slot, so the panes keep their names in their own bars and
    // the tab keeps its own.
    const leaves = paneLeaves(tab.layout);
    if (leaves.length === 1) {
      const named = paneTitleOf(tab, leaves[0]);
      if (named) return named;
      // The program's own word (OSC 0/2) — what Orca's strip shows for a
      // terminal that is doing something. A person's name for the pane still
      // wins above; the number below is only for a shell that says nothing.
      const spoken = termTitles.get(leaves[0]);
      if (spoken) return spoken;
      const prompt = panePrompts.get(leaves[0])?.trim();
      if (prompt) return prompt;
    }
    return tab.title ?? tab.agent ?? t("terminal.numbered", "터미널 {{n}}", { n: tab.term });
  }
  const entry = lanes.get(focusedId);
  return entry ? laneTitle(entry.lane) : t("terminal.label", "터미널");
}
