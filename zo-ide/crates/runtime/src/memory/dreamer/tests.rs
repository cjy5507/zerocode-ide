use super::{
    dream_at_cwd, evaluate_manual_apply_gate, run_dream_fusion_v0, run_quarantine_patch,
    DreamError, Dreamer, FsMemoryStore, JsonlLessonSource, LessonSource, ManualApplyGateRequest,
    MemoryStore, MemoryWriteRequest, QuarantineCheckCommand, QuarantinePatchRequest, WriteOutcome,
};
use decision_core::dreamer::{
    AdvisorRole, CandidateEvidence, CandidateKind, CandidateStatus, DreamJudgeDecision, LessonKind,
    LessonObservation, PatchRisk, PromotionPolicy, QuarantinePatchRun, SelfImproveCandidate,
};
use std::fs;
use std::path::Path;

/// Pin `ZO_CONFIG_HOME` to `home` for the duration of `f`, then restore it.
///
/// The var is process-global, so the caller must already hold the crate's one
/// env lock ([`crate::test_env_lock`]) — hence the guard argument. Taking it by
/// reference makes the requirement a compile error to forget, and keeps this
/// helper from introducing a *second* env mutex: a private lock here would be
/// held across `test_env_lock`, so every guarded test would queue behind the
/// whole crate's env-serialized suite (measured: a 0.05s test holding it for
/// tens of seconds, which reads as a hung run).
fn with_config_home<T>(
    _env: &std::sync::MutexGuard<'static, ()>,
    home: &Path,
    f: impl FnOnce() -> T,
) -> T {
    let previous = std::env::var_os("ZO_CONFIG_HOME");
    std::env::set_var("ZO_CONFIG_HOME", home);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match previous {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn obs(sig: &str, session: &str, verified: bool) -> LessonObservation {
    LessonObservation {
        signature: sig.to_string(),
        session_id: session.to_string(),
        lesson: format!("body {sig}"),
        summary: format!("summary {sig}"),
        kind: LessonKind::Gotcha,
        verified,
    }
}

/// In-memory source/store doubles so the orchestrator is tested without a
/// real filesystem (the point of the DIP seams).
struct VecSource(Vec<LessonObservation>);
impl LessonSource for VecSource {
    fn observations(&self) -> Vec<LessonObservation> {
        self.0.clone()
    }
}

#[derive(Default)]
struct MemStore {
    existing: Vec<String>,
    existing_summaries: Vec<(String, String)>,
    written: std::cell::RefCell<Vec<super::MemoryWriteRequest>>,
}
impl MemoryStore for MemStore {
    fn existing_slugs(&self) -> Vec<String> {
        self.existing.clone()
    }
    fn existing_summaries(&self) -> Vec<(String, String)> {
        if self.existing_summaries.is_empty() {
            self.existing
                .iter()
                .cloned()
                .map(|slug| (slug, String::new()))
                .collect()
        } else {
            self.existing_summaries.clone()
        }
    }
    fn write_entry(
        &self,
        entry: &super::MemoryWriteRequest,
    ) -> Result<WriteOutcome, super::DreamError> {
        self.written.borrow_mut().push(entry.clone());
        Ok(WriteOutcome::Created)
    }
}

#[test]
fn memory_store_lock_serializes_dreamer_and_hand_written_writers() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let dreamer_entry = super::MemoryWriteRequest {
        slug: "dreamer-lesson".to_string(),
        summary: "dreamer summary".to_string(),
        body: "# Dreamer lesson".to_string(),
    };
    let hand_written_entry = super::MemoryWriteRequest {
        slug: "manual-note".to_string(),
        summary: "manual summary".to_string(),
        body: "# Manual note".to_string(),
    };

    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        let dreamer_start = std::sync::Arc::clone(&start);
        let dreamer_memory_dir = &memory_dir;
        let dreamer_entry = &dreamer_entry;
        let dreamer = scope.spawn(move || {
            dreamer_start.wait();
            super::write_memory_entry_transaction(
                dreamer_memory_dir,
                dreamer_entry,
                super::MemoryWriteOwner::Dreamer,
            )
        });
        let hand_written_start = std::sync::Arc::clone(&start);
        let hand_written_memory_dir = &memory_dir;
        let hand_written_entry = &hand_written_entry;
        let hand_written = scope.spawn(move || {
            hand_written_start.wait();
            super::write_memory_entry_transaction(
                hand_written_memory_dir,
                hand_written_entry,
                super::MemoryWriteOwner::HandWritten,
            )
        });

        dreamer.join().unwrap().unwrap();
        hand_written.join().unwrap().unwrap();
    });

    let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    assert!(index.contains("](dreamer-lesson.md)"));
    assert!(index.contains("](manual-note.md)"));
    assert_eq!(
        fs::read_to_string(memory_dir.join("dreamer-lesson.md")).unwrap(),
        "# Dreamer lesson\n"
    );
    assert_eq!(
        fs::read_to_string(memory_dir.join("manual-note.md")).unwrap(),
        "# Manual note\n"
    );
}

#[cfg(unix)]
#[test]
fn memory_store_transaction_stays_on_locked_directory_after_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir(&memory_dir).unwrap();
    let lock = super::acquire_memory_store_lock(&memory_dir).unwrap();
    let original_dir = tmp.path().join("original-memory");
    fs::rename(&memory_dir, &original_dir).unwrap();
    fs::create_dir(&memory_dir).unwrap();
    fs::write(memory_dir.join("MEMORY.md"), "replacement index\n").unwrap();

    super::write_memory_entry_transaction_locked(
        &lock,
        &super::MemoryWriteRequest {
            slug: "retained-note".to_string(),
            summary: "retained summary".to_string(),
            body: "# Retained note".to_string(),
        },
        super::MemoryWriteOwner::Dreamer,
    )
    .unwrap();

    assert!(original_dir.join("retained-note.md").exists());
    assert!(fs::read_to_string(original_dir.join("MEMORY.md"))
        .unwrap()
        .contains("](retained-note.md)"));
    assert!(!memory_dir.join("retained-note.md").exists());
    assert_eq!(
        fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
        "replacement index\n"
    );
}

#[cfg(unix)]
#[test]
fn decay_journal_recovers_after_archive_and_marker_boundaries() {
    for remove_marker_before_recovery in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let store = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        };
        let day = 86_400;
        let now = 200 * day;
        let lesson = decision_core::dreamer::PromotedLesson {
            slug: "crash-decay".to_string(),
            summary: "crash decay".to_string(),
            lesson: "recover interrupted decay".to_string(),
            kind: LessonKind::Workflow,
            distinct_sessions: 2,
            verified: true,
            confidence: 0.9,
            expiry_days: 90,
        };
        let body = super::render_entry_body(&lesson, now - 100 * day, None);
        store
            .write_entry(&super::MemoryWriteRequest {
                slug: lesson.slug.clone(),
                summary: lesson.summary.clone(),
                body: body.clone(),
            })
            .unwrap();

        let lock = super::acquire_memory_store_lock(&memory_dir).unwrap();
        let journal = super::MemoryDecayJournal {
            state: super::MemoryDecayJournalState::Prepared,
            slug: lesson.slug.clone(),
            entry_body_hash: super::dreamer_owned_body_hash(&format!("{body}\n")),
        };
        super::write_memory_decay_journal_retained(&lock.dir, &journal).unwrap();
        let archive = lock
            .dir
            .ensure_private_subdir(Path::new(super::DECAY_ARCHIVE_DIR))
            .unwrap();
        let entry_relative = Path::new("crash-decay.md");
        let entry = lock.dir.open_regular_file(entry_relative).unwrap();
        assert!(lock
            .dir
            .rename_file_no_replace(entry_relative, &entry, &archive, entry_relative)
            .unwrap());
        if remove_marker_before_recovery {
            lock.dir
                .remove_regular_file(&super::dreamer_owned_marker_relative_path(
                    "crash-decay",
                ))
                .unwrap();
        }
        drop(lock);

        store.decay_expired(now).unwrap();
        assert!(!memory_dir.join("crash-decay.md").exists());
        assert!(memory_dir.join("archive/crash-decay.md").exists());
        assert!(!memory_dir
            .join(super::dreamer_owned_marker_relative_path("crash-decay"))
            .exists());
        assert!(!fs::read_to_string(memory_dir.join("MEMORY.md"))
            .unwrap()
            .contains("](crash-decay.md)"));
        assert!(!memory_dir
            .join(super::MEMORY_DECAY_JOURNAL_FILE)
            .exists());

        store.decay_expired(now).unwrap();
        assert!(memory_dir.join("archive/crash-decay.md").exists());
        assert!(!fs::read_to_string(memory_dir.join("MEMORY.md"))
            .unwrap()
            .contains("](crash-decay.md)"));
    }
}

#[cfg(unix)]
#[test]
fn corrupt_decay_journal_fails_closed_and_retry_succeeds_after_discard() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let store = FsMemoryStore {
        memory_dir: memory_dir.clone(),
    };
    let day = 86_400;
    let now = 200 * day;
    let lesson = decision_core::dreamer::PromotedLesson {
        slug: "corrupt-journal-decay".to_string(),
        summary: "corrupt journal decay".to_string(),
        lesson: "retain data when the decay journal is truncated".to_string(),
        kind: LessonKind::Workflow,
        distinct_sessions: 2,
        verified: true,
        confidence: 0.9,
        expiry_days: 90,
    };
    store
        .write_entry(&super::MemoryWriteRequest {
            slug: lesson.slug.clone(),
            summary: lesson.summary.clone(),
            body: super::render_entry_body(&lesson, now - 100 * day, None),
        })
        .unwrap();
    let entry_path = memory_dir.join("corrupt-journal-decay.md");
    let marker_path = memory_dir.join(super::dreamer_owned_marker_relative_path(
        "corrupt-journal-decay",
    ));
    let index_path = memory_dir.join("MEMORY.md");
    let journal_path = memory_dir.join(super::MEMORY_DECAY_JOURNAL_FILE);
    let entry_before = fs::read(&entry_path).unwrap();
    let marker_before = fs::read(&marker_path).unwrap();
    let index_before = fs::read(&index_path).unwrap();
    let truncated = br#"{"state":"prepared","slug":"corrupt-journal-decay""#;
    crate::secure_fs::write_atomic_owner_only(
        &memory_dir,
        Path::new(super::MEMORY_DECAY_JOURNAL_FILE),
        truncated,
    )
    .unwrap();

    let error = store
        .decay_expired(now)
        .expect_err("a corrupt decay journal must stop before mutating memory");
    assert!(error.to_string().contains("memory decay journal is invalid"));
    assert_eq!(fs::read(&entry_path).unwrap(), entry_before);
    assert_eq!(fs::read(&marker_path).unwrap(), marker_before);
    assert_eq!(fs::read(&index_path).unwrap(), index_before);
    assert_eq!(fs::read(&journal_path).unwrap(), truncated);
    assert!(!memory_dir.join("archive/corrupt-journal-decay.md").exists());

    fs::remove_file(&journal_path).unwrap();
    assert_eq!(
        store.decay_expired(now).unwrap(),
        vec!["corrupt-journal-decay"]
    );
    assert!(memory_dir.join("archive/corrupt-journal-decay.md").exists());
}

