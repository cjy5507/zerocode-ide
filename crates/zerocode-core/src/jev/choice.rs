//! A closed choice, in both directions — the one set of rules every Jev
//! question with a closed answer space keeps, whether it is being asked or
//! being read: the browser's numbered controls (`crate::screen_action`), a
//! quiet worker's causes (`crate::stall_cause`), a worker's room
//! (`crate::worker_placement`) and a summons' agent (`crate::summon_choice`).
//!
//! [`asked`] builds the question. [`read`] reads the answer. Both spell the
//! union tag from one constant here, and a source contract holds that no seat
//! spells it again — the one that did spell it by hand, and forgot, sent
//! nothing a reader ever saw.
//!
//! The endpoint's `choice` answer names one option, gives every option a
//! probability, and adds a confidence. An answer that breaks any rule below is
//! discarded whole: a judgment that got the shape wrong has said nothing about
//! the question, and a caller never spends half of one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// How far a set of `options` probabilities may be from summing to one.
///
/// The contract says they sum to one, and they do — before the wire rounds
/// them. Each arrives on the answer grid ([`crate::jev::ANSWER_STEP`]),
/// carrying up to half a step, so a set of `options` of them can stand
/// `options` half-steps from one.
///
/// The bound does not sit ON the grid. Both sides are whole numbers of steps,
/// so the distance between them is one too, and a distance the grid lands on
/// exactly is a distance a double's last bit decides — measured on a sibling
/// check, 38 answers sat on such a bound and 17 passed while 21 were refused.
/// So the bound is the last distance the grid can land on that rounding still
/// explains, carried half a step further, where no last bit decides it.
///
/// This was a flat `1e-6` before, which refused answers that broke no rule: on
/// this machine, against `jev-1.13.0` with fourteen options, 6 of 51 real
/// calls (11.8%) summed to 0.99 and were thrown away whole (measured
/// 2026-09-18, t-4774).
#[must_use]
pub fn probability_sum_tolerance(options: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let steps = (options / 2) as f64;
    steps * crate::jev::ANSWER_STEP + crate::jev::WIRE_ROUNDING
}

/// The union tag a closed choice carries, in both directions: the endpoint
/// reads a question's kind off it, and an answer names its own kind with the
/// same word. One word, so the two can never disagree.
const CHOICE: &str = "choice";

/// The `questions` of a request asking one closed choice, named `question`.
///
/// Every closed choice this product asks is built here because the tag cannot
/// be left off: a body whose question carries no `type` is refused whole by
/// the endpoint (`union_tag_not_found`, 422), and the refusal names the body
/// rather than the question, so a caller sees a request that failed and not a
/// question that was malformed. Four seats used to spell this envelope by
/// hand and one of them — the placement judgment — omitted the tag, which is
/// why its ledger holds no rows at all: every request it ever sent was thrown
/// away before it was read.
#[must_use]
pub fn asked(question: &str, instructions: &str, criteria: Map<String, Value>) -> Value {
    Value::Object(Map::from_iter([(
        question.to_string(),
        envelope(CHOICE, instructions, criteria),
    )]))
}

/// The key a question and its answer name their primitive under — the
/// union tag's own key.
const TAG_KEY: &str = "type";

/// One question's object as the endpoint reads every primitive: the union
/// tag naming its kind, what it asks, and what each answer means. The one
/// spelling of the envelope — a closed choice ([`asked`]) and a Noul
/// (`crate::jev::noul`, t-6187) both wear it, so a second primitive cannot
/// come to spell a key the endpoint reads differently.
pub(super) fn envelope(tag: &str, instructions: &str, criteria: Map<String, Value>) -> Value {
    Value::Object(Map::from_iter([
        (TAG_KEY.to_string(), Value::from(tag)),
        ("instructions".to_string(), Value::from(instructions)),
        ("criteria".to_string(), Value::Object(criteria)),
    ]))
}

/// Whether an answer names `tag` as its kind, read off the key the envelope
/// wrote it under.
pub(super) fn tagged(answer: &Value, tag: &str) -> bool {
    answer.get(TAG_KEY).and_then(Value::as_str) == Some(tag)
}

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
    /// The word a ledger row writes for this refusal: the wire's `schema`,
    /// with the rule that broke after it.
    ///
    /// One word for all of them loses the cause the moment it is written, and
    /// the cause is the whole value of the row — a sum that rounding explains
    /// and an answer that named an option nobody offered are not the same
    /// event. The set is closed and carries not one character of the answer.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::NoAnswer => "schema_no_answer",
            Self::NotAChoice => "schema_not_a_choice",
            Self::UnknownOption => "schema_unknown_option",
            Self::Keys => "schema_keys",
            Self::NotOne => "schema_not_one",
            Self::OutOfRange => "schema_out_of_range",
        }
    }

    /// Why it was refused, for a message.
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
    if !tagged(answer, CHOICE) {
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
    if (total - 1.0).abs() > probability_sum_tolerance(offered.len()) {
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

#[cfg(test)]
mod tests;
