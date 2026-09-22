//! A seat's last few decisions, read off its own rows — what the dashboard
//! lists under "recent" beside the numbers `summary` counts
//! (docs/design/jev-dashboard-and-perfection-20260921.md §2 (b)).
//!
//! One reader, for the reason `summary` is one: eleven seats write eleven
//! row shapes, and a screen that read them itself would be a twelfth
//! opinion about what each key means. What a row was asked, what it
//! answered, whether that answer was acted on and what the seat's own writer
//! later said of it are read here, by the keys the writers already share,
//! and handed on as a digest a screen can draw without knowing any seat.
//!
//! The digest carries no request body: the rows never held one. What it
//! carries of a row is the row's own bookkeeping — an id, a goal sentence a
//! person typed, a count of options — cut to [`ASKED_VALUE_CHARS`].

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::jev::summary::{
    AGREED, APPLIED, AT, CACHED, ELAPSED_MS, LedgerKey, OUTCOME, PRESSED, applied_of,
    asked_something,
};

/// What the seat answered with, when the answer is one word: a control's
/// mark, a room, an agent, an effort move — or, on the stall seat's rows,
/// the cause it named, which that seat spells as what it is.
pub const CHOSEN: LedgerKey = LedgerKey {
    canonical: "chosen",
    also: &["cause"],
};
/// How sure the judgment was of that word, in `[0, 1]`.
pub const CONFIDENCE: LedgerKey = LedgerKey {
    canonical: "confidence",
    also: &[],
};
/// The routing judgment's answer: one choice per axis (`complexity`, `risk`,
/// `intent`), each an object with its own `choice`.
pub const JUDGMENT: LedgerKey = LedgerKey {
    canonical: "jev",
    also: &[],
};
/// The recall judgment's answer: what moved, and whether the top changed.
pub const JUDGED: LedgerKey = LedgerKey {
    canonical: "judged",
    also: &[],
};
/// A label row's key — the request it labels, spelled the way that request
/// spelled it on its own row (`stall`, `effort`: `<dispatch>@<at>`).
pub const LABEL: LedgerKey = LedgerKey {
    canonical: "label",
    also: &[],
};
/// What the coordinator did next, on a label row.
pub const FOLLOWED: LedgerKey = LedgerKey {
    canonical: "followed",
    also: &[],
};

/// The keys under an axis of a routing judgment that name its choice.
const AXIS_CHOICE: &str = "choice";

/// Every key this module reads that `summary` does not, so a contract can
/// walk them.
pub const DECISION_KEYS: &[LedgerKey] = &[CHOSEN, CONFIDENCE, JUDGMENT, JUDGED, LABEL, FOLLOWED];

/// The row facts a digest carries as "what was asked", in the order a screen
/// lists them — the request's own bookkeeping, never its body.
///
/// A table rather than "every other key" because a row also carries the
/// answer's probabilities, the wire's byte counts and the seat's mode, and
/// a screen listing those beside a task id would be listing the ledger.
pub const ASKED_KEYS: &[&str] = &[
    "task",
    "worker",
    "run",
    "dispatch",
    "agent",
    "model",
    crate::summon_choice::WORKER_MODEL_KEY,
    "flow",
    "errand",
    "attempt",
    "candidates",
    "options",
    "offered",
    "panes",
    "inFront",
    "briefChars",
    "quietMs",
];

/// The most of one asked value a digest carries. A walk's goal sentence is
/// a person's own words and can run long; a digest is a line.
pub const ASKED_VALUE_CHARS: usize = 80;

/// One request, as the dashboard lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// When it was asked, in milliseconds since the epoch.
    pub at: i64,
    /// What became of it — [`crate::jev::summary::ANSWERED`], a failure
    /// token or a door's refusal token.
    pub outcome: String,
    /// The call's wall time; absent on a row nothing was sent for.
    pub elapsed_ms: Option<u64>,
    /// Whether the answer came from the memo rather than the wire.
    pub cached: bool,
    /// The request's own facts, by the row's key names ([`ASKED_KEYS`]).
    pub asked: Map<String, Value>,
    /// What it answered: a word ([`CHOSEN`]), an object of axis → choice
    /// (routing), an object of what moved (recall), or `Null`.
    pub answered: Value,
    pub confidence: Option<f64>,
    /// Whether the answer was acted on ([`applied_of`]).
    pub applied: Option<bool>,
    /// Whether the seat's own writer later marked the answer right — on the
    /// row itself, or on a label row that names it.
    pub agreed: Option<bool>,
    /// What followed, when a label row said.
    pub followed: Option<String>,
}

