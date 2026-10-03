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
}

function paintTreeSelection() {
  for (const row of fileTree.querySelectorAll('.tree-row[data-tree-path]')) {
    const selected = selectedTreePaths.has(row.dataset.treePath);
    row.classList.toggle('is-selected', selected);
    row.setAttribute('aria-pressed', String(selected));
  }
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

fileTree.addEventListener('dragover', (event) => {
  if (event.target.closest('.tree-row')) return;
  previewTreeDrop(event, fileTree, '');
});
fileTree.addEventListener('drop', (event) => {
  if (event.target.closest('.tree-row')) return;
  void dropTreePaths(event, '');
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
  const term = Number(/^(?:term|sub):(\d+)(?::|$)/.exec(pane ?? "")?.[1]);
  if (!Number.isInteger(term)) return null;
  const home = tabOfTerm(term)?.worktree;
  if (!home) return null;
  const cwd = treeTermCwds.get(term);
  return cwd && (cwd === home || cwd.startsWith(`${home}/`)) ? cwd : home;
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

function treeRelative(target, pane) {
  const root = treeSlashes(activeWorktreePath ?? "").replace(/\/+$/, "");
  if (!root || typeof target !== "string" || target.length === 0 || target.endsWith("…")) return null;
  const wanted = treeSlashes(target);
  const absolute = wanted.startsWith("/") || (usesWindowsPlatform && /^[A-Za-z]:\//.test(wanted));
  const base = absolute ? null : treePaneBase(pane);
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

/* What one activity touched: a kind and the targets it names, or null. A
 * call that failed touched nothing; a prompt and a turn's end are not tool
 * calls. */
function treeTouchOf(activity) {
  if (activity.phase !== "started" && activity.phase !== "finished") return null;
  const kind = TREE_TOUCH_OF_VERB[activity.verb];
  if (!kind || typeof activity.target !== "string") return null;
  if (activity.verb === "grep" && TREE_PATTERN_SIGNS.test(activity.target)) return null;
  return { kind, targets: [activity.target] };
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
    const touch = treeTouchOf(activity);
    if (!touch) continue;
    for (const target of touch.targets) {
      const relative = treeRelative(target, pane);
      if (relative === null) continue;
      touchTree(relative, touch.kind, now);
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
  }
  for (const [row, marks] of wanted) {
    for (const slot of ["agentTouch", "agentTouchWithin"]) {
      if (marks[slot]) {
        if (row.dataset[slot] !== marks[slot]) row.dataset[slot] = marks[slot];
      } else if (slot in row.dataset) {
        delete row.dataset[slot];
      }
    }
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
  const parts = relative.split("/");
  for (let depth = 1; depth < parts.length; depth += 1) {
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

/* What `loadTree` calls once it has built a folder's rows. */
function dressTreeRows() {
  if (treeTouches.size > 0) scheduleTreeTouchPaint();
}
