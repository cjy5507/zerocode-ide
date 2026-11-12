import { installTerminalWaits } from "./terminal-grid-selection.mjs";
import { runInBothEngines, startedAs } from "./both-engines.mjs";

/* A CJK syllable owns two terminal columns, but the mono stack has no Hangul,
 * so the OS draws it with a fallback face whose advance is short of two
 * columns (Apple SD Gothic Neo: 1.40 columns at 14px). Left alone the row
 * slides; closed with letter-spacing alone, every syllable sits at the left
 * of its two columns with the whole shortfall as a gap after it; grown as far
 * as the row allows and centered (2026-09-10) there were still 4.08px of
 * tracking between two syllables, and Korean read as spaced-out letters
 * ("사 이 드 바", 2026-11-11). The renderer now grows the glyph as far as the
 * row allows its ink, widens it toward its two columns, and centers what is
 * still short — and asks each double-width glyph for its own advance, because
 * an emoji or a Han character is drawn by another face at another one.
 *
 * Shared by the window gate and a focused Chromium/WebKit runner:
 * node ui/tests/terminal-wide-glyphs.mjs. */

// A column edge is a device pixel's business; anything under this is the
// engine's rounding, not a glyph pulling the grid.
const COLUMN_SLACK_PX = 0.6;
// One of each kind of glyph the grid counts as two columns: Hangul, a Han
// character, kana, a full-width letter, and two pictures.
const WIDE_KINDS = "한漢かＡ😀🎉";
// Two palette colours taken in turn, only so neighbouring bars never share a run.
const BAR_COLOURS = [2, 3];
// How much of its two columns a glyph the fallback face draws short must
// cover once it is fitted — unless it has been widened as far as it may be.
const COLUMNS_FILLED = 0.9;
// Two syllables whose ink runs edge to edge (ㅁ over ㅡ over ㅁ) stand closer
// than this fraction of a cell once their columns are filled. With tracking
// between them they stood half a cell apart.
const INK_GAP_CELLS = 0.3;
// The most a fallback glyph is widened, and the air a glyph widened that far
// keeps to itself, in cells (the person's choice of four, 2026-11-12: widened
// by a quarter until no tracking was left, Korean read as packed and as
// another face).
const WIDEN_MOST = 1.15;
const AIR_LEAST_CELLS = 0.1;
// A pixel is ink when it differs from the screen's ground by this much of a
// channel's range: past the fringe of an antialiased edge, short of a stroke.
const INK_CONTRAST = 96;
// The local mono faces a fixture font is cut from, one per platform this
// window runs on. Only its advance matters.
const LOCAL_MONO = ["Menlo", "Consolas", "DejaVu Sans Mono", "Liberation Mono", "Courier New"];
// How many times that face is cut toward half the fallback's advance, and how
// near two of its cells must come to that advance to count as spanning it —
// inside what the window itself calls the same advance (a hundredth of a pixel).
const FIXTURE_CUTS = 6;
const FIXTURE_SLACK_PX = 0.005;

// The many-blank line: this many one-syllable words, a blank between each two,
// all riding in one run (t-103682's delta review: a rounding a blank carries
// adds up along the line).
const RIDING_WORDS = 21;
// Font sizes the underline is read at: the default, and two past 20px, where an
// engine's own underline gap may grow (t-103682's review, 5).
const UNDERLINE_SIZES = [null, 22, 24];
// How many rows of one underlined line the alignment is read on. A row
// stands 1.15 font sizes under the one before it, so ten of them meet the
// device's pixels at as many fractions as a row can: an engine that rounds a
// pressed box onto those pixels (WebKit) misses on some rows and not others,
// and which ones moves with where the screen starts — the count does not.
const UNDERLINE_ROWS = 10;
// The ink and the ground of the painted underlined rows: the run's own colour
// is what tells its line from a ground that is not the screen's.
const UNDERLINE_PAINT = { fg: [250, 250, 250], bg: [0, 0, 160] };
// One-syllable words across the row with a blank between each two, after two
// underlined narrow blanks — 77 of its 80 columns in one run.
const ACROSS_WORDS = 26;
// The fewest columns into that run the last blank read must stand — far past
// the one cell the line's single blank is before its tracking carries it.
const ACROSS_READ_LEAST_COLS = 20;

const WIDE_RIG = "__WIDE_GLYPH_RIG__";