#[test]
fn memory_store_file_lock_retries_before_timing_out() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir(&memory_dir).unwrap();
    let lock_path = Path::new(super::MEMORY_STORE_LOCK_FILE);

    let mut held = Some(
        crate::secure_fs::try_lock_owner_only(&memory_dir, lock_path)
            .unwrap()
            .unwrap(),
    );
    let acquired = super::acquire_memory_store_lock_with_retry(
        &memory_dir,
        1,
        std::time::Duration::ZERO,
        || {},
        |attempt| {
            assert_eq!(attempt, 0);
            drop(held.take());
        },
    )
    .expect("a later advisory-lock attempt must acquire after release");
    assert!(held.is_none());
    drop(acquired);

    let _held = crate::secure_fs::try_lock_owner_only(&memory_dir, lock_path)
        .unwrap()
        .unwrap();
    let mut attempts = Vec::new();
    let Err(error) = super::acquire_memory_store_lock_with_retry(
        &memory_dir,
        2,
        std::time::Duration::ZERO,
        || {},
        |attempt| attempts.push(attempt),
    ) else {
        panic!("a held advisory lock must time out");
    };
    assert_eq!(attempts, vec![0, 1, 2]);
    let DreamError::Io(error) = error;
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("timed out after 3 attempts"));
}

#[test]
fn decay_and_hand_written_writer_serialize_index_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let store = FsMemoryStore {
        memory_dir: memory_dir.clone(),
    };
    let day = 86_400;
    let now = 200 * day;
    let stale_lesson = decision_core::dreamer::PromotedLesson {
        slug: "stale-lesson".to_string(),
        summary: "stale summary".to_string(),
        lesson: "stale lesson".to_string(),
        kind: LessonKind::Workflow,
        distinct_sessions: 2,
        verified: true,
        confidence: 0.9,
        expiry_days: 90,
    };
    store
        .write_entry(&super::MemoryWriteRequest {
            slug: stale_lesson.slug.clone(),
            summary: stale_lesson.summary.clone(),
            body: super::render_entry_body(&stale_lesson, now - 100 * day, None),
        })
        .unwrap();
    let manual_entry = super::MemoryWriteRequest {
        slug: "manual-note".to_string(),
        summary: "manual summary".to_string(),
        body: "# Manual note".to_string(),
    };

    let (writer_paused_tx, writer_paused_rx) = std::sync::mpsc::channel();
    let (resume_writer_tx, resume_writer_rx) = std::sync::mpsc::channel();
    let (decay_contended_tx, decay_contended_rx) = std::sync::mpsc::channel();
    let archived = std::thread::scope(|scope| {
        let writer_memory_dir = &memory_dir;
        let writer_entry = &manual_entry;
        let writer = scope.spawn(move || {
            super::write_memory_entry_transaction_with_before_prepare(
                writer_memory_dir,
                writer_entry,
                super::MemoryWriteOwner::HandWritten,
                || {
                    writer_paused_tx.send(()).unwrap();
                    resume_writer_rx.recv().unwrap();
                },
            )
        });
        writer_paused_rx.recv().unwrap();

        let decay_store = &store;
        let decay = scope.spawn(move || {
            decay_store.decay_expired_with_lock_hooks(
                now,
                || decay_contended_tx.send(()).unwrap(),
                || {},
            )
        });
        decay_contended_rx.recv().unwrap();
        resume_writer_tx.send(()).unwrap();

        writer.join().unwrap().unwrap();
        decay.join().unwrap().unwrap()
    });

    assert_eq!(archived, vec!["stale-lesson"]);
    let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    assert!(!index.contains("](stale-lesson.md)"));
    assert!(index.contains("](manual-note.md)"));
    assert!(memory_dir.join("manual-note.md").exists());
    assert!(memory_dir.join("archive/stale-lesson.md").exists());
}

#[test]
fn decay_recovers_prepared_journal_before_archiving() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let store = FsMemoryStore {
        memory_dir: memory_dir.clone(),
    };
    let day = 86_400;
    let now = 200 * day;
    let stale_lesson = decision_core::dreamer::PromotedLesson {
        slug: "stale-lesson".to_string(),
        summary: "stale summary".to_string(),
        lesson: "stale lesson".to_string(),
        kind: LessonKind::Workflow,
        distinct_sessions: 2,
        verified: true,
        confidence: 0.9,
        expiry_days: 90,
    };
    store
        .write_entry(&super::MemoryWriteRequest {
            slug: stale_lesson.slug.clone(),
            summary: stale_lesson.summary.clone(),
            body: super::render_entry_body(&stale_lesson, now - 100 * day, None),
        })
        .unwrap();

    let index_before = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    super::write_memory_journal(
        &memory_dir,
        &super::MemoryWriteJournal {
            state: super::MemoryWriteJournalState::Prepared,
            slug: "partial-note".to_string(),
            entry_before: None,
            marker_before: None,
            index_before: Some(index_before.clone()),
        },
    )
    .unwrap();
    fs::write(memory_dir.join("partial-note.md"), "partial\n").unwrap();
    fs::write(
        memory_dir.join("MEMORY.md"),
        format!("{index_before}- [partial-note](partial-note.md) — partial\n"),
    )
    .unwrap();

    let archived = store.decay_expired(now).unwrap();

    assert_eq!(archived, vec!["stale-lesson"]);
    assert!(!memory_dir.join("partial-note.md").exists());
    assert!(memory_dir.join("archive/stale-lesson.md").exists());
    let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    assert!(!index.contains("](partial-note.md)"));
    assert!(!index.contains("](stale-lesson.md)"));
    assert!(!memory_dir
        .join(super::MEMORY_WRITE_JOURNAL_FILE)
        .exists());
}

#[test]
fn prepared_memory_journal_recovers_entry_marker_and_index_before_next_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let marker_dir = memory_dir.join(super::DREAMER_OWNED_DIR);
    fs::create_dir(&memory_dir).unwrap();
    fs::create_dir(&marker_dir).unwrap();
    fs::write(memory_dir.join("note.md"), "before\n").unwrap();
    fs::write(marker_dir.join("note.marker"), "before marker\n").unwrap();
    fs::write(memory_dir.join("MEMORY.md"), "before index\n").unwrap();
    let journal = super::MemoryWriteJournal {
        state: super::MemoryWriteJournalState::Prepared,
        slug: "note".to_string(),
        entry_before: Some("before\n".to_string()),
        marker_before: Some("before marker\n".to_string()),
        index_before: Some("before index\n".to_string()),
    };
    super::write_memory_journal(&memory_dir, &journal).unwrap();
    fs::write(memory_dir.join("note.md"), "partial\n").unwrap();
    fs::write(marker_dir.join("note.marker"), "partial marker\n").unwrap();
    fs::write(memory_dir.join("MEMORY.md"), "partial index\n").unwrap();

    super::recover_memory_write_journal(&memory_dir).unwrap();

    assert_eq!(
        fs::read_to_string(memory_dir.join("note.md")).unwrap(),
        "before\n"
    );
    assert_eq!(
        fs::read_to_string(marker_dir.join("note.marker")).unwrap(),
        "before marker\n"
    );
    assert_eq!(
        fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
        "before index\n"
    );
    assert!(!memory_dir
        .join(super::MEMORY_WRITE_JOURNAL_FILE)
        .exists());
}

#[test]
fn committed_memory_journal_keeps_entry_marker_and_index_before_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    let marker_dir = memory_dir.join(super::DREAMER_OWNED_DIR);
    fs::create_dir(&memory_dir).unwrap();
    fs::create_dir(&marker_dir).unwrap();
    fs::write(memory_dir.join("note.md"), "committed\n").unwrap();
    fs::write(marker_dir.join("note.marker"), "committed marker\n").unwrap();
    fs::write(memory_dir.join("MEMORY.md"), "committed index\n").unwrap();
    let journal = super::MemoryWriteJournal {
        state: super::MemoryWriteJournalState::Committed,
        slug: "note".to_string(),
        entry_before: Some("before\n".to_string()),
        marker_before: Some("before marker\n".to_string()),
        index_before: Some("before index\n".to_string()),
    };
    super::write_memory_journal(&memory_dir, &journal).unwrap();

    super::recover_memory_write_journal(&memory_dir).unwrap();

    assert_eq!(
        fs::read_to_string(memory_dir.join("note.md")).unwrap(),
        "committed\n"
    );
    assert_eq!(
        fs::read_to_string(marker_dir.join("note.marker")).unwrap(),
        "committed marker\n"
    );
    assert_eq!(
        fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
        "committed index\n"
    );
    assert!(!memory_dir
        .join(super::MEMORY_WRITE_JOURNAL_FILE)
        .exists());
}

#[test]
fn legacy_failed_goal_terminal_migrates_without_changing_terminal_status() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut legacy = SelfImproveCandidate::new(
        CandidateKind::GoalTerminal,
        "legacy goal failure",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "goal".to_string(),
            detail: "goal failed after verification".to_string(),
            verified: true,
        }],
    );
    legacy.status = CandidateStatus::Applied;
    super::record_self_improve_candidate(cwd, &legacy).unwrap();

    let migrated = super::read_self_improve_candidates(cwd);
    assert_eq!(migrated.len(), 1);
    assert_eq!(migrated[0].kind, CandidateKind::GoalFailure);
    assert_eq!(migrated[0].status, CandidateStatus::Applied);
}

#[test]
fn dreamer_promotion_refuses_an_unindexed_hand_written_slug() {
    let env_lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config-home");
    fs::create_dir_all(&config_home).unwrap();
    let cwd = tmp.path().join("workspace");
    fs::create_dir_all(&cwd).unwrap();

    with_config_home(&env_lock, &config_home, || {
        let memory_dir = crate::memory::paths::memory_write_dir(&cwd, false);
        fs::create_dir_all(&memory_dir).unwrap();
        let path = memory_dir.join("existing-note.md");
        fs::write(&path, "# Hand-written entry\n").unwrap();

        let error = FsMemoryStore::at_cwd(&cwd)
            .write_entry(&super::MemoryWriteRequest {
                slug: "existing-note".to_string(),
                summary: "dreamer summary".to_string(),
                body: "# Dreamer replacement".to_string(),
            })
            .expect_err("Dreamer must not overwrite an unindexed entry");
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(path).unwrap(), "# Hand-written entry\n");
    });
}

