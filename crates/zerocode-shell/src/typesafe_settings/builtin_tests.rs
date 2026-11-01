use super::*;
use zerocode_core::jev::{PLACEMENT, builtin};

fn document(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn the_command_guard_editor_reads_two_typed_questions_and_their_options() {
    use zerocode_core::jev::{COMMAND_GUARD, questions};
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    let questions = builtin::default_questions(COMMAND_GUARD.id).unwrap();
    let editor = builtin_decision_editor(&path, COMMAND_GUARD.id, questions).unwrap();
    let value = serde_json::to_value(editor).unwrap();
    let definitions = value["questions"]
        .as_array()
        .expect("typed questions, not a flattened fingerprint");
    assert_eq!(definitions.len(), 2);
    for [id, instructions, yes, no] in questions::COMMAND_GUARD_QUESTIONS {
        let question = definitions
            .iter()
            .find(|row| row["id"] == id)
            .expect("each named question");
        assert_eq!(question["type"], "noul");
        assert_eq!(question["instructions"], instructions);
        assert_eq!(question["criteria"]["true"], yes);
        assert_eq!(question["criteria"]["false"], no);
    }
}

#[test]
fn builtin_edits_survive_settings_reloads_and_a_delete_is_final() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
    set_enabled(&path, true).unwrap();
    let draft = builtin::Decision {
        name: Some("My placement decision".into()),
        question: Some("Which placement helps this task?".into()),
        criteria: Some("Keep related work together.".into()),
        mode: Some(JevMode::Auto.key().into()),
        ..builtin::Decision::default()
    };
    update_builtin_decision(
        &path,
        PLACEMENT.id,
        "save",
        &builtin::Decision::default(),
        Some(draft),
    )
    .unwrap();
    let saved = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    assert_eq!(saved.mode.as_deref(), Some(JevMode::Shadow.key()));
    assert_eq!(
        document(&path)["smart"][PLACEMENT.setting],
        JevMode::Off.key()
    );
    assert_eq!(
        saved.question.as_deref(),
        Some("Which placement helps this task?")
    );
    update_builtin_decision(&path, PLACEMENT.id, "delete", &saved, None).unwrap();
    let deleted = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    assert!(deleted.deleted);
    set_enabled(&path, false).unwrap();
    set_enabled(&path, true).unwrap();
    assert_eq!(PLACEMENT.mode_in(&document(&path)), JevMode::Off);
    assert_eq!(
        document(&path)["smart"][PLACEMENT.setting],
        JevMode::Off.key()
    );
    assert!(
        builtin::in_root(&document(&path), PLACEMENT.id)
            .unwrap()
            .deleted,
        "turning every feature on again leaves a deleted decision deleted"
    );
    // No road leads back: restore is no action at all, and save and reset
    // would each write the decision back (save clears the mark, reset removes
    // the whole entry), so each is refused and the file stays as it was.
    let before = std::fs::read(&path).unwrap();
    let wording = builtin::Decision {
        name: Some("Back again".into()),
        ..builtin::Decision::default()
    };
    assert_eq!(
        update_builtin_decision(&path, PLACEMENT.id, "restore", &deleted, None),
        Err("builtin_decision_action".into())
    );
    for action in ["save", "reset"] {
        assert_eq!(
            update_builtin_decision(&path, PLACEMENT.id, action, &deleted, Some(wording.clone())),
            Err("builtin_decision_deleted".into()),
            "{action} on a deleted decision"
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        builtin::in_root(&document(&path), PLACEMENT.id)
            .unwrap()
            .deleted
    );
    assert_eq!(PLACEMENT.mode_in(&document(&path)), JevMode::Off);
    assert_eq!(document(&path)["theme"], "dark");
}

#[test]
fn a_stale_builtin_editor_cannot_overwrite_another_windows_choice() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    let blank = builtin::Decision::default();
    let changed = builtin::Decision {
        name: Some("Another window".into()),
        ..blank.clone()
    };
    update_builtin_decision(&path, PLACEMENT.id, "save", &blank, Some(changed)).unwrap();
    let before = std::fs::read(&path).unwrap();
    for action in ["save", "delete", "reset"] {
        assert_eq!(
            update_builtin_decision(&path, PLACEMENT.id, action, &blank, Some(blank.clone())),
            Err("builtin_decision_changed".into())
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn invalid_builtin_wording_writes_nothing_and_the_editor_reads_shipped_defaults() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    std::fs::write(&path, "{\"theme\":\"light\"}\n").unwrap();
    let before = std::fs::read(&path).unwrap();
    let too_long = builtin::Decision {
        question: Some("x".repeat(builtin::QUESTION_MAX + 1)),
        ..builtin::Decision::default()
    };
    assert_eq!(
        update_builtin_decision(
            &path,
            PLACEMENT.id,
            "save",
            &builtin::Decision::default(),
            Some(too_long)
        ),
        Err("builtin_decision_question".into())
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let questions = builtin::default_questions(PLACEMENT.id).unwrap();
    let editor = builtin_decision_editor(&path, PLACEMENT.id, questions.clone()).unwrap();
    assert_eq!(editor.questions, questions);
    assert_eq!(editor.legacy_question_max, builtin::LEGACY_QUESTION_MAX);
    assert_eq!(editor.question_max, builtin::QUESTION_MAX);
    assert!(editor.can_edit_prompt);
    assert!(editor.saved.is_empty());
}

#[test]
fn typed_questions_survive_save_and_reset_and_a_delete_keeps_them_out_of_reach() {
    use zerocode_core::jev::COMMAND_GUARD;
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    let blank = builtin::Decision::default();
    let mut draft = blank.clone();
    let mut question = builtin::default_questions(COMMAND_GUARD.id)
        .unwrap()
        .remove(0);
    question.question.criteria["true"] =
        serde_json::json!({"what": "The original cannot be recovered."});
    draft
        .questions
        .insert(question.id.clone(), question.question);
    draft.mode = Some("auto".into());
    update_builtin_decision(&path, COMMAND_GUARD.id, "save", &blank, Some(draft.clone())).unwrap();
    let saved = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    assert_eq!(saved.mode.as_deref(), Some("shadow"));
    assert_eq!(document(&path)["smart"][COMMAND_GUARD.setting], "off");
    assert!(saved.rubric(COMMAND_GUARD.rubric_version).is_some());
    let before = std::fs::read(&path).unwrap();
    let mut invalid = saved.clone();
    invalid.questions.get_mut(&question.id).unwrap().kind = builtin::QuestionKind::Choice;
    assert!(
        update_builtin_decision(&path, COMMAND_GUARD.id, "save", &saved, Some(invalid)).is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    update_builtin_decision(&path, COMMAND_GUARD.id, "reset", &saved, None).unwrap();
    assert!(builtin::in_root(&document(&path), COMMAND_GUARD.id).is_none());
    update_builtin_decision(&path, COMMAND_GUARD.id, "save", &blank, Some(draft)).unwrap();
    let saved = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    update_builtin_decision(&path, COMMAND_GUARD.id, "delete", &saved, None).unwrap();
    let deleted = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    assert!(deleted.deleted);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        update_builtin_decision(&path, COMMAND_GUARD.id, "restore", &deleted, None),
        Err("builtin_decision_action".into())
    );
    assert_eq!(
        update_builtin_decision(&path, COMMAND_GUARD.id, "reset", &deleted, None),
        Err("builtin_decision_deleted".into())
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

const EDITED_NAME: &str = "My placement decision";
const EDITED_QUESTION: &str = "Which placement helps this task?";
const EDITED_CRITERIA: &str = "Keep related work together.";

/// A delete is the end of the wording too: what the person wrote does not stay
/// in the settings file, and the editor's definition door does not hand it
/// back. The mark and the feature's `off` stay.
#[test]
fn a_deleted_builtin_decision_keeps_none_of_the_wording_the_person_wrote() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
    set_enabled(&path, true).unwrap();
    let draft = builtin::Decision {
        name: Some(EDITED_NAME.into()),
        question: Some(EDITED_QUESTION.into()),
        criteria: Some(EDITED_CRITERIA.into()),
        ..builtin::Decision::default()
    };
    update_builtin_decision(
        &path,
        PLACEMENT.id,
        "save",
        &builtin::Decision::default(),
        Some(draft),
    )
    .unwrap();
    let saved = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(EDITED_NAME)
    );
    update_builtin_decision(&path, PLACEMENT.id, "delete", &saved, None).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    for text in [EDITED_NAME, EDITED_QUESTION, EDITED_CRITERIA] {
        assert!(
            !raw.contains(text),
            "the settings file still holds {text:?}"
        );
    }
    let deleted = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    assert_eq!(
        deleted,
        builtin::Decision {
            deleted: true,
            mode: deleted.mode.clone(),
            ..builtin::Decision::default()
        },
        "only the mark (and the mode word) is kept"
    );
    assert_eq!(
        document(&path)["smart"][PLACEMENT.setting],
        JevMode::Off.key()
    );
    let editor = builtin_decision_editor(&path, PLACEMENT.id, Vec::new()).unwrap();
    assert!(editor.saved.deleted);
    assert_eq!(
        (
            editor.saved.name,
            editor.saved.question,
            editor.saved.criteria
        ),
        (None, None, None),
        "the definition door hands back no wording"
    );
}

/// A shipped question that changed since the person saved their edit makes the
/// stored edit fail core's check; a delete of that decision still goes
/// through, since it keeps none of the stored edit.
#[test]
fn a_delete_goes_through_when_the_stored_questions_no_longer_fit_the_shipped_ones() {
    use zerocode_core::jev::COMMAND_GUARD;
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    let blank = builtin::Decision::default();
    let mut draft = blank.clone();
    let mut question = builtin::default_questions(COMMAND_GUARD.id)
        .unwrap()
        .remove(0);
    question.question.criteria["true"] = serde_json::json!({"what": "The edit the person made."});
    draft
        .questions
        .insert(question.id.clone(), question.question);
    update_builtin_decision(&path, COMMAND_GUARD.id, "save", &blank, Some(draft)).unwrap();
    let mut stale = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    stale.questions.get_mut(&question.id).unwrap().kind = builtin::QuestionKind::Choice;
    assert!(builtin::validate(COMMAND_GUARD.id, &stale).is_err());
    let mut root = document(&path);
    root["smart"][builtin::SETTING][COMMAND_GUARD.id] = builtin::storage_value(&stale).unwrap();
    std::fs::write(&path, serde_json::to_vec(&root).unwrap()).unwrap();
    let expected = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    assert_eq!(expected, stale);
    update_builtin_decision(&path, COMMAND_GUARD.id, "delete", &expected, None)
        .expect("a delete of an edit that no longer fits is not refused");
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("The edit the person made."));
    assert!(
        builtin::in_root(&document(&path), COMMAND_GUARD.id)
            .unwrap()
            .deleted
    );
}

/// A reset after an edit returns the feature to the mode shipped for it.
#[test]
fn a_reset_after_an_edit_returns_the_mode_to_the_recommended_one() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("settings.json");
    set_enabled(&path, true).unwrap();
    let draft = builtin::Decision {
        question: Some(EDITED_QUESTION.into()),
        ..builtin::Decision::default()
    };
    update_builtin_decision(
        &path,
        PLACEMENT.id,
        "save",
        &builtin::Decision::default(),
        Some(draft),
    )
    .unwrap();
    assert_eq!(PLACEMENT.mode_in(&document(&path)), JevMode::Shadow);
    let saved = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    update_builtin_decision(&path, PLACEMENT.id, "reset", &saved, None).unwrap();
    assert!(builtin::in_root(&document(&path), PLACEMENT.id).is_none());
    assert_eq!(PLACEMENT.mode_in(&document(&path)), PLACEMENT.recommended);
}
