//! What the grid supports is defined here, not in the parser (ADR 0002).
//!
//! The byte sequences are the ones real programs emit — `\r\n` from a shell,
//! `ESC[2J` from `clear`, `ESC[?2004h` from a line editor turning on bracketed
//! paste — so a passing test means we handle what actually arrives.

use std::sync::Arc;

use zerocode_pty::{Cell, CellStyle, Color, GridDelta, Rgb, Terminal, TerminalColors};

fn fed(bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(bytes);
    terminal
}

#[test]
fn prints_text_and_advances_the_cursor() {
    let terminal = fed(b"hello");
    assert_eq!(terminal.grid().line(0), "hello");
    assert_eq!(terminal.grid().cursor(), (0, 5));
}

#[test]
fn carriage_return_rewinds_and_overwrites_in_place() {
    // A progress bar redrawing itself: same row, column reset, new text.
    let terminal = fed(b"downloading\rdone");
    assert_eq!(terminal.grid().line(0), "doneloading");
    assert_eq!(terminal.grid().cursor(), (0, 4));
}

#[test]
fn crlf_starts_a_fresh_row() {
    let terminal = fed(b"first\r\nsecond");
    assert_eq!(terminal.grid().line(0), "first");
    assert_eq!(terminal.grid().line(1), "second");
    assert_eq!(terminal.grid().cursor(), (1, 6));
}

#[test]
fn backspace_moves_left_so_the_next_write_replaces() {
    let terminal = fed(b"cat\x08\x08ut");
    assert_eq!(terminal.grid().line(0), "cut");
}

#[test]
fn text_wraps_onto_the_next_row_at_the_right_edge() {
    // 20 columns: 25 characters must spill.
    let terminal = fed(b"abcdefghijklmnopqrstuvwxy");
    assert_eq!(terminal.grid().line(0), "abcdefghijklmnopqrst");
    assert_eq!(terminal.grid().line(1), "uvwxy");
}

#[test]
fn output_past_the_last_row_scrolls_into_scrollback() {
    let mut terminal = Terminal::new(3, 20);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");

    assert_eq!(terminal.grid().scrollback_len(), 1);
    assert_eq!(terminal.grid().scrollback_line(0), "one");
    assert_eq!(terminal.grid().line(0), "two");
    assert_eq!(terminal.grid().line(2), "four");
}

#[test]
fn cursor_position_is_one_based_on_the_wire_and_zero_based_here() {
    let terminal = fed(b"\x1b[3;5Hx");
    assert_eq!(terminal.grid().cursor(), (2, 5));
    assert_eq!(terminal.grid().line(2), "    x");
}

#[test]
fn cursor_motion_stays_inside_the_screen() {
    // Far more than the screen holds, in every direction.
    let terminal = fed(b"\x1b[99B\x1b[99C\x1b[99A\x1b[99D");
    assert_eq!(terminal.grid().cursor(), (0, 0));
}

#[test]
fn erase_display_clears_the_screen_like_clear_does() {
    let mut terminal = fed(b"keep\r\nthis\r\naround");
    terminal.feed(b"\x1b[2J");
    assert_eq!(terminal.grid().visible_text(), "");
}

/// An unsupported motion must degrade to "cursor did not move", never to
/// corrupted text. `ESC[4G` (CHA) is outside our subset, so the cursor stays at
/// column 6 and the following `ESC[K` erases an already-empty tail.
#[test]
fn an_unsupported_motion_leaves_text_intact() {
    // The example moved, not the contract. This was `ESC[4G`, which the grid
    // used to swallow — it is cursor-to-column, most TUIs begin every line
    // they draw with it, and it is implemented now. `ESC[3 q` (cursor shape)
    // is still swallowed, and the rule it stands for is unchanged: a sequence
    // this terminal does not implement must leave the screen alone rather
    // than reach it as stray glyphs.
    let terminal = fed(b"abcdef\x1b[3 q\x1b[K");
    assert_eq!(terminal.grid().line(0), "abcdef");
    assert_eq!(terminal.grid().cursor(), (0, 6));
}

#[test]
fn erase_line_after_a_supported_motion_clears_the_tail() {
    let terminal = fed(b"abcdef\x1b[1;4H\x1b[K");
    assert_eq!(terminal.grid().line(0), "abc");
}

#[test]
fn osc_two_sets_the_window_title() {
    // The em dash is written as UTF-8 bytes: a title is not ASCII-only, and the
    // grid must hand it back intact.
    let terminal = fed(b"\x1b]2;zo \xE2\x80\x94 drain gate\x07");
    assert_eq!(terminal.grid().title(), Some("zo — drain gate"));
}

#[test]
fn osc_zero_also_sets_the_title() {
    let terminal = fed(b"\x1b]0;codex\x07");
    assert_eq!(terminal.grid().title(), Some("codex"));
}

#[test]
fn osc_fifty_two_decodes_one_bounded_clipboard_write() {
    let mut terminal = fed(b"\x1b]52;c;em8g4oCUIOuBnQ==\x07");
    let replayable = serde_json::to_string(&terminal.grid().snapshot()).expect("screen snapshot");
    assert!(
        !replayable.contains("zo") && !replayable.contains("끝"),
        "screen replay must not repeat a clipboard side effect"
    );
    assert!(
        !format!("{terminal:?}").contains("zo — 끝"),
        "clipboard text leaked through terminal Debug output"
    );

    assert_eq!(
        terminal.grid_mut().take_osc52_clipboard_write().as_deref(),
        Some("zo — 끝")
    );
    assert_eq!(
        terminal.grid_mut().take_osc52_clipboard_write(),
        None,
        "a clipboard request is an event, not terminal state"
    );
    assert!(
        terminal.grid_mut().take_delta().is_none(),
        "a clipboard side effect must not create a screen frame"
    );
    terminal.feed(b"\x1b]52;c;_w==\x07");
    assert_eq!(
        terminal.grid_mut().take_osc52_clipboard_write(),
        None,
        "URL-safe base64 is outside Orca's OSC 52 grammar"
    );
    terminal.feed(b"\x1b]52;c;/w==\x07");
    assert_eq!(
        terminal.grid_mut().take_osc52_clipboard_write().as_deref(),
        Some("�"),
        "Orca decodes malformed UTF-8 with replacement rather than rejecting it"
    );
}

#[test]
fn osc_fifty_two_accepts_orcas_selectors_and_whitespace_and_keeps_the_latest() {
    let mut terminal = fed(b"\x1b]52;;Zmlyc3Q=\x07");
    terminal.feed(b"\x1b]52;pqs07;c2Vj b25k\r\n\x07");

    assert_eq!(
        terminal.grid_mut().take_osc52_clipboard_write().as_deref(),
        Some("second"),
        "one bounded slot coalesces a TUI repaint to its last request"
    );
}

#[test]
fn osc_fifty_two_swallows_queries_invalid_or_empty_writes() {
    for sequence in [
        b"\x1b]52;c;?\x07".as_slice(),
        b"\x1b]52;x;dmFsdWU=\x07",
        b"\x1b]52;c;not;base64\x07",
        b"\x1b]52;c;\x07",
        b"\x1b]52;c;%%%\x07",
    ] {
        let mut terminal = fed(sequence);
        assert_eq!(terminal.grid_mut().take_osc52_clipboard_write(), None);
    }

    let oversized = "A".repeat(128 * 1024 + 1);
    let mut terminal = fed(format!("\x1b]52;c;{oversized}\x07").as_bytes());
    assert_eq!(terminal.grid_mut().take_osc52_clipboard_write(), None);
}

#[test]
fn bracketed_paste_mode_tracks_the_program() {
    let mut terminal = fed(b"\x1b[?2004h");
    assert!(terminal.grid().bracketed_paste());
    terminal.feed(b"\x1b[?2004l");
    assert!(!terminal.grid().bracketed_paste());
}

#[test]
fn alternate_screen_is_tracked_and_clears_on_entry() {
    let mut terminal = fed(b"primary text");
    terminal.feed(b"\x1b[?1049h");
    assert!(terminal.grid().alt_screen());
    assert_eq!(terminal.grid().visible_text(), "");
    assert_eq!(terminal.grid().cursor(), (0, 0));

    terminal.feed(b"\x1b[?1049l");
    assert!(!terminal.grid().alt_screen());
}

/// A full-screen program (an editor, a pager, an agent TUI) must be able to
/// take a lane over and give it back. Clearing on exit instead of restoring
/// eats whatever the user was looking at.
#[test]
fn leaving_the_alternate_screen_restores_the_primary_one() {
    let mut terminal = fed(b"before\r\nthe editor");
    terminal.feed(b"\x1b[?1049h");
    // Fits the 20-column fixture, so a wrap does not muddy the assertion.
    terminal.feed(b"a full-screen app");
    assert_eq!(terminal.grid().visible_text(), "a full-screen app");

    terminal.feed(b"\x1b[?1049l");
    assert_eq!(terminal.grid().visible_text(), "before\nthe editor");
    assert_eq!(terminal.grid().cursor(), (1, 10));
}

/// A repeated enter must not park the alternate screen over the saved primary —
/// that loses the original for good.
#[test]
fn entering_the_alternate_screen_twice_still_restores_the_original() {
    let mut terminal = fed(b"original");
    terminal.feed(b"\x1b[?1049h");
    terminal.feed(b"alt");
    terminal.feed(b"\x1b[?1049h");
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(terminal.grid().visible_text(), "original");
}

// ------------------------------------------------------------------- styling

#[test]
fn sgr_paints_the_cells_that_follow_it() {
    let terminal = fed(b"\x1b[1;31mred\x1b[0m plain");
    let red = terminal.grid().cell(0, 0).expect("cell");
    assert_eq!(red.ch, 'r');
    assert!(red.style.bold);
    assert_eq!(red.style.fg, Color::Indexed(1));

    let plain = terminal.grid().cell(0, 4).expect("cell");
    assert_eq!(plain.ch, 'p');
    assert!(!plain.style.bold);
    assert_eq!(plain.style.fg, Color::Default);
}

#[test]
fn a_bare_sgr_is_a_reset() {
    let mut terminal = fed(b"\x1b[1;4;7m");
    assert!(terminal.grid().pen().bold);
    terminal.feed(b"\x1b[m");
    assert_eq!(terminal.grid().pen(), CellStyle::default());
}

#[test]
fn attribute_and_colour_resets_are_individually_addressable() {
    let mut terminal = fed(b"\x1b[1;3;4;7;31;44m");
    terminal.feed(b"\x1b[22;23;24;27;39;49m");
    assert_eq!(terminal.grid().pen(), CellStyle::default());
}

#[test]
fn bright_colour_codes_map_onto_the_upper_half_of_the_palette() {
    let terminal = fed(b"\x1b[91;104m");
    assert_eq!(terminal.grid().pen().fg, Color::Indexed(9));
    assert_eq!(terminal.grid().pen().bg, Color::Indexed(12));
}

/// Both spellings appear in the wild: `;` separated (most programs) and `:`
/// separated subparameters.
#[test]
fn extended_colours_are_decoded_in_both_encodings() {
    let semicolons = fed(b"\x1b[38;5;196;48;2;10;20;30m");
    assert_eq!(semicolons.grid().pen().fg, Color::Indexed(196));
    assert_eq!(semicolons.grid().pen().bg, Color::Rgb(10, 20, 30));

    let colons = fed(b"\x1b[38:2:255:0:0m");
    assert_eq!(colons.grid().pen().fg, Color::Rgb(255, 0, 0));
}

/// The two encodings can appear in one sequence, and then **order decides the
/// result**. Handling all the colon groups first and the semicolon ones after
/// silently reverses `reset, then colour` into `colour, then reset`.
#[test]
fn mixed_semicolon_and_colon_groups_are_applied_in_wire_order() {
    // Reset first, then a colon-form truecolor: the colour must survive.
    let reset_then_colour = fed(b"\x1b[0;38:2:255:0:0m");
    assert_eq!(
        reset_then_colour.grid().pen().fg,
        Color::Rgb(255, 0, 0),
        "the reset preceded the colour, so the colour must win"
    );

    // The same pair the other way round: the reset must win.
    let colour_then_reset = fed(b"\x1b[38:2:255:0:0;0m");
    assert_eq!(colour_then_reset.grid().pen(), CellStyle::default());
}

#[test]
fn an_attribute_after_a_semicolon_colour_still_applies() {
    let terminal = fed(b"\x1b[38;5;196;1m");
    assert_eq!(terminal.grid().pen().fg, Color::Indexed(196));
    assert!(terminal.grid().pen().bold);
}

