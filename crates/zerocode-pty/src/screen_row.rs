//! A row of the live screen, and how far into it anything has been written.
//!
//! Most of a terminal row is the blank a line never reached: an agent prints
//! forty columns into a row of a hundred and sixty, and a build log not many
//! more. Three things a feed does to every row paid for that blank tail a
//! cell at a time. A row scrolling into history was packed by walking back
//! from its right edge to its last written cell ([`PackedRow::pack`]); the
//! row that opened at the bottom was filled cell by cell across the whole
//! width; and a clear wrote every cell of every row it covered. On an agent's
//! screen the first two were 31 % and 15 % of a feed's samples (2026-11-09),
//! the same blanks found and then written again.
//!
//! A row now keeps its **reach**: every cell at or past it is
//! [`Cell::default`]. It is an upper bound, never an exact edge — a cell
//! inside it may be blank too — so a write only ever has to push it out, and
//! a reader takes the cells before it ([`ScreenRow::written`]) and knows the
//! rest. Clearing to the default blank pulls it back in, which is how a
//! recycled row starts at zero.
//!
//! A write the row was not told about must not leave ink past the reach, and
//! that is what the type answers rather than every caller: the row derefs to
//! its `Vec<Cell>`, and taking it mutably that way gives the reach up — the
//! row knows nothing until it is recycled or [`ScreenRow::settle`]d. Only the
//! named writers here keep it: [`ScreenRow::reach_to`] for a print, the fills
//! for an erase, [`ScreenRow::recycle`] for a row that opens. A path nobody
//! taught about the reach is therefore slower, never wrong. Debug builds check
//! the bound wherever a reader leans on it.
//!
//! [`PackedRow::pack`]: crate::packed_row::PackedRow::pack

use std::ops::{Deref, DerefMut, Range};

use crate::grid::Cell;

/// One row of the live screen: a cell per column, and the column past which
/// every cell is still [`Cell::default`].
#[derive(Clone, Default)]
pub(crate) struct ScreenRow {
    cells: Vec<Cell>,
    /// Every cell at or past this column is [`Cell::default`]. Beyond the
    /// row's width (`usize::MAX` after a mutable deref) it says nothing, and
    /// [`Self::reach`] reads it as the whole row.
    reach: usize,
}

