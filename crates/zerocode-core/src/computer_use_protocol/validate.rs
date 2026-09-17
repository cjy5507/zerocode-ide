//! Argument checks every provider makes with the same words
//! (`ActionArgumentValidation`, `KeyboardInputSafety` from the macOS helper).

use super::{ProviderError, bounded_integer, error_code};

/// `clickCount` and friends: absent means the default, present must be a
/// positive integer.
pub fn positive_integer(
    value: Option<f64>,
    default: i64,
    name: &str,
) -> Result<i64, ProviderError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match bounded_integer::<i64>(value) {
        Some(parsed) if value.is_finite() && value > 0.0 => Ok(parsed),
        _ => Err(ProviderError::invalid_argument(format!(
            "{name} must be a positive integer"
        ))),
    }
}

/// `pages`: absent means the default, present must be a positive number.
pub fn positive_number(value: Option<f64>, default: f64, name: &str) -> Result<f64, ProviderError> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(ProviderError::invalid_argument(format!(
            "{name} must be a positive number"
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    /// Absent means left.
    pub fn parse(value: Option<&str>) -> Result<Self, ProviderError> {
        match value {
            None => Ok(Self::Left),
            Some("left") => Ok(Self::Left),
            Some("right") => Ok(Self::Right),
            Some("middle") => Ok(Self::Middle),
            Some(other) => Err(ProviderError::invalid_argument(format!(
                "unsupported mouse button '{other}'"
            ))),
        }
    }

    /// A press and a context menu have accessibility equivalents; a middle
    /// click does not and must reach the app as real events.
    #[must_use]
    pub const fn has_accessibility_action(self) -> bool {
        !matches!(self, Self::Middle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    pub fn parse(value: &str) -> Result<Self, ProviderError> {
        match value {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            other => Err(ProviderError::invalid_argument(format!(
                "unsupported scroll direction: {other}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// `AXScrollUpByPage` / `ScrollUpByPage`: the page action named for this
    /// direction, in whichever vocabulary the element advertised.
    #[must_use]
    pub fn page_action(self, advertised: &[String]) -> Option<&str> {
        let capitalized = match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
        };
        let bare = format!("Scroll{capitalized}ByPage");
        advertised
            .iter()
            .find(|action| action.strip_prefix("AX").unwrap_or(action) == bare)
            .map(String::as_str)
    }
}

/// Why synthetic keyboard input was refused, if it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusFailure {
    TargetNotFocused,
    TargetNotFocusedAfterRestore,
}

/// Synthetic keys go to whichever window is focused, so the target must be.
#[must_use]
pub fn synthetic_input_focus_failure(
    target_window_focused: bool,
    restore_window_requested: bool,
) -> Option<FocusFailure> {
    if target_window_focused {
        return None;
    }
    Some(if restore_window_requested {
        FocusFailure::TargetNotFocusedAfterRestore
    } else {
        FocusFailure::TargetNotFocused
    })
}

impl FocusFailure {
    /// The `window_not_focused` sentence for an app named `app_name`.
    #[must_use]
    pub fn message(self, app_name: &str) -> String {
        match self {
            Self::TargetNotFocused => format!(
                "keyboard input requires the target {app_name} window to be focused; retry with --restore-window or use set-value for editable elements"
            ),
            Self::TargetNotFocusedAfterRestore => format!(
                "keyboard input requires the target {app_name} window to be focused; --restore-window was requested but the target is still not focused; bring it forward manually or check Accessibility permissions"
            ),
        }
    }
}

/// What the helper knows about the keys it is about to post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretEntryFacts {
    /// The keys carry text (`type`, `type-text`, `paste-text`).
    pub writes_text: bool,
    /// The keys are the paste chord — the clipboard's text, typed.
    pub pastes: bool,
    /// The focused element is the platform's own secret field (on macOS the
    /// `AXSecureTextField` subrole) — a hard signal, never a label's word.
    pub secure_field: bool,
    /// Some process holds the keyboard's secure mode.
    pub secure_input_on: bool,
    /// The process that holds it is the one the keys go to — a browser or an
    /// `NSSecureTextField` turns it on for its own focused password field.
    pub secure_input_by_receiver: bool,
    /// The focused element could be read (else `secure_field` is a guess).
    pub focus_read: bool,
}

/// What the helper does with those keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretEntryVerdict {
    /// Post them.
    Clear,
    /// Post them, say who holds the keyboard's secure mode, and let the
    /// landing check say whether they reached the field.
    Watched,
    /// Refuse them before anything is posted: the person types a secret.
    PersonsEntry,
}

impl SecretEntryVerdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Watched => "watched",
            Self::PersonsEntry => "persons_entry",
        }
    }
}

/// Keys never write a secret (B3): text or a paste into the platform's
/// secret field is the person's to type — and so is text while the keyboard's
/// secure mode is held by the very app the keys go to, or held while the
/// focus cannot be read (that mode is how a password field the tree does not
/// show still says it is one). Keys that carry no text are never refused — a
/// Tab or a Return is how a person leaves such a field. While another process
/// holds secure mode, keys are posted and watched: whether they landed is the
/// field's readback's to say.
#[must_use]
pub const fn secret_entry(facts: SecretEntryFacts) -> SecretEntryVerdict {
    let secret = facts.secure_field
        || (facts.secure_input_on && (facts.secure_input_by_receiver || !facts.focus_read));
    if (facts.writes_text || facts.pastes) && secret {
        SecretEntryVerdict::PersonsEntry
    } else if facts.secure_input_on {
        SecretEntryVerdict::Watched
    } else {
        SecretEntryVerdict::Clear
    }
}

/// A refusal the helper spelled `secret_field: <app>[; typed <i> of <n>]`,
/// as the model reads it: whose field, how much went in, what to do.
#[must_use]
pub fn secure_input_sentence(message: &str) -> Option<String> {
    let mut parts = message.split("; ");
    let app = parts
        .next()?
        .strip_prefix(super::unverified_reason::SECRET_FIELD)?
        .strip_prefix(": ")?;
    let typed = parts.find_map(|part| {
        let (done, total) = part.strip_prefix("typed ")?.split_once(" of ")?;
        Some((done.parse::<usize>().ok()?, total.parse::<usize>().ok()?))
    });
    let mut sentence = format!(
        "the focused field in {app} is a password field — the person types it: hand it to them (`handoff`), then look again. Do not retry, paste or put it on the clipboard; only when the user gave you this secret for this field, write it with `set-value --value-stdin` on its element"
    );
    if let Some((done, total)) = typed {
        sentence.push_str(&format!(
            ". {done} of {total} characters went in before the focus moved into it — look at the form before going on"
        ));
    }
    Some(sentence)
}

/// A helper's terse refusal as the model reads it: a password field's
/// (`secure_input`) and a line break the typing would have pressed with
/// (`invalid_argument`, `line_break: <app>`). None for every other refusal,
/// which already speaks for itself.
#[must_use]
pub fn refusal_sentence(code: &str, message: &str) -> Option<String> {
    if code == error_code::SECURE_INPUT {
        return secure_input_sentence(message);
    }
    let line_break = TextEntryRefusal::LineBreak;
    (code == error_code::INVALID_ARGUMENT
        && message
            .strip_prefix(line_break.as_str())
            .is_some_and(|rest| rest.starts_with(':')))
    .then(|| line_break.message().to_string())
}

/// The characters that can carry typed keys into another field — a Tab to
/// the next one, a Return or a line break to a default button and on: text
/// is typed in pieces cut after each, and the secret-field check runs again
/// on whatever holds the focus then. The Swift helper holds the same set.
pub const FOCUS_MOVING_CHARS: &[char] = &['\t', '\r', '\n'];
/// The characters of them that press: a Return or a line break in a field
/// that is not a multi-line text area submits it, or fires its default
/// button — a press the typing road never asks the person about.
pub const PRESSING_CHARS: &[char] = &['\r', '\n'];

/// Why a text is not typed as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEntryRefusal {
    /// A line break outside a multi-line area would press: `key return` is
    /// the press, and it is asked about.
    LineBreak,
}

impl TextEntryRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LineBreak => "line_break",
        }
    }

    /// The refusal as the model reads it.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LineBreak => {
                "a line break in the text would press this field's default button unasked — type the text, then `key --key return` (a press the person may be asked about), then type the rest"
            }
        }
    }
}

