//! Positive evidence that a supported TUI's live input field is empty.

use zerocode_pty::{Cell, TerminalGrid};

/// Read only the cursor's field, never a prompt glyph in transcript text.
/// Unknown layouts, pickers, multiline fields and attachments stay protected.
///
/// One reading serves an idle screen and a running turn's (t-86909 r2): the
/// field is drawn the same whatever the turn does, and what a running turn
/// adds — the spinner and the effort line above the field, a name set into
/// its upper rule, a hint or the status line under it — lies outside the three
/// rows read.
///
/// **Claude Code 2.1.316 hides the terminal's cursor** (t-86909 r3, measured on
/// the real binary, see the recordings under `tests/fixtures`): it sets `?25l`
/// once at its start and never shows the cursor again, a turn running or not,
/// and draws its own caret as a reversed cell with the hidden cursor standing
/// on it. The reading used to ask for a SHOWN cursor and so took no field of
/// this CLI for empty: every pane of it refused the queue's doors. It reads the
/// field's cells instead ([`claude_field`]) and no longer asks whether the
/// cursor is shown. Bracketed paste (on throughout), the cursor on the field's
/// first cell and the rules above and below are asked as before. zo draws its
/// cursor into its bordered composer, and that is still asked.
pub(crate) fn observed(agent: &str, grid: &TerminalGrid) -> bool {
    if !grid.bracketed_paste() {
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
            if !grid.cursor_visible() {
                return false;
            }
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
            upper_rule_kind(&above).is_some()
                && horizontal(&below)
                && col == 2
                && line.starts_with("❯")
                && claude_field(grid, row, col) != Field::Text
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
/// key lands, so the line is then the prompt glyph alone. Read under a hidden
/// cursor as [`observed`] is (t-86909 r3): the caret is then the reversed cell
/// the cursor stands on, and no letter is on the line.
pub(crate) fn holds_probe(agent: &str, grid: &TerminalGrid) -> bool {
    if agent != "claude" || !grid.bracketed_paste() {
        return false;
    }
    let (row, col) = grid.cursor();
    if row == 0 || row + 1 >= grid.rows() {
        return false;
    }
    upper_rule_kind(&grid.line(row - 1)).is_some()
        && horizontal(&grid.line(row + 1))
        && col == 3
        && grid.line(row).trim_end() == "❯"
        && claude_field(grid, row, col) == Field::Empty
}

/// What stands on a Claude Code field from the caret's cell to the row's end
/// (t-86909 r3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// Blank cells and faint ones, and the caret on a blank cell.
    Empty,
    /// The caret on the first letter of a faint run: the placeholder
    /// `Press up to edit queued messages` an empty field shows while the CLI
    /// holds queued messages, or a suggestion. Nothing a person wrote.
    Ghost,
    /// Anything else — plain letters, a paste chip, a letter under the caret
    /// and nothing faint beside it: a draft.
    Text,
}

/// Read the row `row` of a Claude Code field from column `from` (where the
/// terminal's cursor stands, hidden or not) to its end, by the cells' own
/// attributes and never by their words: what a person types is drawn at normal
/// weight and stays text.
///
/// The shapes measured on Claude Code 2.1.316 (the recordings): an empty field
/// is `❯ ` and a reversed blank, the caret. With messages queued in the CLI the
/// placeholder's first letter is the caret (reversed, not faint) and the rest is
/// faint, one run in the alternate-screen layout and a run per word in the inline
/// one. A pane told it lost the keyboard draws no caret at all, so its field is
/// blank. And the caret moved to the start of a draft stands on the draft's first
/// letter, reversed like the placeholder's, followed by plain letters or by
/// nothing. That last shape is why a reversed first letter is a placeholder only
/// when a faint letter follows within two cells and every cell after it is blank
/// or faint: a one-letter draft with the caret on it has blanks behind it, a longer
/// one has plain letters, a paste chip opens with `[`, and a faint word far to the
/// right is not beside the caret. A reversed cell anywhere else is a caret that an
/// empty field does not have.
fn claude_field(grid: &TerminalGrid, row: usize, from: usize) -> Field {
    let quiet = |cell: &Cell| {
        (cell.ch == ' ' || cell.style.dim || cell.is_continuation()) && !cell.style.reverse
    };
    let Some((first, rest)) = grid
        .row_cells(row)
        .get(from..)
        .and_then(<[Cell]>::split_first)
    else {
        return Field::Empty;
    };
    if !first.style.reverse {
        return if quiet(first) && rest.iter().all(quiet) {
            Field::Empty
        } else {
            Field::Text
        };
    }
    if !rest.iter().all(quiet) {
        return Field::Text;
    }
    if first.ch == ' ' {
        return Field::Empty;
    }
    let faint_beside = rest
        .iter()
        .take(2)
        .any(|cell| cell.style.dim && cell.ch != ' ' && !cell.is_continuation());
    if first.ch != '[' && faint_beside {
        Field::Ghost
    } else {
        Field::Text
    }
}

fn horizontal(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 3 && line.chars().all(is_rule_mark)
}

fn is_rule_mark(ch: char) -> bool {
    matches!(ch, '─' | '━')
}

/// How the rule over a Claude Code field is drawn: bare, or with the session's
/// name set into it. Never the name itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    Bare,
    Named,
}

