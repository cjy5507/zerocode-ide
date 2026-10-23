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
fn builtin_edits_delete_restore_and_reset_survive_settings_reloads() {
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
            .deleted
    );
    update_builtin_decision(&path, PLACEMENT.id, "restore", &deleted, None).unwrap();
    let restored = builtin::in_root(&document(&path), PLACEMENT.id).unwrap();
    assert_eq!(restored.question, saved.question);
    assert_eq!(restored.mode, saved.mode);
    update_builtin_decision(&path, PLACEMENT.id, "reset", &restored, None).unwrap();
    assert!(builtin::in_root(&document(&path), PLACEMENT.id).is_none());
    assert_eq!(PLACEMENT.mode_in(&document(&path)), PLACEMENT.recommended);
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
    for action in ["save", "delete", "restore", "reset"] {
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
fn typed_questions_survive_save_delete_restore_and_reset() {
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
    update_builtin_decision(&path, COMMAND_GUARD.id, "save", &blank, Some(draft)).unwrap();
    let saved = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    assert_eq!(saved.mode.as_deref(), Some("shadow"));
    assert_eq!(document(&path)["smart"][COMMAND_GUARD.setting], "off");
    assert!(saved.rubric(COMMAND_GUARD.rubric_version).is_some());
    update_builtin_decision(&path, COMMAND_GUARD.id, "delete", &saved, None).unwrap();
    let deleted = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    update_builtin_decision(&path, COMMAND_GUARD.id, "restore", &deleted, None).unwrap();
    let restored = builtin::in_root(&document(&path), COMMAND_GUARD.id).unwrap();
    assert_eq!(saved.questions, restored.questions);
    let before = std::fs::read(&path).unwrap();
    let mut invalid = restored.clone();
    invalid.questions.get_mut(&question.id).unwrap().kind = builtin::QuestionKind::Choice;
    assert!(
        update_builtin_decision(&path, COMMAND_GUARD.id, "save", &restored, Some(invalid)).is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    update_builtin_decision(&path, COMMAND_GUARD.id, "reset", &restored, None).unwrap();
    assert!(builtin::in_root(&document(&path), COMMAND_GUARD.id).is_none());
}
