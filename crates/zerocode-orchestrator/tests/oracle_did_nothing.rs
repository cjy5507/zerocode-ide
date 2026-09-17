//! What a green receipt proves, and what it does not.
//!
//! Before an automation is allowed to promote a cheap model on the strength of
//! a passing verification, the receipt has to be asked what it actually says.
//! These drive the real [`VerificationRunner`] against real repositories, so
//! the answer is git's and the process exit code's rather than a fixture's.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tempfile::TempDir;
use zerocode_orchestrator::GIT_EXECUTABLE;
use zerocode_orchestrator::Orchestrator;
use zerocode_orchestrator::handoff::{
    HandoffLineage, HandoffManifestV1, PublishBlocker, PublishPolicy, TestReceipt, TestRequirement,
};
use zerocode_orchestrator::test_evidence::{
    TrustedTestEvidence, VerificationCatalog, VerificationCommand, VerificationRunner,
};
use zerocode_orchestrator::workflow_store::WorkflowStore;

const COMMAND_ID: &str = "check-fixed-v1";
const TEST_NAME: &str = "regression";

fn git(root: &Path, args: &[&str]) {
    let output = Command::new(GIT_EXECUTABLE)
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository whose regression check is `fixed.txt exists`.
fn repository() -> TempDir {
    let root = tempfile::tempdir().expect("temp repository");
    git(root.path(), &["init", "-b", "main"]);
    git(root.path(), &["config", "user.name", "ZeroCode Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@zerocode.local"],
    );
    git(root.path(), &["config", "commit.gpgsign", "false"]);
    root
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message]);
}

/// A private root and store, as the runner insists on.
fn private_dir(root: &Path, name: &str) -> std::path::PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).expect("private directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private permissions");
    }
    path
}

fn runner(private: &Path) -> VerificationRunner {
    let command = VerificationCommand::new(
        COMMAND_ID,
        "/bin/sh",
        ["-c", "test -f fixed.txt"],
        Duration::from_secs(60),
    )
    .expect("verification command");
    let catalog = VerificationCatalog::new([command]).expect("catalog");
    VerificationRunner::open(private, catalog).expect("runner")
}

fn lineage(run: &str) -> HandoffLineage {
    HandoffLineage {
        run_id: run.to_string(),
        task_id: "task-1".to_string(),
        dispatch_id: "dispatch-1".to_string(),
        worker_id: "worker-1".to_string(),
        parent_manifest_id: None,
    }
}

/// What the gate asks for, and whether it insists on seeing the check fail
/// first.
fn requirement(must_fail_first: bool) -> TestRequirement {
    TestRequirement {
        name: TEST_NAME.to_string(),
        command_id: COMMAND_ID.to_string(),
        must_fail_first,
    }
}

/// Snapshot the working tree, run the catalogued check against that exact
/// commit, and hand back the receipt the runner produced.
fn verify(
    orchestrator: &Orchestrator,
    store: &WorkflowStore,
    runner: &VerificationRunner,
    root: &Path,
    run: &str,
    at_ms: i64,
) -> (TrustedTestEvidence, HandoffManifestV1) {
    let snapshot = orchestrator
        .handoff_snapshot(root, at_ms)
        .expect("handoff snapshot");
    let manifest = HandoffManifestV1::new(1, at_ms, lineage(run), snapshot, Vec::new())
        .expect("source manifest");
    let evidence = runner
        .run(orchestrator, store, &manifest, &requirement(false))
        .expect("verification runs");
    (evidence, manifest)
}

/// The manifest a worker would hand up: the same snapshot, carrying the
/// receipts named — its own, and any baseline receipt it kept.
fn handed_up(manifest: &HandoffManifestV1, receipts: Vec<TestReceipt>) -> HandoffManifestV1 {
    HandoffManifestV1::new(
        manifest.generation,
        manifest.created_at_ms,
        manifest.lineage.clone(),
        manifest.worktree.clone(),
        receipts,
    )
    .expect("handed-up manifest")
}

fn policy(must_fail_first: bool) -> PublishPolicy {
    PublishPolicy {
        required_tests: vec![requirement(must_fail_first)],
    }
}

