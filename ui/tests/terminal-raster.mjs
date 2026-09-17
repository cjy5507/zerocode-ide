/* What the terminal fidelity measurement found wrong in how a screen is
 * RASTERISED, held against a layout engine (tools/terminal-fidelity,
 * 2026-09-17: the same bytes in this window and in Terminal.app, captured and
 * read cell by cell). Shared by the window gate and a focused Chromium/WebKit
 * runner: node ui/tests/terminal-raster.mjs. */
import { installTerminalWaits } from "./terminal-grid-selection.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

// Claude Code prints these on every tool result and spinner line. The mono
// stack has no glyph for any of them, so each is drawn by a fallback face with
// an advance of its own: measured in WebKit at 15px against a 9.273px cell,
// ⎿ +4.58px, ⠋ +0.98px, ✻ −0.24px (Chromium: +5.97, +1.22, 0).
const OFF_PITCH = "⎿⠋✻";
// A column edge is a device pixel's business; anything under this is the
// engine's rounding, not a glyph pulling the grid.
const COLUMN_SLACK_PX = 0.6;
// Two palette colours taken in turn, only so neighbouring bars never share a run.
const BAR_COLOURS = [2, 3];

export async function testTerminalRaster(page, ok) {
  await installTerminalWaits(page);
  const seen = await page.evaluate(async ({ OFF_PITCH, BAR_COLOURS }) => {
    const priorAnswers = { ...window.__ANSWER__ };
    let term;
    try {
      window.__ANSWER__.term_snapshot = () => null;
      term = await openTermTab({ placement: "tab" });
      const view = termView(term);
      setActiveTab(tabOfTerm(term).id);
      await window.__TERMINAL_READY__(term, view);
      const COLS = 40;
      const ROWS = 3;
      const pad = (cells) => {
        while (cells.length < COLS) cells.push({ ch: " " });
        return cells;
      };
      // Row 0: each symbol then a bar. Row 1: the same columns in ASCII. The
      // bars must stand in the same columns on both rows. Each bar wears a
      // colour of its own so it is a span of its own: a span's box is read at
      // the layout's precision in both engines, where WebKit floors the rect of
      // a character inside a run to a whole pixel.
      const bar = (at) => ({ ch: "|", style: { fg: { indexed: BAR_COLOURS[at % BAR_COLOURS.length] } } });
      const probe = pad([...OFF_PITCH].flatMap((ch, at) => [{ ch }, bar(at)]));
      const reference = pad([...OFF_PITCH].flatMap((ch, at) => [{ ch: "W" }, bar(at)]));
      const frame = {
        rows: [
          { index: 0, cells: probe },
          { index: 1, cells: reference },
          { index: 2, cells: pad([]) },
        ],
        scrolled_lines: 0, cursor: [2, 0], title: null, alt_screen: false, size: [ROWS, COLS], full: true,
        cursor_visible: true, mouse_tracking: "off", mouse_sgr: false, bell: false, view_offset: 0,
        scrollback_len: 0, folds: [],
      };
      await window.__TERM_FEED__(term, frame);
      await window.__UNTIL__(() => view.pre.querySelectorAll(":scope > .term-row").length === ROWS, "raster frame");
      const barsOf = (row) => [...row.querySelectorAll("span")]
        .filter((span) => span.textContent === "|")
        .map((span) => span.getBoundingClientRect().left);
      const rows = view.pre.querySelectorAll(":scope > .term-row");
      return {
        cell: view.cell.width,
        probe: barsOf(rows[0]),
        reference: barsOf(rows[1]),
        termSmoothing: getComputedStyle(view.pre).webkitFontSmoothing ?? null,
        bodySmoothing: getComputedStyle(document.body).webkitFontSmoothing ?? null,
      };
    } finally {
      const tab = term === undefined ? null : tabOfTerm(term);
      if (tab) {
        dropTermView(term);
        dropTab(tab.id);
      }
      window.__ANSWER__ = priorAnswers;
    }
  }, { OFF_PITCH, BAR_COLOURS });

  // The body's rule stays the window's: chrome copy keeps the lighter
  // rendering its comment asks for. The screen does not inherit it — with it,
  // WebKit drew every stroke without the platform's smoothing: a `|` stem at
  // 0.76 peak coverage against Terminal.app's 1.0, 16% less ink in `H`, and a
  // box line split over two pixel rows so 141 of its 250 columns never reached
  // half coverage. The same page with `auto` measured 0.99, Terminal.app's
  // edge sharpness (0.541 vs 0.542 partial pixels), and no broken columns.
  ok("a terminal screen keeps the platform's font smoothing while the window's chrome stays antialiased",
    seen.bodySmoothing === null || (seen.bodySmoothing === "antialiased" && seen.termSmoothing !== "antialiased"),
    JSON.stringify({ body: seen.bodySmoothing, term: seen.termSmoothing }));

  const shifts = seen.probe.map((left, at) => left - seen.reference[at]);
  ok("a narrow glyph a fallback face draws off the cell's pitch still ends on its own column",
    seen.probe.length === OFF_PITCH.length && seen.reference.length === OFF_PITCH.length &&
      shifts.every((shift) => Math.abs(shift) < COLUMN_SLACK_PX),
    JSON.stringify({ cell: seen.cell, shifts: shifts.map((shift) => Math.round(shift * 100) / 100) }));
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
        await testTerminalRaster(page, record);
        if (faults.length) throw new Error(faults.join("\n"));
      } finally { await browser.close(); }
    }
  } finally { files.close(); }
}
