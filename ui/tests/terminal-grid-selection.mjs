/* The buffer-coordinate selection scenario, shared by the full gate and
 * the focused terminal-selection runner. */
import { installHarnessWaits } from "./harness-waits.mjs";

// Opening a tab queues the watched-set declaration and the pull that pays its
// floor. A visible box alone does not mean the window reads the shell yet.
export async function installTerminalWaits(page) {
  await installHarnessWaits(page);
  await page.evaluate(() => {
    window.__TERMINAL_READY__ = async (term, view) => {
      await watchTail;
      await window.__UNTIL__(() => watchedTerms.has(term) && !view.host.hidden
        && view.pre.getBoundingClientRect().width > 0, "terminal declared and laid out");
    };
  });
}

export async function testGridSelection(page, ok) {
  await installTerminalWaits(page);
  const gridSelection = await page.evaluate(async () => {
    const seen = {};
    const priorAnswers = { ...window.__ANSWER__ };
    const wait = (ms) => new Promise((done) => setTimeout(done, ms));
    const schedules = window.__WATCH_SCHEDULES__(["flushScroll", "dragScrollTick"]);
    try {
      window.__ANSWER__.term_snapshot = () => null;
      const term = await openTermTab({ placement: "tab" });
      seen.termOpened = term;
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      const COLS = 24;
      const ROWS = 10;
      const HISTORY = 200;
      // The whole buffer the backend holds: 200 lines of history, then the screen.
      const lines = [];
      for (let i = 0; i < HISTORY; i += 1) lines.push(`hist ${String(i).padStart(3, "0")} line`);
      for (let i = 0; i < ROWS; i += 1) lines.push(`screen ${i} row`);
      let offset = 0;
      const cellsOf = (text) => [...text.padEnd(COLS, " ")].map((ch) => ({ ch }));
      const frameAt = (viewOffset, extra = {}) => {
        offset = viewOffset;
        const top = HISTORY - viewOffset;
        return {
          rows: Array.from({ length: ROWS }, (u, i) => ({ index: i, cells: cellsOf(lines[top + i] ?? "") })),
          scrolled_lines: 0, cursor: [ROWS - 1, 0], title: null, alt_screen: false,
          size: [ROWS, COLS], full: true, cursor_visible: viewOffset === 0, mouse_tracking: "off",
          mouse_sgr: false, bell: false, view_offset: viewOffset, scrollback_len: HISTORY, folds: [],
          ...extra,
        };
      };
      // The screen pulls what the shell printed on its own cadence; `landed` is
      // the last frame's arrival, for the frames a scroll answer prints.
      let landed = Promise.resolve();
      const feed = (delta) => (landed = window.__TERM_FEED__(term, delta));
      // The backend's copy road, answered from the buffer above by the grid's
      // own rule: the end column exclusive, trailing blanks off, a newline per row.
      const asked = [];
      window.__ANSWER__.term_lines = ({ from, to }) => {
        asked.push({ from, to });
        const out = [];
        for (let line = from.line; line <= to.line; line += 1) {
          const text = (lines[line] ?? "").padEnd(COLS, " ");
          out.push(text.slice(line === from.line ? from.col : 0, line === to.line ? to.col : COLS).replace(/[ \t]+$/, ""));
        }
        return out.join("\n");
      };
      // The scroll road: the frame the scroll lands on, as the pump would send it.
      const scrolls = [];
      window.__ANSWER__.term_scroll = ({ lines: by }) => {
        scrolls.push(by);
        feed(frameAt(Math.min(HISTORY, Math.max(0, offset + by))));
        return null;
      };
      window.__ANSWER__.term_mouse = () => null;
      await feed(frameAt(0));
      await window.__UNTIL__(() => view.pre.querySelectorAll(":scope > .term-row").length === ROWS, "terminal frame");

      const box = view.pre.getBoundingClientRect();
      const at = (row, col, extra = {}) => ({
        clientX: box.left + col * view.cell.width,
        clientY: box.top + (row + 0.5) * view.cell.height,
        bubbles: true, cancelable: true, button: 0, ...extra,
      });
      const press = (row, col, extra) => view.host.dispatchEvent(new MouseEvent("mousedown", at(row, col, extra)));
      const move = (row, col, extra) => window.dispatchEvent(new MouseEvent("mousemove", { buttons: 1, ...at(row, col, extra) }));
      const release = (row, col, extra) => window.dispatchEvent(new MouseEvent("mouseup", at(row, col, extra)));
      const drag = (r0, c0, r1, c1, extra) => { press(r0, c0, extra); move(r1, c1, extra); release(r1, c1, extra); };
      const painted = () => [...view.pre.querySelectorAll(":scope > .term-row")]
        .map((node) => [...node.querySelectorAll(".term-selected")].map((s) => s.textContent).join(""));
      const paintedRows = () => painted().map((text, i) => (text === "" ? null : i)).filter((i) => i !== null);
      const range = () => view.selection.range();
      const copied = async () => {
        window.__CLIPBOARD_WRITES__.length = 0;
        await copyTerminalSelection(`term:${term}`);
        await window.__UNTIL__(() => window.__CLIPBOARD_WRITES__.length > 0, "selection copy");
        return window.__CLIPBOARD_WRITES__.at(-1) ?? null;
      };

      // (a) 3행→8행 드래그, 휠로 50행 뒤로, 되돌아오기.
      drag(3, 2, 8, 10);
      seen.range = range();
      seen.paintedRows = paintedRows();
      seen.copyLive = await copied();
      view.host.dispatchEvent(new WheelEvent("wheel", {
        deltaY: -(50.5 * view.cell.height) / termPrefs.sensitivity, deltaMode: 0, bubbles: true, cancelable: true,
      }));
      await window.__UNTIL__(() => offset === 50 && schedules.pending("flushScroll") === 0, "wheel frame");
      await landed;
      seen.scrolledTo = offset;
      seen.rangeAfterScroll = range();
      seen.paintedAfterScroll = paintedRows();
      seen.copyScrolled = await copied();
      seen.askedSame = JSON.stringify(asked.at(-1)) === JSON.stringify(asked.at(-2));
      await feed(frameAt(0));
      seen.paintedBack = paintedRows();

      // (b) 한 행이 델타로 바뀌어도 선택은 좌표를 지킨다.
      await feed({ ...frameAt(0), full: false, rows: [{ index: 5, cells: cellsOf("changed 5 text") }] });
      seen.rangeAfterDelta = range();
      seen.paintedChangedRow = painted()[5].trimEnd();
      seen.paintedRowsAfterDelta = paintedRows();

      // A focus move, a frame, a wheel: none of them is a person clearing it.
      keySink.focus();
      document.body.focus();
      seen.rangeAfterFocus = range();

      // (c) 아래 가장자리 너머 드래그: 자동 스크롤하며 선택이 자란다.
      await feed(frameAt(50));
      press(2, 0);
      const bottomBefore = HISTORY - 50 + ROWS - 1;
      window.dispatchEvent(new MouseEvent("mousemove", {
        clientX: box.left + 4, clientY: box.bottom + 40, bubbles: true, cancelable: true, button: 0, buttons: 1,
      }));
      await window.__UNTIL__(() => offset < 50 && range()?.end.line > bottomBefore, "edge scroll frame");
      await landed;
      seen.autoScrolled = scrolls.filter((by) => by < 0).length;
      seen.autoScrollOffset = offset;
      seen.grewPast = { bottomBefore, end: range()?.end ?? null };
      seen.paintedToBottom = paintedRows().at(-1);
      // Hold the next real flush: a drag tick banks intent before mouseup,
      // while its frame/timer sends that intent only after mouseup. This is the
      // schedule that made lane-c68c792f count an old flight as a new gesture.
      schedules.hold("flushScroll");
      await window.__UNTIL__(() => schedules.pending("flushScroll") > 0, "banked edge scroll");
      seen.inFlightAtRelease = schedules.pending("flushScroll") > 0;
      release(ROWS - 1, COLS);
      const rangeAtRelease = JSON.stringify(range());
      const scrollsAtRelease = scrolls.length;
      seen.dragTimerCancelled = schedules.pending("dragScrollTick") === 0;
      schedules.release("flushScroll");
      await window.__UNTIL__(() => schedules.pending("flushScroll") === 0, "last pre-release scroll flight");
      seen.issuedAfterRelease = scrolls.length - scrollsAtRelease;
      seen.scrollStoppedOnRelease = seen.dragTimerCancelled
        && schedules.pending("dragScrollTick") === 0
        && JSON.stringify(range()) === rangeAtRelease;
      seen.rangeAfterAutoScroll = range();

      // (d) 추적 판: Shift+드래그는 같은 모델을 쓰고 프로그램에는 한 마디도 없다.
      await feed(frameAt(0, { mouse_tracking: "motion" }));
      const mouseBefore = window.__COUNTS__.term_mouse ?? 0;
      drag(1, 2, 1, 10, { shiftKey: true });
      seen.trackedShift = { range: range(), reported: (window.__COUNTS__.term_mouse ?? 0) - mouseBefore, painted: painted()[1] };
      // A plain drag is the program's, and it leaves the selection alone.
      drag(4, 0, 6, 4);
      seen.trackedPlain = { range: range(), reported: (window.__COUNTS__.term_mouse ?? 0) - mouseBefore };
      await feed(frameAt(0));

      // (e) 더블클릭은 낱말, 트리플클릭은 줄; 낱말 모드의 드래그는 낱말 단위로 자란다.
      press(4, 9.5); release(4, 9.5);
      press(4, 9.5); release(4, 9.5);
      seen.word = { range: range(), painted: painted()[4] };
      press(4, 9.5);
      seen.line = { range: range(), painted: painted()[4].trimEnd() };
      release(4, 9.5);
      await wait(TERM_SELECT_MULTI_CLICK.windowMs + 20);
      press(4, 9.5); release(4, 9.5);
      press(4, 9.5); move(4, 1.5);
      seen.wordDrag = { range: range(), painted: painted()[4] };
      release(4, 1.5);
      await wait(TERM_SELECT_MULTI_CLICK.windowMs + 20);

      // Shift+click extends what stands.
      drag(2, 0, 2, 5);
      press(5, 4, { shiftKey: true }); release(5, 4, { shiftKey: true });
      seen.shiftExtended = range();

      // (f) 클릭이 지운다; Esc 도.
      press(6, 3); release(6, 3);
      seen.clearedByClick = { stands: view.selection.stands(), painted: paintedRows() };
      drag(1, 0, 2, 4);
      seen.standsBeforeEsc = view.selection.stands();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      seen.clearedByEsc = { stands: view.selection.stands(), painted: paintedRows() };

      // The selection has no browser side: nothing inside the screen is selectable.
      const selectionStyle = getComputedStyle(view.pre);
      seen.userSelect = selectionStyle.userSelect ?? selectionStyle.getPropertyValue("-webkit-user-select");
      seen.selectionColors = (() => {
        drag(1, 0, 1, 4);
        const span = view.pre.querySelector(".term-selected");
        const style = span ? getComputedStyle(span) : null;
        const root = getComputedStyle(document.documentElement);
        const want = { bg: root.getPropertyValue("--term-selection-bg").trim(), fg: root.getPropertyValue("--term-selection-fg").trim() };
        const probe = document.createElement("span");
        probe.style.color = want.fg;
        probe.style.background = want.bg;
        document.body.appendChild(probe);
        const wanted = getComputedStyle(probe);
        const answer = style !== null && style.backgroundColor === wanted.backgroundColor && style.color === wanted.color;
        probe.remove();
        view.clearSelection();
        return answer;
      })();

      // An alternate-screen scroll is a redraw, not eviction from history.
      // Its text moves underneath the selected coordinates, so both the range
      // and the painted row numbers must stay where the reader left them.
      await feed(frameAt(0, { alt_screen: true }));
      drag(3, 2, 5, 10);
      seen.altRange = range();
      await feed({ ...frameAt(0, { alt_screen: true }), full: false, scrolled_lines: 1,
        rows: [{ index: ROWS - 1, cells: cellsOf("new alt row") }] });
      seen.altRangeAfterShift = range();
      seen.altPaintedAfterShift = paintedRows();

      // Escape cancels a drag held past the edge as well as its highlight.
      // Leave room for scrolling, and allow the already queued flight to land
      // before checking that no later timer tick sends another scroll.
      await feed(frameAt(160));
      press(2, 4);
      window.dispatchEvent(new MouseEvent("mousemove", {
        clientX: box.left + 4 * view.cell.width,
        clientY: box.top + ROWS * view.cell.height + 1,
        bubbles: true, cancelable: true, button: 0, buttons: 1,
      }));
      await window.__UNTIL__(() => schedules.pending("dragScrollTick") > 0, "Escape edge drag");
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await window.__UNTIL__(() => schedules.pending("flushScroll") === 0, "pre-Escape scroll flight");
      seen.escapeStoppedDrag = schedules.pending("dragScrollTick") === 0 && !view.selection.stands();
      release(ROWS - 1, COLS);

      // Fold composition skips buffer lines 151 and 152. Slot 1 therefore
      // names line 153, and copy must ask for that line rather than a hidden body.
      const composedLines = [150, 153, 154, 155, 156, 157, 158, 159, 160, 161];
      await feed({ ...frameAt(50), row_lines: composedLines,
        rows: composedLines.map((line, index) => ({ index, cells: cellsOf(lines[line]) })) });
      drag(1, 0, 1, 8);
      seen.composedRange = range();
      seen.composedCopy = await copied();
      await feed({ ...frameAt(49), row_lines: composedLines.slice(1).concat(162),
        rows: composedLines.slice(1).concat(162).map((line, index) => ({ index, cells: cellsOf(lines[line]) })) });
      seen.composedPaintAfterScroll = paintedRows();
      seen.composedCopyAfterScroll = await copied();

      seen.faults = 0;
    } catch (error) {
      seen.error = String(error?.stack ?? error);
    } finally {
      schedules.restore();
      // Whatever happened above, the stubs and the tab must not outlive this
      // rig — a scroll answered with a frame is not what the next case means.
      window.__ANSWER__ = priorAnswers;
      const tab = tabOfTerm(seen.termOpened);
      if (tab) {
        dropTermView(seen.termOpened);
        dropTab(tab.id);
      }
    }
    return seen;
  });
  {
    const s = gridSelection;
    const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    const rangeA = { start: { line: 203, col: 2 }, end: { line: 208, col: 10 } };
    const textA = "reen 3 row\nscreen 4 row\nscreen 5 row\nscreen 6 row\nscreen 7 row\nscreen 8 r";
    ok(
      "a terminal selection is a grid model: it survives a wheel of fifty rows, copies the same text from history, and is painted again on the way back",
      same(s.range, rangeA) && same(s.paintedRows, [3, 4, 5, 6, 7, 8]) && s.copyLive === textA &&
        s.scrolledTo === 50 && same(s.rangeAfterScroll, rangeA) && same(s.paintedAfterScroll, []) &&
        s.copyScrolled === textA && s.askedSame === true && same(s.paintedBack, [3, 4, 5, 6, 7, 8]),
      JSON.stringify(s),
    );
    ok(
      "a frame that rewrites a selected row, and a focus move, leave the selection standing on its coordinates",
      same(s.rangeAfterDelta, rangeA) && s.paintedChangedRow === "changed 5 text" &&
        same(s.paintedRowsAfterDelta, [3, 4, 5, 6, 7, 8]) && same(s.rangeAfterFocus, rangeA),
      JSON.stringify(s),
    );
    ok(
      "dragging past the bottom edge scrolls toward live and the selection grows with the screen, until the release",
      s.autoScrolled >= 1 && s.autoScrollOffset < 50 &&
        s.grewPast?.end?.line > s.grewPast?.bottomBefore && s.paintedToBottom === 9 &&
        s.inFlightAtRelease && s.issuedAfterRelease === 1 && s.dragTimerCancelled &&
        s.scrollStoppedOnRelease === true && s.rangeAfterAutoScroll?.start?.line === 152,
      JSON.stringify(s),
    );
    ok(
      "on a tracking pane a shift drag selects through the same model and reports nothing, and a plain drag is the program's",
      same(s.trackedShift?.range, { start: { line: 201, col: 2 }, end: { line: 201, col: 10 } }) &&
        s.trackedShift?.reported === 0 && s.trackedShift?.painted === "reen 1 r" &&
        same(s.trackedPlain?.range, s.trackedShift?.range) && s.trackedPlain?.reported >= 2,
      JSON.stringify(s),
    );
    ok(
      "a double click takes the word, a triple click the line, and a drag in word mode grows by words",
      same(s.word?.range, { start: { line: 204, col: 9 }, end: { line: 204, col: 12 } }) && s.word?.painted === "row" &&
        same(s.line?.range, { start: { line: 204, col: 0 }, end: { line: 204, col: 24 } }) && s.line?.painted === "screen 4 row" &&
        same(s.wordDrag?.range, { start: { line: 204, col: 0 }, end: { line: 204, col: 12 } }) && s.wordDrag?.painted === "screen 4 row" &&
        same(s.shiftExtended, { start: { line: 202, col: 0 }, end: { line: 205, col: 4 } }),
      JSON.stringify(s),
    );
    ok(
      "only a click or Escape clears a terminal selection, the screen is not browser-selectable, and the paint wears the selection tokens",
      s.clearedByClick?.stands === false && same(s.clearedByClick?.painted, []) &&
        s.standsBeforeEsc === true && s.clearedByEsc?.stands === false && same(s.clearedByEsc?.painted, []) &&
      s.userSelect === "none" && s.selectionColors === true,
      JSON.stringify(s),
    );
    ok(
      "an alternate-screen shift leaves the selection and its paint at the same coordinates",
      same(s.altRange, { start: { line: 203, col: 2 }, end: { line: 205, col: 10 } }) &&
        same(s.altRangeAfterShift, s.altRange) && same(s.altPaintedAfterShift, [3, 4, 5]),
      JSON.stringify(s),
    );
    ok(
      "Escape cancels auto-scroll while a selection drag is still held past the edge",
      s.escapeStoppedDrag === true,
      JSON.stringify(s),
    );
    ok(
      "composed fold rows select and copy their original buffer lines across a scroll",
      same(s.composedRange, { start: { line: 153, col: 0 }, end: { line: 153, col: 8 } }) &&
        s.composedCopy === "hist 153" && same(s.composedPaintAfterScroll, [0]) &&
        s.composedCopyAfterScroll === s.composedCopy,
      JSON.stringify(s),
    );
  }
}
