/* t-3304: the review's capped history and lost-mouseup cases, through the
 * real view, copy adapter and event listeners. Shared by the window gate and
 * a focused Chromium/WebKit runner: node ui/tests/terminal-selection.mjs. */
import { testGridSelection, installTerminalWaits } from "./terminal-grid-selection.mjs";
import { openWindowTestPage } from "./window-boot.mjs";


export async function testTerminalSelection(page, ok) {
  await installTerminalWaits(page);
  const results = await page.evaluate(async () => {
    const results = [];
    const wait = (ms) => new Promise((done) => setTimeout(done, ms));
    const saved = { ...window.__ANSWER__ };
    const schedules = window.__WATCH_SCHEDULES__(["flushScroll", "dragScrollTick"]);
    let term;
    let view;
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(document, "hidden");
    const restoreHidden = () => {
      if (hiddenDescriptor) Object.defineProperty(document, "hidden", hiddenDescriptor);
      else delete document.hidden;
    };
    try {
      window.__ANSWER__.term_snapshot = () => null;
      term = await openTermTab({ placement: "tab" });
      view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      const COLS = 24, ROWS = 10, HISTORY = 200;
      let lines, offset, trimmed, scrolls;
      const cells = (text) => [...text.padEnd(COLS)].map((ch) => ({ ch }));
      const frame = (extra = {}) => ({
        rows: Array.from({ length: ROWS }, (_, index) => ({ index, cells: cells(lines[HISTORY - offset + index] ?? "") })),
        size: [ROWS, COLS], cursor: [9, 0], full: true, scrolled_lines: 0,
        alt_screen: false, mouse_tracking: "off", mouse_sgr: false,
        cursor_visible: false, scrollback_len: HISTORY, view_offset: offset,
        scrollback_trimmed: trimmed, ...extra,
      });
      const reset = () => {
        view.reset();
        view.host.hidden = false;
        lines = Array.from({ length: HISTORY + ROWS }, (_, i) => `line ${String(i).padStart(3, "0")} text`);
        offset = 50; trimmed = 30; scrolls = 0;
        view.apply(frame());
      };
      window.__ANSWER__.term_lines = ({ from, to }) => {
        const out = [];
        for (let line = from.line; line <= to.line; line += 1) {
          out.push((lines[line] ?? "").padEnd(COLS).slice(
            line === from.line ? from.col : 0, line === to.line ? to.col : COLS,
          ).trimEnd());
        }
        return out.join("\n");
      };
      window.__ANSWER__.term_scroll = ({ lines: by }) => {
        scrolls += 1;
        offset = Math.min(HISTORY, Math.max(0, offset + by));
        view.apply(frame());
        return null;
      };
      const select = (line, end = line, from = 1, to = 8) => {
        view.selection.begin({ from: { line, col: from }, to: { line: end, col: to } });
        view.apply(frame());
      };
      const evict = (count) => {
        lines.splice(0, count);
        lines.push(...Array.from({ length: count }, () => "new output"));
        trimmed += count;
      };
      const painted = () => [...view.pre.querySelectorAll(".term-selected")].map((node) => node.textContent).join("");
      const record = (name, pass, detail) => results.push({ name, pass, detail });

      for (const kind of ["live", "history full", "fold full", "top partial", "cap shrink"]) {
        reset();
        if (kind === "live" || kind === "top partial") offset = 0;
        const line = HISTORY - offset + 3 + (kind === "fold full" ? 2 : 0);
        select(line);
        const before = await view.selectionText();
        const count = kind === "cap shrink" ? 100 : 1;
        evict(count);
        if (kind === "cap shrink") offset += count;
        const extra = kind === "live" || kind === "top partial"
          ? { full: false, scrolled_lines: kind === "live" ? 1 : 0 }
          : kind === "fold full" ? { row_lines: Array.from({ length: ROWS }, (_, i) => HISTORY - offset + i + (i > 0 ? 2 : 0)) } : {};
        const delta = frame(extra);
        if (delta.row_lines) delta.rows = delta.row_lines.map((at, index) => ({ index, cells: cells(lines[at]) }));
        view.apply(delta);
        const after = await view.selectionText();
        const range = view.selection.range();
        record(`selection follows actual eviction in ${kind}`, after === before && range?.start.line === line - count && painted() === before,
          { before, after, range, painted: painted() });
      }

      reset();
      select(153, 202);
      const seamCopy = await view.selectionText();
      evict(3); // Frames consumed while the host was not watched.
      view.apply(frame());
      const range = view.selection.range();
      const copied = await view.selectionText();
      view.apply(frame()); // A read-only snapshot repeats the same origin.
      record("a reveal catches missed trims across the history/screen seam exactly once",
        copied === seamCopy && range.start.line === 150 && range.end.line === 199 && JSON.stringify(view.selection.range()) === JSON.stringify(range),
        { range, repeated: view.selection.range(), sameText: copied === seamCopy });

      reset();
      select(1, 3, 4, 8);
      evict(2); view.apply(frame());
      const clipped = view.selection.range();
      record("trim clips a partially evicted selection to the first retained cell",
        clipped?.start.line === 0 && clipped.start.col === 0 && clipped.end.line === 1 && clipped.end.col === 8, clipped);
      evict(2); view.apply(frame());
      record("a fully evicted selection has no highlight or copied text",
        !view.selection.anchored() && !view.selection.stands() && painted() === "" && await view.selectionText() === "", view.selection.range());
      const box = view.pre.getBoundingClientRect();
      const shiftClick = {
        clientX: box.left + 3 * view.cell.width,
        clientY: box.top + 1.5 * view.cell.height,
        bubbles: true, button: 0, buttons: 1, shiftKey: true,
      };
      view.host.dispatchEvent(new MouseEvent("mousedown", shiftClick));
      window.dispatchEvent(new MouseEvent("mouseup", { ...shiftClick, buttons: 0 }));
      record("Shift-click starts fresh after the whole selection was evicted",
        !view.selection.stands() && painted() === "", view.selection.range());

      reset(); select(153);
      const stationary = JSON.stringify(view.selection.range());
      view.apply(frame({ full: false, scrolled_lines: 1, scrollback_len: HISTORY + 1 }));
      view.apply(frame({ full: false, scrolled_lines: 1, alt_screen: true }));
      record("history growth and alternate rotation do not invent eviction",
        JSON.stringify(view.selection.range()) === stationary, view.selection.range());

      for (const reason of ["blur", "visibility", "hidden host", "lost buttons", "detached host"]) {
        reset();
        await wait(TERM_SELECT_MULTI_CLICK.windowMs + 20);
        const box = view.pre.getBoundingClientRect();
        const at = (y, buttons = 1) => ({ bubbles: true, button: 0, buttons, clientX: box.left + 2 * view.cell.width, clientY: y });
        view.host.dispatchEvent(new MouseEvent("mousedown", at(box.top + 2.5 * view.cell.height)));
        window.dispatchEvent(new MouseEvent("mousemove", at(box.top + ROWS * view.cell.height + 1)));
        await window.__UNTIL__(() => scrolls > 0 && view.selection.stands(), "edge scroll frame");
        const started = scrolls > 0 && view.selection.stands();
        const held = JSON.stringify(view.selection.range());
        const parent = view.host.parentNode, next = view.host.nextSibling;
        if (reason === "blur") window.dispatchEvent(new Event("blur"));
        if (reason === "visibility") {
          Object.defineProperty(document, "hidden", { configurable: true, value: true });
          document.dispatchEvent(new Event("visibilitychange"));
        }
        if (reason === "hidden host") view.host.hidden = true;
        if (reason === "detached host") view.host.remove();
        if (reason === "lost buttons") window.dispatchEvent(new MouseEvent("mousemove", at(box.top + 5.5 * view.cell.height, 0)));
        // A queued backend response after cancellation must not extend the range.
        offset = 40;
        view.apply(frame());
        await window.__UNTIL__(() => schedules.pending("dragScrollTick") === 0
          && schedules.pending("flushScroll") === 0, `edge drag cancelled by ${reason}`);
        const settled = scrolls;
        record(`edge drag ends on ${reason} and preserves the model selection`,
          started && schedules.pending("dragScrollTick") === 0 && JSON.stringify(view.selection.range()) === held,
          { started, settled, scrolls, held: JSON.parse(held), after: view.selection.range() });
        restoreHidden();
        view.host.hidden = false;
        if (!view.host.isConnected) parent.insertBefore(view.host, next);
        window.dispatchEvent(new MouseEvent("mouseup", { button: 0 }));
      }
    } finally {
      schedules.restore();
      restoreHidden();
      if (view) view.host.hidden = false;
      const tab = tabOfTerm(term);
      if (tab) {
        dropTermView(term);
        dropTab(tab.id);
      }
      window.__ANSWER__ = saved;
    }
    return results;
  });
  for (const result of results) ok(result.name, result.pass, JSON.stringify(result.detail));
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { createRequire } = await import("node:module");
  const { webkit } = createRequire(import.meta.url)("playwright");
  const { files, origin } = await createWindowServer();
  try {
    for (const [name, engine] of [["chromium", chromium], ["webkit", webkit]]) {
      const browser = await engine.launch();
      try {
        const { page, faults } = await openWindowTestPage(browser, origin);
        const record = (test, pass, detail) => {
          console.log(JSON.stringify({ engine: name, test, pass, detail: JSON.parse(detail) }));
          if (!pass) process.exitCode = 1;
        };
        await testGridSelection(page, record);
        await testTerminalSelection(page, record);
        if (faults.length) throw new Error(faults.join("\n"));
      } finally { await browser.close(); }
    }
  } finally { files.close(); }
}
