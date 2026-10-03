use super::*;

fn origin(version: u32) -> FeedbackOrigin {
    FeedbackOrigin {
        artifact_id: "page-example".into(),
        version,
        sha256: Some("a".repeat(64)),
        feedback_key: "b".repeat(64),
        followed_link: false,
    }
}

#[test]
fn project_preferences_stay_in_their_project_and_personal_preferences_follow_the_person() {
    let mut book = PreferenceBook::default();
    book.save(
        "Use concise headings",
        PreferenceScope::Personal,
        origin(1),
        10,
    )
    .unwrap();
    book.save(
        "Use the project palette",
        PreferenceScope::Project {
            key: "a".repeat(64),
        },
        origin(2),
        20,
    )
    .unwrap();
    assert_eq!(book.active_for(Some(&"a".repeat(64))).len(), 2);
    assert_eq!(book.active_for(Some(&"b".repeat(64))).len(), 1);
    assert_eq!(book.active_for(None).len(), 1);
}

#[test]
fn repeat_saves_are_idempotent_and_revocation_survives_reopening() {
    let mut book = PreferenceBook::default();
    let id = book
        .save(
            "Prefer short labels",
            PreferenceScope::Personal,
            origin(1),
            10,
        )
        .unwrap();
    let first = book.encode().unwrap();
    assert_eq!(
        book.save(
            " Prefer short labels ",
            PreferenceScope::Personal,
            origin(1),
            20
        )
        .unwrap(),
        id
    );
    assert_eq!(book.encode().unwrap(), first);
    assert!(book.revoke(&id).unwrap());
    assert!(!book.revoke(&id).unwrap());
    let reopened = PreferenceBook::parse(&book.encode().unwrap()).unwrap();
    assert!(reopened.active_for(None).is_empty());
}

#[test]
fn provenance_tracks_the_recorded_artifact_version_without_repeating_a_preference() {
    let mut book = PreferenceBook::default();
    book.save(
        "Use readable contrast",
        PreferenceScope::Personal,
        origin(1),
        10,
    )
    .unwrap();
    book.save(
        "Use readable contrast",
        PreferenceScope::Personal,
        origin(2),
        20,
    )
    .unwrap();
    assert_eq!(book.entries().len(), 1);
    assert_eq!(book.entries()[0].origin.version, 2);
    assert_eq!(book.entries()[0].created_ms, 10);
    assert_eq!(book.entries()[0].updated_ms, 20);
}

#[test]
fn invalid_or_unbounded_requests_do_not_mutate_the_preference_book() {
    let mut book = PreferenceBook::default();
    let empty = book.clone();
    for text in ["", "two\nlines", &"x".repeat(MAX_TEXT_BYTES + 1)] {
        assert_eq!(
            book.save(text, PreferenceScope::Personal, origin(1), 10),
            Err(PreferenceError::InvalidText)
        );
        assert_eq!(book, empty);
    }
    for index in 0..MAX_PER_SCOPE {
        book.save(
            &format!("Preference {index}"),
            PreferenceScope::Personal,
            origin(1),
            10,
        )
        .unwrap();
    }
    let full = book.clone();
    assert_eq!(
        book.save("One too many", PreferenceScope::Personal, origin(1), 10),
        Err(PreferenceError::ScopeFull)
    );
    assert_eq!(book, full);
}

#[test]
fn damaged_or_newer_state_is_refused_instead_of_reset_to_empty() {
    assert_eq!(
        PreferenceBook::parse(b"broken"),
        Err(PreferenceError::InvalidDocument)
    );
    assert_eq!(
        PreferenceBook::parse(br#"{"version":2,"generation":0,"entries":[]}"#),
        Err(PreferenceError::InvalidDocument)
    );
    let mut book = PreferenceBook::default();
    book.save(
        "Prefer readable labels",
        PreferenceScope::Personal,
        origin(1),
        10,
    )
    .unwrap();
    let mut value = serde_json::to_value(&book).unwrap();
    value["entries"][0]["text"] = serde_json::json!("text changed without a matching identity");
    assert!(PreferenceBook::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn nested_directories_share_the_same_workspace_key() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let nested = root.path().join("src/component");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(project_key(root.path()), project_key(&nested));
}

#[test]
fn registered_worktrees_share_project_preferences_but_one_way_pointers_do_not() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main");
    let linked = directory.path().join("linked");
    let stranger = directory.path().join("stranger");
    let gitdir = main.join(".git/worktrees/linked");
    for folder in [&gitdir, &linked, &stranger] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        gitdir.join("gitdir"),
        linked.join(".git").to_string_lossy().as_bytes(),
    )
    .unwrap();
    let pointer = format!("gitdir: {}\n", gitdir.display());
    std::fs::write(linked.join(".git"), &pointer).unwrap();
    std::fs::write(stranger.join(".git"), pointer).unwrap();
    assert_eq!(project_key(&main), project_key(&linked));
    assert_ne!(project_key(&main), project_key(&stranger));
}
