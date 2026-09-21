/* ---- input ----
 *
 * tmux rule: the keyboard belongs to the active terminal. The floating shell
 * when it is open, else the staged lane's TUI. The sink textarea gives the
 * IME somewhere to compose; plain characters are left to it while focused. */

/* ---- one modal focus contract ------------------------------------------
 *
 * A modal is more than a visible scrim. It remembers the hand that opened
 * it, names the control that receives the keyboard, owns both ends of Tab
 * traversal, gives Escape its existing cancel meaning, and hands focus back
 * when it leaves. Keeping any one of those beside an individual dialog is how
 * the Jira card acquired a trap while the worktree form and permission
 * prompts did not.
 *
 * This is also the inventory. Scrim-backed dialogs belong here, as does the
 * new-tab picker: it is a top-layer palette rather than a scrim, but it takes
 * the keyboard on the same terms. Status-bar details, context menus and the
 * coachmark tour are deliberately absent — they do not make the workbench
 * modal and retain their light-dismiss/menu contracts. */
const MODAL_SPECS = Object.freeze({
  "sftp-manager": { initial: "sftp-host", cancel: () => sidebarMenu.hidden ? closeSftpManager() : closeSidebarMenu() },
  "sftp-transfer-scrim": { cancel: () => sftpTransferCancel?.() },
  "sftp-input-scrim": { cancel: () => sftpPromptCancel?.() },
  "sftp-editor-scrim": { cancel: () => sftpEditor?.close?.() },
  "crash-sheet": { initial: "crash-close", cancel: () => closeCrashSheet() },
  "auto-editor-scrim": {
    initial: "auto-name",
    cancel: () => {
      const focused = document.activeElement;
      if (el("auto-form").contains(focused) && focused.matches("input, textarea, select")) {
        focused.blur();
      } else closeAutoForm();
    },
  },
  "action-hub-scrim": { initial: "action-hub-close", cancel: () => closeActionHub() },
  "qc-scrim": { initial: "qc-label", cancel: () => closeQuickCommandEditor() },
  "continue-scrim": { initial: "continue-agent", cancel: () => closeContinueDialog() },
  "qo-scrim": { initial: "qo-input", cancel: () => closePalette() },
  "wt-edit-scrim": { initial: "wt-edit-name", cancel: () => setWorktreeEditor() },
  "wt-evidence-scrim": { initial: "wt-evidence-refresh", cancel: () => closeWorktreeEvidence() },
  "ssh-cred-scrim": { initial: "ssh-cred-value", cancel: () => answerSshCredential(null) },
  "composer-scrim": { initial: "composer-template", cancel: () => closeComposer() },
  "peek-scrim": {
    initial: "key-sink",
    owns: (node) => node === keySink,
    cancel: () => closeBoardPeek(),
  },
  "share-usage-scrim": { initial: "share-usage-copy", cancel: () => closeShareUsage() },
  "commit-blocked-scrim": {
    initial: "commit-blocked-close",
    cancel: () => closeCommitBlockedDialog(),
  },
  "pr-new-scrim": { initial: "pr-new-title", cancel: () => closePullRequestForm() },
  "wt-new-scrim": { initial: "wt-spec", cancel: () => setWorktreeForm(false) },
  "addproj-scrim": { initial: "addproj-browse", cancel: () => setAddProject(false) },
  "pb-scrim": { initial: "pb-list", cancel: () => closePathBrowser([]) },
  "save-scrim": { initial: "save-keep", cancel: () => answerLosing("cancel") },
  "pin-scrim": { initial: "pin-cancel", cancel: () => answerPinned(false) },
  "wt-remove-scrim": { initial: "wt-remove-cancel", cancel: () => closeRemovalPrompt() },
  "wsclean-scrim": { initial: "wsclean-close", cancel: () => closeCleanup() },
  "scm-discard-scrim": { initial: "scm-discard-go", cancel: () => closeScmDiscard() },
  "wsclean-confirm-scrim": {
    initial: "wsclean-confirm-cancel",
    cancel: () => closeCleanupConfirm(false),
  },
  "ext-scrim": { initial: "ext-go", cancel: () => closeExternalWorktrees() },
  "ext-suppress-scrim": {
    initial: "ext-suppress-cancel",
    cancel: () => closeExternalSuppress(),
  },
  "ask-scrim": { initial: "ask-yes", cancel: () => closeAsk(null) },
  "coordinator-scrim": { initial: "coordinator-run", cancel: () => closeCoordinatorPanel() },
  "trust-scrim": { initial: "trust-skip", cancel: () => closeTrust(false) },
  "term-theme-import-scrim": {
    initial: () => el("term-theme-preview-list").querySelector("input")
      ?? el("term-theme-import-cancel"),
    cancel: () => closeTerminalThemeImport(),
  },
  "term-ghostty-import-scrim": {
    initial: "term-ghostty-import-cancel",
    cancel: () => closeGhosttyImport(),
  },
  "ssh-host-scrim": {
    initial: "ssh-host-label",
    cancel: () => {
      if (sshHostMutation === null) closeSshHostDialog();
    },
  },
  "remote-workspace-scrim": {
    initial: "remote-workspace-label",
    cancel: () => {
      if (remoteWorkspaceMutation === null) closeRemoteWorkspaceDialog();
    },
  },
  "remote-server-scrim": {
    initial: "remote-server-name",
    cancel: () => {
      if (remoteServerMutation === null) closeRemoteServerDialog();
    },
  },
  "ssh-target-scrim": {
    initial: "ssh-target-host",
    cancel: () => {
      if (sshTargetMutation === null) closeSshTargetDialog();
    },
  },
  "gh-item-scrim": { initial: "gh-item-shut", cancel: () => closeGithubItem() },
  "jira-item-scrim": { initial: "jira-item-shut", cancel: () => closeJiraItem() },
  "linear-item-scrim": { initial: "linear-item-shut", cancel: () => closeLinearItem() },
  "gl-item-scrim": { initial: "gl-item-shut", cancel: () => closeGitlabItem() },
  "jira-connect-scrim": {
    initial: "jira-site-url",
    // Escape historically dismissed without clearing a failed credential;
    // the Cancel button still owns the clearing path.
    cancel: () => dismissJiraConnectDialog(),
  },
  "jira-create-scrim": { initial: "jira-create-summary", cancel: () => closeJiraCreate() },
  "linear-connect-scrim": {
    initial: "linear-api-key",
    cancel: () => closeLinearConnectDialog(),
  },
  "linear-create-scrim": {
    initial: "linear-create-title-field",
    cancel: () => closeLinearCreate(),
  },
  "onb-scrim": { initial: "onb-next", cancel: () => requestOnboardingSkip() },
  "onb-skip-scrim": {
    initial: "onb-skip-no",
    cancel: () => closeOnboardingSkipConfirm(false),
  },
  "guide-scrim": { initial: "guide-close", cancel: () => closeSetupGuide() },
  "wall-scrim": { initial: "wall-next", cancel: () => closeFeatureWall() },
  "tip-scrim": { initial: "tip-go", cancel: () => closeTip(false) },
  // A permission prompt has allow/deny choices but no generic cancellation.
  // Escape is consumed so it cannot operate the workbench behind the prompt.
  "perm-scrim": { initial: () => el("perm-actions").querySelector("button"), cancel: null },
  "tab-create-pop": {
    initial: () => tcInput,
    cancel: () => closeTabCreate("escape"),
  },
});

const MODAL_TAB_STOP = [
  "a[href]",
  "area[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  "[contenteditable=true]",
  "[tabindex]:not([tabindex=\"-1\"])",
].join(",");

const modalStack = [];
const modalOpeners = new WeakMap();

function modalSpec(root) {
  const spec = MODAL_SPECS[root?.id];
  if (!spec) throw new Error(`unregistered modal: ${root?.id ?? "(missing)"}`);
  return spec;
}

function modalFocusReady(node) {
  return (
    node instanceof HTMLElement
    && node.isConnected
    && !node.matches(":disabled")
    && !node.closest("[hidden], [inert], [aria-hidden=\"true\"]")
    && getComputedStyle(node).visibility !== "hidden"
    && node.getClientRects().length > 0
  );
}

function modalTabStops(entry) {
  return [...entry.root.querySelectorAll(MODAL_TAB_STOP)].filter((node) => {
    if (!modalFocusReady(node) || node.tabIndex < 0) return false;
    if (node.matches('input[type="radio"][name]')) {
      const named = [...entry.root.querySelectorAll('input[type="radio"][name]')]
        .filter((radio) => radio.name === node.name && modalFocusReady(radio));
      const checked = named.find((radio) => radio.checked);
      if (checked && checked !== node) return false;
    }
    return true;
  });
}

function resolveModalTarget(entry, wanted = entry.initial ?? entry.spec.initial) {
  const candidate = typeof wanted === "function"
    ? wanted(entry)
    : typeof wanted === "string"
      ? el(wanted)
      : wanted;
  if (modalFocusReady(candidate)) return candidate;
  const first = modalTabStops(entry)[0];
  if (first) return first;
  const dialog = entry.root.matches('[role="dialog"]')
    ? entry.root
    : entry.root.querySelector('[role="dialog"]');
  if (!(dialog instanceof HTMLElement)) return null;
  // A busy dialog can temporarily disable every control. The dialog itself
  // becomes the non-tabbing emergency seat rather than sending focus behind
  // the scrim.
  if (!dialog.hasAttribute("tabindex")) dialog.tabIndex = -1;
  return modalFocusReady(dialog) ? dialog : null;
}

function focusModalEntry(entry) {
  const target = resolveModalTarget(entry);
  target?.focus({ preventScroll: true });
  return target;
}

function modalOwns(entry, node) {
  return node instanceof Node
    && (entry.root.contains(node) || entry.spec.owns?.(node) === true || entry.owns?.(node) === true);
}

function modalTop() {
  // Backend-driven teardown and old fixture cleanup can remove a surface
  // without walking its close button. A hidden layer cannot remain the stack
  // authority; prune it before the next keyboard decision.
  for (let index = modalStack.length - 1; index >= 0; index -= 1) {
    if (!modalStack[index].root.hidden) continue;
    const [stale] = modalStack.splice(index, 1);
    modalOpeners.delete(stale.root);
  }
  return modalStack.at(-1) ?? null;
}

function modalOpener(root, offered) {
  const candidate = offered instanceof HTMLElement ? offered : document.activeElement;
  if (
    candidate instanceof HTMLElement
    && candidate.isConnected
    && !root.contains(candidate)
    && candidate !== document.body
    && candidate !== document.documentElement
  ) return candidate;
  const remembered = modalOpeners.get(root);
  return remembered?.isConnected ? remembered : null;
}

function showModal(root, options = {}) {
  const spec = modalSpec(root);
  if (options.animated) showing(root);
  else root.hidden = false;

  const existing = modalStack.findIndex((entry) => entry.root === root);
  let entry;
  if (existing >= 0) {
    entry = modalStack[existing];
    entry.initial = options.initial ?? entry.initial;
    entry.fallback = options.fallback ?? entry.fallback;
    entry.owns = options.owns ?? entry.owns;
  } else {
    const opener = modalOpener(root, options.opener);
    entry = {
      root,
      spec,
      opener,
      initial: options.initial,
      fallback: options.fallback,
      owns: options.owns,
    };
    if (opener) modalOpeners.set(root, opener);
    modalStack.push(entry);
  }
  // Repainting a modal already underneath a newer one does not make it the
  // top layer. Only a genuinely new entry establishes stack order.
  if (existing >= 0 && existing !== modalStack.length - 1) return entry;
  const focused = document.activeElement;
  if (!modalOwns(entry, focused) || !modalFocusReady(focused)) focusModalEntry(entry);
  return entry;
}

function modalFallbackFocus() {
  for (const id of ["settings-view", "task-view", "auto-view", "space-view"]) {
    const surface = document.getElementById(id);
    if (!surface || surface.hidden) continue;
    const target = [...surface.querySelectorAll(MODAL_TAB_STOP)].find(modalFocusReady);
    if (target) return target;
  }
  if (modalFocusReady(keySink)) return keySink;
  return modalFocusReady(el("foot-settings")) ? el("foot-settings") : null;
}

function restoreModalFocus(entry) {
  const top = modalTop();
  if (top) {
    const focused = document.activeElement;
    if (modalOwns(top, focused) && modalFocusReady(focused)) return;
    if (modalOwns(top, entry.opener) && modalFocusReady(entry.opener)) {
      entry.opener.focus({ preventScroll: true });
      return;
    }
    focusModalEntry(top);
    return;
  }
  let target = modalFocusReady(entry.opener) ? entry.opener : null;
  if (!target) {
    const fallback = typeof entry.fallback === "function"
      ? entry.fallback(entry)
      : entry.fallback;
    if (modalFocusReady(fallback)) target = fallback;
  }
  target ??= modalFallbackFocus();
  target?.focus({ preventScroll: true });
}

function hideModal(root, options = {}) {
  modalSpec(root);
  const at = modalStack.findIndex((entry) => entry.root === root);
  const wasTop = at >= 0 && at === modalStack.length - 1;
  const entry = at >= 0 ? modalStack.splice(at, 1)[0] : null;
  // Restoration happens as the close begins. A picker commonly opens its
  // chosen destination immediately after this call; restoring after the exit
  // animation would steal focus back from that new destination 150ms later.
  if (entry && wasTop) restoreModalFocus(entry);
  const finish = () => {
    options.after?.();
    if (!modalStack.some((held) => held.root === root)) modalOpeners.delete(root);
  };
  if (options.animated && !root.hidden) closing(root, finish);
  else {
    root.hidden = true;
    finish();
  }
}

function handleModalKeydown(event) {
  const entry = modalTop();
  if (!entry || event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key === "Escape") {
    event.preventDefault();
    // The old window-wide Escape ladder must not also cancel a surface below
    // this one. The top stack entry is the whole ordering rule.
    event.stopImmediatePropagation();
    entry.spec.cancel?.();
    return;
  }
  if (event.key !== "Tab" || event.metaKey || event.ctrlKey || event.altKey) return;
  const stops = modalTabStops(entry);
  if (stops.length === 0) {
    event.preventDefault();
    focusModalEntry(entry);
    return;
  }
  const focused = document.activeElement;
  const first = stops[0];
  const last = stops.at(-1);
  const atStop = stops.includes(focused);
  if (event.shiftKey && (focused === first || !atStop)) {
    event.preventDefault();
    last.focus({ preventScroll: true });
  } else if (!event.shiftKey && (focused === last || !atStop)) {
    event.preventDefault();
    first.focus({ preventScroll: true });
  }
}

// Window capture also sees synthetic window-targeted keys used by the native
// command bridge; document capture would miss those while seeing real keys.
window.addEventListener("keydown", handleModalKeydown, true);

// A narrow test/diagnostic seam: ids reveal ordering without exposing opener
// nodes or callbacks from the manager.
window.__MODAL_STACK__ = () => modalStack.map((entry) => entry.root.id);
window.__MODAL_INVENTORY__ = () => Object.keys(MODAL_SPECS);

/* Where the keyboard goes, asked once.
 *
 * tmux's rule, and now three surfaces can claim it: the floating shell wins
 * while it is up because it is drawn over everything else, then whichever
 * terminal tab is staged, then the lane. Answered in one place so text, keys
 * and paste cannot disagree about which process is being typed into — they
 * did, before terminals could be tabs. */
/* Typing anywhere in the window types into the staged terminal.
 *
 * The sink is where keys are composed, and every path that hands it focus is
 * one more thing that can be wrong — clicking the stage, closing a dialog,
 * the window regaining focus. This is the floor under all of them: a key
 * pressed while nothing is being edited belongs to the terminal on screen.
 *
 * Fields are left alone by name rather than by guessing: an input, a
 * textarea, a select or anything editable is somebody typing into that, and
 * the sink is itself a textarea, so it is already excluded. */