async function standRig(page) {
  return page.evaluate(async ({ WIDE_RIG, WIDE_KINDS, BAR_COLOURS, RIDING_WORDS, UNDERLINE_PAINT, ACROSS_WORDS, UNDERLINE_ROWS }) => {
    const priorAnswers = { ...window.__ANSWER__ };
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const term = await openTermTab({ placement: "tab" });
    const view = termView(term);
    setActiveTab(tabOfTerm(term).id);
    await window.__TERMINAL_READY__(term, view);
    const COLS = 80;
    // The grid marks the second column of a double-width glyph with a NUL cell.
    const CONTINUATION = "\u0000";
    const wideCells = (text, style) => [...text].flatMap((ch) => [style ? { ch, style } : { ch }, { ch: CONTINUATION }]);
    const narrowCells = (text, style) => [...text].map((ch) => (style ? { ch, style } : { ch }));
    const pad = (cells) => {
      while (cells.length < COLS) cells.push({ ch: " " });
      return cells;
    };
    const hangul = "한글자간";
    const riders = ["가나", "다라"];
    const bar = (at) => ({ ch: "|", style: { fg: { indexed: BAR_COLOURS[at % BAR_COLOURS.length] } } });
    const dressed = { bg: { indexed: 4 }, underline: true };
    const painted = { fg: { rgb: UNDERLINE_PAINT.fg }, bg: { rgb: UNDERLINE_PAINT.bg } };
    const lined = { ...painted, underline: true };
    const rowsOf = () => [
      // 0: a Hangul run between two narrow runs — its footprint on the grid.
      pad([...narrowCells("ab "), ...wideCells(hangul), ...narrowCells(" cd")]),
      pad([]),
      // 2: one of each kind of wide glyph, each followed by a bar in a span of its own.
      pad([...WIDE_KINDS].flatMap((ch, at) => [...wideCells(ch), bar(at)])),
      pad([]),
      // 4: a narrow and a wide run in one dress — the strips their grounds paint.
      pad([...narrowCells("ab", dressed), ...wideCells("한글", dressed), ...narrowCells("cd", dressed)]),
      pad([]),
      // 6: ink that runs edge to edge, then the tallest and the deepest syllable,
      // between two empty rows.
      pad([...narrowCells("  "), ...wideCells("믐믐"), ...narrowCells("  "), ...wideCells("홁퉜")]),
      pad([]),
      // 8: two wide words with one blank between them, then a bar — the blank
      // rides in their run and still takes one cell.
      pad([...wideCells(riders[0]), ...narrowCells(" "), ...wideCells(riders[1]), bar(0)]),
      pad([]),
      // 10: a bold Hangul run, then a bar — bold is fitted as the face draws bold.
      pad([...wideCells(hangul, { bold: true }), bar(1)]),
      pad([]),
      // 12: one-syllable words with a blank between each two, then a bar: every
      // blank rides in the one run, and the bar still lands on its column.
      pad([...Array.from({ length: RIDING_WORDS }, (_, at) => [...(at ? narrowCells(" ") : []), ...wideCells("가")]).flat(), bar(0)]),
      pad([]),
      // 14: underlined narrow blanks, two syllables with a blank riding between
      // them and painted blanks with no underline, all on a painted ground: the
      // run's line shows over its ground and stops at its last column.
      pad([...narrowCells("  ", lined), ...wideCells("한", lined), ...narrowCells(" ", lined), ...wideCells("글", lined),
        ...narrowCells("  ", painted)]),
      pad([]),
      // 16: underlined narrow blanks, then one run across the row: the line
      // reaches the last blank riding in it that the screen shows.
      pad([...narrowCells("  ", lined),
        ...Array.from({ length: ACROSS_WORDS }, (_, at) => [...(at ? narrowCells(" ", lined) : []), ...wideCells("가", lined)]).flat()]),
      pad([]),
      // 18 on: an underline over narrow blanks, two syllables with a blank riding
      // between them, and narrow blanks again, row after row. Under the riding
      // blank the wide run's underline stands alone — a grown syllable's ink
      // reaches below it.
      ...Array.from({ length: UNDERLINE_ROWS }, () => pad([...narrowCells("    ", { underline: true }), ...wideCells("한", { underline: true }),
        ...narrowCells(" ", { underline: true }), ...wideCells("글", { underline: true }), ...narrowCells("    ", { underline: true })])),
    ];
    // `cursor` puts the caret on a cell; left out, it stands hidden on the last row.
    const feed = async (cursor = null) => {
      const rows = rowsOf().map((cells, index) => ({ index, cells }));
      await window.__TERM_FEED__(term, {
        rows, scrolled_lines: 0, cursor: cursor ?? [rows.length - 1, 0], title: null, alt_screen: false, size: [rows.length, COLS],
        full: true, cursor_visible: cursor !== null, mouse_tracking: "off", mouse_sgr: false, bell: false, view_offset: 0,
        scrollback_len: 0, folds: [],
      });
      await window.__UNTIL__(() => view.pre.querySelectorAll(":scope > .term-row").length === rows.length
        && view.pre.querySelector(":scope > .term-row")?.textContent.includes(hangul), "wide frame");
    };
    // The screen re-measures a frame after a token moves on the root.
    const settle = async () => {
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      view.measure();
    };
    // What the face draws a glyph at, left alone: a probe built like a painted row.
    const natural = (glyph) => {
      const row = document.createElement("div");
      row.className = "term-row";
      const span = document.createElement("span");
      span.textContent = glyph.repeat(20);
      span.style.display = "inline-block";
      row.appendChild(span);
      view.pre.appendChild(row);
      const width = parseFloat(getComputedStyle(span).width) / 20;
      row.remove();
      return width;
    };
    const rowAt = (index) => view.pre.querySelectorAll(":scope > .term-row")[index];
    const runOf = (index, text) => [...rowAt(index).querySelectorAll("span")].find((span) => span.textContent === text);
    // The Hangul run of row 0: from the end of the run before to the start of
    // the run after — its whole footprint on the grid, margins included.
    const footprint = () => {
      const spans = [...rowAt(0).querySelectorAll("span")];
      const at = spans.findIndex((span) => span.textContent === hangul);
      return {
        cell: view.cell.width,
        cellHeight: view.cell.height,
        rowHeight: rowAt(0).getBoundingClientRect().height,
        footprint: spans[at + 1].getBoundingClientRect().left - spans[at - 1].getBoundingClientRect().right,
        glyphs: [...hangul].length,
        classes: spans[at].className,
      };
    };
    window[WIDE_RIG] = {
      term, view, hangul, riders, feed, settle, natural, rowAt, runOf, footprint, COLS,
      leave() {
        const tab = tabOfTerm(term);
        if (tab) {
          dropTermView(term);
          dropTab(tab.id);
        }
        window.__ANSWER__ = priorAnswers;
        delete window[WIDE_RIG];
      },
    };
    await feed();
  }, { WIDE_RIG, WIDE_KINDS, BAR_COLOURS, RIDING_WORDS, UNDERLINE_PAINT, ACROSS_WORDS, UNDERLINE_ROWS });
}