/// A truncated `38;` must not have its tail read as unrelated attributes —
/// that is how a colour sequence turns into stray bold text.
#[test]
fn a_malformed_extended_colour_does_not_leak_into_attributes() {
    let terminal = fed(b"\x1b[38;1m");
    let pen = terminal.grid().pen();
    assert_eq!(pen.fg, Color::Default);
    assert!(!pen.bold, "the trailing 1 must not be read as bold");
}

/// The failure mode that makes a terminal feel broken: an escape sequence we do
/// not implement leaking onto the screen as text.
#[test]
fn unsupported_sequences_are_swallowed_never_drawn() {
    let terminal = fed(b"\x1b[1;31mred\x1b[0m \x1b[38;2;255;0;0mtruecolor\x1b[m \x1b[6nreport");
    let line = terminal.grid().line(0);
    assert_eq!(line, "red truecolor report");
    assert!(!line.contains('\u{1b}'), "escape leaked: {line:?}");
    assert!(!line.contains('['), "csi leaked: {line:?}");
    assert!(!line.contains("31"), "sgr parameters leaked: {line:?}");
}

#[test]
fn a_multibyte_character_split_across_reads_prints_once() {
    // Korean, three bytes, arriving in two separate pty reads.
    let bytes = "가".as_bytes();
    let mut terminal = Terminal::new(4, 20);
    terminal.feed(&bytes[..1]);
    terminal.feed(&bytes[1..]);
    assert_eq!(terminal.grid().line(0), "가");
}

#[test]
fn resizing_narrower_then_wider_keeps_rows_in_scrollback() {
    let mut terminal = Terminal::new(4, 20);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(terminal.grid().scrollback_len(), 0);

    terminal.grid_mut().resize(2, 20);
    assert_eq!(terminal.grid().rows(), 2);
    assert_eq!(terminal.grid().scrollback_len(), 2);
    assert_eq!(terminal.grid().scrollback_line(0), "one");

    terminal.grid_mut().resize(4, 30);
    assert_eq!(terminal.grid().rows(), 4);
    assert_eq!(terminal.grid().cols(), 30);
}

#[test]
fn the_dirty_flag_reports_changes_once() {
    let mut terminal = Terminal::new(3, 10);
    assert!(!terminal.grid().is_dirty());

    terminal.feed(b"x");
    assert!(terminal.grid().is_dirty());
    assert!(terminal.grid_mut().take_dirty());
    assert!(!terminal.grid().is_dirty());
}

// ------------------------------------------------------------- ready glyph

/// Codex's composer glyph, which is what asks this question in production.
const MARKER: char = '›';

/// The watched glyph is caught in the round it is drawn, and only that round.
///
/// The whole signal is the edge. A prompt delivery waits for the agent to say
/// its input line is *now* drawn, and the glyph sitting in a cell says only
/// that it was drawn at some point — so the answer has to expire the moment
/// somebody takes it, even though the screen still reads the same.
#[test]
fn a_glyph_is_caught_the_round_it_is_drawn_and_not_after() {
    let mut terminal = Terminal::new(4, 20);
    // Arm the watch: nothing was being noticed before this.
    assert!(!terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere);

    terminal.feed("› ".as_bytes());
    assert!(
        terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "the glyph was drawn this round and went unnoticed"
    );
    assert!(
        !terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "the same drawing answered twice — a level, not an edge"
    );
}

/// A glyph still on screen from an earlier round is scrollback, not readiness.
///
/// This is the case the edge exists for: an agent that printed its composer
/// glyph before anybody was waiting has not announced itself to the delivery
/// that arrived afterwards. Reading the screen instead of the round would
/// hand it a prompt while it was still drawing.
#[test]
fn a_glyph_left_on_screen_from_an_earlier_round_is_not_caught() {
    let mut terminal = Terminal::new(4, 20);
    terminal.grid_mut().take_glyph_drawn(Some(MARKER));
    terminal.feed("› ".as_bytes());
    assert!(terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere);

    // Later rounds: the program keeps working, the glyph keeps sitting there.
    terminal.feed(b"\x1b[2;1Hthinking");
    assert!(
        !terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "a glyph drawn in an earlier round was counted again"
    );
    assert!(
        terminal.grid().visible_text().contains(MARKER),
        "the test proved nothing: the glyph is not on screen at all"
    );
}

/// Only the glyph asked about answers.
///
/// Everything an agent prints goes through the same path, and a watch that
/// fires on any character at all makes the first byte of output read as
/// "ready" — which is the state before the handshake, on every program.
#[test]
fn another_character_is_not_the_glyph() {
    let mut terminal = Terminal::new(4, 20);
    terminal.grid_mut().take_glyph_drawn(Some(MARKER));

    terminal.feed("busy… ▸ >".as_bytes());
    assert!(
        !terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "some other character was accepted as the marker"
    );

    // And nothing at all is noticed while nobody is waiting: the round after
    // the watch is dropped draws the glyph and answers no.
    terminal.grid_mut().take_glyph_drawn(None);
    terminal.feed("› ".as_bytes());
    assert!(
        !terminal.grid_mut().take_glyph_drawn(None).anywhere,
        "the glyph was caught by a watch nobody armed"
    );
}

/// A glyph whose bytes arrive in two reads is still one drawn character.
///
/// The reason the question is asked of the grid and not of the byte stream:
/// `›` is three bytes and a pty read can end between any two of them. A search
/// for the bytes would miss it in exactly the case the delivery is waiting on,
/// and then spend the whole eight-second budget.
#[test]
fn a_glyph_split_across_two_reads_is_still_caught() {
    let bytes = "›".as_bytes();
    assert_eq!(bytes.len(), 3, "the split being tested is not a split");
    let mut terminal = Terminal::new(4, 20);
    terminal.grid_mut().take_glyph_drawn(Some(MARKER));

    terminal.feed(&bytes[..2]);
    assert!(
        !terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "half a character was reported as drawn"
    );
    terminal.feed(&bytes[2..]);
    assert!(
        terminal.grid_mut().take_glyph_drawn(Some(MARKER)).anywhere,
        "the glyph was lost at the chunk boundary"
    );
    assert_eq!(terminal.grid().line(0), "›");
}

/// Which SCREEN took the draw is part of the answer, judged at pen time.
///
/// grok's `❯` is its composer glyph only inside the alternate screen — in
/// the normal buffer the same character is starship's prompt, drawn by the
/// very shell that ran the launch command. The readiness door for grok reads
/// the alt half; a draw in the normal buffer must land as `anywhere` alone.
#[test]
fn the_screen_that_took_the_draw_travels_with_the_answer() {
    let mut terminal = Terminal::new(4, 20);
    terminal.grid_mut().take_glyph_drawn(Some('❯'));

    // The shell's prompt, normal buffer.
    terminal.feed("❯ ".as_bytes());
    let shell = terminal.grid_mut().take_glyph_drawn(Some('❯'));
    assert!(
        shell.anywhere && !shell.in_alt_screen,
        "a normal-buffer draw claimed the alternate screen: {shell:?}"
    );

    // grok enters the alternate screen and mounts its composer.
    terminal.feed(b"\x1b[?1049h");
    terminal.feed("❯ ".as_bytes());
    let composer = terminal.grid_mut().take_glyph_drawn(Some('❯'));
    assert!(
        composer.anywhere && composer.in_alt_screen,
        "the alt-screen draw was not judged at pen time: {composer:?}"
    );

    // Taking cleared both halves; leaving the screen draws nothing by itself.
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(
        terminal.grid_mut().take_glyph_drawn(Some('❯')),
        zerocode_pty::GlyphDrawn {
            anywhere: false,
            in_alt_screen: false
        },
        "a take left half an answer behind"
    );
}

// --------------------------------------------------------------------- delta

/// A consumer of [`GridDelta`], written the way the view layer has to write it.
///
/// It exists so the shift contract is tested by *obeying* it rather than by
/// asserting on field values: if the rules on `GridDelta` are wrong, or the
/// grid stops honouring them, this mirror drifts from the real screen and the
/// replay test below fails.
struct Mirror {
    rows: Vec<Vec<Cell>>,
    cols: usize,
}

