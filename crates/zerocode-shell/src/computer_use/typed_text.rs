//! What `type-text` sends for the characters a keyboard has KEYS for —
//! decided once, away from `SendInput`, so a machine without the platform
//! can still prove it.
//!
//! The text an agent types is literal: it lands in the focused field and
//! nowhere else. Two characters put that at risk on Windows, because the
//! keys that produce them mean something to the window around the field as
//! well: the Enter KEY (`VK_RETURN`) is what a dialog manager turns into the
//! default button and a browser into a form submission, and the Tab KEY
//! (`VK_TAB`) is what moves focus to the next control. A `KEYEVENTF_UNICODE`
//! event posts a CHARACTER, not a key — the window sees `WM_KEYDOWN
//! VK_PACKET` plus `WM_CHAR`, and neither `IsDialogMessage` nor a browser's
//! key handling reads a packet as Enter or Tab — so typing stays inside the
//! field. What the field then does with the character is the field's own
//! Enter-as-text: a Win32 edit inserts a line break for the carriage return
//! (the character the Enter key produces as text) and inserts a tab for the
//! tab character; a single-line edit keeps its one line and does not press
//! anything.
//!
//! So a line break in the text — `\n`, `\r\n` or a bare `\r` — is sent as
//! ONE carriage return unit, never as the Enter key and never as a bare
//! line feed (which Windows text controls do not treat as a line break and
//! drop or box); a tab is sent as itself. macOS keeps the same literal
//! contract with its own convention (`keyboardSetUnicodeString` posts the
//! line feed, which Cocoa text views read as a newline).

/// The UTF-16 units `typeText` posts for `text`, one key down and up each.
#[must_use]
pub(super) fn keystroke_units(text: &str) -> Vec<u16> {
    const CARRIAGE_RETURN: u16 = 0x0D;
    let mut units = Vec::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                units.push(CARRIAGE_RETURN);
            }
            '\n' => units.push(CARRIAGE_RETURN),
            other => {
                let mut pair = [0u16; 2];
                units.extend_from_slice(other.encode_utf16(&mut pair));
            }
        }
    }
    units
}

/// How many whole characters of `text` are covered by its first `units`
/// keystroke units — for saying how much of a text arrived when the
/// platform refused the rest.
#[must_use]
pub(super) fn characters_within(text: &str, units: usize) -> usize {
    let mut used = 0;
    let mut characters = 0;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        let width = match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    characters += 1;
                }
                1
            }
            other => other.len_utf16(),
        };
        used += width;
        if used > units {
            break;
        }
        characters += 1;
    }
    characters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_break_is_one_carriage_return_and_a_tab_is_itself() {
        assert_eq!(keystroke_units("a\nb"), vec![0x61, 0x0D, 0x62]);
        assert_eq!(keystroke_units("a\r\nb"), vec![0x61, 0x0D, 0x62]);
        assert_eq!(keystroke_units("a\rb"), vec![0x61, 0x0D, 0x62]);
        assert_eq!(keystroke_units("\n\n"), vec![0x0D, 0x0D]);
        assert_eq!(keystroke_units("\r\n\r\n"), vec![0x0D, 0x0D]);
        assert_eq!(keystroke_units("x\ty"), vec![0x78, 0x09, 0x79]);
        assert!(
            !keystroke_units("a\nb").contains(&0x0A),
            "a bare line feed is not what a Windows edit calls a line break"
        );
    }

    #[test]
    fn every_other_character_is_itself_including_surrogate_pairs() {
        assert_eq!(keystroke_units("한"), vec![0xD55C]);
        assert_eq!(keystroke_units("😀"), vec![0xD83D, 0xDE00]);
        assert_eq!(keystroke_units(""), Vec::<u16>::new());
        let mixed = keystroke_units("a😀\r\n한\t");
        assert_eq!(mixed, vec![0x61, 0xD83D, 0xDE00, 0x0D, 0xD55C, 0x09]);
    }

    #[test]
    fn the_characters_that_arrived_are_counted_by_their_units() {
        assert_eq!(characters_within("abc", 0), 0);
        assert_eq!(characters_within("abc", 2), 2);
        assert_eq!(characters_within("abc", 3), 3);
        assert_eq!(characters_within("abc", 9), 3);
        // An emoji is two units; one unit accepted is not a whole emoji.
        assert_eq!(characters_within("a😀b", 2), 1);
        assert_eq!(characters_within("a😀b", 3), 2);
        // A CRLF is two characters sent as one unit.
        assert_eq!(characters_within("a\r\nb", 2), 3);
        assert_eq!(characters_within("a\r\nb", 3), 4);
    }
}
