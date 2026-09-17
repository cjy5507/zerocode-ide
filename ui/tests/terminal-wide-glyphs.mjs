import { installTerminalWaits } from "./terminal-grid-selection.mjs";

// A CJK syllable owns two terminal columns, but the mono stack has no Hangul,
// so the OS draws it with a fallback face whose advance is short of two
// columns (Apple SD Gothic Neo: 1.40 columns at 14px). Left alone the row
// slides; closed with letter-spacing alone, every syllable sits at the left
// of its two columns with the whole shortfall as a gap after it, and Korean
// reads as spaced-out letters ("자 체 는"). The renderer grows the glyph
// toward its two columns as far as the row height allows and centers it in
// them, and the grid's next column is exactly where the grid says.
export async function testWideGlyphs(page, ok) {
  await installTerminalWaits(page);
  const wide = await page.evaluate(async () => {
    const priorAnswers = { ...window.__ANSWER__ };
    try {
      window.__ANSWER__.term_snapshot = () => null;
      const term = await openTermTab({ placement: "tab" });
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      const COLS = 40;
      const ROWS = 3;
      const hangul = "한글자간";
      // The grid marks the second column of a double-width glyph with a NUL cell.
      const CONTINUATION = "\u0000";
      const cells = [];
      for (const ch of "ab ") cells.push({ ch });
      for (const ch of hangul) {
        cells.push({ ch });
        cells.push({ ch: CONTINUATION });
      }
      for (const ch of " cd") cells.push({ ch });
      while (cells.length < COLS) cells.push({ ch: " " });
      const blank = () => Array.from({ length: COLS }, () => ({ ch: " " }));
      const frame = {
        rows: [{ index: 0, cells }, { index: 1, cells: blank() }, { index: 2, cells: blank() }],
        scrolled_lines: 0, cursor: [2, 0], title: null, alt_screen: false, size: [ROWS, COLS], full: true,
        cursor_visible: true, mouse_tracking: "off", mouse_sgr: false, bell: false, view_offset: 0,
        scrollback_len: 0, folds: [],
      };
      await window.__TERM_FEED__(term, frame);
      await window.__UNTIL__(() => view.pre.querySelectorAll(":scope > .term-row").length === ROWS, "wide frame");
      const row = view.pre.querySelector(":scope > .term-row");
      const spans = [...row.querySelectorAll("span")];
      const at = spans.findIndex((s) => s.textContent === hangul);
      const run = spans[at];
      const before = spans[at - 1].getBoundingClientRect();
      const after = spans[at + 1].getBoundingClientRect();
      const style = getComputedStyle(run);
      const base = parseFloat(getComputedStyle(view.pre).fontSize);
      return {
        cell: view.cell.width,
        cellHeight: view.cell.height,
        rowHeight: row.getBoundingClientRect().height,
        // From the end of the run before to the start of the run after: the
        // Hangul run's whole footprint on the grid, margins included.
        footprint: after.left - before.right,
        fontScale: parseFloat(style.fontSize) / base,
        lead: parseFloat(style.marginLeft) || 0,
        wide: run.classList.contains("term-wide"),
        short: view.cell.spread > 0 || (view.cell.wideScale ?? 1) > 1,
      };
    } finally {
      Object.assign(window.__ANSWER__, priorAnswers);
    }
  });
  const twoColumnsEach = 2 * 4 * wide.cell;
  ok("a run of wide glyphs takes exactly two columns each, so the next column lands where the grid says",
    Math.abs(wide.footprint - twoColumnsEach) < 0.6 && Math.abs(wide.rowHeight - wide.cellHeight) < 0.6, JSON.stringify(wide));
  ok("a wide glyph the fallback face draws short is grown toward its two columns and centered in them",
    !wide.short || (wide.wide && wide.fontScale > 1 && wide.lead > 0), JSON.stringify(wide));
}
