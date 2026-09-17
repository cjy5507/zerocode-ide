//! Turning key presses into terminal bytes.
//!
//! The view layer reports keys as the DOM names them (`"a"`, `"Enter"`,
//! `"ArrowUp"`) and this module owns what the child actually receives. It
//! lives here, not in the webview, for the same reason the grid does (ADR
//! 0002): byte-level terminal knowledge stays in Rust, where it is proven by
//! feeding a function and asserting bytes — no browser, no flakiness.
//!
//! The counterpart of the grid's rule holds on the way in, too: a key this
//! table does not know is **swallowed**, not guessed. Sending an invented
//! sequence teaches the child a lie, and the visible symptom — stray glyphs
//! after pressing a media key — is exactly the failure the grid refuses on
//! output.

use crate::grid::MouseTracking;
use serde::Deserialize;

/// One key press, as the view reports it.
///
/// `key` uses the DOM `KeyboardEvent.key` vocabulary: printable keys arrive as
/// the character they produce (already shifted — `Shift+a` is `"A"`), the rest
/// as names like `"Enter"`.
///
/// `shift` is here because the sentence that used to stand in its place — that
/// no consumer distinguishes a shifted named key — is measurably false of the
/// programs this product hosts. `codex` reads `ESC[Z` for Shift+Tab and moves
/// backwards through its own fields with it; `claude` reads `ESC[1;5C` and
/// `ESC[1;5D` for Ctrl+Right and Ctrl+Left and walks its composer by words
/// with them. Both were sent the unmodified key and did nothing at all.
#[derive(Debug, Clone, Deserialize)]
pub struct KeyPress {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

/// Encode a key press, or `None` for a key the child has no bytes for.
#[must_use]
pub fn encode_key(press: &KeyPress) -> Option<Vec<u8>> {
    let mut ch = press.key.chars();
    let (first, rest) = (ch.next()?, ch.next());

    // A single character is a printable key. Everything else is a named key.
    if rest.is_none() {
        return Some(encode_char(first, press.ctrl, press.alt));
    }

    let held = modifier(press);
    Some(match press.key.as_str() {
        "Enter" => b"\r".to_vec(),
        // DEL, not BS: every modern terminal sends 0x7f for the backspace key
        // and programs read 0x08 as `Ctrl+H`.
        "Backspace" => b"\x7f".to_vec(),
        // Shift+Tab is not Tab wearing a modifier — it has a sequence of its
        // own, and it is the one every TUI binds to "the field before this
        // one". Measured in the `codex` binary, which carries `ESC[Z` and
        // nothing that would read a modified tab.
        "Tab" if press.shift => b"\x1b[Z".to_vec(),
        "Tab" => b"\t".to_vec(),
        "Escape" => b"\x1b".to_vec(),
        "ArrowUp" => cursor_key(b'A', held),
        "ArrowDown" => cursor_key(b'B', held),
        "ArrowRight" => cursor_key(b'C', held),
        "ArrowLeft" => cursor_key(b'D', held),
        "Home" => cursor_key(b'H', held),
        "End" => cursor_key(b'F', held),
        "Insert" => tilde_key(2, held),
        "Delete" => tilde_key(3, held),
        "PageUp" => tilde_key(5, held),
        "PageDown" => tilde_key(6, held),
        // The first four function keys are their own dialect — `ESC O P`
        // rather than a `~` form — and wear their modifiers the way the
        // cursor keys do. `opencode` reads `ESC O Q` and `codex` reads
        // `ESC O S`; both were sent nothing at all, because this table
        // answered `None` for every function key.
        "F1" => function_key(b'P', held),
        "F2" => function_key(b'Q', held),
        "F3" => function_key(b'R', held),
        "F4" => function_key(b'S', held),
        "F5" => tilde_key(15, held),
        "F6" => tilde_key(17, held),
        "F7" => tilde_key(18, held),
        "F8" => tilde_key(19, held),
        "F9" => tilde_key(20, held),
        "F10" => tilde_key(21, held),
        "F11" => tilde_key(23, held),
        "F12" => tilde_key(24, held),
        _ => return None,
    })
}

/// The modifier parameter a terminal writes into a key sequence: one, plus a
/// bit for each modifier held.
///
/// Not invented here — it is what the programs in front of us parse. `claude`
/// carries `ESC[1;5C`, and 5 is 1 + 4: the control bit alone.
fn modifier(press: &KeyPress) -> u8 {
    1 + u8::from(press.shift) + (u8::from(press.alt) << 1) + (u8::from(press.ctrl) << 2)
}

/// A cursor-key sequence: bare while nothing is held, and `CSI 1 ; m <final>`
/// once something is.
fn cursor_key(final_byte: u8, held: u8) -> Vec<u8> {
    if held > 1 {
        return modified_key(final_byte, held);
    }
    vec![0x1b, b'[', final_byte]
}

/// A function key of the `ESC O <final>` family, which wears its modifiers in
/// the cursor keys' clothes rather than its own.
fn function_key(final_byte: u8, held: u8) -> Vec<u8> {
    if held > 1 {
        return modified_key(final_byte, held);
    }
    vec![0x1b, b'O', final_byte]
}

fn modified_key(final_byte: u8, held: u8) -> Vec<u8> {
    format!("\x1b[1;{held}{}", char::from(final_byte)).into_bytes()
}

/// A numbered key of the `CSI n ~` family, with the modifier as its second
/// parameter when one is held.
fn tilde_key(number: u8, held: u8) -> Vec<u8> {
    if held > 1 {
        return format!("\x1b[{number};{held}~").into_bytes();
    }
    format!("\x1b[{number}~").into_bytes()
}

/// One printable character, with control and alt applied in that order.
fn encode_char(ch: char, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    // ESC-prefix is how a terminal spells "alt held": programs that bind
    // `Alt+b` read it as `ESC b`.
    if alt {
        bytes.push(0x1b);
    }
    if ctrl && let Some(byte) = control_byte(ch) {
        bytes.push(byte);
        return bytes;
    }
    let mut buffer = [0u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
    bytes
}

/// The C0 byte a control chord produces, if the character has one.
///
/// `Ctrl+key` on a character without a control mapping falls back to the plain
/// character rather than being dropped: swallowing the letter because the
/// modifier meant nothing would eat real typing.
fn control_byte(ch: char) -> Option<u8> {
    match ch {
        // Ctrl+A..Z (either case) — the classic C0 range.
        'a'..='z' => Some(ch as u8 - b'a' + 1),
        'A'..='Z' => Some(ch as u8 - b'A' + 1),
        ' ' | '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        _ => None,
    }
}

/// Focus in (`ESC[I`) or focus out (`ESC[O`), for a program that asked to be
/// told (`CSI ?1004h`).
///
/// The mode has been parsed and remembered since the grid was written and
/// these bytes were never sent — the window had no wiring for it, so
/// `focus_reporting` was a field nobody read for a purpose. That is worse than
/// not supporting it: a program that asks is told "yes" by the mode being
/// accepted, and then never hears anything. An agent TUI asks on startup
/// (`?1004h` is in the opening burst `claude` sends,
/// docs/reverse/orca-ui-inventory.md 1-b-2), and uses it to dim its own
/// prompt, pause a spinner, or stop rendering a caret it does not own.
///
/// Whether the program asked is the caller's to check, from the grid — this
/// only says what the answer looks like.
#[must_use]
pub const fn encode_focus(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

/// Encode pasted text, honouring bracketed paste when the program asked for
/// it.
///
/// The flag comes from the grid ([`crate::TerminalGrid::bracketed_paste`]):
/// the program on the other side enabled the mode, so pasting without the
/// brackets makes a multi-line paste execute line by line — the classic way a
/// pasted script runs half-finished.
#[must_use]
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = sanitize_paste(text);
    if !bracketed {
        return text.into_bytes();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// What an escape becomes on its way into a paste: the printable glyph that
/// names it (U+241B SYMBOL FOR ESCAPE), which is what Orca substitutes.
const INERT_ESCAPE: char = '␛';

/// Make text safe to hand a terminal as a paste.
///
/// Two changes, and the first one is the important one.
///
/// **Every escape becomes `␛`.** Without this, text containing `ESC[201~`
/// closes the bracketed-paste envelope early and everything after it is read
/// as *typing* — so a paste can issue arbitrary control sequences, and a
/// prompt assembled from a file, a web page or an agent's output becomes a way
/// to drive the terminal. Replacing rather than dropping is deliberate: the
/// glyph is visible, so a person pasting something that contained an escape
/// can see that it did (`sanitizeBracketedPasteText`,
/// terminal-pty-input-transaction-Cyw2TgQo.js:19-27).
///
/// **Line endings become `\r`.** Enter is a carriage return on a terminal;
/// `\n` is a line feed and a TUI reading keys does not have to treat the two
/// alike. Pasting a recipe of three commands should run three commands
/// (`normalizeTerminalPasteLineEndings`, same file:34-36).
///
/// Applied whether or not the program asked for bracketed paste. The envelope
/// is what tells a program that text was pasted; it was never what made the
/// text safe, and a program that has not asked for it is if anything less
/// prepared for what an escape would do.
pub fn sanitize_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => out.push(INERT_ESCAPE),
            // CRLF collapses to one return rather than two.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\r');
            }
            '\n' => out.push('\r'),
            _ => out.push(ch),
        }
    }
    out
}

