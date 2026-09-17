//! A terminal's contents, written down so a window that closed can reopen
//! showing what it showed.
//!
//! Orca keeps a terminal's last output across restarts and replays it into
//! the fresh screen, and both halves are measured:
//!
//! - the CAPTURE serialises each pane at shutdown, capped at **512 KiB** of
//!   UTF-8 (`TERMINAL_SCROLLBACK_SESSION_BUFFER_BYTE_LIMIT`,
//!   I18nProvider-4EBrmTGg.js:46249); when the whole scrollback does not
//!   fit, it binary-searches the largest TAIL of rows that does
//!   (`captureTerminalShutdownLayout`, index-ftls8Hg_.js:100871-100900) —
//!   head-truncation would keep the oldest output and lose the part the
//!   person was looking at;
//! - the REPLAY strips a trailing alt-screen segment first — a buffer whose
//!   last `?1049h` comes after its last `?1049l` ends inside a TUI, and
//!   restoring that paints a dead interface nobody can leave
//!   (`restoreScrollbackBuffers`, I18nProvider:46152) — then feeds the
//!   buffer, one blank line, and `POST_REPLAY_MODE_RESET` (:46043): cursor
//!   style and visibility back to normal, kitty keyboard off, every mouse
//!   mode off, focus reporting off, bracketed paste off. The shell that is
//!   about to own the screen starts from a terminal in its right mind.
//!
//! Ours serialises from the grid the crate already owns — scrollback rows
//! keep their styles here, so the writing is SGR transitions over cells
//! rather than a raw byte tail, and replaying it through the same parser
//! reproduces what was on screen.

use std::num::NonZeroU32;

use crate::grid::{Cell, CellStyle, Color, TerminalGrid};

/// The most a stored buffer may weigh
/// (`TERMINAL_SCROLLBACK_SESSION_BUFFER_BYTE_LIMIT`, I18nProvider:46249).
pub const SCROLLBACK_BUFFER_BYTE_LIMIT: usize = 512 * 1024;

/// What the replay ends with (`POST_REPLAY_MODE_RESET`, I18nProvider:46043):
/// default cursor style, kitty keyboard protocol popped and zeroed, cursor
/// shown, the six mouse modes off, focus reporting off, bracketed paste off.
pub const POST_REPLAY_MODE_RESET: &str = "\x1b[0 q\x1b[<99u\x1b[=0u\x1b[?25h\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1016l\x1b[?1004l\x1b[?2004l";

const ALT_SCREEN_ON: &str = "\x1b[?1049h";
const ALT_SCREEN_OFF: &str = "\x1b[?1049l";