function rearmKeySink(event) {
  if (document.activeElement === keySink) return;
  if (modalTop()) return;
  // A primary chord arrives as two keydowns: the modifier first, then its
  // key. Keep the control that owned focus through both. Resolving only the
  // completed chord here is too late — the modifier-only event would already
  // have moved focus to the terminal before Settings could remember its
  // opener.
  if (hasPrimaryModifier(event)) return;
  // A screen is covering the terminal, so the sink is not what these keys
  // are for — pulling focus back to it would take the arrow keys away from
  // the screen the person is actually reading.
  if (!settingsView.hidden || !taskView.hidden || workspaceBoardOpen) return;
  // Where the key then goes is not this handler's business. The three
  // routing paths already ask the one function that answers that, and with
  // nothing staged they drop the key. Asking again here would put a fourth
  // opinion on a question that exists to have exactly one.
  const editing = document.activeElement?.closest(
    "input, textarea, select, [contenteditable], [data-keyboard-owner]",
  );
  if (editing) return;
  keySink.focus();
}

document.addEventListener("keydown", rearmKeySink, true);

/* And at the moment the window itself comes back. `focus()` moves
 * `activeElement` synchronously but the platform attaches its input context a
 * beat later, so a sink focused BY the first keystroke composes only from the
 * second — the first jamo lands bare, and the input handler below can do no
 * better than drop it. Arming when the window regains focus spends that beat
 * before typing can begin instead of on the keystroke that needed it. */
window.addEventListener("focus", rearmKeySink);

function keyboardTarget() {
  // Drawn above every surface in this window, so it keeps its keys whatever
  // is open underneath it.
  if (!termFloat.hidden) return { kind: "term", term: FLOAT_TERM };
  // The peek borrows the live screen, and the keyboard follows the screen:
  // what you are looking at is what you are typing into.
  if (peekTerm !== null && !el("peek-scrim").hidden) return { kind: "term", term: peekTerm };
  // A screen that covers the stage has taken the terminal off the screen, and
  // a key sent to a shell nobody can see is a key nobody can take back. Both
  // of these cover it whole; the finder and the dialogs do not.
  if (!settingsView.hidden || !taskView.hidden || workspaceBoardOpen) return null;
  // The visible tab in the focused leaf is the only safe keyboard owner.
  // `activeTabId` can briefly name a background or cross-workspace tab while
  // the stage has already fallen back to what this leaf can actually draw.
  const staged = activeTabIn(focusedPane);
  // The pane the pointer last chose, not the shell the tab was opened with:
  // once a tab has been split, "the terminal in front of you" is one of
  // several and typing has to reach the one wearing the active mark.
  if (staged?.kind === "term") {
    const term = activePaneOf(staged);
    return term === null ? null : { kind: "term", term };
  }
  if (staged?.kind === "lane" && focusedId) return { kind: "lane", id: focusedId };
  return null;
}

function emulatorKeyboardTarget() {
  if (!termFloat.hidden || !settingsView.hidden || !taskView.hidden || workspaceBoardOpen)
    return null;
  const tab = currentTab();
  if (tab?.kind !== "emulator" || !tab.live || !tab.interactive) return null;
  return tab;
}

function terminalViewForTarget(target) {
  if (target === null) return null;
  return target.kind === "term" ? viewOfTerm(target.term) : stageView;
}

/* Input hides only the screen that received it. Program output is deliberately
 * absent from this road, and a captured async paste keeps the original target.
 * A mouse move is owned by `makeTermView`, which reveals that same screen. */
function hideTerminalPointerForInput(target) {
  if (termPrefs?.hide_mouse_while_typing !== true) return;
  terminalViewForTarget(target)?.host.classList.add("is-pointer-hidden");
}

/* How many keystrokes have gone to a pty — the caret's reason to hold still.
 *
 * `apply` rewinds the caret's blink when the cursor moves, so a letter never
 * lands in the blink's dark phase. But a cursor also moves under a program
 * that is printing, and the rewind's `getAnimations()` flushes style before
 * it answers — a forced style pass over the rows that frame had just dirtied,
 * on every frame of a stream nobody was typing under. So the rewind is owed
 * to a KEYSTROKE, not to a move: this serial steps when a key or a typed
 * text leaves for a pty, and each caret holds still once per step. A serial
 * rather than a clock, because the frame path then asks one integer compare
 * and never needs to know how long a blink is. */
let typedSerial = 0;

function noteTyped() {
  typedSerial += 1;
}

function routeText(text) {
  const emulator = emulatorKeyboardTarget();
  if (emulator) {
    const prefix = emulator.platform === "android" ? "android" : "ios";
    const target = emulator.platform === "android"
      ? { serial: emulator.udid }
      : { udid: emulator.udid };
    invoke(`${prefix}_text`, { ...target, text }).catch((error) =>
      reportEmulatorError(
        emulator,
        error,
        "emulator.inputFailed",
        true,
      )
    );
    return;
  }
  const target = keyboardTarget();
  if (target === null) return;
  hideTerminalPointerForInput(target);
  if (target.kind === "term") {
    noteTyped();
    invoke("term_text", { term: target.term, text, human: true }).catch(() => {});
  } else {
    invoke("text_input", { id: target.id, text }).catch(() => {});
  }
}

function routeCommittedEnter() {
  const emulator = emulatorKeyboardTarget();
  if (emulator) {
    const prefix = emulator.platform === "android" ? "android" : "ios";
    const target = emulator.platform === "android"
      ? { serial: emulator.udid }
      : { udid: emulator.udid };
    invoke(`${prefix}_button`, { ...target, name: "enter" }).catch((error) =>
      reportEmulatorError(
        emulator,
        error,
        "emulator.inputFailed",
        true,
      )
    );
    return;
  }
  const target = keyboardTarget();
  if (target !== null) routeKeyPress(target, { key: "Enter", ctrl: false, alt: false });
}

/* One door for a named key, wherever it was decided: the document router and
 * the composition-commit Enter both speak through it, so the two can never
 * come to route a key differently. */
function routeKeyPress(target, press) {
  hideTerminalPointerForInput(target);
  if (target.kind === "term") {
    noteTyped();
    invoke("term_key", { term: target.term, press }).catch(() => {});
  } else {
    invoke("key_input", { id: target.id, press }).catch(() => {});
  }
}

/* Which key was physically pressed, independent of the layout on top of it.
 *
 * `event.key` is what the layout produces, and under a Korean input source
 * that is a jamo — ⌘N arrives as "ㅜ", so matching on it loses every shortcut
 * in this window for the person the product is being built for. `event.code`
 * is defined to be layout independent. It is read only for the keys a chord
 * can be, so a layout where the letter rather than its position is what was
 * pressed still works. */
const PHYSICAL_PRINTABLE_KEYS = Object.freeze({
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Space: " ",
});

function physicalPrintableKey(event) {
  if (event.code.startsWith("Key")) return event.code.slice(3).toLowerCase();
  if (event.code.startsWith("Digit")) return event.code.slice(5);
  return PHYSICAL_PRINTABLE_KEYS[event.code] ?? null;
}

function keyId(event) {
  return physicalPrintableKey(event) ?? event.key.toLowerCase();
}

/* macOS Option is two different things: a text-composition modifier and a
 * terminal Alt/Meta modifier. Orca lets each physical side be chosen, so the
 * keydown carrying the printable character is not enough — its `location`
 * belongs to that character, not to the Option key. Remember the modifier's
 * own down/up events and clear the set when the window loses focus. */
const heldMacOptionLocations = new Set();

document.addEventListener("keydown", (event) => {
  if (!usesCommandModifier || event.key !== "Alt") return;
  if (event.location === 1 || event.location === 2) {
    heldMacOptionLocations.add(event.location);
  }
}, true);

document.addEventListener("keyup", (event) => {
  if (!usesCommandModifier || event.key !== "Alt") return;
  heldMacOptionLocations.delete(event.location);
}, true);

window.addEventListener("blur", () => heldMacOptionLocations.clear());

function macOptionSendsAlt(event) {
  if (!usesCommandModifier || !event.altKey) return event.altKey;
  // Orca preserves the three readline word-editing gestures even while the
  // rest of Option is composing text.
  if (
    !event.ctrlKey && !event.metaKey && !event.shiftKey &&
    ["KeyB", "KeyF", "KeyD"].includes(event.code)
  ) return true;
  const mode = termPrefs?.mac_option_as_alt ?? "auto";
  if (mode === "true") return true;
  if (mode === "left") return heldMacOptionLocations.has(1);
  if (mode === "right") return heldMacOptionLocations.has(2);
  if (mode === "false") return false;
  return terminalKeyboardLayoutCategory === "us";
}

/* A control chord is a POSITION, not a glyph.
 *
 * macOS resolves them positionally — measured by Orca with `UCKeyTranslate`,
 * physical C/A/U under Control produce U+0003/U+0001/U+0015 on 2SetHangul,
 * Russian and Greek exactly as they do on ABC, though the same keys unmodified
 * produce U+314A / U+3141 / U+03C8 (terminal-non-latin-control-chord.ts:1-17).
 * The browser does not expose that translation: `event.key` carries the
 * layout's glyph, so Ctrl+C reached the encoder as a jamo, which has no C0
 * byte, and what went to the shell was the jamo. An interrupt that interrupted
 * nothing, for everyone whose input source is not Latin.
 *
 * A Latin `key` stays authoritative — a layout that MOVES letters (Dvorak)
 * reports a real letter and means it (`isLatinLetterKey`, ibid). Shift is
 * excluded for the reason the original excludes it: Ctrl+Shift chords have
 * their own encoding. */
function isNonLatinControlChord(event) {
  return event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey &&
    Array.from(event.key).length === 1 && event.key.codePointAt(0) > 0x7f;
}

function terminalKeyForEvent(event, optionSendsAlt) {
  if (isNonLatinControlChord(event)) return physicalPrintableKey(event) ?? event.key;
  if (!usesCommandModifier || !event.altKey || !optionSendsAlt) return event.key;
  // On macOS `event.key` is already the composed glyph (`⌥B` is `∫`). When
  // Option means terminal Alt, send the unmodified physical key instead.
  return physicalPrintableKey(event) ?? event.key;
}

function isPlainPhysicalJisYenKey(event) {
  return event.keyCode !== 229 && event.code === "IntlYen" && event.key === "¥" &&
    !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
}

function remapsJisYenToBackslash(event) {
  return usesCommandModifier && termPrefs?.jis_yen_to_backslash === true &&
    isPlainPhysicalJisYenKey(event);
}

// The keydown owner below sends the one replacement byte. Keep later browser
// keyboard phases quiet as Orca does, so a future keypress/input listener
// cannot re-introduce the original ¥ beside it.
for (const eventName of ["keypress", "keyup"]) {
  window.addEventListener(eventName, (event) => {
    if (!remapsJisYenToBackslash(event) || keyboardTarget() === null) return;
    const active = document.activeElement;
    if (active === keySink || active === document.body) event.preventDefault();
  }, { capture: true });
}

/* One canonical string per chord. `mod` is deliberately platform-neutral in
 * storage; `hasPrimaryModifier` decides which physical key produces it. */
function chordOf(event) {
  const parts = ["mod"];
  if (event.altKey) parts.push("alt");
  if (event.shiftKey) parts.push("shift");
  parts.push(keyId(event));
  return parts.join("+");
}

function closeLane(id) {
  invoke("close_lane", { id }).catch(showError).finally(refreshWorktrees);
}

/* Keybindings as data, not as branches.
 *
 * Orca keeps around seventy of these in one table — an id a settings panel
 * names the action by, a title it shows, and the chord it *defaults* to,
 * with a user file free to override the chord (measured:
 * `KEYBINDING_DEFINITIONS`, docs/reverse/orca-ui-inventory.md 1-d). The same
 * shape at our size, so that panel has something to read when it lands and
 * the dispatcher never grows a branch per shortcut. */
