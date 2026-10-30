//! Positive evidence that a supported TUI's live input field is empty.

use zerocode_pty::TerminalGrid;

/// Read only the cursor's field, never a prompt glyph in transcript text.
/// Unknown layouts, pickers, multiline fields and attachments stay protected.
pub(crate) fn observed(agent: &str, grid: &TerminalGrid) -> bool {
    if !grid.bracketed_paste() || !grid.cursor_visible() {
        return false;
    }
    let (row, col) = grid.cursor();
    if row == 0 || row + 1 >= grid.rows() {
        return false;
    }
    let above = grid.line(row - 1);
    let below = grid.line(row + 1);
    let line = grid.line(row);
    match agent {
        "zo" => {
            if !border(&above, '╭', '╮') || !border(&below, '╰', '╯') || col != 3 {
                return false;
            }
            let Some(body) = line
                .trim()
                .strip_prefix("│› ")
                .and_then(|s| s.strip_suffix('│'))
            else {
                return false;
            };
            // The placeholder is dim. The same words typed by the person
            // are a draft, even with the cursor moved back to its beginning.
            // A blank row alone may be the scrolled end of a long draft.
            body.trim_end() == "Ask zo to do anything"
                && (3..3 + "Ask zo to do anything".len())
                    .all(|column| grid.cell(row, column).is_some_and(|cell| cell.style.dim))
        }
        "claude" => {
            horizontal(&above)
                && horizontal(&below)
                && col == 2
                && line.starts_with("❯")
                && only_faint_after(grid, row, col)
        }
        _ => false,
    }
}

/// Whether the field holds exactly the hand-over's probe — one space typed on
/// an empty line, the cursor one column on (t-84850 r2): the screen's word
/// that the CLI read it. A suggestion the field drew faint goes the moment a
/// key lands, so the line is then the prompt glyph alone.
pub(crate) fn holds_probe(agent: &str, grid: &TerminalGrid) -> bool {
    if agent != "claude" || !grid.bracketed_paste() || !grid.cursor_visible() {
        return false;
    }
    let (row, col) = grid.cursor();
    if row == 0 || row + 1 >= grid.rows() {
        return false;
    }
    horizontal(&grid.line(row - 1))
        && horizontal(&grid.line(row + 1))
        && col == 3
        && grid.line(row).trim_end() == "❯"
}

/// Whether every cell from `from` to the row's end is blank or drawn faint —
/// a suggestion Claude Code stands in an empty field (`promptSuggestionEnabled`,
/// 2.1.312) — read from the cell's own attribute, never from its words: what a
/// person types is drawn at normal weight and stays words.
fn only_faint_after(grid: &TerminalGrid, row: usize, from: usize) -> bool {
    grid.row_cells(row)
        .iter()
        .skip(from)
        .all(|cell| cell.ch == ' ' || cell.style.dim || cell.is_continuation())
}

fn horizontal(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 3 && line.chars().all(|ch| matches!(ch, '─' | '━'))
}

fn border(line: &str, left: char, right: char) -> bool {
    line.trim()
        .strip_prefix(left)
        .and_then(|s| s.strip_suffix(right))
        .is_some_and(horizontal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_pty::Terminal;

    fn zo(body: &str) -> Terminal {
        let mut term = Terminal::new(8, 64);
        term.feed(
            format!(
                "\x1b[?2004h\x1b[?25h\x1b[3;1H╭{}╮\r\n│› {body}│\r\n╰{}╯\x1b[4;4H",
                "─".repeat(58),
                "─".repeat(58)
            )
            .as_bytes(),
        );
        term
    }

    #[test]
    fn a_zo_placeholder_is_empty_only_in_the_live_bordered_field() {
        let mut term = zo("\x1b[2mAsk zo to do anything\x1b[22m");
        assert!(observed("zo", term.grid()));
        term.feed(b"\x1b[5;4H");
        assert!(
            !observed("zo", term.grid()),
            "a cursor in a picker proved the composer empty"
        );
    }

    #[test]
    fn actual_words_and_multiline_fields_never_count_as_empty() {
        for body in [
            "my unfinished thought",
            "Ask zo to do anything",
            "[Pasted text #1]",
        ] {
            let term = zo(body);
            assert!(
                !observed("zo", term.grid()),
                "a person's words were empty: {body}"
            );
        }
        let mut term = zo("\x1b[2mAsk zo to do anything\x1b[22m");
        term.feed("\x1b[5;1H│  another line│\x1b[4;4H".as_bytes());
        assert!(!observed("zo", term.grid()));
        assert!(
            !observed("zo", zo(" ").grid()),
            "a clipped blank draft row was empty"
        );
    }

    #[test]
    fn an_empty_claude_field_requires_its_cursor_and_both_borders() {
        let mut term = Terminal::new(8, 64);
        term.feed("\x1b[?2004h\x1b[?25h\x1b[3;1H────────\r\n❯ \r\n────────\x1b[4;3H".as_bytes());
        assert!(observed("claude", term.grid()));
        assert!(!observed("unknown", term.grid()));
        term.feed("\x1b[4;3Hdraft\x1b[4;3H".as_bytes());
        assert!(!observed("claude", term.grid()));
        term.feed(b"\x1b[4;1H\x1b[2K\x1b[?25l");
        assert!(!observed("claude", term.grid()));
    }

    fn claude(body: &str, cursor_col: usize) -> Terminal {
        let mut term = Terminal::new(8, 64);
        term.feed(
            format!(
                "\x1b[?2004h\x1b[?25h\x1b[3;1H────────\r\n❯ {body}\r\n────────\x1b[4;{}H",
                cursor_col + 1
            )
            .as_bytes(),
        );
        term
    }

    /// Claude Code 2.1.312 can stand a faint suggestion in an empty field
    /// after a turn (t-84850 r2): the field is empty by the cells' attribute,
    /// and the same words at normal weight are a person's draft.
    #[test]
    fn a_faint_claude_suggestion_is_an_empty_field_and_the_same_words_typed_are_not() {
        assert!(observed(
            "claude",
            claude("\x1b[2mrun the tests again\x1b[22m", 2).grid()
        ));
        assert!(!observed("claude", claude("run the tests again", 2).grid()));
        assert!(
            !observed(
                "claude",
                claude("ru\x1b[2mn the tests again\x1b[22m", 4).grid()
            ),
            "words typed before a faint rest are a draft"
        );
    }

    /// The probe is one space on an empty field, the cursor one column on —
    /// nothing else (t-84850 r2).
    #[test]
    fn the_probe_is_one_space_on_an_empty_claude_field() {
        assert!(holds_probe("claude", claude(" ", 3).grid()));
        assert!(
            !holds_probe("claude", claude("", 2).grid()),
            "an empty field holds no probe"
        );
        assert!(
            !holds_probe("claude", claude("x", 3).grid()),
            "a typed letter is no probe"
        );
        assert!(
            !holds_probe("claude", claude("  ", 4).grid()),
            "two spaces are no probe"
        );
        assert!(!holds_probe("zo", claude(" ", 3).grid()));
    }
}