/* The rows as they stand under the window's own font. */
async function readRows(page) {
  return page.evaluate(({ WIDE_RIG, WIDE_KINDS, RIDING_WORDS }) => {
    const rig = window[WIDE_RIG];
    const { view } = rig;
    const cell = view.cell.width;
    const left = rig.rowAt(0).getBoundingClientRect().left;
    const base = parseFloat(getComputedStyle(view.pre).fontSize);
    const fitOf = (run) => {
      const style = getComputedStyle(run);
      return {
        classes: run.className,
        fontScale: parseFloat(style.fontSize) / base,
        tracking: parseFloat(style.letterSpacing) || 0,
        stretch: parseFloat(style.getPropertyValue("--term-wide-stretch")) || 1,
        transform: style.transform,
      };
    };
    const run = rig.runOf(0, rig.hangul);
    const advance = rig.natural([...rig.hangul][0]);
    // Row 2: where each bar stands against the column the grid gave it.
    const bars = [...rig.rowAt(2).querySelectorAll("span")].filter((span) => span.textContent === "|");
    const shifts = bars.map((span, at) => span.getBoundingClientRect().left - left - (at * 3 + 2) * cell);
    const pictures = [...WIDE_KINDS].filter((ch) => /\p{Extended_Pictographic}/u.test(ch))
      .map((ch) => ({ ch, ...fitOf(rig.runOf(2, ch)) }));
    // Row 4: the strip a narrow run paints, and the wide run's beside it.
    const box = (span) => {
      const rect = span.getBoundingClientRect();
      const row = rig.rowAt(4).getBoundingClientRect();
      return { top: rect.top - row.top, height: rect.height, left: rect.left - left, right: rect.right - left };
    };
    const inkRow = rig.rowAt(6).getBoundingClientRect();
    // Row 8: the runs its glyphs stand in, and the bar after two wide words
    // and their blank — nine columns in.
    const ridden = [...rig.rowAt(8).querySelectorAll("span")];
    const afterRiders = ridden.find((span) => span.textContent === "|");
    const riderColumns = 2 * [...rig.riders.join("")].length + 1;
    // Where the bar after a row's last run stands against the column it was given.
    const barShift = (index, column) => {
      const bar = [...rig.rowAt(index).querySelectorAll("span")].find((span) => span.textContent === "|");
      return bar.getBoundingClientRect().left - left - column * cell;
    };
    const glyphRuns = (index) => [...rig.rowAt(index).querySelectorAll("span")].map((span) => span.textContent).filter((text) => /\p{Script=Hangul}/u.test(text));
    return {
      ...rig.footprint(),
      advance,
      short: advance < 2 * cell - 0.01,
      fit: fitOf(run),
      // What the screen measured of the face, for whoever reads this line in a log.
      measured: { ...view.cell.wide },
      // The window's own ceiling; a window with none has not widened anything.
      stretchLimit: typeof WIDE_GLYPH_STRETCH_LIMIT === "undefined" ? Infinity : WIDE_GLYPH_STRETCH_LIMIT,
      bars: bars.length,
      shifts,
      pictures,
      riding: {
        runs: ridden.map((span) => span.textContent).filter((text) => text.trim() !== ""),
        whole: rig.riders.join(" "),
        shift: afterRiders.getBoundingClientRect().left - left - riderColumns * cell,
      },
      bold: {
        classes: rig.runOf(10, rig.hangul)?.className ?? null,
        advance: rig.natural([...rig.hangul][0]),
        shift: barShift(10, 2 * [...rig.hangul].length),
      },
      manyBlanks: {
        runs: glyphRuns(12).length,
        shift: barShift(12, 2 * RIDING_WORDS + (RIDING_WORDS - 1)),
      },
      narrowStrip: box(rig.runOf(4, "ab")),
      wideStrip: box(rig.runOf(4, "한글")),
      // The ink row with half of the empty row above it and half of the one
      // below, the first fourteen columns: clear of row 4's ground on every edge.
      clip: { x: left, y: inkRow.top - view.cell.height / 2, width: 14 * cell, height: 2 * view.cell.height },
    };
  }, { WIDE_RIG, WIDE_KINDS, RIDING_WORDS });
}