impl Mirror {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows: vec![vec![Cell::default(); cols]; rows],
            cols,
        }
    }

    fn apply(&mut self, delta: &GridDelta) {
        if delta.full {
            // Absolute: whatever we held is meaningless now.
            let (rows, cols) = delta.size;
            self.rows = vec![vec![Cell::default(); cols]; rows];
            self.cols = cols;
        } else {
            for _ in 0..delta.scrolled_lines {
                self.rows.remove(0);
                self.rows.push(vec![Cell::default(); self.cols]);
            }
        }
        for row in &delta.rows {
            self.rows[row.index] = row.cells.clone();
        }
    }

    /// The same trimming rule `visible_text` uses, so the two are comparable.
    fn text(&self) -> String {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|cells| {
                cells
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

#[test]
fn an_untouched_grid_has_no_delta_to_report() {
    let mut terminal = Terminal::new(6, 20);
    assert!(terminal.grid_mut().take_delta().is_none());
}

#[test]
fn one_printed_character_reports_exactly_the_row_it_landed_on() {
    let mut terminal = fed(b"x");

    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(delta.rows.len(), 1, "{delta:?}");
    assert_eq!(delta.rows[0].index, 0);
    assert_eq!(delta.rows[0].cells[0].ch, 'x');
    assert_eq!(delta.rows[0].cells.len(), 20, "rows are not trimmed");
    assert_eq!(delta.cursor, (0, 1));
    assert_eq!(delta.size, (6, 20));
    assert!(!delta.full);
    assert_eq!(delta.scrolled_lines, 0);
}

#[test]
fn a_delta_is_reported_once_and_then_the_screen_is_quiet() {
    let mut terminal = fed(b"x");
    assert!(terminal.grid_mut().take_delta().is_some());
    assert!(terminal.grid_mut().take_delta().is_none());
}

/// A cursor that moves without touching a cell still has to be reported, or the
/// caret in the view sits wherever it was last painted. This is most of what a
/// line editor does: `CR`, arrow keys and `CUP` move the caret constantly and
/// change nothing else.
#[test]
fn a_cursor_move_that_changes_no_cell_is_still_reported() {
    let mut terminal = fed(b"hello");
    terminal.grid_mut().take_delta().expect("first paint");

    terminal.feed(b"\x1b[3;5H");
    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("a cursor move is a change the view has to draw");
    assert_eq!(delta.cursor, (2, 4));
    assert!(
        delta.rows.is_empty(),
        "no cell changed, so no row should be resent: {delta:?}"
    );
    assert_eq!(delta.scrolled_lines, 0);
    assert!(!delta.full);

    // Carriage return: the same case from the byte side.
    terminal.feed(b"\r");
    let delta = terminal.grid_mut().take_delta().expect("carriage return");
    assert_eq!(delta.cursor, (2, 0));
    assert!(delta.rows.is_empty(), "{delta:?}");

    // And a cursor that did not move is still quiet.
    assert!(terminal.grid_mut().take_delta().is_none());
}

/// The whole reason the delta exists: a scrolling screen must not resend the
/// rows that merely moved. Only the newly exposed row is sent, and the shift is
/// reported instead.
#[test]
fn scrolling_reports_the_shift_rather_than_resending_the_rows_that_moved() {
    let mut terminal = Terminal::new(3, 20);
    terminal.feed(b"one\r\ntwo\r\nthree");
    terminal.grid_mut().take_delta().expect("first paint");

    terminal.feed(b"\r\nfour");

    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(delta.scrolled_lines, 1);
    assert_eq!(
        delta.rows.len(),
        1,
        "only the exposed row changed; the rest slid: {delta:?}"
    );
    assert_eq!(delta.rows[0].index, 2);
    assert!(!delta.full);
}

/// The test that actually protects the contract. A consumer that obeys the
/// documented order must end up with exactly the screen the grid has, across
/// many scrolls and several takes.
#[test]
fn replaying_deltas_into_a_mirror_reproduces_the_screen_exactly() {
    let mut terminal = Terminal::new(6, 20);
    let mut mirror = Mirror::new(6, 20);
    let mut shifted_by_more_than_one = false;

    for n in 0..40 {
        terminal.feed(format!("line {n}\r\n").as_bytes());
        // Take at an uneven cadence so several scrolls pile up between takes,
        // which is the case a naive implementation gets wrong.
        if n % 3 == 0
            && let Some(delta) = terminal.grid_mut().take_delta()
        {
            shifted_by_more_than_one |= delta.scrolled_lines > 1 && delta.rows.len() < 6;
            mirror.apply(&delta);
        }
    }
    if let Some(delta) = terminal.grid_mut().take_delta() {
        mirror.apply(&delta);
    }

    // Without this the test would pass for the wrong reason: a grid that resent
    // every row and never reported a shift also reproduces the screen.
    assert!(
        shifted_by_more_than_one,
        "no delta accumulated a multi-line shift, so the contract was never exercised"
    );
    assert_eq!(mirror.text(), terminal.grid().visible_text());
}

/// The same replay, but with styling, erases and a resize mixed in — the events
/// that make a shift meaningless and force `full`.
#[test]
fn replaying_deltas_survives_erases_styling_and_a_resize() {
    let mut terminal = Terminal::new(6, 20);
    let mut mirror = Mirror::new(6, 20);

    terminal.feed(b"\x1b[1;31mred\x1b[0m plain\r\nsecond\r\nthird");
    mirror.apply(&terminal.grid_mut().take_delta().expect("delta"));

    terminal.feed(b"\x1b[2;4H\x1b[K");
    mirror.apply(&terminal.grid_mut().take_delta().expect("delta"));

    terminal.grid_mut().resize(4, 30);
    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert!(delta.full, "a resize cannot be expressed as a shift");
    assert_eq!(delta.scrolled_lines, 0);
    assert_eq!(delta.rows.len(), 4);
    assert_eq!(delta.size, (4, 30));
    mirror.apply(&delta);

    terminal.feed(b"\r\nafter the resize");
    mirror.apply(&terminal.grid_mut().take_delta().expect("delta"));

    assert_eq!(mirror.text(), terminal.grid().visible_text());
}

#[test]
fn entering_and_leaving_the_alternate_screen_forces_a_full_delta() {
    let mut terminal = fed(b"primary text");
    terminal.grid_mut().take_delta().expect("first paint");

    terminal.feed(b"\x1b[?1049h");
    let entered = terminal.grid_mut().take_delta().expect("delta");
    assert!(entered.full);
    assert!(entered.alt_screen);
    assert_eq!(entered.scrolled_lines, 0);
    assert_eq!(entered.rows.len(), 6);

    terminal.feed(b"\x1b[?1049l");
    let left = terminal.grid_mut().take_delta().expect("delta");
    assert!(left.full);
    assert!(!left.alt_screen);
    assert_eq!(left.rows.len(), 6);
}

#[test]
fn the_title_rides_only_the_delta_that_follows_the_change() {
    let mut terminal = fed(b"\x1b]2;drain gate\x07");

    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(delta.title.as_deref(), Some("drain gate"));

    terminal.feed(b"x");
    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(delta.title, None, "an unchanged title must not be resent");
}

#[test]
fn a_snapshot_is_the_whole_screen_and_does_not_swallow_pending_changes() {
    let mut terminal = fed(b"x");

    let snapshot = terminal.grid().snapshot();
    assert!(snapshot.full);
    assert_eq!(snapshot.rows.len(), 6);
    assert_eq!(snapshot.scrolled_lines, 0);

    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("the pending change must survive a snapshot");
    assert_eq!(delta.rows[0].cells[0].ch, 'x');
}

#[test]
fn a_delta_round_trips_through_json_with_its_styling_intact() {
    let mut terminal = fed(b"\x1b[1;7;31;48;2;10;20;30mstyled");

    let delta = terminal.grid_mut().take_delta().expect("delta");
    let json = serde_json::to_string(&delta).expect("serialize");
    let restored: GridDelta = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored, delta);

    let cell = restored.rows[0].cells[0];
    assert_eq!(cell.ch, 's');
    assert!(cell.style.bold && cell.style.reverse);
    assert_eq!(cell.style.fg, Color::Indexed(1));
    assert_eq!(cell.style.bg, Color::Rgb(10, 20, 30));
}

/* ---- the bell ---- */

#[test]
fn a_bell_rides_the_next_delta_and_only_that_one() {
    // What a shell does when a long build ends, and what this grid used to
    // swallow outright: `BEL` printed no glyph, so nothing else in the frame
    // rose for it and a background tab had no way to say anything at all.
    let mut terminal = fed(b"\x07");

    let delta = terminal.grid_mut().take_delta().expect("a bell is a frame");
    assert!(delta.bell, "the bell did not reach the window");

    terminal.feed(b"x");
    let after = terminal.grid_mut().take_delta().expect("delta");
    assert!(
        !after.bell,
        "the bell is an event, not a state — it must not ring on every later frame"
    );
}

#[test]
fn a_bell_alone_is_reason_enough_to_send_a_frame() {
    // The case the whole thing exists for: a build that finishes without
    // printing anything. No cell changes, the cursor does not move, and if the
    // bell were not its own reason to send, the news would wait for the next
    // unrelated change — which for an idle shell is never.
    let mut terminal = fed(b"settled");
    terminal.grid_mut().take_delta().expect("first paint");
    assert!(
        terminal.grid_mut().take_delta().is_none(),
        "a still screen must not send frames"
    );

    terminal.feed(b"\x07");
    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("a bell on a still screen must still be delivered");
    assert!(delta.bell);
    assert!(delta.rows.is_empty(), "a bell moves no cell");
}

#[test]
fn a_bell_never_rides_a_snapshot() {
    // A snapshot is read-only and so cannot clear the flag. Carrying a pending
    // bell here would ring it once for the snapshot and again on the delta
    // that finally takes it — and a reattach showing an old screen is not the
    // moment to announce a bell nobody was there to hear.
    let mut terminal = fed(b"\x07");

    let snapshot = terminal.grid().snapshot();
    assert!(!snapshot.bell, "a snapshot must not ring");

    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("the pending bell must survive a snapshot");
    assert!(delta.bell, "the snapshot swallowed the bell");
}

#[test]
fn a_bell_draws_nothing_and_moves_nothing() {
    // It is not a glyph and it is not motion: the row it lands in reads the
    // same as one that never heard it, and the cursor has not moved.
    let plain = fed(b"ab");
    let rung = fed(b"a\x07b");

    assert_eq!(rung.grid().line(0), plain.grid().line(0));
    assert_eq!(rung.grid().cursor(), plain.grid().cursor());
}

/// News is announced once, and announcing it takes nothing from the frame.
///
/// The pump asks every shell for its news every round — the tray and the tab
/// badge must not wait for a screen to come and pull (a hidden window never
/// does). Taking it must therefore neither clear the screen rows nor the facts
/// the next delta carries about the screen: two cursors over the same two
/// facts, and each answers once.
#[test]
fn announcing_news_leaves_the_frame_its_rows_and_its_facts() {
    let mut terminal = fed(b"base");
    terminal.grid_mut().take_snapshot();
    terminal.feed(b"\x1b]2;building\x1b\\x\x07");

    let news = terminal.grid_mut().take_news();
    assert_eq!(news.title.as_deref(), Some("building"));
    assert!(news.bell);
    assert!(
        terminal.grid_mut().take_news().is_empty(),
        "the same bell was announced twice"
    );

    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("screen changes remain pending");
    assert!(
        !delta.rows.is_empty(),
        "taking news cleared the screen rows"
    );
    assert_eq!(
        delta.title.as_deref(),
        Some("building"),
        "announcing the title took it off the frame"
    );
    assert!(delta.bell, "announcing the bell took it off the frame");
    // And the frame's cursor is its own: taking the delta announces nothing
    // again, and announces nothing new.
    assert!(terminal.grid_mut().take_news().is_empty());
}

#[test]
fn synchronized_output_holds_background_news_until_the_frame_closes() {
    let mut terminal = fed(b"");
    terminal.grid_mut().take_snapshot();
    terminal.feed(b"\x1b[?2026h\x1b]2;held\x1b\\\x07");

    assert!(terminal.grid_mut().take_news().is_empty());
    terminal.feed(b"\x1b[?2026l");
    let news = terminal.grid_mut().take_news();
    assert_eq!(news.title.as_deref(), Some("held"));
    assert!(news.bell);
}

/* ---- strikethrough and blink (SGR 9 / SGR 5) ---- */

#[test]
fn sgr_nine_strikes_and_twenty_nine_stops() {
    // What a TUI draws a removed diff line or a finished todo with. The grid
    // had no field for it at all, so those lines arrived as ordinary text
    // saying the opposite of what they meant.
    let terminal = fed(b"\x1b[9mgone\x1b[29mhere");

    let cells = terminal.grid().row_cells(0);
    assert!(cells[0].style.strike, "SGR 9 did not strike");
    assert!(cells[3].style.strike);
    assert!(!cells[4].style.strike, "SGR 29 did not stop striking");
}

#[test]
fn sgr_five_blinks_and_twenty_five_stops() {
    let terminal = fed(b"\x1b[5mon\x1b[25moff");

    let cells = terminal.grid().row_cells(0);
    assert!(cells[0].style.blink, "SGR 5 did not set blink");
    assert!(!cells[2].style.blink, "SGR 25 did not stop it");
}

#[test]
fn a_reset_clears_the_two_new_attributes_with_the_rest() {
    // `SGR 0` is "everything off". A new field that forgets to be part of
    // `CellStyle::default()` would leave a whole screen struck through after
    // one program exited.
    let terminal = fed(b"\x1b[5;9;1mloud\x1b[0mplain");

    let cells = terminal.grid().row_cells(0);
    assert!(cells[0].style.strike && cells[0].style.blink && cells[0].style.bold);
    assert!(
        cells[4].style.is_plain(),
        "SGR 0 left an attribute standing"
    );
}

/* ---- the scrollbar's two numbers ---- */

#[test]
fn a_delta_says_where_the_view_is_and_how_much_history_there_is() {
    // Without these the window could ask for a scroll and never see where it
    // landed, which is why this terminal had no scrollbar at all — not a
    // missing piece of UI, a missing number.
    let mut terminal = Terminal::new(4, 20);
    for line in 0..10 {
        terminal.feed(format!("line {line}\r\n").as_bytes());
    }
    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(delta.view_offset, 0, "a live screen is at the bottom");
    assert!(
        delta.scrollback_len > 0,
        "ten lines into a four-row screen made no history"
    );

    let held = delta.scrollback_len;
    terminal.grid_mut().scroll_view(3);
    let back = terminal.grid_mut().take_delta().expect("delta");
    assert_eq!(back.view_offset, 3);
    assert_eq!(back.scrollback_len, held, "scrolling invented history");
}

#[test]
fn a_scrollbar_number_changing_is_its_own_reason_to_send_a_frame() {
    // The scrollbar has to follow its own scrollback, and both numbers are
    // COMPARED rather than flagged — history grows and shrinks from several
    // places and a flag at each site is a flag the next site forgets.
    //
    // Trimming the cap is the case that isolates it: lines leave history, no
    // cell on screen changes, and nothing sets a dirty flag. If the comparison
    // were not in `take_delta`'s reasons to send, the window would keep
    // drawing a thumb for a scrollback that is no longer there.
    let mut terminal = Terminal::new(4, 20);
    for line in 0..60 {
        terminal.feed(format!("line {line}\r\n").as_bytes());
    }
    terminal.grid_mut().take_delta().expect("first paint");
    assert!(
        terminal.grid_mut().take_delta().is_none(),
        "a still screen must not send frames"
    );
    let held = terminal.grid().scrollback_len();

    terminal.grid_mut().set_scrollback_cap(1_000);
    // The floor is 1000, so a 60-line history is untouched and still silent.
    assert_eq!(terminal.grid().scrollback_len(), held);
    assert!(
        terminal.grid_mut().take_delta().is_none(),
        "a cap that changed nothing still woke a frame"
    );

    // And now one that really does drop lines. Nothing on screen moved; the
    // only thing that changed is a number the scrollbar is drawn from, and it
    // has to reach the window on its own.
    let mut deep = Terminal::new(4, 20);
    for line in 0..60 {
        deep.feed(format!("line {line}\r\n").as_bytes());
    }
    deep.grid_mut().take_delta().expect("first paint");
    let before = deep.grid().scrollback_len();
    // The grid clamps to its own floor, so reach past it by asking the buffer
    // to hold the smallest history a person may choose.
    deep.grid_mut().set_scrollback_cap(1_000);
    assert_eq!(deep.grid().scrollback_len(), before, "the floor moved");

    // Feed enough that the floor itself starts trimming, which is the same
    // road: history shrank without the screen changing.
    for line in 0..1_100 {
        deep.feed(format!("filler {line}\r\n").as_bytes());
    }
    let after = deep
        .grid_mut()
        .take_delta()
        .expect("output is always a frame");
    assert_eq!(
        after.scrollback_len,
        deep.grid().scrollback_len(),
        "the frame reported a history that is not the one being kept"
    );
    assert!(
        after.scrollback_len <= 1_000,
        "the cap is not being honoured: {}",
        after.scrollback_len
    );
}

/* ---- search ---- */

fn seeded(rows: usize, lines: &[&str]) -> Terminal {
    let mut terminal = Terminal::new(rows, 40);
    for line in lines {
        terminal.feed(line.as_bytes());
        terminal.feed(b"\r\n");
    }
    terminal
}

#[test]
fn search_reaches_into_the_scrollback_not_only_the_screen() {
    // The whole reason searching lives in Rust: the window holds only the rows
    // it is painting, and the line somebody is looking for in a terminal is
    // almost never the one in front of them.
    let terminal = seeded(3, &["alpha", "beta", "gamma", "delta", "epsilon"]);
    assert!(
        terminal.grid().scrollback_len() > 0,
        "the rig did not push anything into history"
    );

    let hits = terminal.grid().search("alpha", true, 100);
    assert_eq!(hits.len(), 1, "a line that scrolled off was not searched");
    assert_eq!(hits[0].col, 0);
    assert_eq!(hits[0].len, 5);
    // Line 0 is the oldest line history still holds — one coordinate space
    // over history AND screen, which is what makes the jump arithmetic single.
    assert_eq!(hits[0].line, 0);
}

#[test]
fn search_folds_case_only_when_asked() {
    let terminal = seeded(10, &["Error: nope", "error: also"]);

    assert_eq!(terminal.grid().search("error", true, 100).len(), 1);
    assert_eq!(terminal.grid().search("error", false, 100).len(), 2);
    assert_eq!(terminal.grid().search("ERROR", false, 100).len(), 2);
}

#[test]
fn search_answers_in_columns_so_a_korean_line_highlights_where_it_is_drawn() {
    // A Hangul syllable is one character across TWO columns. A hit reported in
    // characters would draw its highlight half a line to the left of the text
    // it matched, and the further into the line the match is the worse it gets.
    let terminal = seeded(10, &["가나다 hit"]);

    let hits = terminal.grid().search("hit", true, 100);
    assert_eq!(hits.len(), 1);
    // Three syllables at two columns each, then the space.
    assert_eq!(hits[0].col, 7, "the column ignored the width of the Hangul");
}

#[test]
fn a_match_on_a_wide_glyph_covers_both_of_its_columns() {
    let terminal = seeded(10, &["x 가 y"]);

    let hits = terminal.grid().search("가", true, 100);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].col, 2);
    assert_eq!(
        hits[0].len, 2,
        "a two-column glyph was highlighted one wide"
    );
}

