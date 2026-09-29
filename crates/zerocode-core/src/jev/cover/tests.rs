use serde_json::{Value, json};

use super::*;
use crate::computer_use_protocol::cover::{Coverer, Owner};
use crate::computer_use_protocol::render::Rect;

/// A scene: the target at the screen's corner, one window over its place.
fn scene(owner: Owner, layer: i64) -> Cover {
    Cover {
        target: 5,
        app: "Target".to_string(),
        pid: 10,
        layer: 0,
        window: Rect::new(100.0, 100.0, 300.0, 200.0),
        spot: Rect::new(150.0, 150.0, 40.0, 20.0),
        coverers: vec![Coverer {
            id: 7,
            app: "Other".to_string(),
            owner,
            layer,
            bounds: Rect::new(20.0, 30.0, 200.0, 100.4),
            hides_permille: 1_000,
        }],
        in_front: vec![Rect::new(120.0, 130.0, 200.0, 100.4)],
        hidden_permille: 1_000,
        centre_hidden: true,
    }
}

fn a_choice(chosen: &str, probabilities: &[(&str, f64)], confidence: f64) -> Value {
    json!({
        "type": "choice",
        "choice": chosen,
        "probabilities": probabilities.iter().map(|(word, share)| ((*word).to_string(), json!(share))).collect::<serde_json::Map<_, _>>(),
        "confidence": confidence,
    })
}

fn kinds(chosen: &str, confidence: f64) -> Value {
    let probabilities: Vec<(&str, f64)> = COVER_KINDS
        .iter()
        .map(|(word, _)| (*word, if *word == chosen { 0.96 } else { 0.01 }))
        .collect();
    a_choice(chosen, &probabilities, confidence)
}

fn answers(
    kind: &str,
    moves: &[(&str, f64)],
    confidence: f64,
    covered: f64,
    reversible: f64,
) -> Value {
    let chosen = moves
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or("", |(word, _)| *word);
    json!({
        (COVER_COVERED): { "type": "noul", "noul": covered },
        (COVER_KIND): kinds(kind, confidence),
        (COVER_MOVE): a_choice(chosen, moves, confidence),
        (COVER_REVERSIBLE): { "type": "noul", "noul": reversible },
    })
}

const RAISE_FIRST: [(&str, f64); 4] = [
    ("raise_target", 0.6),
    ("move_target", 0.3),
    ("look_again", 0.05),
    ("ask_person", 0.05),
];

/// One request asks all four questions — two Nouls and two closed choices —
/// each carrying the premise whole, since none sees another's answer; its
/// state is the scene's facts, from the target's corner, and nothing a window
/// shows.
#[test]
fn one_request_asks_four_heads_each_with_its_premise() {
    let asked = ask(&scene(Owner::OtherApp, 3));
    let questions = asked.questions.as_object().expect("an object");
    let kinds: Vec<(&str, &str)> = [COVER_COVERED, COVER_KIND, COVER_MOVE, COVER_REVERSIBLE]
        .into_iter()
        .map(|name| (name, questions[name]["type"].as_str().unwrap_or_default()))
        .collect();
    assert_eq!(
        kinds,
        [
            (COVER_COVERED, "noul"),
            (COVER_KIND, "choice"),
            (COVER_MOVE, "choice"),
            (COVER_REVERSIBLE, "noul"),
        ]
    );
    assert_eq!(questions.len(), 4);
    for question in questions.values() {
        let instructions = question["instructions"].as_str().unwrap_or_default();
        for key in COVER_STATE_KEYS {
            assert!(instructions.contains(&format!("`{key}`")), "{key}");
        }
    }
    assert_eq!(
        asked.state,
        json!({
            "target": { "app": "Target", "layer": 0, "width": 300, "height": 200 },
            "place": { "x": 50, "y": 50, "width": 40, "height": 20,
                       "hidden_permille": 1_000, "centre_hidden": true },
            "over": [{ "owner": "other_app", "app": "Other", "layer": 3,
                       "x": 20, "y": 30, "width": 200, "height": 100,
                       "hides_permille": 1_000 }],
        })
    );
}

/// What stands in front is the system's, an app's waiting on an answer, or
/// not known: the hand stays still for the person, however sure the answer
/// is of a move.
#[test]
fn the_systems_a_waiting_dialog_or_the_unknown_holds_the_hand_for_a_person() {
    for kind in ["system_dialog", "modal", "unknown"] {
        let read = read(&answers(kind, &RAISE_FIRST, 0.95, 0.99, 0.99)).expect("reads");
        assert_eq!(ladder(&read, None), Err(Held::Theirs), "{kind}");
    }
    for kind in ["window", "panel"] {
        let read = read(&answers(kind, &RAISE_FIRST, 0.95, 0.99, 0.99)).expect("reads");
        assert!(ladder(&read, None).is_ok(), "{kind}");
    }
}

/// An answer unsure of what stands in front or of the move, or doubting the
/// moves can do it, holds the hand too — below the seat's own floor, or the
/// act line its graded answers drew.
#[test]
fn an_unsure_or_irreversible_answer_holds_the_hand() {
    let unsure = read(&answers("window", &RAISE_FIRST, 0.3, 0.99, 0.99)).expect("reads");
    assert_eq!(ladder(&unsure, None), Err(Held::Unsure));
    let sure = read(&answers("window", &RAISE_FIRST, 0.8, 0.99, 0.99)).expect("reads");
    assert!(ladder(&sure, None).is_ok());
    assert_eq!(ladder(&sure, Some(900)), Err(Held::Unsure), "a drawn line");
    let doubts = read(&answers("window", &RAISE_FIRST, 0.8, 0.99, 0.5)).expect("reads");
    assert_eq!(ladder(&doubts, None), Err(Held::NotReversible));
}

