//! The screen a consumer rebuilds from a terminal's frames, and the grid's own
//! screen to hold it against — shared by the test binaries that replay frames.
//!
//! The model is `ui/shell-term.js`'s: a full frame replaces every row;
//! `scrolled_lines` splices that many rows off the top and pads the bottom;
//! `view_shift` moves the rows as a block, up toward live and down into
//! history; then `rows` land by index.

use zerocode_pty::grid::{GridDelta, Terminal};

pub fn text(cells: &[zerocode_pty::grid::Cell]) -> String {
    cells
        .iter()
        .map(|cell| cell.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub struct Consumer {
    pub rows: Vec<String>,
}

impl Consumer {
    pub fn from_full(delta: &GridDelta) -> Self {
        assert!(delta.full, "the first frame a consumer gets is a snapshot");
        let mut rows = vec![String::new(); delta.size.0];
        for row in &delta.rows {
            rows[row.index] = text(&row.cells);
        }
        Self { rows }
    }

    pub fn apply(&mut self, delta: &GridDelta) {
        if delta.full {
            for row in &mut self.rows {
                row.clear();
            }
        } else if delta.scrolled_lines > 0 {
            let n = delta.scrolled_lines.min(self.rows.len());
            self.rows.drain(..n);
            while self.rows.len() < delta.size.0 {
                self.rows.push(String::new());
            }
        } else if delta.view_shift > 0 {
            let n = delta.view_shift.unsigned_abs().min(self.rows.len());
            self.rows.drain(..n);
            while self.rows.len() < delta.size.0 {
                self.rows.push(String::new());
            }
        } else if delta.view_shift < 0 {
            let n = delta.view_shift.unsigned_abs().min(self.rows.len());
            self.rows.truncate(self.rows.len() - n);
            while self.rows.len() < delta.size.0 {
                self.rows.insert(0, String::new());
            }
        }
        for row in &delta.rows {
            if row.index < self.rows.len() {
                self.rows[row.index] = text(&row.cells);
            }
        }
    }
}

pub fn truth(terminal: &Terminal) -> Vec<String> {
    let snapshot = terminal.grid().snapshot();
    let mut rows = vec![String::new(); snapshot.size.0];
    for row in &snapshot.rows {
        rows[row.index] = text(&row.cells);
    }
    rows
}

pub fn assert_same(label: &str, got: &[String], want: &[String]) {
    let diffs: Vec<String> = got
        .iter()
        .zip(want)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| format!("row {i}: consumer {a:?} / grid {b:?}"))
        .collect();
    assert!(
        diffs.is_empty(),
        "{label}: {} rows differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
