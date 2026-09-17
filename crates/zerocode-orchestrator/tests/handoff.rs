use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;
use zerocode_orchestrator::GIT_EXECUTABLE;
use zerocode_orchestrator::Orchestrator;
use zerocode_orchestrator::handoff::{
    CoverageGap, GitOperation, HandoffError, HandoffLineage, HandoffManifestV1, HiddenIndexFlag,
    PublishBlocker, PublishPolicy, SnapshotCoverage, SnapshotLimits, TestReceipt, TestRequirement,
    WorktreeSnapshot,
};

fn git_output(root: &Path, args: &[&str]) -> Output {
    Command::new(GIT_EXECUTABLE)
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs")
}

fn git(root: &Path, args: &[&str]) {
    let output = git_output(root, args);
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> TempDir {
    let root = tempfile::tempdir().expect("temp repository");
    git(root.path(), &["init", "-b", "main"]);
    git(root.path(), &["config", "user.name", "ZeroCode Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@zerocode.local"],
    );
    std::fs::write(root.path().join("tracked.txt"), "one\n").expect("seed file");
    git(root.path(), &["add", "tracked.txt"]);
    git(root.path(), &["commit", "-m", "seed"]);
    root
}

fn lineage() -> HandoffLineage {
    HandoffLineage {
        run_id: "run-1".to_string(),
        task_id: "task-1".to_string(),
        dispatch_id: "dispatch-1".to_string(),
        worker_id: "worker-1".to_string(),
        parent_manifest_id: None,
    }
}

#[test]
fn a_clean_snapshot_is_stable_and_does_not_serialize_its_private_path() {
    let root = repository();
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("stable snapshot");
    assert_eq!(
        orchestrator
            .repository_id(root.path())
            .expect("repository identity"),
        snapshot.repository_id
    );

    assert_eq!(snapshot.branch.as_deref(), Some("main"));
    assert!(!snapshot.head_oid.is_empty());
    assert!(snapshot.changes.is_empty());
    assert!(snapshot.coverage.is_complete());
    let canonical = root.path().canonicalize().expect("canonical temp path");
    assert_eq!(
        snapshot.private_path.as_ref().map(|path| path.as_path()),
        Some(canonical.as_path())
    );
    assert!(!format!("{snapshot:?}").contains(&canonical.to_string_lossy().to_string()));

    let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
    assert!(!json.contains(&canonical.to_string_lossy().to_string()));
    assert!(!json.contains("private_path"));
    let received: zerocode_orchestrator::handoff::WorktreeSnapshot =
        serde_json::from_str(&json).expect("deserialize remote snapshot");
    assert!(received.private_path.is_none());
    let mut unknown: serde_json::Value = serde_json::from_str(&json).expect("snapshot value");
    unknown
        .as_object_mut()
        .expect("snapshot object")
        .insert("unversioned_field".to_string(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<zerocode_orchestrator::handoff::WorktreeSnapshot>(unknown)
            .is_err()
    );
}

#[test]
fn an_unborn_worktree_is_named_as_having_no_handoff_head() {
    let root = tempfile::tempdir().expect("empty repository");
    git(root.path(), &["init", "-b", "main"]);
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    assert!(matches!(
        orchestrator.handoff_snapshot(root.path(), 1_000),
        Err(HandoffError::NoHead { .. })
    ));
}

#[test]
fn equal_status_rows_with_different_file_bytes_have_different_digests() {
    let root = repository();
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    std::fs::write(root.path().join("tracked.txt"), "two\n").expect("first edit");
    let first = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("first snapshot");
    std::fs::write(root.path().join("tracked.txt"), "six\n").expect("second edit");
    let second = orchestrator
        .handoff_snapshot(root.path(), 2_000)
        .expect("second snapshot");

    assert_eq!(first.changes, second.changes, "status shape changed");
    assert_ne!(first.content_digest, second.content_digest);
}

#[test]
fn user_diff_presentation_config_does_not_change_the_snapshot_digest() {
    let root = repository();
    std::fs::write(root.path().join("tracked.txt"), "changed\n").expect("edit");
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let first = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("first snapshot");
    for (name, value) in [
        ("color.ui", "always"),
        ("diff.algorithm", "histogram"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.noprefix", "true"),
        ("diff.renames", "true"),
        ("status.renames", "true"),
    ] {
        git(root.path(), &["config", name, value]);
    }
    let second = orchestrator
        .handoff_snapshot(root.path(), 2_000)
        .expect("second snapshot");
    assert_eq!(first.content_digest, second.content_digest);
}

#[test]
fn ignored_contents_are_named_but_excluded_and_block_publication() {
    let root = repository();
    std::fs::write(root.path().join(".gitignore"), ".secret\n").expect("ignore rule");
    git(root.path(), &["add", ".gitignore"]);
    git(root.path(), &["commit", "-m", "ignore local secret"]);
    std::fs::write(root.path().join(".secret"), "first secret\n").expect("ignored file");
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let first = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("first snapshot");
    std::fs::write(root.path().join(".secret"), "other secret\n").expect("ignored edit");
    let second = orchestrator
        .handoff_snapshot(root.path(), 2_000)
        .expect("second snapshot");

    assert_eq!(first.ignored, vec![".secret"]);
    assert_eq!(first.content_digest, second.content_digest);
    assert!(
        first
            .coverage
            .gaps
            .contains(&CoverageGap::IgnoredContent { count: 1 })
    );

    let manifest =
        HandoffManifestV1::new(1, 1_000, lineage(), first, Vec::new()).expect("manifest");
    let blockers = manifest.structural_publish_blockers(&PublishPolicy::default());
    assert!(blockers.iter().any(|blocker| matches!(
        blocker,
        PublishBlocker::IncompleteSnapshot {
            gap: CoverageGap::IgnoredContent { count: 1 }
        }
    )));
    assert!(blockers.contains(&PublishBlocker::DirtySnapshotNotMaterialized));
}

#[test]
fn tests_must_pass_on_the_exact_snapshot_the_manifest_carries() {
    let root = repository();
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("snapshot");
    let policy = PublishPolicy {
        required_tests: vec![TestRequirement {
            name: "rust".to_string(),
            command_id: "cargo-test-v1".to_string(),
            must_fail_first: false,
        }],
    };

    let wrong_command = TestReceipt::new(
        "rust",
        "unrelated-command",
        snapshot.content_digest.clone(),
        10,
        20,
        0,
    );
    let wrong_manifest =
        HandoffManifestV1::new(1, 1_000, lineage(), snapshot.clone(), vec![wrong_command])
            .expect("wrong-command manifest");
    assert!(
        wrong_manifest
            .structural_publish_blockers(&policy)
            .contains(&PublishBlocker::UnexpectedTestCommand {
                name: "rust".to_string(),
            })
    );

    let stale = TestReceipt::new("rust", "cargo-test-v1", "older-snapshot", 10, 20, 0);
    let stale_manifest = HandoffManifestV1::new(1, 1_000, lineage(), snapshot.clone(), vec![stale])
        .expect("stale manifest");
    assert!(
        stale_manifest
            .structural_publish_blockers(&policy)
            .contains(&PublishBlocker::StaleTest {
                name: "rust".to_string()
            })
    );

    let failed = TestReceipt::new(
        "rust",
        "cargo-test-v1",
        snapshot.content_digest.clone(),
        10,
        20,
        1,
    );
    let failed_manifest =
        HandoffManifestV1::new(1, 1_000, lineage(), snapshot.clone(), vec![failed])
            .expect("failed manifest");
    assert!(
        failed_manifest
            .structural_publish_blockers(&policy)
            .contains(&PublishBlocker::FailedTest {
                name: "rust".to_string(),
                exit_code: 1,
            })
    );

    let passed = TestReceipt::new(
        "rust",
        "cargo-test-v1",
        snapshot.content_digest.clone(),
        10,
        20,
        0,
    );
    let ready = HandoffManifestV1::new(1, 1_000, lineage(), snapshot, vec![passed])
        .expect("ready manifest");
    assert!(ready.structural_publish_blockers(&policy).is_empty());
}

#[test]
fn an_in_progress_merge_is_a_state_and_never_publishable() {
    let root = repository();
    git(root.path(), &["switch", "-c", "other"]);
    std::fs::write(root.path().join("tracked.txt"), "other\n").expect("other edit");
    git(root.path(), &["commit", "-am", "other"]);
    git(root.path(), &["switch", "main"]);
    std::fs::write(root.path().join("tracked.txt"), "main\n").expect("main edit");
    git(root.path(), &["commit", "-am", "main"]);
    assert!(
        !git_output(root.path(), &["merge", "other"])
            .status
            .success()
    );

    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("conflicted snapshot");
    assert_eq!(snapshot.operation, Some(GitOperation::Merge));
    assert!(snapshot.has_unmerged_paths());

    let manifest =
        HandoffManifestV1::new(1, 1_000, lineage(), snapshot, Vec::new()).expect("manifest");
    let blockers = manifest.structural_publish_blockers(&PublishPolicy::default());
    assert!(blockers.contains(&PublishBlocker::GitOperation {
        operation: GitOperation::Merge,
    }));
    assert!(blockers.contains(&PublishBlocker::UnmergedPaths));
}

#[test]
fn upstream_ahead_and_behind_are_observations_not_guesses() {
    let root = repository();
    let remote = tempfile::tempdir().expect("bare remote");
    git(remote.path(), &["init", "--bare"]);
    let remote_path = remote.path().to_string_lossy();
    git(root.path(), &["remote", "add", "origin", &remote_path]);
    git(root.path(), &["push", "-u", "origin", "main"]);
    std::fs::write(root.path().join("tracked.txt"), "ahead\n").expect("ahead edit");
    git(root.path(), &["commit", "-am", "ahead"]);

    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("snapshot with upstream");
    assert_eq!(
        snapshot.upstream.as_ref().map(|one| one.name.as_str()),
        Some("origin/main")
    );
    assert_eq!(snapshot.ahead, Some(1));
    assert_eq!(snapshot.behind, Some(0));
}

#[test]
fn moving_a_linked_worktree_does_not_change_its_identity() {
    let root = repository();
    let holder = tempfile::tempdir().expect("worktree holder");
    let first_path = holder.path().join("first");
    let second_path = holder.path().join("second");
    let first_word = first_path.to_string_lossy();
    let second_word = second_path.to_string_lossy();
    git(
        root.path(),
        &["worktree", "add", "-b", "feature/handoff", &first_word],
    );
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let first = orchestrator
        .handoff_snapshot(&first_path, 1_000)
        .expect("first location");
    git(
        root.path(),
        &["worktree", "move", &first_word, &second_word],
    );
    let second = orchestrator
        .handoff_snapshot(&second_path, 2_000)
        .expect("second location");

    assert_eq!(first.worktree_id, second.worktree_id);
    assert_ne!(first.worktree_name, second.worktree_name);
}

#[test]
fn a_content_limit_is_a_blocker_instead_of_a_partial_success() {
    let root = repository();
    std::fs::write(
        root.path().join("tracked.txt"),
        "a change larger than eight bytes\n",
    )
    .expect("large edit");
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot_with_limits(
            root.path(),
            1_000,
            SnapshotLimits {
                content_bytes: 8,
                ..SnapshotLimits::default()
            },
        )
        .expect("bounded snapshot");

    assert_eq!(snapshot.coverage.content_bytes, 8);
    assert!(snapshot.coverage.gaps.iter().any(|gap| matches!(
        gap,
        CoverageGap::ContentByteLimit {
            source,
            limit: 8,
            ..
        } if source == "unstaged_diff"
    )));
    let manifest =
        HandoffManifestV1::new(1, 1_000, lineage(), snapshot, Vec::new()).expect("manifest");
    assert!(
        manifest
            .structural_publish_blockers(&PublishPolicy::default())
            .iter()
            .any(|blocker| {
                matches!(
                    blocker,
                    PublishBlocker::IncompleteSnapshot {
                        gap: CoverageGap::ContentByteLimit { .. }
                    }
                )
            })
    );
}

#[test]
fn index_flags_that_hide_live_bytes_are_explicit_blockers() {
    let root = repository();
    std::fs::write(root.path().join("second.txt"), "one\n").expect("second file");
    git(root.path(), &["add", "second.txt"]);
    git(root.path(), &["commit", "-m", "second file"]);
    git(
        root.path(),
        &["update-index", "--assume-unchanged", "tracked.txt"],
    );
    git(
        root.path(),
        &["update-index", "--skip-worktree", "second.txt"],
    );
    std::fs::write(root.path().join("tracked.txt"), "hidden one\n").expect("assumed edit");
    std::fs::write(root.path().join("second.txt"), "hidden two\n").expect("skipped edit");

    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("fail-closed snapshot");
    assert!(
        snapshot.changes.is_empty(),
        "git itself exposed the hidden paths"
    );
    assert!(
        snapshot
            .coverage
            .gaps
            .contains(&CoverageGap::IndexHiddenContent {
                path: "tracked.txt".to_string(),
                flag: HiddenIndexFlag::AssumeUnchanged,
            })
    );
    assert!(
        snapshot
            .coverage
            .gaps
            .contains(&CoverageGap::IndexHiddenContent {
                path: "second.txt".to_string(),
                flag: HiddenIndexFlag::SkipWorktree,
            })
    );
}

#[test]
fn oversized_git_metadata_is_refused_before_it_enters_a_manifest() {
    let root = repository();
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    assert!(matches!(
        orchestrator.handoff_snapshot_with_limits(
            root.path(),
            1_000,
            SnapshotLimits {
                git_output_bytes: 1,
                ..SnapshotLimits::default()
            },
        ),
        Err(HandoffError::GitOutputTooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn replacing_a_tracked_directory_with_an_external_symlink_reads_no_external_bytes() {
    use std::os::unix::fs::symlink;

    let root = repository();
    std::fs::create_dir(root.path().join("dir")).expect("tracked directory");
    std::fs::write(root.path().join("dir/file.txt"), "inside\n").expect("tracked child");
    git(root.path(), &["add", "dir/file.txt"]);
    git(root.path(), &["commit", "-m", "tracked child"]);

    let outside = tempfile::tempdir().expect("outside directory");
    std::fs::write(outside.path().join("file.txt"), "outside one\n").expect("outside file");
    std::fs::remove_dir_all(root.path().join("dir")).expect("remove tracked directory");
    symlink(outside.path(), root.path().join("dir")).expect("external symlink");

    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let first = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("first snapshot");
    std::fs::write(outside.path().join("file.txt"), "outside two\n").expect("outside edit");
    let second = orchestrator
        .handoff_snapshot(root.path(), 2_000)
        .expect("second snapshot");

    assert_eq!(
        first.content_digest, second.content_digest,
        "outside worktree bytes entered the digest"
    );
    assert!(
        first
            .coverage
            .gaps
            .contains(&CoverageGap::UntrackedContent { count: 1 })
    );
}

#[test]
fn submodule_ignore_all_cannot_hide_a_moved_submodule_commit() {
    let root = repository();
    let source = repository();
    let source_path = source.path().to_string_lossy();
    git(
        root.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &source_path,
            "child",
        ],
    );
    git(root.path(), &["commit", "-am", "add submodule"]);
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let before = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("clean submodule snapshot");

    let child = root.path().join("child");
    git(&child, &["config", "user.name", "ZeroCode Test"]);
    git(&child, &["config", "user.email", "test@zerocode.local"]);
    std::fs::write(child.join("tracked.txt"), "moved\n").expect("submodule edit");
    git(&child, &["commit", "-am", "move submodule"]);
    git(root.path(), &["config", "submodule.child.ignore", "all"]);

    let after = orchestrator
        .handoff_snapshot(root.path(), 2_000)
        .expect("submodule move is visible");
    assert!(after.changes.iter().any(|change| {
        change.path == "child"
            && change
                .submodule
                .is_some_and(|submodule| submodule.commit_changed)
    }));
    assert_ne!(before.content_digest, after.content_digest);
}

#[cfg(target_os = "linux")]
#[test]
fn an_invalid_utf8_path_is_refused_instead_of_collapsed() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let root = repository();
    let name = OsString::from_vec(b"bad-\xff".to_vec());
    let path = root.path().join(&name);
    std::fs::write(&path, "one\n").expect("non utf8 file");
    let added = Command::new(GIT_EXECUTABLE)
        .arg("-C")
        .arg(root.path())
        .arg("add")
        .arg(&name)
        .output()
        .expect("git add");
    assert!(added.status.success());
    git(root.path(), &["commit", "-m", "non utf8 path"]);
    std::fs::write(path, "two\n").expect("edit non utf8 file");

    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    assert!(matches!(
        orchestrator.handoff_snapshot(root.path(), 1_000),
        Err(HandoffError::NonUtf8GitOutput { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_git_command_that_never_returns_hits_the_snapshot_deadline() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    let root = repository();
    let wrapper_dir = tempfile::tempdir().expect("wrapper directory");
    let wrapper = wrapper_dir.path().join("git-wrapper");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\ncase \" $* \" in *\" worktree list --porcelain \"*) exec sleep 5;; esac\nexec git \"$@\"\n",
    )
    .expect("git wrapper");
    let mut permissions = std::fs::metadata(&wrapper)
        .expect("wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).expect("executable wrapper");

    let orchestrator =
        Orchestrator::open_with_git(root.path(), &wrapper).expect("open through wrapper");
    let started = Instant::now();
    let result = orchestrator.handoff_snapshot_with_limits(
        root.path(),
        1_000,
        SnapshotLimits {
            git_timeout: Duration::from_millis(50),
            ..SnapshotLimits::default()
        },
    );
    assert!(matches!(result, Err(HandoffError::GitTimeout { .. })));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn a_descendant_holding_git_pipes_cannot_outlive_the_call_deadline() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    let root = repository();
    let wrapper_dir = tempfile::tempdir().expect("wrapper directory");
    let wrapper = wrapper_dir.path().join("git-wrapper");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\ncase \" $* \" in *\" worktree list --porcelain \"*) (sleep 5) & exit 0;; esac\nexec git \"$@\"\n",
    )
    .expect("git wrapper");
    let mut permissions = std::fs::metadata(&wrapper)
        .expect("wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).expect("executable wrapper");

    let orchestrator =
        Orchestrator::open_with_git(root.path(), &wrapper).expect("open through wrapper");
    let started = Instant::now();
    let result = orchestrator.handoff_snapshot_with_limits(
        root.path(),
        1_000,
        SnapshotLimits {
            git_timeout: Duration::from_millis(50),
            ..SnapshotLimits::default()
        },
    );
    assert!(matches!(result, Err(HandoffError::GitTimeout { .. })));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn every_material_change_to_manifest_evidence_changes_its_id() {
    let root = repository();
    let orchestrator = Orchestrator::open(root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1_000)
        .expect("snapshot");
    let receipt = TestReceipt::new(
        "rust",
        "cargo-test-v1",
        snapshot.content_digest.clone(),
        10,
        20,
        0,
    );
    let first =
        HandoffManifestV1::new(1, 1_000, lineage(), snapshot.clone(), vec![receipt.clone()])
            .expect("first manifest");

    let mut locked = snapshot.clone();
    locked.locked = true;
    let second = HandoffManifestV1::new(1, 1_000, lineage(), locked, vec![receipt.clone()])
        .expect("locked manifest");
    assert_ne!(first.manifest_id, second.manifest_id);

    let mut truncated = receipt;
    truncated.output_truncated = true;
    let third = HandoffManifestV1::new(1, 1_000, lineage(), snapshot, vec![truncated])
        .expect("truncated manifest");
    assert_ne!(first.manifest_id, third.manifest_id);

    let mut tampered = first;
    tampered.schema_version = 999;
    tampered.worktree.locked = true;
    let blockers = tampered.structural_publish_blockers(&PublishPolicy::default());
    assert!(blockers.iter().any(|blocker| matches!(
        blocker,
        PublishBlocker::UnsupportedSchema { manifest: 999, .. }
    )));
    assert!(blockers.contains(&PublishBlocker::ManifestIdMismatch));
}

#[test]
fn manifest_v1_has_a_golden_identity() {
    let snapshot = WorktreeSnapshot {
        schema_version: 1,
        observed_at_ms: 100,
        repository_id: "repo-fixed".to_string(),
        worktree_id: "worktree-fixed".to_string(),
        worktree_name: "feature".to_string(),
        branch: Some("feature/handoff".to_string()),
        detached: false,
        locked: false,
        head_oid: "1".repeat(40),
        upstream: None,
        ahead: None,
        behind: None,
        operation: None,
        changes: Vec::new(),
        ignored: Vec::new(),
        content_digest: "2".repeat(64),
        coverage: SnapshotCoverage {
            changed_paths: 0,
            content_bytes: 0,
            gaps: Vec::new(),
        },
        private_path: None,
    };
    let receipt = TestReceipt::new(
        "rust",
        "cargo-test-v1",
        snapshot.content_digest.clone(),
        110,
        120,
        0,
    );
    let manifest =
        HandoffManifestV1::new(7, 130, lineage(), snapshot, vec![receipt]).expect("manifest");
    assert_eq!(
        manifest.manifest_id,
        "handoff-fb37e72d77ea717f6cc5bd7d1960adadb08a33b7cbba8144f661e0cc5ccbcc8c"
    );
}
