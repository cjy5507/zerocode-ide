#!/usr/bin/env node
/* Phase 0, the comparison: what the DOM terminal and the GPUI grid drew at each
 * checkpoint of a recording, each held against the grid's own screen at that
 * frame (`<case>.reference.jsonl`, `TerminalGrid::snapshot`), never against
 * each other alone.
 *
 *   node tools/native-terminal-probe/phase0/compare.mjs --recordings DIR --dom DIR --gpui DIR --out DIR
 *
 * Three layers, kept apart because they prove different things:
 *
 * - glyphs: which text stands at which column, with which face (bold, italic,
 *   underline, strike) and which ink colour. The DOM's columns come from the
 *   browser's own layout (each grapheme's box, divided by the measured cell);
 *   GPUI's from what its painter placed. The ink colour is held against the
 *   palette the page resolved (`dom-theme.json`); a DOM colour the window's
 *   legibility lift changed is counted as `lifted`, apart from a mismatch.
 * - cursor: the caret's cell and whether it shows. The DOM's from the caret
 *   element the window positions; GPUI's from its model (the capture pass draws
 *   no caret in either renderer).
 * - pixels: one analysis of both pictures (the DOM's screenshot of the grid,
 *   GPUI's `render_to_image`): per cell, the most common colour (the ground)
 *   and how many pixels differ from it (the ink). A cell with a glyph or a
 *   decoration must carry ink and a blank one must not; the ground must be the
 *   cell's background. Ink that only spills from a neighbour (italic, emoji,
 *   a wide glyph's own pair) is told apart from ink where nothing was drawn.
 *
 * A capture in which either renderer's grid is not whole (larger than the page
 * or the window it was drawn in) is refused with exit 3, not compared: every
 * cell past the picture's edge would read as lost.
 *
 * Writes `compare.json` (every mismatch, by case and frame) and `compare.md`. */
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { join, resolve } from "node:path";

const opt = (name, fallback = null) => {
  const at = process.argv.indexOf(name);
  return at >= 0 ? process.argv[at + 1] : fallback;
};
const RECORDINGS = resolve(opt("--recordings"));
const DOM = resolve(opt("--dom"));
const GPUI = resolve(opt("--gpui"));
const OUT = resolve(opt("--out"));
const CASES = (opt("--cases", "") || "").split(",").filter(Boolean);
/* A pixel is ink when one channel differs from the cell's ground by more than this. */
const INK_DELTA = Number(opt("--ink-delta", "48"));
/* A cell carries ink when at least this many of its inner pixels are ink. */
const INK_PIXELS = Number(opt("--ink-pixels", "4"));
/* Two grounds are the same colour within this many levels per channel. */
const GROUND_DELTA = Number(opt("--ground-delta", "14"));
/* Device pixels left off each edge of a cell before it is read. */
const EDGE = Number(opt("--edge", "1"));

// ---------------------------------------------------------------- PNG

/* A PNG's pixels as RGBA bytes: 8-bit RGB or RGBA, not interlaced — what both
 * Playwright and GPUI's `image` crate write. */
function readPng(path) {
  const bytes = readFileSync(path);
  if (bytes.readUInt32BE(0) !== 0x89504e47) throw new Error(`${path}: not a PNG`);
  let at = 8;
  let width = 0;
  let height = 0;
  let colorType = 0;
  const data = [];
  while (at < bytes.length) {
    const length = bytes.readUInt32BE(at);
    const type = bytes.toString("latin1", at + 4, at + 8);
    const body = bytes.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const depth = body[8];
      colorType = body[9];
      if (depth !== 8 || ![2, 6].includes(colorType) || body[12] !== 0) {
        throw new Error(`${path}: PNG depth ${depth}, colour type ${colorType}, interlace ${body[12]} not read here`);
      }
    } else if (type === "IDAT") data.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const channels = colorType === 6 ? 4 : 3;
  const stride = width * channels;
  const out = Buffer.alloc(width * height * 4);
  let previous = Buffer.alloc(stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? line[x - channels] : 0;
      const up = previous[x];
      const corner = x >= channels ? previous[x - channels] : 0;
      let value = line[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - corner;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - corner);
        value += pa <= pb && pa <= pc ? left : pb <= pc ? up : corner;
      }
      line[x] = value & 0xff;
    }
    for (let x = 0; x < width; x += 1) {
      const from = x * channels;
      const to = (y * width + x) * 4;
      out[to] = line[from];
      out[to + 1] = line[from + 1];
      out[to + 2] = line[from + 2];
      out[to + 3] = channels === 4 ? line[from + 3] : 255;
    }
    previous = line;
  }
  return { width, height, rgba: out };
}

