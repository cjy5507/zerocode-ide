import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

/* A pointer crossing a terminal that asked for motion is told once per cell
 * (t-20972).
 *
 * `claude` opens its terminal with `?1000h ?1002h ?1003h ?1006h` — it asks to
 * be told about every move of the pointer — and the window's reporter sent one
 * `term_mouse` for every DOM `mousemove`: one custom-scheme request, one pty
 * write and one redraw of the program for each PIXEL the pointer travelled,
 * where the program numbers its pointer in cells. xterm reports motion when the
 * pointer enters another cell, and so does every terminal that copied it: a
 * report for the cell the program already has says nothing it does not know.
 *
 * The rule pinned here is that one: a motion report for the cell, the button
 * and the modifiers the program was last told is not sent — and a press or a
 * release always is, because those are not positions. */
export async function testTermMouseMotion(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(async () => {
      const out = {};
      const reports = [];
      window.__ANSWER__.term_mouse = (args) => (reports.push({ ...args.event }), null);
      const term = await openTermTab();
      const view = termView(term);
      await window.__TERM_FEED__(term, {
        rows: [], scrolled_lines: 0, cursor: [0, 0], title: null,
        alt_screen: true, size: [24, 80], full: false, cursor_visible: true,
        mouse_tracking: "motion", mouse_sgr: true,
      });
      view.measure();
      // Where to put the pointer: the middle of a cell of the grid the view holds,
      // never a boundary (a pointer exactly between two cells is one rounding error
      // from either). What the reporter then says is read back from what it says —
      // the checks below compare reports with one another, not with a second
      // copy of its arithmetic.
      const box = view.pre.getBoundingClientRect();
      const { rows: ROWS, cols: COLS } = view.gridSize();
      const centerOf = (col) => box.left + box.width * ((col + 0.5) / COLS);
      const middle = box.top + box.height * (3.5 / ROWS);
      const move = (x) => view.host.dispatchEvent(
        new MouseEvent("mousemove", { clientX: x, clientY: middle, bubbles: true }),
      );
      const press = (x) => view.host.dispatchEvent(
        new MouseEvent("mousedown", { clientX: x, clientY: middle, bubbles: true, cancelable: true, button: 0 }),
      );
      const release = (x) => window.dispatchEvent(
        new MouseEvent("mouseup", { clientX: x, clientY: middle, bubbles: true, button: 0 }),
      );

      // A sweep of the pointer across the whole row, a pixel at a time.
      let events = 0;
      for (let x = Math.ceil(box.left) + 1; x < box.right - 1; x += 1) {
        move(x);
        events += 1;
      }
      out.events = events;
      out.reports = reports.length;
      out.moves = reports.every((one) => one.kind === "move" && one.button === 3);
      out.oneRow = new Set(reports.map((one) => one.row)).size === 1;
      // One report per cell, each the next cell to the right: none twice, none skipped.
      out.eachNext = reports.every((one, at) => at === 0 || one.col === reports[at - 1].col + 1);
      out.spans = reports.length > 0 && reports.at(-1).col - reports[0].col + 1 === reports.length;
      out.firstReports = reports.slice(0, 3).map((one) => ({ col: one.col, row: one.row }));
      out.grid = { rows: ROWS, cols: COLS };

      // The pointer going back to a cell it has left is a new fact.
      reports.length = 0;
      const a = centerOf(10);
      const b = centerOf(11);
      for (const x of [a, a + 1, b, b + 1, a, a + 1, a + 2]) move(x);
      out.back = reports.map((one) => one.col);
      out.backTold = out.back.length === 3 && out.back[1] === out.back[0] + 1 && out.back[2] === out.back[0];
      const cellB = out.back[1];

      // A press and a release are never position news: two clicks on one cell
      // are two presses and two releases.
      reports.length = 0;
      press(a);
      release(a);
      press(a);
      release(a);
      out.clicks = reports.map((one) => one.kind);

      // A drag inside the cell it began in says nothing; the first move into
      // the next cell does, once however many pixels it takes.
      reports.length = 0;
      press(a);
      for (const x of [a + 1, a + 2, a + 3]) move(x);
      for (const x of [b, b + 1, b + 2, b + 3, b + 4]) move(x);
      release(b);
      out.drag = reports.map((one) => `${one.kind}:${one.button}:${one.col}`);
      out.cellB = cellB;

      delete window.__ANSWER__.term_mouse;
      return out;
    });
    ok(
      "a pointer sweeping a program that asked for motion is reported once per cell it enters — in order, never twice for one cell, and back into a cell it left is news",
      seen.events > seen.reports * 2 &&
        seen.reports > 1 &&
        seen.moves &&
        seen.oneRow &&
        seen.eachNext &&
        seen.spans &&
        seen.backTold,
      JSON.stringify(seen),
    );
    ok(
      "a press and a release always go to the program, and a drag is told once per cell it enters",
      JSON.stringify(seen.clicks) === JSON.stringify(["press", "release", "press", "release"]) &&
        JSON.stringify(seen.drag.map((one) => one.split(":")[0])) === JSON.stringify(["press", "move", "release"]) &&
        seen.drag[1] === `move:0:${seen.cellB}`,
      JSON.stringify(seen),
    );
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testTermMouseMotion(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally { await browser.close(); files.close(); }
  if (failures) process.exitCode = 1;
}
