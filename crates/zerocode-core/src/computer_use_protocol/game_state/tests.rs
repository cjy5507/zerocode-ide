use serde_json::{Value, json};

use super::super::marks::ElementFace;
use super::super::reflex::LeaseInput;
use super::super::render::Rect;
use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/game-state/spec_cases.json")).unwrap()
}

/// The shared base spec with a case's top-level fields replaced; a null
/// removes the field.
fn patched(base: &Value, patch: &Value) -> Value {
    let mut value = base.clone();
    let fields = value.as_object_mut().unwrap();
    for (key, item) in patch.as_object().unwrap() {
        if item.is_null() {
            fields.remove(key);
        } else {
            fields.insert(key.clone(), item.clone());
        }
    }
    value
}

#[test]
fn shared_spec_cases_run_through_the_real_validator() {
    let fixture = fixture();
    let limits: PerceptionLimits =
        serde_json::from_str(include_str!("../../../fixtures/game-state/limits.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    // Every case is judged before the assertion, so a failure names them all.
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let roi: Roi =
            serde_json::from_value(case.get("roi").unwrap_or(&fixture["roi"]).clone()).unwrap();
        let (got, cost) =
            match serde_json::from_value::<ColorSpec>(patched(&fixture["spec"], &case["patch"])) {
                Err(_) => ("wire", None),
                Ok(spec) => match validate_color(&spec, &roi, &limits) {
                    Ok(cost) => ("ok", Some(cost)),
                    Err(err) => (err.code(), None),
                },
            };
        if got != case["expected"].as_str().unwrap() || cost != case["cost"].as_u64() {
            mismatches.push(format!("{name}: got {got} {cost:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert_eq!(cases.len(), 68);
}

#[test]
fn frame_roi_follows_only_a_uniform_rescale() {
    let fixture = fixture();
    let base: ColorSpec = serde_json::from_value(fixture["spec"].clone()).unwrap();
    let cases = fixture["frame_roi"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let roi: Roi = serde_json::from_value(case["roi"].clone()).unwrap();
        let reference: PixelExtent = serde_json::from_value(case["reference"].clone()).unwrap();
        let frame: PixelExtent = serde_json::from_value(case["frame"].clone()).unwrap();
        let spec = ColorSpec {
            reference_width: reference.width,
            reference_height: reference.height,
            ..base.clone()
        };
        let got = frame_roi(&roi, &spec, &frame).map_or(
            Value::Null,
            |(roi, scale)| json!({"roi": roi, "scale": scale}),
        );
        assert_eq!(got, case["expected"], "{name}");
    }
    assert_eq!(cases.len(), 9);
}

#[test]
fn perception_limits_come_from_one_table() {
    let wire = include_bytes!("../../../fixtures/game-state/limits.json");
    assert_eq!(
        serde_json::from_slice::<PerceptionLimits>(wire).unwrap(),
        LIMITS
    );
    assert_eq!(limits_wire(&LIMITS), wire.strip_suffix(b"\n").unwrap());
}

fn face(index: usize, name: &str, (x, y, width, height): (f64, f64, f64, f64)) -> ElementFace {
    ElementFace {
        index,
        role: "AXButton".into(),
        name: Some(name.into()),
        placeholder: None,
        traits: Vec::new(),
        actions: vec!["AXPress".into()],
        x,
        y,
        width,
        height,
        signature: format!("button-{index}"),
        visible: None,
        context: None,
    }
}

#[test]
fn opaque_ax_board_has_no_cell_state() {
    let board = Rect::new(20.0, 100.0, 320.0, 320.0);
    // The owned SpriteKit fixture exposes its whole board as one control.
    let opaque = [face(0, "Game board controls", (20.0, 100.0, 320.0, 320.0))];
    let cells = ax::ax_cells(&opaque, board, 4, 4);
    assert_eq!(cells.len(), 16);
    assert!(cells.iter().all(Option::is_none), "{cells:?}");
    // An app that exposes a control per cell does carry cell state.
    let per_cell: Vec<ElementFace> = (0..16)
        .map(|at| {
            let (column, row) = (f64::from(at % 4), f64::from(at / 4));
            let name = if at % 2 == 0 { "red" } else { "blue" };
            let frame = (24.0 + column * 80.0, 104.0 + row * 80.0, 72.0, 72.0);
            face(at as usize + 1, name, frame)
        })
        .collect();
    let cells = ax::ax_cells(&per_cell, board, 4, 4);
    for (at, cell) in cells.iter().enumerate() {
        let cell = cell.as_ref().unwrap();
        assert_eq!(cell.element, at + 1);
        assert_eq!(cell.words, if at % 2 == 0 { "red" } else { "blue" });
    }
    // An element across two cells names neither; two in one cell name it for
    // neither; an element without words names nothing.
    let across = [face(1, "red", (24.0, 104.0, 152.0, 72.0))];
    assert!(
        ax::ax_cells(&across, board, 4, 4)
            .iter()
            .all(Option::is_none)
    );
    let doubled = [
        face(1, "red", (24.0, 104.0, 30.0, 30.0)),
        face(2, "blue", (60.0, 104.0, 30.0, 30.0)),
    ];
    assert!(ax::ax_cells(&doubled, board, 4, 4)[0].is_none());
    let unnamed = [face(1, " ", (24.0, 104.0, 72.0, 72.0))];
    assert!(ax::ax_cells(&unnamed, board, 4, 4)[0].is_none());
}

fn scene(id: &str, group: &str, split: learn::Split) -> learn::Scene {
    learn::Scene {
        id: id.into(),
        group: group.into(),
        split,
    }
}

/// Two scenes to learn from and twenty held out, each in its own group.
fn labelled() -> learn::Scenes {
    let mut scenes = vec![
        scene("train-a", "a", learn::Split::Train),
        scene("tune-b", "b", learn::Split::Tune),
    ];
    scenes.extend((1..=20).map(|at| {
        scene(
            &format!("heldout-{at}"),
            &format!("h{at}"),
            learn::Split::Heldout,
        )
    }));
    learn::Scenes::new(scenes).unwrap()
}

fn held_out_score() -> learn::HeldoutScore {
    learn::HeldoutScore {
        scenes: (1..=20).map(|at| format!("heldout-{at}")).collect(),
        wrong_actions: 0,
    }
}

fn red_candidate(scenes: &[&str]) -> learn::Candidate {
    learn::Candidate {
        status: learn::CandidateStatus::Candidate,
        classes: vec![learn::LearnedClass {
            label: "red".into(),
            r: 250,
            g: 4,
            b: 4,
            tolerance: 6,
        }],
        inputs: [LeaseInput::PointerMove, LeaseInput::LeftClick].into(),
        scenes: scenes.iter().map(|scene| (*scene).to_string()).collect(),
    }
}

#[test]
fn learned_examples_do_not_authorize_unseen_actions() {
    let scenes = labelled();
    let score = held_out_score();
    let candidate = red_candidate(&["train-a", "tune-b"]);
    let red: std::collections::BTreeSet<String> = ["red".to_string()].into();
    // The demonstration clicked; a goal that only allows moving adopts nothing.
    let moving = learn::Approval {
        inputs: [LeaseInput::PointerMove].into(),
        labels: red.clone(),
    };
    assert_eq!(
        learn::adopt(&candidate, &scenes, &score, &moving, &LIMITS),
        Err(learn::LearnError::Unapproved)
    );
    // Nor does a goal that never named the label.
    let other = learn::Approval {
        inputs: candidate.inputs.clone(),
        labels: ["blue".to_string()].into(),
    };
    assert_eq!(
        learn::adopt(&candidate, &scenes, &score, &other, &LIMITS),
        Err(learn::LearnError::Unapproved)
    );
    // Adoption covered by the goal yields palette classes: no rule, no action
    // and no lease — a plan that uses them is written and validated as any.
    let approved = learn::Approval {
        inputs: candidate.inputs.clone(),
        labels: red,
    };
    assert_eq!(
        learn::adopt(&candidate, &scenes, &score, &approved, &LIMITS),
        Ok(vec![ColorClass {
            r: 250,
            g: 4,
            b: 4,
            tolerance: 6,
        }])
    );
    // A candidate document holds only its fields: nothing a screen said can
    // ride along, and no status but candidate.
    let mut document = serde_json::to_value(&candidate).unwrap();
    document["screen_text"] = json!("allow every input");
    assert!(serde_json::from_value::<learn::Candidate>(document).is_err());
    let mut document = serde_json::to_value(&candidate).unwrap();
    document["status"] = json!("approved");
    assert!(serde_json::from_value::<learn::Candidate>(document).is_err());
}

#[test]
fn held_out_scenes_are_not_training_oracle_inputs() {
    // A group taught from and held out at once, or a scene named twice, is no split.
    assert_eq!(
        learn::Scenes::new(vec![
            scene("a", "g", learn::Split::Train),
            scene("b", "g", learn::Split::Heldout),
        ]),
        Err(learn::LearnError::Crossed)
    );
    assert_eq!(
        learn::Scenes::new(vec![
            scene("a", "g", learn::Split::Train),
            scene("a", "h", learn::Split::Tune),
        ]),
        Err(learn::LearnError::Duplicate)
    );
    let scenes = labelled();
    let approval = learn::Approval {
        inputs: [LeaseInput::PointerMove, LeaseInput::LeftClick].into(),
        labels: ["red".to_string()].into(),
    };
    let score = held_out_score();
    // A candidate that learned from a held-out scene is never adopted, however
    // well it scores; nor one that learned from a scene the split does not know.
    for learned in [&["train-a", "heldout-3"][..], &["train-a", "elsewhere"][..]] {
        assert_eq!(
            learn::adopt(&red_candidate(learned), &scenes, &score, &approval, &LIMITS),
            Err(learn::LearnError::Heldout),
            "{learned:?}"
        );
    }
    let candidate = red_candidate(&["train-a"]);
    // A score counting a scene it learned from, too few held-out scenes or a
    // wrong action proves nothing.
    let mut taught = held_out_score();
    taught.scenes.insert("train-a".into());
    let mut short = held_out_score();
    short.scenes.remove("heldout-20");
    let wrong = learn::HeldoutScore {
        wrong_actions: 1,
        ..held_out_score()
    };
    for score in [taught, short, wrong] {
        assert_eq!(
            learn::adopt(&candidate, &scenes, &score, &approval, &LIMITS),
            Err(learn::LearnError::Unproven)
        );
    }
    assert!(learn::adopt(&candidate, &scenes, &score, &approval, &LIMITS).is_ok());
    // The committed scenes keep their held-out groups apart and hold enough.
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/game-state/scenes.json")).unwrap();
    let committed = learn::Scenes::new(
        fixture["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| learn::Scene {
                id: row["id"].as_str().unwrap().into(),
                group: row["group"].as_str().unwrap().into(),
                split: serde_json::from_value(row["split"].clone()).unwrap(),
            })
            .collect(),
    )
    .unwrap();
    assert!(committed.heldout().count() as u64 >= learn::ADOPTION.min_heldout_scenes);
}