/// A move that left the place covered hands on to the next the same answer
/// ranked from the runner-up floor — no second question — and never past
/// asking the person.
#[test]
fn a_move_that_left_it_covered_takes_the_runner_up_without_asking_again() {
    let read_of =
        |moves: &[(&str, f64)]| read(&answers("panel", moves, 0.8, 0.99, 0.99)).expect("reads");
    assert_eq!(
        ladder(&read_of(&RAISE_FIRST), None),
        Ok(vec![Move::RaiseTarget, Move::MoveTarget])
    );
    let alone = [
        ("raise_target", 0.8),
        ("move_target", 0.1),
        ("look_again", 0.05),
        ("ask_person", 0.05),
    ];
    assert_eq!(ladder(&read_of(&alone), None), Ok(vec![Move::RaiseTarget]));
    let asks_next = [
        ("move_target", 0.6),
        ("ask_person", 0.3),
        ("raise_target", 0.05),
        ("look_again", 0.05),
    ];
    assert_eq!(
        ladder(&read_of(&asks_next), None),
        Ok(vec![Move::MoveTarget])
    );
    let asks = [
        ("ask_person", 0.6),
        ("move_target", 0.3),
        ("raise_target", 0.05),
        ("look_again", 0.05),
    ];
    assert_eq!(ladder(&read_of(&asks), None), Err(Held::Chosen));
    // Sure that nothing covers it: look again before anything moves.
    let clear = read(&answers("window", &RAISE_FIRST, 0.8, 0.1, 0.99)).expect("reads");
    assert_eq!(
        ladder(&clear, None),
        Ok(vec![Move::LookAgain, Move::RaiseTarget, Move::MoveTarget])
    );
}

/// With no seat the hand brings the target to the front and then moves it,
/// but another app's window above ordinary ones may be the system's, and
/// the facts alone cannot tell: that is the person's.
#[test]
fn todays_rule_raises_then_moves_and_leaves_another_apps_top_window_to_the_person() {
    let moves = Ok(vec![Move::RaiseTarget, Move::MoveTarget]);
    assert_eq!(todays_rule(&scene(Owner::OtherApp, 0)), moves);
    assert_eq!(todays_rule(&scene(Owner::SameApp, 3)), moves);
    assert_eq!(todays_rule(&scene(Owner::ZeroCode, 0)), moves);
    assert_eq!(todays_rule(&scene(Owner::OtherApp, 3)), Err(Held::Theirs));
}

/// The marks can say no: a first move that was made and left the place
/// covered is wrong, asking the person when a move cleared it is wrong, and a
/// move nobody made is not compared.
#[test]
fn the_marks_say_no_when_the_first_move_left_it_covered() {
    let raise = read(&answers("window", &RAISE_FIRST, 0.8, 0.99, 0.99)).expect("reads");
    let rule = todays_rule(&scene(Owner::OtherApp, 0));
    let cleared_by = |tried: &[Move], by: Option<Move>| Outcome {
        tried: tried.to_vec(),
        cleared_by: by,
        not_made: Vec::new(),
    };
    assert_eq!(
        marks(
            &raise,
            &rule,
            &cleared_by(&[Move::RaiseTarget], Some(Move::RaiseTarget))
        ),
        Ok((true, Some(true)))
    );
    assert_eq!(
        marks(
            &raise,
            &rule,
            &cleared_by(
                &[Move::RaiseTarget, Move::MoveTarget],
                Some(Move::MoveTarget)
            )
        ),
        Ok((false, Some(false)))
    );
    let asks = read(&answers(
        "panel",
        &[
            ("ask_person", 0.6),
            ("move_target", 0.3),
            ("raise_target", 0.05),
            ("look_again", 0.05),
        ],
        0.8,
        0.99,
        0.99,
    ))
    .expect("reads");
    assert_eq!(
        marks(
            &asks,
            &rule,
            &cleared_by(&[Move::RaiseTarget], Some(Move::RaiseTarget))
        ),
        Ok((false, Some(true)))
    );
    assert_eq!(
        marks(&asks, &rule, &cleared_by(&[], None)),
        Err(NOT_TRIED),
        "nothing was made"
    );
    let moves = read(&answers(
        "panel",
        &[
            ("move_target", 0.6),
            ("raise_target", 0.3),
            ("look_again", 0.05),
            ("ask_person", 0.05),
        ],
        0.8,
        0.99,
        0.99,
    ))
    .expect("reads");
    assert_eq!(
        marks(
            &moves,
            &rule,
            &cleared_by(&[Move::RaiseTarget], Some(Move::RaiseTarget))
        ),
        Err(NOT_TRIED)
    );
}

/// A broken head discards the answer whole.
#[test]
fn a_broken_head_refuses_the_whole_answer() {
    let mut broken = answers("window", &RAISE_FIRST, 0.8, 0.99, 0.99);
    broken[COVER_REVERSIBLE] = json!({ "type": "noul", "noul": 1.5 });
    assert!(read(&broken).is_err());
    let mut unknown = answers("window", &RAISE_FIRST, 0.8, 0.99, 0.99);
    unknown[COVER_MOVE]["choice"] = json!("close_it");
    assert!(read(&unknown).is_err());
}
