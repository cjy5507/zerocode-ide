//! Review notes pinned to lines of a diff.
//!
//! Orca keeps these on the worktree (`diffComments` on the indexed worktree,
//! `worktree-diff-comments-selector-B4UAFatV.js`): a note is written while
//! reading a change, lives until it is delivered to an agent, and is removed
//! the moment it is — a note is a draft of an instruction, not a record.
//! The shape below is the part of Orca's comment this window uses; PR-review
//! fields (`author`, `url`, `source: "markdown"`) belong to surfaces not
//! built yet and are deliberately absent rather than carried empty.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffNote {
    pub id: String,
    /// The checkout this note is about — the workspace root it was written
    /// in. Notes follow the checkout, not the window: two windows on one
    /// workspace see one set of notes.
    pub workspace: String,
    /// Workspace-relative path of the file the note is about.
    pub file_path: String,
    /// The new-side line the note hangs from. `0` means the whole file —
    /// Orca's own convention (`formatDiffComment`: `lineNumber === 0` says
    /// `Scope: file`).
    pub line_number: u32,
    /// When the note spans lines, where the span starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub body: String,
}

impl DiffNote {
    /// The compact line label a card wears: `L12`, `L3-L9`, or nothing for a
    /// whole-file note. Orca's `getDiffCommentLineLabel(comment, compact)`.
    pub fn line_label(&self) -> String {
        match self.start_line {
            Some(start) if start != self.line_number => {
                format!("L{start}-L{}", self.line_number)
            }
            _ if self.line_number == 0 => String::new(),
            _ => format!("L{}", self.line_number),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_stored_without_a_span_still_loads() {
        // The field arrived after the first notes could have been written;
        // a note from before it must not fail to parse.
        let held: DiffNote = serde_json::from_str(
            r#"{"id":"n1","workspace":"/w","file_path":"src/a.rs",
                "line_number":12,"body":"이 이름이 모호합니다"}"#,
        )
        .expect("an old note no longer parses");
        assert_eq!(held.start_line, None);
        assert_eq!(held.line_label(), "L12");
    }

    #[test]
    fn the_line_label_speaks_spans_files_and_lines() {
        let mut note = DiffNote {
            id: "n".into(),
            workspace: "/w".into(),
            file_path: "a".into(),
            line_number: 9,
            start_line: Some(3),
            body: "b".into(),
        };
        assert_eq!(note.line_label(), "L3-L9");
        note.start_line = None;
        assert_eq!(note.line_label(), "L9");
        note.line_number = 0;
        assert_eq!(note.line_label(), "");
    }
}