const ACTIONS = [
  { id: "tree.undo", group: "global", title: () => t("tree.undo", "되돌리기"), chord: "mod+z",
    when: () => explorerOwnsShortcut(), run: () => void runTreeHistory("fs_undo") },
  { id: "tree.redo", group: "global", title: () => t("tree.redo", "다시 실행"), chord: "mod+shift+z",
    when: () => explorerOwnsShortcut(), run: () => void runTreeHistory("fs_redo") },
  { id: "lane.new", group: "agents",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "tab", "agent", "new", "launch"], title: () => t("session.new", "새 스레드"), chord: "mod+n", run: () => openLane() },
  {
    // What ⌘W means everywhere: close the tab in front of you. On the lane's
    // tab that is the lane, because the tab is the lane's only surface.
    id: "tab.close", group: "tabs", allowInTerminal: true,
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "close", "tab", "pane"],
    title: () => t("tab.close", "탭 닫기"),
    chord: "mod+w",
    run: () => {
      // A split terminal answers first.
      //
      // Orca files this as a second action (`terminal.closePane`) whose
      // default chord is also ⌘W, and it is worth being exact about what
      // decides between them, because it is NOT which listener runs first.
      // The app-level handler steps aside on its own: it returns when the
      // active tab is a terminal and the focus is inside one
      // (`activeTabType === "terminal" && context === "terminal"`,
      // Terminal-C-wPg97C.js:4101, where `context` is read off the focused
      // element). And it could not close a terminal tab in any case — its
      // only two branches are the editor and the browser (:4104-4105). So
      // terminal tabs are closed exclusively through the pane path, which
      // closes the TAB once the pane it took was the last one
      // (`executeClosePane`, OnboardingInlineCommandTerminal-Caul4Dbw.js
      // :27922).
      //
      // This window has one keydown handler and no context axis, so the same
      // outcome is reached by asking the pane first. With one pane left there
      // is nothing to close but the tab, and the chord means what it always
      // meant.
      if (closeActivePane()) return;
      const active = currentTab();
      if (active) closeTab(active.id);
    },
  },
  {
    id: "tab.reopen", group: "tabs",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "tab", "reopen", "restore", "closed"],
    title: () => t("tab.reopen", "닫은 탭 다시 열기"),
    chord: "mod+shift+t",
    run: () => reopenClosedTab(),
  },
  {
    // What ⌘S means everywhere. Only the file surface has anything to write:
    // a terminal's contents are the program's, not this window's.
    id: "file.save", group: "editors",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "editor", "save"],
    title: () => t("file.saveAction", "저장"),
    chord: "mod+s",
    run: () => saveFile(currentTab()),
  },
  {
    // Orca binds ⌘F to search; ⌘⇧F here already means "search the project",
    // so this is the narrower one — search the file in front of you.
    id: "file.find", group: "editors", allowInTerminal: true,
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "editor", "find", "search"],
    title: () => t("file.find", "파일에서 찾기"),
    chord: "mod+f",
    run: () => openFind(),
  },
  {
    // Orca's own pair for walking matches (`matchSearchNavigate`,
    // terminal-shortcut-policy-BK9MulHK.js:1230), and the chords every editor
    // on this platform uses. They step whichever finder is open — the file's
    // or the terminal's — because to a person they are the same gesture, and
    // two bindings that differ only in which surface is in front would be two
    // things to learn for one idea.
    id: "find.next", group: "editors", allowInTerminal: true,
    keywords: ["shortcut", "editor", "find", "search", "next"],
    title: () => t("file.findNext", "다음 일치"),
    chord: "mod+g",
    run: () => stepFind(1),
  },
  {
    // NO DEFAULT CHORD, and that is a decision rather than an omission.
    //
    // Orca binds ⇧⌘G to this (`matchSearchNavigate`). In this window ⇧⌘G is
    // already source control — VS Code's binding, shipped, discoverable, and
    // in somebody's fingers. Taking it for a finder step would break a working
    // control to add a second way to do something the bar's own ↑ and
    // Shift+Enter already do. The action exists, appears on the shortcuts
    // screen, and can be bound to ⇧⌘G by anyone who wants Orca's arrangement —
    // which is what a remappable action registry is FOR.
    id: "find.prev", group: "editors", allowInTerminal: true,
    keywords: ["shortcut", "editor", "find", "search", "previous"],
    title: () => t("file.findPrev", "이전 일치"),
    chord: null,
    run: () => stepFind(-1),
  },
  {
    id: "file.quickOpen", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "global", "file", "quick open"],
    title: () => t("file.goto", "파일로 이동"),
    chord: "mod+p",
    run: () => togglePalette("file"),
  },
  {
    id: "worktree.jump", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "global", "worktree", "switch", "jump"],
    title: () => t("worktree.jump", "워크트리로 이동"),
    // Orca's own binding for this, and the reason it is not ⌘⇧P: there is no
    // general command palette to compete with (measured, 1-d).
    chord: "mod+j",
    // The same refusal `nav-tasks` draws, so the chord cannot reach a finder
    // the row beside it says is closed. `el(...).disabled` is the one place
    // that fact already lives — asked here rather than duplicated.
    run: () => {
      if (!el("nav-tasks").disabled) togglePalette("worktree");
    },
  },
  {
    id: "terminal.toggle", group: "global",
    // Orca's own, copied: `searchKeywords` and `allowInTerminal` on the
    // definition this mirrors (`floatingTerminal.toggle`,
    // `plugin-manifest-Bupg0PSo.js:4746` — a toggle you cannot press while
    // a terminal has the keyboard is a toggle for the panel you are not in).
    allowInTerminal: true,
    keywords: ["shortcut", "floating terminal", "terminal"],
    title: () => t("terminal.label", "터미널"),
    // Orca's measured default first (`floatingTerminal.toggle`,
    // `platformBindings(["Mod+Alt+A"])`), the ⌘` this window already taught
    // people second — the label shows the first, both keep working.
    chord: ["mod+alt+a", "mod+`"],
    // Orca's toggle claims nothing while the feature is off — the doors are
    // hidden and the chord must not stay behind as a secret one (the same
    // `if (enabled)` its own handler wears, use-floating-workspace-panel.ts
    // :113-117). The panel cannot be open here: disabling closed it.
    run: () => {
      if (floatingWorkspacePrefs.enabled) setTermVisible(termFloat.hidden);
    },
  },
  {
    // Orca's own binding, and its own meaning: the centre opens a terminal as
    // a tab, with a shell of its own (docs/reverse/orca-ui-inventory.md 1-d).
    id: "terminal.newTab", group: "tabs",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "tab", "terminal", "new"],
    title: () => t("terminal.newTab", "새 터미널 탭"),
    chord: "mod+t",
    run: () => openTermTab({ door: "terminal" }),
  },
  {
    // The other half of the original's pair, and the reason ⌘T could stop
    // launching agents without taking the keyboard door away with it. The
    // original's own title for it names what it opens: "New agent tab
    // (default agent)" (`shared/keybindings.ts:548-559`).
    id: "terminal.newAgentTab", group: "tabs",
    keywords: ["shortcut", "tab", "agent", "new", "default", "launch"],
    title: () => t("terminal.newAgentTab", "새 에이전트 탭"),
    chord: darwinOnly("mod+alt+t"),
    run: () => void launchNewAgentTab(),
  },
  {
    id: "worktree.create", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "global", "worktree", "create", "new workspace"],
    title: () => t("worktree.create", "새 워크트리"),
    // Orca binds Mod+Shift+N to the same thing; Mod+N is a thread here.
    chord: "mod+shift+n",
    run: () => setWorktreeForm(wtNewScrim.hidden),
  },
  {
    id: "view.toggleSidebar", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "sidebar", "left"],
    title: () => t("view.sidebar", "스레드 패널"),
    chord: "mod+b",
    run: () => setPanelFolded("sidebar", !folded.sidebar),
  },
  {
    id: "view.toggleAside", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "sidebar", "right"],
    title: () => t("view.aside", "파일 패널"),
    chord: "mod+l",
    // Orca's ⌘L is TWO actions split by scope: the browser's address bar
    // when a browser tab is in front (`browser.focusAddressBar`,
    // plugin-manifest), the panel otherwise. No scope axis here, so the run
    // asks — the tab.close idiom.
    run: () => {
      // The browser meaning only while the browser is actually LOOKABLE —
      // focusing an address bar under a scrim types into a hidden field.
      if (browserChordScope()) {
        const front = currentTab();
        docHost(front.pane, "browser").querySelector(".browser-address")?.focus();
        return;
      }
      setPanelFolded("aside", !folded.aside);
    },
  },
  {
    // Orca's Browser group, `scope: "browser"` — `when` is the scope: the
    // chord acts only on a browser tab that is in FRONT and UNCOVERED, and
    // otherwise the key falls through to whoever owns it. A page hidden
    // under a dialog reloading on ⌘R was the review's finding 2.
    id: "browser.reload", group: "browser",
    keywords: ["shortcut", "browser", "reload", "refresh"],
    title: () => t("browser.reloadAction", "브라우저 새로고침"),
    chord: "mod+r",
    when: browserChordScope,
    run: () => invoke("browser_reload", { label: currentTab().label }).catch(() => {}),
  },
  {
    // The chord moved up to the zoom router (`zoom.in` below), which still
    // hands a front-and-uncovered browser its page zoom first — this row
    // stays a palette door to the same step.
    id: "browser.zoomIn", group: "browser",
    keywords: ["shortcut", "browser", "zoom", "in"],
    title: () => t("browser.zoomIn", "브라우저 확대"),
    chord: null,
    when: browserChordScope,
    run: () => browserZoomStep(currentTab(), 1),
  },
  {
    id: "browser.zoomOut", group: "browser",
    keywords: ["shortcut", "browser", "zoom", "out"],
    title: () => t("browser.zoomOut", "브라우저 축소"),
    chord: null,
    when: browserChordScope,
    run: () => browserZoomStep(currentTab(), -1),
  },
  {
    id: "browser.zoomReset", group: "browser",
    keywords: ["shortcut", "browser", "zoom", "reset"],
    title: () => t("browser.zoomReset", "브라우저 실제 크기"),
    chord: null,
    when: browserChordScope,
    run: () => browserZoomStep(currentTab(), 0),
  },
  {
    // One chord, four domains — Orca's resolveZoomTarget on its own menu
    // accelerators (resolve-zoom-target.ts:33-56). `allowInTerminal` because
    // the terminal domain IS the terminal owning the keyboard: under
    // terminal-first the chord must still arrive here to reach the pane's
    // own font (useTerminalFontZoom.ts — "terminal zoom is focus-owned").
    id: "zoom.in", group: "global", allowInTerminal: true,
    keywords: ["shortcut", "zoom", "in", "font", "bigger"],
    title: () => t("zoom.in", "확대"),
    chord: "mod+=",
    run: () => routeZoom("in"),
  },
  {
    id: "zoom.out", group: "global", allowInTerminal: true,
    keywords: ["shortcut", "zoom", "out", "font", "smaller"],
    title: () => t("zoom.out", "축소"),
    chord: "mod+-",
    run: () => routeZoom("out"),
  },
  {
    id: "zoom.reset", group: "global", allowInTerminal: true,
    keywords: ["shortcut", "zoom", "reset", "font"],
    title: () => t("zoom.reset", "실제 크기"),
    chord: "mod+0",
    run: () => routeZoom("reset"),
  },
  {
    id: "activity.files", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "sidebar", "explorer", "files"],
    title: () => t("view.files", "파일 탐색기"),
    chord: "mod+shift+e",
    run: () => revealActivity("files", null),
  },
  {
    id: "activity.search", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "sidebar", "search"],
    title: () => t("view.search", "내용 검색"),
    chord: "mod+shift+f",
    run: () => revealActivity("files", "text"),
  },
  {
    id: "activity.scm", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "sidebar", "source control", "git"],
    title: () => t("view.scm", "소스 컨트롤"),
    chord: "mod+shift+g",
    run: () => revealActivity("scm", null),
  },
  {
    id: "project.open", group: "global",
    title: () => t("sidebar.openFolder", "다른 폴더 열기"),
    // Ours rather than measured: Orca's table has no entry for this, but ⌘O
    // means open in every editor a person has used, and its absence would be
    // the surprise.
    chord: "mod+o",
    run: () => openAnotherProject(),
  },
  {
    id: "app.settings", group: "global",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js`).
    keywords: ["shortcut", "settings", "preferences"],
    title: () => t("app.settings", "설정"),
    chord: "mod+,",
    run: () => setSettingsOpen(settingsView.hidden),
  },
  {
    id: "terminal.splitRight", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js:5676`).
    keywords: ["shortcut", "pane", "split", "right"],
    title: () => t("terminal.splitRight", "터미널 오른쪽으로 분할"),
    // Measured, macOS row: `{darwin: ["Mod+D"], linux: ["Mod+Shift+D"], …}`.
    // The other platforms' rows differ from each other and from this one; we
    // ship the row for the platform this window runs on.
    chord: "mod+d",
    run: () => splitActivePane("vertical"),
  },
  {
    id: "terminal.splitDown", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js:5694`).
    keywords: ["shortcut", "pane", "split", "down"],
    title: () => t("terminal.splitDown", "터미널 아래로 분할"),
    chord: "mod+shift+d",
    run: () => splitActivePane("horizontal"),
  },
  {
    id: "terminal.focusNextPane", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js:5582`).
    keywords: ["shortcut", "pane", "focus", "next"],
    title: () => t("terminal.focusNextPane", "다음 페인"),
    chord: "mod+]",
    run: () => focusPane(1),
  },
  {
    id: "terminal.focusPreviousPane", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (`plugin-manifest-Dq3wpxrr.js:5595`).
    keywords: ["shortcut", "pane", "focus", "previous"],
    title: () => t("terminal.focusPreviousPane", "이전 페인"),
    chord: "mod+[",
    run: () => focusPane(-1),
  },
  {
    id: "terminal.expandPane", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (I18nProvider-4EBrmTGg.js:80967-80974).
    keywords: ["shortcut", "pane", "expand", "collapse"],
    title: () => t("terminal.expandPane", "페인 펼치기"),
    // Measured: `Mod+Shift+Enter`, every platform's row (:80973).
    chord: "mod+shift+enter",
    run: () => {
      const tab = currentTab();
      if (tab?.kind === "term") togglePaneExpanded(tab, activePaneOf(tab));
    },
  },
  {
    id: "terminal.equalizePaneSizes", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (I18nProvider-4EBrmTGg.js:80959-80966).
    keywords: ["shortcut", "pane", "split", "equalize", "resize", "balance", "size"],
    title: () => t("terminal.equalize", "페인 크기 균등"),
    // Orca ships this unbound (`platformBindings([])`, :80965) — the row
    // exists so somebody can GIVE it a key, and so search finds the verb.
    chord: null,
    run: () => equalizeActivePanes(currentTab()),
  },
  {
    id: "terminal.setTitle", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (I18nProvider-4EBrmTGg.js:80975-80982).
    keywords: ["shortcut", "terminal", "pane", "set title", "title", "rename"],
    title: () => t("terminal.setTitle", "이름 지정…"),
    chord: null,
    run: () => {
      const tab = currentTab();
      if (tab?.kind === "term") startPaneRename(tab, activePaneOf(tab));
    },
  },
  {
    id: "terminal.clearPaneTitle", group: "panes", scope: "terminal",
    // Orca's own, copied: `searchKeywords` on the definition
    // this mirrors (I18nProvider-4EBrmTGg.js:80983-80990).
    keywords: ["shortcut", "terminal", "pane", "clear title", "remove title", "title"],
    title: () => t("terminal.clearPaneTitle", "페인 이름 지우기"),
    chord: null,
    run: () => {
      const tab = currentTab();
      if (tab?.kind === "term") clearPaneTitle(tab, activePaneOf(tab));
    },
  },
];

/* The chords in force, which is the defaults with the person's own on top.
 *
 * Orca keeps only the overrides — its store starts at `EMPTY_KEYBINDINGS = {}`
 * and holds `snapshot.overrides` (`store-BgJxB0hr.js:33313-33341`) — so a
 * chord nobody moved has nothing stored for it, and a default we improve later
 * reaches everybody who never moved it. An action can carry SEVERAL chords, and
 * an empty list is the measured way to say "this action has no key at all".
 *
 * Rebuilt rather than frozen: `BOUND` used to be a `const` built once from the
 * defaults, which is the whole reason the settings panel could only ever
 * display them. */
let keybindingOverrides = {};
let BOUND = new Map();

function chordsFor(action) {
  // Some actions ship with no key at all — Orca does the same
  // (`platformBindings([])` on equalize and the title pair) — and their
  // default is the empty list, not a list holding nothing.
  // A default can also be SEVERAL keys: Orca's `platformBindings` takes a
  // list, and the floating terminal keeps its measured ⌘⌥A beside the ⌘`
  // this window already taught people.
  const fallback = Array.isArray(action.chord) ? action.chord : action.chord ? [action.chord] : [];
  const moved = keybindingOverrides[action.id];
  if (!Array.isArray(moved)) return fallback;
  // Orca canonicalises an override before resolving it — junk is dropped and
  // duplicates collapse (`getEffectiveKeybindingsForAction`,
  // `plugin-manifest-Dq3wpxrr.js:6173-6181`).
  const kept = [];
  for (const chord of moved) {
    if (typeof chord === "string" && chord !== "" && !kept.includes(chord)) kept.push(chord);
  }
  // An empty list is an instruction; a list that was ONLY junk is a file
  // somebody edited, and the safe reading of that is the default rather than
  // an action silently bound to nothing.
  return kept.length === 0 && moved.length > 0 ? fallback : kept;
}

function chordsForId(actionId) {
  const action = ACTIONS.find((candidate) => candidate.id === actionId);
  return action ? chordsFor(action) : [];
}

function rebuildBound() {
  BOUND = new Map();
  // Two passes, so a chord the PERSON moved always beats a chord an update
  // shipped (1-g0 리뷰 발견 4): one pass made the winner whoever sat later
  // in the table — a new default could silently swallow a years-old
  // override, and which one won depended on declaration order.
  for (const action of ACTIONS) {
    if (Array.isArray(keybindingOverrides[action.id])) continue;
    for (const chord of chordsFor(action)) BOUND.set(chord, action);
  }
  for (const action of ACTIONS) {
    if (!Array.isArray(keybindingOverrides[action.id])) continue;
    for (const chord of chordsFor(action)) BOUND.set(chord, action);
  }
}

/* ⌘1…⌘9 stage the worktree at that place in the sidebar.
 *
 * Orca's own meaning, measured: `workspace.selectByIndex` — "Select Workspace
 * 1–9", bound to `Mod+1` with the note that one row covers the whole range
 * (`I18nProvider-*.js`). A workspace there is a worktree, which is what the
 * digits address here too.
 *
 * They used to stage a lane by rail slot. The inventory recorded that as a
 * divergence to resolve "when the tab host is built" — it is built now, and
 * lanes have their own way in: they are rows under the worktree that owns
 * them, and the tab strip stages whichever one you pick.
 *
 * A rule rather than nine entries: the index IS the digit, so a table would
 * only be repeating itself. */
function digitIndex(chord) {
  const digit = chord.slice("mod+".length);
  return digit.length === 1 && digit >= "1" && digit <= "9" ? Number(digit) : null;
}

/* Resolve once so the capture-phase focus guard and the shortcut router agree
 * about whether a primary-modifier key belongs to the application. */
function primaryShortcut(event) {
  if (!hasPrimaryModifier(event) || isPopout) return null;
  const chord = chordOf(event);
  const action = BOUND.get(chord);
  const terminalFirst = terminalShortcutPolicy === "terminal-first" &&
    terminalOwnsShortcutContext();
  if (
    action && (!action.when || action.when()) &&
    (!terminalFirst || shortcutRunsInTerminal(action))
  ) return { action, path: null };
  if (terminalFirst) return null;
  const index = digitIndex(chord);
  const path = index === null ? null : worktreeOrder[index - 1];
  return path ? { action: null, path } : null;
}

