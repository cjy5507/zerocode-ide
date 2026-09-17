//! The bare terminal's notification protocols — the `terminal` road of
//! `PushNotification` (t-2943, `docs/design/zo-push-notification.md` §1).
//!
//! Three sequences in one write, so every terminal hears the one it knows:
//! `BEL` for the terminal's own bell/badge, OSC 9 for iTerm2, and OSC 777
//! `notify` for `WezTerm`, kitty and the rxvt family. A terminal that knows
//! none of them swallows the OSCs whole and shows nothing — the same contract
//! the fold markers (`super::folds`) rely on — so nothing here can ever land
//! in the transcript as text.

/// The terminal bell.
pub const BEL: char = '\u{7}';

/// The bytes that ring a bare terminal for one notice. Pure, so the e2e can
/// look for exactly these in a pty capture.
#[must_use]
pub fn terminal_notification(title: &str, body: &str) -> String {
    let title = plain(title).replace(';', ",");
    let body = plain(body);
    format!("{BEL}\u{1b}]9;{body}{BEL}\u{1b}]777;notify;{title};{body}{BEL}")
}

/// One line with no control bytes — an ESC or BEL inside the payload would
/// end the sequence early and leak the rest onto the screen.
fn plain(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_sequences_carry_the_title_and_body_once_each() {
        let bytes = terminal_notification("zo · api", "Build is green");
        assert!(bytes.starts_with(BEL));
        assert!(bytes.contains("\u{1b}]9;Build is green\u{7}"));
        assert!(bytes.contains("\u{1b}]777;notify;zo · api;Build is green\u{7}"));
        assert_eq!(bytes.matches(BEL).count(), 3, "one BEL to ring, two to terminate");
    }

    #[test]
    fn control_bytes_and_a_separator_in_the_title_cannot_break_the_sequence() {
        let bytes = terminal_notification("a;b\u{1b}c", "one\ntwo\u{7}three");
        assert!(bytes.contains("]777;notify;a,b c;one two three\u{7}"));
        assert_eq!(bytes.matches(BEL).count(), 3);
        assert_eq!(bytes.matches('\u{1b}').count(), 2, "only the two OSC openers");
    }
}