/// The rule over a Claude Code field: a bare run of rule marks, or one with the
/// session's name set into it near its right end — `────── name ─`, which
/// Claude Code 2.1.316 draws over the field of a named session, whether a turn
/// runs or not (t-86909 r2: all three captures of a running pane carried it).
/// The name stands between two spaces with rule marks after it, and at least
/// three marks open the line. A name with no mark after it, a name on its own
/// and a line of words are no border. Only this upper rule takes a name; the
/// lower one is drawn bare ([`horizontal`]).
fn upper_rule_kind(line: &str) -> Option<Rule> {
    let line = line.trim();
    if line.chars().take_while(|ch| is_rule_mark(*ch)).count() < 3 {
        return None;
    }
    let rest = line.trim_start_matches(is_rule_mark);
    if rest.is_empty() {
        return Some(Rule::Bare);
    }
    let inner = rest.strip_prefix(' ')?;
    let named = inner.trim_end_matches(is_rule_mark);
    (named.len() < inner.len()
        && named
            .strip_suffix(' ')
            .is_some_and(|name| !name.trim().is_empty()))
    .then_some(Rule::Named)
}

fn border(line: &str, left: char, right: char) -> bool {
    line.trim()
        .strip_prefix(left)
        .and_then(|s| s.strip_suffix(right))
        .is_some_and(horizontal)
}

/// One line of verdicts about the field a door refused to type at (t-86909 r3),
/// for the window log: the next time a refusal comes back from a real pane the
/// log says WHICH part of the reading failed. Verdict words and numbers only —
/// no cell's character is in it, so a draft, a session's name or a path cannot
/// reach the log through it.
pub(crate) fn look(agent: &str, grid: &TerminalGrid) -> String {
    if !reads(agent) {
        return format!("look {agent}: unread");
    }
    let (row, col) = grid.cursor();
    let cursor = if grid.cursor_visible() {
        "shown"
    } else {
        "hidden"
    };
    let paste = if grid.bracketed_paste() { "on" } else { "off" };
    let mut said = format!(
        "look {agent}: cursor={cursor} paste={paste} row={row} of={} col={col}",
        grid.rows()
    );
    if row == 0 || row + 1 >= grid.rows() {
        said.push_str(" edge=yes");
        return said;
    }
    let above = grid.line(row - 1);
    let below = grid.line(row + 1);
    if agent == "claude" {
        let upper = match upper_rule_kind(&above) {
            Some(Rule::Bare) => "rule",
            Some(Rule::Named) => "named",
            None => "other",
        };
        let lower = if horizontal(&below) { "rule" } else { "other" };
        let glyph = if grid.line(row).starts_with("❯") {
            "yes"
        } else {
            "no"
        };
        let caret = match grid.cell(row, col) {
            Some(cell) if cell.style.reverse && cell.ch == ' ' => "blank",
            Some(cell) if cell.style.reverse => "letter",
            Some(_) => "none",
            None => "off",
        };
        let field = match claude_field(grid, row, col) {
            Field::Empty => "empty",
            Field::Ghost => "ghost",
            Field::Text => "text",
        };
        said.push_str(&format!(
            " upper={upper} lower={lower} glyph={glyph} caret={caret} field={field}"
        ));
    } else {
        let top = if border(&above, '╭', '╮') {
            "border"
        } else {
            "other"
        };
        let bottom = if border(&below, '╰', '╯') {
            "border"
        } else {
            "other"
        };
        let prompt = if grid.line(row).trim().starts_with("│› ") {
            "yes"
        } else {
            "no"
        };
        said.push_str(&format!(" top={top} bottom={bottom} prompt={prompt}"));
    }
    said
}

