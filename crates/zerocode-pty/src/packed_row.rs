//! A row of cells, packed.
//!
//! Three kinds of row are kept far longer than they are read cell by cell.
//! History: a pane's scrollback is painted only while somebody scrolls back,
//! searched only when somebody searches, and held the whole time — and an
//! inline agent (zo and codex draw no alternate screen) fills it with every
//! line it finishes. The primary screen a full-screen program parks while its
//! alternate screen is up: nothing reads it until the program leaves. And the
//! copy of each screen row the window was last sent, which is only ever
//! compared against. Held as screen cells each cost a full [`Cell`] — 44
//! bytes — for every column, written or blank. Packed, a row is what it
//! shows, the way a frame already crosses to the window (`GridRow` on the
//! wire): its scalars up to its last non-default cell, one per cell, the
//! style runs where ink was set, and the zero-width riders where there are
//! any. Every reader still sees cells — [`PackedRow::cells`] gives back
//! exactly the cells that were packed (t-54317).

use crate::grid::{CONTINUATION, Cell, CellStyle, ZeroWidth};

/// Cells sharing one style that is not plain, as the row's `[start, start + len)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyleRun {
    start: u32,
    len: u32,
    style: CellStyle,
}

impl StyleRun {
    const fn end(&self) -> u32 {
        self.start + self.len
    }
}

/// One packed row: the [`Cell`]s it held up to its last non-default one, a
/// column apiece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackedRow {
    /// One scalar per stored cell, the continuation of a wide glyph included.
    chars: Box<[char]>,
    /// The runs of non-plain style, in column order and disjoint.
    runs: Box<[StyleRun]>,
    /// The zero-width riders, by column, in column order. Empty for almost
    /// every row a terminal prints.
    riders: Box<[(u32, ZeroWidth)]>,
}

/// A column as the packed row counts it. A row is one screen line, far below
/// `u32::MAX` columns; a larger index saturates rather than wraps.
fn column(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

impl PackedRow {
    /// Pack `cells` up to the last one that is not [`Cell::default`] — the
    /// default tail is what every reader pads back, so it is not kept.
    pub(crate) fn pack(cells: &[Cell]) -> Self {
        let used = cells
            .iter()
            .rposition(|cell| *cell != Cell::default())
            .map_or(0, |at| at + 1);
        let cells = &cells[..used];
        let mut runs: Vec<StyleRun> = Vec::new();
        let mut riders = Vec::new();
        for (at, cell) in cells.iter().enumerate() {
            let at = column(at);
            if !cell.zw.is_empty() {
                riders.push((at, cell.zw));
            }
            if cell.style.is_plain() {
                continue;
            }
            match runs.last_mut() {
                Some(run) if run.style == cell.style && run.end() == at => run.len += 1,
                _ => runs.push(StyleRun {
                    start: at,
                    len: 1,
                    style: cell.style,
                }),
            }
        }
        Self {
            chars: cells.iter().map(|cell| cell.ch).collect(),
            runs: runs.into_boxed_slice(),
            riders: riders.into_boxed_slice(),
        }
    }

    /// How many cells were stored — the row's width up to its last
    /// non-default cell.
    pub(crate) fn len(&self) -> usize {
        self.chars.len()
    }

    /// One stored cell, or `None` past the stored width.
    pub(crate) fn cell(&self, col: usize) -> Option<Cell> {
        let ch = *self.chars.get(col)?;
        let at = column(col);
        let style = self
            .runs
            .get(self.runs.partition_point(|run| run.end() <= at))
            .filter(|run| run.start <= at)
            .map_or_else(CellStyle::default, |run| run.style);
        let zw = self
            .riders
            .binary_search_by_key(&at, |(rider_at, _)| *rider_at)
            .map_or_else(|_| ZeroWidth::default(), |found| self.riders[found].1);
        Some(Cell { ch, style, zw })
    }

    /// Every stored cell, in column order.
    pub(crate) fn cells(&self) -> Cells<'_> {
        Cells {
            row: self,
            at: 0,
            run: 0,
            rider: 0,
        }
    }

    /// The row as `width` cells: the stored ones, cut at `width`, then the
    /// default tail back out to it.
    pub(crate) fn padded(&self, width: usize) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(width);
        cells.extend(self.cells().take(width));
        cells.resize(width, Cell::default());
        cells
    }

    /// Whether `cells` is the row this was packed from: the stored cells,
    /// then nothing but default ones.
    pub(crate) fn matches(&self, cells: &[Cell]) -> bool {
        cells.len() >= self.len()
            && self
                .cells()
                .zip(cells)
                .all(|(packed, cell)| packed == *cell)
            && cells[self.len()..]
                .iter()
                .all(|cell| *cell == Cell::default())
    }

    /// The row's text: each glyph's scalars with its riders after it, the
    /// second column of a wide glyph left out.
    pub(crate) fn text_into(&self, out: &mut String) {
        let mut riders = self.riders.iter().peekable();
        for (at, ch) in self.chars.iter().enumerate() {
            let at = column(at);
            let rider = riders.next_if(|(rider_at, _)| *rider_at == at);
            if *ch == CONTINUATION {
                continue;
            }
            out.push(*ch);
            if let Some((_, zw)) = rider {
                out.extend(zw.scalars());
            }
        }
    }

    /// Every style that is not plain, a run apiece — what the row's links
    /// are read from without rebuilding a cell.
    pub(crate) fn styles(&self) -> impl Iterator<Item = &CellStyle> {
        self.runs.iter().map(|run| &run.style)
    }

    /// The bytes this row's own allocations hold — what a row costs beyond
    /// the place it is kept in.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> usize {
        self.chars.len() * size_of::<char>()
            + self.runs.len() * size_of::<StyleRun>()
            + self.riders.len() * size_of::<(u32, ZeroWidth)>()
    }
}