impl ScreenRow {
    /// `cols` default cells: nothing written yet.
    pub(crate) fn blank(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
            reach: 0,
        }
    }

    /// `cells`, which hold nothing but [`Cell::default`] at or past `reach` —
    /// the caller's knowledge, checked in debug builds.
    pub(crate) fn with_reach(cells: Vec<Cell>, reach: usize) -> Self {
        let row = Self { cells, reach };
        row.debug_check();
        row
    }

    /// How far into the row anything but the default blank may stand: the
    /// cells from here on are [`Cell::default`].
    #[inline]
    pub(crate) fn reach(&self) -> usize {
        self.reach.min(self.cells.len())
    }

    /// The cells before [`Self::reach`] — everything the row may show, for a
    /// reader that pads the rest back with [`Cell::default`] (`PackedRow`).
    #[inline]
    pub(crate) fn written(&self) -> &[Cell] {
        self.debug_check();
        &self.cells[..self.reach()]
    }

    /// The row's cells as they stand, a column apiece.
    pub(crate) fn to_cells(&self) -> Vec<Cell> {
        self.cells.clone()
    }

    /// The cells a print is about to write up to `end`, with everything
    /// already written in front of them: `[0, max(reach, end))`, which becomes
    /// the reach. A wide glyph or a cluster the print lands on has all its
    /// columns inside that span — they are written, so they stand before the
    /// old reach — and so does any cell the print blanks.
    #[inline]
    pub(crate) fn reach_to(&mut self, end: usize) -> &mut [Cell] {
        let reach = self.reach().max(end.min(self.cells.len()));
        self.reach = reach;
        &mut self.cells[..reach]
    }

    /// The whole row, for a writer that learns how far it wrote only as it
    /// goes — a stretch of wide and narrow glyphs ends where the next one no
    /// longer fits. `write` answers its own result and the column its last
    /// write ended at, and that column extends the reach. Cells past the old
    /// reach that it did not report must be left default; debug builds check.
    #[inline]
    pub(crate) fn write_run<T>(&mut self, write: impl FnOnce(&mut [Cell]) -> (T, usize)) -> T {
        let (answer, end) = write(&mut self.cells);
        self.reach = self.reach().max(end.min(self.cells.len()));
        self.debug_check();
        answer
    }

    /// Write `blank` over the columns in `span` (clamped to the row). A
    /// default blank needs to go down only where something may have been
    /// written, and when the span runs to the reach it pulls the reach back
    /// to where the span starts; any other blank is ink — a coloured
    /// background is a painted bar — and extends the reach like a print.
    pub(crate) fn fill(&mut self, span: Range<usize>, blank: Cell) {
        let len = self.cells.len();
        let (start, end) = (span.start.min(len), span.end.min(len));
        if start >= end {
            return;
        }
        if blank.is_default_blank() {
            let reach = self.reach();
            if start < reach {
                self.cells[start..end.min(reach)].fill(blank);
            }
            if end >= reach {
                self.reach = reach.min(start);
            }
        } else {
            self.cells[start..end].fill(blank);
            self.reach = self.reach().max(end);
        }
    }

    /// The whole row as `blank` — [`Self::fill`] across every column.
    pub(crate) fn fill_all(&mut self, blank: Cell) {
        self.fill(0..self.cells.len(), blank);
    }

    /// Make this row `cols` cells of `blank`, keeping its allocation: the row
    /// that opens at the bottom of a scroll is the one that just left the
    /// top. A default blank rewrites only the cells before the reach, since
    /// the rest already are one.
    pub(crate) fn recycle(&mut self, cols: usize, blank: Cell) {
        if blank.is_default_blank() {
            let reach = self.reach();
            self.cells[..reach].fill(blank);
            self.cells.resize(cols, blank);
            self.reach = 0;
        } else {
            self.cells.clear();
            self.cells.resize(cols, blank);
            self.reach = cols;
        }
    }

    /// Find the reach again after a write it was not told about — a resize
    /// re-fitting the row, a mutable deref — rather than wait for the row's
    /// next recycle to learn it.
    pub(crate) fn settle(&mut self) {
        self.reach = self
            .cells
            .iter()
            .rposition(|cell| !cell.is_default_blank())
            .map_or(0, |at| at + 1);
    }

    /// Forget the reach, as a write through the deref does — for a test that
    /// holds a grid whose every row walks its whole width beside one that
    /// keeps its reaches.
    #[cfg(test)]
    pub(crate) fn forget_reach(&mut self) {
        self.reach = usize::MAX;
    }

    /// Debug builds: nothing but the default blank stands at or past the
    /// reach. The bound is what every reader of [`Self::written`] trusts, and
    /// a writer that forgot to extend it would cut a line short in history
    /// without a word.
    #[inline]
    fn debug_check(&self) {
        debug_assert!(
            self.cells[self.reach()..]
                .iter()
                .all(Cell::is_default_blank),
            "a screen row holds ink past its reach ({} of {} columns)",
            self.reach(),
            self.cells.len(),
        );
    }
}

/// The cells alone, as the plain `Vec<Cell>` a row used to be: the reach is
/// a bound a grid keeps for its own walks, not something the screen shows,
/// so two grids holding the same cells print — and compare — the same.
impl std::fmt::Debug for ScreenRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cells.fmt(f)
    }
}

/// A row built whole in a test: its reach unknown, as after a mutable deref.
#[cfg(test)]
impl From<Vec<Cell>> for ScreenRow {
    fn from(cells: Vec<Cell>) -> Self {
        Self {
            cells,
            reach: usize::MAX,
        }
    }
}

impl Deref for ScreenRow {
    type Target = Vec<Cell>;

    #[inline]
    fn deref(&self) -> &Vec<Cell> {
        &self.cells
    }
}

