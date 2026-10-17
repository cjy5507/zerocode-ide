use serde_json::json;

use super::store::{self, Error};
use super::*;

/// 2026-10-17T05:00:00Z: when the store's stamps are written.
pub(super) const NOW: i64 = 1_792_213_200_000;

/// A yes-or-no decision asking at a turn's end, keeping every rule.
pub(super) fn yes_no(id: &str, name: &str) -> Definition {
    Definition::new(
        id.to_string(),
        name.to_string(),
        "Did this turn change what a person using the product sees?".to_string(),
        Answer::YesNo {
            yes: "The evidence shows a visible change.".to_string(),
            no: "Nothing visible changed, or the evidence cannot tell.".to_string(),
        },
        Point::TurnEnd,
    )
}

fn choice(options: &[(&str, &str)]) -> Answer {
    Answer::Choice(
        options
            .iter()
            .map(|(name, meaning)| ((*name).to_string(), (*meaning).to_string()))
            .collect(),
    )
}

/// Every line a person writes is read by the detector the door clears a
/// request with (`crate::credential::may_carry_a_credential`). A line that may
/// carry a credential is refused and pointed at — its field, and its number
/// counted from one — and the store keeps nothing.
#[test]
fn a_question_with_a_secret_line_is_refused_with_its_line() {
    let mut definition = yes_no("d-0000000a", "Release notes");
    assert!(
        check(&definition).is_empty(),
        "the fixture keeps every rule"
    );
    definition.question = "Did this turn change what a person sees?\nRead the diff first.\n\
                           Authorization: Bearer abc\nThen answer."
        .to_string();
    let third = [Refusal::SecretLine {
        field: Field::Question,
        line: 3,
    }];
    assert_eq!(check(&definition), third);
    let home = tempfile::tempdir().unwrap();
    assert!(
        matches!(
            store::create(home.path(), &definition, JevMode::Shadow, NOW),
            Err(Error::Refused(refused)) if refused == third
        ),
        "the store keeps nothing the check refuses",
    );
    assert!(store::list(home.path()).unwrap().is_empty());

    let mut named = yes_no("d-0000000b", "Rotate the deploy password");
    named.answer = Answer::YesNo {
        yes: "It rotated.".to_string(),
        no: "Nothing rotated.\nAsk with Bearer abc.".to_string(),
    };
    assert_eq!(
        check(&named),
        [
            Refusal::SecretLine {
                field: Field::Name,
                line: 1,
            },
            Refusal::SecretLine {
                field: Field::No,
                line: 2,
            },
        ],
    );

    let mut chose = yes_no("d-0000000c", "Ship or wait");
    chose.answer = choice(&[
        ("ship", "Ship it.\nexport TOKEN=1"),
        ("password", "Ask for one."),
    ]);
    assert_eq!(
        check(&chose),
        [
            Refusal::SecretLine {
                field: Field::OptionName("password".to_string()),
                line: 1,
            },
            Refusal::SecretLine {
                field: Field::Meaning("ship".to_string()),
                line: 2,
            },
        ],
    );

    let mut scored = yes_no("d-0000000d", "Risk");
    scored.answer = Answer::Score(vec![
        "Nothing to do.".to_string(),
        "Someone runs mysql -phunter2 by hand.".to_string(),
        "Stop.".to_string(),
    ]);
    assert_eq!(
        check(&scored),
        [Refusal::SecretLine {
            field: Field::Level(1),
            line: 1,
        }],
    );
}

/// A score asks the wire's own primitive: its tag, the person's question, and
/// the levels in order, lowest first, as a list — and nothing else.
#[test]
fn a_score_asks_its_levels_in_order() {
    let mut definition = yes_no("d-0000000a", "Risk");
    definition.answer = Answer::Score(vec![
        "Low.".to_string(),
        "Some.".to_string(),
        "High.".to_string(),
    ]);
    let questions = questions(&definition);
    assert_eq!(questions.as_object().map(Map::len), Some(1));
    let question = &questions[definition.id.as_str()];
    assert_eq!(question.as_object().map(Map::len), Some(3), "{question}");
    assert_eq!(question["type"], "score");
    assert_eq!(question["criteria"], json!(["Low.", "Some.", "High."]));
    let instructions = question["instructions"].as_str().unwrap();
    assert!(
        instructions.starts_with(&definition.question),
        "{instructions}"
    );
}