/// [`PackedRow::cells`]: the stored cells, rebuilt one at a time.
pub(crate) struct Cells<'a> {
    row: &'a PackedRow,
    at: usize,
    /// The first run that does not end at or before `at`.
    run: usize,
    /// The first rider at or after `at`.
    rider: usize,
}

impl Iterator for Cells<'_> {
    type Item = Cell;

    fn next(&mut self) -> Option<Cell> {
        let ch = *self.row.chars.get(self.at)?;
        let at = column(self.at);
        while self
            .row
            .runs
            .get(self.run)
            .is_some_and(|run| run.end() <= at)
        {
            self.run += 1;
        }
        let style = self
            .row
            .runs
            .get(self.run)
            .filter(|run| run.start <= at)
            .map_or_else(CellStyle::default, |run| run.style);
        let zw = match self.row.riders.get(self.rider) {
            Some((rider_at, zw)) if *rider_at == at => {
                self.rider += 1;
                *zw
            }
            _ => ZeroWidth::default(),
        };
        self.at += 1;
        Some(Cell { ch, style, zw })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.row.chars.len() - self.at;
        (left, Some(left))
    }
}

impl ExactSizeIterator for Cells<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect()
    }

    /// Packing loses nothing a reader can see: every cell comes back with its
    /// character, its style (runs split and merged where the style changes)
    /// and its riders, by index and by the whole-row walk alike — and the
    /// row matches the cells it came from, padded or not, and nothing else.
    #[test]
    fn a_packed_row_gives_back_every_cell_it_was_given() {
        let mut cells = row("ab한\0cde   ");
        let red = CellStyle {
            fg: Color::Indexed(1),
            bold: true,
            ..CellStyle::default()
        };
        let link = CellStyle {
            link: std::num::NonZeroU32::new(3),
            ..CellStyle::default()
        };
        cells[0].style = red;
        cells[1].style = red;
        cells[2].style = link;
        cells[3].style = link;
        cells[6].style = red;
        cells[5].zw = ZeroWidth::from("\u{301}".to_string());
        cells[6].zw = ZeroWidth::from("\u{fe0f}".to_string());
        let packed = PackedRow::pack(&cells);

        assert_eq!(packed.len(), 7, "the default tail was kept");
        assert_eq!(packed.cells().collect::<Vec<_>>(), cells[..7]);
        assert_eq!(
            (0..7).map(|col| packed.cell(col)).collect::<Vec<_>>(),
            cells[..7].iter().copied().map(Some).collect::<Vec<_>>()
        );
        assert_eq!(packed.cell(7), None);
        assert_eq!(packed.padded(cells.len()), cells);
        assert_eq!(packed.padded(3), cells[..3]);
        assert_eq!(packed.runs.len(), 3, "equal neighbours share one run");
        assert!(packed.matches(&cells) && packed.matches(&cells[..7]));
        let mut other = cells.clone();
        other[9].style = red;
        assert!(!packed.matches(&other), "a styled blank past the ink");
        other = cells.clone();
        other[6].zw = ZeroWidth::default();
        assert!(!packed.matches(&other), "a rider gone");
        assert!(!packed.matches(&cells[..6]), "a shorter row");

        let mut text = String::new();
        packed.text_into(&mut text);
        assert_eq!(text, "ab한cd\u{301}e\u{fe0f}");
    }

    /// A plain row pays for its scalars alone: no run, no rider, no tail.
    #[test]
    fn a_plain_row_costs_its_scalars() {
        let packed = PackedRow::pack(&row("plain words      "));
        assert_eq!(packed.heap_bytes(), "plain words".len() * size_of::<char>());
        assert_eq!(PackedRow::pack(&row("    ")).heap_bytes(), 0);
    }
}