#[test]
fn a_green_receipt_never_says_the_worker_changed_anything() {
    let workspace = tempfile::tempdir().expect("workspace");
    let private = private_dir(workspace.path(), "verification");
    let store_dir = private_dir(workspace.path(), "authority");
    let store = WorkflowStore::open(store_dir.join("workflow.sqlite")).expect("store");
    let runner = runner(&private);

    // ---------------------------------------------------------- an honest fix
    // The bug is present at base: the check fails. The worker adds the file,
    // and the check passes. Two receipts, one red and one green.
    let honest = repository();
    fs::write(honest.path().join("README.md"), "seed\n").expect("seed");
    commit(honest.path(), "seed");
    let honest_repo = Orchestrator::open(honest.path()).expect("open honest repository");

    let (before, _) = verify(
        &honest_repo,
        &store,
        &runner,
        honest.path(),
        "honest-before",
        1_000,
    );
    assert!(
        !before.succeeded(),
        "the check has to fail on the unfixed tree, or it is not a check"
    );
    assert_eq!(before.receipt().exit_code, 1);

    fs::write(honest.path().join("fixed.txt"), "fix\n").expect("fix");
    commit(honest.path(), "fix");
    let (after, after_manifest) = verify(
        &honest_repo,
        &store,
        &runner,
        honest.path(),
        "honest-after",
        2_000,
    );
    assert!(after.succeeded(), "the fix has to turn the check green");
    assert_ne!(
        before.source_oid(),
        after.source_oid(),
        "the two receipts are bound to different commits"
    );

    // ------------------------------------------------------- a run that did nothing
    // The same check, but the tree already satisfies it. Nobody fixes anything,
    // because there is nothing to fix. The check is green all the same.
    let idle = repository();
    fs::write(idle.path().join("README.md"), "seed\n").expect("seed");
    fs::write(idle.path().join("fixed.txt"), "fix\n").expect("already fixed");
    commit(idle.path(), "seed");
    let idle_repo = Orchestrator::open(idle.path()).expect("open idle repository");

    let (idle_evidence, idle_manifest) =
        verify(&idle_repo, &store, &runner, idle.path(), "idle", 3_000);
    assert!(
        idle_evidence.succeeded(),
        "a tree that was already green stays green when nobody touches it"
    );

    // ------------------------------------------------------------- the finding
    // Both manifests clear a gate that asks only for green. It reads one
    // receipt, on one commit, and asks whether it passed on the exact tree it
    // names — so the fix and the idle run are the same answer to it.
    let green_only = policy(false);
    assert!(
        handed_up(&after_manifest, vec![after.receipt().clone()])
            .structural_publish_blockers(&green_only)
            .is_empty(),
        "an honest fix publishes"
    );
    assert!(
        handed_up(&idle_manifest, vec![idle_evidence.receipt().clone()])
            .structural_publish_blockers(&green_only)
            .is_empty(),
        "a run that did nothing publishes on exactly the same evidence"
    );

    // ------------------------------------------------- and the rule that tells them apart
    // Asked to see the check fail first, the gate parts them: the honest fix
    // kept the red receipt its base commit produced, and the idle run has no
    // such receipt to keep because its check never failed anywhere.
    let strict = policy(true);
    assert!(
        handed_up(
            &after_manifest,
            vec![before.receipt().clone(), after.receipt().clone()]
        )
        .structural_publish_blockers(&strict)
        .is_empty(),
        "a fix carrying both halves publishes under the strict gate"
    );
    assert!(
        handed_up(&idle_manifest, vec![idle_evidence.receipt().clone()])
            .structural_publish_blockers(&strict)
            .contains(&PublishBlocker::NoBaselineFailure {
                name: TEST_NAME.to_string()
            }),
        "a run that did nothing is refused when the check must first fail"
    );
}

#[test]
fn a_check_that_cannot_fail_is_green_on_an_unfixed_tree() {
    let workspace = tempfile::tempdir().expect("workspace");
    let private = private_dir(workspace.path(), "verification");
    let store_dir = private_dir(workspace.path(), "authority");
    let store = WorkflowStore::open(store_dir.join("workflow.sqlite")).expect("store");

    // The worker was told to write a regression test first. It wrote one that
    // asserts nothing about the bug — `true` exits 0 anywhere.
    let vacuous = VerificationCommand::new(
        COMMAND_ID,
        "/bin/sh",
        ["-c", "true"],
        Duration::from_secs(60),
    )
    .expect("vacuous command");
    let runner = VerificationRunner::open(
        &private,
        VerificationCatalog::new([vacuous]).expect("catalog"),
    )
    .expect("runner");

    let repo = repository();
    fs::write(repo.path().join("README.md"), "seed\n").expect("seed");
    commit(repo.path(), "seed");
    let orchestrator = Orchestrator::open(repo.path()).expect("open repository");

    let (evidence, manifest) = verify(
        &orchestrator,
        &store,
        &runner,
        repo.path(),
        "vacuous",
        1_000,
    );

    assert!(
        evidence.succeeded(),
        "a check with no discrimination passes on the bug it was meant to catch"
    );
    assert!(
        handed_up(&manifest, vec![evidence.receipt().clone()])
            .structural_publish_blockers(&policy(false))
            .is_empty(),
        "and a gate that asks only for green publishes it"
    );
    // It can never produce the red half, so the strict gate stops it — which
    // is the whole point of asking for one.
    assert!(
        handed_up(&manifest, vec![evidence.receipt().clone()])
            .structural_publish_blockers(&policy(true))
            .contains(&PublishBlocker::NoBaselineFailure {
                name: TEST_NAME.to_string()
            }),
        "a check that cannot fail can never show the failure the gate asks for"
    );
}