/// The last `n` requests of a ledger, newest first, each read with the label
/// row that names it when one was written later.
///
/// Newest first because the list is read from the top and the top is now;
/// the counters read the same rows oldest first because a window is a
/// count from the end, and neither order is the file's business.
#[must_use]
pub fn recent(rows: &[Value], n: usize) -> Vec<Decision> {
    if n == 0 {
        return Vec::new();
    }
    let labels = label_rows(rows);
    rows.iter()
        .rev()
        .filter(|row| asked_something(row).is_some())
        .take(n)
        .map(|row| digest(row, &labels))
        .collect()
}

/// Every label row, by the key it names — the seats that learn what
/// followed only later (stall, effort) write it as a row of its own.
fn label_rows(rows: &[Value]) -> HashMap<&str, &Value> {
    rows.iter()
        .filter(|row| asked_something(row).is_none())
        .filter_map(|row| Some((LABEL.read(row)?.as_str()?, row)))
        .collect()
}

/// The label row that names `row`, if any: a request row spells its own key
/// under the seat's name (`stall`, `effort`), so the row is asked for any
/// string a label names.
fn label_of<'rows>(row: &Value, labels: &HashMap<&str, &'rows Value>) -> Option<&'rows Value> {
    if labels.is_empty() {
        return None;
    }
    row.as_object()?
        .values()
        .filter_map(Value::as_str)
        .find_map(|word| labels.get(word).copied())
}

fn digest(row: &Value, labels: &HashMap<&str, &Value>) -> Decision {
    let label = label_of(row, labels);
    let on_either = |key: LedgerKey| {
        key.read(row)
            .or_else(|| label.and_then(|late| key.read(late)))
    };
    Decision {
        at: AT.read(row).and_then(Value::as_i64).unwrap_or(0),
        outcome: OUTCOME
            .read(row)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        elapsed_ms: ELAPSED_MS.read(row).and_then(Value::as_u64),
        cached: CACHED.read(row).and_then(Value::as_bool).unwrap_or(false),
        asked: asked_facts(row),
        answered: answered_of(row),
        confidence: CONFIDENCE.read(row).and_then(Value::as_f64),
        applied: applied_of(row).or_else(|| {
            label.and_then(|late| {
                APPLIED
                    .read(late)
                    .and_then(Value::as_bool)
                    .or_else(|| PRESSED.read(late).and_then(Value::as_bool))
            })
        }),
        agreed: on_either(AGREED).and_then(Value::as_bool),
        followed: on_either(FOLLOWED)
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// The request's facts by their own names: a scalar as it is, a string cut
/// to a line, a list as its length, an object left out.
fn asked_facts(row: &Value) -> Map<String, Value> {
    let mut facts = Map::new();
    for key in ASKED_KEYS {
        let Some(value) = row.get(*key) else {
            continue;
        };
        let carried = match value {
            Value::String(text) => Value::from(cut(text, ASKED_VALUE_CHARS)),
            Value::Array(items) => Value::from(items.len()),
            Value::Object(_) | Value::Null => continue,
            scalar => scalar.clone(),
        };
        facts.insert((*key).to_string(), carried);
    }
    facts
}

/// What the row answered, in the shape its seat wrote: a word, the routing
/// axes' choices, or what recall moved. `Null` when it answered nothing a
/// screen can name.
fn answered_of(row: &Value) -> Value {
    if let Some(word) = CHOSEN.read(row) {
        return word.clone();
    }
    if let Some(axes) = JUDGMENT.read(row).and_then(Value::as_object) {
        let choices: Map<String, Value> = axes
            .iter()
            .filter_map(|(axis, judged)| Some((axis.clone(), judged.get(AXIS_CHOICE)?.clone())))
            .collect();
        if !choices.is_empty() {
            return Value::Object(choices);
        }
    }
    if let Some(judged) = JUDGED.read(row).and_then(Value::as_object) {
        // The two numbers a person reads of a reorder: how many moved and
        // whether the first changed. The lists of names are the row's, not
        // a line's.
        let scalars: Map<String, Value> = judged
            .iter()
            .filter(|(_, value)| value.is_number() || value.is_boolean())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if !scalars.is_empty() {
            return Value::Object(scalars);
        }
    }
    Value::Null
}

/// The first `chars` characters of `text`, on a character boundary, with a
/// mark when anything was cut.
fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars).collect();
    format!("{kept}{}", crate::jev::CUT_MARK)
}

#[cfg(test)]
mod tests;
