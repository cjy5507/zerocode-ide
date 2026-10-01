use super::*;
use crate::summon_difficulty::SPEC_CHAR_CAP;
use crate::summon_difficulty::lineup::Lineup;
use serde_json::json;

fn look() -> Look {
    Look {
        title: "task".into(),
        spec: "가".repeat(SPEC_CHAR_CAP + 1),
        attempt: 2,
        failures: 1,
        retry_of: true,
    }
}

fn model_ask() -> ModelAsk {
    let lineup = Lineup::from_catalog(
        &json!({"models": [
            {"provider": "claude", "id": "model-a", "builtin": true},
            {"provider": "claude", "id": "model-b", "builtin": true},
        ]}),
        "claude",
    )
    .unwrap();
    let options = crate::summon_model::options(
        "claude",
        &lineup,
        None,
        &Default::default(),
        &["medium", "high"],
        |_| None,
        0,
    );
    crate::summon_model::ask(&look(), &options).unwrap()
}

/// A1: the state's words go once — the difficulty's own keys, word for word,
/// inside the model question's state — and both questions ride beside it.
#[test]
fn one_state_carries_both_questions() {
    let asked = AssignAsk {
        difficulty: Some(look()),
        model: Some(model_ask()),
        ..Default::default()
    };
    assert!(asked.shared());
    let state = asked.state();
    for (key, value) in look().state().as_object().unwrap() {
        assert_eq!(&state[key], value, "{key} is the difficulty's own");
    }
    let questions = asked.questions();
    let questions = questions.as_object().unwrap();
    assert_eq!(questions.len(), 2);
    assert_eq!(
        questions[crate::summon_difficulty::QUESTION],
        crate::summon_difficulty::questions()[crate::summon_difficulty::QUESTION],
        "the difficulty's words do not change"
    );
    assert_eq!(
        questions[crate::summon_model::QUESTION],
        model_ask().questions[crate::summon_model::QUESTION]
    );
    let alone = asked.only(false, true, false);
    assert!(!alone.shared());
    assert_eq!(alone.state(), look().state());
    assert!(asked.only(false, false, false).is_empty());
}

/// The door clears one body under one seat's list of what it sends: the two
/// seats must send the same texts under the same caps for that to be one
/// door, not two.
#[test]
fn every_seat_sends_the_same_texts_under_the_same_caps() {
    assert_eq!(
        crate::jev::SUMMON_DIFFICULTY.sends,
        crate::jev::SUMMON_MODEL.sends
    );
    assert_eq!(crate::jev::SUMMON.sends, crate::jev::SUMMON_MODEL.sends);
}