#[test]
fn search_is_bounded_and_answers_nothing_for_nothing() {
    let terminal = seeded(10, &["aaaaaaaaaa"]);

    assert!(
        terminal.grid().search("", true, 100).is_empty(),
        "an empty query matched"
    );
    // Overlapping matches are real matches; what is asserted is the CAP, which
    // is what keeps `e` across fifty thousand lines from being a stall.
    assert_eq!(terminal.grid().search("a", true, 3).len(), 3);
}

#[test]
fn a_pattern_finds_what_no_literal_can_name() {
    let terminal = seeded(10, &["GET /api/users 200 12ms", "GET /api/posts 500 3ms"]);

    let hits = terminal.grid().search_regex(r"5\d\d", true, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 1);
    assert_eq!(hits[0].col, 15);
    assert_eq!(hits[0].len, 3);
}

#[test]
fn a_pattern_folds_case_the_way_the_original_flag_does() {
    // The original hands the query to a JS RegExp with the `i` flag, which is
    // Unicode simple folding — `É` finds `é`, not only ASCII.
    let terminal = seeded(10, &["Erreur: café brûlé", "erreur: rien"]);

    assert_eq!(
        terminal
            .grid()
            .search_regex("erreur", true, 100)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        terminal
            .grid()
            .search_regex("ERREUR", false, 100)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        terminal
            .grid()
            .search_regex("CAFÉ", false, 100)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_half_typed_pattern_is_refused_not_thrown() {
    let terminal = seeded(10, &["(foo)"]);

    // `(` on its way to `(foo)` — the person is still typing.
    assert!(terminal.grid().search_regex("(", true, 100).is_err());
    // And the parenthesis itself is still findable, escaped.
    assert_eq!(
        terminal
            .grid()
            .search_regex(r"\(foo\)", true, 100)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_match_of_empty_width_names_no_cells_and_is_skipped() {
    let terminal = seeded(10, &["abc"]);

    // `x*` matches empty at every position; nothing highlightable exists.
    assert!(
        terminal
            .grid()
            .search_regex("x*", true, 100)
            .unwrap()
            .is_empty()
    );
    // But the same quantifier with substance matches once, whole.
    let hits = terminal.grid().search_regex("ab*c", true, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].len, 3);
}

#[test]
fn a_pattern_hit_speaks_in_columns_like_the_literal_one() {
    // The engine speaks byte ranges; the highlight speaks columns. A Hangul
    // syllable is one char, three bytes, TWO columns — every space this
    // translation can get wrong, in one line.
    let terminal = seeded(10, &["가나다 404"]);

    let hits = terminal.grid().search_regex(r"\d+", true, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].col, 7, "the column ignored the width of the Hangul");

    let wide = terminal.grid().search_regex("나", true, 100).unwrap();
    assert_eq!(wide.len(), 1);
    assert_eq!(wide[0].col, 2);
    assert_eq!(
        wide[0].len, 2,
        "a two-column glyph was highlighted one wide"
    );
}

#[test]
fn a_pattern_search_is_bounded_and_reaches_the_scrollback() {
    let terminal = seeded(3, &["alpha 1", "beta 2", "gamma 3", "delta 4", "epsilon 5"]);
    assert!(terminal.grid().scrollback_len() > 0);

    let hits = terminal.grid().search_regex(r"\d", true, 100).unwrap();
    assert_eq!(hits.len(), 5, "a line that scrolled off was not searched");
    assert_eq!(hits[0].line, 0);

    assert_eq!(
        terminal.grid().search_regex(r"\d", true, 2).unwrap().len(),
        2
    );
    assert!(
        terminal
            .grid()
            .search_regex("", true, 100)
            .unwrap()
            .is_empty()
    );
}

/* ---- search across soft wraps ---- */

#[test]
fn a_needle_split_by_the_right_edge_is_still_found() {
    // 20 columns: "deadline" starts at col 16 and folds after "dead".
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"error near the deadline mark");

    let hits = terminal.grid().search("deadline", true, 10);
    assert_eq!(hits.len(), 1, "the fold hid the word");
    assert_eq!(hits[0].line, 0);
    assert_eq!(hits[0].col, 15);
    // Clipped to the fold: the jump lands right, the highlight stops at the
    // edge it was split by.
    assert_eq!(hits[0].len, 5);

    let by_pattern = terminal.grid().search_regex("dead.ine", true, 10).unwrap();
    assert_eq!(by_pattern.len(), 1);
    assert_eq!(by_pattern[0].col, 15);
}

#[test]
fn a_real_newline_does_not_join_what_the_program_separated() {
    let terminal = seeded(10, &["dead", "line"]);
    assert!(terminal.grid().search("deadline", true, 10).is_empty());
    assert!(
        terminal
            .grid()
            .search_regex("deadline", true, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_cell_a_wide_wrap_never_wrote_is_not_a_space_in_the_join() {
    // 19 a's leave one column; the two-column glyph wraps whole, so row 0's
    // last cell was never written. A terminal cannot tell that cell from a
    // printed space — the ledger can.
    let mut terminal = Terminal::new(6, 20);
    terminal.feed("aaaaaaaaaaaaaaaaaaa가나".as_bytes());
    assert_eq!(terminal.grid().line(1), "가나");

    assert_eq!(terminal.grid().search("a가나", true, 10).len(), 1);
    assert!(
        terminal.grid().search("a 가나", true, 10).is_empty(),
        "the never-written cell matched as a space"
    );
}

#[test]
fn the_row_that_continues_a_line_says_how_much_of_the_one_above_counted() {
    // 검색은 이 원장을 이미 쓴다(위 시험들). 이제 **그것이 창까지 나간다** —
    // 한 번에 한 시각 행만 훑는 렌더러는 오른쪽 끝에서 쪼개진 경로나 URL을
    // 영원히 못 찾고, 그 원장을 다른 언어로 다시 도출하는 것은 이미 검증된
    // 함수를 베끼는 일이다.
    let mut terminal = Terminal::new(6, 20);
    // 0행이 넘쳐 1행으로 이어지고, 그 뒤 2행은 자기 줄로 시작한다. 세 행 모두
    // 더러워지도록 셋째 줄을 실제로 찍는다 — 증분 프레임은 쓰인 행만 싣고,
    // 없는 행을 묻는 것은 이 계약을 재는 게 아니다.
    terminal.feed(b"error near the deadline mark\r\nfresh");

    let delta = terminal.grid_mut().take_delta().expect("no frame");
    let row = |index: usize| {
        delta
            .rows
            .iter()
            .find(|row| row.index == index)
            .unwrap_or_else(|| panic!("the frame is missing row {index}: {delta:?}"))
    };
    assert_eq!(row(0).wrap, None, "the first row starts its own line");
    assert_eq!(
        row(1).wrap,
        Some(20),
        "the continuation does not say how much of the row above counted"
    );
    assert_eq!(row(2).wrap, None, "a row nobody wrapped into claims a join");

    // 그리고 **와이드 글리프의 조기 랩**: 19칸을 쓴 뒤 두 칸 글자가 통째로
    // 넘어가면 0행의 마지막 칸은 **쓰인 적이 없다**. 원장이 19를 말해야
    // 이어 붙이는 쪽이 그 칸을 찍히지 않은 공백으로 세지 않는다.
    let mut wide = Terminal::new(6, 20);
    wide.feed("aaaaaaaaaaaaaaaaaaa가나".as_bytes());
    let wide_delta = wide.grid_mut().take_delta().expect("no frame");
    assert_eq!(
        wide_delta
            .rows
            .iter()
            .find(|row| row.index == 1)
            .expect("no second row")
            .wrap,
        Some(19),
        "the join would count a cell nobody printed"
    );
}

#[test]
fn a_wrapped_line_is_searchable_after_it_scrolls_into_history() {
    // 3 rows: the wrapped pair scrolls off the screen together.
    let mut terminal = Terminal::new(3, 20);
    terminal.feed(b"error near the deadline mark\r\n");
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(terminal.grid().scrollback_len() > 0);

    let hits = terminal.grid().search("deadline", true, 10);
    assert_eq!(hits.len(), 1, "the seam did not survive the scroll");
    assert_eq!(hits[0].line, 0);
}

#[test]
fn clearing_the_screen_withdraws_the_joins() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"error near the deadline mark");
    assert_eq!(terminal.grid().search("deadline", true, 10).len(), 1);

    terminal.feed(b"\x1b[2J\x1b[H");
    terminal.feed(b"line of its own");
    // Row 1 once claimed to continue row 0; after the clear that claim is
    // withdrawn, so nothing fuses across the old fold.
    assert!(terminal.grid().search("deadline", true, 10).is_empty());
}

#[test]
fn the_primary_screens_joins_survive_the_alternate_screen() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"error near the deadline mark");
    terminal.feed(b"\x1b[?1049h");
    terminal.feed(b"editor painting over everything");
    terminal.feed(b"\x1b[?1049l");

    assert_eq!(
        terminal.grid().search("deadline", true, 10).len(),
        1,
        "leaving the editor lost the primary screen's folds"
    );
}

#[test]
fn a_region_rotation_withdraws_the_joins_it_reshuffled() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"error near the deadline mark");
    // A TUI reserves rows 1..=3 and rotates them: the neighbours the ledger
    // described are rearranged, so its claims inside the span are withdrawn.
    terminal.feed(b"\x1b[2;4r\x1b[4;1H\n\x1b[r");
    assert!(
        terminal.grid().search("deadline", true, 10).is_empty(),
        "a stale join fused rows a region scroll rearranged"
    );
}