/* Each cell's pixels as a histogram of colours: its most common colour (the
 * ground it shows) and, against any ground asked about, how many pixels are ink. */
function cellsOf(png, { rows, cols, cellW, cellH, scale }) {
  // A cell past the picture's edge would read as a blank black cell: lost ink, not a renderer's fault.
  const needW = Math.round(cols * cellW * scale) - EDGE;
  const needH = Math.round(rows * cellH * scale) - EDGE;
  if (png.width < needW || png.height < needH) {
    throw new Error(`a ${png.width}×${png.height} picture holds only part of a ${rows}×${cols} grid (${needW}×${needH} device pixels)`);
  }
  const out = [];
  for (let r = 0; r < rows; r += 1) {
    const row = [];
    for (let c = 0; c < cols; c += 1) {
      // One device pixel off every edge: a cell's edge rows and columns are shared with its
      // neighbours (a 16.09 px row spans 32.19 device pixels) and blend their colours.
      const x0 = Math.round(c * cellW * scale) + EDGE;
      const x1 = Math.round((c + 1) * cellW * scale) - EDGE;
      const y0 = Math.round(r * cellH * scale) + EDGE;
      const y1 = Math.round((r + 1) * cellH * scale) - EDGE;
      const counts = new Map();
      let total = 0;
      for (let y = y0; y < y1 && y < png.height; y += 1) {
        for (let x = x0; x < x1 && x < png.width; x += 1) {
          const at = (y * png.width + x) * 4;
          const key = (png.rgba[at] << 16) | (png.rgba[at + 1] << 8) | png.rgba[at + 2];
          counts.set(key, (counts.get(key) ?? 0) + 1);
          total += 1;
        }
      }
      let mode = 0;
      let best = -1;
      for (const [key, n] of counts) if (n > best) { best = n; mode = key; }
      const split = (key) => [(key >> 16) & 0xff, (key >> 8) & 0xff, key & 0xff];
      row.push({
        total,
        ground: split(mode),
        inkAgainst: (bg) => {
          let ink = 0;
          for (const [key, n] of counts) {
            const [r2, g2, b2] = split(key);
            if (Math.max(Math.abs(r2 - bg[0]), Math.abs(g2 - bg[1]), Math.abs(b2 - bg[2])) > INK_DELTA) ink += n;
          }
          return ink;
        },
      });
    }
    out.push(row);
  }
  return out;
}

// ---------------------------------------------------------------- the reference

const CONTINUATION = "\u0000";

/* A wire row (`WireRow`: text, runs, zw) as cells; the older cell form too. */
function cellsOfRow(row, cols) {
  let cells;
  if (Array.isArray(row.cells)) {
    cells = row.cells.map((cell) => ({ ch: cell.ch, style: cell.style ?? {}, zw: cell.zw ?? "" }));
  } else {
    cells = [...row.text].map((ch) => ({ ch, style: {}, zw: "" }));
    for (const [at, len, style] of row.runs ?? []) {
      for (let c = at; c < at + len && c < cells.length; c += 1) cells[c].style = style;
    }
    for (const [at, riders] of row.zw ?? []) if (cells[at]) cells[at].zw = riders;
  }
  while (cells.length < cols) cells.push({ ch: " ", style: {}, zw: "" });
  return cells.slice(0, cols);
}

function screenOf(screen) {
  const [rows, cols] = screen.size;
  const grid = Array.from({ length: rows }, () => cellsOfRow({ text: "" }, cols));
  for (const row of screen.rows) if (row.index < rows) grid[row.index] = cellsOfRow(row, cols);
  return { rows, cols, grid, cursor: screen.cursor, cursorVisible: screen.cursor_visible };
}