function terminalOwnsShortcutContext() {
  const active = document.activeElement;
  return keyboardTarget() !== null && (active === keySink || active === document.body);
}

function shortcutRunsInTerminal(action) {
  return action?.scope === "terminal" || action?.allowInTerminal === true;
}

function isCtrlTabSwitcherChord(event) {
  return event.key === "Tab" && event.ctrlKey && !event.metaKey && !event.altKey;
}

function consumeTabSwitcherEvent(event) {
  event.preventDefault();
  event.stopPropagation();
}

window.addEventListener("keydown", (event) => {
  if (isCtrlTabSwitcherChord(event)) {
    if (
      terminalShortcutPolicy === "terminal-first" &&
      terminalOwnsShortcutContext()
    ) return;
    if (openOrAdvanceTabSwitcher(event.shiftKey ? -1 : 1)) {
      consumeTabSwitcherEvent(event);
    }
    return;
  }
  if (tabSwitcher && event.key === "Escape") {
    consumeTabSwitcherEvent(event);
    cancelTabSwitcher();
  }
}, { capture: true });

window.addEventListener("keyup", (event) => {
  if (!tabSwitcher || event.key !== "Control") return;
  consumeTabSwitcherEvent(event);
  commitTabSwitcher();
}, { capture: true });

window.addEventListener("blur", cancelTabSwitcher);

window.addEventListener("keydown", (event) => {
  if (hasPrimaryModifier(event)) {
    // 팝아웃에는 이 창의 것이 아닌 문이 없다 — 탭도, 워크스페이스도, 설정도
    // 메인 창의 것이다. 여기서 코드를 태우면 보이지 않는 탭 띠에 터미널이
    // 열리고, 그 프로세스는 아무도 볼 수 없는 곳에서 돌아간다.
    if (isPopout) return;
    const shortcut = primaryShortcut(event);
    // `when` is asked BEFORE the key is consumed (1-g0 리뷰 발견 2): an
    // out-of-scope chord must fall through untouched, not die as a no-op —
    // ⌘R on a file tab is not this window's to eat.
    if (shortcut?.action) {
      event.preventDefault();
      shortcut.action.run();
      return;
    }
    if (shortcut?.path) {
      event.preventDefault();
      activateWorktree(shortcut.path);
      return;
    }
    // Command is not terminal input on Apple platforms — except the clipboard
    // pair above. Asking before this return is the difference between a paste
    // and a native event the webview never emits. On Windows an unclaimed
    // Control chord is a real control character, so it continues below.
    if (usesCommandModifier) {
      const target = keyboardTarget();
      const active = document.activeElement;
      if (
        !event.defaultPrevented && target !== null &&
        (active === keySink || active === document.body) &&
        terminalClipboardShortcut(event, target)
      ) return;
      return;
    }
  }

  // The IME owns this keystroke, so the window must not touch it — and
  // `isComposing` alone does not say so. It is false on the keydown that
  // *starts* a composition, which is the first jamo of every Korean syllable;
  // letting that through reaches the `preventDefault` below and the IME never
  // receives the key at all. WebKit reports a key the input method is
  // handling as `Process`, and every engine still sets the legacy 229 for it.
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key === "Escape") {
    // 코치마크는 스크림이 없으므로 이 자리에 서야 한다: 아래 화면보다 앞이고,
    // 모달 스택보다 뒤다. 모달은 캡처 단계에서 이미 자기 취소 의미를 썼다.
    if (!el("tour").hidden) {
      event.preventDefault();
      closeTour();
      return;
    }
    if (!el("workspace-board-settings-pop").hidden) {
      event.preventDefault();
      closeWorkspaceBoardSettings();
      return;
    }
    if (workspaceBoardOpen) {
      event.preventDefault();
      setWorkspaceBoardOpen(false);
      return;
    }
    if (!taskView.hidden) {
      event.preventDefault();
      const focused = document.activeElement;
      if (taskView.contains(focused) && focused.matches("input, textarea, select")) {
        focused.blur();
        return;
      }
      setTaskOpen(false);
      return;
    }
    // The automation form is a layer inside its page, so one Escape puts the
    // list back and the next puts the page away. Written as two steps rather
    // than one so a half-typed schedule is never lost to a stray key.
    if (!autoView.hidden) {
      event.preventDefault();
      if (autoDraft) {
        const focused = document.activeElement;
        if (el("auto-form").contains(focused) && focused.matches("input, textarea, select")) {
          focused.blur();
        } else {
          closeAutoForm();
        }
      } else if (autoSelectedId) {
        autoSelectedId = null;
        autoRuns = [];
        paintAutomations();
        paintAutomationDetail();
      } else {
        setAutoOpen(false);
      }
      return;
    }
    if (!settingsView.hidden) {
      event.preventDefault();
      setSettingsOpen(false);
      return;
    }
    // 골라 둔 소스 컨트롤 행들 — 화면을 닫기 전에 선택부터 걷는다(원본
    // use-selection.ts의 Escape 층). 화면보다 앞인 이유는 선택이 더 가벼운
    // 상태이기 때문이다: 사람이 Escape를 누를 때 지우려는 것은 방금 만든
    // 그것이지, 그 밑에서 읽고 있던 문서가 아니다.
    if (scmSelected.size > 0) {
      event.preventDefault();
      clearScmSelection();
      return;
    }
    // Any surface that is READ rather than typed into: a file, a diff, a
    // notebook. `ESCAPE_CLOSES` is that family, by name.
    //
    // It used to be asked as "is anything listening" (`keyboardTarget() ===
    // null`), which is true of every kind but a terminal and a lane — so the
    // key closed browser panes, the board and the graph as readily as a file,
    // and a browser close takes the restore record with it (t-5453). Naming
    // the family is the same correction this road already took once, when the
    // test was `kind !== "lane"` and Escape closed terminals.
    //
    // Still only while nothing is listening: a document that HAS the keyboard
    // (a pane split beside it, the floating terminal over it) is a document
    // somebody is typing past, and `keyboardTarget()` is the window's own
    // answer to that — it already accounts for the float and for a screen
    // covering the stage.
    const reading = currentTab();
    if (reading && ESCAPE_CLOSES.has(reading.kind) && keyboardTarget() === null) {
      event.preventDefault();
      closeTab(reading.id);
      return;
    }
  }
  // Reading, not typing — a file or a diff keeps its keys on the page instead
  // of sending them to a terminal the person cannot even see. Asked as "is
  // anything listening" rather than as a list of surfaces that are not.
  const target = keyboardTarget();
  if (target === null) return;
  // Whatever has focus owns its keys, and the sink is the one thing that
  // means "the terminal". Asked that way round rather than as a list of
  // element types: the rows in this window are tab stops now, and a focused
  // row's Enter has to reach the row instead of being preventDefault'd into
  // a pty. `document.body` is what `activeElement` reports when nothing at
  // all is focused, which is the case the terminal should have.
  const active = document.activeElement;
  if (active !== keySink && active !== document.body) return;
  if (remapsJisYenToBackslash(event)) {
    event.preventDefault();
    routeText("\\");
    return;
  }
  const optionSendsAlt = macOptionSendsAlt(event);
  const optionComposes = usesCommandModifier && event.altKey && !optionSendsAlt;
  const plainText = event.key.length === 1 && !event.ctrlKey && !event.metaKey &&
    (!event.altKey || optionComposes);
  if (plainText && active === keySink) return; // IME path owns it
  // A key an earlier owner already spent — the rescue's Backspace editing a
  // half-composed syllable — must not ALSO reach the pty as itself.
  if (event.defaultPrevented) return;
  if (terminalClipboardShortcut(event, target)) return;
  event.preventDefault();
  // Hangul arriving HERE — past `rearmKeySink`, which pulls body-focused
  // typing back to the sink at capture so the insert lands there and the
  // input backstop rescues it — is a road nobody has mapped: a modifier
  // held over a jamo, a covering screen. Worth its line in the black box
  // before it goes out as a "key" the pty will spell as bare jamo.
  if (HANGUL_ANY.test(event.key)) traceIme(`key.body ${event.key}`);
  routeKeyPress(target, {
    key: terminalKeyForEvent(event, optionSendsAlt),
    ctrl: event.ctrlKey,
    alt: event.altKey && (!usesCommandModifier || optionSendsAlt),
    // Shift travels because a named key wearing it is a DIFFERENT key to the
    // program: Shift+Tab is `ESC[Z` and walks a TUI's fields backwards, which
    // is what codex binds it to. The encoder owns the spelling; this only has
    // to stop dropping the fact on the floor.
    shift: event.shiftKey,
  });
});

/* The platform clipboard modifier has two narrow terminal exceptions that the
 * terminal menu already promises. Copy needs a live selection owned by THIS
 * target; without one Ctrl+C stays on the encoder/SIGINT road and Command+C
 * stays the browser's ordinary no-selection copy. Paste captures the address
 * before the plugin read and enters through the sanitized backend paste door.
 *
 * One helper serves two call sites: Apple asks before the early "unclaimed
 * Command belongs to the browser" return, while non-Apple Control asks after
 * application shortcuts and falls through on a selection-free Ctrl+C. A
 * declaration is hoisted, so keeping it here preserves the two keydown source
 * blocks that the backend's ownership gates audit.
 */
function terminalClipboardShortcut(event, target) {
  const clipboardModifier = usesCommandModifier
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
  if (!clipboardModifier || event.altKey || event.shiftKey) return false;
  const key = keyId(event);
  if (key === "c") {
    // The selection is the grid model's, owned by THIS target's screen; the
    // text comes from the backend, after the key has been claimed.
    const expectedKey = terminalTargetKey(target);
    if (!terminalSelectionStands(expectedKey)) return false;
    event.preventDefault();
    void copyTerminalSelection(expectedKey);
    return true;
  }
  if (key !== "v") return false;
  event.preventDefault();
  void pasteClipboardAt(target);
  return true;
}

/* Hangul, and every other script that composes.
 *
 * A Korean syllable is assembled by the IME across several keystrokes, and
 * only the finished syllable may reach the pty — sending the half-built jamo
 * would type `ㅎ`, `하`, `한` as three separate things. So the sink is drained
 * when composition ENDS, and left alone while it is running.
 *
 * `input.isComposing` alone cannot decide that, which is why nothing Korean
 * could be typed into this window at all: WebKit fires the last `input` of a
 * composition with `isComposing` still true and then `compositionend`, so a
 * handler that returns on `isComposing` drops the finished syllable and the
 * next one, forever. Tracking the state across the two events is
 * order-independent — whichever of `compositionend` and the final `input`
 * arrives last does the drain, and the other finds the sink already empty. */
let composing = false;

/* The Enter that COMMITS a composition, remembered until its text has gone.
 *
 * WebKit never re-delivers it: pressing Return over a live composition
 * arrives once, as keyCode 229, and the guards below rightly leave that key
 * to the IME — but Chromium follows the commit with a real Enter keydown and
 * WebKit does not, so the person's Enter simply vanished. Worse, the
 * textarea's default action then inserted a LINE FEED, and the drain shipped
 * it as text: `\n` is Ctrl+J to a TUI, which claude's composer takes as
 * "insert a newline" — so typing 한글 and pressing Enter grew the message a
 * line instead of sending it (reported: "인풋창 한글 이상"). The default is
 * suppressed, the Enter is remembered here, and the drain re-speaks it as a
 * real key AFTER the committed syllable — the order the person typed. Only
 * after text: an order-B engine drains empty at `compositionend` and the
 * syllable arrives in the final `input`, and the Enter must not overtake it. */
let commitEnter = false;

/* ---- 입력 컨텍스트가 늦은 자모는 우리가 조합한다 (1-fi) ----
 *
 * The platform attaches its input context a beat after focus — and a beat
 * after an input-source switch — so the first keystrokes of a Korean word
 * can arrive RAW: bare compatibility jamo the IME never saw. Dropping them
 * (the old stance) eats the first letter; forwarding them types ㅎㅏㄴ one
 * jamo at a time, which is what xterm does and what the drop existed to
 * avoid. Both lose. What neither does is the obvious third thing: the 2-set
 * automaton is a published, finite fold — so COMPOSE the raw jamo here,
 * show the growing syllable in the same preedit box a real composition
 * uses, and hand the pty finished text. A race that swallows the whole
 * word now yields the whole word, perfectly; only a race that ends
 * mid-syllable still shows a seam, and that seam is one jamo wide.
 *
 * `foldHangul` is the automaton alone — jamo in, text out, no state kept
 * between calls. The rescue below keeps the raw keys and refolds from
 * scratch on every change: Backspace is a pop instead of an inverse
 * automaton, which is how the fold stays simple enough to trust. */
/* The compatibility-jamo block, one spelling for every place that asks —
 * the keydown that recognizes the race and the input backstop behind it. */
const HANGUL_JAMO = /^[\u3130-\u318f]+$/;
const HANGUL_CHO = [..."ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ"];
const HANGUL_JUNG = [..."ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ"];
const HANGUL_JONG = [..."ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ"];
const HANGUL_V_PAIR = { ㅗㅏ: "ㅘ", ㅗㅐ: "ㅙ", ㅗㅣ: "ㅚ", ㅜㅓ: "ㅝ", ㅜㅔ: "ㅞ", ㅜㅣ: "ㅟ", ㅡㅣ: "ㅢ" };
const HANGUL_T_PAIR = {
  ㄱㅅ: "ㄳ", ㄴㅈ: "ㄵ", ㄴㅎ: "ㄶ", ㄹㄱ: "ㄺ", ㄹㅁ: "ㄻ", ㄹㅂ: "ㄼ",
  ㄹㅅ: "ㄽ", ㄹㅌ: "ㄾ", ㄹㅍ: "ㄿ", ㄹㅎ: "ㅀ", ㅂㅅ: "ㅄ",
};
/* The clusters back apart, for the vowel that steals a syllable's tail:
 * 닭 + ㅣ is 달 + 기, and the automaton knows that because ㄺ knows it is
 * ㄹ then ㄱ. Derived from the pair table so the two can never disagree. */
const HANGUL_T_SPLIT = Object.fromEntries(
  Object.entries(HANGUL_T_PAIR).map(([two, one]) => [one, [...two]]),
);

/* One syllable's parts as a character: the standard block arithmetic when
 * the parts line up, the parts themselves in a row when they cannot (a lone
 * vowel, a cluster typed directly). */
function hangulSyllable(part) {
  const L = part.L ? HANGUL_CHO.indexOf(part.L) : -1;
  const V = part.V ? HANGUL_JUNG.indexOf(part.V) : -1;
  // In the block arithmetic "no final" is 0 and finals are 1-based, which is
  // exactly indexOf's -1 plus one.
  const T = part.T ? HANGUL_JONG.indexOf(part.T) : -1;
  if (L >= 0 && V >= 0 && (part.T === null || T >= 0)) {
    return String.fromCharCode(0xac00 + (L * 21 + V) * 28 + T + 1);
  }
  return `${part.L ?? ""}${part.V ?? ""}${part.T ?? ""}`;
}

/* The 2-set automaton, whole: initial, vowel (digraphs), final (clusters),
 * and the tail that migrates when a vowel follows a closed syllable. */
