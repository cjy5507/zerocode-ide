//! The screen the GPUI grid draws: the rows a consumer of `GridDelta` holds,
//! moved and overwritten by the shift contract on `GridDelta` (zerocode-pty
//! grid.rs) — the same contract the window's view follows.

use zerocode_pty::{Cell, GridDelta};

#[derive(Debug, Clone)]
pub struct Screen {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Vec<Cell>>,
    pub row_generations: Vec<u64>,
    next_generation: u64,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub frames: usize,
}

/// A frame field this replay does not draw, named so a run can say so rather
/// than draw something else and call it equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported(pub String);

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Self {
        let row_generations: Vec<u64> = (1..=rows as u64).collect();
        let next_generation = (rows as u64).saturating_add(1).max(2);
        Self {
            rows,
            cols,
            cells: vec![vec![Cell::default(); cols]; rows],
            row_generations,
            next_generation,
            cursor: (0, 0),
            cursor_visible: true,
            frames: 0,
        }
    }

    fn bump_generation(&mut self) -> u64 {
        let stamp = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        stamp
    }

    /// One frame, in the contract's order: a full frame replaces everything;
    /// otherwise the rows move by `scrolled_lines` (up) or `view_shift` (up when
    /// positive, down when negative) with blanks over what they exposed; then
    /// the named rows are overwritten.
    pub fn apply(&mut self, delta: &GridDelta) -> Result<(), Unsupported> {
        if !delta.folds.is_empty() || delta.rows.iter().any(|row| row.fold.is_some()) {
            return Err(Unsupported(
                "folds (OSC 7788 regions): the window hides and shows rows for them; this replay does not".into(),
            ));
        }
        let (rows, cols) = delta.size;
        if delta.full {
            self.rows = rows;
            self.cols = cols;
            self.cells = vec![vec![Cell::default(); cols]; rows];
            self.row_generations = (0..rows).map(|_| self.bump_generation()).collect();
        } else {
            if (rows, cols) != (self.rows, self.cols) {
                return Err(Unsupported(format!(
                    "the size changed to {rows}×{cols} on a frame that is not full"
                )));
            }
            self.shift_up(delta.scrolled_lines);
            match delta.view_shift.signum() {
                1 => self.shift_up(delta.view_shift.unsigned_abs()),
                -1 => self.shift_down(delta.view_shift.unsigned_abs()),
                _ => {}
            }
        }
        for row in &delta.rows {
            let Some(held) = self.cells.get_mut(row.index) else {
                continue;
            };
            let take = row.cells.len().min(self.cols);
            let same = held.len() == self.cols
                && held[..take] == row.cells[..take]
                && held[take..].iter().all(|c| *c == Cell::default());
            if !same {
                held.clear();
                held.extend(row.cells.iter().copied().take(self.cols));
                held.resize(self.cols, Cell::default());
                let stamp = self.bump_generation();
                self.row_generations[row.index] = stamp;
            }
        }
        self.cursor = delta.cursor;
        self.cursor_visible = delta.cursor_visible;
        self.frames += 1;
        Ok(())
    }

    fn shift_up(&mut self, lines: usize) {
        let lines = lines.min(self.rows);
        if lines == 0 {
            return;
        }
        self.cells.drain(..lines);
        self.cells
            .extend((0..lines).map(|_| vec![Cell::default(); self.cols]));
        self.row_generations.drain(..lines);
        for _ in 0..lines {
            let stamp = self.bump_generation();
            self.row_generations.push(stamp);
        }
    }

    fn shift_down(&mut self, lines: usize) {
        let lines = lines.min(self.rows);
        if lines == 0 {
            return;
        }
        self.cells.truncate(self.rows - lines);
        self.row_generations.truncate(self.rows - lines);
        for _ in 0..lines {
            self.cells.insert(0, vec![Cell::default(); self.cols]);
            let stamp = self.bump_generation();
            self.row_generations.insert(0, stamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_pty::Terminal;

    /// One thing done to a terminal before a pull.
    type Step = Box<dyn Fn(&mut Terminal)>;

    /// Every frame a terminal hands one reader, applied here, leaves the screen
    /// the grid itself holds — through scrolling, a scroll region and a view
    /// moved into history.
    #[test]
    fn frames_applied_in_order_leave_the_grids_own_screen() {
        let mut terminal = Terminal::new(6, 20);
        let mut screen = Screen::new(6, 20);
        let mut steps: Vec<Step> = Vec::new();
        for line in 0..14 {
            steps.push(Box::new(move |terminal: &mut Terminal| {
                terminal.feed(format!("line {line}\r\n").as_bytes());
            }));
        }
        steps.push(Box::new(|terminal: &mut Terminal| {
            terminal.feed(b"\x1b[2;5r\x1b[5;1H\nin region\n")
        }));
        steps.push(Box::new(|terminal: &mut Terminal| {
            terminal.grid_mut().scroll_view(3)
        }));
        steps.push(Box::new(|terminal: &mut Terminal| {
            terminal.grid_mut().scroll_view(-2)
        }));
        steps.push(Box::new(|terminal: &mut Terminal| {
            terminal.grid_mut().view_to_bottom()
        }));
        for step in steps {
            step(&mut terminal);
            let frames = {
                let (readers, grid) = terminal.readers_mut();
                readers.pull("test", grid, std::time::Instant::now())
            };
            for frame in &frames {
                screen.apply(frame).unwrap();
            }
            let truth = terminal.grid().snapshot();
            for row in &truth.rows {
                assert_eq!(screen.cells[row.index], row.cells, "row {}", row.index);
            }
            assert_eq!(screen.cursor, truth.cursor);
        }
    }

    #[test]
    fn a_size_change_without_a_full_frame_is_refused() {
        let mut screen = Screen::new(4, 10);
        let mut terminal = Terminal::new(5, 10);
        terminal.feed(b"x");
        let mut delta = terminal.grid().snapshot();
        delta.full = false;
        assert!(screen.apply(&delta).is_err());
    }

    #[test]
    fn row_generations_bump_only_for_changed_rows_and_shift_with_scrolls() {
        let mut terminal = Terminal::new(6, 20);
        let mut screen = Screen::new(6, 20);
        terminal.feed(b"row0\r\nrow1\r\nrow2\r\nrow3\r\nrow4\r\nrow5");
        let frames = {
            let (readers, grid) = terminal.readers_mut();
            readers.pull("test", grid, std::time::Instant::now())
        };
        for frame in &frames {
            screen.apply(frame).unwrap();
        }
        let before = screen.row_generations.clone();

        // Single-row edit leaves the other five rows untouched.
        terminal.feed(b"\x1b[2;1Hedit");
        let frames = {
            let (readers, grid) = terminal.readers_mut();
            readers.pull("test", grid, std::time::Instant::now())
        };
        for frame in &frames {
            screen.apply(frame).unwrap();
        }
        assert_ne!(screen.row_generations[1], before[1]);
        for unchanged in [0, 2, 3, 4, 5] {
            assert_eq!(screen.row_generations[unchanged], before[unchanged]);
        }

        // Scroll up by 1 shifts rows 1..5 into 0..4 with their exact generations intact.
        let pre_scroll = screen.row_generations.clone();
        terminal.feed(b"\x1b[6;1H\r\nrow6");
        let frames = {
            let (readers, grid) = terminal.readers_mut();
            readers.pull("test", grid, std::time::Instant::now())
        };
        for frame in &frames {
            screen.apply(frame).unwrap();
        }
        for row in 0..5 {
            assert_eq!(screen.row_generations[row], pre_scroll[row + 1]);
        }
        assert_ne!(screen.row_generations[5], pre_scroll[5]);
    }
}