/// The grid's contents as replayable bytes, newest rows first to survive
/// the cap.
///
/// Scrollback oldest-first, then the visible screen with its trailing blank
/// rows trimmed. When the whole thing exceeds `max_bytes`, Orca's binary
/// search finds the largest tail that fits — and a tail that cannot fit even
/// one row is stored as nothing, which restores as nothing.
#[must_use]
pub fn serialize_tail(grid: &TerminalGrid, max_bytes: usize) -> String {
    let mut rows: Vec<&[Cell]> = (0..grid.scrollback_len())
        .filter_map(|index| grid.stored_scrollback_cells(index))
        .collect();
    let mut visible: Vec<&[Cell]> = (0..grid.screen_rows())
        .map(|row| grid.row_cells(row))
        .collect();
    while visible.last().is_some_and(|cells| row_end(cells) == 0) {
        visible.pop();
    }
    rows.extend(visible);

    let whole = render_rows(grid, &rows);
    if whole.len() <= max_bytes {
        return whole;
    }
    let mut lo = 1usize;
    let mut hi = rows.len();
    let mut best = String::new();
    while lo <= hi {
        let mid = usize::midpoint(lo, hi);
        let attempt = render_rows(grid, &rows[rows.len() - mid..]);
        if attempt.len() <= max_bytes {
            best = attempt;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    best
}

/// A stored buffer as the bytes to feed a fresh terminal — the alt-screen
/// strip, the buffer, one line break, and the mode reset.
#[must_use]
pub fn replay_payload(stored: &str) -> Vec<u8> {
    let last_on = stored.rfind(ALT_SCREEN_ON);
    let last_off = stored.rfind(ALT_SCREEN_OFF);
    let kept = match (last_on, last_off) {
        (Some(on), Some(off)) if on > off => &stored[..on],
        (Some(on), None) => &stored[..on],
        _ => stored,
    };
    let mut bytes = Vec::with_capacity(kept.len() + 2 + POST_REPLAY_MODE_RESET.len());
    bytes.extend_from_slice(kept.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(POST_REPLAY_MODE_RESET.as_bytes());
    bytes
}

fn render_rows(grid: &TerminalGrid, rows: &[&[Cell]]) -> String {
    let mut out = String::new();
    for (at, cells) in rows.iter().enumerate() {
        if at > 0 {
            out.push_str("\r\n");
        }
        render_row(&mut out, grid, cells);
    }
    out
}

/// What closes an explicit hyperlink — `OSC 8` with no address.
const HYPERLINK_CLOSE: &str = "\x1b]8;;\x1b\\";

/// Where a row stops being worth writing: past its last cell carrying ink. A
/// STYLED blank stays — a row of spaces on a coloured background is a painted
/// bar, and trimming it un-paints it.
///
/// What counts as ink is [`Cell::is_ink`]'s to say, not this function's: a
/// board card trims its own trailing rows by the same rule, and two spellings
/// of "blank" is how one of them comes to eat a bar the other kept.
fn row_end(cells: &[Cell]) -> usize {
    cells.iter().rposition(Cell::is_ink).map_or(0, |at| at + 1)
}

fn render_row(out: &mut String, grid: &TerminalGrid, cells: &[Cell]) {
    let mut current = CellStyle::default();
    let mut linked: Option<NonZeroU32> = None;
    for cell in &cells[..row_end(cells)] {
        // The glyph in front already advanced two columns; writing anything
        // for its shadow column would write it twice.
        if cell.is_continuation() {
            continue;
        }
        // An address is not a rendition: it travels as its own OSC 8
        // transition, so the SGR comparison is made on the paint alone and a
        // link's edges do not re-send colours that did not change.
        if cell.style.link != linked {
            match cell.style.link.and_then(|id| grid.link_uri(id)) {
                Some(uri) => {
                    out.push_str("\x1b]8;;");
                    out.push_str(uri);
                    out.push_str("\x1b\\");
                }
                // Either the row left a link, or it names an id this grid no
                // longer knows — both end the link rather than guess at one.
                None => out.push_str(HYPERLINK_CLOSE),
            }
            linked = cell.style.link;
        }
        let paint = CellStyle {
            link: None,
            ..cell.style
        };
        if paint != current {
            push_sgr(out, &paint);
            current = paint;
        }
        out.push(cell.ch);
        for scalar in cell.zw.scalars() {
            out.push(scalar);
        }
    }
    // A row that ended inside a link closes it: a replayed buffer whose last
    // line left one open would make every byte the next program prints part
    // of somebody else's address.
    if linked.is_some() {
        out.push_str(HYPERLINK_CLOSE);
    }
    if current != CellStyle::default() {
        out.push_str("\x1b[0m");
    }
}

/// One SGR that says everything this style is — reset first, then each fact.
/// A diff against the previous style would be smaller, but a full statement
/// cannot inherit a stale attribute from a row the cap cut in half.
fn push_sgr(out: &mut String, style: &CellStyle) {
    use std::fmt::Write as _;
    out.push_str("\x1b[0");
    if style.bold {
        out.push_str(";1");
    }
    if style.dim {
        out.push_str(";2");
    }
    if style.italic {
        out.push_str(";3");
    }
    if style.underline {
        out.push_str(";4");
    }
    if style.reverse {
        out.push_str(";7");
    }
    match style.fg {
        Color::Default => {}
        Color::Indexed(n @ 0..=7) => {
            let _ = write!(out, ";{}", 30 + u16::from(n));
        }
        Color::Indexed(n @ 8..=15) => {
            let _ = write!(out, ";{}", 90 + u16::from(n) - 8);
        }
        Color::Indexed(n) => {
            let _ = write!(out, ";38;5;{n}");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, ";38;2;{r};{g};{b}");
        }
    }
    match style.bg {
        Color::Default => {}
        Color::Indexed(n @ 0..=7) => {
            let _ = write!(out, ";{}", 40 + u16::from(n));
        }
        Color::Indexed(n @ 8..=15) => {
            let _ = write!(out, ";{}", 100 + u16::from(n) - 8);
        }
        Color::Indexed(n) => {
            let _ = write!(out, ";48;5;{n}");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, ";48;2;{r};{g};{b}");
        }
    }
    out.push('m');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Terminal;

    fn fed(rows: usize, cols: usize, bytes: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(rows, cols);
        terminal.feed(bytes);
        terminal
    }

    /// A link survives being written down and read back, and never outlives
    /// its own row.
    ///
    /// The address is what a stored screen would otherwise lose: the cell
    /// carries an id into a table that dies with the terminal, so a buffer
    /// that wrote only the text would replay the words with nothing behind
    /// them.
    #[test]
    fn a_serialized_hyperlink_replays_as_the_same_address() {
        let first = fed(
            3,
            20,
            b"open \x1b]8;;https://example.com/x\x1b\\here\x1b]8;;\x1b\\ done",
        );
        let written = serialize_tail(first.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT);
        let mut second = Terminal::new(3, 20);
        second.feed(written.as_bytes());

        let replayed = second.grid().row_cells(0);
        let id = replayed[5].style.link.expect("the linked cell came back");
        assert_eq!(
            second.grid().link_uri(id),
            Some("https://example.com/x"),
            "the address behind the replayed cell"
        );
        assert_eq!(replayed[4].style.link, None, "the space before it");
        assert_eq!(replayed[9].style.link, None, "the text after the close");
    }

    /// A row that ends inside a link closes it on the way out — otherwise the
    /// next program to print into that terminal writes into somebody else's
    /// address.
    #[test]
    fn a_row_that_ends_inside_a_link_closes_it() {
        let terminal = fed(3, 20, b"\x1b]8;;https://open\x1b\\tail");
        let written = serialize_tail(terminal.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT);
        assert!(
            written.trim_end().ends_with("\x1b]8;;\x1b\\") || written.contains("\x1b]8;;\x1b\\"),
            "the buffer left a link open: {written:?}"
        );

        let mut second = Terminal::new(3, 20);
        second.feed(written.as_bytes());
        second.feed(b"\r\nafter");
        assert_eq!(
            second.grid().row_cells(1)[0].style.link,
            None,
            "text printed after the replay joined the stored link"
        );
    }

    /// The round trip is the property: what a terminal showed, written down
    /// and fed to a fresh terminal, shows again — text, colour, weight, and
    /// a double-width glyph — including the rows that had already scrolled
    /// off the top.
    #[test]
    fn a_serialized_screen_replays_as_the_same_screen() {
        let first = fed(
            3,
            20,
            b"scrolled away\r\n\x1b[1;31mbold red\x1b[0m\r\nplain \x1b[42m  \x1b[0m\r\n\xed\x95\x9c\xea\xb8\x80",
        );
        let written = serialize_tail(first.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT);
        let mut second = Terminal::new(3, 20);
        second.feed(written.as_bytes());
        // The visible screen agrees…
        assert_eq!(second.grid().visible_text(), first.grid().visible_text());
        // …the scrolled-off row came along…
        assert_eq!(second.grid().scrollback_line(0), "scrolled away");
        // …and the styles survived: bold red text, and the painted blank.
        let styled = second.grid().cell(0, 0).expect("bold row");
        assert!(styled.style.bold);
        assert_eq!(styled.style.fg, Color::Indexed(1));
        let bar = second.grid().cell(1, 6).expect("painted blank");
        assert_eq!(bar.style.bg, Color::Indexed(2));
        assert_eq!(bar.ch, ' ');
        // The wide glyph holds both of its columns.
        assert!(second.grid().cell(2, 1).expect("shadow").is_continuation());
    }

    /// Compact history and an old full-width replay buffer have the same
    /// persistence contract: styles, wide glyphs, wrapping, and painted blank
    /// tails survive a serialize/restore round trip. Literal trailing default
    /// spaces model the rows written by the old full-width representation;
    /// reopening them must remain valid even though new history stores less.
    #[test]
    fn compact_and_old_full_width_rows_restore_equivalently() {
        let first = fed(
            4,
            16,
            "\x1b[1;31mstyled\x1b[0m         \r\n\
             \u{d55c}\u{ae00}            \r\n\
             wrapped-across-the-edge\r\n\
             tail\x1b[15G\x1b[44m \x1b[0m\r\n\
             newest"
                .as_bytes(),
        );
        let written = serialize_tail(first.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT);
        let mut restored = Terminal::new(4, 16);
        restored.feed(written.as_bytes());

        assert_eq!(
            serialize_tail(restored.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT),
            written,
            "a second save changed the restored session"
        );
        assert!(written.contains("\x1b[0;1;31mstyled"));
        assert!(written.contains("\u{d55c}\u{ae00}"));
        assert!(written.contains("wrapped-across-"));
        assert!(written.contains("\x1b[0;44m "));
    }

    /// The cap keeps the NEWEST rows — Orca's binary search over the tail —
    /// and what cannot fit at all is stored as nothing.
    #[test]
    fn the_cap_keeps_the_tail_not_the_head() {
        let mut terminal = Terminal::new(4, 10);
        for line in 0..200 {
            terminal.feed(format!("line {line}\r\n").as_bytes());
        }
        let whole = serialize_tail(terminal.grid(), SCROLLBACK_BUFFER_BYTE_LIMIT);
        assert!(whole.contains("line 0") && whole.contains("line 199"));
        let capped = serialize_tail(terminal.grid(), 120);
        assert!(capped.len() <= 120);
        assert!(
            capped.contains("line 199"),
            "the newest row must survive: {capped:?}"
        );
        assert!(!capped.contains("line 0"), "the oldest row must go first");
        assert_eq!(serialize_tail(terminal.grid(), 0), "");
    }

    /// The replay strip: a buffer that ends inside the alternate screen is
    /// cut at the entry — the scrollback restores, the dead TUI does not —
    /// while one that LEFT the alternate screen keeps everything. Both
    /// endings then get the line break and the measured mode reset.
    #[test]
    fn a_buffer_ending_inside_the_alt_screen_is_cut_at_the_door() {
        let stuck = format!("before{ALT_SCREEN_ON}a dead tui");
        let replayed = replay_payload(&stuck);
        let text = String::from_utf8(replayed).expect("utf8");
        assert!(text.starts_with("before\r\n"));
        assert!(!text.contains("dead tui"));
        assert!(text.ends_with(POST_REPLAY_MODE_RESET));

        let escaped = format!("before{ALT_SCREEN_ON}inside{ALT_SCREEN_OFF}after");
        let kept = String::from_utf8(replay_payload(&escaped)).expect("utf8");
        assert!(kept.contains("after"));
        assert!(kept.ends_with(POST_REPLAY_MODE_RESET));
    }

    /// The reset is the measured byte string, verbatim — cursor style,
    /// kitty pop, cursor shown, six mouse modes, focus, bracketed paste.
    #[test]
    fn the_mode_reset_is_orcas_own() {
        assert_eq!(
            POST_REPLAY_MODE_RESET,
            "\u{1b}[0 q\u{1b}[<99u\u{1b}[=0u\u{1b}[?25h\u{1b}[?9l\u{1b}[?1000l\u{1b}[?1002l\u{1b}[?1003l\u{1b}[?1006l\u{1b}[?1016l\u{1b}[?1004l\u{1b}[?2004l"
        );
        assert_eq!(SCROLLBACK_BUFFER_BYTE_LIMIT, 512 * 1024);
    }

    /// Why the replay has to come first. The reset is right for a terminal
    /// nothing has spoken into yet, and wrong for one a program already set
    /// up: a TUI that switches bracketed paste on once — zo does, at start —
    /// loses it to a replay that lands after, and every multi-line paste then
    /// arrives as typed keys whose first line break submits (2026-09-17). The
    /// shell feeds a restored leaf's screen before it holds the terminal, so
    /// the program's own modes always land last.
    #[test]
    fn a_replay_ahead_of_the_program_keeps_the_modes_the_program_sets() {
        let stored = serialize_tail(
            fed(3, 20, b"an old screen").grid(),
            SCROLLBACK_BUFFER_BYTE_LIMIT,
        );
        let program_start = b"\x1b[?2004h\x1b[?1004h";

        let mut first = Terminal::new(3, 20);
        first.feed(&replay_payload(&stored));
        first.feed(program_start);
        assert!(
            first.grid().bracketed_paste(),
            "the program's bracketed paste was lost to a replay that came first"
        );

        let mut late = Terminal::new(3, 20);
        late.feed(program_start);
        late.feed(&replay_payload(&stored));
        assert!(
            !late.grid().bracketed_paste(),
            "a replay after the program no longer resets its modes — the \
             ordering rule this test explains is moot, revisit the reset"
        );
    }
}
