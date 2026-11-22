//! The terminal grid as a GPUI element: every cell of the screen painted at
//! its own column, every frame.
//!
//! The shape is sheprd's terminal painter (`sheprd-gpui/src/terminal_painter.rs`
//! and `glyphs.rs`, Apache-2.0, read at fae8e9fed754; nothing copied): a
//! glyph is shaped once per text and face and kept, colour is applied when it
//! is painted, and each glyph is placed by the grid rather than by a shaped
//! line's advance — so a wide glyph lands on its two columns and nothing after
//! it drifts. Grounds are painted as runs of one colour. Zed's terminal element
//! (GPL-3.0, read only) shapes whole runs with a forced cell width instead;
//! the per-cell road was taken because a recording's wide glyphs, joined emoji
//! and combining marks must land on exactly the columns the grid gave them.
//!
//! GPUI keeps no painted scene between frames: a frame redraws every cell,
//! whatever changed. The DOM view rewrites only the rows a frame changed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, Font, FontFeatures, FontId, FontStyle, FontWeight,
    GlobalElementId, GlyphId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, Rgba,
    ShapedLine, SharedString, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill,
    point, px, size,
};
use zerocode_pty::{Cell, CellStyle};

use crate::model::Screen;
use crate::theme::{Theme, hex, hsla};

/// Kept shapes; past this, a glyph is shaped for the one paint that needs it.
const CACHE_LIMIT: usize = 8192;
/// Bold and italic change a glyph's shape; colour does not.
const FACES: usize = 4;

fn face(style: &CellStyle) -> usize {
    usize::from(style.bold) | (usize::from(style.italic) << 1)
}

/// One glyph as painted: what a capture writes beside its picture.
#[derive(Debug, Clone)]
pub struct PaintedGlyph {
    pub row: usize,
    pub col: usize,
    pub cells: usize,
    pub text: String,
    pub fg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub emoji: bool,
    pub glyphs: usize,
    pub advance: f32,
}

/// A run of one ground colour on one row.
#[derive(Debug, Clone)]
pub struct PaintedGround {
    pub row: usize,
    pub col: usize,
    pub cells: usize,
    pub color: Rgba,
}

#[derive(Debug, Clone, Default)]
pub struct Record {
    pub glyphs: Vec<PaintedGlyph>,
    pub grounds: Vec<PaintedGround>,
    pub cursor: Option<(usize, usize, usize)>,
    pub errors: usize,
}

/// What one paint did, for the timing pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaintCount {
    pub glyphs: usize,
    pub shaped: usize,
    pub grounds: usize,
    pub errors: usize,
    pub rebuilt_rows: usize,
    pub reused_rows: usize,
}

#[derive(Debug, Clone, Copy)]
struct CachedGround {
    col: usize,
    cells: usize,
    color: Rgba,
    hsla: Hsla,
}

#[derive(Debug, Clone, Copy)]
struct CachedGlyph {
    x_offset: f32,
    baseline_y: f32,
    font_id: FontId,
    glyph_id: GlyphId,
    font_size: Pixels,
    fg: Hsla,
    is_emoji: bool,
}

#[derive(Debug, Clone, Copy)]
struct CachedDecor {
    col: usize,
    cells: usize,
    y_offset: f32,
    color: Hsla,
    strike: bool,
}

#[derive(Debug, Clone, Default)]
struct CachedRow {
    cols: usize,
    recorded: bool,
    grounds: Vec<CachedGround>,
    glyphs: Vec<CachedGlyph>,
    decors: Vec<CachedDecor>,
    record_glyphs: Vec<PaintedGlyph>,
}

pub struct Painter {
    pub theme: Theme,
    family: SharedString,
    shaped: Vec<ShapedLine>,
    /// `ascii[face * 128 + byte]`: most of any grid, found without hashing.
    ascii: Vec<Option<u32>>,
    other: [HashMap<String, u32>; FACES],
    /// Glyphs wider than their columns (an emoji is 19 px at 14 px, two
    /// columns are 16.9), shaped again at the size that fits them.
    fitted: [HashMap<String, u32>; FACES],
    row_cache: HashMap<u64, CachedRow>,
    frame_rows: Vec<u64>,
    scratch: String,
    /// Off for the capture pass's first picture (the DOM capture hides its own
    /// caret too) and on for its second: the comparison finds the caret where
    /// the two pictures differ.
    pub draw_cursor: bool,
    pub record: Option<Record>,
    pub count: PaintCount,
}

