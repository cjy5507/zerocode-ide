//! Task titles.
//!
//! A dispatched task arrives as a free-form spec (often a whole paragraph, often
//! pasted). Two bounded, single-line strings are derived from it: a short
//! `task_title` for the lane rail and switcher, and a longer `display_name` for
//! the window title and the worktree branch summary.
//!
//! Truncation is by **character**, not byte. Slicing a UTF-8 string by byte
//! offset panics mid-codepoint, and Korean or emoji specs hit that immediately.
//! Character truncation can still split a multi-codepoint grapheme cluster (a
//! flag, a ZWJ family emoji) — the tests below pin that this degrades to a
//! shorter string rather than to invalid text.

use serde::{Deserialize, Serialize};

pub const TASK_TITLE_MAX_CHARS: usize = 80;
pub const DISPLAY_NAME_MAX_CHARS: usize = 160;

const ELLIPSIS: char = '…';

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeTask {
    pub task_title: String,
    pub display_name: String,
}

impl WorktreeTask {
    /// Derive both titles. Explicit values win; anything blank falls back to the
    /// first non-empty line of the spec, and `display_name` finally falls back
    /// to `task_title` so it is never empty when the spec has any content.
    pub fn from_spec(spec: &str, task_title: Option<&str>, display_name: Option<&str>) -> Self {
        let task_title =
            normalize_single_line(task_title.unwrap_or_default(), TASK_TITLE_MAX_CHARS)
                .unwrap_or_else(|| derive_title_from_spec(spec));
        let display_name =
            normalize_single_line(display_name.unwrap_or_default(), DISPLAY_NAME_MAX_CHARS)
                .unwrap_or_else(|| task_title.clone());
        Self {
            task_title,
            display_name,
        }
    }
}

fn derive_title_from_spec(spec: &str) -> String {
    spec.lines()
        .find_map(|line| normalize_single_line(line, TASK_TITLE_MAX_CHARS))
        .unwrap_or_default()
}

/// Strip escape sequences and neuter control characters, replacing each with a
/// space so the words on either side stay separate.
///
/// A title is written straight to a terminal — a lane rail, a window title,
/// `zerocode lane`'s own stderr — and a spec is untrusted text somebody pasted.
/// Left alone, `ESC]0;…BEL` retitles the user's window, `ESC[2J` clears it and
/// `\r` hides the rest of the line.
///
/// **Dropping the escape byte alone is not enough**, which is the mistake this
/// replaced. A sequence's parameters are ordinary printable characters, so
/// `ESC[2J` survived as the text `[2J`, and `ESC]0;…BEL` was worse than ugly:
/// everything between the introducer and the terminator is a string its sender
/// chose, and it became the name of the lane.
///
/// This is a matcher, not a parser: it never interprets a sequence, only finds
/// where one ends. That asymmetry is what makes it safe to keep small. Removing
/// too much costs a few characters of a title; removing too little still leaves
/// the escape byte gone, because anything not recognised as a sequence falls
/// through to the control-character rule below. Neither direction can put an
/// escape back on the wire.
fn disarm(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // `ESC [` (CSI) and `ESC ]`/`P`/`X`/`^`/`_` (the string sequences),
            // plus the two-character escapes like `ESC c`.
            '\u{1b}' => {
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        skip_control_sequence(&mut chars);
                    }
                    Some(']') => {
                        chars.next();
                        skip_string_sequence(&mut chars, StringEnd::BelOrSt);
                    }
                    Some('P' | 'X' | '^' | '_') => {
                        chars.next();
                        skip_string_sequence(&mut chars, StringEnd::StOnly);
                    }
                    // Everything else — `ESC c`, but also the three-character
                    // charset designators like `ESC ( B`.
                    Some(_) => skip_escape_sequence(&mut chars),
                    // A trailing `ESC` introduces nothing.
                    None => {}
                }
                out.push(' ');
            }
            // The same sequences spelled as single C1 codepoints, which a
            // terminal obeys identically.
            '\u{9b}' => {
                skip_control_sequence(&mut chars);
                out.push(' ');
            }
            '\u{9d}' => {
                skip_string_sequence(&mut chars, StringEnd::BelOrSt);
                out.push(' ');
            }
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                skip_string_sequence(&mut chars, StringEnd::StOnly);
                out.push(' ');
            }
            other if other.is_control() => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Consume a CSI body: parameter and intermediate bytes, then one final byte.
