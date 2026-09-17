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
