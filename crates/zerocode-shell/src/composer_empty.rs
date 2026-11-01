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

/// The agents whose input field [`observed`] can read as empty — its two arms
/// (t-86909). The window types a key at a pane's running turn on the person's
/// behalf (the queue's 바로 보내기 stop, a steer's words) only where it can
/// see the line is free, so the catalog row says `stop_door` for these and
/// for no other agent: a CLI this module has no arm for is never typed at.
pub(crate) fn reads(agent: &str) -> bool {
    matches!(agent, "zo" | "claude")
}

/// Whether the window may type an agent's stop key at its pane for the
/// conversation queue's 바로 보내기 (t-86909): the catalog names the key
/// (`interrupt_key`) and the pane's input line can be read as empty
/// ([`reads`]) — the answer door types nothing it cannot look at first.
pub(crate) fn stop_door(agent: &str, interrupt_key: Option<&str>) -> bool {
    interrupt_key.is_some() && reads(agent)
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

    /// `reads` and `observed` are one list: the agents it names are the ones
    /// `observed` has an arm for, and a field drawn for one of them is not
    /// read as empty for any other (t-86909).
    #[test]
    fn the_agents_this_module_reads_are_the_ones_observed_has_an_arm_for() {
        let mut claude_field = Terminal::new(8, 64);
        claude_field
            .feed("\x1b[?2004h\x1b[?25h\x1b[3;1H────────\r\n❯ \r\n────────\x1b[4;3H".as_bytes());
        let zo_field = zo("\x1b[2mAsk zo to do anything\x1b[22m");
        assert!(reads("claude") && observed("claude", claude_field.grid()));
        assert!(reads("zo") && observed("zo", zo_field.grid()));
        for other in ["codex", "antigravity", "gemini", "opencode", "unknown"] {
            assert!(!reads(other), "{other}");
            assert!(
                !observed(other, claude_field.grid()),
                "{other} on claude's field"
            );
            assert!(!observed(other, zo_field.grid()), "{other} on zo's field");
        }
    }

    /// The queue's 바로 보내기 stop is typed only where the catalog names the
    /// stop key AND the pane's input line can be read as empty (t-86909): the
    /// answer door types nothing it cannot look at first. Read as the page
    /// reads it, off the catalog's own rows.
    #[test]
    fn the_stop_door_opens_only_where_the_key_is_named_and_the_line_can_be_read() {
        assert!(stop_door("claude", Some("Escape")));
        assert!(stop_door("zo", Some("Escape")));
        assert!(!stop_door("claude", None));
        assert!(!stop_door("codex", None));
        assert!(!stop_door("codex", Some("Escape")));
        assert!(!stop_door("antigravity", Some("Escape")));
        let mut open: Vec<&str> = zerocode_core::agent_presence(None, "macos")
            .into_iter()
            .filter(|row| stop_door(row.id, row.interrupt_key))
            .map(|row| row.id)
            .collect();
        open.sort_unstable();
        assert_eq!(open, ["claude", "zo"]);
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