/* The caret over the first syllable of row 0: its box against two columns, and
 * the copy it draws against the run that glyph stands in — its dress and the
 * box it stands in on the screen. */
async function readCaret(page) {
  return page.evaluate(async ({ WIDE_RIG }) => {
    const rig = window[WIDE_RIG];
    const { view } = rig;
    const caret = view.pre.parentElement.querySelector(":scope > .term-caret");
    await rig.feed([0, 3]);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
    const run = rig.runOf(0, rig.hangul);
    const copy = caret.firstElementChild ?? caret;
    const fit = (node) => {
      const style = getComputedStyle(node);
      const box = node.getBoundingClientRect();
      return { size: parseFloat(style.fontSize), tracking: parseFloat(style.letterSpacing) || 0, transform: style.transform,
        left: box.left, top: box.top, height: box.height };
    };
    const seen = {
      shown: !caret.hidden,
      text: caret.textContent,
      first: [...rig.hangul][0],
      width: caret.getBoundingClientRect().width,
      cell: view.cell.width,
      copyClasses: copy === caret ? "" : copy.className,
      runClasses: run.className,
      copy: fit(copy),
      run: fit(run),
    };
    await rig.feed();
    return seen;
  }, { WIDE_RIG });
}

/* The line drawn between each pair of columns in `spans` of a picture of
 * `clip`: the top and the bottom of the pixel rows that carry ink there, inset
 * a fifth of a cell from each column edge, in CSS pixels — or null where
 * nothing is. Ink is what differs from the picture's first pixel, the screen's
 * ground, or — given `ink`, over a painted ground — what stands near that colour. */
