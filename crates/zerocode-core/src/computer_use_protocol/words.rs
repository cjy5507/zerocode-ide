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

/// How many candidates an ambiguous answer names.
const NAMED_CANDIDATES: usize = 5;
/// Two lines share a row when their vertical centres are closer than this
/// share of the taller one's height — the same row of a form, read left to
/// right, even when OCR boxes the words a few points apart.
const SAME_ROW_SHARE: f64 = 0.5;

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

    fn center_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    /// Whether `other` shares this line's row: their vertical centres closer
    /// than half the taller one.
    fn shares_row(&self, other: &Self) -> bool {
        (self.center_y() - other.center_y()).abs() < self.height.max(other.height) * SAME_ROW_SHARE
    }

    /// Whether `self` comes before `other` as a person reads the screen: an
    /// earlier row, or the same row further left.
    fn reads_before(&self, other: &Self) -> bool {
        if self.shares_row(other) {
            self.x < other.x
        } else {
            self.center_y() < other.center_y()
        }
    }
}

/// The lines of a `readText --ocr` answer, in the order a person reads them:
/// top to bottom by row (a row starts at the first line its successors share
/// it with), left to right inside a row.
#[must_use]
pub fn lines(answer: &Value) -> Vec<ReadLine> {
    let read: Vec<ReadLine> = answer
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| lines.iter().filter_map(ReadLine::from_value).collect())
        .unwrap_or_default();
    reading_order(read)
}

#[must_use]
pub fn reading_order(mut read: Vec<ReadLine>) -> Vec<ReadLine> {
    read.sort_by(|a, b| a.center_y().total_cmp(&b.center_y()));
    let mut rows: Vec<Vec<ReadLine>> = Vec::new();
    for line in read {
        match rows.last_mut() {
            Some(row) if row[0].shares_row(&line) => row.push(line),
            _ => rows.push(vec![line]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.x.total_cmp(&b.x));
            row
        })
        .collect()
}

/// Words as a person reads them: case and runs of space do not matter.
fn plain(words: &str) -> String {
    words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The line a press on `words` means among `lines` (in reading order): the
/// first that reads them after the line reading `after`, when given;
/// otherwise the only line that reads them, or the only one that reads them
/// exactly. Refused by name when none or several do.
///
/// # Errors
///
/// `element_not_found` when no line reads the words (or the anchor), and
/// `ambiguous_target` naming up to five candidates when several do.
pub fn choose<'a>(
    lines: &'a [ReadLine],
    words: &str,
    after: Option<&str>,
) -> Result<&'a ReadLine, ProviderError> {
    let wanted = plain(words);
    if wanted.is_empty() {
        return Err(ProviderError::invalid_argument(
            "--text must read something",
        ));
    }
    let reading: Vec<&ReadLine> = lines
        .iter()
        .filter(|line| plain(&line.text).contains(&wanted))
        .collect();
    let exact: Vec<&ReadLine> = reading
        .iter()
        .copied()
        .filter(|line| plain(&line.text) == wanted)
        .collect();
    if let Some(anchor_words) = after {
        let anchor_words = plain(anchor_words);
        let anchor = lines
            .iter()
            .find(|line| plain(&line.text).contains(&anchor_words))
            .ok_or_else(|| {
                ProviderError::new(
                    error_code::ELEMENT_NOT_FOUND,
                    format!("no line on the screen reads '{anchor_words}' to look after"),
                )
            })?;
        let following =
            |pool: &[&'a ReadLine]| pool.iter().copied().find(|line| anchor.reads_before(line));
        return following(&exact)
            .or_else(|| following(&reading))
            .ok_or_else(|| {
                ProviderError::new(
                    error_code::ELEMENT_NOT_FOUND,
                    format!("no line after '{anchor_words}' reads '{wanted}'"),
                )
            });
    }
    match (exact.as_slice(), reading.as_slice()) {
        ([one], _) | ([], [one]) => Ok(*one),
        (_, []) => Err(ProviderError::new(
            error_code::ELEMENT_NOT_FOUND,
            format!(
                "no line on the screen reads '{wanted}' ({} lines read: {})",
                lines.len(),
                lines
                    .iter()
                    .take(NAMED_CANDIDATES)
                    .map(|line| format!("'{}'", line.text))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        ([], many) | (many, _) => Err(ProviderError::new(
            error_code::AMBIGUOUS_TARGET,
            format!(
                "{} lines read '{wanted}': {} — name the line it follows with after-text, or press a coordinate",
                many.len(),
                many.iter()
                    .take(NAMED_CANDIDATES)
                    .map(|line| {
                        let (x, y) = line.center();
                        format!("'{}' at ({x:.0}, {y:.0})", line.text)
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )),
    }
}

#[cfg(test)]
mod tests;
