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

const WIDE_RIG = "__WIDE_GLYPH_RIG__";

async function standRig(page) {
  return page.evaluate(async ({ WIDE_RIG, WIDE_KINDS, BAR_COLOURS }) => {
    const priorAnswers = { ...window.__ANSWER__ };
    window.__ANSWER__.term_snapshot = () => null;
    window.__ANSWER__.term_resize = () => null;
    const term = await openTermTab({ placement: "tab" });
    const view = termView(term);
    setActiveTab(tabOfTerm(term).id);
    await window.__TERMINAL_READY__(term, view);
    const COLS = 40;
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
    ];
    const feed = async () => {
      const rows = rowsOf().map((cells, index) => ({ index, cells }));
      await window.__TERM_FEED__(term, {
        rows, scrolled_lines: 0, cursor: [rows.length - 1, 0], title: null, alt_screen: false, size: [rows.length, COLS],
        full: true, cursor_visible: false, mouse_tracking: "off", mouse_sgr: false, bell: false, view_offset: 0,
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
      term, view, hangul, riders, feed, settle, natural, rowAt, runOf, footprint,
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
  }, { WIDE_RIG, WIDE_KINDS, BAR_COLOURS });
}

/* The rows as they stand under the window's own font. */
async function readRows(page) {
  return page.evaluate(({ WIDE_RIG, WIDE_KINDS }) => {
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
      narrowStrip: box(rig.runOf(4, "ab")),
      wideStrip: box(rig.runOf(4, "한글")),
      // The ink row with half of the empty row above it and half of the one
      // below, the first fourteen columns: clear of row 4's ground on every edge.
      clip: { x: left, y: inkRow.top - view.cell.height / 2, width: 14 * cell, height: 2 * view.cell.height },
    };
  }, { WIDE_RIG, WIDE_KINDS });
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
  try {
    rows = await readRows(page);
    const picture = await page.screenshot({ clip: rows.clip });
    ink = await readInk(page, picture.toString("base64"), rows.clip, rows.cell);
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

  // Asked only of a face that is dressed: a run left as its face draws it is
  // not a box of its own, and its blanks stay the narrow runs they are.
  ok("a blank between two wide words rides in their run and still takes one cell",
    !rows.short || (rows.riding.runs[0] === rows.riding.whole && Math.abs(rows.riding.shift) < COLUMN_SLACK_PX),
    JSON.stringify({ short: rows.short, ...rows.riding }));

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
