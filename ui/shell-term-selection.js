/* ---- the terminal's selection, as a model of the grid ------------------
 *
 * What a person dragged over in a terminal is a range of CELLS — a start
 * line and column, an end line and column — and it stays that range while
 * the program keeps printing under it and while the reader wheels through
 * history to see more of it. The browser's own selection could not be that:
 * it lives in Text nodes, the renderer keeps only the visible rows in the DOM
 * and rewrites them on every frame and every scroll, so scrolling took the
 * selection away and nothing past the bottom of the pane could be selected
 * at all (「zo에서 선택 드래그 유지가 안 됨, 스크롤을 내리면…」, 2026-09-08).
 *
 * xterm holds its selection in buffer coordinates (`SelectionModel`) and
 * draws it over whatever rows happen to be on screen (`SelectionRenderLayer`);
 * this is that model in this window's spelling. It knows nothing about the
 * DOM, the pointer, or the clipboard: `ui/shell-term.js` maps a pointer to a
 * cell, paints the rows the model names, and asks the backend for the text
 * between the two points (`term_lines`) — the history the window never held
 * is over there.
 *
 * Coordinates are the ones `TermHit` speaks (crates/zerocode-pty/src/grid.rs):
 * `line` is absolute over history and screen — 0 is the oldest line the
 * scrollback holds, `scrollback_len + row` is a row of the live screen — and
 * `col` is a cell BOUNDARY, 0 before the first cell and `cols` after the
 * last. A span's `to` is exclusive. The alternate screen has no history, so
 * there the same rule names rows and nothing else. */

/* Auto-scroll, for a drag that leaves the screen by its top or bottom edge:
 * every `intervalMs` the view moves by up to `maxRows` rows, reaching that
 * speed `thresholdPx` past the edge. xterm's own three (SelectionService:
 * DRAG_SCROLL_INTERVAL, DRAG_SCROLL_MAX_THRESHOLD, DRAG_SCROLL_MAX_SPEED). */
const TERM_SELECT_DRAG_SCROLL = Object.freeze({
  intervalMs: 50,
  thresholdPx: 50,
  maxRows: 15,
});

// MouseEvent.buttons uses a bitmask: the primary button may be held with
// another button, so equality with this value would end a valid drag.
const TERM_SELECT_PRIMARY_BUTTON_MASK = 1;

/* What makes a second press a double click and a third a triple: within
 * `windowMs` of the press before it, and within `slopPx` of where that press
 * landed. Counted here rather than read off `event.detail` so the rule is one
 * number in one place, and so a press the renderer receives synthetically is
 * counted by the same clock (xterm: CLEAR_MOUSE_DOWN_TIME, CLEAR_MOUSE_DISTANCE). */
const TERM_SELECT_MULTI_CLICK = Object.freeze({
  windowMs: 400,
  slopPx: 10,
});

/* How many rows one auto-scroll tick moves for a pointer `distancePx` past
 * the edge: at least one, up to the table's ceiling, in proportion. */
function termSelectDragRows(distancePx) {
  const share = Math.max(0, distancePx) / TERM_SELECT_DRAG_SCROLL.thresholdPx;
  const rows = Math.ceil(share * TERM_SELECT_DRAG_SCROLL.maxRows);
  return Math.min(TERM_SELECT_DRAG_SCROLL.maxRows, Math.max(1, rows));
}

/* Which press of a run this is — 1, 2 or 3 — by the table above. A fourth
 * press starts over at one, the way every terminal's triple click ends. */
function makeTermClickCounter() {
  let last = null;
  return {
    count(x, y, now) {
      const near = last !== null &&
        now - last.at <= TERM_SELECT_MULTI_CLICK.windowMs &&
        Math.abs(x - last.x) <= TERM_SELECT_MULTI_CLICK.slopPx &&
        Math.abs(y - last.y) <= TERM_SELECT_MULTI_CLICK.slopPx;
      const count = near ? (last.count % 3) + 1 : 1;
      last = { x, y, at: now, count };
      return count;
    },
  };
}

/* Where a pointer at (`x`, `y`) — relative to the screen's top-left corner —
 * lands on a grid of `rows` × `cols` cells of the given size.
 *
 * Three answers, because three gestures ask three different questions. `row`
 * is the row under the pointer. `col` is the nearest cell BOUNDARY, which is
 * where a selection's edge goes: a caret dropped between letters lands on
 * whichever side of the glyph the pointer is on, and a drag that rounds the
 * same way feels like every text field. `cell` is the cell itself, which is
 * what a double click names the word under. Past the top or bottom the
 * answer is the edge row's outer corner, and `edge` says which — the drag
 * keeps growing while the view auto-scrolls (-1 above, 1 below, 0 inside).
 * `null` when there is no grid to speak of. */
