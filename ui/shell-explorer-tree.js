/* Selection and drag preview use the existing lazy tree. Mutations and their
 * inverse are owned by one backend log; the window never replays paths. */
const selectedTreePaths = new Set();
let treeSelectionAnchor = null;
let treeDrag = null;
let treeDropProbe = null;
let treeHistoryBusy = false;
let treeUndoTimer = null;

function resetTreeSelection() {
  selectedTreePaths.clear();
  treeSelectionAnchor = null;
  finishTreeDrag();
  scheduleTreeSelectionReport();
}

function paintTreeSelection() {
  for (const row of fileTree.querySelectorAll('.tree-row[data-tree-path]')) {
    const selected = selectedTreePaths.has(row.dataset.treePath);
    row.classList.toggle('is-selected', selected);
    // A treeitem says it is chosen with aria-selected (t-24298); pressed is a
    // toggle button's word and was never true of a row.
    row.setAttribute('aria-selected', String(selected));
  }
  scheduleTreeSelectionReport();
}

function selectTreeRow(row, event) {
  const path = row.dataset.treePath;
  if (event.shiftKey && treeSelectionAnchor) {
    const visible = [...fileTree.querySelectorAll('.tree-row[data-tree-path]')].filter((held) => !held.closest('.tree-children[hidden]'));
    const from = visible.findIndex((held) => held.dataset.treePath === treeSelectionAnchor);
    const to = visible.indexOf(row);
    if (from >= 0 && to >= 0) {
      if (!hasPrimaryModifier(event)) selectedTreePaths.clear();
      for (const held of visible.slice(Math.min(from, to), Math.max(from, to) + 1)) selectedTreePaths.add(held.dataset.treePath);
    }
  } else if (hasPrimaryModifier(event)) {
    if (selectedTreePaths.has(path)) selectedTreePaths.delete(path);
    else selectedTreePaths.add(path);
    treeSelectionAnchor = path;
  } else {
    selectedTreePaths.clear();
    selectedTreePaths.add(path);
    treeSelectionAnchor = path;
  }
  paintTreeSelection();
  clearTreeReveal(row);
  setTreeCursor(row);
  row.focus({ preventScroll: true });
}

function treeDragPaths() {
  // Selecting a directory already includes its descendants on disk.
  return [...selectedTreePaths].filter((path, _, paths) => !paths.some((parent) => parent !== path && path.startsWith(`${parent}/`)));
}