#[test]
fn self_improve_candidate_store_appends_and_coalesces_evidence() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "streaming turn failed",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "turn".to_string(),
            detail: "timeout".to_string(),
            verified: false,
        }],
    );
    let mut planned = candidate.clone();
    planned.status = CandidateStatus::Planned;
    planned.evidence = vec![CandidateEvidence {
        session_id: "s2".to_string(),
        source: "turn".to_string(),
        detail: "provider error".to_string(),
        verified: false,
    }];

    super::record_self_improve_candidate(cwd, &candidate).unwrap();
    super::record_self_improve_candidate(cwd, &planned).unwrap();
    fs::write(
        cwd.join(".zo")
            .join("dream")
            .join("candidates")
            .join("bad.jsonl"),
        "{not json\n",
    )
    .unwrap();

    let mut proposed_again = candidate.clone();
    proposed_again.evidence = vec![CandidateEvidence {
        session_id: "s3".to_string(),
        source: "turn".to_string(),
        detail: "later retry".to_string(),
        verified: false,
    }];
    super::record_self_improve_candidate(cwd, &proposed_again).unwrap();

    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, candidate.id);
    assert_eq!(candidates[0].status, CandidateStatus::Planned);
    assert_eq!(candidates[0].evidence.len(), 3);
}

#[test]
fn self_improve_candidate_store_never_downgrades_terminal_status() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut applied = SelfImproveCandidate::new(
        CandidateKind::GoalTerminal,
        "goal reached terminal state",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "goal".to_string(),
            detail: "applied".to_string(),
            verified: true,
        }],
    );
    applied.status = CandidateStatus::Applied;
    let proposed_again = SelfImproveCandidate::new(
        CandidateKind::GoalTerminal,
        "goal reached terminal state",
        vec![CandidateEvidence {
            session_id: "s2".to_string(),
            source: "goal".to_string(),
            detail: "proposed again".to_string(),
            verified: true,
        }],
    );

    super::record_self_improve_candidate(cwd, &applied).unwrap();
    super::record_self_improve_candidate(cwd, &proposed_again).unwrap();

    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].status, CandidateStatus::Applied);
    assert_eq!(candidates[0].evidence.len(), 2);
}

#[test]
fn candidate_retention_keeps_terminal_state_and_old_verified_evidence() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut applied = SelfImproveCandidate::new(
        CandidateKind::GoalTerminal,
        "retained terminal candidate",
        vec![CandidateEvidence {
            session_id: "original-session".to_string(),
            source: "goal".to_string(),
            detail: "verified terminal evidence".to_string(),
            verified: true,
        }],
    );
    applied.status = CandidateStatus::Applied;
    super::record_self_improve_candidate(cwd, &applied).unwrap();

    for index in 0..(super::MAX_SELF_IMPROVE_CANDIDATE_LINES + 3) {
        let observed = SelfImproveCandidate::new(
            CandidateKind::GoalTerminal,
            "retained terminal candidate",
            vec![CandidateEvidence {
                session_id: format!("later-session-{index}"),
                source: "goal".to_string(),
                detail: format!("later observation {index}"),
                verified: false,
            }],
        );
        super::record_self_improve_candidate(cwd, &observed).unwrap();
    }

    let candidate_dir = cwd.join(".zo").join("dream").join("candidates");
    let path = candidate_dir.join(format!("{}.jsonl", super::safe_stem(&applied.id)));
    let retained_lines = fs::read_to_string(path).unwrap().lines().count();
    assert_eq!(retained_lines, super::MAX_SELF_IMPROVE_CANDIDATE_LINES);

    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].status, CandidateStatus::Applied);
    assert!(candidates[0].evidence.iter().any(|evidence| {
        evidence.session_id == "original-session"
            && evidence.detail == "verified terminal evidence"
    }));
}

#[test]
fn concurrent_candidate_writers_preserve_all_distinct_evidence() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let writers: Vec<_> = (0..super::MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE)
        .map(|index| {
            let cwd = cwd.clone();
            std::thread::spawn(move || {
                let candidate = SelfImproveCandidate::new(
                    CandidateKind::TurnFailure,
                    "concurrent candidate",
                    vec![CandidateEvidence {
                        session_id: format!("session-{index}"),
                        source: "turn".to_string(),
                        detail: format!("evidence-{index}"),
                        verified: index % 2 == 0,
                    }],
                );
                super::record_self_improve_candidate(&cwd, &candidate).unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }

    let candidates = super::read_self_improve_candidates(&cwd);
    assert_eq!(candidates.len(), 1);
    for index in 0..super::MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE {
        assert!(candidates[0]
            .evidence
            .iter()
            .any(|evidence| evidence.session_id == format!("session-{index}")));
    }
}

#[cfg(unix)]
#[test]
fn candidate_store_files_and_directories_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "private candidate store",
        Vec::new(),
    );

    super::record_self_improve_candidate(cwd, &candidate).unwrap();

    let zo = cwd.join(".zo");
    let dream = zo.join("dream");
    let candidates = dream.join("candidates");
    let candidate_file = candidates.join(format!("{}.jsonl", super::safe_stem(&candidate.id)));
    for dir in [&zo, &dream, &candidates] {
        assert_eq!(fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
    }
    for file in [candidate_file, candidates.join(".candidate-store.lock")] {
        assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[cfg(unix)]
#[test]
fn candidate_store_rejects_hardlink_without_side_effects() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "hardlinked candidate",
        Vec::new(),
    );
    let candidate_dir = super::ensure_repo_dream_child_dir_no_symlink(
        cwd,
        super::SELF_IMPROVE_CANDIDATES_DIR,
    )
    .unwrap();
    let victim = cwd.join("victim.jsonl");
    fs::write(&victim, "sentinel\n").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
    fs::hard_link(
        &victim,
        candidate_dir.join(format!("{}.jsonl", super::safe_stem(&candidate.id))),
    )
    .unwrap();

    assert!(super::record_self_improve_candidate(cwd, &candidate).is_err());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "sentinel\n");
    assert_eq!(fs::metadata(&victim).unwrap().mode() & 0o777, 0o644);
    assert_eq!(fs::metadata(&victim).unwrap().nlink(), 2);
}

#[cfg(unix)]
#[test]
fn candidate_transaction_stays_on_locked_directory_after_parent_replacement() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "retained candidate store",
        Vec::new(),
    );
    super::record_self_improve_candidate(cwd, &candidate).unwrap();
    let candidate_dir = cwd.join(".zo/dream/candidates");
    let retained_dir = cwd.join(".zo/dream/original-candidates");
    let store = super::lock_candidate_store(cwd, &candidate_dir).unwrap();

    fs::rename(&candidate_dir, &retained_dir).unwrap();
    fs::create_dir(&candidate_dir).unwrap();
    let candidate_name = format!("{}.jsonl", super::safe_stem(&candidate.id));
    fs::write(candidate_dir.join(&candidate_name), "replacement\n").unwrap();

    super::record_self_improve_candidate_retained(&store, &candidate).unwrap();
    drop(store);

    assert_eq!(
        fs::read_to_string(candidate_dir.join(&candidate_name)).unwrap(),
        "replacement\n"
    );
    assert_eq!(
        fs::read_to_string(retained_dir.join(&candidate_name))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn candidate_store_process_lock_serializes_writers() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "serialized candidate lock",
        Vec::new(),
    );
    super::record_self_improve_candidate(&cwd, &candidate).unwrap();
    let candidate_dir = cwd.join(".zo").join("dream").join("candidates");
    let held_lock = super::lock_candidate_store(&cwd, &candidate_dir).unwrap();
    let blocked_cwd = cwd.clone();
    let blocked_candidate = candidate.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let result = super::record_self_improve_candidate(&blocked_cwd, &blocked_candidate);
        let _ = result_tx.send(result);
    });

    assert!(
        result_rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err(),
        "same-process writer must wait for the candidate transaction"
    );
    drop(held_lock);
    result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("writer must resume after the candidate transaction")
        .expect("serialized candidate write must succeed");
    writer.join().unwrap();
}

#[test]
fn candidate_store_file_lock_contention_is_bounded() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "bounded candidate file lock",
        Vec::new(),
    );
    super::record_self_improve_candidate(&cwd, &candidate).unwrap();
    let candidate_dir = cwd.join(".zo").join("dream").join("candidates");
    let _held_lock = crate::secure_fs::try_lock_owner_only(
        &candidate_dir,
        Path::new(".candidate-store.lock"),
    )
    .unwrap()
    .unwrap();

    let error = super::record_self_improve_candidate(&cwd, &candidate)
        .expect_err("advisory file-lock contention must remain bounded");
    assert!(
        error.to_string().contains("lock acquisition timed out"),
        "{error}"
    );
}

#[test]
fn rejected_candidate_stays_terminal_when_observed_again() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut rejected = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "rejected failure",
        Vec::new(),
    );
    rejected.status = CandidateStatus::Rejected;
    let observed_again = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "rejected failure",
        vec![CandidateEvidence {
            session_id: "s2".to_string(),
            source: "turn".to_string(),
            detail: "same issue".to_string(),
            verified: false,
        }],
    );

    super::record_self_improve_candidate(cwd, &rejected).unwrap();
    super::record_self_improve_candidate(cwd, &observed_again).unwrap();

    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].status, CandidateStatus::Rejected);
}

#[test]
fn evidence_cap_preserves_independent_sessions() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut evidence = vec![CandidateEvidence {
        session_id: "independent".to_string(),
        source: "deep_gate".to_string(),
        detail: "independent evidence".to_string(),
        verified: true,
    }];
    evidence.extend((0..80).map(|index| CandidateEvidence {
        session_id: "noisy".to_string(),
        source: "turn".to_string(),
        detail: format!("repeated-{index}"),
        verified: false,
    }));
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "noisy failure",
        evidence,
    );

    super::record_self_improve_candidate(cwd, &candidate).unwrap();
    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].evidence.len(),
        super::MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE
    );
    assert!(candidates[0]
        .evidence
        .iter()
        .any(|item| item.session_id == "independent"));
}

#[test]
fn legacy_cancellation_is_migrated_and_skipped_by_fusion() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let legacy = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "turn cancelled before normal completion",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "turn".to_string(),
            detail: "user abort".to_string(),
            verified: false,
        }],
    );

    super::record_self_improve_candidate(cwd, &legacy).unwrap();
    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, CandidateKind::UserCancelled);
    assert!(!candidates[0].kind.is_actionable());
    assert!(super::run_dream_fusion_v0(cwd, "legacy-cancel").unwrap().is_none());
}

