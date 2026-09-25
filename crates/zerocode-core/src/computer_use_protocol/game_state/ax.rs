//! What the accessibility tree can say about a board's cells. A cell has
//! state here only when one element with words lies wholly inside it. An
//! element that covers the board — a canvas, a single "game board" control —
//! lies inside no cell, so a board drawn in pixels has no cell state here,
//! however its frame is divided; its cells are the pixel kernel's to read.

use super::super::marks::ElementFace;
use super::super::render::Rect;

/// The one element that names a cell: its walk index and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxCell {
    pub element: usize,
    pub words: String,
}

/// For each of a board's `rows` × `columns` equal pitch boxes, in reading
/// order, the element that alone lies inside it and has words — or nothing,
/// when none does or two do.
#[must_use]
pub fn ax_cells(
    faces: &[ElementFace],
    board: Rect,
    rows: u32,
    columns: u32,
) -> Vec<Option<AxCell>> {
    let (width, height) = (
        board.width / f64::from(columns.max(1)),
        board.height / f64::from(rows.max(1)),
    );
    let mut cells = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let cell = Rect::new(
                board.x + f64::from(column) * width,
                board.y + f64::from(row) * height,
                width,
                height,
            );
            let mut inside = faces.iter().filter(|face| {
                let seen = face.seen();
                !face.words().trim().is_empty()
                    && seen.width > 0.0
                    && seen.height > 0.0
                    && seen.x >= cell.x
                    && seen.y >= cell.y
                    && seen.x + seen.width <= cell.x + cell.width
                    && seen.y + seen.height <= cell.y + cell.height
            });
            cells.push(match (inside.next(), inside.next()) {
                (Some(face), None) => Some(AxCell {
                    element: face.index,
                    words: face.words().to_string(),
                }),
                _ => None,
            });
        }
    }
    cells
}