const parseRgb = (text) => {
  const found = String(text ?? "").match(/rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+))?\s*\)/);
  return found ? [Number(found[1]), Number(found[2]), Number(found[3]), found[4] === undefined ? 1 : Number(found[4])] : null;
};
const parseHex = (text) => {
  const found = String(text ?? "").match(/^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})?$/i);
  return found ? [parseInt(found[1], 16), parseInt(found[2], 16), parseInt(found[3], 16), found[4] ? parseInt(found[4], 16) / 255 : 1] : null;
};
const near = (a, b, delta = 2) => a && b && Math.abs(a[0] - b[0]) <= delta && Math.abs(a[1] - b[1]) <= delta && Math.abs(a[2] - b[2]) <= delta;

/* The ink and ground a style asks for, by the window's own run rules (`runsOf`):
 * reverse first, then bold lifts the low half of the 16 colours. */
function paintOf(style, theme) {
  const palette = theme.palette.map(parseRgb);
  const fgDefault = parseRgb(theme.fg);
  const bgDefault = parseRgb(theme.bgSolid);
  const color = (spec) => {
    if (spec == null || spec === "default") return null;
    if (spec.indexed !== undefined) return { rgb: palette[spec.indexed], index: spec.indexed };
    if (spec.rgb) return { rgb: [...spec.rgb, 1], index: null };
    return null;
  };
  let fgSpec = style.fg;
  let bgSpec = style.bg;
  let fgFall = { rgb: fgDefault, index: null };
  let bgFall = null;
  if (style.reverse) {
    [fgSpec, bgSpec] = [bgSpec, fgSpec];
    fgFall = { rgb: bgDefault, index: null };
    bgFall = { rgb: fgDefault, index: null };
  }
  if (style.bold && fgSpec?.indexed !== undefined && fgSpec.indexed < 8) fgSpec = { indexed: fgSpec.indexed + 8 };
  return { fg: color(fgSpec) ?? fgFall, bg: color(bgSpec) ?? bgFall };
}

/* The glyphs a screen asks to be drawn: every cell that is not a wide glyph's
 * second column and not a plain blank. */
function expectedGlyphs(screen) {
  const out = [];
  screen.grid.forEach((row, r) => {
    row.forEach((cell, c) => {
      if (cell.ch === CONTINUATION) return;
      const text = cell.ch + (cell.zw ?? "");
      if (text.trim() === "") return;
      const cells = row[c + 1]?.ch === CONTINUATION ? 2 : 1;
      out.push({ row: r, col: c, cells, text, style: cell.style ?? {} });
    });
  });
  return out;
}

const key = (row, col) => `${row}:${col}`;

// ---------------------------------------------------------------- one checkpoint, one renderer

/* The longest run of equal texts two rows share, as index pairs. */
function align(want, have) {
  const n = want.length;
  const m = have.length;
  const table = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      table[i][j] = want[i].text === have[j].text ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }
  const pairs = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (want[i].text === have[j].text) {
      pairs.push([i, j]);
      i += 1;
      j += 1;
    } else if (table[i + 1][j] >= table[i][j + 1]) i += 1;
    else j += 1;
  }
  return pairs;
}

/* Glyphs drawn against glyphs asked for, row by row. A drawn glyph's place is
 * its column as a fraction (the DOM's layout box divided by the cell; GPUI's
 * painted column): `misplaced` is a glyph half a cell or more from its column. */
