//! A Noul's two directions: the question's shape, and the rules an answer
//! keeps to be one.

use serde_json::json;

use super::*;

#[test]
fn a_noul_question_names_its_kind_and_what_yes_and_no_mean() {
    let asked = question("The screen is a sign-in page.", "It is.", "It is not.");
    assert_eq!(
        asked,
        json!({
            "type": "noul",
            "instructions": "The screen is a sign-in page.",
            "criteria": {"true": "It is.", "false": "It is not."},
        })
    );
}

/// The contract's own answer reads as its probability of yes; an absent
/// answer, one of another kind and one outside `[0, 1]` are refused whole,
/// each by its own word, and every word is a schema failure the judge
/// counts.
#[test]
fn an_answer_is_one_probability_of_yes_or_it_is_refused() {
    let answers = json!({
        "walled": {"type": "noul", "noul": 0.93},
        "odd": {"type": "choice", "choice": "a"},
        "far": {"type": "noul", "noul": 1.2},
        "blank": {"type": "noul"},
    });
    assert_eq!(read(&answers, "walled"), Ok(0.93));
    assert_eq!(read(&answers, "missing"), Err(NoulRefusal::NoAnswer));
    assert_eq!(read(&answers, "odd"), Err(NoulRefusal::NotANoul));
    assert_eq!(read(&answers, "far"), Err(NoulRefusal::OutOfRange));
    assert_eq!(read(&answers, "blank"), Err(NoulRefusal::OutOfRange));
    let mut tokens = Vec::new();
    for refusal in [
        NoulRefusal::NoAnswer,
        NoulRefusal::NotANoul,
        NoulRefusal::OutOfRange,
    ] {
        assert!(
            crate::jev::promote::names_a_schema_failure(refusal.token()),
            "{}",
            refusal.token()
        );
        tokens.push(refusal.token());
    }
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(tokens.len(), 3, "two refusals share a word");
}
