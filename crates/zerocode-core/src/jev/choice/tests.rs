//! What the envelope promises: a question the endpoint will read, and a tag
//! no seat can forget because no seat writes it.

use super::*;

/// The four seats that ask a closed choice, as source. A seat added to the
/// table and not to this list is a seat this contract does not hold, so the
/// list is named in [`the_union_tag_is_spelled_in_one_file`]'s own failure.
const SEATS: [(&str, &str); 4] = [
    ("browser_action", include_str!("../../browser_action.rs")),
    ("stall_cause", include_str!("../../stall_cause.rs")),
    (
        "worker_placement",
        include_str!("../../worker_placement.rs"),
    ),
    ("summon_choice", include_str!("../../summon_choice.rs")),
];

fn criteria() -> Map<String, Value> {
    Map::from_iter([(
        "room".to_string(),
        Value::from("tab. The pane stands in front of the person."),
    )])
}

/// The tag is there, and it is the same word the answer is read against.
///
/// This is the whole reason the builder exists: the endpoint refuses a body
/// whose question carries no `type` and names the body in the refusal, so the
/// omission does not look like a malformed question — it looks like a request
/// that failed.
#[test]
fn a_question_carries_the_union_tag_the_answer_is_read_against() {
    let questions = asked("placement", "Choose the room.", criteria());
    let question = questions
        .get("placement")
        .and_then(Value::as_object)
        .expect("the question stands under its own name");

    assert_eq!(question.get("type").and_then(Value::as_str), Some(CHOICE));
    assert_eq!(
        question.get("instructions").and_then(Value::as_str),
        Some("Choose the room.")
    );
    assert_eq!(
        question.get("criteria").and_then(Value::as_object),
        Some(&criteria())
    );
    assert_eq!(question.len(), 3, "the envelope carries nothing else");

    /* The same word both ways: an answer tagged with what `asked` sends is
     * the answer `read` accepts. A drift between the two would pass every
     * test that only looked at one side. */
    let answer = serde_json::json!({
        "placement": {
            "type": question.get("type").expect("the tag"),
            "choice": "tab",
            "probabilities": {"tab": 1.0},
            "confidence": 0.9,
        }
    });
    let offered = BTreeSet::from(["tab".to_string()]);
    assert_eq!(
        read(&answer, "placement", &offered)
            .expect("the answer")
            .chosen,
        "tab"
    );
}

/// The union tag is spelled in this file and nowhere else.
///
/// Four seats used to write this envelope by hand, and `worker_placement`
/// left the tag off — so every placement request it ever sent was refused
/// whole and its ledger holds no rows at all. A word copied into four files
/// is a word three of them can keep while the fourth loses it; a word in one
/// file cannot be forgotten at a call site, because a call site does not say
/// it.
#[test]
fn the_union_tag_is_spelled_in_one_file() {
    let quoted = format!("\"{CHOICE}\"");
    for (name, source) in SEATS {
        assert!(
            !source.contains(&quoted),
            "{name} spells the union tag itself ({quoted}); build its question with `choice::asked` \
             so the tag lives only in jev/choice.rs"
        );
    }
}

/// Every seat asks through the builder.
///
/// The contract above says no seat writes the tag. This one says each seat
/// still asks a closed choice — without it, a seat could satisfy the first
/// test by not asking at all, or by hand-rolling an envelope that omits the
/// tag exactly as `worker_placement` did.
#[test]
fn every_seat_builds_its_question_here() {
    for (name, source) in SEATS {
        assert!(
            source.contains("choice::asked("),
            "{name} does not build its question with `choice::asked`"
        );
    }
}
