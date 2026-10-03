use super::*;
use zerocode_core::user_preferences::FeedbackOrigin;

fn save(root: &Path, book: &PreferenceBook) {
    crate::secure_fs::ensure_private_dir(root, Path::new(DIRECTORY_NAME)).unwrap();
    crate::secure_fs::write_atomic_owner_only(root, &Path::new(DIRECTORY_NAME).join(FILE_NAME), &book.encode().unwrap()).unwrap();
}

fn origin() -> FeedbackOrigin {
    FeedbackOrigin { artifact_id: "page-example".into(), version: 2, sha256: None, feedback_key: "a".repeat(64), followed_link: false }
}

#[test]
fn preferences_are_live_scoped_and_revocation_replaces_the_last_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let source = UserPreferenceSource::at(&root, &root);
    assert!(source.reminder().is_none());
    let mut book = PreferenceBook::default();
    let id = book.save("Use concise headings", PreferenceScope::Personal, origin(), 10).unwrap();
    book.save("A different project only", PreferenceScope::Project { key: "b".repeat(64) }, origin(), 10).unwrap();
    save(&root, &book);
    let first = source.reminder().unwrap();
    assert!(first.contains("Use concise headings"));
    assert!(!first.contains("A different project only"));
    assert_eq!(source.reminder().unwrap(), first);
    book.revoke(&id).unwrap();
    save(&root, &book);
    let revoked = source.reminder().unwrap();
    assert!(!revoked.contains("Use concise headings"));
    assert!(revoked.contains("[]"));
    book.save("Use concise headings", PreferenceScope::Personal, origin(), 20).unwrap();
    save(&root, &book);
    assert_ne!(source.reminder().unwrap(), first);
}

#[test]
fn missing_or_invalid_state_after_prior_context_does_not_reuse_old_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let source = UserPreferenceSource::at(&root, &root);
    source.note_prior_context();
    assert!(source.reminder().unwrap().contains("[]"));
    std::fs::create_dir_all(root.join(DIRECTORY_NAME)).unwrap();
    std::fs::write(root.join(DIRECTORY_NAME).join(FILE_NAME), b"invalid").unwrap();
    assert!(source.reminder().unwrap().contains("unavailable"));
    save(&root, &PreferenceBook::default());
    assert!(source.reminder().unwrap().contains("[]"));
}

#[test]
fn withheld_recall_revokes_old_context_without_reading_or_resurfacing_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let source = UserPreferenceSource::at(&root, &root);
    let mut book = PreferenceBook::default();
    book.save("Preference must stay private while recall is withheld", PreferenceScope::Personal, origin(), 10).unwrap();
    save(&root, &book);
    assert!(source.withheld_reminder().is_none());
    let first = source.reminder().unwrap();
    assert!(first.contains("Preference must stay private"));
    std::fs::remove_dir_all(root.join(DIRECTORY_NAME)).unwrap();
    std::fs::write(root.join(DIRECTORY_NAME), b"not a readable preference directory").unwrap();
    let withheld = source.withheld_reminder().unwrap();
    assert!(withheld.contains("withheld for this request"));
    assert!(!withheld.contains("Preference must stay private"));
    assert_eq!(source.withheld_reminder().unwrap(), withheld);
    std::fs::remove_file(root.join(DIRECTORY_NAME)).unwrap();
    save(&root, &book);
    let restored = source.reminder().unwrap();
    assert!(restored.contains("Preference must stay private"));
    assert_ne!(restored, first);
}

#[test]
fn the_secure_preference_reader_refuses_oversized_state_before_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("oversized.json");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(u64::try_from(MAX_BYTES + 1).unwrap()).unwrap();
    assert!(crate::secure_fs::read_regular_file_absolute_no_follow_bounded(&path, u64::try_from(MAX_BYTES).unwrap()).is_err());
}
