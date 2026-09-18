//! What the envelope promises: a question the endpoint will read, and a tag
//! no seat can forget because no seat writes it.

use serde_json::json;

use super::*;

/// The four seats that ask a closed choice, as source. A seat added to the
/// table and not to this list is a seat this contract does not hold, so the
/// list is named in [`the_union_tag_is_spelled_in_one_file`]'s own failure.
const SEATS: [(&str, &str); 4] = [
    ("screen_action", include_str!("../../screen_action.rs")),
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

/// A sum the wire's rounding explains is not a broken rule; the next step out
/// of the grid is.
#[test]
fn the_sum_rule_admits_the_roundings_own_distance_and_nothing_past_it() {
    // Every number arrives on the grid, so every distance from one does too:
    // the bound must sit BETWEEN the last distance rounding explains and the
    // grid's next step, never on either.
    for options in 2..=24_usize {
        let bound = probability_sum_tolerance(options);
        #[allow(clippy::cast_precision_loss)]
        let explained = (options / 2) as f64 * crate::jev::ANSWER_STEP;
        let next = explained + crate::jev::ANSWER_STEP;
        assert!(
            bound > explained && bound < next,
            "{options} options: {bound} is not between {explained} and {next}"
        );
    }

    // The worked case, as `jev-1.13.0` actually answered it on this machine
    // (2026-09-18, t-4774): fourteen options, every number on the grid, the
    // set summing to 0.99. The flat `1e-6` bound threw 6 of 51 real calls
    // away whole for exactly this.
    let said: [(&str, f64); 14] = [
        ("give_up", 0.93),
        ("mark:11", 0.03),
        ("mark:1", 0.01),
        ("mark:10", 0.01),
        ("mark:9", 0.01),
        ("done", 0.0),
        ("mark:2", 0.0),
        ("mark:3", 0.0),
        ("mark:4", 0.0),
        ("mark:5", 0.0),
        ("mark:6", 0.0),
        ("mark:7", 0.0),
        ("mark:8", 0.0),
        ("mark:12", 0.0),
    ];
    let offered: BTreeSet<String> = said.iter().map(|(name, _)| (*name).to_string()).collect();
    let probabilities: Map<String, Value> = said
        .iter()
        .map(|(name, share)| ((*name).to_string(), json!(share)))
        .collect();
    let total: f64 = said.iter().map(|(_, share)| share).sum();
    assert!(
        (total - 1.0).abs() > 1e-6,
        "the fixture is a sum the rounding moved"
    );
    let answers = json!({
        "q": {
            "type": "choice",
            "choice": "give_up",
            "probabilities": Value::Object(probabilities),
            "confidence": 0.93,
        }
    });
    assert!(
        read(&answers, "q", &offered).is_ok(),
        "a sum {total} the rounding explains is refused"
    );

    // And a set that misses by more than the rounding could is still refused.
    let mut broken = answers.clone();
    broken["q"]["probabilities"]["give_up"] = json!(0.5);
    assert_eq!(
        read(&broken, "q", &offered),
        Err(ChoiceRefusal::NotOne),
        "a sum no rounding explains is still a broken rule"
    );
}

/// Each rule a closed choice can break says so in its own closed word, and no
/// two of them say the same one.
#[test]
fn every_broken_rule_has_a_word_of_its_own() {
    let words: BTreeSet<&str> = [
        ChoiceRefusal::NoAnswer,
        ChoiceRefusal::NotAChoice,
        ChoiceRefusal::UnknownOption,
        ChoiceRefusal::Keys,
        ChoiceRefusal::NotOne,
        ChoiceRefusal::OutOfRange,
    ]
    .into_iter()
    .map(ChoiceRefusal::token)
    .collect();
    assert_eq!(words.len(), 6, "two rules share a word: {words:?}");
    for word in &words {
        assert!(
            word.starts_with("schema"),
            "{word} does not read as a schema failure"
        );
        assert!(
            word.chars()
                .all(|letter| letter.is_ascii_lowercase() || letter == '_'),
            "{word} is not one closed word"
        );
    }
}

#[test]
fn every_refusal_token_is_built_on_the_word_the_judge_recognises() {
    // The promotion rule refuses to raise a seat while replies are arriving
    // malformed, and it finds them by the word these tokens begin with. A
    // seventh rule spelled without it would be a malformed reply the judge
    // walked past.
    for refusal in [
        ChoiceRefusal::NoAnswer,
        ChoiceRefusal::NotAChoice,
        ChoiceRefusal::UnknownOption,
        ChoiceRefusal::Keys,
        ChoiceRefusal::NotOne,
        ChoiceRefusal::OutOfRange,
    ] {
        assert!(
            crate::jev::promote::names_a_schema_failure(refusal.token()),
            "{} is not built on {}",
            refusal.token(),
            crate::jev::promote::SCHEMA
        );
    }
}
