//! A closed choice's answer, read against the options that were offered —
//! the one set of rules every Jev question with a closed answer space keeps:
//! the browser's numbered controls (`crate::browser_action`) and a quiet
//! worker's causes (`crate::stall_cause`).
//!
//! The endpoint's `choice` answer names one option, gives every option a
//! probability, and adds a confidence. An answer that breaks any rule below is
//! discarded whole: a judgment that got the shape wrong has said nothing about
//! the question, and a caller never spends half of one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// How far a set of probabilities may be from summing to one.
pub const PROBABILITY_SUM_TOLERANCE: f64 = 1e-6;

/// What the `type` of a closed choice's answer reads.
const CHOICE: &str = "choice";

/// A validated answer: the option it chose, every option's probability, and
/// its confidence — which is the shape of the distribution, not a rate of
/// being right.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub chosen: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// Every way an answer fails to be one. All of them discard the answer whole
/// and all of them reach a ledger as `schema`; the variants exist so a test
/// and a message can say which rule was broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceRefusal {
    /// The question was not answered.
    NoAnswer,
    /// The answer is not a choice.
    NotAChoice,
    /// The chosen name was not offered.
    UnknownOption,
    /// The probabilities do not name exactly the options that were offered.
    Keys,
    /// The probabilities do not sum to one.
    NotOne,
    /// A probability or the confidence is not a number in `[0, 1]`.
    OutOfRange,
}

impl ChoiceRefusal {
    /// Why it was refused, for a message. The ledger writes `schema` for all
    /// of these — a closed set of failure tokens is the point of that table.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoAnswer => "the answer has no verdict for the question that was asked",
            Self::NotAChoice => "the answer is not a choice",
            Self::UnknownOption => "the answer chose an option that was not offered",
            Self::Keys => "the probabilities do not name exactly the options that were offered",
            Self::NotOne => "the probabilities do not sum to one",
            Self::OutOfRange => "a probability or the confidence is not a number in [0, 1]",
        }
    }
}

/// What the endpoint's `answers` map says about the question named `question`,
/// judged against the options it `offered`.
///
/// # Errors
///
/// [`ChoiceRefusal`] names the first rule the answer broke.
pub fn read(
    answers: &Value,
    question: &str,
    offered: &BTreeSet<String>,
) -> Result<Choice, ChoiceRefusal> {
    let answer = answers.get(question).ok_or(ChoiceRefusal::NoAnswer)?;
    if answer.get("type").and_then(Value::as_str) != Some(CHOICE) {
        return Err(ChoiceRefusal::NotAChoice);
    }
    let chosen = answer
        .get(CHOICE)
        .and_then(Value::as_str)
        .ok_or(ChoiceRefusal::NoAnswer)?;
    if !offered.contains(chosen) {
        return Err(ChoiceRefusal::UnknownOption);
    }
    let given = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(ChoiceRefusal::Keys)?;
    if given.len() != offered.len() || !given.keys().all(|name| offered.contains(name)) {
        return Err(ChoiceRefusal::Keys);
    }
    let mut probabilities = BTreeMap::new();
    let mut total = 0.0;
    for (name, value) in given {
        let share = value.as_f64().ok_or(ChoiceRefusal::OutOfRange)?;
        if !share.is_finite() || !(0.0..=1.0).contains(&share) {
            return Err(ChoiceRefusal::OutOfRange);
        }
        total += share;
        probabilities.insert(name.clone(), share);
    }
    if (total - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(ChoiceRefusal::NotOne);
    }
    let confidence = answer
        .get("confidence")
        .and_then(Value::as_f64)
        .ok_or(ChoiceRefusal::OutOfRange)?;
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(ChoiceRefusal::OutOfRange);
    }
    Ok(Choice {
        chosen: chosen.to_string(),
        probabilities,
        confidence,
    })
}