/// A legacy host-side cancellation record (no explicit user-cancel origin)
/// keeps its `TurnFailure` kind but retires from fusion: pre-segmentation it
/// aggregated every host cancel into one blob whose capped session score would
/// permanently outrank the per-signature candidates. Live producers record the
/// specific "host stopped consuming" summary, so no current signal is lost.
#[test]
fn ambiguous_legacy_cancellation_is_retired_from_fusion() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let legacy = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "turn cancelled before normal completion",
        vec![CandidateEvidence {
            session_id: "s-host".to_string(),
            source: "turn".to_string(),
            detail: "render channel closed".to_string(),
            verified: false,
        }],
    );

    super::record_self_improve_candidate(cwd, &legacy).unwrap();
    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, CandidateKind::TurnFailure);
    assert!(candidates[0].status.is_terminal());
    assert!(super::run_dream_fusion_v0(cwd, "legacy-host").unwrap().is_none());
}

/// The pre-segmentation store aggregated every turn failure into one generic
/// candidate. On read it must demote to terminal so its mixed-cause evidence
/// mountain can no longer outrank the per-signature candidates that replaced
/// it — fusion then selects a concrete failure mode instead of the blob.
#[test]
fn legacy_generic_turn_failure_is_demoted_and_segmented_candidate_wins() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let legacy = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "turn failed before normal completion",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "turn".to_string(),
            detail: "provider_transient".to_string(),
            verified: false,
        }],
    );
    let segmented = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "turn failure: provider_non_retryable · 400 INVALID_ARGUMENT",
        vec![CandidateEvidence {
            session_id: "s2".to_string(),
            source: "turn".to_string(),
            detail: "api returned 400 Bad Request (INVALID_ARGUMENT)".to_string(),
            verified: false,
        }],
    );

    super::record_self_improve_candidate(cwd, &legacy).unwrap();
    super::record_self_improve_candidate(cwd, &segmented).unwrap();

    let candidates = super::read_self_improve_candidates(cwd);
    let legacy_read = candidates
        .iter()
        .find(|candidate| candidate.summary == "turn failed before normal completion")
        .expect("legacy record still listed");
    assert_eq!(
        legacy_read.status,
        CandidateStatus::Rejected,
        "generic blob demotes to terminal on read"
    );
    let report = super::run_dream_fusion_v0(cwd, "segmented")
        .unwrap()
        .expect("segmented candidate stays actionable");
    assert_eq!(
        report.summary,
        "turn failure: provider_non_retryable · 400 INVALID_ARGUMENT"
    );
}

/// An `Applied` record under the legacy generic id keeps its terminal outcome:
/// the read-side demotion only rewrites `Proposed` records.
#[test]
fn applied_legacy_generic_turn_failure_keeps_its_outcome() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let mut legacy = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "turn failed before normal completion",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "turn".to_string(),
            detail: "provider_transient".to_string(),
            verified: false,
        }],
    );
    legacy.status = CandidateStatus::Applied;

    super::record_self_improve_candidate(cwd, &legacy).unwrap();

    let candidates = super::read_self_improve_candidates(cwd);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].status, CandidateStatus::Applied);
}

#[test]
fn quarantine_preserves_run_and_cleanup_errors() {
    let run_error = DreamError::Io(std::io::Error::other("patch failed"));
    let cleanup_error = DreamError::Io(std::io::Error::other("cleanup failed"));
    let error = super::finish_quarantine_run(Err(run_error), Err(cleanup_error))
        .expect_err("both failures must be reported");
    let message = error.to_string();
    assert!(message.contains("patch failed"), "{message}");
    assert!(message.contains("cleanup failed"), "{message}");
}

#[test]
fn quarantine_artifact_cleanup_errors_keep_primary_context() {
    for context in [
        "quarantine setup artifact cleanup",
        "quarantine run artifact cleanup",
    ] {
        let primary = DreamError::Io(std::io::Error::other("primary failure"));
        let cleanup = DreamError::Io(std::io::Error::other("artifact cleanup failure"));
        let error = super::finish_with_cleanup::<()>(Err(primary), Err(cleanup), context)
            .expect_err("both errors must be combined");
        let message = error.to_string();
        assert!(message.contains("primary failure"), "{message}");
        assert!(message.contains(context), "{message}");
        assert!(message.contains("artifact cleanup failure"), "{message}");
    }
}

#[cfg(unix)]
#[test]
fn memory_index_and_marker_writes_do_not_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir_all(&memory_dir).unwrap();
    let index_target = tmp.path().join("index-target");
    fs::write(&index_target, "sentinel-index").unwrap();
    symlink(&index_target, memory_dir.join("MEMORY.md")).unwrap();
    assert!(super::upsert_index(&memory_dir.join("MEMORY.md"), "lesson", "summary").is_err());
    assert_eq!(fs::read_to_string(&index_target).unwrap(), "sentinel-index");

    let marker_dir = memory_dir.join(super::DREAMER_OWNED_DIR);
    fs::create_dir_all(&marker_dir).unwrap();
    let marker_target = tmp.path().join("marker-target");
    fs::write(&marker_target, "sentinel-marker").unwrap();
    symlink(
        &marker_target,
        super::dreamer_owned_marker_path(&memory_dir, "lesson"),
    )
    .unwrap();
    assert!(super::write_dreamer_owned_marker(&memory_dir, "lesson", "body").is_err());
    assert_eq!(fs::read_to_string(&marker_target).unwrap(), "sentinel-marker");
}

#[test]
fn changed_paths_from_nul_preserves_unusual_and_copy_paths() {
    let output = b"crates/keep.rs\0secret/creds.txt\0path with spaces.rs\0copy/new.rs\0";
    let paths = super::changed_paths_from_nul(output).unwrap();
    assert_eq!(
        paths,
        vec![
            "crates/keep.rs",
            "secret/creds.txt",
            "path with spaces.rs",
            "copy/new.rs",
        ]
    );
    assert!(super::changed_paths_from_nul(b"unterminated").is_err());
    assert!(super::changed_paths_from_nul(b"../escape\0").is_err());
}

#[test]
fn run_dream_fusion_skips_terminal_candidates() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    // The only candidate is Applied — already acted on, so fusion must not
    // regenerate it (previously it re-proposed the just-applied candidate).
    let mut applied = SelfImproveCandidate::new(
        CandidateKind::GoalTerminal,
        "already applied fix",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "goal".to_string(),
            detail: "applied".to_string(),
            verified: true,
        }],
    );
    applied.status = CandidateStatus::Applied;
    super::record_self_improve_candidate(cwd, &applied).unwrap();
    assert!(
        super::run_dream_fusion_v0(cwd, "run-1").unwrap().is_none(),
        "an Applied-only store must yield no fusion report"
    );
}

#[test]
fn mark_self_improve_candidate_applied_advances_status() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::GoalFailure,
        "fix to apply",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "goal".to_string(),
            detail: "proposed".to_string(),
            verified: true,
        }],
    );
    let id = candidate.id.clone();
    super::record_self_improve_candidate(cwd, &candidate).unwrap();
    super::mark_self_improve_candidate_applied(cwd, &id).unwrap();
    let stored = super::read_self_improve_candidates(cwd);
    let found = stored.iter().find(|c| c.id == id).expect("candidate present");
    assert_eq!(found.status, CandidateStatus::Applied);
}

#[test]
fn self_improve_pulse_respects_enabled_flag() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    assert!(!super::record_self_improve_pulse_if_enabled(
        false,
        cwd,
        CandidateKind::PostTurn,
        "s1",
        "post_turn",
        "successful turn persisted",
        "foreground turn completed",
        true,
    ));
    assert!(super::read_self_improve_candidates(cwd).is_empty());

    fs::create_dir_all(cwd.join(".zo")).unwrap();
    fs::write(
        cwd.join(".zo").join("settings.local.json"),
        r#"{"autoDreamEnabled":false}"#,
    )
    .unwrap();
    assert!(!super::record_self_improve_pulse(
        cwd,
        CandidateKind::PostTurn,
        "s1",
        "post_turn",
        "successful turn persisted",
        "foreground turn completed",
        true,
    ));
    assert!(super::read_self_improve_candidates(cwd).is_empty());

    assert!(super::record_self_improve_pulse_if_enabled(
        true,
        cwd,
        CandidateKind::PostTurn,
        "s1",
        "post_turn",
        "successful turn persisted",
        "foreground turn completed",
        true,
    ));
    assert_eq!(super::read_self_improve_candidates(cwd).len(), 1);
}

#[test]
fn self_improve_scheduler_throttles_attempts_and_failures() {
    use std::time::{Duration, SystemTime};
    let now = SystemTime::now();
    let interval = Duration::from_secs(60);
    let backoff = Duration::from_secs(300);

    assert!(super::should_run_self_improve(
        None, None, now, interval, backoff
    ));
    assert!(!super::should_run_self_improve(
        Some(now - Duration::from_secs(10)),
        None,
        now,
        interval,
        backoff,
    ));
    assert!(!super::should_run_self_improve(
        Some(now - Duration::from_secs(120)),
        Some(now - Duration::from_secs(60)),
        now,
        interval,
        backoff,
    ));
    assert!(super::should_run_self_improve(
        Some(now - Duration::from_secs(120)),
        Some(now - Duration::from_secs(600)),
        now,
        interval,
        backoff,
    ));
}

#[test]
fn self_improve_attempt_and_failure_markers_are_written() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    super::record_self_improve_attempt(cwd).unwrap();
    let attempt = fs::read_to_string(
        cwd.join(".zo")
            .join("dream")
            .join(super::SELF_IMPROVE_ATTEMPT_MARKER),
    )
    .unwrap();
    assert!(attempt.trim().parse::<u64>().is_ok());

    let error = DreamError::Io(std::io::Error::other("simulated self-improve failure"));
    super::record_self_improve_failure(cwd, &error).unwrap();
    let failure = fs::read_to_string(
        cwd.join(".zo")
            .join("dream")
            .join(super::SELF_IMPROVE_ERROR_FILE),
    )
    .unwrap();
    assert!(failure.contains("dreamer_io_error"));
    assert!(!failure.contains("simulated self-improve failure"));
    assert!(failure.contains("tsMs"));

    let state = super::read_self_improve_schedule_state(cwd).expect("read schedule state");
    assert!(state.last_attempt.is_some());
    assert!(state.last_failure.is_some());
}

#[test]
fn self_improve_schedule_state_is_empty_without_markers() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let state = super::read_self_improve_schedule_state(tmp.path()).expect("read schedule state");
    assert_eq!(state.last_attempt, None);
    assert_eq!(state.last_failure, None);
}

#[test]
fn self_improve_schedule_state_readonly_does_not_create_dream_dir() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();

    let state = super::read_self_improve_schedule_state_readonly(tmp.path())
        .expect("readonly state succeeds");

    assert_eq!(state.last_attempt, None);
    assert_eq!(state.last_failure, None);
    assert!(!tmp.path().join(".zo").exists());
}