///
/// Stops early on anything outside that range rather than running away — an
/// unfinished sequence is malformed, and eating the rest of the line for it
/// would lose text a terminal would have shown.
fn skip_control_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&ch) = chars.peek() {
        match ch {
            // Parameter bytes `0..?` and intermediates ` ..;/`.
            '\u{20}'..='\u{3f}' => {
                chars.next();
            }
            // The final byte ends the sequence and belongs to it.
            '\u{40}'..='\u{7e}' => {
                chars.next();
                return;
            }
            _ => return,
        }
    }
}

/// Consume an escape that is neither `CSI` nor a string sequence: any run of
/// intermediate bytes, then one final byte.
///
/// Most such escapes are two characters (`ESC c`, `ESC 7`), but the charset
/// designators are three — `ESC ( B`, `ESC # 8`, `ESC % G` — and assuming a
/// pair left their final byte behind as text, so a spec of nothing but those
/// was named `B`.
///
/// Anything outside both ranges is not part of the escape and is left for the
/// caller to emit, so an `ESC` in front of ordinary words cannot eat them.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&ch) = chars.peek() {
        match ch {
            '\u{20}'..='\u{2f}' => {
                chars.next();
            }
            '\u{30}'..='\u{7e}' => {
                chars.next();
                return;
            }
            _ => return,
        }
    }
}

/// Which terminators end a string sequence.
///
/// `BEL` ends an `OSC` and nothing else — `DCS`, `SOS`, `PM` and `APC` run
/// until `ST`. Accepting `BEL` for all of them let a payload that happened to
/// contain one spill its remainder into the title.
#[derive(Clone, Copy)]
enum StringEnd {
    BelOrSt,
    StOnly,
}