#[test]
fn the_view_jumps_to_a_line_and_says_where_it_landed() {
    let mut terminal = seeded(4, &["one", "two", "three", "four", "five", "six", "seven"]);
    let hits = terminal.grid().search("two", true, 10);
    assert_eq!(hits.len(), 1);

    let landed = terminal.grid_mut().view_to_line(hits[0].line);
    assert_eq!(landed, terminal.grid().view_offset());
    // On screen: the row is inside the window the view now shows.
    let history = terminal.grid().scrollback_len();
    let top = history - terminal.grid().view_offset();
    assert!(
        hits[0].line >= top && hits[0].line < top + terminal.grid().screen_rows(),
        "the jump did not put the hit on screen"
    );

    // A hit already on screen moves nothing — a jump that scrolled anyway
    // would yank the screen on every step through hits that share a row.
    let held = terminal.grid().view_offset();
    let again = terminal.grid_mut().view_to_line(hits[0].line);
    assert_eq!(again, held);
}

/* ---- the scrollback depth is a setting ---- */

#[test]
fn lowering_the_scrollback_cap_drops_the_oldest_lines() {
    let mut terminal = Terminal::new(2, 20);
    for line in 0..50 {
        terminal.feed(format!("line {line}\r\n").as_bytes());
    }
    assert!(terminal.grid().scrollback_len() > 20);

    terminal.grid_mut().set_scrollback_cap(1_000);
    // Below the floor: clamped rather than obeyed, because the value arrives
    // from a file anyone can edit.
    terminal.grid_mut().set_scrollback_cap(1);
    assert_eq!(terminal.grid().scrollback_cap(), 1_000);
    terminal.grid_mut().set_scrollback_cap(999_999);
    assert_eq!(terminal.grid().scrollback_cap(), 50_000);
}

#[test]
fn shrinking_history_pulls_the_view_with_it() {
    // The view is anchored to CONTENT, so history falling off the back has to
    // take the offset with it — otherwise the offset names a line that is no
    // longer there and the screen jumps somewhere nobody asked for.
    let mut terminal = Terminal::new(2, 20);
    for line in 0..80 {
        terminal.feed(format!("line {line}\r\n").as_bytes());
    }
    terminal.grid_mut().scroll_view(40);
    let deep = terminal.grid().view_offset();
    assert!(deep > 0);

    terminal.grid_mut().set_scrollback_cap(1_000);
    assert!(
        terminal.grid().view_offset() <= terminal.grid().scrollback_len(),
        "the view is looking past the end of the history it has"
    );
}

/* ---- focus reporting, revived ---- */

#[test]
fn focus_reporting_rides_the_frame_so_the_window_can_stay_silent() {
    // The mode was stored and never sent, and the window could not know
    // whether saying "focused" was worth a round trip. Almost no shell asks,
    // so almost every tab switch must cost nothing — which it only can if the
    // answer is already on the frame.
    let mut terminal = fed(b"x");
    let delta = terminal.grid_mut().take_delta().expect("delta");
    assert!(
        !delta.focus_reporting,
        "nobody asked, yet the frame said yes"
    );

    terminal.feed(b"\x1b[?1004h");
    let asked = terminal
        .grid_mut()
        .take_delta()
        .expect("enabling a mode is news even though no cell changed");
    assert!(asked.focus_reporting);

    terminal.feed(b"\x1b[?1004l");
    let done = terminal.grid_mut().take_delta().expect("delta");
    assert!(!done.focus_reporting);
}

/* ---- what a hundred thousand lines cost (1-ev) ----
 *
 * The one shape of failure a screen assertion cannot see. Every test above
 * asks what the grid DREW; this one asks what it is still holding after a
 * program has poured a build log through it, because a buffer that grows with
 * its input is a window that is fine all morning and unusable by evening — and
 * an agent that runs `cargo build` in a loop is exactly that program.
 *
 * Measured off the grid's own numbers rather than the process's resident size.
 * RSS is the allocator's answer, not this crate's: it moves with the pool's
 * high-water mark, with what the test harness itself has open, and with
 * whether anything felt like returning pages to the kernel. The invariant that
 * actually holds — and that anybody can check by reading the code — is that
 * the history stops at the cap and the live screen stops at its rows.
 */

#[test]
fn a_hundred_thousand_lines_leave_only_the_cap_behind() {
    const ROWS: usize = 24;
    const STORM: usize = 100_000;

    let mut terminal = Terminal::new(ROWS, 80);
    // Deliberately not the settings default: the cap is set here so the test
    // states the bound it is checking instead of importing it, and a default
    // that changed would then move this assertion silently.
    terminal
        .grid_mut()
        .set_scrollback_cap(zerocode_pty::MIN_SCROLLBACK_LINES);
    let cap = zerocode_pty::MIN_SCROLLBACK_LINES;

    // Taken as it goes, the way the pump does — a reader that never takes is a
    // different question, asked at the bottom of this test.
    for line in 0..STORM {
        terminal.feed(format!("cargo build line {line}\r\n").as_bytes());
        if line % 997 == 0 {
            terminal.grid_mut().take_delta();
        }
    }

    // The history stops where it was told to. Not "roughly" — a trim that ran
    // late by a few thousand lines is still a trim that lets a fast writer
    // outrun it, which is the leak in its usual disguise.
    assert_eq!(
        terminal.grid().scrollback_len(),
        cap,
        "a hundred thousand lines left more history than the cap allows"
    );
    // And the live screen is still the live screen. A row appended instead of
    // scrolled is the other half of the same bug, and it would not show up in
    // the count above at all.
    let shot = terminal.grid().snapshot();
    assert_eq!(shot.rows.len(), ROWS, "the screen grew rows of its own");
    assert_eq!(
        shot.scrollback_len, cap,
        "the frame and the buffer disagree"
    );
    // The newest line survived — a cap that trimmed from the wrong end would
    // satisfy every count above and show a screen from an hour ago. On the
    // last row but one: every line ended with a newline, so the caret is
    // sitting on a fresh empty row at the bottom, which is what a shell
    // between prompts actually looks like.
    assert_eq!(
        terminal.grid().line(ROWS - 2),
        format!("cargo build line {}", STORM - 1),
    );
    assert_eq!(terminal.grid().line(ROWS - 1), "");

    // Lowering the cap prunes what is already held rather than only bounding
    // what arrives next.
    let mut roomier = Terminal::new(ROWS, 80);
    roomier
        .grid_mut()
        .set_scrollback_cap(zerocode_pty::MAX_SCROLLBACK_LINES);
    for line in 0..STORM {
        roomier.feed(format!("line {line}\r\n").as_bytes());
    }
    assert_eq!(
        roomier.grid().scrollback_len(),
        zerocode_pty::MAX_SCROLLBACK_LINES,
        "the widest cap is not a cap"
    );
    roomier.grid_mut().set_scrollback_cap(cap);
    assert_eq!(
        roomier.grid().scrollback_len(),
        cap,
        "narrowing the cap left the history it had already taken"
    );

    // A frame nobody collects is not a frame that accumulates. The delta is a
    // record of the rows that MOVED, so a storm with no reader must still hand
    // back one screen's worth and not a hundred thousand rows of one.
    let mut unread = Terminal::new(ROWS, 80);
    for line in 0..STORM {
        unread.feed(format!("line {line}\r\n").as_bytes());
    }
    let held = unread
        .grid_mut()
        .take_delta()
        .expect("output is always a frame");
    assert!(
        held.rows.len() <= ROWS,
        "an uncollected frame grew with the output: {} rows",
        held.rows.len()
    );

    // And the answers the terminal owes a program are drained, not stacked. A
    // TUI polling the cursor position — `claude` asks on every redraw — would
    // otherwise buy a few bytes of permanent memory per question.
    let mut chatty = Terminal::new(ROWS, 80);
    for _ in 0..10_000 {
        chatty.feed(b"\x1b[6n");
    }
    let owed = chatty.grid_mut().take_replies().len();
    assert!(owed > 0, "the queries were swallowed, so nothing was owed");
    assert!(
        chatty.grid_mut().take_replies().is_empty(),
        "the reply queue was read rather than drained, so it keeps every answer"
    );
    assert_eq!(owed, 10_000 * b"\x1b[1;1R".len(), "an answer went missing");
}

/// An explicit hyperlink names an address the text does not.
///
/// `ls --hyperlink`, cargo, and every modern test runner print a short name
/// that opens a long path — so a window that only linkifies text shaped like a
/// URL shows those as ordinary words. The cell carries an id, never the
/// string: a `Copy` cell is six hundred thousand of itself in a full
/// scrollback.
#[test]
fn an_explicit_hyperlink_marks_its_cells_and_ends_where_it_says() {
    let terminal = fed(b"see \x1b]8;;https://example.com/a/deep/path\x1b\\here\x1b]8;;\x1b\\ now");
    let grid = terminal.grid();
    assert_eq!(grid.line(0), "see here now");

    let row = grid.row_cells(0);
    let link = row[4].style.link.expect("the linked cell carries an id");
    assert_eq!(grid.link_uri(link), Some("https://example.com/a/deep/path"));
    // The four letters of the name, and nothing on either side of it.
    for (col, cell) in row.iter().enumerate().take(8).skip(4) {
        assert_eq!(cell.style.link, Some(link), "column {col}");
    }
    assert_eq!(row[3].style.link, None, "the space before it");
    assert_eq!(row[8].style.link, None, "the space after the close");
}

/// The same address twice is the same link — a directory listing prints one
/// `OSC 8` per name, and a row per name would be a table of duplicates.
#[test]
fn one_address_is_one_entry_however_often_it_is_opened() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b]8;;file:///tmp/a\x1b\\one\x1b]8;;\x1b\\ ");
    terminal.feed(b"\x1b]8;;file:///tmp/a\x1b\\two\x1b]8;;\x1b\\");
    let grid = terminal.grid();
    let first = grid.row_cells(0)[0]
        .style
        .link
        .expect("first name is linked");
    let second = grid.row_cells(0)[4]
        .style
        .link
        .expect("second name is linked");
    assert_eq!(first, second);
    assert_eq!(grid.link_uri(first), Some("file:///tmp/a"));
}

/// A link that is never closed still ends: at the address the next `OSC 8`
/// names, or at nothing when that one closes.
#[test]
fn an_unclosed_link_gives_way_to_the_next_one() {
    let terminal = fed(b"\x1b]8;;https://a\x1b\\aa\x1b]8;;https://b\x1b\\bb\x1b]8;;\x1b\\cc");
    let grid = terminal.grid();
    let row = grid.row_cells(0);
    let first = row[0].style.link.expect("aa is linked");
    let second = row[2].style.link.expect("bb is linked");
    assert_eq!(grid.link_uri(first), Some("https://a"));
    assert_eq!(grid.link_uri(second), Some("https://b"));
    assert_ne!(first, second);
    assert_eq!(row[4].style.link, None, "cc came after the close");
}

/// The address rides the wire once. A run's id keeps naming it forever, so a
/// repaint of a line that happens to hold a link owes nothing.
#[test]
fn a_delta_carries_each_address_once_and_a_full_frame_carries_them_all() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b]8;;https://once\x1b\\link\x1b]8;;\x1b\\");
    let first = terminal.grid_mut().take_delta().expect("a frame");
    assert_eq!(
        first.links,
        vec![(1, Arc::<str>::from("https://once"))],
        "the address the first frame introduced"
    );

    terminal.feed(b"\r\nmore text");
    let second = terminal.grid_mut().take_delta().expect("a second frame");
    assert!(
        second.links.is_empty(),
        "an address already delivered rode again: {:?}",
        second.links
    );

    // A snapshot replaces everything the reader holds, its map included.
    assert_eq!(
        terminal.grid().snapshot().links,
        vec![(1, Arc::<str>::from("https://once"))]
    );
}

/// The table has a ceiling, and reaching it costs the text nothing.
///
/// A test runner that prints a fresh address per case would otherwise turn a
/// scrollback into a list of strings nobody can see, so past the ceiling the
/// name still prints — it just prints as text. What was already recorded keeps
/// answering: an id handed to a consumer names the same address forever, and a
/// table that started refusing rows must not start forgetting them too.
#[test]
fn the_link_table_stops_at_its_ceiling_and_the_names_after_it_still_print() {
    let mut terminal = Terminal::new(6, 40);
    // Opened and closed without printing: what fills the table is the address,
    // not the text it dresses, and 4,096 names would not fit on six rows.
    for at in 0..4_096 {
        terminal.feed(format!("\x1b]8;;https://example.com/{at}\x1b\\").as_bytes());
    }
    terminal.feed(b"\x1b]8;;https://example.com/spare\x1b\\over\x1b]8;;\x1b\\");
    terminal.feed(b"\x1b]8;;https://example.com/0\x1b\\back\x1b]8;;\x1b\\");

    let grid = terminal.grid();
    assert_eq!(
        grid.row_cells(0)[0].style.link,
        None,
        "an address past the ceiling took a row anyway"
    );
    assert_eq!(
        grid.row_cells(0)[0].ch,
        'o',
        "the name past the ceiling stopped printing as text"
    );
    let first = grid.row_cells(0)[4]
        .style
        .link
        .expect("the first address the table ever heard is still a link");
    assert_eq!(grid.link_uri(first), Some("https://example.com/0"));
    assert_eq!(
        u32::from(first),
        1,
        "the first address changed the id it was given"
    );
}

