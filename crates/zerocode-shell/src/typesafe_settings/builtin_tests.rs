use super::*;
use zerocode_core::jev::{PLACEMENT, builtin};

fn document(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
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
    let words = builtin::default_words(PLACEMENT.id).unwrap();
    let editor = builtin_decision_editor(&path, PLACEMENT.id, &words).unwrap();
    assert_eq!(format!("{}\n{}", editor.question, editor.criteria), words);
    assert!(editor.can_edit_prompt);
    assert!(editor.saved.is_empty());
}