function bindExplorerRow(row, relative, entry) {
  row.dataset.treePath = relative;
  row.draggable = true;
  row.classList.toggle('is-selected', selectedTreePaths.has(relative));
  row.addEventListener('click', (event) => {
    if (event.target.closest('input, button')) return;
    selectTreeRow(row, event);
    if (hasPrimaryModifier(event) || event.shiftKey) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);
  row.addEventListener('dragstart', (event) => {
    if (event.target.closest('input')) { event.preventDefault(); return; }
    if (!selectedTreePaths.has(relative)) selectTreeRow(row, event);
    const paths = treeDragPaths();
    if (!explorerPolicy || paths.length > explorerPolicy.move_cap) { event.preventDefault(); return; }
    treeDrag = { paths, root: activeWorktreePath };
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('application/x-zerocode-tree', JSON.stringify(treeDrag));
    event.dataTransfer.setData('text/plain', paths.map(treeAbsolute).join('\n'));
  });
  row.addEventListener('dragend', finishTreeDrag);
  if (entry.is_dir) {
    row.addEventListener('dragover', (event) => previewTreeDrop(event, row, relative));
    row.addEventListener('dragleave', (event) => {
      if (row.contains(event.relatedTarget)) return;
      row.classList.remove('is-drop-target', 'is-drop-conflict');
    });
    row.addEventListener('drop', (event) => void dropTreePaths(event, relative));
  }
}

function previewTreeDrop(event, row, dir) {
  if (!treeDrag || treeDrag.root !== activeWorktreePath) return;
  event.preventDefault();
  event.stopPropagation();
  const paths = treeDrag.paths;
  const invalid = paths.some((path) => path === dir || dir.startsWith(`${path}/`) || path.slice(0, Math.max(0, path.lastIndexOf('/'))) === dir);
  const names = paths.map(basename);
  const duplicates = new Set(names).size !== names.length;
  if (!treeDropProbe || treeDropProbe.dir !== dir) {
    const probe = { dir, conflict: invalid || duplicates, pending: true };
    treeDropProbe = probe;
    invoke('list_dir', { path: dir }).then((entries) => {
      if (treeDropProbe !== probe) return;
      probe.conflict ||= entries.some((entry) => names.includes(entry.name));
      probe.pending = false;
      paintTreeDrop(row);
    }).catch(() => { if (treeDropProbe === probe) { probe.conflict = true; probe.pending = false; paintTreeDrop(row); } });
  }
  event.dataTransfer.dropEffect = treeDropProbe.conflict || treeDropProbe.pending ? 'none' : 'move';
  paintTreeDrop(row);
}

function paintTreeDrop(row) {
  if (!treeDrag || !treeDropProbe) return;
  fileTree.classList.remove('is-drop-target', 'is-drop-conflict');
  for (const held of fileTree.querySelectorAll('.is-drop-target, .is-drop-conflict')) held.classList.remove('is-drop-target', 'is-drop-conflict');
  row.classList.add(treeDropProbe.conflict ? 'is-drop-conflict' : 'is-drop-target');
  const badge = el("tree-drag-badge");
  badge.textContent = t("tree.dragPreview", "{{count}}개 → {{folder}}").replace('{{count}}', String(treeDrag.paths.length)).replace('{{folder}}', treeDropProbe.dir || t("file.projectRoot", "프로젝트 루트"));
  if (treeDropProbe.conflict) badge.textContent += ` · ${t("tree.dragConflict", "같은 이름 또는 이동할 수 없는 경로")}`;
  else if (treeDropProbe.pending) badge.textContent += ` · ${t("tree.dragChecking", "확인 중…")}`;
  badge.classList.toggle('is-conflict', treeDropProbe.conflict);
  badge.hidden = false;
}

async function dropTreePaths(event, dir) {
  event.preventDefault();
  event.stopPropagation();
  if (!treeDrag || treeDrag.root !== activeWorktreePath || treeDropProbe?.dir !== dir || treeDropProbe.conflict || treeDropProbe.pending) { finishTreeDrag(); return; }
  const drag = treeDrag;
  finishTreeDrag();
  try {
    const operation = await invoke("fs_move", { paths: drag.paths.map((path) => `${drag.root}/${path}`), dir: `${drag.root}/${dir}`, root: drag.root });
    if (activeWorktreePath !== drag.root) return;
    selectedTreePaths.clear();
    scheduleTreeSelectionReport();
    await loadTree(fileTree, '');
    fileTree.focus({ preventScroll: true });
    showTreeUndo(operation);
  } catch (error) { showError(treeOperationError(error)); }
}

function finishTreeDrag() {
  treeDrag = null;
  treeDropProbe = null;
  el("tree-drag-badge").hidden = true;
  fileTree.classList.remove('is-drop-target', 'is-drop-conflict');
  for (const row of fileTree.querySelectorAll('.is-drop-target, .is-drop-conflict')) row.classList.remove('is-drop-target', 'is-drop-conflict');
}

function explorerOwnsShortcut() {
  const active = document.activeElement;
  return !terminalOwnsShortcutContext() && !!active?.closest('#activity-files') && !active.closest('input, textarea, [contenteditable="true"]');
}

function treeOperationError(error) {
  const messages = {
    'tree.conflict': t("tree.conflict", "그 자리에 다른 파일이 있어 작업하지 않았습니다."),
    'tree.changed': t("tree.changed", "파일이 다른 파일로 바뀌었거나 없어져 작업하지 않았습니다."),
    'tree.emptyHistory': t("tree.emptyHistory", "이 워크트리에 되돌릴 작업이 없습니다."),
    'tree.invalidMove': t("tree.invalidMove", "이 경로로는 파일을 옮길 수 없습니다."),
    'tree.tooMany': t("tree.tooMany", "선택한 파일 수가 작업 한도를 넘었습니다."),
    'tree.failed': t("tree.failed", "파일 작업을 마치지 못했습니다. 경로와 권한을 확인하세요."),
    'tree.rollbackFailed': t("tree.rollbackFailed", "일부 파일을 되돌리지 못했습니다. 되돌리기로 남은 파일을 복원하세요."),
  };
  return messages[String(error)] ?? messages['tree.failed'];
}

function showTreeUndo(operation) {
  if (!operation) return;
  const labels = {
    move: t("tree.undoMove", "옮김 되돌리기"), rename: t("tree.undoRename", "이름 변경 되돌리기"),
    trash: t("tree.undoTrash", "삭제 되돌리기"), create: t("tree.undoCreate", "만들기 되돌리기"), duplicate: t("tree.undoDuplicate", "복제 되돌리기"),
  };
  el("tree-undo-label").textContent = labels[operation.kind] ?? t("tree.undo", "되돌리기");
  el("tree-undo-origin").textContent = operation.by?.kind === 'agent'
    ? t("tree.originAgent", "에이전트 · {{agent}} · {{pane}}").replace('{{agent}}', operation.by.agent).replace('{{pane}}', operation.by.pane)
    : t("tree.originHuman", "사람");
  const button = el("tree-undo-button");
  button.textContent = operation.redo ? t("tree.redo", "다시 실행") : t("tree.undo", "되돌리기");
  button.dataset.command = operation.redo ? "fs_redo" : "fs_undo";
  el("tree-undo-toast").hidden = false;
  clearTimeout(treeUndoTimer);
  if (explorerPolicy) treeUndoTimer = setTimeout(() => { el("tree-undo-toast").hidden = true; }, explorerPolicy.toast_ms);
}

async function runTreeHistory(command) {
  if (treeHistoryBusy) return;
  const root = activeWorktreePath;
  treeHistoryBusy = true;
  try {
    const operation = await invoke(command, { root });
    if (root !== activeWorktreePath) return;
    await loadTree(fileTree, '');
    fileTree.focus({ preventScroll: true });
    showTreeUndo({ ...operation, redo: command === "fs_undo" });
  } catch (error) { showError(treeOperationError(error)); }
  finally { treeHistoryBusy = false; }
}
el("tree-undo-button").addEventListener('click', () => void runTreeHistory(el("tree-undo-button").dataset.command));
listen('tree:operation', (event) => {
  if (event.payload.root !== activeWorktreePath) return;
  showTreeUndo(event.payload.operation);
  if (event.payload.operation.by?.kind === 'agent') void loadTree(fileTree, '');
});

// The tree's empty ground is its root — the workspace's, or the folder the
// person pinned (t-24298).
fileTree.addEventListener('dragover', (event) => {
  if (event.target.closest('.tree-row')) return;
  previewTreeDrop(event, fileTree, treePinnedRoot());
});
fileTree.addEventListener('drop', (event) => {
  if (event.target.closest('.tree-row')) return;
  void dropTreePaths(event, treePinnedRoot());
});

/* ---- what the agents are touching, worn by the tree (t-24298) ----
 *
 * The behaviour is the claude-code-filetree mod's (MIT, read for what it does
 * and not copied): a file an agent reads, writes or commits lights up in its
 * own colour, the collapsed folders above it carry that colour too, and with
 * "follow" on the tree unfolds to the file. The mod hears Claude alone; this
 * tree hears every agent through the one road their activity already travels
 * — the `hook:activity` batches the sidebar draws its activity line from,
 * whose verbs are the backend's one table of tool names (`Tool::as_str`,
 * zerocode-core hook.rs). So there is no agent's name below, and an agent
 * that reports a tool call is an agent this tree can follow.
 *
 * Only the active workspace's tree is painted, and only paths inside it: an
 * agent in another checkout is that checkout's news. */

/* How long a touched file stays lit. Agents space their tool calls a few
 * seconds apart, so a glance that comes over from the terminal a call later
 * still finds the file — and a busy turn lights its last file or two rather
 * than its whole trail. */
const TREE_TOUCH_HOLD_MS = 6_000;

/* The most files the tree holds lit at once. A search can name hundreds, and
 * a tree lit everywhere is pointing at nothing; past this the oldest go
 * first. It also bounds what one paint walks. */
const TREE_TOUCH_CAP = 64;

/* Which kind of touch a verb is. Searches are reads; edits and writes are
 * writes. A verb not here (a shell command, a fetch, a delegation) names no
 * file of its own. */
const TREE_TOUCH_OF_VERB = Object.freeze({ read: "read", grep: "read", edit: "write", write: "write" });

/* The kinds, weakest first: a folder holding a read and a write wears the
 * write, and a file committed while lit stays the commit's colour. */
const TREE_TOUCH_RANK = Object.freeze({ read: 1, write: 2, commit: 3 });

/* A search's target is usually its pattern, not a path (the backend puts
 * `pattern` ahead of `path` on purpose — the row wants what was looked for).
 * A pattern resolved as a path would unfold folders for nothing, so a search
 * lights only a target that reads as a path. */
const TREE_PATTERN_SIGNS = /[\s*?[\]{}()|^$\\]/;

const TREE_FOLLOW_KEY = "zerocode.explorer-follow.v1";

/* relative path → { kind, until }, newest last. Relative to `treeTouchRoot`,
 * so a workspace switch drops what another checkout's agents did. */
const treeTouches = new Map();
let treeTouchRoot = null;
let treeTouchFrame = 0;
let treeTouchSweep = null;
/* How many times the tree's marks were painted — a burst of activity paints
 * once a frame, and the harness counts it (`explorer-agent-burst`). */
let treeTouchPaints = 0;

/* Where each pane's process stands, from the backend's `term:cwd` — the base
 * a relative path in that pane's tool calls is read from. Bounded by the
 * panes open: a pane no tab holds is dropped when the map grows past them. */
const treeTermCwds = new Map();
const TREE_CWD_KEEP = 64;

let treeFollowsAgent = readTreeFollow();

function readTreeFollow() {
  try { return localStorage.getItem(TREE_FOLLOW_KEY) !== "off"; }
  catch { return true; }
}

/* An explorer preference, kept where the explorer keeps its search filters:
 * a view choice of this window that the backend has no reason to hold. */
function setTreeFollow(on) {
  treeFollowsAgent = on;
  try { localStorage.setItem(TREE_FOLLOW_KEY, on ? "on" : "off"); }
  catch { /* Storage refusal keeps the choice for this session. */ }
  paintTreeFollow();
}

function paintTreeFollow() {
  const button = el("tree-follow");
  button.setAttribute("aria-pressed", String(treeFollowsAgent));
  button.classList.toggle("is-active", treeFollowsAgent);
}

el("tree-follow").addEventListener("click", () => setTreeFollow(!treeFollowsAgent));
paintTreeFollow();

listen("term:cwd", (event) => {
  const { term, cwd } = event.payload ?? {};
  if (typeof term !== "number" || typeof cwd !== "string" || cwd.length === 0) return;
  treeTermCwds.set(term, cwd);
  followAgentFolder(term, cwd);
  if (treeTermCwds.size <= TREE_CWD_KEEP) return;
  for (const held of [...treeTermCwds.keys()]) if (!tabOfTerm(held)) treeTermCwds.delete(held);
});

/* Is the tree where somebody can see it — its panel up, its column open, no
 * search list standing in its place. */
function fileTreeShowing() {
  return !el("activity-files").hidden && !folded.aside && !fileTree.hidden;
}

function treeRowOf(relative) {
  return fileTree.querySelector(`.tree-row[data-tree-path="${CSS.escape(relative)}"]`);
}

/* The folder a pane's relative paths are read from: where its process stands
 * when that is inside the pane's own workspace, else the workspace itself. A
 * helper's card (`sub:<term>:<id>`) is its parent pane's. A pane no tab
 * holds has no base, and its relative paths resolve to nothing. */
function treePaneBase(pane) {
  const home = treePaneHome(pane);
  if (!home) return null;
  const cwd = treeTermCwds.get(treePaneTerm(pane));
  return cwd && (cwd === home || cwd.startsWith(`${home}/`)) ? cwd : home;
}

/* The pane an activity card belongs to: `term:<n>`, or a helper's
 * `sub:<n>:<id>`, which is its parent's. */
function treePaneTerm(pane) {
  const term = Number(/^(?:term|sub):(\d+)(?::|$)/.exec(pane ?? "")?.[1]);
  return Number.isInteger(term) ? term : null;
}

/* A target as a path relative to the active workspace, or null — outside
 * it, climbing out of it, clamped by the backend (`…`, the row's 120-character
 * cut), or relative with nowhere to be relative to. `.` and `..` are walked
 * here because an agent writes them and the tree's rows never do. On Windows
 * an agent writes backslashes and a drive, and the tree's own paths are
 * forward-slashed (`treeAbsolute`), so both are read in that one spelling. */
function treeSlashes(path) {
  return usesWindowsPlatform ? path.replace(/\\/g, "/") : path;
}

function treeRelative(target, pane, cwd = null) {
  const root = treeSlashes(activeWorktreePath ?? "").replace(/\/+$/, "");
  if (!root || typeof target !== "string" || target.length === 0 || target.endsWith("…")) return null;
  const wanted = treeSlashes(target);
  const absolute = wanted.startsWith("/") || (usesWindowsPlatform && /^[A-Za-z]:\//.test(wanted));
  // A call that said where it ran (`activity.cwd`) is read from there; the
  // pane's own folder is the fallback.
  const base = absolute ? null : (typeof cwd === "string" && cwd ? cwd : treePaneBase(pane));
  if (!absolute && base === null) return null;
  const joined = absolute ? wanted : `${treeSlashes(base)}/${wanted}`;
  const parts = [];
  for (const part of joined.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
      continue;
    }
    parts.push(part);
  }
  const walked = `${joined.startsWith("/") ? "/" : ""}${parts.join("/")}`;
  return walked.startsWith(`${root}/`) ? walked.slice(root.length + 1) : null;
}

/* What one activity touched: each file it names with the kind of touch. A
 * call that failed touched nothing; a prompt and a turn's end are not tool
 * calls. The backend reads a shell command's reads and a patch's files with
 * the shell's grammar (`reads`, `writes`); when it named those, the target is
 * the command or the patch itself and names no file. */
function treeTouchesOf(activity) {
  if (activity.phase !== "started" && activity.phase !== "finished") return [];
  const touches = [];
  const named = (activity.reads?.length ?? 0) + (activity.writes?.length ?? 0) > 0;
  const kind = TREE_TOUCH_OF_VERB[activity.verb];
  if (kind && !named && typeof activity.target === "string" &&
      !(activity.verb === "grep" && TREE_PATTERN_SIGNS.test(activity.target))) {
    touches.push({ kind, target: activity.target });
  }
  for (const target of activity.reads ?? []) touches.push({ kind: "read", target });
  for (const target of activity.writes ?? []) touches.push({ kind: "write", target });
  return touches;
}

function touchTree(relative, kind, now) {
  const held = treeTouches.get(relative);
  const keep = held && held.until > now && TREE_TOUCH_RANK[held.kind] > TREE_TOUCH_RANK[kind];
  treeTouches.delete(relative);
  treeTouches.set(relative, { kind: keep ? held.kind : kind, until: now + TREE_TOUCH_HOLD_MS });
  while (treeTouches.size > TREE_TOUCH_CAP) treeTouches.delete(treeTouches.keys().next().value);
}

/* One batch of what an agent did (`hook:activity`, ui/shell.js). Cheap by
 * construction: it only writes the map and asks for a frame — the paint and
 * the unfolding happen once per frame however many events arrived. */
function noteTreeActivities(pane, activities) {
  if (!activeWorktreePath) return;
  if (treeTouchRoot !== activeWorktreePath) {
    treeTouches.clear();
    treeTouchRoot = activeWorktreePath;
  }
  const now = performance.now();
  let newest = null;
  for (const stamped of activities) {
    const activity = stamped?.activity;
    if (!activity) continue;
    if (activity.phase === "prompted") {
      revealTreeMentions(pane, activity.target);
      continue;
    }
    noteTreeVcs(pane, activity, now);
    for (const { kind, target } of treeTouchesOf(activity)) {
      const relative = treeRelative(target, pane, activity.cwd);
      if (relative === null) continue;
      touchTree(relative, kind, now);
      newest = relative;
    }
  }
  if (newest === null) return;
  scheduleTreeTouchPaint();
  if (treeTouchSweep === null) armTreeTouchSweep();
  if (treeFollowsAgent && fileTreeShowing()) followTreeTo(newest);
}

function scheduleTreeTouchPaint() {
  if (treeTouchFrame !== 0) return;
  treeTouchFrame = requestAnimationFrame(() => {
    treeTouchFrame = 0;
    paintTreeTouches();
  });
}

/* Every lit row, set from the map in one pass of writes. A file's row wears
 * its kind; each collapsed folder above it wears the strongest kind beneath
 * it, which is how a file the tree has not unfolded to still says where it
 * is. Rows built later (a folder opened, the tree reloaded) are dressed by
 * the next paint, which `loadTree` asks for. */
function paintTreeTouches() {
  treeTouchPaints += 1;
  const wanted = new Map();
  const want = (row, slot, kind) => {
    const marks = wanted.get(row) ?? {};
    if (!marks[slot] || TREE_TOUCH_RANK[kind] > TREE_TOUCH_RANK[marks[slot]]) marks[slot] = kind;
    wanted.set(row, marks);
  };
  if (treeTouchRoot === activeWorktreePath) {
    for (const [relative, held] of treeTouches) {
      const row = treeRowOf(relative);
      if (row) want(row, "agentTouch", held.kind);
      const parts = relative.split("/");
      for (let depth = parts.length - 1; depth >= 1; depth -= 1) {
        const folder = treeRowOf(parts.slice(0, depth).join("/"));
        if (folder && folder.getAttribute("aria-expanded") !== "true") want(folder, "agentTouchWithin", held.kind);
      }
    }
  }
  for (const row of fileTree.querySelectorAll("[data-agent-touch], [data-agent-touch-within]")) {
    if (wanted.has(row)) continue;
    delete row.dataset.agentTouch;
    delete row.dataset.agentTouchWithin;
    labelTreeRow(row);
  }
  for (const [row, marks] of wanted) {
    let moved = false;
    for (const slot of ["agentTouch", "agentTouchWithin"]) {
      if (marks[slot]) {
        if (row.dataset[slot] === marks[slot]) continue;
        row.dataset[slot] = marks[slot];
        moved = true;
      } else if (slot in row.dataset) {
        delete row.dataset[slot];
        moved = true;
      }
    }
    if (moved) labelTreeRow(row);
  }
}

/* One timer for every mark: armed for the soonest expiry while any is held,
 * and gone when none is. */
function armTreeTouchSweep() {
  treeTouchSweep = null;
  let soonest = Infinity;
  for (const held of treeTouches.values()) soonest = Math.min(soonest, held.until);
  if (soonest === Infinity) return;
  treeTouchSweep = setTimeout(sweepTreeTouches, Math.max(0, soonest - performance.now()));
}

function sweepTreeTouches() {
  const now = performance.now();
  for (const [relative, held] of treeTouches) if (held.until <= now) treeTouches.delete(relative);
  scheduleTreeTouchPaint();
  armTreeTouchSweep();
}

/* Unfold the tree down to `relative` and bring its row into view, opening
 * each folder through the row's own door (`_treeUnfold`, which lists a
 * folder once). Shared by every road that points the tree at a path — the
 * agent being followed, an `@path` the person wrote, a menu's "show in the
 * tree". The row it reaches, or null when the path is not in this tree. */
async function revealInTree(relative, { scroll = true } = {}) {
  const root = activeWorktreePath;
  const pinned = treePinnedRoot();
  // Outside a pinned root there is no row to reach, and nothing to list.
  if (pinned && !relative.startsWith(`${pinned}/`)) return null;
  const parts = relative.split("/");
  for (let depth = treeRootDepth() + 1; depth < parts.length; depth += 1) {
    const folder = treeRowOf(parts.slice(0, depth).join("/"));
    if (!folder?._treeUnfold) return null;
    await folder._treeUnfold(true);
    if (root !== activeWorktreePath) return null;
  }
  const row = treeRowOf(relative);
  if (row && scroll) row.scrollIntoView({ block: "nearest" });
  return row;
}

/* Follow the newest touch, one walk at a time: a burst that names twenty
 * files while a folder is being listed walks to the last of them once,
 * rather than twenty walks racing each other's scrolls. */
let treeFollowTarget = null;
let treeFollowing = false;

function followTreeTo(relative) {
  treeFollowTarget = relative;
  if (treeFollowing) return;
  treeFollowing = true;
  void (async () => {
    try {
      while (treeFollowTarget !== null) {
        const next = treeFollowTarget;
        treeFollowTarget = null;
        await revealInTree(next);
      }
    } finally {
      treeFollowing = false;
      scheduleTreeTouchPaint();
    }
  })();
}

/* ---- git on the tree's rows (t-24298) ----
 *
 * The mod prints a changed file's exact lines (+N -N), counts each kind of
 * change on a folder, and puts the branch with its upstream and ahead/behind
 * at the top. Every number here is one `scm_status` and `upstream_status`
 * already answer for the source-control panel — numstat rides the status —
 * so the tree asks git for nothing of its own. */

/* A folder's roll-up, strongest first: what a folder says when it holds more
 * than one kind of change. A conflict stops work; a deletion loses work; a
 * modification and an addition are work in progress; an untracked file is
 * not yet anyone's business. The letters are the explorer's own (`U` is
 * Orca's untracked) with git clients' `!` for a conflict, so a chip and a
 * file row never use one letter for two things. A rename counts as the
 * modification it is, a copy as the addition. */
const TREE_GIT_ROLLUP = Object.freeze([
  Object.freeze({ kind: "conflict", letter: "!" }),
  Object.freeze({ kind: "deleted", letter: "D" }),
  Object.freeze({ kind: "modified", letter: "M" }),
  Object.freeze({ kind: "added", letter: "A" }),
  Object.freeze({ kind: "untracked", letter: "U" }),
]);
const TREE_GIT_FOLD = Object.freeze({ renamed: "modified", copied: "added" });

/* The last answer, indexed once per answer rather than once per row: the
 * entry behind each changed file, and the counts beneath each folder. */
let treeGit = { files: new Map(), folders: new Map() };

function treeGitKind(entry) {
  if (entry.conflict) return "conflict";
  const decoration = gitDecorationOf(entry.code);
  return TREE_GIT_FOLD[decoration] ?? decoration;
}

function indexTreeGit() {
  const files = new Map();
  const folders = new Map();
  for (const entry of scmEntries) {
    // git names an untracked folder once, with its slash; the row is the
    // folder's own, so the slash goes.
    const path = entry.path.replace(/\/+$/, "");
    files.set(path, entry);
    const kind = treeGitKind(entry);
    if (!kind) continue;
    const parts = path.split("/");
    for (let depth = 1; depth <= parts.length; depth += 1) {
      const folder = parts.slice(0, depth).join("/");
      if (depth === parts.length && !entry.path.endsWith("/")) break;
      const counts = folders.get(folder) ?? {};
      counts[kind] = (counts[kind] ?? 0) + 1;
      folders.set(folder, counts);
    }
  }
  treeGit = { files, folders };
}

function treeGitWord(kind) {
  switch (kind) {
    case "conflict":
      return t("tree.git.conflict", "충돌");
    case "deleted":
      return t("tree.git.deleted", "삭제됨");
    case "modified":
      return t("tree.git.modified", "수정됨");
    case "added":
      return t("tree.git.added", "추가됨");
    case "untracked":
      return t("tree.git.untracked", "추적 안 됨");
    default:
      return "";
  }
}

/* The part of a row's git face the badge letter does not say: a file's +N -N
 * (the source-control row's own builder, `paintScmTally`), a conflict's
 * colour, and a folder's counts of each kind beneath it in the roll-up's
 * order, the folder wearing the strongest. */
function dressTreeGit(row, relative, isDir, ignored) {
  const badge = row.querySelector(".badge");
  const tally = row.querySelector(".tree-tally");
  tally?.replaceChildren();
  if (ignored) return;
  if (!isDir) {
    const entry = treeGit.files.get(relative);
    if (!entry) return;
    if (tally) paintScmTally(tally, entry);
    if (entry.conflict) badge.dataset.git = "conflict";
    return;
  }
  const counts = treeGit.folders.get(relative);
  if (!counts) return;
  badge.replaceChildren();
  const said = [];
  for (const { kind, letter } of TREE_GIT_ROLLUP) {
    if (!counts[kind]) continue;
    const chip = document.createElement("span");
    chip.className = "tree-count git-ink";
    chip.dataset.git = kind;
    chip.textContent = `${letter}${counts[kind]}`;
    badge.appendChild(chip);
    said.push(`${treeGitWord(kind)} ${counts[kind]}`);
  }
  badge.dataset.git = TREE_GIT_ROLLUP.find(({ kind }) => counts[kind])?.kind ?? "";
  badge.dataset.tip = said.join(" · ");
}

/* A fresh answer, worn by every row on screen — written in place, one pass,
 * no listing (`refreshScm` calls this as soon as `vcsCodes` moves). */
function paintTreeGit() {
  indexTreeGit();
  for (const row of fileTree.querySelectorAll(".tree-row[data-tree-path]")) {
    paintTreeBadge(row, row.dataset.treePath, row.classList.contains("is-dir"));
  }
  settleTreeCommit();
}

/* The last standing each checkout's head was told, by checkout: an answer
 * asked for one workspace is never drawn over another, and coming back to a
 * workspace shows what was last known there at once instead of asking git
 * on the switch. Bounded by the checkouts this window can hold. */
const treeHeadStanding = new Map();
const TREE_HEAD_KEEP = 64;

/* The head: the branch, the upstream it tracks, and how far ahead and
 * behind. `askUpstream` hands it each answer with the checkout it was asked
 * for; a workspace's first listing paints its branch (the catalog's) before
 * git has said anything. A folder that is not a repository has no branch,
 * and the head keeps only its switches. */
function paintTreeHead(asked = null) {
  if (asked !== null) {
    treeHeadStanding.delete(asked);
    treeHeadStanding.set(asked, upstreamState);
    while (treeHeadStanding.size > TREE_HEAD_KEEP) treeHeadStanding.delete(treeHeadStanding.keys().next().value);
  }
  const host = el("tree-head-branch");
  const branch = worktreeAt(activeWorktreePath)?.branch ?? null;
  host.replaceChildren();
  host.hidden = !branch;
  if (!branch) return;
  const standing = treeHeadStanding.get(activeWorktreePath) ?? null;
  const part = (className, text) => {
    const span = document.createElement("span");
    span.className = className;
    span.textContent = text;
    host.appendChild(span);
  };
  host.insertAdjacentHTML("beforeend", icon("branch"));
  part("tree-head-name", branch);
  if (standing?.upstream) {
    part("tree-head-upstream", `→ ${standing.upstream}`);
    if (standing.ahead > 0) part("tree-head-ahead", `↑${standing.ahead}`);
    if (standing.behind > 0) part("tree-head-behind", `↓${standing.behind}`);
  }
  host.dataset.tip = standing?.upstream
    ? t("tree.headTip", "{{branch}} — {{upstream}}보다 {{ahead}}개 앞서고 {{behind}}개 뒤져 있습니다", {
      branch, upstream: standing.upstream, ahead: standing.ahead, behind: standing.behind,
    })
    : branch;
}

/* What `loadTree` calls once it has built a folder's rows: they wear what the
 * tree already knows — the agents' marks, the roles and the one tab stop —
 * and a fresh root wears its branch at once. */
function dressTreeRows(container) {
  for (const row of container.querySelectorAll(":scope > .tree-row[data-tree-path]")) {
    const path = row.dataset.treePath;
    row.setAttribute("role", "treeitem");
    row.setAttribute("aria-level", String(path.split("/").length - treeRootDepth()));
    row.setAttribute("aria-selected", String(selectedTreePaths.has(path)));
    row.tabIndex = path === treeCursorPath ? 0 : -1;
  }
  for (const group of container.querySelectorAll(":scope > .tree-children")) group.setAttribute("role", "group");
  if (container === fileTree) {
    paintTreeHead();
    paintTreePin();
    if (!fileTree.querySelector('.tree-row[tabindex="0"]')) {
      const first = fileTree.querySelector(":scope > .tree-row[data-tree-path]");
      if (first) first.tabIndex = 0;
    }
  }
  if (treeTouches.size > 0) scheduleTreeTouchPaint();
  if (treeAgentFolder !== null) paintAgentFolder();
  if (container === fileTree) paintTreeVcs();
}

/* ---- the keyboard and the screen reader (t-24298) ----
 *
 * The tree is a `tree` of `treeitem`s with one tab stop that moves with the
 * cursor (`treeCursorPath`), the way every file explorer walks: the arrows
 * move through the visible rows and carry the selection, Right opens a
 * folder and then steps into it, Left climbs to the folder and then folds
 * it, Home and End reach the ends, Enter and Space are the row's own door
 * (`actsAsButton`). The tree is marked a keyboard owner, so the window's key
 * sink leaves these keys with it instead of sending them to a terminal. */
let treeCursorPath = null;

function setTreeCursor(row) {
  const path = row.dataset.treePath;
  if (path === treeCursorPath && row.tabIndex === 0) return;
  for (const held of fileTree.querySelectorAll('.tree-row[tabindex="0"]')) if (held !== row) held.tabIndex = -1;
  treeCursorPath = path;
  row.tabIndex = 0;
}

function visibleTreeRows() {
  return [...fileTree.querySelectorAll(".tree-row[data-tree-path]")].filter((row) => !row.closest(".tree-children[hidden]"));
}

fileTree.addEventListener("focusin", (event) => {
  const row = event.target.closest?.(".tree-row[data-tree-path]");
  if (row) setTreeCursor(row);
});

/* The tree itself is focusable for the code that hands it the keyboard (its
 * undo chord, a drop); a person tabbing in lands on the cursor's row. */
fileTree.addEventListener("focus", () => {
  const row = (treeCursorPath === null ? null : treeRowOf(treeCursorPath)) ??
    fileTree.querySelector(":scope > .tree-row[data-tree-path]");
  row?.focus({ preventScroll: true });
});

fileTree.addEventListener("keydown", (event) => {
  const row = event.target.closest?.(".tree-row[data-tree-path]");
  if (!row || event.target.closest("input") || hasPrimaryModifier(event) || event.altKey) return;
  const rows = visibleTreeRows();
  const at = rows.indexOf(row);
  let next = null;
  switch (event.key) {
    case "ArrowDown":
      next = rows[Math.min(rows.length - 1, at + 1)];
      break;
    case "ArrowUp":
      next = rows[Math.max(0, at - 1)];
      break;
    case "Home":
      next = rows[0];
      break;
    case "End":
      next = rows[rows.length - 1];
      break;
    case "ArrowRight":
      if (!row.classList.contains("is-dir")) return;
      if (row.getAttribute("aria-expanded") !== "true") {
        event.preventDefault();
        void row._treeUnfold?.(true);
        return;
      }
      next = row.nextElementSibling?.querySelector(":scope > .tree-row[data-tree-path]") ?? null;
      break;
    case "ArrowLeft":
      if (row.classList.contains("is-dir") && row.getAttribute("aria-expanded") === "true") {
        event.preventDefault();
        void row._treeUnfold?.(false);
        return;
      }
      next = row.parentElement?.closest(".tree-children")?.previousElementSibling ?? null;
      break;
    default:
      return;
  }
  event.preventDefault();
  if (!next || next === row) return;
  selectTreeRow(next, { shiftKey: event.shiftKey });
  next.scrollIntoView({ block: "nearest" });
});

/* A double-click hands the file to the system's own app for it — the click
 * before it already opened the preview here. */
fileTree.addEventListener("dblclick", (event) => {
  const row = event.target.closest?.(".tree-row.is-file[data-tree-path]");
  if (!row || event.target.closest("input")) return;
  invoke("fs_open_default", { path: treeAbsolute(row.dataset.treePath) }).catch((error) => showError(String(error)));
});

function treeTouchWord(kind) {
  switch (kind) {
    case "read":
      return t("tree.touch.read", "에이전트가 읽는 중");
    case "write":
      return t("tree.touch.write", "에이전트가 쓰는 중");
    case "commit":
      return t("tree.touch.commit", "에이전트가 커밋함");
    default:
      return "";
  }
}

/* What a screen reader says for a row: its name, its git state in words
 * (and a file's +N -N), and what an agent is doing to it — the same facts
 * the row shows, never a raw letter. Rewritten whenever one of them moves. */
function labelTreeRow(row) {
  const path = row.dataset.treePath;
  const words = [row.querySelector(".tree-name")?.textContent ?? ""];
  if (row.classList.contains("is-ignored")) {
    words.push(t("tree.git.ignored", "무시됨"));
  } else if (row.classList.contains("is-dir")) {
    if (treeGit.folders.has(path)) words.push(row.querySelector(".badge")?.dataset.tip ?? "");
  } else {
    const entry = treeGit.files.get(path);
    if (entry) words.push(treeGitWord(treeGitKind(entry)), row.querySelector(".tree-tally")?.textContent ?? "");
  }
  words.push(treeTouchWord(row.dataset.agentTouch ?? row.dataset.agentTouchWithin));
  if (row.dataset.agentCwd === "true") words.push(t("tree.agentHere", "에이전트 작업 폴더"));
  row.setAttribute("aria-label", words.filter(Boolean).join(", "));
}

/* ---- the person's own pointer: `@path` in a prompt (t-24298) ----
 *
 * A prompt the person sends is an activity too (the `prompt` verb every hook
 * agent reports), and an `@path` in it names a file the person is thinking
 * about. The tree unfolds to it whether follow is on or not — it is the
 * person's pointer, not the agent's. Read from the start of the line or after
 * a space, so a mail address is not a mention; trailing punctuation is the
 * sentence's, not the path's. */
const TREE_MENTION = /(?:^|\s)@([^\s"'`]+)/g;
const TREE_MENTION_TAIL = /[.,;:!?)\]]+$/;
let treeMentionWalk = Promise.resolve();

function revealTreeMentions(pane, text) {
  if (typeof text !== "string" || !fileTreeShowing()) return;
  for (const match of text.matchAll(TREE_MENTION)) {
    const said = match[1].replace(TREE_MENTION_TAIL, "");
    if (said.endsWith("…")) continue;
    const relative = treeRelative(said, pane);
    if (relative === null) continue;
    treeMentionWalk = treeMentionWalk.then(() => revealTreePath(relative)).catch(() => {});
  }
}

function clearTreeReveal(keep = null) {
  for (const held of fileTree.querySelectorAll(".tree-row.is-revealed")) if (held !== keep) held.classList.remove("is-revealed");
}

/* Unfold to a path, mark its row as the one pointed at and make it the tab
 * stop — focused only when the person asked from a menu, never from a
 * prompt they are still looking past. */
async function revealTreePath(relative, { focus = false } = {}) {
  const row = await revealInTree(relative);
  if (!row) return null;
  clearTreeReveal(row);
  row.classList.add("is-revealed");
  setTreeCursor(row);
  if (focus) row.focus({ preventScroll: true });
  return row;
}

/* "Show in the file tree" from another surface's menu: the files panel up, any
 * search list put away, the row revealed and focused. */
async function showInTree(relative) {
  setPanelFolded("aside", false);
  setActivityItem("files");
  if (fileTree.hidden) {
    fileSearch.value = "";
    await runFileSearch();
  }
  return revealTreePath(relative, { focus: true });
}

/* ---- the tree's root: the agent's folder, or one the person pins (t-24298)
 *
 * The mod's tree follows its session's working folder, or a folder pinned
 * with `/filetree <path>`. Here the tree is the active workspace's — the
 * sidebar already moves it with the agent from checkout to checkout
 * (`followPaneIntoWorktree`) — so inside it the agent's working folder is
 * followed the way a file is: revealed and marked while follow is on, never
 * made the whole view under the person. A folder the PERSON pins does become
 * the whole view, per workspace, until they unpin it from the head. */
const TREE_ROOT_KEY = "zerocode.explorer-root.v1";
let treeRoots = null;

function heldTreeRoots() {
  if (treeRoots !== null) return treeRoots;
  try {
    const saved = JSON.parse(localStorage.getItem(TREE_ROOT_KEY) ?? "{}");
    treeRoots = saved && typeof saved === "object" ? saved : {};
  } catch {
    treeRoots = {};
  }
  return treeRoots;
}

function keepTreeRoots() {
  try { localStorage.setItem(TREE_ROOT_KEY, JSON.stringify(heldTreeRoots())); }
  catch { /* Storage refusal keeps the pin for this session. */ }
}

/* The folder the whole tree shows for this workspace: "" for its root. */
function treePinnedRoot() {
  const held = activeWorktreePath ? heldTreeRoots()[activeWorktreePath] : undefined;
  return typeof held === "string" ? held : "";
}

function treeRootDepth() {
  const pinned = treePinnedRoot();
  return pinned ? pinned.split("/").length : 0;
}

function forgetTreeRoot() {
  if (!activeWorktreePath || !(activeWorktreePath in heldTreeRoots())) return;
  delete heldTreeRoots()[activeWorktreePath];
  keepTreeRoots();
}

/* Pin a folder as the whole tree, or unpin with "". The tree is reloaded
 * from its new top; selections under the old one are let go. */
async function pinTreeRoot(relative) {
  if (!activeWorktreePath) return;
  if (relative) heldTreeRoots()[activeWorktreePath] = relative;
  else delete heldTreeRoots()[activeWorktreePath];
  keepTreeRoots();
  resetTreeSelection();
  treeCursorPath = null;
  await loadTree(fileTree, "");
}

function paintTreePin() {
  const pinned = treePinnedRoot();
  el("tree-pin").hidden = !pinned;
  el("tree-pin-path").textContent = pinned;
  el("tree-pin").dataset.tip = pinned;
}

el("tree-unpin").addEventListener("click", () => void pinTreeRoot(""));

/* Where an agent's pane in this workspace stands, if that is a folder below
 * its root: revealed and marked (`data-agent-cwd`) while follow is on, the
 * mark gone when the pane steps back to the root. A plain shell is a person
 * walking, not an agent working, and is not followed. */
let treeAgentFolder = null;

function followAgentFolder(term, cwd) {
  const tab = tabOfTerm(term);
  if (!tab || tab.worktree !== activeWorktreePath || (!tab.agent && !paneAgents.has(term))) return;
  const relative = treeRelative(cwd, null);
  treeAgentFolder = relative === null ? null : { root: activeWorktreePath, relative };
  paintAgentFolder();
  if (relative !== null && treeFollowsAgent && fileTreeShowing()) {
    void revealInTree(relative).then(paintAgentFolder);
  }
}

function paintAgentFolder() {
  const here = treeAgentFolder?.root === activeWorktreePath ? treeAgentFolder.relative : null;
  const row = here === null ? null : treeRowOf(here);
  for (const held of fileTree.querySelectorAll("[data-agent-cwd]")) {
    if (held === row) continue;
    delete held.dataset.agentCwd;
    labelTreeRow(held);
  }
  if (row && row.dataset.agentCwd !== "true") {
    row.dataset.agentCwd = "true";
    labelTreeRow(row);
  }
}

/* ---- what an agent is doing in git, under the tree (t-24298) ----
 *
 * The mod's footer: a commit, a push, a pull, a PR in flight, said on one
 * line while it runs. A shell call carries its git and gh operations
 * (`activity.vcs`, read by the backend with the shell's own grammar), so the
 * line is in git's own words — `git add · git commit` — and the frame around
 * them is translated, the way an activity row is. It clears when the call
 * finishes or fails, or the pane's turn ends; only this workspace's panes
 * speak on it. */

/* A git call this long has an end the tree never heard — the batch carrying
 * it was lost, or the pane went away mid-command. Ten minutes is past any
 * push or clone a person would still be waiting on. */
const TREE_VCS_RUNNING_MAX_MS = 10 * 60_000;

/* pane → { steps, target, until }, newest last. */
const treeVcsRuns = new Map();
let treeVcsSweep = null;
/* The files git held as changed when a commit finished, for the next answer
 * to compare against — what left the list is what the commit took. */
let treeCommitWatch = null;

function noteTreeVcs(pane, activity, now) {
  if (activity.phase === "stopped") {
    if (treeVcsRuns.delete(pane)) paintTreeVcs();
    return;
  }
  const steps = Array.isArray(activity.vcs) ? activity.vcs : [];
  if (steps.length === 0) return;
  if (activity.phase === "started") {
    treeVcsRuns.delete(pane);
    treeVcsRuns.set(pane, { steps, target: activity.target ?? null, until: now + TREE_VCS_RUNNING_MAX_MS });
  } else {
    const held = treeVcsRuns.get(pane);
    if (held && (held.target === null || held.target === (activity.target ?? null))) treeVcsRuns.delete(pane);
    if (activity.phase === "finished" && steps.some((step) => step.tool === "git" && step.verb === "commit") && treePaneHome(pane) === activeWorktreePath) {
      treeCommitWatch = { root: activeWorktreePath, before: new Set(scmEntries.map((entry) => entry.path)) };
    }
  }
  paintTreeVcs();
}

/* The workspace a pane works in — its tab's; a helper's card is its
 * parent's. */
function treePaneHome(pane) {
  const term = treePaneTerm(pane);
  return term === null ? null : tabOfTerm(term)?.worktree ?? null;
}

function paintTreeVcs() {
  const host = el("tree-ops");
  const now = performance.now();
  for (const [pane, held] of treeVcsRuns) if (held.until <= now) treeVcsRuns.delete(pane);
  const here = [...treeVcsRuns.entries()].filter(([pane]) => treePaneHome(pane) === activeWorktreePath);
  clearTimeout(treeVcsSweep);
  treeVcsSweep = null;
  if (treeVcsRuns.size > 0) {
    const soonest = Math.min(...[...treeVcsRuns.values()].map((held) => held.until));
    treeVcsSweep = setTimeout(paintTreeVcs, Math.max(0, soonest - now));
  }
  if (here.length === 0) {
    host.hidden = true;
    host.replaceChildren();
    return;
  }
  const [, newest] = here[here.length - 1];
  const op = newest.steps.map((step) => `${step.tool} ${step.verb}`).join(" · ");
  host.replaceChildren();
  host.insertAdjacentHTML("beforeend", icon("branch"));
  const said = document.createElement("span");
  said.className = "tree-ops-said";
  said.textContent = t("tree.vcs.running", "{{op}} 진행 중", { op });
  host.appendChild(said);
  if (here.length > 1) {
    const more = document.createElement("span");
    more.className = "tree-ops-more";
    more.textContent = `+${here.length - 1}`;
    host.appendChild(more);
  }
  host.hidden = false;
}

/* A fresh answer after a commit finished: what git no longer holds as
 * changed went into the commit, and wears the commit's colour. */
function settleTreeCommit() {
  const watch = treeCommitWatch;
  if (!watch) return;
  treeCommitWatch = null;
  if (watch.root !== activeWorktreePath) return;
  if (treeTouchRoot !== activeWorktreePath) {
    treeTouches.clear();
    treeTouchRoot = activeWorktreePath;
  }
  const now = performance.now();
  let newest = null;
  for (const path of watch.before) {
    if (treeGit.files.has(path)) continue;
    touchTree(path, "commit", now);
    newest = path;
  }
  if (newest === null) return;
  scheduleTreeTouchPaint();
  if (treeTouchSweep === null) armTreeTouchSweep();
  if (treeFollowsAgent && fileTreeShowing()) followTreeTo(newest);
}

/* ---- the selection, the agents' context (t-24298) ----
 *
 * What the person has selected in the tree is what "this" means in their
 * next prompt — the mod hands it to Claude with the prompt. Here the backend
 * holds it (`tree_selection`) and the prompt hook's brief carries it to the
 * agents whose prompt takes context (crates/zerocode-shell tree_selection.rs).
 * Said once a frame however fast the arrows move, and only when it changed:
 * the backend starts with nothing selected, and a workspace switch with
 * nothing selected says nothing, so a switch still asks only what the move
 * needs. */
const TREE_SELECTION_NONE = "[]";
let treeSelectionSaid = TREE_SELECTION_NONE;
let treeSelectionFrame = 0;

function scheduleTreeSelectionReport() {
  if (treeSelectionFrame !== 0) return;
  treeSelectionFrame = requestAnimationFrame(() => {
    treeSelectionFrame = 0;
    reportTreeSelection();
  });
}

function reportTreeSelection() {
  const root = activeWorktreePath;
  if (!root) return;
  const paths = [...selectedTreePaths];
  const said = paths.length === 0 ? TREE_SELECTION_NONE : JSON.stringify([root, paths]);
  if (said === treeSelectionSaid) return;
  treeSelectionSaid = said;
  // A backend without the door keeps the selection the window's own.
  invoke("tree_selection", { root, paths }).catch(() => {});
}
