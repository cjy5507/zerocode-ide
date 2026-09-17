import { mkdir } from 'node:fs/promises';

export async function testExplorer(page, ok, primaryEvent, capture) {
  const present = await page.evaluate(() => !!document.getElementById('file-search-filters'));
  ok('explorer search exposes its filter row', present);
  if (!present) return;
  const result = await page.evaluate(async (primary) => {
    const root = activeWorktreePath;
    const original = { ...window.__ANSWER__ };
    const calls = [];
    window.__ANSWER__.search_text = (args) => { calls.push({ command: 'search_text', args }); return []; };
    window.__ANSWER__.fs_move = (args) => { calls.push({ command: 'fs_move', args }); return { kind: 'move', by: { kind: 'human' }, count: args.paths.length }; };
    window.__ANSWER__.fs_undo = (args) => { calls.push({ command: 'fs_undo', args }); return { kind: 'move', by: { kind: 'human' }, count: 2 }; };
    window.__ANSWER__.fs_redo = (args) => { calls.push({ command: 'fs_redo', args }); return { kind: 'move', by: { kind: 'human' }, count: 2 }; };
    window.__ANSWER__.list_dir = ({path}) => path === 'dest' ? [] : [
      { name: 'dest', is_dir: true }, { name: 'a.rs', is_dir: false }, { name: 'b.rs', is_dir: false },
    ];
    revealActivity("files", "text");
    setFileSearchMode('text');
    fileSearch.value = 'needle';
    el('file-filter-regex').click();
    await runFileSearch();
    const regex = calls.findLast((c) => c.command === 'search_text')?.args.options?.regex;
    el('file-filter-include').value = 'src/**';
    el('file-filter-include').dispatchEvent(new Event('input', { bubbles: true }));
    restoreExplorerWorktree('/tmp/another-explorer');
    const other = !el('file-filter-regex').classList.contains('is-active') && el('file-filter-include').value === '';
    restoreExplorerWorktree(root);
    const restored = el('file-filter-regex').getAttribute('aria-pressed') === 'true' && el('file-filter-include').value === 'src/**';
    fileSearch.value = '[';
    window.__ANSWER__.search_text = () => Promise.reject('file.searchInvalidRegex');
    await runFileSearch();
    const invalid = !el('file-search-error').hidden && el('file-search-error').textContent.length > 0;
    fileSearch.value = '';
    setFileSearchMode('name');
    await loadTree(fileTree, '');
    const rows = [...fileTree.querySelectorAll(':scope > .tree-row')];
    rows[1].dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, ...primary }));
    rows[2].dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, ...primary }));
    const dragBefore = { ...window.__COUNTS__ };
    const dt = new DataTransfer();
    rows[1].dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    rows[0].dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt }));
    await new Promise((done) => setTimeout(done, 30));
    const badge = el('tree-drag-badge').textContent;
    rows[0].dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
    await new Promise((done) => setTimeout(done, 30));
    const moves = calls.filter((c) => c.command === 'fs_move');
    const toast = !el('tree-undo-toast').hidden;
    const dragInvokes = Object.fromEntries(['list_dir', 'fs_move'].map((command) => [command, (window.__COUNTS__[command] ?? 0) - (dragBefore[command] ?? 0)]));
    const before = calls.filter((c) => c.command === 'fs_undo').length;
    keySink.focus();
    keySink.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'z', code: 'KeyZ', ...primary }));
    await Promise.resolve();
    const terminalSafe = calls.filter((c) => c.command === 'fs_undo').length === before;
    fileTree.focus();
    fileTree.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'z', code: 'KeyZ', ...primary }));
    await new Promise((done) => setTimeout(done, 30));
    const undo = calls.filter((c) => c.command === 'fs_undo').length === before + 1;
    const oldBindings = keybindingOverrides;
    keybindingOverrides = { ...oldBindings, 'tree.undo': ['mod+alt+u'] };
    rebuildBound();
    fileTree.focus();
    fileTree.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'u', code: 'KeyU', altKey: true, ...primary }));
    await new Promise((done) => setTimeout(done, 30));
    const rebound = calls.filter((c) => c.command === 'fs_undo').length === before + 2;
    keybindingOverrides = oldBindings;
    rebuildBound();
    window.__EXPLORER_TEST_RESTORE__ = () => {
      window.__ANSWER__ = original;
      restoreExplorerWorktree(root);
      resetExplorerFilters();
      fileSearch.value = '';
      setFileSearchMode('name');
      finishTreeDrag();
      el('tree-undo-toast').hidden = true;
    };
    return { regex, other, restored, invalid, badge, moves, toast, terminalSafe, undo, rebound, dragInvokes, counters: calls.reduce((all, c) => ({ ...all, [c.command]: (all[c.command] ?? 0) + 1 }), {}) };
  }, primaryEvent);
  ok('regex is sent as an option and filters restore per worktree', result.regex && result.other && result.restored, JSON.stringify(result));
  ok('invalid search syntax is an inline sentence', result.invalid, JSON.stringify(result));
  ok('two selected files move in one invoke with a preview badge and undo toast', result.moves.length === 1 && result.moves[0].args.paths.length === 2 && result.badge.includes('2') && result.toast, JSON.stringify(result));
  ok('explorer undo honors focus and the configurable key table', result.terminalSafe && result.undo && result.rebound, JSON.stringify(result));
  const boundaries = await page.evaluate(async (primary) => {
    const seen = [];
    window.__ANSWER__.fs_move = (args) => { seen.push(['move', args]); return { kind: 'move', by: { kind: 'human' } }; };
    window.__ANSWER__.fs_undo = (args) => { seen.push(['undo', args]); return { kind: 'move', by: { kind: 'human' } }; };
    window.__ANSWER__.fs_redo = (args) => { seen.push(['redo', args]); return { kind: 'move', by: { kind: 'human' } }; };
    window.__ANSWER__.list_dir = ({ path }) => path === 'dest' ? [{ name: 'a.rs', is_dir: false }] : [
      { name: 'dest', is_dir: true }, { name: 'a.rs', is_dir: false }, { name: 'b.rs', is_dir: false },
    ];
    resetTreeSelection(); await loadTree(fileTree, '');
    const rows = [...fileTree.querySelectorAll(':scope > .tree-row')];
    rows[1].dispatchEvent(new MouseEvent('click', { bubbles: true, ...primary }));
    rows[2].dispatchEvent(new MouseEvent('click', { bubbles: true, shiftKey: true }));
    const range = selectedTreePaths.size;
    const dt = new DataTransfer();
    rows[1].dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    rows[0].dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt }));
    await new Promise((done) => setTimeout(done, 30));
    const conflict = el('tree-drag-badge').classList.contains('is-conflict');
    rows[0].dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
    await new Promise((done) => setTimeout(done, 30));
    const rejected = !seen.some(([kind]) => kind === 'move');
    fileSearch.focus();
    fileSearch.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'z', code: 'KeyZ', ...primary }));
    const editable = !seen.some(([kind]) => kind === 'undo');
    fileTree.focus();
    fileTree.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'z', code: 'KeyZ', shiftKey: true, ...primary }));
    await new Promise((done) => setTimeout(done, 30));
    const redo = seen.filter(([kind]) => kind === 'redo').length === 1;
    return { range, conflict, rejected, editable, redo };
  }, primaryEvent);
  ok('range selection, collision refusal, editable focus and redo work through the real rows', boundaries.range === 2 && boundaries.conflict && boundaries.rejected && boundaries.editable && boundaries.redo, JSON.stringify(boundaries));
  console.log('EXPLORER_INVOKES ' + JSON.stringify({ scripted: result.counters, drag: result.dragInvokes }));
  if (capture) {
    await mkdir(capture, { recursive: true });
    await page.evaluate(async () => {
      revealActivity('files', 'text');
      window.__ANSWER__.search_text = () => [{ path: 'src/search.rs', line: 12, text: 'let needle = query;' }];
      fileSearch.value = 'needle';
      el('tree-undo-toast').hidden = true;
      await runFileSearch();
    });
    await page.locator('#activity-files').screenshot({ path: `${capture}/filter-row.png` });
    await page.evaluate(async () => {
      fileSearch.value = ''; setFileSearchMode('name'); el('tree-undo-toast').hidden = true; resetTreeSelection();
      window.__ANSWER__.list_dir = ({path}) => path === 'dest' ? [] : [{name:'dest',is_dir:true},{name:'a.rs',is_dir:false},{name:'b.rs',is_dir:false}];
      await loadTree(fileTree, '');
      const rows = [...fileTree.querySelectorAll(':scope > .tree-row')];
      rows[1].dispatchEvent(new MouseEvent('click', { bubbles: true, ...window.__TEST_PRIMARY_EVENT__ }));
      rows[2].dispatchEvent(new MouseEvent('click', { bubbles: true, ...window.__TEST_PRIMARY_EVENT__ }));
      const dataTransfer = new DataTransfer();
      rows[1].dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer }));
      rows[0].dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }));
    });
    await page.waitForTimeout(50);
    await page.locator('#activity-files').screenshot({ path: `${capture}/drag-badge.png` });
    await page.evaluate(() => { finishTreeDrag(); showTreeUndo({ kind: 'move', by: { kind: 'agent', agent: 'codex', pane: 'term-8' }, count: 2 }); });
    await page.locator('#activity-files').screenshot({ path: `${capture}/undo-toast.png` });
  }
  const contrast = await page.evaluate(async () => {
    const prior = document.documentElement.dataset.theme;
    const entries = [];
    const rgb = (value) => value.match(/[\d.]+/g).slice(0, 3).map(Number);
    const luminance = (values) => values.map((v) => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [.2126, .7152, .0722][i], 0);
    for (const theme of ['dark', 'light']) {
      document.documentElement.dataset.theme = theme;
      setFileSearchMode('text');
      await new Promise((done) => setTimeout(done, 250));
      const input = el('file-filter-include');
      const style = getComputedStyle(input);
      const values = [luminance(rgb(style.color)), luminance(rgb(style.backgroundColor))].sort((a,b) => b-a);
      const placeholder = getComputedStyle(input, '::placeholder');
      const muted = [luminance(rgb(placeholder.color)), luminance(rgb(style.backgroundColor))].sort((a,b) => b-a);
      const box = el('file-search-filters').getBoundingClientRect();
      entries.push({ theme, ratio: (values[0] + .05) / (values[1] + .05), placeholderRatio: (muted[0] + .05) / (muted[1] + .05), fits: box.width <= el('activity-files').getBoundingClientRect().width });
    }
    document.documentElement.dataset.theme = prior;
    return entries;
  });
  ok('explorer filter text has readable contrast in both themes and fits its panel', contrast.every((sample) => sample.ratio >= 4.5 && sample.placeholderRatio >= 4.5 && sample.fits), JSON.stringify(contrast));
  if (capture) {
    await page.evaluate(async () => {
      document.documentElement.dataset.theme = 'light'; setFileSearchMode('text'); fileSearch.value = 'needle';
      el('tree-undo-toast').hidden = true; await runFileSearch();
    });
    await page.waitForTimeout(250);
    await page.locator('#activity-files').screenshot({ path: `${capture}/filter-row-light.png` });
    await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
  }
  await page.evaluate(() => { window.__EXPLORER_TEST_RESTORE__(); delete window.__EXPLORER_TEST_RESTORE__; });
}
