//! A Noul, in both directions — whether a condition holds, answered as one
//! probability of yes (docs.typesafe.ai/primitives/noul): the second of the
//! vendor's primitives this product asks, beside the closed choice
//! (`crate::jev::choice`).
//!
//! A Noul has no confidence beside it: its answer is two outcomes, yes and
//! no, and the one number says both. A value near a half is a judgment that
//! yes and no are about as likely — not a middling degree of the condition.
//!
//! [`question`] builds one question's object, in the envelope a closed
//! choice wears too (`crate::jev::choice`); [`read`] reads its answer. An
//! answer that breaks a rule below is refused whole, by its own word, as a
//! closed choice's is: a caller never spends half of one.

use serde_json::{Map, Value};

/// The union tag a Noul carries, in both directions: the endpoint reads a
/// question's kind off it, and the answer names its own kind with it.
const NOUL: &str = "noul";

/// The keys a Noul's criteria says what each outcome means under.
const YES: &str = "true";
const NO: &str = "false";

/// One Noul question's object: `instructions` say what the condition is,
/// `yes` and `no` what each outcome means — the boundary cases a condition
/// alone leaves open.
#[must_use]
pub fn question(instructions: &str, yes: &str, no: &str) -> Value {
    super::choice::envelope(
        NOUL,
        instructions,
        Map::from_iter([
            (YES.to_string(), Value::from(yes)),
            (NO.to_string(), Value::from(no)),
        ]),
    )
}

/// Every way a Noul's answer fails to be one. Each discards the answer whole
/// and reaches a ledger under a `schema` word of its own, so a reader can
/// tell a broken Noul from a broken choice in the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoulRefusal {
    /// The question was not answered.
    NoAnswer,
    /// The answer is not a Noul.
    NotANoul,
    /// The probability is not a number in `[0, 1]`.
    OutOfRange,
}

impl NoulRefusal {
    /// The word a ledger row writes for this refusal: the wire's `schema`,
    /// with the rule that broke after it — the family the judge counts as a
    /// reply that arrived malformed (`crate::jev::promote::SCHEMA`).
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::NoAnswer => "schema_no_noul",
            Self::NotANoul => "schema_not_a_noul",
            Self::OutOfRange => "schema_noul_out_of_range",
        }
    }
}

/// The probability of yes the endpoint's `answers` map gives the Noul named
/// `question`.
///
/// # Errors
///
/// [`NoulRefusal`] names the rule the answer broke.
pub fn read(answers: &Value, question: &str) -> Result<f64, NoulRefusal> {
    let answer = answers.get(question).ok_or(NoulRefusal::NoAnswer)?;
    if !super::choice::tagged(answer, NOUL) {
        return Err(NoulRefusal::NotANoul);
    }
    let yes = answer
        .get(NOUL)
        .and_then(Value::as_f64)
        .ok_or(NoulRefusal::OutOfRange)?;
    if !yes.is_finite() || !(0.0..=1.0).contains(&yes) {
        return Err(NoulRefusal::OutOfRange);
    }
    Ok(yes)
}

#[cfg(test)]
mod tests;