/// What the pointer did, as the view reports it.
///
/// Cells, not pixels: the window owns the font metrics and the child owns the
/// grid, so the translation belongs on the window side and the number that
/// crosses is the one the child speaks in.
#[derive(Debug, Clone, Deserialize)]
pub struct MouseEvent {
    /// Zero-based cell under the pointer.
    pub row: usize,
    pub col: usize,
    /// 0 left, 1 middle, 2 right, 3 none (a bare move), 64 wheel up, 65 wheel
    /// down — xterm's own numbering, so nothing has to be translated twice.
    pub button: u8,
    pub kind: MouseKind,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseKind {
    Press,
    Release,
    Move,
}

/// Encode a mouse event, or `None` when this program should not hear about it.
///
/// The same rule the key table follows: what the child has no bytes for is
/// **swallowed**, not guessed. Here that covers three cases beyond an unknown
/// button — a program that asked for nothing, a move to a program that only
/// asked about clicks, and a position the legacy encoding cannot express.
/// That last one is why SGR exists: `CSI M` spends one byte per coordinate
/// with a +32 bias, so column 224 has nowhere to go, and sending the wrong
/// cell is worse than sending none.
#[must_use]
pub fn encode_mouse(event: &MouseEvent, tracking: MouseTracking, sgr: bool) -> Option<Vec<u8>> {
    if tracking == MouseTracking::Off {
        return None;
    }
    let wheel = event.button >= 64;
    match event.kind {
        // A wheel notch is a press with no release, and it does not move.
        MouseKind::Release if wheel => return None,
        MouseKind::Move if wheel => return None,
        MouseKind::Move => match tracking {
            MouseTracking::Motion => {}
            // Button 3 is "none held", which is precisely what this level does
            // not report.
            MouseTracking::Drag if event.button < 3 => {}
            _ => return None,
        },
        _ => {}
    }

    let mut code = u32::from(event.button);
    if event.kind == MouseKind::Move {
        code += 32;
    }
    if event.shift {
        code += 4;
    }
    if event.alt {
        code += 8;
    }
    if event.ctrl {
        code += 16;
    }

    if sgr {
        // SGR keeps the button on release, which is the whole reason a program
        // asks for it: the legacy form cannot say which button was let go.
        let last = if event.kind == MouseKind::Release {
            'm'
        } else {
            'M'
        };
        return Some(
            format!("\x1b[<{};{};{}{}", code, event.col + 1, event.row + 1, last).into_bytes(),
        );
    }

    // Legacy: a release is button 3, and which one it was is lost.
    let legacy = if event.kind == MouseKind::Release {
        code - u32::from(event.button) + 3
    } else {
        code
    };
    let button = u8::try_from(legacy + 32).ok()?;
    let column = u8::try_from(event.col + 33).ok()?;
    let row = u8::try_from(event.row + 33).ok()?;
    Some(vec![0x1b, b'[', b'M', button, column, row])
}

#[cfg(test)]
mod mouse_encoding {
    use super::*;

