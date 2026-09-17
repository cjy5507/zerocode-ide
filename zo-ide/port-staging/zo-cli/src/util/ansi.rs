//! Single source of truth for ANSI escape handling.
//!
//! Two flavours of caller need slightly different post-processing:
//!
//! - [`strip_ansi`] — used by the markdown renderer to measure the
//!   *visible* width of a styled string. Drops every CSI sequence and
//!   leaves the remaining characters intact, including control bytes.
//! - [`sanitize_inline`] — used by the widget layer before handing a
//!   free-form descriptor (tool summary, system text, etc.) to a
//!   `ratatui::Span`. Drops CSI sequences *and* replaces any other
//!   control character with a space, since `Span` is a single-line
//!   primitive and embedded `\n`/`\r` would staircase the cursor in
//!   alt-screen + raw-mode terminals.
//!
//! Both functions share the [`skip_csi`] helper so the CSI grammar
//! lives in exactly one place — fix it here and both call sites benefit.

use std::iter::Peekable;
use std::str::Chars;

/// Remove ANSI CSI escape sequences (`ESC '[' ... letter`) from `input`,
/// leaving every other character — including non-CSI control bytes —
/// untouched.
#[must_use]
pub fn strip_ansi(input: &str) -> String {
    if !input.contains('\u{1b}') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            skip_csi(&mut chars);
            continue;
        }
        out.push(ch);
    }
    out
}

/// Drop ANSI CSI escape sequences *and* replace any remaining control
/// character with a single space, producing a string that is safe to
/// feed into a single-line `ratatui::Span`.
///
/// Fast path: if no control or escape byte is present at all, the
/// input is cheap-cloned without scanning the codepoint stream — keeps
/// the per-frame allocation budget low on clean ASCII rows.
#[must_use]
pub fn sanitize_inline(text: &str) -> String {
    if text.bytes().all(|b| b >= 0x20 && b != 0x7f) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            skip_csi(&mut chars);
            continue;
        }
        if ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// After consuming an `ESC`, advance `chars` past the rest of a CSI
/// sequence (`[` followed by parameter bytes terminated by an ASCII
/// letter). Tolerates the degenerate case of a lone `ESC` by leaving
/// `chars` untouched.
fn skip_csi(chars: &mut Peekable<Chars<'_>>) {
    if !matches!(chars.peek(), Some('[')) {
        return;
    }
    chars.next();
    for next in chars.by_ref() {
        if next.is_ascii_alphabetic() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_inline, strip_ansi};

    #[test]
    fn strip_ansi_removes_sgr_sequence() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
    }

    #[test]
    fn strip_ansi_passes_clean_input() {
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn sanitize_inline_drops_csi_body_and_esc() {
        // Regression: previously the ESC byte became a space and the
        // `[35;92;33M` payload leaked through as visible text.
        let raw = "\u{1b}[35;92;33Mhello\u{1b}[0m";
        assert_eq!(sanitize_inline(raw), "hello");
    }

    #[test]
    fn sanitize_inline_replaces_newline_with_space() {
        assert_eq!(sanitize_inline("a\nb"), "a b");
    }

    #[test]
    fn sanitize_inline_fast_path_returns_clean_input() {
        assert_eq!(sanitize_inline("plain ascii"), "plain ascii");
    }

    #[test]
    fn sanitize_inline_handles_lone_esc() {
        assert_eq!(sanitize_inline("a\u{1b}b"), "ab");
    }
}