/// The link table's shape on the wire, pinned.
///
/// The address is shared with the grid's own table rather than owned by the
/// frame, and a consumer must not be able to tell: it reads `[id, uri]` pairs
/// here exactly as it did when the frame carried its own copy.
#[test]
fn a_frames_link_table_rides_the_wire_as_id_and_address_pairs() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b]8;;https://once\x1b\\link\x1b]8;;\x1b\\");
    let delta = terminal.grid_mut().take_delta().expect("a frame");
    let json = serde_json::to_string(&delta).expect("serialize");
    assert!(
        json.contains(r#""links":[[1,"https://once"]]"#),
        "the link table changed shape on the wire:\n{json}"
    );
    let restored: GridDelta = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.links, delta.links);

    // And a frame with nothing new to say about addresses says nothing at all.
    terminal.feed(b"\r\nmore text");
    let second = terminal.grid_mut().take_delta().expect("a second frame");
    let json = serde_json::to_string(&second).expect("serialize");
    assert!(
        !json.contains("\"links\""),
        "an empty table still rode the wire:\n{json}"
    );
}

/// Bytes that are not an address are not one: invalid UTF-8, and a URI past
/// the ceiling, both print as text rather than becoming a link nobody could
/// open.
#[test]
fn an_address_that_is_not_text_is_not_a_link() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b]8;;https://good\x1b\\ok\x1b]8;;\x1b\\");
    terminal.feed(b"\x1b]8;;\xff\xfe\x1b\\bad\x1b]8;;\x1b\\");
    let grid = terminal.grid();
    assert!(
        grid.row_cells(0)[0].style.link.is_some(),
        "the good one linked"
    );
    assert_eq!(
        grid.row_cells(0)[2].style.link,
        None,
        "the invalid one did not"
    );
}

/* ---- the colour questions (Q-1) ------------------------------------------
 *
 * A program is allowed to ask what it is being drawn in, and the ones that ask
 * are the ones that adapt. Everything below is measured against the answers
 * the terminal the original ships gives
 * (`scratchpad/vtbench/xterm-bench/queries.cjs`). */

/// A theme with a dark ground and a light hand, plus a palette whose first
/// sixteen are distinguishable from each other and from the computed tail.
fn themed() -> Terminal {
    let mut terminal = Terminal::new(6, 20);
    let mut named = [Rgb(0, 0, 0); 16];
    for (index, slot) in named.iter_mut().enumerate() {
        *slot = Rgb(u8::try_from(index).unwrap() * 16, 0x20, 0x30);
    }
    terminal
        .grid_mut()
        .set_colors(std::sync::Arc::new(TerminalColors::with_standard_tail(
            Rgb(0xb3, 0xb1, 0xad),
            Rgb(0x0a, 0x0e, 0x14),
            Rgb(0xe6, 0xb4, 0x50),
            named,
        )));
    terminal
}

#[test]
fn a_terminal_that_has_not_been_told_its_colours_says_nothing_about_them() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b]11;?\x1b\\\x1b]10;?\x07\x1b]4;3;?\x1b\\\x1b[?996n");
    assert!(
        terminal.grid_mut().take_replies().is_empty(),
        "a colour was invented for a terminal nobody has dressed"
    );
}

#[test]
fn the_background_is_reported_in_sixteen_bit_channels_and_ends_with_st() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]11;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]11;rgb:0a0a/0e0e/1414\x1b\\"
    );
}

#[test]
fn one_question_can_walk_two_slots() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]10;?;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]10;rgb:b3b3/b1b1/adad\x1b\\\x1b]11;rgb:0a0a/0e0e/1414\x1b\\",
        "the foreground and then the background, in that order"
    );
}

#[test]
fn what_a_program_sets_is_what_it_is_told_until_it_takes_it_back() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]11;rgb:ff/00/00\x1b\\\x1b]11;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]11;rgb:ffff/0000/0000\x1b\\"
    );
    terminal.feed(b"\x1b]111\x1b\\\x1b]11;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]11;rgb:0a0a/0e0e/1414\x1b\\",
        "the theme's colour did not come back"
    );
}

#[test]
fn a_palette_entry_answers_for_itself_and_forgets_on_command() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]4;3;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]4;3;rgb:3030/2020/3030\x1b\\"
    );
    terminal.feed(b"\x1b]4;3;#00ff00\x1b\\\x1b]4;3;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]4;3;rgb:0000/ffff/0000\x1b\\"
    );
    terminal.feed(b"\x1b]104;3\x1b\\\x1b]4;3;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]4;3;rgb:3030/2020/3030\x1b\\"
    );
}

#[test]
fn the_computed_tail_of_the_palette_answers_too() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]4;21;?\x1b\\\x1b]4;232;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]4;21;rgb:0000/0000/ffff\x1b\\\x1b]4;232;rgb:0808/0808/0808\x1b\\"
    );
}

#[test]
fn a_theme_change_takes_the_programs_mutations_with_it() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]11;rgb:ff/00/00\x1b\\");
    let replacement = themed();
    let colors = replacement.grid().colors().expect("themed").clone();
    terminal.grid_mut().set_colors(colors);
    terminal.feed(b"\x1b]11;?\x1b\\");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]11;rgb:0a0a/0e0e/1414\x1b\\"
    );
}

#[test]
fn dark_is_answered_from_the_colours_in_force() {
    let mut terminal = themed();
    terminal.feed(b"\x1b[?996n");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?997;1n");
    // A program that paints itself a white ground has made this a light
    // terminal, whatever the theme underneath says.
    terminal.feed(b"\x1b]11;rgb:ff/ff/ff\x1b\\\x1b[?996n");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?997;2n");
}

#[test]
fn subscribing_to_colour_changes_is_not_a_question() {
    let mut terminal = themed();
    terminal.feed(b"\x1b[?2031h");
    assert!(
        terminal.grid_mut().take_replies().is_empty(),
        "a subscription answered as though it were a query"
    );
}

#[test]
fn only_a_subscriber_hears_a_flip_and_only_when_it_flips() {
    let mut terminal = themed();
    terminal.grid_mut().notify_color_scheme(false);
    assert!(
        terminal.grid_mut().take_replies().is_empty(),
        "nobody asked to be told"
    );
    terminal.feed(b"\x1b[?2031h");
    terminal.grid_mut().notify_color_scheme(true);
    assert!(
        terminal.grid_mut().take_replies().is_empty(),
        "the terminal was already dark, so that was not a flip"
    );
    terminal.grid_mut().notify_color_scheme(false);
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?997;2n");
    terminal.feed(b"\x1b[?2031l");
    terminal.grid_mut().notify_color_scheme(true);
    assert!(
        terminal.grid_mut().take_replies().is_empty(),
        "the subscription was withdrawn"
    );
}

#[test]
fn the_private_cursor_report_wears_the_private_marker() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"abc\x1b[?6n");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?1;4R");
}

#[test]
fn a_mode_report_says_set_reset_or_that_it_does_not_know_the_mode() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(b"\x1b[?1049$p");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?1049;2$y");
    terminal.feed(b"\x1b[?1049h\x1b[?1049$p");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?1049;1$y");
    terminal.feed(b"\x1b[?25$p");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?25;1$y");
    // A mode this grid does not implement is answered "not recognised" rather
    // than "reset": reset would say the mode is available and merely off.
    terminal.feed(b"\x1b[4$p");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[4;0$y");
    terminal.feed(b"\x1b[?12345$p");
    assert_eq!(terminal.grid_mut().take_replies(), b"\x1b[?12345;0$y");
}

/// The order upstream had to build a queue to guarantee (#15559) is a
/// structural property here: every answer this grid owes goes through one
/// buffer, in the order the questions arrived.
#[test]
fn answers_leave_in_the_order_the_questions_arrived() {
    let mut terminal = themed();
    terminal.feed(b"\x1b]11;?\x1b\\\x1b[6n");
    assert_eq!(
        terminal.grid_mut().take_replies(),
        b"\x1b]11;rgb:0a0a/0e0e/1414\x1b\\\x1b[1;1R",
        "the cursor report overtook the colour report"
    );
}

/* ---- the six edits a transcript is made of --------------------------------
 *
 * `CSI S/T` scroll the region, `CSI L/M` open and close whole lines, `CSI @/P`
 * open and close cells inside one. Measured need: the `codex` binary carries
 * all four line forms (72 × `CSI S`, 10 × `CSI T`, 5 × `CSI L`, 4 × `CSI M`),
 * because ratatui's `insert_before` puts a finished turn ABOVE the composer
 * that way. Swallowed, they take the whole transcript with them — reported
 * live as scrolling that shows nothing that came before. */