#[cfg(unix)]
#[test]
fn self_improve_schedule_state_readonly_rejects_symlinked_dream_dir() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".zo")).unwrap();
    let outside = tmp.path().join("outside-dream");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, tmp.path().join(".zo/dream")).unwrap();

    let error = super::read_self_improve_schedule_state_readonly(tmp.path())
        .expect_err("symlinked dream dir rejected");
    assert!(
        error.to_string().contains("not a real directory"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn self_improve_schedule_state_rejects_symlink_marker() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let dream_dir = tmp.path().join(".zo/dream");
    std::fs::create_dir_all(&dream_dir).unwrap();
    let target = tmp.path().join("outside-marker.json");
    std::fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, dream_dir.join(".last_self_improve_attempt")).unwrap();

    let error = super::read_self_improve_schedule_state(tmp.path()).expect_err("symlink marker rejected");
    assert!(
        error.to_string().contains("not a regular file"),
        "unexpected error: {error}"
    );

    let error = super::record_self_improve_attempt(tmp.path()).expect_err("symlink marker write rejected");
    assert!(
        error.to_string().contains("not a regular file"),
        "unexpected error: {error}"
    );
}

#[test]
fn self_improve_lock_is_exclusive_and_released_on_drop() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();

    let held = super::try_acquire_self_improve_lock(cwd).unwrap();
    assert!(held.is_some());
    assert!(super::try_acquire_self_improve_lock(cwd).unwrap().is_none());
    drop(held);
    assert!(super::try_acquire_self_improve_lock(cwd).unwrap().is_some());
}

fn git_ok(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn registered_worktrees(cwd: &Path) -> Vec<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git worktree list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(std::path::PathBuf::from)
        .collect()
}

fn quarantine_run_entries(cwd: &Path) -> Vec<String> {
    let root = super::quarantine_dir(cwd);
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(std::fs::FileType::is_dir)
                .and_then(|_| entry.file_name().into_string().ok())
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn init_patch_repo(cwd: &Path) {
    git_ok(cwd, &["init"]);
    git_ok(cwd, &["config", "user.email", "dreamer@example.invalid"]);
    git_ok(cwd, &["config", "user.name", "Dreamer Test"]);
    fs::write(cwd.join(".gitignore"), ".zo/dream/quarantine/\n").unwrap();
    fs::write(cwd.join("file.txt"), "old\n").unwrap();
    git_ok(cwd, &["add", ".gitignore", "file.txt"]);
    git_ok(cwd, &["commit", "-m", "base"]);
}

#[test]
fn dream_fusion_v0_writes_judge_report_without_patch_artifacts() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let candidate = SelfImproveCandidate::new(
        CandidateKind::TurnFailure,
        "streaming turn failed",
        vec![CandidateEvidence {
            session_id: "s1".to_string(),
            source: "turn".to_string(),
            detail: "provider timeout".to_string(),
            verified: false,
        }],
    );
    super::record_self_improve_candidate(cwd, &candidate).unwrap();

    let report = run_dream_fusion_v0(cwd, "fusion-run")
        .unwrap()
        .expect("candidate should produce report");

    assert_eq!(report.candidate_id, candidate.id);
    assert_eq!(report.decision, DreamJudgeDecision::PlanPatch);
    assert_eq!(report.findings.len(), 4);
    assert!(report
        .findings
        .iter()
        .any(|f| f.role == AdvisorRole::RootCause));
    assert!(cwd
        .join(".zo")
        .join("dream")
        .join("fusion")
        .join("fusion-run.json")
        .exists());
    assert!(
        !cwd.join(".zo").join("dream").join("quarantine").exists(),
        "DreamFusion v0 must not create patch/quarantine artifacts"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_patch_runner_isolates_artifacts_and_manual_apply_gate() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let base = git_stdout(cwd, &["rev-parse", "HEAD"]);
    let worktrees_before = registered_worktrees(cwd);
    let patch = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let request = QuarantinePatchRequest {
        run_id: "run-1".to_string(),
        candidate_id: "candidate-1".to_string(),
        patch_diff: patch.to_string(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: true,
        checks: vec![QuarantineCheckCommand {
            name: "git diff lists patched file".to_string(),
            program: "git".to_string(),
            args: vec![
                "diff".to_string(),
                "--name-only".to_string(),
                "--".to_string(),
                "file.txt".to_string(),
            ],
        }],
        risk: PatchRisk::Low,
    };

    let run = run_quarantine_patch(cwd, &request).unwrap();

    assert_eq!(registered_worktrees(cwd), worktrees_before);
    assert_eq!(run.base_commit, base);
    assert_eq!(run.changed_paths, vec!["file.txt"]);
    assert_eq!(run.check_results.len(), 1);
    assert_eq!(fs::read_to_string(cwd.join("file.txt")).unwrap(), "old\n");

    let check_result = &run.check_results[0];
    if check_result.stderr == "strict_filesystem_network_isolation_unavailable" {
        assert!(!check_result.success);
        assert_eq!(check_result.exit_code, None);
        let rejected = evaluate_manual_apply_gate(
            cwd,
            &ManualApplyGateRequest {
                approved_by_user: true,
                run: run.clone(),
                allowed_paths: vec!["file.txt".to_string()],
                reviewer_accepted: true,
            },
        );
        assert!(!rejected.eligible);
        assert!(
            rejected
                .reasons
                .contains(&"focused_checks_not_green".to_string())
        );
        return;
    }
    assert!(
        check_result.success,
        "unexpected check result: {check_result:?}"
    );
    let run_dir = super::quarantine_run_dir(cwd, &run.run_id);
    assert!(fs::read_to_string(run_dir.join("patch.diff"))
        .unwrap()
        .contains("+new"));
    assert!(fs::read_to_string(run_dir.join("checks.json"))
        .unwrap()
        .contains("git diff lists patched file"));
    assert!(fs::read_to_string(run_dir.join("metadata.json"))
        .unwrap()
        .contains("candidate-1"));

    let accepted = evaluate_manual_apply_gate(
        cwd,
        &ManualApplyGateRequest {
            approved_by_user: true,
            run: run.clone(),
            allowed_paths: vec!["file.txt".to_string()],
            reviewer_accepted: true,
        },
    );
    assert!(
        accepted.eligible,
        "unexpected gate rejection: {:?}",
        accepted.reasons
    );

    fs::write(cwd.join("file.txt"), "dirty\n").unwrap();
    let rejected = evaluate_manual_apply_gate(
        cwd,
        &ManualApplyGateRequest {
            approved_by_user: true,
            run,
            allowed_paths: vec!["file.txt".to_string()],
            reviewer_accepted: true,
        },
    );
    assert!(!rejected.eligible);
    assert!(rejected.reasons.contains(&"worktree_not_clean".to_string()));
}

#[test]
fn quarantine_never_executes_checks_without_explicit_authorization() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let sentinel = cwd.join("unauthorized-check-ran");
    let request = QuarantinePatchRequest {
        run_id: "unauthorized-run".to_string(),
        candidate_id: "candidate-unauthorized".to_string(),
        patch_diff: "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: vec![QuarantineCheckCommand {
            name: "must not execute".to_string(),
            program: "touch".to_string(),
            args: vec![sentinel.to_string_lossy().into_owned()],
        }],
        risk: PatchRisk::Low,
    };

    let error = run_quarantine_patch(cwd, &request).expect_err("authorization gate must reject");
    assert!(error.to_string().contains("execution authorization"));
    assert!(!sentinel.exists(), "unauthorized checks must never run");
}