async function readLines(page, clip, cell, spans, ink = null) {
  const png = (await page.screenshot({ clip })).toString("base64");
  return page.evaluate(async ({ png, clip, cell, spans, ink, INK_CONTRAST }) => {
    const bytes = Uint8Array.from(atob(png), (ch) => ch.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext("2d");
    context.drawImage(bitmap, 0, 0);
    const { data } = context.getImageData(0, 0, bitmap.width, bitmap.height);
    const scale = bitmap.width / clip.width;
    const apart = (at, rgb) => Math.max(Math.abs(data[at] - rgb[0]), Math.abs(data[at + 1] - rgb[1]), Math.abs(data[at + 2] - rgb[2]));
    const ground = [data[0], data[1], data[2]];
    const inked = (x, y) => {
      const at = (y * bitmap.width + x) * 4;
      return ink === null ? apart(at, ground) > INK_CONTRAST : apart(at, ink) < INK_CONTRAST;
    };
    return spans.map(([from, to]) => {
      const x0 = Math.floor((from + 0.2) * cell * scale);
      const x1 = Math.ceil((to - 0.2) * cell * scale);
      const rows = [];
      for (let y = 0; y < bitmap.height; y += 1) {
        for (let x = x0; x < x1; x += 1) if (inked(x, y)) { rows.push(y); break; }
      }
      return rows.length === 0 ? null : { top: rows[0] / scale, bottom: (rows[rows.length - 1] + 1) / scale };
    });
  }, { png, clip, cell, spans, ink, INK_CONTRAST });
}

/* The underline of rows 18 on at each of `UNDERLINE_SIZES`: under each row's
 * narrow blanks before and after, and under the blank riding in its wide run
 * (column 6) — each the underline alone — with the size of a device pixel. */
async function readUnderlines(page) {
  const seen = [];
  try {
    for (const size of UNDERLINE_SIZES) {
      const at = await page.evaluate(async ({ WIDE_RIG, size, UNDERLINE_ROWS }) => {
        const rig = window[WIDE_RIG];
        const root = document.documentElement;
        rig.heldSize ??= root.style.getPropertyValue("--term-font-size");
        if (size !== null) root.style.setProperty("--term-font-size", `${size}px`);
        else if (rig.heldSize) root.style.setProperty("--term-font-size", rig.heldSize);
        else root.style.removeProperty("--term-font-size");
        await rig.settle();
        return {
          size: parseFloat(getComputedStyle(rig.view.pre).fontSize),
          pixel: 1 / devicePixelRatio,
          cell: rig.view.cell.width,
          clips: Array.from({ length: UNDERLINE_ROWS }, (_, at) => {
            const row = rig.rowAt(18 + at).getBoundingClientRect();
            return { x: row.left, y: row.top, width: 13 * rig.view.cell.width, height: row.height };
          }),
        };
      }, { WIDE_RIG, size, UNDERLINE_ROWS });
      const rows = [];
      for (const clip of at.clips) {
        const [narrow, wide, after] = await readLines(page, clip, at.cell, [[0, 4], [6, 7], [9, 13]]);
        rows.push({ narrow, wide, after });
      }
      seen.push({ size: at.size, pixel: at.pixel, rows });
    }
  } finally {
    await page.evaluate(async (WIDE_RIG) => {
      const rig = window[WIDE_RIG];
      if (rig.heldSize) document.documentElement.style.setProperty("--term-font-size", rig.heldSize);
      else document.documentElement.style.removeProperty("--term-font-size");
      await rig.settle();
    }, WIDE_RIG);
  }
  return seen;
}

/* Rows 14 and 16 at the default size, on a painted ground: the line under the
 * blank riding in a short run beside the one under the narrow blanks before
 * it, and none under the blanks just past the run; and the line under the
 * last blank riding in a run across the row that the screen shows — a test
 * screen is narrower than 80 columns — beside the one under the narrow blanks
 * that open the row. */
async function readUnderlineReach(page) {
  const at = await page.evaluate((WIDE_RIG) => {
    const rig = window[WIDE_RIG];
    const shown = rig.view.pre.getBoundingClientRect().right;
    const clip = (index) => {
      const row = rig.rowAt(index).getBoundingClientRect();
      return { x: row.left, y: row.top, width: Math.min(rig.COLS * rig.view.cell.width, shown - row.left), height: row.height };
    };
    return { cell: rig.view.cell.width, pixel: 1 / devicePixelRatio, short: clip(14), across: clip(16) };
  }, WIDE_RIG);
  // The riding blanks of row 16 stand at columns 4, 7, 10, …: the last whole one on screen.
  const shownCols = Math.floor(at.across.width / at.cell);
  const lastBlank = Math.min(3 * (ACROSS_WORDS - 2) + 4, 3 * Math.floor((shownCols - 5) / 3) + 4);
  const [narrow, wide, past] = await readLines(page, at.short, at.cell, [[0, 2], [4, 5], [7, 9]], UNDERLINE_PAINT.fg);
  const [opening, last] = await readLines(page, at.across, at.cell, [[0, 2], [lastBlank, lastBlank + 1]], UNDERLINE_PAINT.fg);
  return { pixel: at.pixel, narrow, wide, past, opening, last, lastBlank, shownCols };
}

/* Where the ink of a picture of the ink row stands: its top and bottom in CSS
 * pixels from the picture's top, and the widest empty stretch between the two
 * edge-to-edge syllables (columns 2 to 6). The picture's first pixel is the
 * screen's ground. */
async function readInk(page, png, clip, cell) {
  return page.evaluate(async ({ png, clip, cell, INK_CONTRAST }) => {
    const bytes = Uint8Array.from(atob(png), (ch) => ch.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext("2d");
    context.drawImage(bitmap, 0, 0);
    const { data } = context.getImageData(0, 0, bitmap.width, bitmap.height);
    const scale = bitmap.width / clip.width;
    const inked = (x, y) => {
      const at = (y * bitmap.width + x) * 4;
      return Math.max(Math.abs(data[at] - data[0]), Math.abs(data[at + 1] - data[1]), Math.abs(data[at + 2] - data[2])) > INK_CONTRAST;
    };
    let top = bitmap.height;
    let bottom = -1;
    const columns = new Array(bitmap.width).fill(false);
    for (let y = 0; y < bitmap.height; y += 1) {
      for (let x = 0; x < bitmap.width; x += 1) {
        if (!inked(x, y)) continue;
        columns[x] = true;
        if (y < top) top = y;
        if (y > bottom) bottom = y;
      }
    }
    // The pair stands on columns 2 to 6; the widest empty stretch between its
    // first and last inked pixel column is the gap between the two syllables.
    const from = Math.floor(2 * cell * scale);
    const to = Math.ceil(6 * cell * scale);
    const pair = columns.slice(from, to);
    const first = pair.indexOf(true);
    const last = pair.lastIndexOf(true);
    let gap = 0;
    let empty = 0;
    for (let x = first; x <= last; x += 1) {
      empty = pair[x] ? 0 : empty + 1;
      if (empty > gap) gap = empty;
    }
    return { top: top / scale, bottom: (bottom + 1) / scale, gap: gap / scale, inked: bottom >= 0 && first >= 0 };
  }, { png, clip, cell, INK_CONTRAST });
}

/* The same rows after the font moved under them. */
async function readChanges(page) {
  return page.evaluate(async ({ WIDE_RIG, LOCAL_MONO, FIXTURE_CUTS, FIXTURE_SLACK_PX }) => {
    const rig = window[WIDE_RIG];
    const { view } = rig;
    const root = document.documentElement;
    const seen = {};
    const keep = (name) => {
      const held = root.style.getPropertyValue(name);
      return () => (held ? root.style.setProperty(name, held) : root.style.removeProperty(name));
    };

    // A larger font, and no new frame: the run already painted follows the
    // measurement, because its fit sits once on the terminal.
    const size = parseFloat(getComputedStyle(view.pre).fontSize);
    const giveSizeBack = keep("--term-font-size");
    root.style.setProperty("--term-font-size", `${size + 2}px`);
    await rig.settle();
    seen.grown = { was: size, ...rig.footprint() };
    giveSizeBack();
    await rig.settle();

    // A face whose Hangul already spans two columns: a local mono face cut to
    // half the fallback's advance, so two cells ARE that advance.
    const advance = rig.natural([...rig.hangul][0]);
    const source = LOCAL_MONO.map((name) => `local("${name}")`).join(", ");
    const giveFaceBack = keep("--term-font-choice");
    const faces = [];
    const stand = async (family, sizeAdjust) => {
      const face = new FontFace(family, source, { sizeAdjust });
      await face.load();
      document.fonts.add(face);
      faces.push(face);
      root.style.setProperty("--term-font-choice", `"${family}"`);
      await rig.settle();
    };
    try {
      // Cut again from what the last cut measured: an engine steps a face's
      // size, so the first cut lands within a step of half the advance and
      // the next ones walk onto it.
      let cut = 100;
      for (let turn = 0; turn < FIXTURE_CUTS; turn += 1) {
        await stand(`zc-wide-fixture-${turn}`, `${cut}%`);
        if (Math.abs(2 * view.cell.width - advance) < FIXTURE_SLACK_PX) break;
        cut *= advance / 2 / view.cell.width;
      }
      if (Math.abs(2 * view.cell.width - advance) >= FIXTURE_SLACK_PX) throw new Error(`the fixture face stands ${2 * view.cell.width - advance}px off two columns`);
      await rig.feed();
      const run = rig.runOf(0, rig.hangul);
      const style = getComputedStyle(run);
      seen.spanning = {
        ...rig.footprint(),
        advance: rig.natural([...rig.hangul][0]),
        fontScale: parseFloat(style.fontSize) / parseFloat(getComputedStyle(view.pre).fontSize),
        tracking: parseFloat(style.letterSpacing) || 0,
        display: style.display,
      };
    } catch (error) {
      // No local mono face under any of these names: nothing to cut the fixture from.
      seen.spanning = { unbuilt: String(error) };
    } finally {
      giveFaceBack();
      for (const face of faces) document.fonts.delete(face);
      await rig.settle();
    }
    seen.restored = { cell: view.cell.width, size: parseFloat(getComputedStyle(view.pre).fontSize) };
    return seen;
  }, { WIDE_RIG, LOCAL_MONO, FIXTURE_CUTS, FIXTURE_SLACK_PX });
}

export async function testWideGlyphs(page, ok) {
  await installTerminalWaits(page);
  await standRig(page);
  let rows;
  let ink;
  let changes;
  let caret;
  let underlines;
  let reach;
  try {
    rows = await readRows(page);
    const picture = await page.screenshot({ clip: rows.clip });
    ink = await readInk(page, picture.toString("base64"), rows.clip, rows.cell);
    caret = await readCaret(page);
    underlines = await readUnderlines(page);
    reach = await readUnderlineReach(page);
    changes = await readChanges(page);
  } finally {
    await page.evaluate((WIDE_RIG) => window[WIDE_RIG]?.leave(), WIDE_RIG);
  }

  const onItsColumns = (seen) => Math.abs(seen.footprint - 2 * seen.glyphs * seen.cell) < COLUMN_SLACK_PX
    && Math.abs(seen.rowHeight - seen.cellHeight) < COLUMN_SLACK_PX;
  ok("a run of wide glyphs takes exactly two columns each, so the next column lands where the grid says",
    onItsColumns(rows), JSON.stringify(rows));

  ok("every kind of double-width glyph — Hangul, Han, kana, a full-width letter, a picture — ends on its own column",
    rows.bars === [...WIDE_KINDS].length && rows.shifts.every((shift) => Math.abs(shift) < COLUMN_SLACK_PX),
    JSON.stringify({ cell: rows.cell, shifts: rows.shifts.map((shift) => Math.round(shift * 100) / 100) }));

  // What is left between two glyphs is tracking; the rest of the two columns is glyph.
  const filled = 1 - rows.fit.tracking / (2 * rows.cell);
  const widenedOut = rows.fit.stretch >= rows.stretchLimit - 0.01;
  ok("a wide glyph the fallback face draws short is grown and widened until it covers its two columns, or as far as it may be widened",
    !rows.short || (rows.fit.classes.includes("term-wide") && (filled >= COLUMNS_FILLED || widenedOut)),
    JSON.stringify({ short: rows.short, advance: rows.advance, cell: rows.cell, filled, fit: rows.fit, limit: rows.stretchLimit }));

  ok("a wide glyph is widened by no more than a sixth, and one widened that far keeps air between it and the next",
    !rows.short || (rows.fit.stretch <= WIDEN_MOST + 0.01 && (!widenedOut || rows.fit.tracking >= AIR_LEAST_CELLS * rows.cell)),
    JSON.stringify({ short: rows.short, stretch: rows.fit.stretch, most: WIDEN_MOST, tracking: rows.fit.tracking, least: AIR_LEAST_CELLS * rows.cell }));

  // Asked only of a face that is dressed: a run left as its face draws it is
  // not a box of its own, and its blanks stay the narrow runs they are.
  ok("a blank between two wide words rides in their run and still takes one cell",
    !rows.short || (rows.riding.runs[0] === rows.riding.whole && Math.abs(rows.riding.shift) < COLUMN_SLACK_PX),
    JSON.stringify({ short: rows.short, ...rows.riding }));

  ok("a bold wide run is fitted as bold is drawn and ends on its own column",
    Math.abs(rows.bold.shift) < COLUMN_SLACK_PX, JSON.stringify(rows.bold));

  ok("twenty blanks riding between wide words in one run still end the line on its own column",
    !rows.short || (rows.manyBlanks.runs === 1 && Math.abs(rows.manyBlanks.shift) < COLUMN_SLACK_PX),
    JSON.stringify({ short: rows.short, ...rows.manyBlanks }));

  ok("the caret over a wide glyph covers its two columns and draws the glyph as its row does",
    caret.shown && caret.text === caret.first && Math.abs(caret.width - 2 * caret.cell) < COLUMN_SLACK_PX
      && caret.copyClasses.split(" ").every((name) => caret.runClasses.split(" ").includes(name)) && Math.abs(caret.copy.size - caret.run.size) < 0.01
      && Math.abs(caret.copy.tracking - caret.run.tracking) < 0.01 && caret.copy.transform === caret.run.transform
      && ["left", "top", "height"].every((side) => Math.abs(caret.copy[side] - caret.run[side]) < COLUMN_SLACK_PX),
    JSON.stringify(caret));

  // How far a line stands from the one beside it, in device pixels by edge:
  // 0 the same pixel rows, 1 one device pixel off, 2 further or not drawn.
  const offBy = (seen, beside, pixel) => {
    if (seen === null || beside === null) return 2;
    const most = Math.max(Math.abs(seen.top - beside.top), Math.abs(seen.bottom - beside.bottom));
    return most < pixel / 2 ? 0 : most < 1.5 * pixel ? 1 : 2;
  };
  ok("an underline under a wide run stands where the one under the blanks beside it does on most rows and never a device pixel further, at the default size and past 20px",
    underlines.every(({ pixel, rows: lines }) => {
      const off = lines.map(({ narrow, wide, after }) => Math.max(offBy(wide, narrow, pixel), offBy(wide, after, pixel)));
      return off.every((step) => step < 2) && 2 * off.filter((step) => step === 0).length > off.length;
    }),
    JSON.stringify(underlines));

  ok("an underlined wide run's line shows over a painted ground, reaches the last blank riding in a run across the row, and stops where the run ends",
    reach.narrow !== null && reach.past === null && reach.lastBlank - 2 >= ACROSS_READ_LEAST_COLS
      && [[reach.wide, reach.narrow], [reach.last, reach.opening]].every(([seen, beside]) => offBy(seen, beside, reach.pixel) < 2),
    JSON.stringify(reach));

  ok("a picture is sized to its columns and never stretched",
    rows.pictures.length > 0 && rows.pictures.every((seen) => seen.stretch === 1 && seen.transform === "none"),
    JSON.stringify(rows.pictures));

  const lead = Math.max(rows.fit.tracking, 0) / 2;
  ok("a wide run's ground is the strip a narrow run paints, on its own columns",
    Math.abs(rows.wideStrip.top - rows.narrowStrip.top) < COLUMN_SLACK_PX
      && Math.abs(rows.wideStrip.height - rows.narrowStrip.height) < COLUMN_SLACK_PX
      && rows.wideStrip.left - rows.narrowStrip.right > -COLUMN_SLACK_PX
      && rows.wideStrip.left - rows.narrowStrip.right < lead + COLUMN_SLACK_PX,
    JSON.stringify({ narrow: rows.narrowStrip, wide: rows.wideStrip, lead }));

  // The ink row stands half a row down the picture: a device pixel of slack
  // on each edge for a row that starts between two of them.
  const inkRow = [rows.cellHeight / 2, (3 * rows.cellHeight) / 2];
  ok("the ink of a grown wide glyph stays inside its own row",
    ink.inked && ink.top > inkRow[0] - 1 && ink.bottom < inkRow[1] + 1, JSON.stringify({ ink, row: inkRow }));
  // Not asked of a face that needs no fitting, nor of one widened as far as
  // it may be and still short: what is left there is the face's own.
  ok("two syllables a fallback face draws short stand a fraction of a cell apart, not as spaced-out letters",
    !rows.short || widenedOut || (ink.inked && ink.gap <= INK_GAP_CELLS * rows.cell),
    JSON.stringify({ short: rows.short, widenedOut, gap: ink.gap, most: INK_GAP_CELLS * rows.cell }));

  ok("a wide run already painted follows a change of the font size",
    changes.grown.cell > rows.cell && onItsColumns(changes.grown), JSON.stringify(changes.grown));

  const spanning = changes.spanning;
  ok("a face that already draws Hangul across two columns is left exactly as it draws",
    spanning.unbuilt !== undefined || (Math.abs(spanning.advance - 2 * spanning.cell) < 0.01
      && !spanning.classes.includes("term-wide") && spanning.display === "inline"
      && spanning.fontScale === 1 && spanning.tracking === 0 && onItsColumns(spanning)),
    JSON.stringify(spanning));
  ok("the font the wide-glyph checks moved is given back",
    Math.abs(changes.restored.cell - rows.cell) < 0.01, JSON.stringify(changes.restored));
}

if (startedAs(import.meta.url)) await runInBothEngines([testWideGlyphs]);
