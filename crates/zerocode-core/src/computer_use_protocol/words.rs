//! A press by the words a screen shows (t-37883).
//!
//! A window with no tree to name a control by — a phone mirrored on the
//! Mac, a remote desktop, a game's menu — still shows its controls' words,
//! and the helper reads them (`readText --ocr`: lines in screen points). A
//! batch step that names its control by those words is read again at the
//! press, after whatever the steps before it scrolled or opened, so a plan
//! for a whole screen needs no look between its steps — the pixel twin of the
//! tree's click by reading (`QUERY_CLICK_KEYS`).
//!
//! This module chooses the line a press means, by the rule the tree's click
//! keeps (the helper's `chosenIndex`): the one line that reads the words, or
//! the one that reads them exactly; none or several is answered by name, and
//! the press never guesses between two. Words that recur down a form ("Yes",
//! "No", "Select") are told apart by the line they follow (`after`).

use serde_json::Value;

use super::{ProviderError, error_code};

/// One line the helper read, in screen points.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ReadLine {
    /// A line as the helper renders it (`renderLine`): its text and frame.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let number = |key: &str| value.get(key).and_then(Value::as_f64);
        Some(Self {
            text: value.get("text")?.as_str()?.to_string(),
            x: number("x")?,
            y: number("y")?,
            width: number("width")?,
            height: number("height")?,
        })
    }

    /// The point a press on this line lands on.
    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// The lines of a `readText --ocr` answer (red: as the helper listed them).
#[must_use]
pub fn lines(answer: &Value) -> Vec<ReadLine> {
    answer
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(ReadLine::from_value).collect())
        .unwrap_or_default()
}

/// The line a press on `words` means (red: none is chosen yet).
///
/// # Errors
///
/// Always, until the choice is written.
pub fn choose<'a>(
    lines: &'a [ReadLine],
    words: &str,
    after: Option<&str>,
) -> Result<&'a ReadLine, ProviderError> {
    let _ = (lines, after);
    Err(ProviderError::new(
        error_code::ELEMENT_NOT_FOUND,
        format!("no line is chosen for '{words}' yet"),
    ))
}

#[cfg(test)]
mod tests;