/// How a text is typed (B3): into a multi-line text area, as one piece — its
/// Tabs and line breaks are content; anywhere else, refused when it holds a
/// line break (`TextEntryRefusal::LineBreak`), else cut after each Tab so the
/// secret-field check runs again where the focus went.
pub fn text_entry_plan(text: &str, multi_line: bool) -> Result<Vec<&str>, TextEntryRefusal> {
    if multi_line {
        return Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![text]
        });
    }
    if text.contains(PRESSING_CHARS) {
        return Err(TextEntryRefusal::LineBreak);
    }
    Ok(focus_moving_pieces(text))
}

/// Text cut after every focus-moving character, each piece keeping its own:
/// `"alice\thunter2"` is `["alice\t", "hunter2"]`.
#[must_use]
pub fn focus_moving_pieces(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    for (at, glyph) in text.char_indices() {
        if FOCUS_MOVING_CHARS.contains(&glyph) {
            let end = at + glyph.len_utf8();
            pieces.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A case table's text cell: `\t`, `\r`, `\n` and `\\` spelled out, so a
    /// row stays one line.
    fn unescape(cell: &str) -> String {
        let mut out = String::new();
        let mut glyphs = cell.chars();
        while let Some(glyph) = glyphs.next() {
            if glyph != '\\' {
                out.push(glyph);
                continue;
            }
            match glyphs.next() {
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        out
    }

    fn escape(text: &str) -> String {
        text.replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
    }

    /// The one table the Swift mirror runs too.
    #[test]
    fn keys_never_write_a_secret_by_the_shared_case_table() {
        let table = include_str!("cases/secret_entry.tsv");
        let flag = |word: &str| match word {
            "true" => true,
            "false" => false,
            other => panic!("not a flag: {other}"),
        };
        let mut rows = 0;
        for line in table
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        {
            let cells: Vec<&str> = line.split('\t').collect();
            let facts = SecretEntryFacts {
                writes_text: flag(cells[0]),
                pastes: flag(cells[1]),
                secure_field: flag(cells[2]),
                secure_input_on: flag(cells[3]),
                secure_input_by_receiver: flag(cells[4]),
                focus_read: flag(cells[5]),
            };
            assert_eq!(secret_entry(facts).as_str(), cells[6], "{line}");
            rows += 1;
        }
        assert!(rows >= 16, "the table was read: {rows}");
        let table = include_str!("cases/text_entry.tsv");
        let mut rows = 0;
        for line in table
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        {
            let cells: Vec<&str> = line.split('\t').collect();
            let text = unescape(cells[0]);
            let planned = match text_entry_plan(&text, flag(cells[1])) {
                Ok(pieces) => pieces
                    .iter()
                    .map(|piece| escape(piece))
                    .collect::<Vec<_>>()
                    .join("|"),
                Err(refusal) => format!("refused:{}", refusal.as_str()),
            };
            assert_eq!(planned, cells[2], "{line}");
            rows += 1;
        }
        assert!(rows >= 8, "the text table was read: {rows}");
        assert_eq!(
            focus_moving_pieces("alice\thunter2"),
            ["alice\t", "hunter2"]
        );
        assert_eq!(focus_moving_pieces("a\r\nb\n"), ["a\r", "\n", "b\n"]);
        assert_eq!(focus_moving_pieces("plain"), ["plain"]);
        assert!(focus_moving_pieces("").is_empty());
        let refused = secure_input_sentence("secret_field: Safari").expect("the helper's words");
        assert!(
            refused.contains("Safari")
                && refused.contains("handoff")
                && !refused.contains("went in")
        );
        let midway =
            secure_input_sentence("secret_field: Mail; typed 6 of 13").expect("the helper's words");
        assert!(midway.contains("6 of 13"), "{midway}");
        assert_eq!(
            secure_input_sentence("app_blocked: x"),
            None,
            "another code's words pass through"
        );
        assert!(
            refusal_sentence(error_code::INVALID_ARGUMENT, "line_break: Safari")
                .is_some_and(|said| said.contains("key --key return"))
        );
        assert_eq!(
            refusal_sentence(error_code::INVALID_ARGUMENT, "line_breaking news"),
            None
        );
        assert_eq!(
            refusal_sentence(error_code::SECURE_INPUT, "secret_field: Mail"),
            secure_input_sentence("secret_field: Mail")
        );
    }

    // ---- ActionArgumentValidationTests.swift + KeyboardInputSafetyTests.swift ----

    #[test]
    fn positive_integers_accept_positive_values_and_defaults() {
        assert_eq!(positive_integer(None, 1, "clickCount").unwrap(), 1);
        assert_eq!(positive_integer(Some(2.0), 1, "clickCount").unwrap(), 2);
        for bad in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert_eq!(
                positive_integer(Some(bad), 1, "clickCount")
                    .unwrap_err()
                    .message,
                "clickCount must be a positive integer"
            );
        }
    }

    #[test]
    fn positive_numbers_accept_positive_values_and_defaults() {
        assert_eq!(positive_number(None, 1.0, "pages").unwrap(), 1.0);
        assert_eq!(positive_number(Some(0.5), 1.0, "pages").unwrap(), 0.5);
        for bad in [0.0, -0.5, f64::NAN] {
            assert_eq!(
                positive_number(Some(bad), 1.0, "pages")
                    .unwrap_err()
                    .message,
                "pages must be a positive number"
            );
        }
    }

    #[test]
    fn mouse_buttons_default_to_left_and_only_middle_lacks_an_accessibility_action() {
        assert_eq!(MouseButton::parse(None).unwrap(), MouseButton::Left);
        assert_eq!(
            MouseButton::parse(Some("right")).unwrap(),
            MouseButton::Right
        );
        assert_eq!(
            MouseButton::parse(Some("middle")).unwrap(),
            MouseButton::Middle
        );
        assert_eq!(
            MouseButton::parse(Some("primary")).unwrap_err().message,
            "unsupported mouse button 'primary'"
        );
        assert_eq!(
            MouseButton::parse(Some("")).unwrap_err().message,
            "unsupported mouse button ''"
        );
        assert!(MouseButton::Left.has_accessibility_action());
        assert!(MouseButton::Right.has_accessibility_action());
        assert!(!MouseButton::Middle.has_accessibility_action());
    }

    #[test]
    fn scroll_directions_reject_unknown_directions_and_name_their_page_action() {
        assert_eq!(
            ScrollDirection::parse("down").unwrap(),
            ScrollDirection::Down
        );
        assert_eq!(
            ScrollDirection::parse("diagonal").unwrap_err().message,
            "unsupported scroll direction: diagonal"
        );
        let mac = vec!["AXScrollDownByPage".to_string()];
        assert_eq!(
            ScrollDirection::Down.page_action(&mac),
            Some("AXScrollDownByPage")
        );
        let win = vec!["ScrollUpByPage".to_string()];
        assert_eq!(
            ScrollDirection::Up.page_action(&win),
            Some("ScrollUpByPage")
        );
        assert_eq!(ScrollDirection::Left.page_action(&win), None);
    }

    #[test]
    fn synthetic_input_requires_a_focused_target_window() {
        assert_eq!(synthetic_input_focus_failure(true, false), None);
        assert_eq!(synthetic_input_focus_failure(true, true), None);
        assert_eq!(
            synthetic_input_focus_failure(false, false),
            Some(FocusFailure::TargetNotFocused)
        );
        assert_eq!(
            synthetic_input_focus_failure(false, true),
            Some(FocusFailure::TargetNotFocusedAfterRestore)
        );
        assert!(
            FocusFailure::TargetNotFocused
                .message("Notes")
                .contains("retry with --restore-window")
        );
        assert!(
            FocusFailure::TargetNotFocusedAfterRestore
                .message("Notes")
                .contains("still not focused")
        );
    }
}