impl Painter {
    pub fn new(theme: Theme, family: impl Into<SharedString>, draw_cursor: bool) -> Self {
        Self {
            theme,
            family: family.into(),
            shaped: Vec::new(),
            ascii: vec![None; FACES * 128],
            other: Default::default(),
            fitted: Default::default(),
            row_cache: HashMap::new(),
            frame_rows: Vec::new(),
            scratch: String::new(),
            draw_cursor,
            record: None,
            count: PaintCount::default(),
        }
    }

    fn font(&self, face: usize) -> Font {
        Font {
            family: self.family.clone(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight(if face & 1 != 0 {
                self.theme.bold_weight
            } else {
                self.theme.weight
            }),
            style: if face & 2 != 0 {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }

    fn shape(&self, face: usize, text: &str, window: &Window) -> ShapedLine {
        self.shape_at(face, text, self.theme.font_size, window)
    }

    fn shape_at(&self, face: usize, text: &str, font_size: f32, window: &Window) -> ShapedLine {
        window.text_system().shape_line(
            SharedString::from(text.to_owned()),
            px(font_size),
            &[TextRun {
                len: text.len(),
                font: self.font(face),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
    }

    /// The kept shape of this cell's glyph, shaping it the first time; `None`
    /// once the cache is full, when the caller shapes for this paint alone.
    fn kept(&mut self, cell: &Cell, window: &Window) -> Option<usize> {
        let face = face(&cell.style);
        if cell.zw.is_empty() && cell.ch.is_ascii() {
            let slot = face * 128 + cell.ch as usize;
            if let Some(id) = self.ascii[slot] {
                return Some(id as usize);
            }
            if self.shaped.len() >= CACHE_LIMIT {
                return None;
            }
            let line = self.shape(face, cell.ch.encode_utf8(&mut [0; 4]), window);
            self.count.shaped += 1;
            self.shaped.push(line);
            let id = self.shaped.len() - 1;
            self.ascii[slot] = Some(id as u32);
            return Some(id);
        }
        self.scratch.clear();
        self.scratch.push(cell.ch);
        self.scratch.extend(cell.zw.scalars());
        if let Some(id) = self.other[face].get(self.scratch.as_str()) {
            return Some(*id as usize);
        }
        if self.shaped.len() >= CACHE_LIMIT {
            return None;
        }
        let text = std::mem::take(&mut self.scratch);
        let line = self.shape(face, &text, window);
        self.count.shaped += 1;
        self.shaped.push(line);
        let id = self.shaped.len() - 1;
        self.other[face].insert(text, id as u32);
        Some(id)
    }

    /// This cell's glyph shaped at `ratio` of the face's size, kept like the
    /// rest; `None` once the cache is full.
    fn fitted(&mut self, cell: &Cell, ratio: f32, window: &Window) -> Option<usize> {
        let face = face(&cell.style);
        self.scratch.clear();
        self.scratch.push(cell.ch);
        self.scratch.extend(cell.zw.scalars());
        if let Some(id) = self.fitted[face].get(self.scratch.as_str()) {
            return Some(*id as usize);
        }
        if self.shaped.len() >= CACHE_LIMIT {
            return None;
        }
        let text = std::mem::take(&mut self.scratch);
        let line = self.shape_at(face, &text, self.theme.font_size * ratio, window);
        self.count.shaped += 1;
        self.shaped.push(line);
        let id = self.shaped.len() - 1;
        self.fitted[face].insert(text, id as u32);
        Some(id)
    }

    /// The ground a cell shows: a wide glyph's second column shows the glyph's.
    fn ground(&self, row: &[Cell], col: usize) -> Option<Rgba> {
        let at = if col > 0 && row[col].is_continuation() {
            col - 1
        } else {
            col
        };
        self.theme.paint(&row[at].style).1
    }

    fn build_row_cache(
        &mut self,
        r: usize,
        row: &[Cell],
        capture_record: bool,
        window: &Window,
    ) -> CachedRow {
        let (cw, ch) = (self.theme.cell_width, self.theme.cell_height);
        let mut cached = CachedRow {
            cols: row.len(),
            recorded: capture_record,
            grounds: Vec::new(),
            glyphs: Vec::new(),
            decors: Vec::new(),
            record_glyphs: Vec::new(),
        };
        let mut col = 0;
        while col < row.len() {
            let ground = self.ground(row, col);
            let start = col;
            col += 1;
            while col < row.len() && self.ground(row, col) == ground {
                col += 1;
            }
            if let Some(color) = ground {
                cached.grounds.push(CachedGround {
                    col: start,
                    cells: col - start,
                    color,
                    hsla: hsla(color),
                });
            }
        }
        let mut open_underline: Option<(usize, usize, f32, Hsla)> = None;
        let mut open_strike: Option<(usize, usize, Hsla)> = None;
        let flush_decor = |decors: &mut Vec<CachedDecor>,
                           open: &mut Option<(usize, usize, f32, Hsla)>,
                           strike: bool| {
            if let Some((start, cells, y_offset, color)) = open.take() {
                decors.push(CachedDecor {
                    col: start,
                    cells,
                    y_offset,
                    color,
                    strike,
                });
            }
        };
        let flush_strike = |decors: &mut Vec<CachedDecor>,
                            open: &mut Option<(usize, usize, Hsla)>| {
            if let Some((start, cells, color)) = open.take() {
                decors.push(CachedDecor {
                    col: start,
                    cells,
                    y_offset: ch / 2.0,
                    color,
                    strike: true,
                });
            }
        };

        for (c, cell) in row.iter().enumerate() {
            if cell.is_continuation() {
                continue;
            }
            let cells = if row.get(c + 1).is_some_and(Cell::is_continuation) {
                2
            } else {
                1
            };
            let (fg, _) = self.theme.paint(&cell.style);
            let fg_hsla = hsla(fg);
            let span = cells as f32 * cw;
            let style = cell.style;
            let blank = cell.ch == ' ' && cell.zw.is_empty();
            let mut painted = None;
            if !blank {
                let once;
                let kept = self.kept(cell, window).map(|id| {
                    let width = self.shaped[id].width.as_f32();
                    if cells == 2 && width > span + 0.5 {
                        self.fitted(cell, span / width, window).unwrap_or(id)
                    } else {
                        id
                    }
                });
                let line = match kept {
                    Some(id) => &self.shaped[id],
                    None => {
                        self.scratch.clear();
                        self.scratch.push(cell.ch);
                        self.scratch.extend(cell.zw.scalars());
                        once = self.shape(face(&style), &self.scratch.clone(), window);
                        self.count.shaped += 1;
                        &once
                    }
                };
                let width = line.width.as_f32();
                let lead = if cells == 2 {
                    ((span - width) / 2.0).max(0.0)
                } else {
                    0.0
                };
                let ascent = line.ascent.as_f32();
                let descent = line.descent.as_f32();
                let baseline_y = (ch - ascent - descent) / 2.0 + ascent;
                let mut glyphs = 0;
                let mut emoji = false;
                for run in &line.runs {
                    for glyph in &run.glyphs {
                        if glyph.is_emoji {
                            emoji = true;
                        }
                        cached.glyphs.push(CachedGlyph {
                            x_offset: c as f32 * cw + lead + glyph.position.x.as_f32(),
                            baseline_y,
                            font_id: run.font_id,
                            glyph_id: glyph.id,
                            font_size: line.font_size,
                            fg: fg_hsla,
                            is_emoji: glyph.is_emoji,
                        });
                        glyphs += 1;
                    }
                }
                painted = Some((glyphs, emoji, width, ascent, descent));
            }
            let (ascent, descent) = painted.map_or((ch * 0.8, ch * 0.2), |(_, _, _, a, d)| (a, d));
            let baseline_y = (ch - ascent - descent) / 2.0 + ascent;
            if style.underline {
                let y = baseline_y + (descent / 2.0).max(1.0);
                match open_underline {
                    Some((start, count, oy, oc))
                        if start + count == c && (oy - y).abs() < 0.01 && oc == fg_hsla =>
                    {
                        open_underline = Some((start, count + cells, oy, oc));
                    }
                    _ => {
                        flush_decor(&mut cached.decors, &mut open_underline, false);
                        open_underline = Some((c, cells, y, fg_hsla));
                    }
                }
            } else {
                flush_decor(&mut cached.decors, &mut open_underline, false);
            }
            if style.strike {
                match open_strike {
                    Some((start, count, oc)) if start + count == c && oc == fg_hsla => {
                        open_strike = Some((start, count + cells, oc));
                    }
                    _ => {
                        flush_strike(&mut cached.decors, &mut open_strike);
                        open_strike = Some((c, cells, fg_hsla));
                    }
                }
            } else {
                flush_strike(&mut cached.decors, &mut open_strike);
            }
            if capture_record && (painted.is_some() || style.underline || style.strike) {
                let mut text = String::new();
                text.push(cell.ch);
                text.extend(cell.zw.scalars());
                let (glyphs, emoji, advance) =
                    painted.map_or((0, false, 0.0), |(g, e, w, _, _)| (g, e, w));
                cached.record_glyphs.push(PaintedGlyph {
                    row: r,
                    col: c,
                    cells,
                    text,
                    fg,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    strike: style.strike,
                    emoji,
                    glyphs,
                    advance,
                });
            }
        }
        flush_decor(&mut cached.decors, &mut open_underline, false);
        flush_strike(&mut cached.decors, &mut open_strike);
        cached
    }

    pub fn paint(&mut self, screen: &Screen, origin: Point<Pixels>, window: &mut Window) {
        self.count = PaintCount::default();
        let capture_record = self.record.is_some();
        if let Some(record) = &mut self.record {
            *record = Record::default();
        }
        let (cw, ch) = (self.theme.cell_width, self.theme.cell_height);
        let whole = Bounds::new(
            origin,
            size(px(screen.cols as f32 * cw), px(screen.rows as f32 * ch)),
        );
        window.paint_quad(fill(whole, hsla(self.theme.bg)));

        self.frame_rows.clear();
        for (r, row) in screen.cells.iter().enumerate() {
            let stamp = screen.row_generations.get(r).copied().unwrap_or(0);
            let hit = stamp != 0
                && self.row_cache.get(&stamp).is_some_and(|cached| {
                    cached.cols == row.len() && (!capture_record || cached.recorded)
                });
            if hit {
                self.count.reused_rows += 1;
            } else {
                let built = self.build_row_cache(r, row, capture_record, window);
                self.row_cache.insert(stamp, built);
                self.count.rebuilt_rows += 1;
            }
            self.frame_rows.push(stamp);
        }
        if self.row_cache.len() > screen.rows.saturating_mul(2).max(64) {
            self.row_cache
                .retain(|stamp, _| self.frame_rows.contains(stamp));
        }

        // Grounds first, from cached per-row runs, so glyphs paint over them.
        for (r, &stamp) in self.frame_rows.iter().enumerate() {
            let Some(cached) = self.row_cache.get(&stamp) else {
                continue;
            };
            let row_y = origin.y + px(r as f32 * ch);
            for g in &cached.grounds {
                let bounds = Bounds::new(
                    point(origin.x + px(g.col as f32 * cw), row_y),
                    size(px(g.cells as f32 * cw), px(ch)),
                );
                window.paint_quad(fill(bounds, g.hsla));
                self.count.grounds += 1;
                if let Some(record) = &mut self.record {
                    record.grounds.push(PaintedGround {
                        row: r,
                        col: g.col,
                        cells: g.cells,
                        color: g.color,
                    });
                }
            }
        }

        // Glyphs and batched decorations from cached per-row primitive lists.
        for (r, &stamp) in self.frame_rows.iter().enumerate() {
            let Some(cached) = self.row_cache.get(&stamp) else {
                continue;
            };
            let row_y = origin.y + px(r as f32 * ch);
            let mut row_errors = 0usize;
            for g in &cached.glyphs {
                let at = point(origin.x + px(g.x_offset), row_y + px(g.baseline_y));
                let result = if g.is_emoji {
                    window.paint_emoji(at, g.font_id, g.glyph_id, g.font_size)
                } else {
                    window.paint_glyph(at, g.font_id, g.glyph_id, g.font_size, g.fg)
                };
                row_errors += usize::from(result.is_err());
            }
            self.count.glyphs += cached.glyphs.len();
            self.count.errors += row_errors;
            for d in &cached.decors {
                let at = point(origin.x + px(d.col as f32 * cw), row_y + px(d.y_offset));
                let span = px(d.cells as f32 * cw);
                if d.strike {
                    window.paint_strikethrough(
                        at,
                        span,
                        &StrikethroughStyle {
                            thickness: px(1.0),
                            color: Some(d.color),
                        },
                    );
                } else {
                    window.paint_underline(
                        at,
                        span,
                        &UnderlineStyle {
                            thickness: px(1.0),
                            color: Some(d.color),
                            wavy: false,
                        },
                    );
                }
            }
            if let Some(record) = &mut self.record {
                record.errors += row_errors;
                for glyph in &cached.record_glyphs {
                    let mut entry = glyph.clone();
                    entry.row = r;
                    record.glyphs.push(entry);
                }
            }
        }
        if self.draw_cursor && screen.cursor_visible {
            let (r, c) = screen.cursor;
            if let Some(row) = screen.cells.get(r)
                && c < row.len()
            {
                let cells = if row.get(c + 1).is_some_and(Cell::is_continuation) {
                    2
                } else {
                    1
                };
                let bounds = Bounds::new(
                    origin + point(px(c as f32 * cw), px(r as f32 * ch)),
                    size(px(cells as f32 * cw), px(ch)),
                );
                window.paint_quad(fill(bounds, hsla(self.theme.cursor)));
                // The glyph under the caret, in the ground's colour.
                let cell = row[c];
                let bg = self.theme.bg;
                self.cell(r, c, cells, &cell, bg, origin, window);
                if let Some(record) = &mut self.record {
                    record.cursor = Some((r, c, cells));
                }
            }
        }
    }

    /// One cell's glyph and decorations at its own columns.
    #[allow(clippy::too_many_arguments)]
    fn cell(
        &mut self,
        r: usize,
        c: usize,
        cells: usize,
        cell: &Cell,
        fg: Rgba,
        origin: Point<Pixels>,
        window: &mut Window,
    ) {
        let (cw, ch) = (self.theme.cell_width, self.theme.cell_height);
        let top_left = origin + point(px(c as f32 * cw), px(r as f32 * ch));
        let span = cells as f32 * cw;
        let style = cell.style;
        let blank = cell.ch == ' ' && cell.zw.is_empty();
        let mut painted = None;
        if !blank {
            let once;
            // A glyph wider than its columns is shaped again at the size that
            // fits them (the window fits a wide run the same way); a narrower
            // one keeps its size. Nothing is scaled up: a taller glyph would
            // cross into the rows above and below.
            let kept = self.kept(cell, window).map(|id| {
                let width = self.shaped[id].width.as_f32();
                if cells == 2 && width > span + 0.5 {
                    self.fitted(cell, span / width, window).unwrap_or(id)
                } else {
                    id
                }
            });
            let line = match kept {
                Some(id) => &self.shaped[id],
                None => {
                    self.scratch.clear();
                    self.scratch.push(cell.ch);
                    self.scratch.extend(cell.zw.scalars());
                    once = self.shape(face(&style), &self.scratch.clone(), window);
                    self.count.shaped += 1;
                    &once
                }
            };
            let width = line.width.as_f32();
            // A wide glyph is centred on its two columns, as the window seats
            // its wide runs; a narrow one starts at its column's left edge, as
            // the window's tracking leaves a narrow fallback glyph.
            let lead = if cells == 2 {
                ((span - width) / 2.0).max(0.0)
            } else {
                0.0
            };
            let ascent = line.ascent.as_f32();
            let descent = line.descent.as_f32();
            let baseline = top_left + point(px(lead), px((ch - ascent - descent) / 2.0 + ascent));
            let mut glyphs = 0;
            let mut emoji = false;
            let mut errors = 0;
            for run in &line.runs {
                for glyph in &run.glyphs {
                    let at = baseline + point(glyph.position.x, px(0.0));
                    let result = if glyph.is_emoji {
                        emoji = true;
                        window.paint_emoji(at, run.font_id, glyph.id, line.font_size)
                    } else {
                        window.paint_glyph(at, run.font_id, glyph.id, line.font_size, hsla(fg))
                    };
                    glyphs += 1;
                    errors += usize::from(result.is_err());
                }
            }
            painted = Some((glyphs, emoji, width, ascent, descent));
            self.count.glyphs += glyphs;
            self.count.errors += errors;
            if let Some(record) = &mut self.record {
                record.errors += errors;
            }
        }
        let (ascent, descent) = painted.map_or((ch * 0.8, ch * 0.2), |(_, _, _, a, d)| (a, d));
        let baseline_y = (ch - ascent - descent) / 2.0 + ascent;
        if style.underline {
            window.paint_underline(
                top_left + point(px(0.0), px(baseline_y + (descent / 2.0).max(1.0))),
                px(span),
                &UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(hsla(fg)),
                    wavy: false,
                },
            );
        }
        if style.strike {
            window.paint_strikethrough(
                top_left + point(px(0.0), px(ch / 2.0)),
                px(span),
                &StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(hsla(fg)),
                },
            );
        }
        if let Some(record) = &mut self.record
            && (painted.is_some() || style.underline || style.strike)
        {
            let mut text = String::new();
            text.push(cell.ch);
            text.extend(cell.zw.scalars());
            let (glyphs, emoji, advance) =
                painted.map_or((0, false, 0.0), |(g, e, w, _, _)| (g, e, w));
            record.glyphs.push(PaintedGlyph {
                row: r,
                col: c,
                cells,
                text,
                fg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                strike: style.strike,
                emoji,
                glyphs,
                advance,
            });
        }
    }
}

impl Record {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "glyphs": self.glyphs.iter().map(|glyph| serde_json::json!({
                "row": glyph.row, "col": glyph.col, "cells": glyph.cells, "text": glyph.text,
                "fg": hex(glyph.fg), "bold": glyph.bold, "italic": glyph.italic,
                "underline": glyph.underline, "strike": glyph.strike, "emoji": glyph.emoji,
                "glyphs": glyph.glyphs, "advance": glyph.advance,
            })).collect::<Vec<_>>(),
            "grounds": self.grounds.iter().map(|ground| serde_json::json!({
                "row": ground.row, "col": ground.col, "cells": ground.cells, "color": hex(ground.color),
            })).collect::<Vec<_>>(),
            "cursor": self.cursor,
            "errors": self.errors,
        })
    }
}

/// The element: the screen's size in cells, painted by the shared painter.
pub struct TerminalGrid {
    pub screen: Rc<RefCell<Screen>>,
    pub painter: Rc<RefCell<Painter>>,
}

impl IntoElement for TerminalGrid {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalGrid {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (cols, rows) = {
            let screen = self.screen.borrow();
            (screen.cols, screen.rows)
        };
        let (cw, ch) = {
            let painter = self.painter.borrow();
            (painter.theme.cell_width, painter.theme.cell_height)
        };
        let mut style = Style::default();
        style.size.width = px(cols as f32 * cw).into();
        style.size.height = px(rows as f32 * ch).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let screen = self.screen.borrow();
        self.painter
            .borrow_mut()
            .paint(&screen, bounds.origin, window);
    }
}
