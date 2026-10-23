use super::*;
use serde_json::json;

fn edit(value: Value) -> Decision {
    serde_json::from_value(json!({"questions": value})).unwrap()
}

#[test]
fn a_choice_edit_never_offers_an_unavailable_action() {
    let draft = edit(
        json!({"place": {"type": "choice", "instructions": "Where should this task run?",
        "criteria": {"tab": {"what": "Keep it together", "examples": ["review"]}, "split": "Beside it"}}}),
    );
    let mut body = json!({"questions": {"place": {"type": "choice", "instructions": "Original",
        "criteria": {"tab": "A tab", "background": null}}}});
    apply(&draft, &mut body).unwrap();
    assert_eq!(
        body["questions"]["place"]["criteria"],
        json!({
            "tab": {"what": "Keep it together", "examples": ["review"]}, "background": null
        })
    );
}

#[test]
fn score_levels_keep_their_order_and_refuse_a_changed_answer_scale() {
    let draft = edit(
        json!({"difficulty": {"type": "score", "instructions": "How hard?",
        "criteria": ["One file", {"work": "Several files"}, null]}}),
    );
    let original = json!({"questions": {"difficulty": {"type": "score", "instructions": "Original",
        "criteria": ["Small", "Medium", "Large"]}}});
    let mut body = original.clone();
    apply(&draft, &mut body).unwrap();
    assert_eq!(
        body["questions"]["difficulty"]["criteria"],
        json!(["One file", {"work": "Several files"}, null])
    );
    body["questions"]["difficulty"]["criteria"] = json!(["Small", "Large"]);
    let before = body.clone();
    assert!(apply(&draft, &mut body).is_err());
    assert_eq!(body, before);
}

#[test]
fn editing_a_batch_question_preserves_each_subject_and_its_other_head() {
    let draft = edit(
        json!({"q*": {"type": "choice", "instructions": "Prioritize blocked work.",
        "criteria": {"now": "Someone is waiting."}, "contextual": true}}),
    );
    let mut body = json!({"questions": {
        "q9": {"type": "choice", "instructions": "Evaluate letters[0].", "criteria": {"now": "Now", "later": "Later"}},
        "q10": {"type": "choice", "instructions": "Evaluate letters[1].", "criteria": {"now": "Now", "later": "Later"}},
        "q9/urgent": {"type": "noul", "instructions": "Is letters[0] urgent?", "criteria": null}
    }});
    let urgent = body["questions"]["q9/urgent"].clone();
    apply(&draft, &mut body).unwrap();
    for (id, scope) in [
        ("q9", "Evaluate letters[0]."),
        ("q10", "Evaluate letters[1]."),
    ] {
        assert_eq!(body["questions"][id]["instructions"]["item_context"], scope);
        assert_eq!(
            body["questions"][id]["instructions"]["instructions"],
            "Prioritize blocked work."
        );
    }
    assert_eq!(body["questions"]["q9/urgent"], urgent);
}

#[test]
fn typed_wording_takes_precedence_over_legacy_common_instructions() {
    let mut draft = edit(
        json!({"guard": {"type": "noul", "instructions": "Only this question.", "criteria": {"true": null}}}),
    );
    draft.question = Some("Old common question.".into());
    let mut body = json!({"state": {}, "questions": {
        "guard": {"type": "noul", "instructions": "Guard", "criteria": {"true": "Yes", "false": "No"}},
        "other": {"type": "noul", "instructions": "Other", "criteria": null}
    }});
    apply(&draft, &mut body).unwrap();
    assert_eq!(
        body["questions"]["guard"]["instructions"],
        "Only this question."
    );
    assert_eq!(
        body["questions"]["guard"]["criteria"],
        json!({"true": null, "false": "No"})
    );
    assert!(
        body["questions"]["other"]["instructions"]
            .as_str()
            .unwrap()
            .contains("__builtinDecision")
    );
}