function compareGlyphs(expected, drawn, theme, renderer) {
  const out = { expected: expected.length, drawn: drawn.length, missing: [], extra: [], text: [], face: [], color: [], lifted: [], misplaced: [], maxOffset: 0, links: 0 };
  const byRow = (list, place) => {
    const rows = new Map();
    for (const glyph of list) {
      if (!rows.has(glyph.row)) rows.set(glyph.row, []);
      rows.get(glyph.row).push(glyph);
    }
    for (const row of rows.values()) row.sort((a, b) => place(a) - place(b));
    return rows;
  };
  const want = byRow(expected, (glyph) => glyph.col);
  const have = byRow(drawn, (glyph) => glyph.pos);
  for (const row of new Set([...want.keys(), ...have.keys()])) {
    const w = want.get(row) ?? [];
    const h = have.get(row) ?? [];
    const pairs = align(w, h);
    const usedW = new Set(pairs.map(([i]) => i));
    const usedH = new Set(pairs.map(([, j]) => j));
    for (const [i, j] of pairs) {
      const glyph = w[i];
      const got = h[j];
      const offset = got.pos - glyph.col;
      out.maxOffset = Math.max(out.maxOffset, Math.abs(offset));
      if (Math.abs(offset) >= 0.5) out.misplaced.push({ row, col: glyph.col, text: glyph.text, offset: Math.round(offset * 100) / 100 });
      if (got.link) out.links += 1;
      const style = glyph.style;
      const faces = ["bold", "italic", "underline", "strike"].filter((name) => Boolean(style[name]) !== Boolean(got[name]));
      if (faces.length) out.face.push({ row, col: glyph.col, text: glyph.text, differs: faces });
      const paint = paintOf(style, theme);
      const wantInk = paint.fg.rgb;
      if (got.color && wantInk && !near(got.color, wantInk)) {
        const entry = { row, col: glyph.col, text: glyph.text, want: wantInk.slice(0, 3), got: got.color.slice(0, 3) };
        // The window raises an ink that sits too close to its ground (a minimum contrast, as Orca and
        // xterm do): its class then names another palette index than the style asked for, or the ink is inline.
        const index = (got.cls ?? "").match(/\bterm-fg-(\d+)\b/);
        const lifted = renderer === "dom" && (index ? Number(index[1]) !== paint.fg.index : got.inline === true);
        (lifted ? out.lifted : out.color).push(entry);
      }
    }
    // What is left on both sides at the same column is the wrong text there; the rest is missing or extra.
    const leftH = h.filter((unused, j) => !usedH.has(j));
    for (const [i, glyph] of w.entries()) {
      if (usedW.has(i)) continue;
      const at = leftH.findIndex((got) => Math.abs(got.pos - glyph.col) < 0.5);
      if (at >= 0) {
        out.text.push({ row, col: glyph.col, want: glyph.text, got: leftH[at].text });
        leftH.splice(at, 1);
      } else out.missing.push({ row, col: glyph.col, text: glyph.text });
    }
    for (const got of leftH) out.extra.push({ row, col: Math.round(got.pos * 100) / 100, text: got.text });
  }
  out.maxOffset = Math.round(out.maxOffset * 100) / 100;
  return out;
}

function domGlyphs(state) {
  const cellW = state.state.cell.width;
  const out = [];
  state.state.rows.forEach((row, r) => {
    for (const glyph of row.glyphs) {
      const span = row.spans[glyph.span] ?? {};
      const cls = span.cls ?? "";
      out.push({
        row: r,
        pos: glyph.x / cellW,
        text: glyph.text,
        // The run's own SGR classes. A link the window found in the text wears an
        // underline whose colour is transparent until hovered or ⌘ is held
        // (`.term-link`, shell.css): not a face, counted under `links`.
        bold: /\bbold\b/.test(cls),
        italic: /\bitalic\b/.test(cls),
        underline: /\bunderline\b/.test(cls),
        strike: /\bstrike\b/.test(cls),
        link: /\bterm-link\b/.test(cls),
        color: parseRgb(span.color),
        cls,
        inline: !/\bterm-fg-/.test(cls) && /rgb/.test(span.color ?? ""),
      });
    }
  });
  return out;
}

function gpuiGlyphs(state) {
  return state.painted.glyphs
    .filter((glyph) => glyph.text.trim() !== "")
    .map((glyph) => ({ ...glyph, pos: glyph.col, color: parseHex(glyph.fg), noGlyph: glyph.glyphs === 0 }));
}

function domCursor(state) {
  const caret = state.state.caret;
  const found = String(caret.transform ?? "").match(/translate\(\s*(-?[\d.]+)px\s*,\s*(-?[\d.]+)px\s*\)/);
  if (!found) return { visible: !caret.hidden, row: null, col: null };
  return {
    visible: !caret.hidden,
    col: Math.round(Number(found[1]) / state.state.cell.width),
    row: Math.round(Number(found[2]) / state.state.cell.height),
  };
}