impl DerefMut for ScreenRow {
    /// Any write through here may land anywhere, so the row stops claiming a
    /// reach until it is recycled or settled.
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<Cell> {
        self.reach = usize::MAX;
        &mut self.cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CellStyle;

    fn ink(ch: char) -> Cell {
        Cell {
            ch,
            ..Cell::default()
        }
    }

    fn painted() -> Cell {
        Cell {
            style: CellStyle {
                reverse: true,
                ..CellStyle::default()
            },
            ..Cell::default()
        }
    }

    #[test]
    fn a_blank_row_has_written_nothing() {
        let row = ScreenRow::blank(8);
        assert_eq!(row.reach(), 0);
        assert!(row.written().is_empty());
        assert_eq!(row.len(), 8);
    }

    #[test]
    fn a_print_extends_the_reach_to_its_end_and_never_pulls_it_back() {
        let mut row = ScreenRow::blank(10);
        row.reach_to(4)[3] = ink('a');
        assert_eq!(row.reach(), 4);
        row.reach_to(2)[0] = ink('b');
        assert_eq!(row.reach(), 4, "a print short of the reach keeps it");
        assert_eq!(row.written().len(), 4);
        assert_eq!(row.reach_to(40).len(), 10, "the reach stops at the edge");
    }

    #[test]
    fn a_default_fill_running_to_the_reach_pulls_it_back_to_the_fill_start() {
        let mut row = ScreenRow::blank(10);
        row.reach_to(6).fill(ink('x'));
        row.fill(3..10, Cell::default());
        assert_eq!(row.reach(), 3);
        assert!(row[3..].iter().all(Cell::is_default_blank));
        row.fill(0..1, Cell::default());
        assert_eq!(row.reach(), 3, "a fill short of the reach leaves it");
        assert_eq!(row[0], Cell::default());
        assert_eq!(row[1], ink('x'));
    }

    #[test]
    fn a_painted_fill_is_ink_and_extends_the_reach() {
        let mut row = ScreenRow::blank(10);
        row.fill(2..7, painted());
        assert_eq!(row.reach(), 7);
        row.fill_all(painted());
        assert_eq!(row.reach(), 10);
        row.fill_all(Cell::default());
        assert_eq!(row.reach(), 0);
        assert!(row.iter().all(Cell::is_default_blank));
    }

    #[test]
    fn a_recycled_row_is_blank_at_its_new_width_whatever_it_held() {
        let mut row = ScreenRow::blank(6);
        row.reach_to(5).fill(ink('z'));
        row.recycle(9, Cell::default());
        assert_eq!(row.len(), 9);
        assert_eq!(row.reach(), 0);
        assert!(row.iter().all(Cell::is_default_blank));

        row.reach_to(9).fill(ink('z'));
        row.recycle(4, painted());
        assert_eq!(row.len(), 4);
        assert_eq!(row.reach(), 4, "a painted blank is ink");
        assert!(row.iter().all(|cell| *cell == painted()));
    }

    #[test]
    fn a_mutable_deref_gives_the_reach_up_until_the_row_settles() {
        let mut row = ScreenRow::blank(8);
        row[6] = ink('q');
        assert_eq!(row.reach(), 8, "a write the row was not told about");
        row.settle();
        assert_eq!(row.reach(), 7);
        row.recycle(8, Cell::default());
        assert_eq!(row.reach(), 0);
    }

    #[test]
    fn a_run_reports_how_far_it_wrote() {
        let mut row = ScreenRow::blank(8);
        let wrote = row.write_run(|cells| {
            cells[2] = ink('k');
            cells[3] = ink('k');
            ("two", 4)
        });
        assert_eq!(wrote, "two");
        assert_eq!(row.reach(), 4);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "ink past its reach")]
    fn ink_past_a_claimed_reach_is_caught_in_debug_builds() {
        let mut cells = vec![Cell::default(); 4];
        cells[3] = ink('!');
        drop(ScreenRow::with_reach(cells, 2));
    }
}