#[test]
fn typed_rubrics_depend_on_content_and_not_object_key_order() {
    let one = edit(serde_json::from_str(r#"{"guard":{"type":"noul","instructions":"Safe?","criteria":{"true":{"a":"A","b":"B"},"false":"No"}}}"#).unwrap());
    let mut two = edit(serde_json::from_str(r#"{"guard":{"criteria":{"false":"No","true":{"b":"B","a":"A"}},"instructions":"Safe?","type":"noul"}}"#).unwrap());
    assert_eq!(one.rubric(1), two.rubric(1));
    two.questions.get_mut("guard").unwrap().criteria["false"] = json!("Something else");
    assert_ne!(one.rubric(1), two.rubric(1));
}

#[test]
fn the_saved_builtin_schema_cannot_change_types_keys_or_scale() {
    let definitions = default_questions(crate::jev::COMMAND_GUARD.id).unwrap();
    let original = definitions[0].clone();
    let mut draft = Decision::default();
    draft
        .questions
        .insert(original.id.clone(), original.question);
    assert!(validate(crate::jev::COMMAND_GUARD.id, &draft).is_ok());
    draft.questions.get_mut(&original.id).unwrap().kind = QuestionKind::Choice;
    assert_eq!(
        validate(crate::jev::COMMAND_GUARD.id, &draft),
        Err("builtin_decision_question_schema")
    );
    draft.questions.clear();
    draft
        .questions
        .insert("invented".into(), definitions[0].question.clone());
    assert_eq!(
        validate(crate::jev::COMMAND_GUARD.id, &draft),
        Err("builtin_decision_question_schema")
    );
}

#[test]
fn an_unchanged_decision_preserves_the_request_and_rubric() {
    let mut body = json!({"state": {"item": "value"}, "questions": {"asked": {
        "type": "choice", "instructions": {"task": "Read"}, "criteria": {"one": null, "two": ["A", "B"]}
    }}});
    let bytes = body.to_string();
    apply(&Decision::default(), &mut body).unwrap();
    assert_eq!(body.to_string(), bytes);
    assert_eq!(Decision::default().rubric(1), None);
}

#[test]
fn editing_noul_instructions_does_not_invent_unspecified_criteria() {
    let draft =
        edit(json!({"asked": {"type": "noul", "instructions": "Changed?", "criteria": {}}}));
    let mut body = json!({"questions": {"asked": {"type": "noul", "instructions": "Before?", "criteria": null}}});
    apply(&draft, &mut body).unwrap();
    assert_eq!(body["questions"]["asked"]["instructions"], "Changed?");
    assert!(body["questions"]["asked"]["criteria"].is_null());
}

#[test]
fn every_shipped_core_definition_fits_the_editing_contract() {
    for seat in crate::jev::JEV_USES {
        let Some(definitions) = default_questions(seat.id) else {
            continue;
        };
        let edits = definitions
            .iter()
            .map(|row| (row.id.clone(), row.question.clone()))
            .collect();
        validate_against(&edits, &definitions)
            .unwrap_or_else(|error| panic!("{}: {error}", seat.id));
    }
}

#[test]
fn a_client_that_cannot_read_typed_edits_stays_off_even_for_an_explicit_on() {
    let seat = &crate::jev::COMMAND_GUARD;
    let mut draft = edit(
        json!({"irreversible": {"type": "noul", "instructions": "Recoverable?", "criteria": {}}}),
    );
    draft.mode = Some(JevMode::On.key().into());
    let stored = storage_value(&draft).unwrap();
    // The previous client ignores the new fields, and already honors deleted.
    let mut older: Decision = serde_json::from_value(stored.clone()).unwrap();
    older.questions.clear();
    assert_eq!(older.mode(seat), Some(JevMode::Off));
    let root = json!({"smart": {(SETTING): {(seat.id): stored}}});
    assert_eq!(in_root(&root, seat.id), Some(draft.clone()));
    assert_eq!(
        in_root(&root, seat.id).unwrap().mode(seat),
        Some(JevMode::On)
    );
    assert_eq!(serde_json::to_value(&draft).unwrap()["deleted"], false);

    for bad in [json!(null), json!(3), json!("2")] {
        let mut changed = root.clone();
        changed["smart"][SETTING][seat.id]["questionSchema"] = bad;
        assert_eq!(
            in_root(&changed, seat.id).unwrap().mode(seat),
            Some(JevMode::Off)
        );
    }
}

#[test]
fn credential_descriptions_are_editable_but_credential_values_are_refused() {
    let mut draft = edit(
        json!({"recovery": {"type": "choice", "instructions": "Does the dialog ask for a password?",
        "criteria": {"ask_person": "The person handles their own password."}}}),
    );
    assert!(schema::validate_edits(&draft.questions).is_ok());
    let secret = ["unit", "fixture", "placeholder"].join("-");
    for value in [
        json!(format!("Authorization: Bearer {secret}")),
        json!({"password": secret}),
        json!({"nested": [{"api_key": secret}]}),
    ] {
        draft.questions.get_mut("recovery").unwrap().criteria["ask_person"] = value;
        assert_eq!(
            schema::validate_edits(&draft.questions),
            Err("builtin_decision_credential")
        );
    }
}

#[test]
fn legacy_wording_keeps_its_wire_bound_when_typed_questions_get_a_larger_editor() {
    let draft = Decision {
        question: Some("q".repeat(LEGACY_QUESTION_MAX + 1)),
        ..Decision::default()
    };
    assert_eq!(
        validate(crate::jev::COMMAND_GUARD.id, &draft),
        Err("builtin_decision_question")
    );
    let mut body = json!({"state": {}, "questions": {"irreversible": {"type": "noul", "instructions": "Before?", "criteria": null}}});
    apply(&draft, &mut body).unwrap();
    assert!(
        body["state"][DECISION_STATE_KEY]["question"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= LEGACY_QUESTION_MAX
    );
}