#[test]
fn quarantine_rejects_disallowed_paths_before_running_checks() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let sentinel = cwd.join("check-ran");
    let request = QuarantinePatchRequest {
        run_id: "blocked-run".to_string(),
        candidate_id: "candidate-blocked".to_string(),
        patch_diff: "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
        allowed_paths: vec!["crates".to_string()],
        checks_authorized: true,
        checks: vec![QuarantineCheckCommand {
            name: "must not execute".to_string(),
            program: "touch".to_string(),
            args: vec![sentinel.to_string_lossy().into_owned()],
        }],
        risk: PatchRisk::Low,
    };

    let error = run_quarantine_patch(cwd, &request).expect_err("path gate must reject");
    assert!(error.to_string().contains("disallowed or symlinked"));
    assert!(!sentinel.exists(), "checks must not run before path validation");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_patch_runner_cleans_worktree_after_apply_failure() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let worktrees_before = registered_worktrees(cwd);
    let runs_before = quarantine_run_entries(cwd);
    let request = QuarantinePatchRequest {
        run_id: "bad-run".to_string(),
        candidate_id: "candidate-1".to_string(),
        patch_diff: "not a patch\n".to_string(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: Vec::new(),
        risk: PatchRisk::Low,
    };

    let error = run_quarantine_patch(cwd, &request).expect_err("invalid patch must fail");
    assert!(error.to_string().contains("git apply failed"));
    assert_eq!(registered_worktrees(cwd), worktrees_before);
    assert_eq!(quarantine_run_entries(cwd), runs_before);
    assert_eq!(fs::read_to_string(cwd.join("file.txt")).unwrap(), "old\n");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_run_prunes_stale_worktree_registrations_at_start() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let stale_parent = tempfile::tempdir().unwrap();
    let stale_worktree = stale_parent.path().join("worktree");
    git_ok(
        cwd,
        &[
            "worktree",
            "add",
            "--detach",
            stale_worktree.to_string_lossy().as_ref(),
            "HEAD",
        ],
    );
    let stale_worktree = fs::canonicalize(stale_worktree).unwrap();
    assert!(registered_worktrees(cwd).contains(&stale_worktree));
    stale_parent.close().unwrap();

    let request = QuarantinePatchRequest {
        run_id: "prune-stale-worktree".to_string(),
        candidate_id: "candidate-prune".to_string(),
        patch_diff: "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: Vec::new(),
        risk: PatchRisk::Low,
    };

    run_quarantine_patch(cwd, &request).unwrap();

    assert!(!registered_worktrees(cwd).contains(&stale_worktree));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_missing_head_leaves_no_run_artifacts_or_worktree_registration() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    git_ok(cwd, &["init"]);
    let worktrees_before = registered_worktrees(cwd);
    let runs_before = quarantine_run_entries(cwd);
    let request = QuarantinePatchRequest {
        run_id: "headless-run".to_string(),
        candidate_id: "candidate-1".to_string(),
        patch_diff: String::new(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: Vec::new(),
        risk: PatchRisk::Low,
    };

    let error = run_quarantine_patch(cwd, &request).expect_err("missing HEAD must fail");
    assert!(error.to_string().contains("git rev-parse HEAD failed"));
    assert_eq!(registered_worktrees(cwd), worktrees_before);
    assert_eq!(quarantine_run_entries(cwd), runs_before);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_worktree_setup_failure_cleans_run_artifacts_and_registration() {
    use std::os::unix::fs::PermissionsExt as _;

    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let worktrees_before = registered_worktrees(cwd);
    let runs_before = quarantine_run_entries(cwd);
    let git_dir = cwd.join(".git");
    let original_permissions = fs::metadata(&git_dir).unwrap().permissions();
    let mut blocked_permissions = original_permissions.clone();
    blocked_permissions.set_mode(0o500);
    fs::set_permissions(&git_dir, blocked_permissions).unwrap();
    let request = QuarantinePatchRequest {
        run_id: "setup-failure".to_string(),
        candidate_id: "candidate-1".to_string(),
        patch_diff: String::new(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: Vec::new(),
        risk: PatchRisk::Low,
    };

    let result = run_quarantine_patch(cwd, &request);
    fs::set_permissions(&git_dir, original_permissions).unwrap();
    let error = result.expect_err("unwritable git metadata must fail worktree setup");
    assert!(error.to_string().contains("git worktree add failed"));
    assert_eq!(registered_worktrees(cwd), worktrees_before);
    assert_eq!(quarantine_run_entries(cwd), runs_before);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn quarantine_collision_resistant_ids_preserve_prior_audit_run() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let worktrees_before = registered_worktrees(cwd);
    let patch = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let first_request = QuarantinePatchRequest {
        run_id: "Case Run".to_string(),
        candidate_id: "candidate-1".to_string(),
        patch_diff: patch.to_string(),
        allowed_paths: vec!["file.txt".to_string()],
        checks_authorized: false,
        checks: Vec::new(),
        risk: PatchRisk::Low,
    };
    let first = run_quarantine_patch(cwd, &first_request).unwrap();
    let first_dir = super::quarantine_run_dir(cwd, &first.run_id);
    let first_metadata = fs::read_to_string(first_dir.join("metadata.json")).unwrap();

    let mut case_collision = first_request.clone();
    case_collision.run_id = "case-run".to_string();
    let second = run_quarantine_patch(cwd, &case_collision).unwrap();
    assert_ne!(first.run_id, second.run_id);
    assert_eq!(fs::read_to_string(first_dir.join("metadata.json")).unwrap(), first_metadata);

    let collision = run_quarantine_patch(cwd, &first_request)
        .expect_err("identical run identifiers must not reuse an audit directory");
    let DreamError::Io(collision) = collision;
    assert_eq!(collision.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(first_dir.join("metadata.json")).unwrap(), first_metadata);
    assert_eq!(registered_worktrees(cwd), worktrees_before);
}

#[cfg(unix)]
#[test]
fn manual_apply_gate_rejects_symlink_changed_paths() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    std::os::unix::fs::symlink("file.txt", cwd.join("link.txt")).unwrap();
    git_ok(cwd, &["add", "link.txt"]);
    git_ok(cwd, &["commit", "-m", "add symlink"]);
    let run = QuarantinePatchRun {
        run_id: "run-symlink".to_string(),
        candidate_id: "candidate-symlink".to_string(),
        base_commit: git_stdout(cwd, &["rev-parse", "HEAD"]),
        patch_digest: "0".repeat(64),
        changed_paths: vec!["link.txt".to_string()],
        check_results: vec![decision_core::dreamer::PatchCheckResult {
            name: "focused".to_string(),
            command: vec!["git".to_string(), "status".to_string()],
            exit_code: Some(0),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }],
        risk: PatchRisk::Low,
    };

    let decision = evaluate_manual_apply_gate(
        cwd,
        &ManualApplyGateRequest {
            approved_by_user: true,
            run,
            allowed_paths: vec!["link.txt".to_string()],
            reviewer_accepted: true,
        },
    );

    assert!(!decision.eligible);
    assert!(decision
        .reasons
        .contains(&"path_not_allowlisted".to_string()));
}

#[test]
fn manual_apply_gate_rejects_missing_approval_bad_paths_and_high_risk() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    init_patch_repo(cwd);
    let run = QuarantinePatchRun {
        run_id: "run-2".to_string(),
        candidate_id: "candidate-2".to_string(),
        base_commit: git_stdout(cwd, &["rev-parse", "HEAD"]),
        patch_digest: "0".repeat(64),
        changed_paths: vec!["../escape.txt".to_string()],
        check_results: vec![decision_core::dreamer::PatchCheckResult {
            name: "focused".to_string(),
            command: vec!["git".to_string(), "status".to_string()],
            exit_code: Some(0),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }],
        risk: PatchRisk::High,
    };

    let decision = evaluate_manual_apply_gate(
        cwd,
        &ManualApplyGateRequest {
            approved_by_user: false,
            run,
            allowed_paths: vec!["file.txt".to_string()],
            reviewer_accepted: false,
        },
    );

    assert!(!decision.eligible);
    assert!(decision
        .reasons
        .contains(&"missing_user_approval".to_string()));
    assert!(decision
        .reasons
        .contains(&"path_not_allowlisted".to_string()));
    assert!(decision
        .reasons
        .contains(&"reviewer_not_accepted".to_string()));
    assert!(decision.reasons.contains(&"high_risk_patch".to_string()));
}

#[test]
fn run_applies_only_gated_promotions() {
    let _lock = crate::test_env_lock();
    // `a` repeated+verified → promote; `b` single session → skipped.
    let source = VecSource(vec![
        obs("a", "s1", true),
        obs("a", "s2", false),
        obs("b", "s1", true),
    ]);
    let store = MemStore::default();
    let report = Dreamer::new(source, store, PromotionPolicy::default())
        .run()
        .unwrap();

    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.applied[0].slug, "gotcha-a");
    assert!(!report.is_noop());
    // The skip is preserved in the audit trail.
    assert!(report.plan.skipped.iter().any(|s| s.slug == "gotcha-b"));
}

#[test]
fn run_writes_body_with_provenance() {
    let _lock = crate::test_env_lock();
    let source = VecSource(vec![obs("a", "s1", true), obs("a", "s2", true)]);
    let store = MemStore::default();
    let dreamer = Dreamer::new(source, store, PromotionPolicy::default());
    let report = dreamer.run().unwrap();

    assert_eq!(report.applied.len(), 1);
    let written = dreamer.store.written.borrow();
    let body = &written[0].body;
    assert!(body.contains("body a"));
    assert!(body.contains("- source: dreamer"));
    assert!(body.contains("confidence:"));
    let classification = crate::memory::classify_memory_body(body);
    assert_eq!(classification.source, crate::memory::MemorySource::Dreamer);
    assert_eq!(classification.kind, crate::memory::MemoryKind::Gotcha);
    assert!(!classification.protected);
}

#[test]
fn existing_slug_is_idempotent() {
    let _lock = crate::test_env_lock();
    let source = VecSource(vec![obs("a", "s1", true), obs("a", "s2", true)]);
    let store = MemStore {
        existing: vec!["gotcha-a".to_string()],
        ..MemStore::default()
    };
    let report = Dreamer::new(source, store, PromotionPolicy::default())
        .run()
        .unwrap();

    assert!(report.is_noop());
}

#[test]
fn existing_summary_template_prevents_family_growth_across_runs() {
    let _lock = crate::test_env_lock();
    let observations = ["s1", "s2"]
        .into_iter()
        .map(|session| LessonObservation {
            signature: "recurring tool failure: read_file".to_string(),
            session_id: session.to_string(),
            lesson: "`read_file` repeatedly failed; verify its inputs".to_string(),
            summary: "`read_file` is error-prone here — verify its inputs before relying on it"
                .to_string(),
            kind: LessonKind::Gotcha,
            verified: true,
        })
        .collect();
    let store = MemStore {
        existing_summaries: vec![(
            "gotcha-recurring-tool-failure-bash".to_string(),
            "`bash` is error-prone here — verify its inputs before relying on it".to_string(),
        )],
        ..MemStore::default()
    };

    let report = Dreamer::new(VecSource(observations), store, PromotionPolicy::default())
        .run()
        .expect("dreamer run");

    assert!(report.is_noop(), "dreamer report: {report:?}");
}

#[test]
fn jsonl_source_and_fs_store_round_trip_on_disk() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let dream_dir = cwd.join(".zo").join("dream");
        fs::create_dir_all(&dream_dir).unwrap();

        // Two distinct sessions report the same verified lesson → promotable.
        let mut log = String::new();
        for o in [
            obs("disk lesson", "s1", true),
            obs("disk lesson", "s2", true),
        ] {
            log.push_str(&serde_json::to_string(&o).unwrap());
            log.push('\n');
        }
        // Plus a corrupt line that must be tolerated, not fatal.
        log.push_str("{not valid json\n");
        fs::write(dream_dir.join("session.jsonl"), log).unwrap();

        // The on-disk source parses both good lines, skips the corrupt one.
        assert_eq!(JsonlLessonSource::at_cwd(cwd).observations().len(), 2);

        let report = dream_at_cwd(cwd, None).unwrap();
        assert_eq!(report.applied.len(), 1);
        let slug = &report.applied[0].slug;
        assert_eq!(slug, "gotcha-disk-lesson");

        // Entry file + index pointer were written in the standard layout.
        let memory_dir = crate::memory::paths::memory_write_dir(cwd, true);
        let entry = fs::read_to_string(memory_dir.join(format!("{slug}.md"))).unwrap();
        assert!(entry.contains("disk lesson"));
        let classification = crate::memory::classify_memory_body(&entry);
        assert_eq!(classification.source, crate::memory::MemorySource::Dreamer);
        assert_eq!(classification.kind, crate::memory::MemoryKind::Gotcha);
        assert!(!classification.protected);
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(index.contains("# Zo — Persistent Memory Index"));
        assert!(index.contains(&format!("- [{slug}]({slug}.md) — ")));

        // A second pass is a no-op: the slug is now known.
        let store = FsMemoryStore::local_at_cwd(cwd);
        assert!(store.existing_slugs().contains(slug));
        assert!(dream_at_cwd(cwd, None).unwrap().is_noop());
    });
}

#[test]
fn local_memory_cannot_override_a_read_only_global_slug() {
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let workspace = tempfile::tempdir().unwrap();
        let request = MemoryWriteRequest {
            slug: "durable-constraint".to_string(),
            summary: "global constraint".to_string(),
            body: "keep the global rule\n\n---\n- memory_metadata: v=1;source=hand_written;kind=constraint;protected=true;resolved_task_log=false;written_at=1".to_string(),
        };
        super::write_hand_written_memory_entry(workspace.path(), false, &request)
            .expect("explicit global write");
        let error = super::write_hand_written_memory_entry(workspace.path(), true, &request)
            .expect_err("local write must not shadow global knowledge");
        assert!(error.to_string().contains("read-only global"), "{error}");
        assert!(
            !crate::memory::paths::memory_write_dir(workspace.path(), true)
                .join("durable-constraint.md")
                .exists()
        );
    });
}