function foldHangul(jamos) {
  let out = "";
  let part = null;
  const commit = () => {
    if (part) out += hangulSyllable(part);
    part = null;
  };
  for (const ch of jamos) {
    const vowel = HANGUL_JUNG.includes(ch);
    const consonant = !vowel && (HANGUL_CHO.includes(ch) || HANGUL_JONG.includes(ch));
    if (!vowel && !consonant) {
      commit();
      out += ch;
      continue;
    }
    if (consonant) {
      if (part?.V && !part.T && HANGUL_JONG.includes(ch)) {
        part.T = ch;
        continue;
      }
      if (part?.T && HANGUL_T_PAIR[part.T + ch]) {
        part.T = HANGUL_T_PAIR[part.T + ch];
        continue;
      }
      commit();
      if (HANGUL_CHO.includes(ch)) part = { L: ch, V: null, T: null };
      else out += ch;
      continue;
    }
    if (part?.T) {
      const [keep, move] = HANGUL_T_SPLIT[part.T] ?? ["", part.T];
      part.T = keep || null;
      commit();
      part = { L: move, V: ch, T: null };
      continue;
    }
    if (part?.L && !part.V) {
      part.V = ch;
      continue;
    }
    if (part?.V && HANGUL_V_PAIR[part.V + ch]) {
      part.V = HANGUL_V_PAIR[part.V + ch];
      continue;
    }
    commit();
    part = { L: null, V: ch, T: null };
  }
  commit();
  return out;
}

/* A syllable back into its jamo — the fold's inverse, from the same tables.
 * The word-run below needs it because a broken-attach IME commits syllables
 * built from an INCOMPLETE stream, and only at the jamo level can the stolen
 * head be put back where it belonged. The automaton then re-decides every
 * boundary exactly as the IME itself would have with the whole stream in
 * hand (로+ㄱ becomes 록 — the true reading, where the old seam refused the
 * join). Cluster tails ride whole: the fold already splits ㄺ when a vowel
 * comes to steal its half. Non-hangul passes through untouched. */
function decomposeHangul(text) {
  const jamos = [];
  for (const ch of text) {
    const code = ch.codePointAt(0);
    if (code < 0xac00 || code > 0xd7a3) {
      jamos.push(ch);
      continue;
    }
    const at = code - 0xac00;
    jamos.push(HANGUL_CHO[Math.floor(at / 588)]);
    jamos.push(HANGUL_JUNG[Math.floor(at / 28) % 21]);
    const tail = at % 28;
    if (tail > 0) jamos.push(HANGUL_JONG[tail - 1]);
  }
  return jamos;
}

/* The word a broken-attach IME is building, as jamo (1-fi²).
 *
 * The seam used to repair ONE syllable: the stolen head joined to the first
 * commit, and 로 came out whole. The black box then recorded why that was
 * not enough (window-errors.log, 2026-08-18): the IME runs the whole word
 * one jamo SHORT, so every later boundary lands wrong — 문자 committed as
 * ㅜ, ㄴ, 자 and the pty read 무ㄴ자; 계속진행해 walked out as ㄱ ㅖ 속…. So
 * when a composition opens over rescued jamo, the repair is the WORD: every
 * commit is decomposed back to jamo and refolded behind the stolen head,
 * the caret shows the true word growing, and the pty receives it whole when
 * the run settles, meets a word boundary, or the person presses Enter.
 * Healthy compositions — nothing rescued — drain per commit exactly as
 * before, and never enter this road. */
let wordJamos = [];
let wordTimer = 0;

function flushWord() {
  clearTimeout(wordTimer);
  wordTimer = 0;
  if (!wordJamos.length) return;
  const text = foldHangul(wordJamos);
  wordJamos = [];
  traceIme(`word.flush out=${text}`);
  if (STRAY_JAMO.test(text)) reportStrayJamo("word.flush", text);
  clearPreedit();
  routeText(text);
}

/* How long a self-composed syllable may sit before it is committed to the
 * pty on its own. A real IME holds a composition forever; the pty cannot —
 * text the person believes typed must not still be in this window when an
 * agent reads the prompt. One second is longer than any inter-key gap in a
 * word and shorter than "why is my letter missing". */
const RESCUE_SETTLE_MS = 1000;
/* An INCOMPLETE tail — a bare jamo still waiting for its vowel — earns extra
 * settle rounds before it is committed: sending it is exactly the split this
 * machinery exists to prevent, and the person is probably mid-keystroke. */
const RESCUE_SETTLE_ROUNDS = 3;
let rescueJamos = [];
let rescueTimer = 0;
let rescueWaits = 0;

function flushRescue() {
  clearTimeout(rescueTimer);
  rescueTimer = 0;
  rescueWaits = 0;
  const text = foldHangul(rescueJamos);
  rescueJamos = [];
  if (!text) return;
  traceIme(`rescue.flush out=${text}`);
  if (STRAY_JAMO.test(text)) reportStrayJamo("rescue.flush", text);
  clearPreedit();
  routeText(text);
}

function settleRescue() {
  rescueTimer = 0;
  if (!rescueJamos.length) return;
  const shown = foldHangul(rescueJamos);
  if (HANGUL_JAMO.test(shown.slice(-1)) && rescueWaits < RESCUE_SETTLE_ROUNDS) {
    rescueWaits += 1;
    rescueTimer = setTimeout(settleRescue, RESCUE_SETTLE_MS);
    return;
  }
  flushRescue();
}

function holdRescue(jamos) {
  clearTimeout(rescueTimer);
  rescueWaits = 0;
  rescueJamos.push(...jamos);
  paintPreedit(foldHangul(rescueJamos));
  rescueTimer = setTimeout(settleRescue, RESCUE_SETTLE_MS);
}

/* ---- 입력 통로의 블랙박스 ----
 *
 * A sighting was reported that this code CANNOT explain: "ㅇㅇ 썼는데 응으로
 * 표시". 응 is ㅇ+ㅡ+ㅇ, and an audit of every road here — the fold, the
 * seam joins, the input backstop — found none that can mint a vowel absent
 * from the event stream, while probing the composer proved it renders the
 * bytes we send verbatim. So the ㅡ arrived from OUTSIDE this file, and the
 * next sighting has to show from where. Every decision on the Korean input
 * road drops one line into this ring: held in memory only, never sent
 * anywhere, read as `__IME_TRACE__` at the window while an incident is
 * still on screen. */
const IME_TRACE_CAP = 200; // a couple of sentences of typing — the whole incident, nothing older
const imeTrace = [];
window.__IME_TRACE__ = imeTrace;

function traceIme(line) {
  if (imeTrace.length >= IME_TRACE_CAP) imeTrace.shift();
  imeTrace.push(`${Date.now()} ${line}`);
}

/* Any hangul at all — compat jamo or finished syllable. Distinct from
 * HANGUL_JAMO on purpose: this asks "did hangul reach a road meant for
 * control keys", which is precisely the anomaly the ring exists to catch. */
const HANGUL_ANY = /[\u3130-\u318f\uac00-\ud7a3]/;

/* A compat jamo in text LEAVING for the pty. The machinery above exists so
 * only finished syllables leave this window — bare jamo going out is either
 * someone typing ㅋㅋ on purpose or the machinery failing, and after the
 * fact the ring alone cannot say which, because it lives in memory and dies
 * with the window (this is how "ㄱ ㅖ 속" outlived its own evidence). So the
 * moment one leaves, the ring goes into the husk log through the same door
 * every window fault uses. At most once a minute: ㅋㅋ is a word people
 * actually type. */
const STRAY_JAMO = /[\u3130-\u318f]/;
const STRAY_JAMO_REPORT_EVERY_MS = 60_000;
let strayJamoReportedAt = 0;

/* Only hangul survives onto disk. The ring's memory-only promise stands for
 * everything else it heard — a password drained into a shell moments before
 * must not ride a ㅋㅋ report into a log file. The stamp and the kind are
 * the ring's own words and stay; in the payload every non-hangul character
 * becomes a dot, position kept, so the journey still reads. */
function scrubImePayload(payload) {
  return payload.replace(/[^\u3130-\u318f\uac00-\ud7a3 ]/g, "\u00b7");
}

function scrubbedImeTrace() {
  return imeTrace.map((line) => {
    const stampEnd = line.indexOf(" ");
    const kindEnd = line.indexOf(" ", stampEnd + 1);
    if (kindEnd < 0) return line;
    return line.slice(0, kindEnd + 1) + scrubImePayload(line.slice(kindEnd + 1));
  });
}

function reportStrayJamo(road, text) {
  const now = Date.now();
  if (now - strayJamoReportedAt < STRAY_JAMO_REPORT_EVERY_MS) return;
  strayJamoReportedAt = now;
  const leaked = scrubImePayload(text);
  const trail = scrubbedImeTrace().join("\n");
  void invoke("log_window_error", {
    message: `ime: bare jamo left through ${road}: ${leaked}\n${trail}`,
  });
}

/* The keystrokes themselves, at the sink, before the raw insert happens.
 * A properly attached IME reports `Process`/229 and is left alone; a bare
 * jamo as the KEY is the race's own signature, and preventDefault here is
 * what keeps the half-letter out of the sink entirely. Runs at the target,
 * so it is ahead of the document-level routing by event order, not by
 * luck. */
keySink.addEventListener("keydown", (event) => {
  // The key that broke the last composition open is this one: whatever it
  // does next, the half letter it committed goes out ahead of it.
  escortHalfLetter();
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) {
    traceIme(`key.ime ${event.key}`);
    // The bare Enter that commits: kept (see `commitEnter`). Modified ones
    // stay the IME's whole — Shift+Enter has its own encoding downstream and
    // re-speaking it as a plain Enter would submit what asked for a newline.
    if (
      event.key === "Enter" &&
      !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey
    ) {
      event.preventDefault();
      commitEnter = true;
    }
    return;
  }
  // A real keystroke outdates a remembered commit-Enter: whatever that Enter
  // was going to submit, the person has typed past it.
  commitEnter = false;
  if (event.metaKey || event.ctrlKey || event.altKey) {
    flushWord();
    flushRescue();
    return;
  }
  const jamo = event.key.length === 1 && HANGUL_JAMO.test(event.key);
  if (jamo) {
    event.preventDefault();
    traceIme(`key.raw ${event.key}`);
    holdRescue([event.key]);
    return;
  }
  // A modifier going DOWN is not a keystroke. ㅖ, ㅒ and every double jamo
  // BEGIN with Shift, and treating that Shift as a control key flushed the
  // held head 112ms after it was caught — the husk tape's own numbers:
  // `input.raw ㄱ` then `rescue.flush ㄱ`, and 계속 split at exactly ㄱ|ㅖ
  // every time while shiftless words healed. The IME itself composes
  // straight through Shift, and so does the rescue now.
  if (event.key === "Shift") return;
  if (!rescueJamos.length && !wordJamos.length) return;
  if (event.key === "Backspace") {
    event.preventDefault();
    traceIme("key.pop");
    const standing = rescueJamos.length ? rescueJamos : wordJamos;
    standing.pop();
    if (rescueJamos.length || wordJamos.length) {
      paintPreedit(foldHangul(rescueJamos.length ? rescueJamos : wordJamos));
    } else {
      clearTimeout(rescueTimer);
      clearPreedit();
    }
    return;
  }
  // Enter, Escape, an ASCII letter: the text goes first, the key after —
  // which is macOS's own order when a composition meets a control key. The
  // word is older than the rescue, so it speaks first.
  flushWord();
  flushRescue();
});

/* How long a blur may stand before the rescue believes it.
 *
 * The input-source attach — the very race the rescue exists for — can blur
 * and refocus the sink within a frame or two, and flushing on that blink is
 * how 계속 landed as "ㄱ ㅖ 속" (reported: "첫타이핑이 ㄱ ㅖ 속진행해 이런식"):
 * the held ㄱ went out alone on the blink's blur, the ㅖ after it, and only
 * then did the IME stand up for 속. A real departure — a click on a panel,
 * leaving the window — almost never returns within this beat, and when it
 * does, a half syllable held a beat longer is invisible. */
const BLUR_FLUSH_GRACE_MS = 150;
let blurFlushTimer = 0;

keySink.addEventListener("focus", () => {
  // One line for the tape: the attach dance is a blur-focus flutter, and
  // telling it apart from a real departure is exactly what the black box
  // is for.
  traceIme("focus");
});

/* blur의 반대편 끝 — 포커스가 어디로 갔는가. 폭풍 로그(05:07, keySink
 * focus↔blur 9초 15번+)는 진동만 남기고 상대를 안 남겼다: relatedTarget이
 * 그 상대다. null이면 네이티브 뷰(플로팅 셸의 웹뷰, 창 밖)로 나간 것이고,
 * 이름이 있으면 DOM 안의 그 요소가 훔친 것이다 — 다음 폭풍은 제 상대를
 * 제 손으로 적는다. */