/// Consume a string sequence body up to and including its terminator: `ESC \`
/// or the C1 `ST` always, and `BEL` as well when the sequence is an `OSC`.
///
/// An unterminated one swallows the rest, exactly as a terminal would — better
/// a short title than one holding a payload that was never closed.
fn skip_string_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, end: StringEnd) {
    while let Some(ch) = chars.next() {
        match ch {
            '\u{07}' if matches!(end, StringEnd::BelOrSt) => return,
            '\u{9c}' => return,
            '\u{1b}' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Collapse whitespace to single spaces, trim, and cap the length. Returns
/// `None` for input that is empty or whitespace-only, which is what makes the
/// fallback chain above readable.
///
/// Escape sequences are removed and control characters neutered first — see
/// [`disarm`].
fn normalize_single_line(value: &str, max_chars: usize) -> Option<String> {
    let disarmed = disarm(value);
    let collapsed = disarmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= max_chars {
        return Some(collapsed);
    }
    let mut out: String = collapsed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    while out.ends_with(' ') {
        out.pop();
    }
    out.push(ELLIPSIS);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_first_non_empty_line_of_the_spec() {
        let task =
            WorktreeTask::from_spec("\n\n  Fix the flaky drain test  \nmore detail", None, None);
        assert_eq!(task.task_title, "Fix the flaky drain test");
        assert_eq!(task.display_name, "Fix the flaky drain test");
    }

    #[test]
    fn explicit_values_win_and_whitespace_collapses() {
        let task = WorktreeTask::from_spec(
            "ignored spec",
            Some("  add   lane   rails  "),
            Some("Add lane rails\tto the sidebar"),
        );
        assert_eq!(task.task_title, "add lane rails");
        assert_eq!(task.display_name, "Add lane rails to the sidebar");
    }

    #[test]
    fn long_titles_are_capped_at_the_documented_lengths() {
        let spec = "x".repeat(500);
        let task = WorktreeTask::from_spec(&spec, None, Some(&spec));
        assert_eq!(task.task_title.chars().count(), TASK_TITLE_MAX_CHARS);
        assert_eq!(task.display_name.chars().count(), DISPLAY_NAME_MAX_CHARS);
        assert!(task.task_title.ends_with('…'));
    }

    #[test]
    fn korean_and_emoji_specs_truncate_without_corrupting_text() {
        let spec = "레인 레일 구현 ".repeat(40);
        let task = WorktreeTask::from_spec(&spec, None, None);
        assert_eq!(task.task_title.chars().count(), TASK_TITLE_MAX_CHARS);
        assert!(task.task_title.is_char_boundary(task.task_title.len()));

        let emoji = "🚀🛠️👩‍💻 ".repeat(60);
        let task = WorktreeTask::from_spec(&emoji, None, None);
        assert_eq!(task.task_title.chars().count(), TASK_TITLE_MAX_CHARS);
        // Round-tripping proves the bytes are still valid UTF-8 after the cut.
        let json = serde_json::to_string(&task).expect("serialize");
        assert_eq!(
            serde_json::from_str::<WorktreeTask>(&json).expect("deserialize"),
            task
        );
    }

    #[test]
    fn a_blank_spec_yields_blank_titles_rather_than_a_panic() {
        let task = WorktreeTask::from_spec("   \n\t\n", None, None);
        assert_eq!(task.task_title, "");
        assert_eq!(task.display_name, "");
    }

    /// A title is written straight to a terminal — a lane header, a window
    /// title, `zerocode lane`'s own stderr. A spec is untrusted text, and an
    /// escape sequence surviving into it is a terminal injection: `ESC]0;…BEL`
    /// retitles the user's window, `ESC[2J` clears it, and a `\r` hides
    /// everything before it on the line.
    ///
    /// ESC is not whitespace, so collapsing whitespace does not remove it. The
    /// control characters have to go on their own.
    #[test]
    fn control_characters_never_survive_into_a_title() {
        let hostile = "\x1b]0;pwned\x07 fix \x1b[2J the \x1b[31mdrain\x1b[0m gate\r\x08\x7f\u{9b}";
        let task = WorktreeTask::from_spec(hostile, None, None);

        assert!(
            !task.task_title.chars().any(char::is_control),
            "a control character survived: {:?}",
            task.task_title
        );
        assert!(
            !task.display_name.chars().any(char::is_control),
            "a control character survived: {:?}",
            task.display_name
        );
        // The readable words are kept; only the control bytes are neutered.
        assert!(task.task_title.contains("fix"), "{:?}", task.task_title);
        assert!(task.task_title.contains("gate"), "{:?}", task.task_title);
    }

    /// The same guard on the explicit-value path, which does not go through the
    /// spec at all.
    #[test]
    fn an_explicit_title_is_neutered_too() {
        let task = WorktreeTask::from_spec(
            "ignored",
            Some("\x1b]0;pwned\x07lane one"),
            Some("\x1b[2Jdisplay name"),
        );
        assert!(!task.task_title.chars().any(char::is_control));
        assert!(!task.display_name.chars().any(char::is_control));
        assert!(
            task.task_title.contains("lane one"),
            "{:?}",
            task.task_title
        );
    }

    /// Neutering must not touch the scripts this product is written for.
    #[test]
    fn korean_emoji_and_combining_marks_are_left_alone() {
        let task = WorktreeTask::from_spec("드레인 게이트 🚀 e\u{301} 안녕", None, None);
        assert_eq!(task.task_title, "드레인 게이트 🚀 e\u{301} 안녕");
    }

    /// A spec that is nothing but control characters has no title in it, and
    /// must degrade the same way a blank spec does rather than to a row of
    /// spaces.
    #[test]
    fn a_spec_of_only_control_characters_yields_a_blank_title() {
        let task = WorktreeTask::from_spec("\x1b\x07\x08\x7f\u{9b}", None, None);
        assert_eq!(task.task_title, "");
        assert_eq!(task.display_name, "");
    }

    /// Dropping the escape *byte* alone is not enough. A sequence's parameters
    /// are ordinary printable characters, so `ESC[2J` would survive as the text
    /// `[2J` — and `ESC]0;…BEL` is worse than ugly, because the payload between
    /// the introducer and the terminator is a string the sender chose. Removing
    /// the sequence whole is what stops it from naming the lane.
    #[test]
    fn an_escape_sequence_leaves_no_residue_behind_it() {
        assert_eq!(
            WorktreeTask::from_spec("\x1b[2J\x1b[31m", None, None).task_title,
            ""
        );
        assert_eq!(
            WorktreeTask::from_spec("\x1b]0;a string the sender chose\x07", None, None).task_title,
            ""
        );
        // The words around the sequences survive; only the sequences go.
        assert_eq!(
            WorktreeTask::from_spec("\x1b[1;31mred\x1b[0m gate", None, None).task_title,
            "red gate"
        );
    }

    /// `ESC \` and the C1 single-byte introducers say the same things in fewer
    /// bytes, and a terminal obeys them identically — so they cannot be the hole
    /// the residue comes back through.
    #[test]
    fn the_shorter_spellings_of_the_same_sequences_are_stripped_too() {
        // U+009B is CSI in one codepoint; U+009D is OSC.
        assert_eq!(
            WorktreeTask::from_spec("\u{9b}31mred", None, None).task_title,
            "red"
        );
        assert_eq!(
            WorktreeTask::from_spec("\u{9d}0;title\x07after", None, None).task_title,
            "after"
        );
        // A string terminator rather than BEL.
        assert_eq!(
            WorktreeTask::from_spec("\x1b]0;t\x1b\\kept", None, None).task_title,
            "kept"
        );
    }

    /// An unterminated string sequence swallows the rest, exactly as a terminal
    /// would — better a short title than one holding a payload that was never
    /// closed.
    #[test]
    fn an_unterminated_sequence_consumes_what_follows_it() {
        assert_eq!(
            WorktreeTask::from_spec("before \x1b]0;never ends", None, None).task_title,
            "before"
        );
    }

    /// Not every escape is two characters. The charset designators carry an
    /// intermediate byte before their final one — `ESC ( B` selects ASCII,
    /// `ESC # 8` fills the screen with `E` — so treating every escape as a pair
    /// left the final byte behind as text, and a spec of nothing but those was
    /// named `B` or `8`.
    #[test]
    fn escapes_with_intermediate_bytes_leave_no_final_byte_behind() {
        for spec in ["\x1b(B", "\x1b)0", "\x1b#8", "\x1b%G", "\x1b(B\x1b#8"] {
            assert_eq!(
                WorktreeTask::from_spec(spec, None, None).task_title,
                "",
                "residue from {spec:?}"
            );
        }
        // The two-character forms still stop at their final byte.
        assert_eq!(
            WorktreeTask::from_spec("\x1b7saved\x1b8", None, None).task_title,
            "saved"
        );
        // And real words on either side survive.
        assert_eq!(
            WorktreeTask::from_spec("\x1b(Bdrain gate", None, None).task_title,
            "drain gate"
        );
    }

    /// `BEL` ends an `OSC` and nothing else. `DCS`, `SOS`, `PM` and `APC` run
    /// until `ST`, so accepting `BEL` for all of them let a payload containing
    /// one spill its remainder into the title.
    #[test]
    fn only_an_osc_ends_at_a_bell() {
        assert_eq!(
            WorktreeTask::from_spec("\x1bPq\x07leaked\x1b\\", None, None).task_title,
            ""
        );
        assert_eq!(
            WorktreeTask::from_spec("\u{90}q\x07leaked\u{9c}", None, None).task_title,
            ""
        );
        // The `ST` that really does end it lets the text after through.
        assert_eq!(
            WorktreeTask::from_spec("\x1bPq\x1b\\kept", None, None).task_title,
            "kept"
        );
    }

    /// An escape before text we do not recognise as a sequence must not eat it.
    /// Only the intermediate range may repeat; anything else ends the escape
    /// immediately.
    #[test]
    fn an_escape_before_ordinary_text_consumes_none_of_it() {
        assert_eq!(
            WorktreeTask::from_spec("\x1b\u{9c}드레인 게이트", None, None).task_title,
            "드레인 게이트"
        );
    }
}