/// Screens of the real Claude Code 2.1.316 and fields built after them, for this module's tests and
/// the answer door's (t-86909 r3).
#[cfg(test)]
pub(crate) mod fixtures {
    use zerocode_pty::Terminal;

    pub(crate) const INLINE: &str =
        include_str!("../tests/fixtures/claude-code-2.1.316-queue-inline.reads");
    pub(crate) const FULLSCREEN: &str =
        include_str!("../tests/fixtures/claude-code-2.1.316-queue-fullscreen.reads");
    /// An empty field at rest and the drafts typed into it, taken from the real CLI.
    pub(crate) const DRAFTS: &str =
        include_str!("../tests/fixtures/claude-code-2.1.316-drafts.reads");

    /// The marks of each recording: the moments the screen was read.
    pub(crate) const RECORDED_MARKS: [(&str, &[&str]); 2] = [
        (INLINE, &["idle", "running", "queued", "after-escape"]),
        (
            FULLSCREEN,
            &["running", "queued-one", "queued-two", "after-escape"],
        ),
    ];

    /// A recorded start replayed up to a mark: every read the CLI wrote until then.
    pub(crate) fn recorded(fixture: &str, mark: &str) -> Terminal {
        use base64::Engine as _;
        let (mut rows, mut cols, mut until) = (40, 110, None);
        let mut reads = Vec::new();
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["size", r, c] => {
                    rows = r.parse().unwrap();
                    cols = c.parse().unwrap();
                }
                ["mark", name, ms] if *name == mark => until = Some(ms.parse::<u64>().unwrap()),
                ["mark", ..] => {}
                [ms, bytes] => reads.push((
                    ms.parse::<u64>().unwrap(),
                    base64::engine::general_purpose::STANDARD
                        .decode(bytes)
                        .unwrap(),
                )),
                _ => {}
            }
        }
        let until = until.unwrap_or_else(|| panic!("the recording has no mark {mark}"));
        let mut term = Terminal::new(rows, cols);
        for (_, bytes) in reads.iter().filter(|(ms, _)| *ms <= until) {
            term.feed(bytes);
        }
        term
    }

    /// The width the running Claude Code captures were taken at.
    pub(crate) const WIDTH: usize = 96;

    /// A rule across the screen with `name` set into it near the right end, the way Claude Code
    /// 2.1.316 draws the session's name into the field's upper border (`───── name ─`). The name is
    /// measured in columns — a Korean letter takes two — so the row is the screen's width.
    pub(crate) fn named_rule(name: &str) -> String {
        let columns: usize = name
            .chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .sum();
        format!("{} {name} ─", "─".repeat(WIDTH - columns - 3))
    }

    /// The caret as 2.1.316 draws it on an empty cell.
    pub(crate) const CARET: &str = "\x1b[7m \x1b[27m";

    /// A 2.1.316 field built by hand, with the terminal's cursor hidden and the caret a reversed
    /// cell the way the recordings draw it: `field` is the row after the upper rule (which carries
    /// a made-up session name), `cursor_col` where the hidden cursor stands. Bracketed paste on.
    pub(crate) fn hidden(field: &str, cursor_col: usize) -> Terminal {
        let mut term = Terminal::new(8, WIDTH + 4);
        term.feed(
            format!(
                "\x1b[?2004h\x1b[?25l\x1b[3;1H{}\r\n{field}\r\n{}\x1b[4;{}H",
                named_rule("zc-run00000-w00000"),
                "─".repeat(WIDTH),
                cursor_col + 1
            )
            .as_bytes(),
        );
        term
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        CARET, DRAFTS, RECORDED_MARKS, WIDTH as RUNNING_WIDTH, hidden, named_rule, recorded,
    };
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

    /// Claude Code's screen while a turn runs, from the text of three captures of a real pane
    /// (t-86909 r2) — the session's name, the paths and the words replaced by made-up ones. The
    /// captures were text only: no cursor, no modes, no attributes. So the fixture gives the
    /// field what the idle screen has — bracketed paste on, the cursor shown at `cursor_col` on
    /// the field's row — and what it takes from the captures is the SHAPE: the spinner and the
    /// effort line above the field, a name set into the upper rule, the footer under the lower.
    fn claude_running(
        above: &[&str],
        upper: &str,
        field: &str,
        cursor_col: usize,
        below: &[&str],
    ) -> Terminal {
        let mut rows: Vec<String> = above.iter().map(|row| (*row).to_string()).collect();
        rows.push(upper.to_string());
        let at = rows.len();
        rows.push(field.to_string());
        rows.push("─".repeat(RUNNING_WIDTH));
        rows.extend(below.iter().map(|row| (*row).to_string()));
        let mut term = Terminal::new(rows.len() + 1, RUNNING_WIDTH + 4);
        term.feed("\x1b[?2004h\x1b[?25h".as_bytes());
        term.feed(rows.join("\r\n").as_bytes());
        term.feed(format!("\x1b[{};{}H", at + 1, cursor_col + 1).as_bytes());
        term
    }

    /// The rows above the field in the first capture: a reply, a tool step, the spinner and the
    /// effort line the spinner leaves at the right.
    fn spinner_rows() -> Vec<&'static str> {
        vec![
            "",
            "⏺ I will read the review report first.",
            "",
            "⏺ Check the working tree",
            "  ⎿  $ git status --short | head -20; git log --oneline -5; ls /tmp/scratch/ 2>&1 |",
            "     head -20",
            "",
            "✶ Bloviating… (6s · ↓ 422 tokens · thought for 1s)",
            "                                                                                ◈ max · /effort",
        ]
    }

    const RUNNING_HINT: &str = "  paste again to expand";
    const RUNNING_STATUS: &str =
        "  ⏵⏵ bypass permissions on (shift+tab to cycle) · esc to interrupt · ← 1 agent";

    /// The field of a running turn is empty, read the way an idle one is — also when the upper
    /// rule carries the session's name (t-86909 r2, P2-4). Until this was measured the reading
    /// took only a bare rule, so a named session's field was never read as empty: the queue's
    /// 끼워 넣기 and 바로 보내기 refused it with `turn-screen` for good.
    #[test]
    fn a_running_claude_field_is_empty_under_a_rule_that_carries_the_sessions_name() {
        let named = named_rule("zc-run00000-w00000");
        let spinner = spinner_rows();
        for (what, term) in [
            (
                "the first capture: spinner, effort line, name in the rule, a hint under",
                claude_running(&spinner, &named, "❯", 2, &[RUNNING_HINT]),
            ),
            (
                "the second and third: no effort line, the status line under",
                claude_running(&spinner[..8], &named, "❯", 2, &[RUNNING_STATUS]),
            ),
            (
                "a name of Korean words",
                claude_running(
                    &spinner,
                    &named_rule("검수 보고서 대기"),
                    "❯",
                    2,
                    &[RUNNING_STATUS],
                ),
            ),
            (
                "a heavy rule",
                claude_running(
                    &spinner,
                    &named.replace('─', "━"),
                    "❯",
                    2,
                    &[RUNNING_STATUS],
                ),
            ),
            (
                "a name with spaces in it",
                claude_running(
                    &spinner,
                    &named_rule("my work session"),
                    "❯",
                    2,
                    &[RUNNING_STATUS],
                ),
            ),
            (
                "no name at all, as the idle screen draws it",
                claude_running(
                    &spinner,
                    &"─".repeat(RUNNING_WIDTH),
                    "❯",
                    2,
                    &[RUNNING_STATUS],
                ),
            ),
        ] {
            assert!(
                observed("claude", term.grid()),
                "{what}: the running field was not read as empty"
            );
        }
    }

    /// A name in the rule proves nothing by itself: the field under it is empty only if it is,
    /// and a rule that is not one — a name with no rule after it, too few marks before it, a
    /// name and no marks, an empty name, a name in the LOWER rule — leaves the field protected.
    #[test]
    fn a_name_in_the_rule_never_makes_a_field_with_words_or_a_stray_line_empty() {
        let named = named_rule("zc-run00000-w00000");
        let spinner = spinner_rows();
        let words = claude_running(&spinner, &named, "❯ fix the login", 15, &[RUNNING_STATUS]);
        assert!(
            !observed("claude", words.grid()),
            "words under a named rule were read as an empty field"
        );
        let faint = claude_running(
            &spinner,
            &named,
            "❯ \x1b[2mrun the tests again\x1b[22m",
            2,
            &[RUNNING_STATUS],
        );
        assert!(
            observed("claude", faint.grid()),
            "a faint suggestion is still an empty field under a named rule"
        );
        let typed_before_faint = claude_running(
            &spinner,
            &named,
            "❯ ru\x1b[2mn the tests again\x1b[22m",
            4,
            &[RUNNING_STATUS],
        );
        assert!(
            !observed("claude", typed_before_faint.grid()),
            "words typed before a faint rest are a draft"
        );
        for (what, upper) in [
            (
                "a name with no rule mark after it",
                "─".repeat(70) + " zc-run00000-w00000",
            ),
            (
                "two marks before the name",
                "── zc-run00000-w00000 ─".to_string(),
            ),
            (
                "a name and no marks before it",
                " zc-run00000-w00000 ─".to_string(),
            ),
            ("a name and nothing else", "zc-run00000-w00000".to_string()),
            ("an empty name", "─".repeat(70) + "   ─"),
            (
                "a hyphen run, not a rule",
                "-".repeat(70) + " zc-run00000-w00000 -",
            ),
        ] {
            let term = claude_running(&spinner, &upper, "❯", 2, &[RUNNING_STATUS]);
            assert!(
                !observed("claude", term.grid()),
                "{what}: `{upper}` was taken for the field's upper border"
            );
        }
        // The lower rule is drawn bare; a name there is not the field's border.
        let mut lower_named = Terminal::new(8, RUNNING_WIDTH + 4);
        lower_named.feed(
            format!(
                "\x1b[?2004h\x1b[?25h\x1b[3;1H{}\r\n❯ \r\n{}\x1b[4;3H",
                "─".repeat(RUNNING_WIDTH),
                named_rule("zc-run00000-w00000")
            )
            .as_bytes(),
        );
        assert!(
            !observed("claude", lower_named.grid()),
            "a name in the lower rule was taken for the field's border"
        );
    }

    /// The hand-over's probe is read off the same field, so a named session's field holds it
    /// the same way (t-86909 r2): one space typed on the empty line, the cursor one column on.
    #[test]
    fn the_probe_is_read_under_a_rule_that_carries_the_sessions_name_too() {
        let named = named_rule("zc-run00000-w00000");
        let field = |cursor_col: usize| {
            let mut term = Terminal::new(6, RUNNING_WIDTH + 4);
            term.feed(
                format!(
                    "\x1b[?2004h\x1b[?25h{named}\r\n❯ \r\n{}\x1b[2;{}H",
                    "─".repeat(RUNNING_WIDTH),
                    cursor_col + 1
                )
                .as_bytes(),
            );
            term
        };
        assert!(holds_probe("claude", field(3).grid()));
        assert!(
            !holds_probe("claude", field(2).grid()),
            "an empty field holds no probe"
        );
    }

    /// zo while a turn runs (t-86909 r2, P2-4). Read from zo's own drawing, not from a capture:
    /// `build_bottom_pane` in `zo-ide/crates/zo-ide/src/tui/view.rs` stands the status line
    /// (`• Working (12s • esc to interrupt)`), a blank row, the queued input if any, a blank
    /// row, then the same bordered composer an idle zo has — `render_with_effort` in
    /// `tui/composer.rs` draws the `╭…╮` / `│› Ask zo to do anything…│` / `╰…╯` rows whatever the
    /// turn is doing; the effort tier and the border's glow change only their colours — and
    /// two footer rows. The painter shows the cursor whenever the frame gives it a place in the
    /// composer (`painter.rs` `end`), which a running turn does.
    fn zo_running(styled_body: &str, plain_body: &str) -> Terminal {
        let width = 64;
        let inner = "─".repeat(width - 2);
        let pad = " ".repeat(width - 4 - plain_body.chars().count());
        let mut term = Terminal::new(11, width);
        term.feed("\x1b[?2004h\x1b[?25h".as_bytes());
        term.feed(
            format!(
                "\r\n• Working (12s • esc to interrupt)\r\n\r\n\r\n╭{inner}╮\r\n│› {styled_body}{pad}│\r\n╰{inner}╯\r\n\r\n  gpt-test high · 91% context left · ~/work/project\r\n  ? for shortcuts\x1b[6;4H"
            )
            .as_bytes(),
        );
        term
    }

    #[test]
    fn a_running_zo_composer_is_empty_when_it_shows_its_placeholder_and_not_when_it_shows_words() {
        let placeholder = "Ask zo to do anything";
        let dim = format!("\x1b[2m{placeholder}\x1b[22m");
        assert!(
            observed("zo", zo_running(&dim, placeholder).grid()),
            "zo's composer with a turn running was not read as empty"
        );
        for body in ["fix the login", placeholder] {
            assert!(
                !observed("zo", zo_running(body, body).grid()),
                "words in a running zo's composer were read as empty: {body}"
            );
        }
        // The Claude reading is not zo's: the same rows are no Claude field.
        assert!(!observed("claude", zo_running(&dim, placeholder).grid()));
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

    // ---- Claude Code 2.1.316 as it really draws (t-86909 r3) --------------------------------
    //
    // Measured on the real binary in a pty, run against a fake Messages API with the network shut
    // to everything but 127.0.0.1 (the two `.reads` files, taken with a scratch HOME): while a
    // turn runs, and at rest, it HIDES the terminal's cursor (`?25l`, once, at the start) and
    // draws its own caret as a reversed cell on the field's row; the terminal's cursor stays on
    // that cell, hidden. Bracketed paste is on. With a message queued in the CLI the empty field
    // shows `Press up to edit queued messages`: its first letter is the reversed cell, the rest
    // drawn faint. Escape with messages queued does not put them back on the field — the CLI sends
    // them at once as the next turn. Until now the reading asked for a SHOWN cursor and so took
    // no field of this CLI for empty: the queue's 끼워 넣기 and 바로 보내기 refused every pane.

    /// What the reading stands on, stated about the recordings themselves so that a change of the
    /// grid or of the files is noticed here and not as a refusal in somebody's pane.
    #[test]
    fn the_recorded_screens_hide_the_cursor_keep_bracketed_paste_on_and_stand_the_cursor_on_the_caret()
     {
        for (fixture, marks) in RECORDED_MARKS {
            for mark in marks {
                let term = recorded(fixture, mark);
                let grid = term.grid();
                assert!(grid.bracketed_paste(), "{mark}: bracketed paste is off");
                assert!(
                    !grid.cursor_visible(),
                    "{mark}: the terminal's cursor is shown — the CLI draws its own caret"
                );
                let (row, col) = grid.cursor();
                assert_eq!(
                    col, 2,
                    "{mark}: the cursor is not on the field's first cell"
                );
                assert!(
                    grid.line(row).starts_with('❯'),
                    "{mark}: not the field's row"
                );
                assert!(
                    grid.cell(row, col).is_some_and(|cell| cell.style.reverse),
                    "{mark}: the caret is not a reversed cell"
                );
            }
        }
    }

    /// The field of a real Claude Code 2.1.316 is empty — turn running or not, queue empty or
    /// holding messages (`Press up to edit queued messages`, its first letter the caret) — and the
    /// reading says so. Inline and alternate-screen layouts both.
    #[test]
    fn the_field_of_a_real_claude_code_2_1_316_is_read_as_empty_whatever_the_turn_does() {
        for (fixture, marks) in RECORDED_MARKS {
            for mark in marks {
                let term = recorded(fixture, mark);
                assert!(
                    observed("claude", term.grid()),
                    "{mark}: the recorded field was not read as empty"
                );
            }
        }
    }

    /// The placeholder as the alternate-screen layout draws it (one faint run) and as the inline
    /// one does (a faint run per word, plain gaps between).
    fn placeholder_runs() -> [String; 2] {
        [
            "❯ \x1b[7mP\x1b[27m\x1b[2mress up to edit queued messages\x1b[22m".to_string(),
            "❯ \x1b[7mP\x1b[27m\x1b[2mress\x1b[22m \x1b[2mup\x1b[22m \x1b[2mto\x1b[22m \x1b[2medit\x1b[22m \x1b[2mqueued\x1b[22m \x1b[2mmessages\x1b[22m"
                .to_string(),
        ]
    }

    #[test]
    fn a_hidden_cursor_and_a_caret_cell_make_an_empty_field_and_the_placeholder_is_one_too() {
        assert!(observed("claude", hidden(&format!("❯ {CARET}"), 2).grid()));
        for field in placeholder_runs() {
            assert!(
                observed("claude", hidden(&field, 2).grid()),
                "the queued placeholder was not read as empty: {field:?}"
            );
        }
        // A pane the window has told it lost the keyboard stops drawing the caret: the field
        // is `❯` and blanks, and the hidden cursor still stands on its first cell.
        assert!(observed("claude", hidden("❯ ", 2).grid()));
    }

    /// Never empty, however the caret and the cursor stand (t-86909 r3): what a person wrote
    /// reads as words. The dangerous shapes are the ones where the caret sits on the first cell,
    /// so the field LOOKS like the placeholder's — a reversed first letter and then nothing, or
    /// plain letters, or a chip, or something faint far to the right.
    #[test]
    fn a_draft_is_never_read_as_empty_under_a_hidden_cursor() {
        let faint_far = "❯ \x1b[7mx\x1b[27m\x1b[60G\x1b[2mstatus\x1b[22m";
        let cases: [(&str, String, usize); 10] = [
            ("one letter, caret after it", format!("❯ x{CARET}"), 3),
            (
                "one letter, caret moved to the start",
                "❯ \x1b[7mx\x1b[27m".to_string(),
                2,
            ),
            (
                "one letter and nothing else but a faint status far right",
                faint_far.to_string(),
                2,
            ),
            (
                "letters after the caret's letter",
                "❯ \x1b[7mx\x1b[27myz".to_string(),
                2,
            ),
            (
                "the placeholder's words typed by a person",
                "❯ \x1b[7mP\x1b[27mress up to edit queued messages".to_string(),
                2,
            ),
            (
                "a paste chip, caret at the start",
                "❯ \x1b[7m[\x1b[27mPasted text #1 +12 lines]xyz".to_string(),
                2,
            ),
            (
                "a faint paste chip, caret at the start",
                "❯ \x1b[7m[\x1b[27m\x1b[2mPasted text #1 +12 lines]\x1b[22m".to_string(),
                2,
            ),
            (
                "a letter and a plain word far right",
                "❯ \x1b[7mx\x1b[27m\x1b[60Gword".to_string(),
                2,
            ),
            (
                "the caret blank but a plain letter after it",
                format!("❯ {CARET}x"),
                2,
            ),
            ("the cursor off the first cell", format!("❯ {CARET}"), 3),
        ];
        for (what, field, cursor_col) in cases {
            assert!(
                !observed("claude", hidden(&field, cursor_col).grid()),
                "{what}: a person's draft was read as empty ({field:?})"
            );
        }
    }

    /// The hand-over's probe — one space typed on the empty field, the caret one cell on — is
    /// read under a hidden cursor too (t-86909 r3): the cursor was once shown there as well.
    #[test]
    fn the_probe_is_read_under_a_hidden_cursor_and_its_caret_cell() {
        assert!(holds_probe(
            "claude",
            hidden(&format!("❯  {CARET}"), 3).grid()
        ));
        assert!(
            !holds_probe("claude", hidden(&format!("❯ {CARET}"), 2).grid()),
            "an empty field holds no probe"
        );
        assert!(
            !holds_probe("claude", hidden(&format!("❯ x{CARET}"), 3).grid()),
            "a typed letter is no probe"
        );
    }

    /// What a door that refuses writes in the window log (t-86909 r3): the verdicts the reading
    /// reached, and no cell's character — a draft, a name or a path cannot reach the log through
    /// it. Cursor shown or hidden, bracketed paste, the cursor's column, whether the rules above
    /// and below the field and the prompt glyph were found, what the caret cell and the rest of
    /// the row were.
    #[test]
    fn the_look_a_refusal_logs_is_verdicts_and_not_words() {
        let secret = "passw0rd-in-the-draft";
        let draft = hidden(&format!("❯ {secret}{CARET}"), 2 + secret.len());
        let said = look("claude", draft.grid());
        assert!(!said.is_empty(), "the look said nothing");
        for token in [
            "cursor=hidden",
            "paste=on",
            "upper=named",
            "lower=rule",
            "glyph=yes",
        ] {
            assert!(said.contains(token), "{token} missing from {said:?}");
        }
        assert!(
            said.chars()
                .all(|ch| ch.is_ascii() && !ch.is_ascii_uppercase()),
            "something other than verdict words and numbers: {said:?}"
        );
        for fragment in ["passw0rd", "draft", "zc-run", "run00000"] {
            assert!(
                !said.contains(fragment),
                "{fragment} reached the log: {said:?}"
            );
        }
        let empty = look("claude", hidden(&format!("❯ {CARET}"), 2).grid());
        assert_ne!(said, empty, "a draft and an empty field looked the same");
        // Not the agent this module can read: it says so and nothing else.
        assert!(look("codex", draft.grid()).contains("unread"));
    }

    /// The drafts the real CLI drew (t-86909 r3): the shapes the hand-built ones above imitate. A
    /// letter with the caret after it, the caret moved onto it, three letters, a paste chip, the
    /// caret on a letter in front of a chip — none is an empty field, and the field before them is.
    #[test]
    fn the_drafts_a_real_claude_code_2_1_316_drew_are_never_read_as_empty() {
        assert!(
            observed("claude", recorded(DRAFTS, "idle").grid()),
            "the empty field before the drafts was not read as empty"
        );
        for mark in [
            "typed-x",
            "x-caret-at-start",
            "xyz-caret-at-end",
            "xyz-caret-at-start",
            "big-paste",
            "big-paste-caret-at-start",
            "q-left-arrow",
        ] {
            assert!(
                !observed("claude", recorded(DRAFTS, mark).grid()),
                "{mark}: a real draft was read as empty"
            );
        }
    }

    /// The probe of the hand-over on a real field (t-86909 r3): a space typed on the empty field
    /// is the recorded screen of a typed letter with that letter's cell blank — the caret stands
    /// one cell on, the cursor with it, hidden. It holds the probe; the letter is none.
    #[test]
    fn the_probe_on_a_real_claude_code_2_1_316_field_is_held_and_a_letter_is_not() {
        let letter = recorded(DRAFTS, "typed-x");
        let (row, col) = letter.grid().cursor();
        assert!(
            !holds_probe("claude", letter.grid()),
            "a letter is no probe"
        );
        let mut space = recorded(DRAFTS, "typed-x");
        // the letter's cell is the one before the caret (the column is 0-based, the move 1-based)
        space.feed(format!("\x1b[{};{}H ", row + 1, col).as_bytes());
        assert!(
            holds_probe("claude", space.grid()),
            "the probe a real field shows was not held"
        );
        assert!(
            !holds_probe("claude", recorded(DRAFTS, "idle").grid()),
            "an empty field holds no probe"
        );
    }
}