    fn at(row: usize, col: usize, button: u8, kind: MouseKind) -> MouseEvent {
        MouseEvent {
            row,
            col,
            button,
            kind,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    /// A program that never asked hears nothing.
    #[test]
    fn silence_until_the_program_asks() {
        let press = at(0, 0, 0, MouseKind::Press);
        assert_eq!(encode_mouse(&press, MouseTracking::Off, false), None);
        assert_eq!(encode_mouse(&press, MouseTracking::Off, true), None);
    }

    /// The legacy form biases every field by 32 and is one-based on screen.
    #[test]
    fn the_legacy_form_is_biased_by_thirty_two() {
        assert_eq!(
            encode_mouse(&at(0, 0, 0, MouseKind::Press), MouseTracking::Click, false),
            Some(b"\x1b[M\x20\x21\x21".to_vec())
        );
        // A release does not say which button it was — that is the whole
        // reason a program asks for SGR.
        assert_eq!(
            encode_mouse(
                &at(0, 0, 2, MouseKind::Release),
                MouseTracking::Click,
                false
            ),
            Some(b"\x1b[M\x23\x21\x21".to_vec())
        );
    }

    /// SGR keeps the button on release and can address a wide screen.
    #[test]
    fn sgr_keeps_the_button_and_the_column() {
        assert_eq!(
            encode_mouse(&at(9, 4, 1, MouseKind::Press), MouseTracking::Click, true),
            Some(b"\x1b[<1;5;10M".to_vec())
        );
        assert_eq!(
            encode_mouse(&at(9, 4, 1, MouseKind::Release), MouseTracking::Click, true),
            Some(b"\x1b[<1;5;10m".to_vec())
        );
        assert_eq!(
            encode_mouse(&at(0, 299, 0, MouseKind::Press), MouseTracking::Click, true),
            Some(b"\x1b[<0;300;1M".to_vec())
        );
    }

    /// Beyond what one byte can carry, the legacy form says nothing rather
    /// than naming the wrong cell.
    #[test]
    fn the_legacy_form_refuses_what_it_cannot_say() {
        assert_eq!(
            encode_mouse(
                &at(0, 299, 0, MouseKind::Press),
                MouseTracking::Click,
                false
            ),
            None
        );
    }

    /// Motion is reported at the level that asked for it, and only there.
    #[test]
    fn motion_is_reported_only_where_it_was_asked_for() {
        let dragging = at(1, 1, 0, MouseKind::Move);
        let hovering = at(1, 1, 3, MouseKind::Move);
        assert_eq!(encode_mouse(&dragging, MouseTracking::Click, true), None);
        assert_eq!(
            encode_mouse(&dragging, MouseTracking::Drag, true),
            Some(b"\x1b[<32;2;2M".to_vec()),
            "a drag was not reported to the level that asked for drags"
        );
        assert_eq!(
            encode_mouse(&hovering, MouseTracking::Drag, true),
            None,
            "a bare hover was reported to a level that only asked about drags"
        );
        assert_eq!(
            encode_mouse(&hovering, MouseTracking::Motion, true),
            Some(b"\x1b[<35;2;2M".to_vec())
        );
    }

    /// A wheel notch is a press with no release, and it does not move.
    #[test]
    fn a_wheel_is_a_press_and_nothing_else() {
        assert_eq!(
            encode_mouse(&at(0, 0, 64, MouseKind::Press), MouseTracking::Click, true),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                &at(0, 0, 65, MouseKind::Release),
                MouseTracking::Click,
                true
            ),
            None
        );
        assert_eq!(
            encode_mouse(&at(0, 0, 64, MouseKind::Move), MouseTracking::Motion, true),
            None
        );
    }

    /// The wheel's bytes, exactly, for the modes an agent TUI actually sets.
    ///
    /// Measured rather than assumed. `claude` opens a real terminal with
    /// `?1049h ?1000h ?1002h ?1003h ?1006h`, so the wheel is encoded at
    /// `Motion` + SGR; and fed exactly these bytes over a pty it scrolls its
    /// transcript (8.3 KB of redraw against 0.2 KB idle), which is what says
    /// these are the right bytes and not merely self-consistent ones.
    ///
    /// Pinned per field, because every one of them is a way to be silently
    /// wrong: 64 is up and 65 is down, the column comes before the row, both
    /// are ONE-based, and the final byte is an upper-case `M` — a lower-case
    /// `m` is a release, and a wheel has none, so a TUI reading one would see
    /// a button let go that was never pressed.
    #[test]
    fn the_wheel_is_sgr_to_the_byte() {
        for tracking in [
            MouseTracking::Click,
            MouseTracking::Drag,
            MouseTracking::Motion,
        ] {
            assert_eq!(
                encode_mouse(&at(11, 39, 64, MouseKind::Press), tracking, true),
                Some(b"\x1b[<64;40;12M".to_vec()),
                "wheel up is not the SGR press xterm defines"
            );
            assert_eq!(
                encode_mouse(&at(11, 39, 65, MouseKind::Press), tracking, true),
                Some(b"\x1b[<65;40;12M".to_vec()),
                "wheel down is not the SGR press xterm defines"
            );
        }
        // Shift is the terminal's own key and never reaches the program, so
        // the shifted weight (64 + 4) has no business being sent — the window
        // is what withholds it (`ui/shell.js`, the wheel handler).
        //
        // Without SGR the same notch is the legacy form: button + 32, and the
        // coordinates biased by 32 on top of being one-based.
        assert_eq!(
            encode_mouse(
                &at(11, 39, 64, MouseKind::Press),
                MouseTracking::Motion,
                false
            ),
            Some(vec![0x1b, b'[', b'M', 64 + 32, 39 + 33, 11 + 33]),
        );
    }

    /// Modifiers ride the same field, in xterm's own weights.
    #[test]
    fn modifiers_ride_the_button_field() {
        let mut event = at(0, 0, 0, MouseKind::Press);
        event.shift = true;
        assert_eq!(
            encode_mouse(&event, MouseTracking::Click, true),
            Some(b"\x1b[<4;1;1M".to_vec())
        );
        event.alt = true;
        event.ctrl = true;
        assert_eq!(
            encode_mouse(&event, MouseTracking::Click, true),
            Some(b"\x1b[<28;1;1M".to_vec())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str) -> KeyPress {
        KeyPress {
            key: key.to_string(),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    fn chord(key: &str, ctrl: bool, alt: bool) -> KeyPress {
        KeyPress {
            key: key.to_string(),
            ctrl,
            alt,
            shift: false,
        }
    }

    /// The three modifiers at once, for the keys that wear them as a number.
    fn held(key: &str, ctrl: bool, alt: bool, shift: bool) -> KeyPress {
        KeyPress {
            key: key.to_string(),
            ctrl,
            alt,
            shift,
        }
    }

    /// Shift+Tab is its own sequence, and the field before this one is what
    /// every TUI binds it to. Measured in the `codex` binary: it carries
    /// `ESC[Z` and nothing that would read a tab wearing a modifier.
    #[test]
    fn shift_and_tab_is_a_sequence_of_its_own() {
        assert_eq!(
            encode_key(&held("Tab", false, false, true)),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(encode_key(&press("Tab")), Some(b"\t".to_vec()));
    }

    /// A cursor key wearing a modifier says so in a parameter, and the number
    /// is one plus a bit per modifier. `claude` carries `ESC[1;5C` and walks
    /// its composer a word at a time with it — 5 being 1 + the control bit.
    #[test]
    fn a_modified_cursor_key_carries_the_modifier_it_was_pressed_with() {
        assert_eq!(
            encode_key(&held("ArrowRight", true, false, false)),
            Some(b"\x1b[1;5C".to_vec()),
            "the word-forward chord claude reads"
        );
        assert_eq!(
            encode_key(&held("ArrowLeft", true, false, false)),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            encode_key(&held("ArrowUp", false, false, true)),
            Some(b"\x1b[1;2A".to_vec()),
            "shift alone is 2"
        );
        assert_eq!(
            encode_key(&held("ArrowDown", false, true, false)),
            Some(b"\x1b[1;3B".to_vec()),
            "alt alone is 3"
        );
        assert_eq!(
            encode_key(&held("Home", true, true, true)),
            Some(b"\x1b[1;8H".to_vec()),
            "all three together"
        );
        // And nothing held is still the bare key: a program reading the plain
        // form must not start seeing a parameter it never saw before.
        assert_eq!(encode_key(&press("ArrowRight")), Some(b"\x1b[C".to_vec()));
        assert_eq!(encode_key(&press("Home")), Some(b"\x1b[H".to_vec()));
    }

    /// The numbered keys wear the modifier as a second parameter.
    #[test]
    fn a_modified_numbered_key_wears_the_modifier_second() {
        assert_eq!(encode_key(&press("Delete")), Some(b"\x1b[3~".to_vec()));
        assert_eq!(
            encode_key(&held("Delete", true, false, false)),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(
            encode_key(&held("PageUp", false, false, true)),
            Some(b"\x1b[5;2~".to_vec())
        );
    }

    /// Function keys existed nowhere in this table, so every one of them was
    /// swallowed. `opencode` reads `ESC O Q` and `codex` reads `ESC O S`.
    #[test]
    fn the_function_keys_are_answered_in_their_two_dialects() {
        assert_eq!(encode_key(&press("F1")), Some(b"\x1bOP".to_vec()));
        assert_eq!(encode_key(&press("F2")), Some(b"\x1bOQ".to_vec()));
        assert_eq!(encode_key(&press("F4")), Some(b"\x1bOS".to_vec()));
        assert_eq!(encode_key(&press("F5")), Some(b"\x1b[15~".to_vec()));
        assert_eq!(encode_key(&press("F10")), Some(b"\x1b[21~".to_vec()));
        assert_eq!(encode_key(&press("F12")), Some(b"\x1b[24~".to_vec()));
        // The first four take the cursor keys' modified clothes, not their own.
        assert_eq!(
            encode_key(&held("F1", true, false, false)),
            Some(b"\x1b[1;5P".to_vec())
        );
        assert_eq!(
            encode_key(&held("F5", false, false, true)),
            Some(b"\x1b[15;2~".to_vec())
        );
    }

    /// Shift on a key this table does not spell with a modifier changes
    /// nothing: Enter is a carriage return however it was pressed.
    #[test]
    fn shift_does_not_leak_into_the_keys_that_have_no_modified_form() {
        assert_eq!(
            encode_key(&held("Enter", false, false, true)),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            encode_key(&held("Backspace", false, false, true)),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(
            encode_key(&held("Escape", false, false, true)),
            Some(b"\x1b".to_vec())
        );
    }

    #[test]
    fn printable_keys_pass_through_as_utf8() {
        assert_eq!(encode_key(&press("a")), Some(b"a".to_vec()));
        assert_eq!(encode_key(&press("A")), Some(b"A".to_vec()));
        assert_eq!(encode_key(&press("한")), Some("한".as_bytes().to_vec()));
    }

    #[test]
    fn named_keys_encode_to_their_sequences() {
        assert_eq!(encode_key(&press("Enter")), Some(b"\r".to_vec()));
        assert_eq!(encode_key(&press("Backspace")), Some(b"\x7f".to_vec()));
        assert_eq!(encode_key(&press("ArrowUp")), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(&press("Delete")), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode_key(&press("Escape")), Some(b"\x1b".to_vec()));
    }

    /// The rule the module exists for: an unknown key must produce nothing,
    /// because an invented sequence reaches the child as typed garbage.
    #[test]
    fn unknown_named_keys_are_swallowed() {
        assert_eq!(encode_key(&press("MediaPlayPause")), None);
        assert_eq!(encode_key(&press("F13")), None);
        assert_eq!(encode_key(&press("")), None);
    }

    #[test]
    fn control_chords_map_to_c0_bytes() {
        assert_eq!(encode_key(&chord("c", true, false)), Some(vec![0x03]));
        assert_eq!(encode_key(&chord("C", true, false)), Some(vec![0x03]));
        assert_eq!(encode_key(&chord("d", true, false)), Some(vec![0x04]));
        assert_eq!(encode_key(&chord(" ", true, false)), Some(vec![0x00]));
        assert_eq!(encode_key(&chord("[", true, false)), Some(vec![0x1b]));
    }

    /// Ctrl on a character with no C0 mapping types the character instead of
    /// eating it.
    #[test]
    fn control_without_a_mapping_falls_back_to_the_character() {
        assert_eq!(encode_key(&chord("1", true, false)), Some(b"1".to_vec()));
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            encode_key(&chord("b", false, true)),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode_key(&chord("c", true, true)),
            Some(vec![0x1b, 0x03]),
            "alt and ctrl stack: ESC then the control byte"
        );
    }

    #[test]
    fn paste_is_wrapped_only_when_the_program_asked() {
        // A newline arrives as a carriage return either way: Enter is `\r` on
        // a terminal, and a pasted recipe of commands should run them.
        assert_eq!(encode_paste("ls\n", false), b"ls\r".to_vec());
        assert_eq!(
            encode_paste("ls\n", true),
            b"\x1b[200~ls\r\x1b[201~".to_vec()
        );
    }

    /// A paste cannot talk to the terminal.
    ///
    /// The hole this closes: text containing `ESC[201~` closes the envelope
    /// early, and every byte after it is read as typing rather than as
    /// content. A prompt built from a file, a web page or another agent's
    /// output would then be able to issue any control sequence it liked —
    /// clear the screen, retitle the window, or answer a prompt on the
    /// person's behalf. The escape is replaced rather than removed so the
    /// paste still shows that something was there.
    #[test]
    fn a_paste_cannot_close_its_own_envelope_and_start_typing() {
        let hostile = "hello\x1b[201~\x1b[2J\x1b]0;owned\x07";
        let sent = String::from_utf8(encode_paste(hostile, true)).expect("utf-8");
        assert_eq!(
            sent.matches('\x1b').count(),
            2,
            "the only escapes left are the envelope's own two: {sent:?}"
        );
        assert!(sent.starts_with("\x1b[200~") && sent.ends_with("\x1b[201~"));
        assert!(
            sent.contains("hello␛[201~␛[2J␛]0;owned\x07"),
            "the content was not neutralised: {sent:?}"
        );
        // Unwrapped is not a reason to leave it armed — a program that never
        // asked for bracketed paste is no better prepared for an escape.
        let bare = String::from_utf8(encode_paste(hostile, false)).expect("utf-8");
        assert!(
            !bare.contains('\x1b'),
            "an unwrapped paste kept its escapes"
        );
    }

    /// CRLF is one Enter, not two.
    #[test]
    fn a_windows_line_ending_is_one_return() {
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb".to_vec());
        assert_eq!(encode_paste("a\rb", false), b"a\rb".to_vec());
        assert_eq!(encode_paste("a\n\nb", false), b"a\r\rb".to_vec());
    }

    /// The long-dispatch contract at the size that used to break it: an
    /// 80KB briefing crosses as ONE bracketed frame — opened before the
    /// first byte, closed after the last, the body's own escapes inert
    /// inside it, and both end markers intact. Orca proves this with a wire
    /// repro tool (`repro-orchestration-long-prompt.mjs`: frame present, no
    /// unframed line breaks, marker before the submit byte); this is the
    /// same verdict on our own encoder, where the property actually lives.
    #[test]
    fn an_eighty_kilobyte_paste_is_one_frame_with_nothing_alive_inside() {
        let marker = "marker_0123456789abcdef";
        let mut briefing = format!("LONG_PROMPT_START {marker}\n");
        while briefing.len() < 80 * 1024 {
            briefing.push_str(
                "This slice is long on purpose, and \x1b[201~ inside it \
                 must not close the envelope early.\n",
            );
        }
        briefing.push_str(&format!("LONG_PROMPT_END {marker}"));

        let bytes = encode_paste(&briefing, true);
        assert!(
            bytes.starts_with(b"\x1b[200~"),
            "the frame does not open first"
        );
        assert!(
            bytes.ends_with(b"\x1b[201~"),
            "the frame does not close last"
        );
        // The ONLY escapes on the wire are the envelope's own two: every
        // escape the body carried arrived as the visible glyph instead.
        let body = &bytes[6..bytes.len() - 6];
        assert!(
            !body.contains(&0x1b),
            "a live escape survived inside the frame"
        );
        // Nothing was cut on the way: the payload still holds the whole
        // briefing, both markers included.
        assert!(body.len() >= 80 * 1024, "the briefing was truncated");
        let text = std::str::from_utf8(body).expect("utf-8");
        assert!(text.starts_with(&format!("LONG_PROMPT_START {marker}")));
        assert!(text.ends_with(&format!("LONG_PROMPT_END {marker}")));
        // And no submit byte rides the frame — line feeds became returns a
        // TUI reads as pasted lines, and the ONE Enter that submits is the
        // window's own, sent after the envelope (`TeamWindow::paste`).
        assert!(!body.contains(&b'\n'), "a raw line feed survived");
    }
}