#[test]
fn compaction_curation_gate_skips_an_empty_workspace_without_writing_markers() {
    let workspace = tempfile::tempdir().unwrap();
    let report = super::maybe_auto_dream_on_compaction(
        workspace.path(),
        std::time::Duration::ZERO,
        Some("claude-opus-5"),
    )
    .expect("an empty workspace is a normal gated skip");
    assert!(report.is_none());
    assert!(
        !workspace.path().join(".zo").exists(),
        "the cheap gate must not create a dream directory"
    );
}

#[test]
fn a_promoted_lesson_records_the_model_whose_pass_promoted_it() {
    // Observations are mined from turn logs that carry no model of their own,
    // so the honest stamp is the model running the pass — and recall compares
    // against exactly that identity later.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let dream_dir = cwd.join(".zo").join("dream");
        fs::create_dir_all(&dream_dir).unwrap();
        let mut log = String::new();
        for o in [obs("tagged lesson", "s1", true), obs("tagged lesson", "s2", true)] {
            log.push_str(&serde_json::to_string(&o).unwrap());
            log.push('\n');
        }
        fs::write(dream_dir.join("session.jsonl"), log).unwrap();

        let report = dream_at_cwd(cwd, Some("GPT-5.6-Sol")).unwrap();
        let slug = &report.applied[0].slug;
        let memory_dir = crate::memory::paths::memory_write_dir(cwd, true);
        let entry = fs::read_to_string(memory_dir.join(format!("{slug}.md"))).unwrap();

        let classification = crate::memory::classify_memory_body(&entry);
        assert_eq!(
            classification.model.as_ref().map(crate::memory::MemoryModelTag::as_str),
            Some("gpt-5.6-sol"),
            "the pass normalizes and records its model: {entry}"
        );
        // The rest of the trailer is untouched by the new field.
        assert_eq!(classification.source, crate::memory::MemorySource::Dreamer);
        assert!(!classification.protected);
    });
}

#[test]
fn a_pass_with_no_model_to_name_promotes_untagged_entries() {
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let dream_dir = cwd.join(".zo").join("dream");
        fs::create_dir_all(&dream_dir).unwrap();
        let mut log = String::new();
        for o in [obs("untagged lesson", "s1", true), obs("untagged lesson", "s2", true)] {
            log.push_str(&serde_json::to_string(&o).unwrap());
            log.push('\n');
        }
        fs::write(dream_dir.join("session.jsonl"), log).unwrap();

        let report = dream_at_cwd(cwd, None).unwrap();
        let slug = &report.applied[0].slug;
        let memory_dir = crate::memory::paths::memory_write_dir(cwd, true);
        let entry = fs::read_to_string(memory_dir.join(format!("{slug}.md"))).unwrap();
        assert!(!entry.contains("model="), "no model, no tag: {entry}");
        assert_eq!(crate::memory::classify_memory_body(&entry).model, None);
    });
}

#[test]
fn user_pattern_source_promotes_verified_structured_preferences_only() {
    let env_lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("repo");
    let config_home = tmp.path().join("home").join(".zo");
    fs::create_dir_all(&cwd).unwrap();
    let summary = "pattern-7a";

    with_config_home(&env_lock, &config_home, || {
        assert!(!super::record_user_pattern_observation(&cwd, "s0", summary, false));
        assert!(!super::record_user_pattern_observation(
            &cwd,
            "s0",
            "pattern-7a
raw-extra",
            true
        ));
        assert!(super::record_user_pattern_observation(&cwd, "s1", summary, true));
        assert!(super::record_user_pattern_observation(&cwd, "s2", summary, true));

        let report = dream_at_cwd(&cwd, None).unwrap();
        assert_eq!(report.applied.len(), 1);
        assert!(report.applied[0].slug.starts_with("preference-user-pattern-"));
        let memory_dir = crate::memory::paths::memory_write_dir(&cwd, true);
        let body = fs::read_to_string(memory_dir.join(format!("{}.md", report.applied[0].slug)))
            .expect("promoted preference body");
        assert!(body.contains(summary));
        let classification = crate::memory::classify_memory_body(&body);
        assert_eq!(classification.kind, crate::memory::MemoryKind::Preference);
    });
}

#[test]
fn user_pattern_reader_revalidates_direct_jsonl_records() {
    let env_lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("repo");
    let config_home = tmp.path().join("home").join(".zo");
    fs::create_dir_all(&cwd).unwrap();

    with_config_home(&env_lock, &config_home, || {
        let dir = super::user_pattern_dir(&cwd);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(super::USER_PATTERN_FILE),
            r#"{"ts_ms":1,"observation":{"signature":"user-pattern:wrong","session_id":"s1","lesson":"pattern-7a","summary":"pattern-7a","kind":"preference","verified":true}}
{"ts_ms":2,"observation":{"signature":"user-pattern:wrong","session_id":"s2","lesson":"pattern-7a","summary":"pattern-7a","kind":"preference","verified":true}}
"#,
        )
        .unwrap();

        let source = super::UserPatternLessonSource::at_cwd(&cwd);

        assert!(source.observations().is_empty());
    });
}

#[cfg(unix)]
#[test]
fn user_pattern_writer_uses_global_state_not_symlinked_workspace_zo_dir() {
    let env_lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("repo");
    let target = tmp.path().join("target");
    let config_home = tmp.path().join("home").join(".zo");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, cwd.join(core_types::paths::ZO_DIR_NAME)).unwrap();

    with_config_home(&env_lock, &config_home, || {
        assert!(super::record_user_pattern_observation(&cwd, "s1", "pattern-7a", true));
        assert!(!target.join(super::DREAM_DIR).exists());
        assert!(super::user_pattern_dir(&cwd).starts_with(&config_home));
    });
}

#[test]
fn throttle_allows_first_run_and_blocks_within_window() {
    use std::time::{Duration, SystemTime};
    let _lock = crate::test_env_lock();
    let now = SystemTime::now();
    let window = std::time::Duration::from_secs(3600);

    // Never run before → allowed.
    assert!(super::should_auto_dream(None, now, window));
    // Ran 10 minutes ago, 1h window → blocked.
    let recent = now - Duration::from_secs(600);
    assert!(!super::should_auto_dream(Some(recent), now, window));
    // Ran 2 hours ago → allowed again.
    let old = now - Duration::from_secs(7200);
    assert!(super::should_auto_dream(Some(old), now, window));
    // Future timestamp (clock skew) → treated as just-ran, blocked.
    let future = now + Duration::from_secs(600);
    assert!(!super::should_auto_dream(Some(future), now, window));
}

#[test]
fn producer_and_consumer_must_share_one_cwd() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        // Regression guard for the cwd-divergence bug: the producer
        // (record_verified_check) and the consumer (dream_at_cwd / auto-dream)
        // must root `.zo/` at the SAME directory. If they diverge — as they
        // did when the producer used the live process cwd (changed by
        // EnterWorktree) while the consumer used the stable workspace cwd — the
        // verified lessons are stranded and never promoted.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        // Bug shape: producer writes under the worktree, consumer reads the
        // workspace → nothing is ever seen.
        assert!(super::record_verified_check(
            &worktree,
            "s1",
            Some("cargo test")
        ));
        assert!(super::record_verified_check(
            &worktree,
            "s2",
            Some("cargo test")
        ));
        assert!(
            dream_at_cwd(&workspace, None).unwrap().is_noop(),
            "divergent cwds strand the lessons (this is the bug we fixed)"
        );

        // Fixed shape: both sides agree on the workspace → the lesson promotes.
        assert!(super::record_verified_check(
            &workspace,
            "s1",
            Some("cargo test")
        ));
        assert!(super::record_verified_check(
            &workspace,
            "s2",
            Some("cargo test")
        ));
        assert_eq!(dream_at_cwd(&workspace, None).unwrap().applied.len(), 1);
    });
}

#[test]
fn verified_check_observation_requires_a_command() {
    let _lock = crate::test_env_lock();
    // No command → not a verified outcome → nothing to record.
    assert!(super::verified_check_observation("s1", None).is_none());
    assert!(super::verified_check_observation("s1", Some("   ")).is_none());

    // With a command → a verified Workflow lesson keyed on the command alone.
    let obs = super::verified_check_observation("s1", Some("cargo test")).unwrap();
    assert!(obs.verified);
    assert_eq!(obs.kind, LessonKind::Workflow);
    assert!(obs.signature.contains("cargo test"));
    // The signature ignores the session, so distinct sessions dedup together.
    let other = super::verified_check_observation("s2", Some("cargo test")).unwrap();
    assert_eq!(obs.signature, other.signature);
}

#[test]
fn record_verified_check_promotes_after_repeating_across_sessions() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        // One green accept per session for the same project check command.
        assert!(super::record_verified_check(
            cwd,
            "sess-1",
            Some("cargo test")
        ));
        assert!(super::record_verified_check(
            cwd,
            "sess-2",
            Some("cargo test")
        ));
        // An accept with no check command records nothing.
        assert!(!super::record_verified_check(cwd, "sess-3", None));

        // Two distinct sessions agree → the verified workflow lesson promotes.
        let report = dream_at_cwd(cwd, None).unwrap();
        assert_eq!(report.applied.len(), 1);
        assert!(report.applied[0].slug.starts_with("workflow-"));
    });
}

#[test]
fn single_green_accept_does_not_promote() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        // Only one session saw it → below the repetition bar → not promoted.
        assert!(super::record_verified_check(
            cwd,
            "sess-1",
            Some("npm test")
        ));
        assert!(dream_at_cwd(cwd, None).unwrap().is_noop());
    });
}

#[test]
fn record_observation_feeds_the_dreamer_end_to_end() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        // The full closed loop with no test doubles: a session *records*
        // observations via the producer API, and a later dreaming pass promotes
        // the repeated+verified one — proving producer and consumer agree on the
        // on-disk format.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        // Two distinct sessions each record the same verified lesson.
        super::record_observation(cwd, &obs("loop lesson", "sess-1", true)).unwrap();
        super::record_observation(cwd, &obs("loop lesson", "sess-2", true)).unwrap();
        // A one-off unverified lesson that must NOT be promoted.
        super::record_observation(cwd, &obs("noise", "sess-1", false)).unwrap();

        let report = dream_at_cwd(cwd, None).unwrap();
        let promoted: Vec<&str> = report.applied.iter().map(|a| a.slug.as_str()).collect();
        assert_eq!(promoted, vec!["gotcha-loop-lesson"]);
        assert!(report.plan.skipped.iter().any(|s| s.slug == "gotcha-noise"));
    });
}