/* One picture held against the screen asked for, cell by cell: a glyph or a
 * decoration must leave ink (pixels away from the cell's own ground), a blank
 * cell must not, and the ground must be the cell's background — unless ink
 * covers most of the cell (a full block). Missing ink where the style asked
 * for an ink as dark as its ground is `faint` (the window lifts such ink; the
 * GPUI grid draws what was asked); ink in a blank cell beside a glyph is that
 * glyph's overhang (`spill`), apart from ink with nothing near it. */
function comparePixels(screen, cells, theme) {
  const out = { missingInk: [], faint: 0, strayInk: [], spill: 0, ground: [], cells: 0, inked: [] };
  const glyphAt = new Map();
  for (const glyph of expectedGlyphs(screen)) glyphAt.set(key(glyph.row, glyph.col), glyph);
  const decorated = (cell) => Boolean(cell.style?.underline || cell.style?.strike);
  const fallbackGround = parseRgb(theme.bgSolid);
  for (let r = 0; r < screen.rows; r += 1) {
    const inkedRow = [];
    for (let c = 0; c < screen.cols; c += 1) {
      const seen = cells[r]?.[c];
      if (!seen) {
        inkedRow.push(null);
        continue;
      }
      out.cells += 1;
      const cell = screen.grid[r][c];
      const ownerCol = cell.ch === CONTINUATION ? c - 1 : c;
      const owner = glyphAt.get(key(r, ownerCol));
      const style = screen.grid[r][ownerCol]?.style ?? {};
      const paint = paintOf(style, theme);
      const bg = paint.bg?.rgb ?? fallbackGround;
      const ink = seen.inkAgainst(bg);
      const inked = ink >= INK_PIXELS;
      inkedRow.push(inked);
      const wanted = Boolean(owner) || decorated(cell);
      if (wanted && !inked) {
        const pair = owner?.cells === 2 ? cells[r][cell.ch === CONTINUATION ? c - 1 : c + 1] : null;
        const pairInked = pair && pair.inkAgainst(bg) >= INK_PIXELS;
        if (!pairInked) {
          if (paint.fg.rgb && near(paint.fg.rgb, bg, INK_DELTA)) out.faint += 1;
          else out.missingInk.push({ row: r, col: c, text: owner?.text ?? "(decoration)", ink });
        }
      } else if (!wanted && inked) {
        const beside = [c - 2, c - 1, c + 1].some((at) => glyphAt.has(key(r, at)));
        if (beside) out.spill += 1;
        else out.strayInk.push({ row: r, col: c, ink });
      }
      const covered = ink / Math.max(1, seen.total) >= 0.45;
      if (!near(seen.ground, bg, GROUND_DELTA) && !covered) out.ground.push({ row: r, col: c, want: bg.slice(0, 3), got: seen.ground });
    }
    out.inked.push(inkedRow);
  }
  return out;
}

/* Where a caret was painted, from two pictures of one frame — without and with
 * the caret: the cells whose pixels differ. The first such cell is the caret's
 * place; how many follow it on that row is its width in cells. */
function caretFromPixels(plain, withCaret, { rows, cols, cellW, cellH, scale }) {
  if (plain.width !== withCaret.width || plain.height !== withCaret.height) return { error: "the two pictures differ in size" };
  const hits = [];
  for (let r = 0; r < rows; r += 1) {
    for (let c = 0; c < cols; c += 1) {
      const x0 = Math.round(c * cellW * scale) + EDGE;
      const x1 = Math.round((c + 1) * cellW * scale) - EDGE;
      const y0 = Math.round(r * cellH * scale) + EDGE;
      const y1 = Math.round((r + 1) * cellH * scale) - EDGE;
      let differ = 0;
      for (let y = y0; y < y1 && y < plain.height; y += 1) {
        for (let x = x0; x < x1 && x < plain.width; x += 1) {
          const at = (y * plain.width + x) * 4;
          if (Math.max(Math.abs(plain.rgba[at] - withCaret.rgba[at]), Math.abs(plain.rgba[at + 1] - withCaret.rgba[at + 1]),
            Math.abs(plain.rgba[at + 2] - withCaret.rgba[at + 2])) > INK_DELTA) differ += 1;
        }
      }
      if (differ >= INK_PIXELS) hits.push({ row: r, col: c, differ });
    }
  }
  if (!hits.length) return { visible: false, row: null, col: null, cells: 0, hits: 0 };
  const first = hits[0];
  let cells = 1;
  while (hits.some((hit) => hit.row === first.row && hit.col === first.col + cells)) cells += 1;
  return { visible: true, row: first.row, col: first.col, cells, hits: hits.length };
}