function termPointOnGrid(x, y, cellWidth, cellHeight, cols, rows) {
  if (!(cellWidth > 0) || !(cellHeight > 0) || !(rows > 0) || !(cols > 0)) return null;
  const above = y < 0;
  const below = y >= rows * cellHeight;
  const row = above ? 0 : below ? rows - 1 : Math.floor(y / cellHeight);
  const boundary = Math.min(cols, Math.max(0, Math.round(x / cellWidth)));
  const cell = Math.min(cols - 1, Math.max(0, Math.floor(x / cellWidth)));
  return {
    row,
    col: above ? 0 : below ? cols : boundary,
    cell: above ? 0 : below ? cols - 1 : cell,
    edge: above ? -1 : below ? 1 : 0,
  };
}

/* The word a double click names, as a column span `[from, to)` of one row's
 * cells: the run of non-separators around `col`. A separator under the
 * pointer is no word — a terminal row is padded to its full width, so "the
 * word here" on a blank would be eighty spaces. The second column of a wide
 * glyph (`continuation`) belongs to the glyph in front of it and never breaks
 * a word. The separators are the shell's, not prose's — a path is one word
 * (Orca's `wordSeparator` option; the list rides the settings snapshot). */
function termWordSpan(cells, col, separators, continuation) {
  const separates = (index) => {
    const cell = cells[index];
    if (cell === undefined) return true;
    if (cell.ch === continuation) return false;
    return separators.includes(cell.ch);
  };
  if (col < 0 || col >= cells.length || separates(col)) return null;
  let from = col;
  let to = col + 1;
  while (from > 0 && !separates(from - 1)) from -= 1;
  while (to < cells.length && !separates(to)) to += 1;
  return { from, to };
}

function termPointBefore(a, b) {
  return a.line < b.line || (a.line === b.line && a.col < b.col);
}

/* The model itself: two SPANS, the one the gesture began on and the one the
 * pointer is on now, and the selection is everything between their outer
 * edges. A span is a degenerate point in character mode, a word in word mode
 * and a whole row in line mode — which is what makes a drag in word mode grow
 * by words and a drag in line mode grow by rows without a case for either
 * here: the union of two spans is the union of two spans. Ordering is the
 * reader's, not the gesture's: a drag upward has its start above its end. */
function makeTermSelection() {
  let anchor = null;
  let focus = null;

  function range() {
    if (anchor === null || focus === null) return null;
    const start = termPointBefore(focus.from, anchor.from) ? focus.from : anchor.from;
    const end = termPointBefore(anchor.to, focus.to) ? focus.to : anchor.to;
    if (!termPointBefore(start, end)) return null;
    return { start, end };
  }

  function clear() {
    anchor = null;
    focus = null;
  }

  return {
    /* A press: this span, and nothing else. A plain click is a press whose
     * span is a point, which is how a click clears what stood before. */
    begin(span) {
      anchor = span;
      focus = span;
    },
    /* The pointer moved, or shift+click named a new far end. */
    extend(span) {
      if (anchor !== null) focus = span;
    },
    clear,
    /* Whether a press has happened at all — a shift+click has something to
     * extend from even when the last click left an empty selection. */
    anchored: () => anchor !== null,
    /* Whether there is anything to paint or copy. */
    stands: () => range() !== null,
    range,
    /* The columns of `line` that are selected, `[from, to)`, or `null` —
     * `width` being how many cells the row has, which bounds a boundary that
     * a shorter row cannot reach. */
    columnsOn(line, width) {
      const held = range();
      if (held === null || line < held.start.line || line > held.end.line) return null;
      const from = line === held.start.line ? held.start.col : 0;
      const to = line === held.end.line ? Math.min(held.end.col, width) : width;
      return from < to ? { from, to } : null;
    },
    /* History was trimmed at its cap: every line moved up by `lines`. The
     * selection follows the text it was on, the way a reader's view does;
     * what fell off the top is gone. */
    shift(lines) {
      if (!(lines > 0)) return;
      const held = range();
      // A retired range has no anchor for the next Shift-click to extend.
      // The end is exclusive, so an end at the retained origin is gone too.
      if (held !== null && !termPointBefore({ line: lines, col: 0 }, held.end)) {
        clear();
        return;
      }
      const move = (point) => ({ line: Math.max(0, point.line - lines), col: point.line - lines < 0 ? 0 : point.col });
      const moveSpan = (span) => ({ from: move(span.from), to: move(span.to) });
      if (anchor !== null) anchor = moveSpan(anchor);
      if (focus !== null) focus = moveSpan(focus);
    },
  };
}