function focusThief(event) {
  const to = event.relatedTarget;
  if (!to) return "native";
  return `${to.tagName.toLowerCase()}${to.id ? `#${to.id}` : ""}${
    to.classList?.length ? `.${to.classList[0]}` : ""}`;
}

keySink.addEventListener("blur", (event) => {
  traceIme(`blur→${focusThief(event)}`);
  // A remembered commit-Enter dies with the focus AT ONCE: whatever it was
  // going to submit, the person has left the terminal — a grace period on a
  // submit key would fire it into a surface the person is no longer facing.
  commitEnter = false;
  clearTimeout(blurFlushTimer);
  blurFlushTimer = setTimeout(() => {
    blurFlushTimer = 0;
    // The blink, not a departure: focus already came home, the rescue and
    // the seam stand exactly as the person left them mid-word.
    if (document.activeElement === keySink) return;
    flushWord();
    flushRescue();
  }, BLUR_FLUSH_GRACE_MS);
});

/* ---- 조합 중인 글자를 커서 자리에 (1-ff) ----
 *
 * The sink composes invisibly — it is a 1px field parked in a corner — so
 * while the IME assembled a syllable the person watched nothing change and
 * the finished letter appeared out of thin air. xterm answers this with a
 * `composition-view` div riding the cursor cell (`updateCompositionElements`),
 * and so does this: one box, dressed in the terminal's own font tokens, worn
 * by whichever screen owns the keyboard, positioned by copying the caret's
 * transform — the one place that already knows where the cursor is drawn.
 *
 * One box for the whole window rather than one per view: there is one
 * keyboard and one IME, so two live preedits is a state that cannot exist.
 * It is REMOVED when cleared, not just hidden, so a closing terminal cannot
 * keep it — `dropTermView` sweeps it with the host it was riding. */
const preeditBox = document.createElement("div");
preeditBox.className = "term-preedit";
preeditBox.hidden = true;

/* The screen whose caret the composition rides: the same screen `routeText`
 * will hand the finished syllable to, asked of the same function. */
function preeditView() {
  return terminalViewForTarget(keyboardTarget());
}

function paintPreedit(text) {
  const view = preeditView();
  if (!view || !text) {
    clearPreedit();
    return;
  }
  if (preeditBox.parentNode !== view.host) view.host.appendChild(preeditBox);
  preeditBox.textContent = text;
  preeditBox.style.transform = view.caret.style.transform;
  preeditBox.style.height = view.caret.style.height;
  preeditBox.style.lineHeight = view.caret.style.height;
  preeditBox.hidden = false;
  /* The IME's own candidate window (한자 변환, emoji search) opens at the
   * focused field, and the field is parked at the window's corner. Wearing
   * the caret's place for the length of the composition puts that window
   * beside the text it is about — xterm moves its textarea the same way. */
  const at = view.caret.getBoundingClientRect();
  keySink.style.left = `${at.left}px`;
  keySink.style.top = `${at.top}px`;
}

function clearPreedit() {
  preeditBox.remove();
  preeditBox.hidden = true;
  preeditBox.textContent = "";
  keySink.style.left = "";
  keySink.style.top = "";
}

/* ---- 반쪽에서 끊긴 커밋은 제 키를 달고서야 나간다 (t-5835) ----
 *
 * 조합이 낱자 하나만 남긴 채 끝나면 드레인은 그것을 완성된 음절과 똑같이
 * pty로 보냈다. 그래서 `ㅎ`·`ㅁ`·`ㅣ`가 판에 찍혔다(window-errors.log,
 * 2026-09-22: 「bare jamo left through drain」 다섯 번, 줄마다 흔적 링이 붙어
 * 있다). 링이 보여 주는 이 창의 이벤트 차례가 가르는 자리다:
 *
 *   조합 이벤트가 먼저 오고, 그것을 일으킨 keydown(229)이 뒤에 온다 —
 *   `comp.start`·`comp.update ㅍ` 다음에 `key.ime ㅍ`. 그러니 사람이 누른
 *   키가 일으킨 커밋은 언제나 그 키를 바로 뒤에 달고 있다: 로그에 남은 성한
 *   커밋 444건에서 커밋 다음 사건까지 p50 3 ms·p90 15 ms, 404건이 20 ms 안.
 *   반대로 사람이 아무것도 누르지 않았는데 판이 조합을 가져가 버린 커밋은
 *   가장 빠른 것도 97 ms 뒤에야 다음 사건을 봤다 (다섯 건 모두 97~581 ms).
 *
 * 그 사이가 이 박자다. 낱자만 든 커밋은 여기 붙들렸다가 뒤따라오는 사람의
 * 행동 — 키다운, 새 조합, input — 앞에 실려 나가고, 박자가 조용히 지나가면
 * 아예 나가지 않는다. ㅋㅋ·ㅠㅠ 처럼 낱자를 정말로 치는 글은 다음 ㅋ의
 * 키다운이 곧장 놓아 주므로 예전과 같은 박자로 판에 닿는다.
 *
 * 온전한 음절은 이 길을 타지 않는다 — 끝난 글자는 기다릴 것이 없다. */
const HALF_LETTER_ESCORT_MS = 48;
let halfLetter = "";
let halfLetterAt = 0;
let halfLetterTimer = 0;

function holdHalfLetter(text) {
  clearTimeout(halfLetterTimer);
  halfLetter = text;
  halfLetterAt = performance.now();
  traceIme(`half.hold ${text}`);
  // The timer only makes sure the answer is given when nothing else asks —
  // it is `escortHalfLetter` again, which by then reads a passed beat and
  // lets nothing out. A timer running late therefore changes no verdict.
  halfLetterTimer = setTimeout(escortHalfLetter, HALF_LETTER_ESCORT_MS);
}

/* 붙들린 반쪽의 운명을 정하는 한 자리 — 사람의 다음 행동이 시작되기 전에.
 *
 * 박자를 재는 자리가 여기인 것이 요점이다: 문이 언제 열렸는지를 문에서 재야
 * 판이 멎어 있던 동안에도 답이 같다. 타이머가 제때 못 뛰면 붙들린 반쪽이
 * 뒤늦은 조합을 타고 나가 버린다 — 전체 하네스를 한꺼번에 돌릴 때 실제로
 * 그랬다.
 *
 * 판으로 가는 낱자는 여전히 허스크가 듣는다: 이 길로 나가는 것은 사람이 친
 * ㅋㅋ이고, 「drain」 이름으로 찍히는 줄이 다시 보이면 그것은 회귀다. */
function escortHalfLetter() {
  if (!halfLetter) return;
  clearTimeout(halfLetterTimer);
  halfLetterTimer = 0;
  const text = halfLetter;
  halfLetter = "";
  if (performance.now() - halfLetterAt > HALF_LETTER_ESCORT_MS) {
    traceIme(`half.dropped ${text}`);
    return;
  }
  traceIme(`half.escort out=${text}`);
  reportStrayJamo("escort", text);
  routeText(text);
}

function drainKeySink() {
  clearPreedit();
  const text = keySink.value;
  keySink.value = "";
  if (wordJamos.length) {
    wordJamos.push(...decomposeHangul(text));
    traceIme(`drain word=${foldHangul(wordJamos)}${commitEnter ? " +enter" : ""}`);
    if (commitEnter) {
      flushWord();
      commitEnter = false;
      routeCommittedEnter();
      return;
    }
    // A word boundary in the commit — space, punctuation, anything the
    // automaton passes through — ends the run now. Hangul keeps growing
    // and settles on the same clock the rescue trusts.
    const tail = wordJamos.at(-1) ?? "";
    if (tail && !HANGUL_JAMO.test(tail) && !HANGUL_ANY.test(tail)) {
      flushWord();
      return;
    }
    paintPreedit(foldHangul(wordJamos));
    clearTimeout(wordTimer);
    wordTimer = setTimeout(flushWord, RESCUE_SETTLE_MS);
    return;
  }
  const joined = text;
  traceIme(`drain sink=${text} out=${joined}${commitEnter && joined ? " +enter" : ""}`);
  // 반쪽 글자만 든 커밋은 사람이 끝낸 글자가 아니다 (t-5835). 그것을 일으킨
  // 키를 한 박자 기다렸다가, 그 키 앞에 실려 나간다. Enter로 커밋된 것은
  // 그 키가 이미 여기 와 있으므로 기다릴 것이 없다.
  if (!commitEnter && HANGUL_JAMO.test(joined)) {
    holdHalfLetter(joined);
    return;
  }
  if (STRAY_JAMO.test(joined)) reportStrayJamo("drain", joined);
  if (joined) routeText(joined);
  // The Enter that committed this text goes AFTER it, as the key it was —
  // never before it (order-B drains empty first and the flag waits), and
  // never as the `\n` the textarea would have written (see `commitEnter`).
  if (commitEnter && joined) {
    commitEnter = false;
    routeCommittedEnter();
  }
}

keySink.addEventListener("compositionstart", () => {
  // Whatever this composition commits comes after the half letter the last
  // one left behind, never before it.
  escortHalfLetter();
  // The input context has arrived — mid-word, possibly mid-syllable. What
  // the rescue holds must NOT go to the pty here: 로 typed across this seam
  // used to land as "ㄹ" then "ㅗ" (reported). The half-word waits beside
  // the composition and `drainKeySink` joins the two when the IME commits.
  clearTimeout(rescueTimer);
  rescueTimer = 0;
  rescueWaits = 0;
  if (rescueJamos.length) {
    wordJamos.push(...rescueJamos);
    rescueJamos = [];
  }
  // Growth is arriving; the settle waits for the drain that follows it.
  clearTimeout(wordTimer);
  wordTimer = 0;
  traceIme(`comp.start word=${foldHangul(wordJamos)}`);
  // A commit-Enter still armed here belonged to a composition that committed
  // nothing — the only shape that leaves it unconsumed. New word, clean slate.
  commitEnter = false;
  composing = true;
});

keySink.addEventListener("compositionupdate", (event) => {
  traceIme(`comp.update ${event.data ?? ""}`);
  // The person watching the caret sees the TRUE word assembling — the
  // stolen head refolded with the composing tail — not ㅗ beside a debt.
  paintPreedit(wordJamos.length
    ? foldHangul([...wordJamos, ...decomposeHangul(event.data ?? "")])
    : (event.data ?? ""));
});

keySink.addEventListener("compositionend", () => {
  traceIme("comp.end");
  composing = false;
  drainKeySink();
});

keySink.addEventListener("input", () => {
  // A composition's own trailing input finds the sink already drained — that
  // is the engine finishing, not the person starting, and it must not be what
  // lets a held half letter out. Text standing here is a real insertion.
  if (keySink.value !== "") escortHalfLetter();
  if (composing) return;
  // A finished composition never looks like this: bare compatibility jamo
  // (ㄴ, ㅏ — U+3130-318F) landing OUTSIDE a composition are keystrokes the
  // IME never received — the input-context race, arriving through a road the
  // keydown rescue above did not cover (its preventDefault normally keeps
  // raw jamo out of the sink entirely). This used to DROP them, which ate
  // the first letter of the first word after every focus handoff and was
  // reported exactly so. They go to the same automaton instead: folded,
  // shown at the caret, committed as finished syllables.
  if (HANGUL_JAMO.test(keySink.value)) {
    const raw = [...keySink.value];
    keySink.value = "";
    traceIme(`input.raw ${raw.join("")}`);
    holdRescue(raw);
    return;
  }
  drainKeySink();
});

keySink.addEventListener("paste", (event) => {
  event.preventDefault();
  const target = keyboardTarget();
  const emulator = emulatorKeyboardTarget();
  if (target === null && emulator === null) return;
  const text = event.clipboardData.getData("text");
  if (text) {
    if (emulator) routeText(text);
    else void pasteTextAt({ ...target }, text);
    return;
  }
  if (emulator) return;
  // 글자가 없으면 그림을 본다 — Orca의 계약: 이미지는 임시 파일로 앉고,
  // 터미널에는 그 경로가 글자로 붙는다. 실행되는 것은 없다.
  const image = [...(event.clipboardData.items ?? [])]
    .find((item) => item.kind === "file" && item.type.startsWith("image/"));
  const file = image?.getAsFile();
  if (!file) return;
  void pasteImageAt({ ...target }, file);
});

/* 클립보드 이미지 → 임시 파일 → 그 경로를 기존 paste 문(term_paste /
 * paste_input — bracketed-paste 소독은 백엔드 소유)으로.
 *
 * 바이트는 JSON 필드가 아니라 요청 본문 그대로 건넌다. base64 는 그림의
 * 4/3 크기 문자열을 이쪽에 한 벌 더 만들고 백엔드가 그것을 다시 풀어야
 * 했으며, 둘 다 그림이 얼마나 큰지 묻기도 전에 치르는 값이었다. 종류는
 * 본문에 실을 자리가 없어 헤더로 간다. */
async function savePastedImage(file) {
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    return await invoke("save_pasted_image", bytes, { headers: { "x-image-kind": file.type } });
  } catch (error) {
    showError(t("clipboard.imagePasteFailed", "클립보드의 이미지를 붙여넣지 못했습니다."));
    return null;
  }
}

/* 그림을 앉히고 그 경로를 판에 붙인다. 앉히는 반쪽(`savePastedImage`)은
 * 입력줄의 첨부(t-2993)도 쓴다 — 거기서는 경로가 붙지 않고 칩이 된다. */
async function pasteImageAt(target, file) {
  const path = await savePastedImage(file);
  if (path) await pasteTextAt(target, path);
}

for (const surface of [stageView.host, floatView.host]) focusesTheKeyboard(surface);

/* ---- lane opening ---- */

/* One open at a time. A key repeat or a double click must not fan out into
 * several sessions — the serve's pool showed exactly that scar: three
 * sessions created 11ms apart. */
let openingLane = false;

async function openLane(sessionId = null, worktree = null) {
  if (!zoAvailable || openingLane) return;
  openingLane = true;
  const { rows, cols } = stageView.gridSize();
  try {
    const lane = await invoke("open_lane", { session: sessionId, worktree, rows, cols });
    upsertLane(lane);
    setFocused(lane.id);
  } catch (error) {
    showError(error);
  } finally {
    openingLane = false;
    // There is no detached-session ledger to race now. The lane returned by
    // `open_lane` already names its workspace, so one redraw is sufficient.
    refreshWorktrees();
  }
}

/* The sidebar's floor, and the command in the title-bar row. Every one of
 * these does something this window already does — a control that only looks
 * the part is a dead button, which is worse than a missing one. */
el("foot-settings").addEventListener("click", () => setSettingsOpen(true));
/* 단축키 도움말 — 그리고 Alt를 누른 채 눌렀을 때만 나오는 항목 하나.
 *
 * Orca에서 온보딩을 다시 여는 길은 사이드바 Help 드롭다운의 **Alt를 눌러야
 * 나타나는** 항목 하나뿐이다(`revealAdminOptions(altKey)`). 일반 사용자에게
 * 재실행 경로가 없다는 그 선택을 그대로 가져왔다: 한 번 끝낸 사람에게 전체화면
 * 마법사를 다시 여는 버튼은, 실수로 눌리는 쪽이 훨씬 잦다. */
el("foot-keys").addEventListener("click", (event) => {
  if (!event.altKey) {
    setSettingsOpen(true, "settings-keys");
    return;
  }
  const bounds = event.currentTarget.getBoundingClientRect();
  openSidebarMenu(bounds.right, bounds.top - 6, [
    {
      label: t("onboard.reopen", "온보딩 다시 보기"),
      run: () => {
        invoke("reopen_onboarding")
          .then((state) => openOnboarding(state))
          .catch(showError);
      },
    },
  ]);
});
/* 도움말 메뉴. 기능 투어로 가는 **유일한** 문이고, 체크리스트를 치운 사람이
 * 그것을 되찾는 문이기도 하다.
 *
 * Orca의 기능 투어는 네이티브 Help 메뉴의 한 항목에서만 열리며 자동 노출 경로가
 * 아예 없다 — 자기 제품을 소개하는 모달을 사람이 부르지도 않았는데 띄우지 않은
 * 그 선택은 옳다. 여기서도 그대로다: `openFeatureWall`을 부르는 곳은 이 메뉴
 * 하나뿐이다. */
el("foot-guide").addEventListener("click", (event) => {
  const bounds = event.currentTarget.getBoundingClientRect();
  openSidebarMenu(bounds.right, bounds.top - 6, [
    {
      label: t("guide.title", "시작하기"),
      run: () => {
        // 치워 둔 사람에게도 열린다. 치우기는 사이드바에서 치우는 것이지
        // 목록을 버리는 것이 아니다.
        openSetupGuide();
      },
    },
    {
      label: t("wall.title", "ZeroCode 둘러보기"),
      run: openFeatureWall,
    },
  ]);
});
/* The toolbar's crosshair, Orca's own right-hand pair with the board door
 * (ScrollToCurrentWorkspaceToolbarButton.tsx:20): unfold the project that
 * holds the active workspace and put its row under the eye. The new-session
 * and terminal buttons that sat here were ours alone — their chords survive
 * them, and the terminal keeps its own toggle in the activity rail. */
el("foot-reveal").addEventListener("click", revealActiveWorktree);
el("run-command").addEventListener("click", () => openTermTab({ door: "terminal" }));

/* What that button will actually start, on the button. Orca's `▷ 명령` names
 * the command it runs; ours reads the same setting the terminal does, so the
 * label and the effect cannot disagree. */
function paintCommandLabel() {
  // The argv, not the line. Splitting the line here on whitespace named `my`
  // for a program living at `/opt/my tools/zo` — the quoting exists precisely
  // so that path stays one word, and re-splitting threw that away.
  invoke("terminal_command_argv")
    .then((words) => {
      const program = (words ?? [])[0];
      el("run-command-label").textContent = program ? basename(program) : t("app.shell", "셸");
    })
    .catch(() => {});
}

/* ---- file tree (read-only, lazy, with working-tree badges) ---- */

/* path → porcelain code, refreshed at boot. Prefix matching lets a directory
 * carry the badge of anything modified beneath it. */
let vcsCodes = [];
let showGitIgnoredFiles = true;

function paintGitIgnoredFilesPreference() {
  el("show-git-ignored-files").checked = showGitIgnoredFiles;
}

function setShowGitIgnoredFiles(visible) {
  showGitIgnoredFiles = Boolean(visible);
  paintGitIgnoredFilesPreference();
  loadTree(fileTree, "").catch(showError);
  void commitSetting(
    "show_git_ignored_files",
    "set_show_git_ignored_files",
    { visible: showGitIgnoredFiles },
  );
}

el("show-git-ignored-files").addEventListener("change", (event) => {
  setShowGitIgnoredFiles(event.target.checked);
});

/* A porcelain code as the one letter the tree shows.
 *
 * Orca's explorer prints a single letter per row — `U` for an untracked file,
 * `M` for a modified one (measured on 1.4.164, side by side). Ours printed
 * git's raw two-column code, so an untracked file said `??`: correct, and
 * readable only by somebody who already knows porcelain. The letter is the
 * one that describes the file rather than the index, because that is what a
 * file tree is describing — with the index letter as the fallback for a
 * change that is only staged.
 *
 * `!!` never reaches here: an ignored row is said by its weight, not by a
 * letter competing with the real ones. */
function badgeLetter(code) {
  if (code === "" || code === "!!") return "";
  if (code.trim() === "??") return "U";
  const worktree = code.slice(1, 2).trim();
  const index = code.slice(0, 1).trim();
  return worktree || index || code.trim().slice(0, 1);
}

/* Which of git's decoration colours a porcelain code wears.
 *
 * The same reading as `badgeLetter` — the worktree column describes the file
 * and the index column is the fallback — so the letter and its colour can
 * never disagree about which change they are naming. An unrecognised code
 * returns "" and the badge keeps the muted default: a confident wrong colour
 * is worse than no colour. */
const GIT_DECORATION = {
  A: "added",
  M: "modified",
  D: "deleted",
  R: "renamed",
  C: "copied",
};

function gitDecorationOf(code) {
  const bare = code.trim();
  if (bare === "" || bare === "!!") return "";
  if (bare === "??") return "untracked";
  const letter = (code.slice(1, 2).trim() || code.slice(0, 1).trim()).toUpperCase();
  return GIT_DECORATION[letter] ?? "";
}

function badgeFor(relativePath) {
  if (relativePath === "") return "";
  // Ignored first, and only downward: `!! target` is the one line git prints
  // for that whole tree, so everything beneath it is ignored too.
  for (const [path, code] of vcsCodes) {
    if (code !== "!!") continue;
    if (path === relativePath || relativePath.startsWith(`${path}/`)) return code;
  }
  // Then the roll-up, which ignored entries take no part in. `crates/.DS_Store`
  // being ignored says nothing about `crates`, which is full of tracked
  // source — reading it as an answer about the parent dims the repository.
  for (const [path, code] of vcsCodes) {
    if (code === "!!") continue;
    if (path === relativePath || path.startsWith(`${relativePath}/`)) return code;
  }
  return "";
}

/* One folder's entries, from the backend or from the answer already fetched.
 *
 * A workspace switch reads the root directory and runs git at the same time,
 * because neither needs the other — but the tree cannot PAINT until the
 * badges are in, so its listing arrives before it is wanted and waits here.
 * One entry deep and dropped as soon as it is taken, so this is a handoff and
 * never a cache: a tree painted from a listing that outlived a file being
 * created is a tree that is quietly wrong. */
let listedAhead = null;

async function readDir(path) {
  try {
    listedAhead = { path, entries: await invoke("list_dir", { path }) };
  } catch {
    listedAhead = null;
  }
}

/* ---- the tree's right hand (P0-15 후반) ----
 *
 * Orca's file-explorer context menu (file-explorer-row-context-menu.tsx:
 * 144-311): three groups cut by two separators — create / paths·open·reveal /
 * rename·delete. The doors this window does not have yet stay off the menu
 * rather than greyed: OS file-clipboard Copy (:159-164), Add as Project
 * (:195-203), Open in Orca Browser (:219-227), remote Download (:246-256),
 * Collapse Folder (:257-262), Find in Folder (:263-271) — the gap map's
 * honest remainder. View File (:213-218) is not ported at all, because here
 * the ROW is that door already. */

/* The tree hands the backend absolute paths and keeps relative ones for
 * itself. "/" is correct on every target: Rust's PathBuf accepts it as a
 * separator on Windows too, and the fence (`inside_root`) canonicalizes
 * before comparing. */
function treeAbsolute(relative) {
  return `${activeWorktreePath}/${relative}`;
}

/* One line of the tree turned into a text field — creation and rename share
 * it. Enter answers; Escape and a click elsewhere abandon: an edit that was
 * wandered away from must not touch the disk. The field swallows its own
 * pointer and key events because the ROW under it is a button (open/fold),
 * and the window behind both listens for chords. */
function treeNameField(initial, settle) {
  const field = document.createElement("input");
  field.type = "text";
  field.className = "tree-name-edit";
  field.value = initial;
  field.spellcheck = false;
  for (const swallowed of ["pointerdown", "click", "dblclick"]) {
    field.addEventListener(swallowed, (event) => event.stopPropagation());
  }
  let done = false;
  const answer = (value) => {
    if (done) return;
    done = true;
    settle(value);
  };
  field.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter") answer(field.value.trim());
    else if (event.key === "Escape") answer(null);
  });
  field.addEventListener("blur", () => answer(null));
  return field;
}

/* New File / New Folder — an input row planted where the entry will live
 * (Orca :150-157; the seat is the folder itself for a folder row and the
 * row's own directory for a file — FileExplorerVirtualRows.tsx:187). A
 * folder is unfolded first so the input stands in a drawn room, through the
 * same door the row's click uses. */
async function startTreeCreate(kind, { row, container, path, relative, entry }) {
  let seatContainer = container;
  let seatPath = path;
  if (entry.is_dir) {
    await row._treeUnfold(true);
    seatContainer = row.nextElementSibling;
    seatPath = relative;
  }
  const ghost = document.createElement("div");
  ghost.className = `tree-row is-editing${kind === "folder" ? " is-dir" : " is-file"}`;
  ghost.innerHTML = '<span class="twist"></span><span class="tree-glyph"></span>';
  ghost.querySelector(".tree-glyph").innerHTML = icon(kind === "folder" ? "folder" : "file");
  const field = treeNameField("", async (name) => {
    ghost.remove();
    if (!name) return;
    try {
      await invoke("fs_create", { dir: treeAbsolute(seatPath), name, kind });
      await loadTree(seatContainer, seatPath);
      // A file just made is a file about to be written — open it the way the
      // row's own click would.
      if (kind !== "folder") {
        openPath(seatPath ? `${seatPath}/${name}` : name, { preview: true });
      }
    } catch (error) {
      showError(String(error).startsWith("tree.") ? treeOperationError(error) : error);
    }
  });
  ghost.appendChild(field);
  seatContainer.prepend(ghost);
  field.focus();
}

/* Rename — in the row's own name slot (Orca :296-304). A taken destination
 * is the backend's refusal to show, not this field's to guess. */
function startTreeRename({ row, container, path, relative, entry }) {
  const name = row.querySelector(".tree-name");
  if (!name || row.querySelector(".tree-name-edit")) return;
  name.hidden = true;
  const field = treeNameField(entry.name, async (asked) => {
    field.remove();
    name.hidden = false;
    if (!asked || asked === entry.name) return;
    try {
      await invoke("fs_rename", { path: treeAbsolute(relative), name: asked });
      await loadTree(container, path);
    } catch (error) {
      showError(String(error).startsWith("tree.") ? treeOperationError(error) : error);
    }
  });
  name.after(field);
  field.focus();
  field.select();
}

function treeMenuAt(x, y, seat) {
  const { row, container, path, relative, entry } = seat;
  const reloadRoom = async () => {
    await loadTree(container, path);
  };
  const ask = (command, payload, after = null) => async () => {
    try {
      await invoke(command, payload);
      if (after) await after();
    } catch (error) {
      showError(String(error).startsWith("tree.") ? treeOperationError(error) : error);
    }
  };
  // The reveal word is the platform's, exactly as Orca writes it
  // (file-explorer-row-context-menu.tsx:45-50): Finder on macOS, the
  // containing folder on Linux, File Explorer on Windows. Read here, at
  // raise time, so a locale change reaches the next opening unasked.
  const revealLabel = usesCommandModifier
    ? t("tree.menu.revealMac", "Finder에서 보기")
    : usesLinuxPlatform
      ? t("tree.menu.revealLinux", "폴더 열기")
      : t("tree.menu.revealWindows", "파일 탐색기에서 보기");
  const items = [
    {
      label: t("tree.menu.newFile", "새 파일"),
      run: () => {
        void startTreeCreate("file", seat);
      },
    },
    {
      label: t("tree.menu.newFolder", "새 폴더"),
      run: () => {
        void startTreeCreate("folder", seat);
      },
    },
    { separator: true },
    {
      label: t("tree.menu.copyPath", "경로 복사"),
      run: () => clipboardText.write(treeAbsolute(relative)),
    },
    {
      label: t("tree.menu.copyRelativePath", "상대 경로 복사"),
      run: () => clipboardText.write(relative),
    },
  ];
  if (!entry.is_dir) {
    items.push({
      label: t("tree.menu.duplicate", "복제"),
      run: ask("fs_duplicate", { path: treeAbsolute(relative) }, reloadRoom),
    });
    // The row's own click, offered as a named door too (Orca's View File,
    // file-explorer-row-context-menu.tsx:214-217 — onViewFile IS the row's
    // handleClick upstream).
    items.push({
      label: t("tree.menu.viewFile", "파일 보기"),
      run: () => openPath(relative, { preview: true }),
    });
    // Any local file may stand in a browser tab as its file:// self — the
    // predicate only refuses remote files, which this tree does not hold
    // (useWorkspaceFileBrowserActionPredicate, file-preview.ts:46-64; the
    // open walks the address bar's own ladder, which admits file:).
    items.push({
      label: t("tree.menu.openInBrowser", "ZeroCode 브라우저에서 열기"),
      run: () => {
        void openBrowserTab(pathAsFileUrl(treeAbsolute(relative)));
      },
    });
  }
  // Orca's "Add as Project..." stands just above Open in Terminal, and ONLY
  // inside a FOLDER project (canShowAddAsProjectAction = isDirectory &&
  // isFolderRepo(activeRepo), file-explorer-row-context-menu.tsx:195-203):
  // inside a git repository the subfolders are the repository's own content,
  // not projects-in-waiting. The flow is its two dialogs in their order —
  // AddProjectFromFolderDialog ("Add Project" / "Add this folder as a
  // separate Orca project."), then NonGitFolderDialog when the folder is not
  // a repository, because the capability downgrade is confirmed, never
  // fallen into (repos.ts:3223-3229). The add itself is the same door a
  // picked folder walks (`openProjectAt`) — register AND activate, Orca's
  // finishProjectAddWithDefaultCheckout.
  if (entry.is_dir) {
    const activeProject = projects.find((held) => held.path === activeProjectPath);
    const folderProject =
      activeProject !== undefined && !activeProject.worktrees.some((held) => !held.is_folder);
    if (folderProject) {
      items.push({
        label: t("tree.menu.addAsProject", "프로젝트로 추가..."),
        run: async () => {
          const where = treeAbsolute(relative);
          // The path rides the box under the body, not the sentence — the
          // original's own layout (AddProjectFromFolderDialog.tsx:178-195:
          // "Add this folder as a separate Orca project." over a mono path
          // box), and a sentence with a long path folded into it wraps into
          // something nobody can read anyway.
          const adding = await askConfirm({
            title: t("project.addTitle", "프로젝트 추가"),
            body: t("project.addBody", "이 폴더를 별도의 프로젝트로 추가합니다."),
            detail: where,
            confirm: t("project.addConfirm", "프로젝트 추가"),
            deny: t("app.cancel", "취소"),
            cancel: false,
          });
          if (adding !== true) return;
          let kind;
          try {
            kind = await invoke("project_kind", { path: where });
          } catch (error) {
            showError(error);
            return;
          }
          if (kind !== "git") {
            // The capability question always says where the check ran — the
            // original appends "This path was checked locally." inside the
            // body (`checkedHostDescription`, NonGitFolderDialog.tsx:142) —
            // and shows the path in the box below it (:145-149). The SSH
            // variant of the sentence arrives with SSH hosts.
            const opening = await askConfirm({
              title: t("project.nonGitTitle", "폴더로 열기"),
              body: t(
                "project.nonGitBody",
                "이 폴더는 Git 저장소가 아닙니다. 편집기·터미널·검색은 쓸 수 있지만 Git 기반 기능은 쓸 수 없습니다.",
              ),
              note: t("project.checkedLocally", "이 경로는 로컬에서 확인했습니다."),
              detail: where,
              confirm: t("project.nonGitTitle", "폴더로 열기"),
              deny: t("app.cancel", "취소"),
              cancel: false,
            });
            if (opening !== true) return;
          }
          await openProjectAt(where);
        },
      });
    }
  }
  if (entry.is_dir) {
    items.push({
      label: t("tree.menu.openInTerminal", "터미널에서 열기"),
      run: async () => {
        try {
          const term = await invoke("open_term_tab", {
            rows: 24,
            cols: 96,
            plain: true,
            cwd: treeAbsolute(relative),
          });
          mountTermTab(term, {}, { placement: "tab" });
        } catch (error) {
          showError(error);
        }
      },
    });
  }
  // `renderedAs` is the one place that knows which extensions are markdown —
  // the same answer Orca reads from `detectLanguage` (:228).
  if (!entry.is_dir && renderedAs(relative) === "markdown") {
    items.push({
      label: t("mdview.open", "Markdown 프리뷰"),
      run: () => {
        void openMarkdownPreview(relative, { preview: true });
      },
    });
  }
  // Only a folder that is standing open offers to fold (Orca's
  // shouldShowCollapseFolderAction: isDirectory && isExpanded) — and the
  // fold takes the SUBTREE down with it, so reopening does not spring the
  // whole depth back (the menu's name in the source is CollapseFolderSubtree).
  if (entry.is_dir && row.getAttribute("aria-expanded") === "true") {
    items.push({
      label: t("tree.menu.collapseFolder", "폴더 접기"),
      run: () => {
        const children = row.nextElementSibling;
        for (const held of children?.querySelectorAll?.('.tree-row[aria-expanded="true"]') ?? []) {
          void held._treeUnfold?.(false);
        }
        void row._treeUnfold?.(false);
      },
    });
  }
  items.push(
    {
      label: revealLabel,
      run: ask("fs_reveal", { path: treeAbsolute(relative) }),
    },
    { separator: true },
    {
      label: t("tree.menu.rename", "이름 바꾸기"),
      run: () => startTreeRename(seat),
    },
    {
      label: t("tree.menu.delete", "삭제"),
      danger: true,
      run: ask("fs_trash", { paths: [treeAbsolute(relative)] }, reloadRoom),
    },
  );
  openSidebarMenu(x, y, items, row);
}

async function loadTree(container, path) {
  let entries = [];
  const ahead = listedAhead;
  listedAhead = null;
  try {
    entries = ahead?.path === path ? ahead.entries : await invoke("list_dir", { path });
  } catch {
    return;
  }
  container.replaceChildren();
  for (const entry of entries) {
    // `.git` is the repository's own machinery, not project content — the one
    // entry a file panel omits rather than dims.
    if (entry.name === ".git") continue;
    const relative = path ? `${path}/${entry.name}` : entry.name;
    const code = badgeFor(relative);
    const ignored = code === "!!";
    if (ignored && !showGitIgnoredFiles) continue;
    const row = document.createElement("div");
    // Nothing is hidden: an ignored entry is shown and *marked*, the way a
    // file panel tells you `target/` is build output instead of pretending
    // it does not exist.
    row.className = `tree-row${entry.is_dir ? " is-dir" : ""}${ignored ? " is-ignored" : ""}`;
    row.innerHTML =
      '<span class="twist"></span><span class="tree-glyph"></span>' +
      '<span class="tree-name"></span><span class="badge"></span>';
    // A folder can be opened and a file cannot, so only one of them gets a
    // chevron — and the space is kept either way so every name in the tree
    // starts on the same vertical line.
    row.querySelector(".twist").innerHTML = entry.is_dir ? icon("chevron") : "";
    // Which file it is, at a glance — the resolver Orca's explorer, quick
    // open and change rows all share (getFileTypeIcon), ported whole above.
    // The glyphs are lucide (MIT), the one Orca artwork we may take.
    row.querySelector(".tree-glyph").innerHTML = icon(
      entry.is_dir ? "folder" : fileTypeIcon(entry.name),
    );
    row.querySelector(".tree-name").textContent = entry.name;
    // One letter, the way Orca's explorer prints it — except "ignored",
    // which Orca says with a ⊘ at the row's edge rather than with another
    // letter competing with the real ones (measured live: `.re-scratch`,
    // `target/` and `.DS_Store` each carry the circle-slash).
    if (ignored) row.querySelector(".badge").innerHTML = icon("ban");
    else row.querySelector(".badge").textContent = badgeLetter(code);
    row.querySelector(".badge").dataset.tip = code.trim();
    row.querySelector(".badge").dataset.git = gitDecorationOf(code);
    container.appendChild(row);
    bindExplorerRow(row, relative, entry);
    if (!entry.is_dir) {
      row.classList.add("is-file");
      actsAsButton(row, () => openPath(relative, { preview: true }));
    }
    if (entry.is_dir) {
      const children = document.createElement("div");
      children.className = "tree-children";
      children.hidden = true;
      container.appendChild(children);
      row.setAttribute("aria-expanded", "false");
      // One door for folding, whoever knocks: the row's own click and the
      // context menu's New File both open the same folder the same way, and
      // `want` lets the menu say "open" without toggling a folder that
      // already is. A member, not a captured global — the menu holds only
      // the row it was raised on.
      const unfold = async (want) => {
        const fold = want === undefined ? !children.hidden : !want;
        if (fold === children.hidden) return;
        children.hidden = fold;
        row.setAttribute("aria-expanded", fold ? "false" : "true");
        row.querySelector(".icon--twist").classList.toggle("is-open", !fold);
        row.querySelector(".tree-glyph").innerHTML = icon(fold ? "folder" : "folder-open");
        if (!fold && children.childElementCount === 0) {
          await loadTree(children, relative);
        }
      };
      row._treeUnfold = unfold;
      actsAsButton(row, () => {
        void unfold();
      });
    }
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
      treeMenuAt(event.clientX, event.clientY, { row, container, path, relative, entry });
    });
  }
}

/* ---- searching the checkout: by name, or by what is inside ----
 *
 * One field, two questions, the way Orca puts 이름/내용 in this exact spot.
 * Searching swaps the tree for a flat list and clearing brings it back, so
 * the panel is never showing two answers at once. */

const fileSearch = el("file-search");
const fileHits = el("file-hits");
const fileTextHits = el("file-text-hits");
/* Name matching stays in the catalog, while content matching opens files;
 * the latter waits longer so typing one word does not read the tree twice. */
let fileSearchTimer = null;
let fileSearchMode = "name";
let fileSearchRequest = 0;

function setFileSearchMode(mode) {
  fileSearchMode = mode;
  paintExplorerFilters();
  for (const name of ["name", "text"]) {
    const button = el(`file-mode-${name}`);
    button.classList.toggle("is-active", name === mode);
    button.setAttribute("aria-selected", name === mode ? "true" : "false");
  }
  // The icon row above offers a door to each of these two questions, so which
  // door is lit changes with the answer to this one.
  paintActivityTabs();
  // The placeholder is the only label the field gets, so it has to say which
  // question is being asked rather than repeat the panel's name.
  fileSearch.placeholder =
    mode === "name" ? t("file.searchName", "파일 찾기") : t("file.searchContent", "내용 찾기");
  runFileSearch();
}

async function runFileSearch() {
  const request = ++fileSearchRequest;
  const query = fileSearch.value.trim();
  const mode = fileSearchMode;
  const root = activeWorktreePath;
  el("file-search-error").hidden = true;
  if (!query) {
    fileHits.hidden = true;
    fileTextHits.hidden = true;
    fileTree.hidden = false;
    return;
  }

  let hits;
  try {
    const command = mode === "name" ? "search_files" : "search_text";
    const options = mode === "text" ? explorerSearchOptions() : null;
    hits = await invoke(command, options ? { query, options } : { query });
  } catch (error) {
    if (request === fileSearchRequest && root === activeWorktreePath) {
      fileTextHits.replaceChildren();
      explorerSearchError(error);
    }
    return;
  }
  if (request !== fileSearchRequest || root !== activeWorktreePath) return;

  fileTree.hidden = true;
  if (mode === "name") {
    fileTextHits.hidden = true;
    paintNameHits(hits);
  } else {
    fileHits.hidden = true;
    paintTextHits(hits, query);
  }
}

function paintNameHits(hits) {
  const rows = document.createDocumentFragment();
  for (const hit of hits) {
    const row = document.createElement("div");
    row.className = "tree-row is-file";
    // The same three slots the tree's own rows use, so a hit list reads as
    // the tree filtered rather than as a different kind of list. Every hit is
    // a file, so the chevron slot stays empty and only keeps the alignment.
    row.innerHTML =
      '<span class="twist"></span><span class="tree-glyph"></span>' +
      '<span class="tree-name"></span>';
    row.querySelector(".tree-glyph").innerHTML = icon(fileTypeIcon(hit));
    row.querySelector(".tree-name").textContent = hit;
    row.dataset.tip = hit;
    actsAsButton(row, () => openPath(hit, { preview: true }));
    rows.appendChild(row);
  }
  fileHits.replaceChildren(rows);
  fileHits.hidden = false;
}

/* The line, with the words that were searched for marked inside it.
 *
 * Built out of text nodes and `<mark>` elements rather than assembled as an
 * HTML string: `hit.text` is a line off somebody's disk, and the one thing
 * that must never happen to a line off somebody's disk is being parsed as
 * markup. `textContent` on every fragment means a hit containing `<script>`
 * is a hit that says `<script>`.
 *
 * Case-insensitive, matching how the search itself was run, so the mark lands
 * on what the backend actually found. */
function paintNeedle(host, line, needle) {
  host.replaceChildren();
  const want = needle.toLowerCase();
  if (want === "") {
    host.textContent = line;
    return;
  }
  const hay = line.toLowerCase();
  let from = 0;
  for (let at = hay.indexOf(want); at !== -1; at = hay.indexOf(want, from)) {
    if (at > from) host.append(line.slice(from, at));
    const mark = document.createElement("mark");
    mark.className = "hit-mark";
    // Sliced out of the ORIGINAL, so the marked text keeps the casing it has
    // on disk rather than the casing it was typed in.
    mark.textContent = line.slice(at, at + needle.length);
    host.append(mark);
    from = at + needle.length;
  }
  if (from < line.length) host.append(line.slice(from));
}

/* A content hit is three facts — which file, which line, what the line says.
 * The number is the one the viewer's own gutter shows, so the eye carries
 * from this list into the file without recounting. */
function paintTextHits(hits, query) {
  const rows = document.createDocumentFragment();
  for (const hit of hits) {
    const row = document.createElement("div");
    row.className = "text-hit";
    row.innerHTML =
      '<span class="hit-path"></span><span class="hit-line"></span>' +
      '<span class="hit-text"></span>';
    row.querySelector(".hit-path").textContent = hit.path;
    row.querySelector(".hit-line").textContent = hit.line;
    paintNeedle(row.querySelector(".hit-text"), hit.text, query);
    row.dataset.tip = `${hit.path}:${hit.line}`;
    actsAsButton(row, () => openPath(hit.path, { preview: true }));
    rows.appendChild(row);
  }
  if (hits.length === 0) {
    const empty = document.createElement("p");
    empty.className = "hit-empty";
    // Says where it looked, because the surprising part of an empty content
    // search is usually the part that was skipped.
    empty.textContent = t("file.notFound", "이 워크트리에서 찾지 못했습니다 — target·node_modules는 건너뜁니다.");
    rows.appendChild(empty);
  }
  fileTextHits.replaceChildren(rows);
  fileTextHits.hidden = false;
}

fileSearch.addEventListener("input", () => scheduleFileSearch());

el("file-mode-name").addEventListener("click", () => setFileSearchMode("name"));
el("file-mode-text").addEventListener("click", () => setFileSearchMode("text"));

/* ---- the finder (⌘P a file, ⌘J a worktree) ----
 *
 * One surface, two questions. Orca has no general command palette: ⌘P opens a
 * file and ⌘J jumps between worktrees (measured, 1-d), and that is the right
 * shape — this is not a command bar that grew a file mode, it is a finder
 * that knows what it is finding. A mode owns three things and nothing else in
 * here branches: what it is called, where its rows come from, and what Enter
 * does with one. */

const qoScrim = el("qo-scrim");
const qoTitle = el("qo-title");
const qoInput = el("qo-input");
const qoResults = el("qo-results");
const qoEmpty = el("qo-empty");
/* The quick finder waits just long enough to collapse a typing burst without
 * making a keyboard-driven palette feel detached from the field. */
const PALETTE_QUERY_DEBOUNCE_MS = 150;
let qoTimer = null;
let paletteMode = null;

/* The rows on screen, in the order they are drawn.
 *
 * Kept beside the DOM rather than read back out of it: a row's label is
 * written to be read — a branch name, a shortened path — and the thing it
 * would act on is not always that string. Letting the text be the identity is
 * the mistake the removal dialog already paid for once. */
let paletteRows = [];

/* Which lane colour a worktree wears here: the slot of the lane running in
 * it, or none. Lane colour is the one thread of identity here, so a surface
 * quotes it instead of introducing a second accent. */
function laneSlotOf(worktreePath) {
  for (const id of order) {
    const entry = lanes.get(id);
    if (entry?.lane.worktree_id === worktreePath) return slotClass(id);
  }
  return null;
}

const FINDERS = {
  file: {
    title: () => t("file.goto", "파일로 이동"),
    placeholder: () => t("file.namePlaceholder", "파일 이름"),
    // A project is too big to list unasked, so this one waits for a word.
    listsEmpty: false,
    find: (query) => invoke("search_files", { query }),
    row: (hit) => ({ label: hit, hint: "", lane: null, run: () => openPath(hit, { preview: true }) }),
  },
  worktree: {
    title: () => t("worktree.jump", "워크트리로 이동"),
    placeholder: () => t("worktree.pathHint", "브랜치 또는 경로"),
    // The jump list is short and known up front, so it is on screen before a
    // key is pressed — which is what makes it a jump rather than a search.
    listsEmpty: true,
    find: async (query) => {
      const worktrees = await invoke("list_worktrees");
      const needle = query.toLowerCase();
      // The agents nested inside each workspace stay searchable — the tree
      // filter that used to walk `.wt-children` rows died with its field, and
      // a session you can name must still surface the workspace it lives in.
      // There is no global session container to consult (none in the
      // original either), so the lanes are folded per workspace here.
      const laneWords = new Map();
      for (const id of order) {
        const entry = lanes.get(id);
        if (!entry) continue;
        const at = entry.lane.worktree_id;
        laneWords.set(at, `${laneWords.get(at) ?? ""} ${laneTitle(entry.lane).toLowerCase()}`);
      }
      return worktrees.filter(
        (worktree) =>
          (worktree.branch ?? "").toLowerCase().includes(needle) ||
          worktree.path.toLowerCase().includes(needle) ||
          (laneWords.get(worktree.path) ?? "").includes(needle),
      );
    },
    row: (worktree) => ({
      label: worktree.branch ?? basename(worktree.path),
      hint: worktree.is_main && !worktree.is_folder
        ? t("worktree.main", "기본")
        : basename(worktree.path),
      lane: laneSlotOf(worktree.path),
      run: () => activateWorktree(worktree.path),
    }),
  },
};

function openPalette(mode) {
  const finder = FINDERS[mode];
  if (!finder) return;
  const opener = document.activeElement;
  // Both float on the same scrim, so two of them open at once is two dialogs
  // stacked on one darkening layer with only the later one reachable.
  setSettingsOpen(false);
  setTaskOpen(false);
  paletteMode = mode;
  qoScrim.dataset.finder = mode;
  el("qo-finder-mark-icon").setAttribute(
    "href",
    mode === "worktree" ? "#i-branch" : "#i-file",
  );
  // Already a way to produce the words rather than the words — which is
  // exactly what `say` wants, so the title follows a language change instead
  // of reverting to whichever finder the markup happened to name.
  say(qoTitle, finder.title);
  qoInput.placeholder = finder.placeholder();
  qoInput.value = "";
  qoResults.replaceChildren();
  // Opening is not an answer, so a fresh panel does not claim it found nothing.
  qoEmpty.hidden = true;
  paletteRows = [];
  showModal(qoScrim, { animated: true, opener });
  paintNavCurrent();
  markFirstRun("used_palette");
  if (finder.listsEmpty) runPaletteQuery();
}

function closePalette() {
  hideModal(qoScrim, { animated: true });
  paletteMode = null;
  paletteRows = [];
  paintNavCurrent();
}

/* Pressing a finder's own chord again closes it; pressing the other one
 * switches, because two finders stacked on one scrim is nobody's intent. */
function togglePalette(mode) {
  if (!qoScrim.hidden && paletteMode === mode) closePalette();
  else openPalette(mode);
}

async function runPaletteQuery() {
  const finder = FINDERS[paletteMode];
  if (!finder) return;
  const asked = paletteMode;
  const query = qoInput.value.trim();
  if (!query && !finder.listsEmpty) {
    qoResults.replaceChildren();
    qoEmpty.hidden = true;
    paletteRows = [];
    return;
  }
  let found = [];
  try {
    found = await finder.find(query);
  } catch (error) {
    showError(error);
    return;
  }
  // The finder changed — or closed — while this was in flight; its rows
  // belong to a question nobody is asking now.
  if (paletteMode !== asked) return;
  qoResults.replaceChildren();
  paletteRows = found.slice(0, 50).map((item) => finder.row(item));
  paletteRows.forEach((row, index) => {
    const node = document.createElement("div");
    node.className = `qo-row${index === 0 ? " is-sel" : ""}${row.lane ? ` ${row.lane} has-lane` : ""}`;
    node.id = `qo-row-${index}`;
    node.setAttribute("role", "option");
    node.setAttribute("aria-selected", index === 0 ? "true" : "false");
    const icon = asked === "worktree" ? "#i-branch" : "#i-file";
    node.innerHTML = `<svg class="icon qo-row-icon" aria-hidden="true"><use href="${icon}"></use></svg>` +
      '<span class="qo-label"></span><span class="qo-hint"></span>';
    node.querySelector(".qo-label").textContent = row.label;
    node.querySelector(".qo-hint").textContent = row.hint;
    node.dataset.tip = row.label;
    node.addEventListener("click", () => commitPaletteRow(index));
    qoResults.appendChild(node);
  });
  qoEmpty.hidden = paletteRows.length > 0;
  announceSelectedRow();
}

/* Point the field at the row the arrows have landed on. The field keeps the
 * focus throughout — that is what makes this a list you drive rather than a
 * list you tab through — so this attribute is the only thing that tells a
 * screen reader which row Enter would take. */
function announceSelectedRow() {
  const selected = qoSelected();
  if (selected) qoInput.setAttribute("aria-activedescendant", selected.id);
  else qoInput.removeAttribute("aria-activedescendant");
}

function commitPaletteRow(index) {
  const row = paletteRows[index];
  if (!row) return;
  closePalette();
  row.run();
}

function qoSelected() {
  return qoResults.querySelector(".is-sel");
}

function qoMove(step) {
  const rows = [...qoResults.children];
  if (rows.length === 0) return;
  const current = qoSelected();
  let index = current ? rows.indexOf(current) + step : 0;
  index = Math.max(0, Math.min(rows.length - 1, index));
  current?.classList.remove("is-sel");
  current?.setAttribute("aria-selected", "false");
  rows[index].classList.add("is-sel");
  rows[index].setAttribute("aria-selected", "true");
  rows[index].scrollIntoView({ block: "nearest" });
  announceSelectedRow();
}

qoInput.addEventListener("input", () => {
  clearTimeout(qoTimer);
  qoTimer = setTimeout(runPaletteQuery, PALETTE_QUERY_DEBOUNCE_MS);
});

qoInput.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") {
    event.preventDefault();
    qoMove(1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    qoMove(-1);
  } else if (event.key === "Enter") {
    event.preventDefault();
    const selected = qoSelected();
    if (selected) commitPaletteRow([...qoResults.children].indexOf(selected));
  } else if (event.key === "Escape") {
    event.preventDefault();
    closePalette();
  }
});

qoScrim.addEventListener("mousedown", (event) => {
  if (event.target === qoScrim) closePalette();
});