// ---------------------------------------------------------------- the run

const readJsonl = (path) => readFileSync(path, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
const readJson = (path) => (existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null);
const theme = JSON.parse(readFileSync(join(DOM, "dom-theme.json"), "utf8"));
const meta = (name) => JSON.parse(readFileSync(join(RECORDINGS, `${name}.meta.json`), "utf8"));
const names = CASES.length ? CASES : JSON.parse(readFileSync(join(DOM, "dom-capture-index.json"), "utf8")).index.map((entry) => entry.case);
/* Both renderers must show the same rows and columns, all of them: a capture where
 * either one's grid is larger than its page or window is refused, not compared. */
const cut = [];
for (const name of names) {
  for (const state of readJson(join(DOM, `${name}.dom-capture.json`))?.states ?? []) {
    if (state.grid?.fits === false) cut.push(`${name} frame ${state.frame}: the DOM grid (${Math.round(state.grid.needW)}×${Math.round(state.grid.needH)}) is not whole in its ${Math.round(state.grid.width)}×${Math.round(state.grid.height)} box on a ${state.grid.viewport?.width}×${state.grid.viewport?.height} page`);
  }
  for (const state of readJson(join(GPUI, `${name}.gpui-capture.json`))?.states ?? []) {
    if (state.gridBounds?.fits === false) cut.push(`${name} frame ${state.frame}: the GPUI grid (${Math.round(state.gridBounds.width)}×${Math.round(state.gridBounds.height)}) is larger than its ${state.viewport?.[0]}×${state.viewport?.[1]} window`);
  }
}
if (cut.length) {
  console.error(`compare: refused, ${cut.length} pictures hold only part of the grid:\n  ${cut.slice(0, 8).join("\n  ")}${cut.length > 8 ? `\n  … and ${cut.length - 8} more` : ""}`);
  process.exit(3);
}
const short =(list, n = 6) => ({ count: list.length, first: list.slice(0, n) });
const glyphLayer = (glyph) => ({
  drawn: glyph.drawn, missing: short(glyph.missing), extra: short(glyph.extra), text: short(glyph.text),
  face: short(glyph.face), color: short(glyph.color), lifted: short(glyph.lifted), misplaced: short(glyph.misplaced),
  maxOffset: glyph.maxOffset, links: glyph.links,
});
const pixelLayer = (pixels, scale) => ({
  scale, cells: pixels.cells, missingInk: short(pixels.missingInk), faint: pixels.faint,
  strayInk: short(pixels.strayInk), spill: pixels.spill, ground: short(pixels.ground),
});
const sameCaret = (got, want) => got.visible === want.visible && (!want.visible || (got.row === want.row && got.col === want.col));

const report = {
  thresholds: { INK_DELTA, INK_PIXELS, GROUND_DELTA, EDGE },
  fonts: { dom: readJson(join(DOM, "dom-fonts.json")), gpui: readJson(join(GPUI, "gpui-capture-index.json"))?.font ?? null },
  cases: [],
};
for (const name of names) {
  const references = new Map(readJsonl(join(RECORDINGS, `${name}.reference.jsonl`)).map((line) => [line.frame, line.screen]));
  const dom = JSON.parse(readFileSync(join(DOM, `${name}.dom-capture.json`), "utf8"));
  const gpuiPath = join(GPUI, `${name}.gpui-capture.json`);
  const gpui = existsSync(gpuiPath) ? JSON.parse(readFileSync(gpuiPath, "utf8")) : { states: [] };
  const gpuiByFrame = new Map(gpui.states.map((state) => [state.frame, state]));
  const checkpoints = [];
  for (const domState of dom.states) {
    const k = domState.frame;
    const screen = screenOf(references.get(k));
    const expected = expectedGlyphs(screen);
    const wantCaret = { visible: screen.cursorVisible, row: screen.cursor[0], col: screen.cursor[1] };
    const row = { frame: k, rows: screen.rows, cols: screen.cols, expectedGlyphs: expected.length };
    // DOM. Its screenshot is the grid at the page's device pixel ratio.
    const domGlyph = compareGlyphs(expected, domGlyphs(domState), theme, "dom");
    const domCaret = domCursor(domState);
    const domScale = domState.grid?.dpr ?? 2;
    const domPng = readPng(domState.png);
    const domPixels = comparePixels(screen, cellsOf(domPng, { rows: screen.rows, cols: screen.cols, cellW: domState.state.cell.width, cellH: domState.state.cell.height, scale: domScale }), theme);
    const domGeometry = { rows: screen.rows, cols: screen.cols, cellW: domState.state.cell.width, cellH: domState.state.cell.height, scale: domScale };
    const domPixelCaret = domState.caretPng ? caretFromPixels(domPng, readPng(domState.caretPng), domGeometry) : { visible: false, row: null, col: null, cells: 0, note: "caret element hidden: no second picture" };
    row.dom = {
      appliedIndex: domState.appliedIndex,
      gridFits: domState.grid?.fits ?? null,
      geometry: { dpr: domScale, viewport: domState.grid?.viewport ?? null, clip: domState.clip ?? null, pre: domState.grid ? { left: domState.grid.left, top: domState.grid.top, width: domState.grid.width, height: domState.grid.height } : null, rowsShown: domState.grid?.rowsShown ?? null },
      pixelCursor: { got: domPixelCaret, same: sameCaret(domPixelCaret, wantCaret), layer: "pixels: with and without the caret" },
      glyphs: glyphLayer(domGlyph),
      cursor: { want: wantCaret, got: domCaret, same: sameCaret(domCaret, wantCaret), layer: "caret element" },
      pixels: pixelLayer(domPixels, domScale),
    };
    // GPUI. Its picture is the window's own frame, read back.
    const gpuiState = gpuiByFrame.get(k);
    if (gpuiState) {
      const drawn = gpuiGlyphs(gpuiState);
      const glyph = compareGlyphs(expected, drawn, theme, "gpui");
      const cursor = { visible: gpuiState.cursor.visible, row: gpuiState.cursor.row, col: gpuiState.cursor.col };
      const gpuiGeometry = { rows: screen.rows, cols: screen.cols, cellW: gpuiState.grid.cellW, cellH: gpuiState.grid.cellH, scale: gpuiState.scale };
      const gpuiPng = readPng(gpuiState.png);
      const pixels = comparePixels(screen, cellsOf(gpuiPng, gpuiGeometry), theme);
      const gpuiPixelCaret = gpuiState.cursorPng ? caretFromPixels(gpuiPng, readPng(gpuiState.cursorPng), gpuiGeometry) : { visible: false, row: null, col: null, cells: 0, note: "model cursor hidden: no second picture" };
      row.gpui = {
        viewport: gpuiState.viewport,
        geometry: { scale: gpuiState.scale, viewport: gpuiState.viewport, gridBounds: gpuiState.gridBounds ?? null },
        pixelCursor: { got: gpuiPixelCaret, same: sameCaret(gpuiPixelCaret, wantCaret), layer: "pixels: with and without the caret" },
        glyphs: { ...glyphLayer(glyph), noGlyph: short(drawn.filter((g) => g.noGlyph).map((g) => ({ row: g.row, col: g.col, text: g.text }))) },
        cursor: { want: wantCaret, got: cursor, same: sameCaret(cursor, wantCaret), layer: "model" },
        pixels: pixelLayer(pixels, gpuiState.scale),
        scene: { quads: gpuiState.sceneQuads, underlines: gpuiState.sceneUnderlines, paintErrors: gpuiState.painted.errors },
      };
      // The two pictures against each other: cells one inked and the other not.
      let disagree = 0;
      for (let r = 0; r < screen.rows; r += 1) {
        for (let c = 0; c < screen.cols; c += 1) {
          if (domPixels.inked[r][c] !== null && pixels.inked[r][c] !== null && domPixels.inked[r][c] !== pixels.inked[r][c]) disagree += 1;
        }
      }
      row.inkDisagree = disagree;
    } else {
      row.gpui = null;
    }
    checkpoints.push(row);
  }
  const total = (pick) => checkpoints.reduce((sum, row) => sum + (pick(row) ?? 0), 0);
  const most = (pick) => checkpoints.reduce((max, row) => Math.max(max, pick(row) ?? 0), 0);
  const totals = (side) => ({
    missing: total((row) => row[side].glyphs.missing.count), extra: total((row) => row[side].glyphs.extra.count),
    text: total((row) => row[side].glyphs.text.count), face: total((row) => row[side].glyphs.face.count),
    color: total((row) => row[side].glyphs.color.count), lifted: total((row) => row[side].glyphs.lifted.count),
    misplaced: total((row) => row[side].glyphs.misplaced.count), maxOffset: most((row) => row[side].glyphs.maxOffset),
    links: total((row) => row[side].glyphs.links),
    cursorWrong: checkpoints.filter((row) => !row[side].cursor.same).length,
    pixelCursorWrong: checkpoints.filter((row) => !row[side].pixelCursor.same).length,
    missingInk: total((row) => row[side].pixels.missingInk.count), faint: total((row) => row[side].pixels.faint),
    strayInk: total((row) => row[side].pixels.strayInk.count), spill: total((row) => row[side].pixels.spill),
    ground: total((row) => row[side].pixels.ground.count),
  });
  report.cases.push({
    case: name,
    frames: meta(name).frames,
    checkpoints: checkpoints.length,
    totals: {
      expectedGlyphs: total((row) => row.expectedGlyphs),
      dom: totals("dom"),
      gpui: checkpoints.every((row) => row.gpui) ? { ...totals("gpui"), noGlyph: total((row) => row.gpui.glyphs.noGlyph.count), inkDisagree: total((row) => row.inkDisagree) } : null,
    },
    checkpointsDetail: checkpoints,
  });
}

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, "compare.json"), JSON.stringify(report, null, 1));
const fontLine = `Fonts: the DOM drew ASCII in ${report.fonts.dom?.primary ?? "?"} (CDP getPlatformFontsForNode); GPUI painted with ${JSON.stringify(report.fonts.gpui)}.`;
const lines = [
  "# Phase 0 equivalence",
  "",
  fontLine,
  "",
  `Pixels: each cell read ${EDGE} device pixel in from its edges; a pixel is ink when a channel differs from the cell's background by > ${INK_DELTA}; a cell is inked at ≥ ${INK_PIXELS} such pixels; grounds are equal within ${GROUND_DELTA}.`,
  "Glyphs: aligned row by row by text; `misplaced` is a glyph drawn half a cell or more from its column, `max offset` the largest distance in cells.",
  "",
  "Cursor: `cursor (model/element)` is the DOM's caret element or GPUI's model cursor; `cursor (pixels)` is the cell found by comparing a picture with the caret against one without.",
  "",
  "| case | checkpoints | expected glyphs | renderer | missing | extra | text | face | colour | lifted | misplaced | max offset | cursor (model/element) wrong | cursor (pixels) wrong | missing ink | faint by request | stray ink | spill | ground | DOM≠GPUI inked cells |",
  "|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
];
for (const one of report.cases) {
  const { dom, gpui } = one.totals;
  const cells = (side, extra) => `${side.missing} | ${side.extra} | ${side.text} | ${side.face} | ${side.color} | ${side.lifted} | ${side.misplaced} | ${side.maxOffset} | ${side.cursorWrong} | ${side.pixelCursorWrong} | ${side.missingInk} | ${side.faint} | ${side.strayInk} | ${side.spill} | ${side.ground} | ${extra}`;
  lines.push(`| ${one.case} | ${one.checkpoints} | ${one.totals.expectedGlyphs} | DOM | ${cells(dom, "")} |`);
  lines.push(gpui ? `| | | | GPUI | ${cells(gpui, gpui.inkDisagree)} |` : "| | | | GPUI | (no capture) |");
}
writeFileSync(join(OUT, "compare.md"), `${lines.join("\n")}\n`);
console.log(lines.join("\n"));