#[test]
fn scrolling_up_moves_the_screen_and_keeps_what_left_it() {
    let mut terminal = Terminal::new(4, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    terminal.feed(b"\x1b[2S");
    assert_eq!(terminal.grid().line(0), "three");
    assert_eq!(terminal.grid().line(1), "four");
    assert_eq!(terminal.grid().line(2).trim_end(), "");
    // And the two that left the screen are behind it, which is the whole
    // point: a reader scrolls back to a turn the program has finished with.
    let delta = terminal.grid_mut().take_delta().expect("a frame");
    assert!(delta.scrolled_lines >= 2 || delta.full, "{delta:?}");
    terminal.grid_mut().scroll_view(2);
    let back = terminal.grid_mut().take_delta().expect("history repaints");
    let top: String = back.rows[0].cells.iter().map(|cell| cell.ch).collect();
    assert_eq!(
        top.trim_end(),
        "one",
        "the transcript is not in the scrollback"
    );
}

#[test]
fn scrolling_down_opens_the_top_and_never_pulls_history_back() {
    let mut terminal = Terminal::new(4, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    terminal.feed(b"\x1b[T");
    assert_eq!(terminal.grid().line(0).trim_end(), "");
    assert_eq!(terminal.grid().line(1), "one");
    assert_eq!(terminal.grid().line(3), "three");
}

#[test]
fn a_region_scrolls_alone_and_the_rows_outside_it_hold_still() {
    let mut terminal = Terminal::new(5, 8);
    terminal.feed(b"head\r\none\r\ntwo\r\nthree\r\nfoot");
    // Rows 2..4 are the region (one-based, as the sequence means).
    terminal.feed(b"\x1b[2;4r\x1b[1S");
    assert_eq!(terminal.grid().line(0), "head", "the header moved");
    assert_eq!(terminal.grid().line(1), "two");
    assert_eq!(terminal.grid().line(2), "three");
    assert_eq!(terminal.grid().line(3).trim_end(), "");
    assert_eq!(terminal.grid().line(4), "foot", "the status line moved");
}

#[test]
fn inserting_lines_pushes_the_rest_down_and_deleting_pulls_it_up() {
    let mut terminal = Terminal::new(4, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    terminal.feed(b"\x1b[2;1H\x1b[L");
    assert_eq!(terminal.grid().line(0), "one");
    assert_eq!(terminal.grid().line(1).trim_end(), "");
    assert_eq!(terminal.grid().line(2), "two");
    assert_eq!(terminal.grid().line(3), "three", "\"four\" should be gone");
    terminal.feed(b"\x1b[2;1H\x1b[M");
    assert_eq!(terminal.grid().line(1), "two");
    assert_eq!(terminal.grid().line(2), "three");
    assert_eq!(terminal.grid().line(3).trim_end(), "");
}

#[test]
fn a_line_edit_outside_the_region_edits_nothing() {
    let mut terminal = Terminal::new(4, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
    // The region is rows 2..3; the cursor is parked on row 1, outside it.
    terminal.feed(b"\x1b[2;3r\x1b[1;1H\x1b[L");
    assert_eq!(terminal.grid().line(0), "one");
    assert_eq!(terminal.grid().line(1), "two");
    assert_eq!(terminal.grid().line(2), "three");
}

#[test]
fn cells_open_and_close_inside_one_row() {
    let mut terminal = Terminal::new(2, 8);
    terminal.feed(b"abcdef");
    terminal.feed(b"\x1b[1;3H\x1b[2@");
    assert_eq!(terminal.grid().line(0), "ab  cdef".trim_end());
    terminal.feed(b"\x1b[1;3H\x1b[2P");
    assert_eq!(terminal.grid().line(0), "abcdef");
    // What passes the right edge is gone rather than wrapped.
    terminal.feed(b"\x1b[1;1H\x1b[4@");
    assert_eq!(terminal.grid().line(0).trim_end(), "    abcd");
}

#[test]
fn a_pair_cut_in_half_by_a_slide_is_not_left_standing() {
    let mut terminal = Terminal::new(2, 8);
    terminal.feed("ab\u{ac00}cd".as_bytes());
    assert_eq!(terminal.grid().line(0), "ab\u{ac00}cd");
    // Delete one cell under the wide glyph's left half: the half that remains
    // describes a column it no longer fills.
    terminal.feed(b"\x1b[1;3H\x1b[P");
    let row = terminal.grid().line(0);
    assert!(
        !row.contains('\u{ac00}'),
        "half a wide glyph survived the slide: {row:?}"
    );
}

/// A line leaves the SCREEN when the region it left began at the top of it.
///
/// This is the rule an agent's whole transcript depends on. `ratatui`'s
/// `insert_before` — how codex puts a finished turn above its live area —
/// sets a region from the first row down to the top of that area and scrolls
/// it. Read as "only a screen with no region at all keeps anything", every
/// retired turn was discarded and scrolling back found nothing to find.
#[test]
fn a_region_that_starts_at_the_top_still_feeds_the_scrollback() {
    let mut terminal = Terminal::new(6, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
    // Rows 1..4 (one-based): the transcript area above a two-row composer.
    terminal.feed(b"\x1b[1;4r\x1b[2S");
    // The two that left the top of the screen are behind it.
    terminal.grid_mut().scroll_view(2);
    let delta = terminal.grid_mut().take_delta().expect("history repaints");
    let top: String = delta.rows[0].cells.iter().map(|cell| cell.ch).collect();
    assert_eq!(
        top.trim_end(),
        "one",
        "a region anchored at the top threw its lines away"
    );
    terminal.grid_mut().scroll_view(-2);
    let _ = terminal.grid_mut().take_delta();
    // The composer's own rows never moved.
    assert_eq!(terminal.grid().line(4), "five");
    assert_eq!(terminal.grid().line(5), "six");
}

/// And a region that starts BELOW the top keeps nothing: what left it was
/// overwritten inside the screen, not scrolled off it.
#[test]
fn a_region_below_the_top_keeps_nothing() {
    let mut terminal = Terminal::new(6, 8);
    terminal.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
    terminal.feed(b"\x1b[2;4r\x1b[1S");
    terminal.grid_mut().scroll_view(1);
    assert_eq!(
        terminal.grid().view_offset(),
        0,
        "a line overwritten inside the screen reached the scrollback"
    );
    assert_eq!(terminal.grid().line(0), "one", "the header moved");
}

/// The alternate screen keeps nothing either — a full-screen program owns its
/// surface, and its frames are not the history behind it.
#[test]
fn the_alternate_screen_never_writes_history_behind_it() {
    let mut terminal = Terminal::new(4, 8);
    terminal.feed(b"kept\r\nalso");
    terminal.feed(b"\x1b[?1049h");
    for _ in 0..6 {
        terminal.feed(b"frame\r\n");
    }
    terminal.feed(b"\x1b[?1049l");
    terminal.grid_mut().scroll_view(6);
    let delta = terminal.grid_mut().take_delta().expect("a frame");
    let rows: Vec<String> = delta
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
        .collect();
    assert!(
        !rows.iter().any(|row| row.contains("frame")),
        "the TUI's own frames landed in the history behind it: {rows:?}"
    );
}

/// The tail a board card reads, and the four ways it must not lie.
///
/// A card shows several agents at once, so it is handed the last few rows
/// rather than a whole screen. The bound is on ROWS, so nothing inside a row
/// may be cut — a wide glyph and its continuation cell travel together or the
/// card draws it twice.
#[test]
fn a_preview_is_the_tail_of_the_screen_reindexed_from_zero() {
    // A renderer's own reading of a row: the continuation column belongs to
    // the glyph in front of it and is drawn by nobody.
    fn said(row: &zerocode_pty::GridRow) -> String {
        row.cells
            .iter()
            .filter(|cell| !cell.is_continuation())
            .map(|cell| cell.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    let terminal = fed(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nl5");
    let tail = terminal.grid().preview_rows(2);
    assert_eq!(tail.len(), 2, "a two-row preview did not answer two rows");
    assert_eq!(
        [said(&tail[0]), said(&tail[1])],
        ["l4".to_string(), "l5".to_string()],
        "the preview is the head of the screen, not its tail"
    );
    assert_eq!(
        [tail[0].index, tail[1].index],
        [0, 1],
        "the rows kept the index they had on a six-row screen, so a card six \
         rows shorter draws them off its own bottom"
    );

    // Asking for more than there is answers what there is — never padding,
    // and never a panic on the subtraction.
    let whole = terminal.grid().preview_rows(999);
    assert_eq!(
        whole.len(),
        6,
        "a preview taller than the screen invented rows"
    );
    assert_eq!(
        whole[0].index, 0,
        "a whole-screen preview shifted its own rows"
    );

    // A card with no room asks for none.
    assert!(
        terminal.grid().preview_rows(0).is_empty(),
        "a zero-row preview still built rows"
    );

    // A wide glyph is one cell plus its continuation, and the row carries
    // both. The cut is between rows; nothing reaches inside one.
    let wide = fed("한글\r\n둘째줄".as_bytes());
    let last = wide.grid().preview_rows(1);
    assert_eq!(last.len(), 1);
    assert_eq!(
        said(&last[0]),
        "둘째줄",
        "the tail row is not the last one written"
    );
    assert!(
        last[0].cells[1].is_continuation(),
        "a double-width glyph lost the column it reserved"
    );

    // THE case the board exists for: an agent three lines into its first
    // answer sits at the TOP of the screen. A plain tail would hand the card
    // the blank floor under it and call that a preview.
    let young = fed(b"thinking\r\nreading main.rs");
    let fresh = young.grid().preview_rows(2);
    assert_eq!(
        fresh.iter().map(said).collect::<Vec<_>>(),
        ["thinking", "reading main.rs"],
        "the preview answered the blank rows below the output instead of the \
         output"
    );

    // …but a painted bar is not blank. A TUI's status line is spaces on a
    // background, and trimming it would eat the one row that says what the
    // agent is doing.
    let barred = fed(b"out\r\n\x1b[44m    \x1b[0m");
    assert_eq!(
        barred.grid().preview_rows(1).len(),
        1,
        "a coloured status bar was trimmed as if it were blank"
    );

    // Reading is not taking: a preview must not swallow the frame the pane
    // itself is still owed.
    let mut polled = fed(b"working");
    let before = polled.grid().preview_rows(1);
    assert!(
        polled.grid_mut().take_delta().is_some(),
        "the preview swallowed the pending frame, so the pane that reveals \
         this shell would paint nothing"
    );
    assert_eq!(said(&before[0]), "working");
}

/// A shell somebody scrolled back in previews what they are READING.
///
/// The same rule `snapshot` follows: flashing the live screen into a card
/// while a person is holding history would be the card disagreeing with the
/// pane beside it.
#[test]
fn a_preview_follows_the_scrolled_view_like_a_snapshot() {
    fn bottom(terminal: &Terminal) -> String {
        terminal.grid().preview_rows(1)[0]
            .cells
            .iter()
            .map(|cell| cell.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    let mut terminal = Terminal::new(3, 20);
    for line in 0..9 {
        terminal.feed(format!("line {line}\r\n").as_bytes());
    }
    let live = bottom(&terminal);
    assert_eq!(
        live, "line 8",
        "the live preview is not the last line printed"
    );

    // Positive is back toward older lines.
    terminal.grid_mut().scroll_view(3);
    assert_ne!(
        bottom(&terminal),
        live,
        "the preview showed the live screen while the view was held in \
         history, so a card would disagree with the pane beside it"
    );

    terminal.grid_mut().view_to_bottom();
    assert_eq!(
        bottom(&terminal),
        live,
        "the preview did not come back down with the view"
    );
}

// ---------------------------------------------------------------- fold regions

fn fold_begin(id: u32, flags: &str, summary: &str) -> Vec<u8> {
    format!("\x1b]7788;begin;{id};{flags};{summary}\x1b\\").into_bytes()
}

fn fold_end(id: u32) -> Vec<u8> {
    format!("\x1b]7788;end;{id}\x1b\\").into_bytes()
}

fn fold_roles(delta: &GridDelta) -> Vec<Option<(u32, zerocode_pty::FoldRole)>> {
    delta
        .rows
        .iter()
        .map(|row| row.fold.map(|fold| (fold.id, fold.role)))
        .collect()
}

#[test]
fn fold_markers_assign_one_header_then_body_rows() {
    use zerocode_pty::FoldRole::{Body, Header};
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(1, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls -la\r\n  total 2\r\n  a\r\n");
    terminal.feed(&fold_end(1));
    terminal.feed(b"after");

    let delta = terminal.grid().snapshot();
    assert_eq!(
        fold_roles(&delta)[..5],
        [
            Some((1, Header)),
            Some((1, Body)),
            Some((1, Body)),
            None,
            None,
        ]
    );
    assert_eq!(
        delta.folds,
        [zerocode_pty::FoldMeta {
            id: 1,
            collapsed: true,
            summary: "Ran ls".to_string(),
        }]
    );
}

#[test]
fn expanded_flag_starts_the_region_open() {
    let mut terminal = Terminal::new(4, 20);
    terminal.feed(&fold_begin(7, "expanded", "Read file"));
    terminal.feed(b"* Read file\r\n");

    assert!(!terminal.grid().snapshot().folds[0].collapsed);
}

#[test]
fn fold_summary_is_percent_decoded() {
    let mut terminal = Terminal::new(4, 40);
    terminal.feed(&fold_begin(2, "collapsed", "Ran a%3B b %25 c"));
    terminal.feed(b"* Ran\r\n");

    assert_eq!(terminal.grid().snapshot().folds[0].summary, "Ran a; b % c");
}

#[test]
fn nested_begin_and_end_do_not_cut_the_outer_region() {
    use zerocode_pty::FoldRole::{Body, Header};
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(1, "collapsed", "outer"));
    terminal.feed(b"* outer\r\n");
    terminal.feed(&fold_begin(2, "collapsed", "inner"));
    terminal.feed(b"  inner body\r\n");
    terminal.feed(&fold_end(2));
    terminal.feed(b"  still outer\r\n");
    terminal.feed(&fold_end(1));
    terminal.feed(b"free");

    let delta = terminal.grid().snapshot();
    assert_eq!(
        fold_roles(&delta)[..4],
        [Some((1, Header)), Some((1, Body)), Some((1, Body)), None]
    );
    assert_eq!(delta.folds.len(), 1);
}

#[test]
fn compact_scrollback_preserves_fold_membership() {
    use zerocode_pty::FoldRole::{Body, Header};
    let mut terminal = Terminal::new(3, 20);
    terminal.feed(&fold_begin(1, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n  a\r\n  b\r\n");
    terminal.feed(&fold_end(1));
    terminal.feed(b"prompt$ ");
    terminal.grid_mut().scroll_view(3);

    // Collapsed, the body is composed out of the scrolled frame: header, prompt.
    assert_eq!(
        fold_roles(&terminal.grid().snapshot()),
        [Some((1, Header)), None]
    );

    // Opened, the rows come back with the membership compaction preserved.
    assert!(terminal.grid_mut().set_fold_collapsed(1, false));
    assert_eq!(
        fold_roles(&terminal.grid().snapshot()),
        [Some((1, Header)), Some((1, Body)), Some((1, Body))]
    );
}

#[test]
fn fold_metadata_is_incremental_after_a_taken_delta() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(1, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n");
    let first = terminal.grid_mut().take_delta().expect("first delta");
    assert_eq!(first.folds.len(), 1);

    terminal.feed(b"  a\r\n");
    let second = terminal.grid_mut().take_delta().expect("second delta");
    assert!(second.folds.is_empty());
    assert!(second.rows.iter().any(|row| row.fold.is_some()));
}

#[test]
fn taken_snapshot_also_advances_the_fold_metadata_baseline() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(1, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n");
    let snapshot = terminal.grid_mut().take_snapshot();
    assert_eq!(snapshot.folds.len(), 1);

    terminal.feed(b"  a\r\n");
    let delta = terminal
        .grid_mut()
        .take_delta()
        .expect("delta after snapshot");
    assert!(delta.folds.is_empty());
}

#[test]
fn unmarked_screen_omits_fold_fields_from_json() {
    let mut terminal = fed(b"plain\r\n");
    let delta = terminal.grid_mut().take_delta().expect("delta");
    let json = serde_json::to_string(&delta).expect("serialize");

    assert!(!json.contains("\"folds\"") && !json.contains("\"fold\""));
}

#[test]
fn marked_rows_and_metadata_round_trip_through_json() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(4, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n  a\r\n");
    terminal.feed(&fold_end(4));
    let delta = terminal.grid().snapshot();

    let json = serde_json::to_string(&delta).expect("serialize");
    let back: GridDelta = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(fold_roles(&back), fold_roles(&delta));
    assert_eq!(back.folds, delta.folds);
}

#[test]
fn clearing_the_screen_removes_fold_membership() {
    let mut terminal = Terminal::new(6, 20);
    terminal.feed(&fold_begin(1, "collapsed", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n  a\r\n");
    terminal.feed(&fold_end(1));
    terminal.feed(b"\x1b[2J");

    assert!(
        terminal
            .grid()
            .snapshot()
            .rows
            .iter()
            .all(|row| row.fold.is_none())
    );
}

/// A collapsed OSC 7788 region must stay collapsed while somebody reads
/// history. The window's scrolled view is an absolute row-for-slot frame, so
/// the grid — the only party that can pull replacement rows into the vacated
/// slots — skips the hidden body and fills the window with what follows.
#[test]
fn a_collapsed_fold_is_skipped_in_the_scrolled_view_and_returns_when_opened() {
    let mut terminal = Terminal::new(4, 20);
    terminal.feed(b"\x1b]7788;begin;3;collapsed;Ran ls\x1b\\");
    terminal.feed(b"* Ran ls\r\n");
    terminal.feed(b"  one\r\n  two\r\n");
    terminal.feed(b"  three\x1b]7788;end;3\x1b\\\r\n");
    terminal.feed(b"after\r\nmore\r\nlast\r\nprompt$");
    assert!(
        terminal.grid().scrollback_len() >= 4,
        "the region rolled into history"
    );

    let plain = |rows: &[zerocode_pty::GridRow]| -> Vec<String> {
        rows.iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    };

    // Scroll back to where the header sits at the top of the view.
    let back = terminal.grid().scrollback_len();
    terminal.grid_mut().scroll_view(back as isize);
    let folded = terminal.grid().snapshot();
    let shown = plain(&folded.rows);
    assert_eq!(shown.len(), 4, "the window is still four rows tall");
    assert_eq!(shown[0], "* Ran ls", "the header stays");
    assert!(
        !shown
            .iter()
            .any(|row| row == "  one" || row == "  two" || row == "  three"),
        "a collapsed body must not ride out in scrollback: {shown:?}"
    );
    assert_eq!(shown[1], "after", "the vacated slots pull the next rows up");

    // The person opens it: the body comes back in place.
    assert!(terminal.grid_mut().set_fold_collapsed(3, false));
    let opened = plain(&terminal.grid().snapshot().rows);
    assert_eq!(opened[1], "  one");
    // And a frame goes out for the change without any other activity.
    assert!(
        terminal.grid_mut().take_delta().is_some(),
        "flipping a fold must produce a frame"
    );
    // Unknown ids are refused so a stale handle cannot invent a region.
    assert!(!terminal.grid_mut().set_fold_collapsed(99, true));
}

/// `teaser=<n>` names the density line a folded cell keeps: it is a row of
/// the region, drawn only while the region is collapsed, and the body it
/// stands for takes its place when the region opens — in a scrolled frame as
/// much as on the live screen.
#[test]
fn a_teaser_row_shows_collapsed_and_gives_way_to_the_body_when_opened() {
    use zerocode_pty::FoldRole::{Body, Header, Teaser};
    let mut terminal = Terminal::new(4, 24);
    terminal.feed(&fold_begin(5, "collapsed,teaser=1", "Ran ls"));
    terminal.feed(b"* Ran ls\r\n  ... +2 lines\r\n  one\r\n  two\r\n");
    terminal.feed(&fold_end(5));
    terminal.feed(b"after\r\nprompt$ ");

    // Collapsed, scrolled back: the frame carries the header and the teaser
    // and no body row at all — the grid composed them out.
    terminal.grid_mut().scroll_view(10);
    let collapsed_roles: Vec<_> = fold_roles(&terminal.grid().snapshot());
    assert!(collapsed_roles.contains(&Some((5, Header))));
    assert!(collapsed_roles.contains(&Some((5, Teaser))));
    assert!(
        !collapsed_roles.contains(&Some((5, Body))),
        "{collapsed_roles:?}"
    );

    // Collapsed: the teaser stands in for the body.
    let shown: Vec<String> = terminal
        .grid()
        .snapshot()
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    assert_eq!(shown[0], "* Ran ls");
    assert_eq!(shown[1], "  ... +2 lines");
    assert!(
        !shown.iter().any(|row| row == "  one" || row == "  two"),
        "{shown:?}"
    );

    // Opened: the body is back and the teaser is gone.
    assert!(terminal.grid_mut().set_fold_collapsed(5, false));
    let opened: Vec<String> = terminal
        .grid()
        .snapshot()
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    assert_eq!(opened[0], "* Ran ls");
    assert_eq!(opened[1], "  one");
    assert_eq!(opened[2], "  two");
    // Opened, the frame carries the body rows and no teaser.
    let opened_roles: Vec<_> = fold_roles(&terminal.grid().snapshot());
    assert!(opened_roles.contains(&Some((5, Body))), "{opened_roles:?}");
    assert!(
        !opened_roles.contains(&Some((5, Teaser))),
        "{opened_roles:?}"
    );
    assert!(
        !opened.iter().any(|row| row == "  ... +2 lines"),
        "{opened:?}"
    );
}

/// A collapsed fold on the LIVE screen does not leave the screen short: the
/// rows it hides are made up from history, so the bottom row stays the bottom
/// row and the cursor lands where the bottom now is. An inline TUI that folds
/// its tool output keeps its composer on the pane's floor.
#[test]
fn a_live_screen_with_folded_rows_fills_from_history_and_keeps_its_bottom() {
    use zerocode_pty::FoldRole::Body;
    let mut terminal = Terminal::new(6, 20);
    for line in 0..10 {
        terminal.feed(format!("h{line}\r\n").as_bytes());
    }
    terminal.feed(&fold_begin(9, "collapsed", "Ran tool"));
    terminal.feed(b"* Ran tool\r\n  b1\r\n  b2\r\n  b3\r\n");
    terminal.feed(&fold_end(9));
    terminal.feed(b"prompt$ ");
    let text = |delta: &GridDelta| -> Vec<String> {
        delta
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    };

    let frame = terminal.grid_mut().take_delta().expect("a frame");
    assert!(frame.full, "a composed live frame is absolute");
    assert_eq!(frame.view_offset, 0, "the person did not scroll");
    let shown = text(&frame);
    assert_eq!(shown.len(), 6, "{shown:?}");
    assert_eq!(
        shown.last().map(String::as_str),
        Some("prompt$"),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|row| row.starts_with("  b")),
        "hidden body rows left in: {shown:?}"
    );
    assert!(shown.contains(&"* Ran tool".to_string()), "{shown:?}");
    // The screen itself still held h9 above the fold; the three hidden rows
    // are made up by the three lines of history before it.
    assert_eq!(&shown[..4], &["h6", "h7", "h8", "h9"], "{shown:?}");
    assert!(!fold_roles(&frame).contains(&Some((9, Body))));
    assert_eq!(
        frame.cursor,
        (5, 8),
        "the caret sits on the composer row at the bottom"
    );

    // Opened: the body is back on the live screen and nothing is made up.
    assert!(terminal.grid_mut().set_fold_collapsed(9, false));
    let opened = text(&terminal.grid().snapshot());
    assert_eq!(opened.len(), 6);
    assert!(opened.iter().any(|row| row == "  b3"), "{opened:?}");
    assert_eq!(terminal.grid().snapshot().cursor, (5, 8));
}

/// A row that was written but not CHANGED is not a row to send.
///
/// `dirty_rows` records that somebody wrote to a row, which is the question
/// the parser can answer cheaply and is not the question the wire is asking.
/// The programs that cost the most are exactly the ones where the two answers
/// differ: a full-screen TUI repaints its entire frame to move one spinner,
/// and before the content gate every one of those rows crossed — measured at
/// 24×80, changing one row resent all twenty-four, on every frame, forever.
/// At 220×60 each such row is a 9.7 KB clone, a packed `String`, ~370 B of
/// JSON inside a script the webview parses as source, and then a rebuild and
/// a repaint on the other side.
#[test]
fn a_redraw_that_changed_nothing_sends_nothing() {
    // A TUI's own shape: home, then address and write every row.
    let paint = |mark: usize, marked: usize| {
        let mut out = String::from("\x1b[H");
        for row in 0..6 {
            out.push_str(&format!("\x1b[{};1H", row + 1));
            let n = if row == marked { mark } else { 0 };
            out.push_str(&format!("\x1b[3{}mrow {row} f{n}\x1b[0m", row % 8));
        }
        out
    };

    let mut terminal = Terminal::new(6, 20);
    terminal.feed(paint(0, 0).as_bytes());
    let first = terminal
        .grid_mut()
        .take_delta()
        .expect("the first paint is a frame");
    assert_eq!(first.rows.len(), 6, "the first paint owes every row");

    // The same bytes again, three times over. The screen is identical, so
    // there is no frame at all — not an empty one, which would still be an
    // emit, a script to parse and a pass through the consumer's `apply`.
    for round in 1..=3 {
        terminal.feed(paint(0, 0).as_bytes());
        assert!(
            terminal.grid_mut().take_delta().is_none(),
            "redraw {round} of an unchanged screen produced a frame"
        );
    }

    // And a redraw where exactly one row differs owes exactly that row.
    terminal.feed(paint(9, 3).as_bytes());
    let one = terminal
        .grid_mut()
        .take_delta()
        .expect("a changed row is a frame");
    let touched: Vec<usize> = one.rows.iter().map(|row| row.index).collect();
    assert_eq!(touched, vec![3], "a one-row change sent {touched:?}");

    // The gate must not swallow the row that goes BACK, either.
    terminal.feed(paint(0, 3).as_bytes());
    let back = terminal
        .grid_mut()
        .take_delta()
        .expect("changing it back is also a change");
    assert_eq!(
        back.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(terminal.grid().line(3), "row 3 f0");
}

/// A snapshot is a frame the consumer installs, so the content gate's record of
/// what the consumer holds has to become that snapshot too.
///
/// The window takes one every time a background tab comes back into view
/// (`term_snapshot`). The record still described the last DELTA — from before
/// the tab went away — while the consumer held the snapshot. A row that then
/// went back to its old content matched the stale record and was never sent,
/// so the snapshot's version stayed on screen: a line the program had erased,
/// still showing.
#[test]
fn a_row_that_returns_to_its_old_self_after_a_snapshot_is_still_sent() {
    let mut terminal = Terminal::new(4, 20);

    // Watched. Row 1 is written and then erased, each as its own frame, so the
    // record says — for certain, whatever the first frame was — that the
    // consumer holds row 1 blank.
    terminal.feed(b"\x1b[1;1Hprompt$\x1b[2;1Hx");
    let _ = terminal
        .grid_mut()
        .take_delta()
        .expect("the first paint is a frame");
    terminal.feed(b"\x1b[2;1H\x1b[2K");
    let blank = terminal
        .grid_mut()
        .take_delta()
        .expect("erasing a row is a frame");
    assert_eq!(
        blank.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![1]
    );

    // The tab goes to the background. A program prints on row 1 while nobody
    // watches — the pump takes only news for a shell it is not showing.
    terminal.feed(b"\x1b[2;1Hworking...");
    let _ = terminal.grid_mut().take_news();

    // The tab comes back, and the consumer rebuilds from a snapshot: it now
    // holds `working...` on row 1.
    let snapshot = terminal.grid_mut().take_snapshot();
    assert!(snapshot.full);

    // The program erases the line — back to exactly what the last delta said.
    terminal.feed(b"\x1b[2;1H\x1b[2K");
    assert_eq!(terminal.grid().line(1), "");
    let erased = terminal
        .grid_mut()
        .take_delta()
        .expect("erasing a line the consumer shows is a change");
    assert_eq!(
        erased.rows.iter().map(|row| row.index).collect::<Vec<_>>(),
        vec![1],
        "the erased row was held back, so `working...` would stay on screen"
    );
}