#[test]
fn turn_trace_failures_promote_into_memory_through_dream_at_cwd() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        use crate::turn_trace::{append, TurnRecord};
        // The plan-3 closed loop end-to-end: the externalized turn trace records
        // tool failures, and a dreaming pass mines them (alongside the dream/
        // producer) into a promoted gotcha — no deep-gate green accept involved.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        // `bash` errors in two distinct sessions; `grep` only in one.
        let mut rec1 = TurnRecord::terminal(
            "sess-1",
            1,
            crate::turn_trace::TurnOutcome::Completed,
            0,
            None,
        );
        rec1.error_tools = vec!["bash".to_string(), "grep".to_string()];
        append(cwd, &rec1).unwrap();
        let mut rec2 = TurnRecord::terminal(
            "sess-2",
            1,
            crate::turn_trace::TurnOutcome::Completed,
            0,
            None,
        );
        rec2.error_tools = vec!["bash".to_string()];
        append(cwd, &rec2).unwrap();

        let report = dream_at_cwd(cwd, None).unwrap();
        let promoted: Vec<&str> = report.applied.iter().map(|a| a.slug.as_str()).collect();
        // `bash` repeated across two sessions → promoted; `grep` (one session)
        // stays below the repetition bar.
        assert_eq!(promoted, vec!["gotcha-recurring-tool-failure-bash"]);

        // The promoted entry is a real memory file describing the recurring tool.
        let entry = fs::read_to_string(
            crate::memory::paths::memory_write_dir(cwd, true)
                .join("gotcha-recurring-tool-failure-bash.md"),
        )
        .unwrap();
        assert!(entry.contains("bash"));
    });
}

#[test]
fn composite_source_merges_both_producers() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        use crate::turn_trace::{append, TurnRecord};
        // Both signal streams reach one dreaming pass: a green-accept workflow
        // lesson (dream/) and a recurring tool failure (turns/) promote together.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        // Producer 1: green-accept check command in two sessions.
        assert!(super::record_verified_check(cwd, "s1", Some("cargo test")));
        assert!(super::record_verified_check(cwd, "s2", Some("cargo test")));

        // Producer 2: a tool failing in two sessions.
        for (i, sess) in ["s1", "s2"].iter().enumerate() {
            let mut rec = TurnRecord::terminal(
                sess,
                i as u64,
                crate::turn_trace::TurnOutcome::Completed,
                0,
                None,
            );
            rec.error_tools = vec!["read_file".to_string()];
            append(cwd, &rec).unwrap();
        }

        let report = dream_at_cwd(cwd, None).unwrap();
        let mut promoted: Vec<&str> = report.applied.iter().map(|a| a.slug.as_str()).collect();
        promoted.sort_unstable();
        assert_eq!(
            promoted,
            vec![
                "gotcha-recurring-tool-failure-read-file",
                "workflow-verified-check-command-cargo-test"
            ]
        );
    });
}

#[test]
fn maybe_auto_dream_throttles_second_call() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let dream_dir = cwd.join(".zo").join("dream");
    fs::create_dir_all(&dream_dir).unwrap();

    let mut log = String::new();
    for o in [obs("auto x", "s1", true), obs("auto x", "s2", true)] {
        log.push_str(&serde_json::to_string(&o).unwrap());
        log.push('\n');
    }
    fs::write(dream_dir.join("s.jsonl"), log).unwrap();

    // First call (no marker) runs and promotes; marker is then stamped.
    let first = super::maybe_auto_dream(cwd, std::time::Duration::from_secs(3600), None).unwrap();
    assert!(first.is_some());
    assert_eq!(first.unwrap().applied.len(), 1);

    // Immediate second call is throttled → None, no extra work.
    let second = super::maybe_auto_dream(cwd, std::time::Duration::from_secs(3600), None).unwrap();
    assert!(second.is_none());
}

#[test]
fn automation_events_promote_through_dream_at_cwd_without_goal_text() {
    // Hermetic: `dream_at_cwd`/`at_cwd` resolve the store through the real
    // global config home when `ZO_CONFIG_HOME` is unset — i.e. under plain
    // `cargo test` — so this test used to deposit a slug in the developer's
    // actual `~/.zo/projects` on every run.
    let config_home = tempfile::tempdir().unwrap();
    let env_lock = crate::test_env_lock();
    with_config_home(&env_lock, config_home.path(), || {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        super::record_automation_event(cwd, "sess-1", "goal", "repair_queued", false).unwrap();
        super::record_automation_event(cwd, "sess-1", "goal", "succeeded", true).unwrap();
        super::record_automation_event(cwd, "sess-2", "goal", "repair_queued", false).unwrap();
        super::record_automation_event(cwd, "sess-2", "goal", "succeeded", true).unwrap();

        let report = dream_at_cwd(cwd, None).unwrap();
        let promoted: Vec<&str> = report.applied.iter().map(|a| a.slug.as_str()).collect();
        assert_eq!(
            promoted,
            vec!["workflow-goal-automation-repaired-then-succeeded"]
        );
        let entry = fs::read_to_string(
            crate::memory::paths::memory_write_dir(cwd, true)
                .join("workflow-goal-automation-repaired-then-succeeded.md"),
        )
        .unwrap();
        assert!(entry.contains("/goal"));
        assert!(!entry.contains("secret goal text"));
    });
}

#[test]
fn auto_dream_failure_is_recorded_without_failing_startup() {
    let _lock = crate::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let error = DreamError::Io(std::io::Error::other("simulated failure"));

    super::record_auto_dream_failure(cwd, &error).unwrap();

    let marker = fs::read_to_string(
        cwd.join(".zo")
            .join("dream")
            .join(super::AUTO_DREAM_ERROR_FILE),
    )
    .unwrap();
    assert!(marker.contains("dreamer_io_error"));
    assert!(!marker.contains("simulated failure"));
    assert!(marker.contains("tsMs"));
}

/// **중복 저장 회귀 핀** — `MemoryWrite` 도구는 매 저장마다 `written_at`을 새로
/// 찍으므로, 모델이 세션마다 같은 교훈을 재저장하면 바이트만 다른 같은 내용이
/// journal·entry·marker·index 네 파일을 다시 썼다. mtime churn은 외관 문제가
/// 아니다: 보존 스윕이 mtime으로 나이를 재므로, 재저장만 되는 store가 "방금
/// 작성됨"으로 읽혔다.
#[test]
fn re_saving_an_identical_hand_written_entry_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir_all(&memory_dir).unwrap();
    let request = |written_at: u64| MemoryWriteRequest {
        slug: "user-lesson".to_string(),
        summary: "a lesson the model keeps re-recording".to_string(),
        body: format!(
            "the lesson body\n\n---\n{}",
            crate::memory::MemoryClassification::hand_written(
                crate::memory::MemoryKind::Gotcha,
                Some(written_at),
                None,
            )
            .metadata_line()
        ),
    };

    let first = super::write_memory_entry_transaction(
        &memory_dir,
        &request(1_000),
        super::MemoryWriteOwner::HandWritten,
    )
    .unwrap();
    assert_eq!(first, WriteOutcome::Created);
    let entry_path = memory_dir.join("user-lesson.md");
    let index_path = memory_dir.join("MEMORY.md");
    let entry_before = fs::read_to_string(&entry_path).unwrap();
    let entry_mtime = fs::metadata(&entry_path).unwrap().modified().unwrap();
    let index_mtime = fs::metadata(&index_path).unwrap().modified().unwrap();

    // The re-save: same content, later stamp — the shape every session produces.
    std::thread::sleep(std::time::Duration::from_millis(30));
    let second = super::write_memory_entry_transaction(
        &memory_dir,
        &request(2_000),
        super::MemoryWriteOwner::HandWritten,
    )
    .unwrap();

    assert_eq!(second, WriteOutcome::Unchanged);
    assert_eq!(
        fs::read_to_string(&entry_path).unwrap(),
        entry_before,
        "the original written_at survives — 'written' keeps meaning authored"
    );
    assert_eq!(
        fs::metadata(&entry_path).unwrap().modified().unwrap(),
        entry_mtime,
        "the entry was not rewritten"
    );
    assert_eq!(
        fs::metadata(&index_path).unwrap().modified().unwrap(),
        index_mtime,
        "the index was not rewritten"
    );

    // A body that actually changed still writes.
    let third = super::write_memory_entry_transaction(
        &memory_dir,
        &MemoryWriteRequest {
            slug: "user-lesson".to_string(),
            summary: "a lesson the model keeps re-recording".to_string(),
            body: "a genuinely different lesson body".to_string(),
        },
        super::MemoryWriteOwner::HandWritten,
    )
    .unwrap();
    assert_eq!(third, WriteOutcome::Updated);
}

/// 같은 body 라도 **summary 가 바뀌면** 진짜 쓰기다 — index 포인터가 갱신돼야 한다.
#[test]
fn a_changed_summary_is_a_real_write() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir_all(&memory_dir).unwrap();
    let request = |summary: &str| MemoryWriteRequest {
        slug: "renamed".to_string(),
        summary: summary.to_string(),
        body: "same body".to_string(),
    };
    assert_eq!(
        super::write_memory_entry_transaction(
            &memory_dir,
            &request("old summary"),
            super::MemoryWriteOwner::HandWritten,
        )
        .unwrap(),
        WriteOutcome::Created
    );
    assert_eq!(
        super::write_memory_entry_transaction(
            &memory_dir,
            &request("new summary"),
            super::MemoryWriteOwner::HandWritten,
        )
        .unwrap(),
        WriteOutcome::Updated
    );
    let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    assert!(index.contains("new summary"), "{index}");
    assert!(!index.contains("old summary"), "{index}");
}

/// Dreamer 소유 항목의 동일 재승격도 무변경으로 끝난다 (직접 store 호출 경로 —
/// 정상 흐름에선 curation 의 `AlreadyKnown` 이 먼저 거르지만, 그 게이트가
/// 무너져도 store 가 최후의 멱등성을 지킨다).
#[test]
fn an_identical_dreamer_promotion_is_unchanged_at_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().join("memory");
    fs::create_dir_all(&memory_dir).unwrap();
    let request = MemoryWriteRequest {
        slug: "settled".to_string(),
        summary: "settled lesson".to_string(),
        body: "identical dreamer body".to_string(),
    };
    assert_eq!(
        super::write_memory_entry_transaction(
            &memory_dir,
            &request,
            super::MemoryWriteOwner::Dreamer,
        )
        .unwrap(),
        WriteOutcome::Created
    );
    assert_eq!(
        super::write_memory_entry_transaction(
            &memory_dir,
            &request,
            super::MemoryWriteOwner::Dreamer,
        )
        .unwrap(),
        WriteOutcome::Unchanged
    );
}
