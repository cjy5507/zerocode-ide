//! What one checkout can prove, and what it must never be read as proving.
//!
//! Every case here stands on a real Git checkout, a real authority store and a
//! real rebuilt ledger, because the failures this read exists to prevent are
//! all failures of AGREEMENT between those three — a digest that moved, a
//! coordinator's note that looks like a test, a dispatch from the checkout
//! next door.

use std::path::Path;
use std::process::Command;

use rusqlite::params;
use tempfile::TempDir;
use zerocode_core::agent_teams::{LEADER_PANE, Team};
use zerocode_core::orchestration::{
    Ledger, LedgerProjectionV1, NoLauncher, ResultAuthor, ReviewAuthor, incarnation_actor, plan,
};
use zerocode_orchestrator::handoff::{HandoffLineage, HandoffManifestV1, TestReceipt};
use zerocode_orchestrator::workflow::{NewAssignment, WorkflowRecord};
use zerocode_orchestrator::workflow_store::{ReadOnlyWorkflows, WorkflowStore, WorkflowStoreError};
use zerocode_orchestrator::worktree_evidence::{
    Currency, EvidenceTarget, Freshness, GitSnapshotSource, LedgerSource, MAX_EXECUTIONS,
    ReceiptSource, SourceState, WorkflowSource, WorktreeEvidenceV1, read,
};
use zerocode_orchestrator::{GIT_EXECUTABLE, Orchestrator};

/* ---- a real repository ------------------------------------------------- */

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new(GIT_EXECUTABLE)
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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

/// A private directory for a store, made the way the window makes its vault —
/// beside the checkout and never inside it, so the store's own files never
/// become changes in the tree it is read against.
fn vault(root: &Path) -> std::path::PathBuf {
    let private = root.join("authority");
    std::fs::create_dir_all(&private).expect("private directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
            .expect("private permissions");
    }
    private.join("authority.sqlite")
}

/* ---- reading --------------------------------------------------------- */

struct Read {
    evidence: WorktreeEvidenceV1,
    json: String,
}

fn observe(
    checkout: &Path,
    ledger: Option<&Ledger>,
    store: Option<&Path>,
    observed_at_ms: i64,
) -> Read {
    let orchestrator = Orchestrator::open(checkout).ok();
    let evidence = read(
        &EvidenceTarget::local(checkout),
        &GitSnapshotSource::new(orchestrator.as_ref()),
        &LedgerSource::new(ledger),
        &WorkflowSource::new(store),
        observed_at_ms,
    );
    let json = serde_json::to_string(&evidence).expect("evidence serializes");
    Read { evidence, json }
}

/* ---- a ledger with rows in it ------------------------------------------ */

/// One run holding one worker seated in `checkout`, carrying one dispatch for
/// one task whose result is `result`.
///
/// Built through the projection the store actually persists and rebuilt by the
/// ledger's own loader, so a row this test writes is a row the product would
/// accept.
fn ledger_with(rows: serde_json::Value) -> Ledger {
    let projection: LedgerProjectionV1 =
        serde_json::from_value(rows).expect("the projection is well formed");
    Ledger::rebuild(projection).expect("the ledger accepts its own rows")
}

fn one_run(checkout: &str, result: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": zerocode_core::orchestration::PROJECTION_SCHEMA,
        "next_id": 9,
        "runs": [{ "id": "run-1", "name": "nightly", "created_ms": 10, "auto": null }],
        "tasks": [{
            "run": "run-1", "id": "t-1", "spec": "the whole instruction, in the coordinator's own words",
            "title": "make it work",
            "deps": [], "parent": null, "status": "completed", "result": result,
            "failures": 0, "created_ms": 11,
        }],
        "dispatches": [{
            "run": "run-1", "id": "d-1", "task": "t-1", "worker": "w-1",
            "started_ms": 12, "ended_ms": 30, "succeeded": true, "retry_of": "d-0",
        }],
        "workers": [{
            "run": "run-1", "id": "w-1", "team": TEAM, "agent": "codex", "pane": WORKER_PANE,
            "state": "released", "started_ms": 12, "dispatch": null,
            "checkout": checkout, "quiet_at": null, "archive": null,
        }],
        "messages": [], "inboxes": [], "bound": [], "served": [], "acked": [],
        "gates": [], "attachments": [],
    })
}

/// The window the fixture's run lives in: the leader's pane, and the pane
/// beside it that the worker was started in — never the leader's own, as no
/// worker the window starts is.
const TEAM: &str = "team-1";
const WORKER_PANE: &str = "%2";

/// The commit the worker's `worker_done` named, and so the source a review of
/// its attempt has to name.
const HANDED_IN: &str = "abc1234";

/// What the coordinator writes when it has looked.
const COORDINATOR_REVIEW: &str = r#"{"verified":true,"reviewedBy":"coordinator","merged":true}"#;

/// Every review key a worker could put in its own report, a forged author
/// included. The body is the worker's to send; none of it is the host's word
/// about who wrote it.
fn a_workers_review_keys() -> String {
    serde_json::json!({
        "ok": true,
        "head": HANDED_IN,
        "verified": true,
        "reviewedBy": "coordinator",
        "merged": true,
        "author": "coordinator",
        "resultAuthor": { "kind": "coordinator", "seat": format!("{TEAM}/{LEADER_PANE}") },
    })
    .to_string()
}

/// One verb through the planner the window runs, from `pane`, with the
/// receipt filed the way the window files it — so a row it leaves is a row
/// the ledger wrote, never one this test spelled. Answers what the verb
/// printed.
fn carry_out(ledger: &mut Ledger, pane: &str, argv: &[&str], now_ms: i64) -> serde_json::Value {
    let mut team = Team::new(TEAM, "token", 7);
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
    let actor = incarnation_actor("worktree-evidence", &format!("{TEAM}/{pane}"));
    let decided = plan(
        ledger,
        &mut team,
        &NoLauncher,
        &argv,
        pane,
        now_ms,
        Some(&actor),
    );
    assert_eq!(
        decided.reply.exit_code, 0,
        "`{argv:?}` was refused: {}",
        decided.reply.stderr
    );
    ledger.file_receipt(&decided, now_ms);
    serde_json::from_str(&decided.reply.stdout).expect("the verb answers JSON")
}

/// The fixture's run one step earlier — its worker still carrying the
/// attempt — and then the worker's own `worker_done`, naming the commit it
/// handed in and carrying every review key it could think of.
fn handed_in(checkout: &str) -> Ledger {
    let mut rows = one_run(checkout, "");
    rows["tasks"][0]["status"] = "dispatched".into();
    rows["dispatches"][0]["ended_ms"] = serde_json::Value::Null;
    rows["dispatches"][0]["succeeded"] = serde_json::Value::Null;
    rows["workers"][0]["state"] = "active".into();
    rows["workers"][0]["dispatch"] = "d-1".into();
    let mut ledger = ledger_with(rows);
    let body = a_workers_review_keys();
    carry_out(
        &mut ledger,
        WORKER_PANE,
        &[
            "send",
            "--run",
            "run-1",
            "--type",
            "worker_done",
            "--body",
            &body,
            "--retry-request",
            "hand-in",
        ],
        31,
    );
    ledger
}

/// The run's coordinator sits down in its leader pane and writes its review
/// through the one correction verb, naming the attempt and the source it
/// looked at.
fn reviewed_by_the_coordinator(ledger: &mut Ledger, attempt: &str, source: &str) {
    let seated = carry_out(
        ledger,
        LEADER_PANE,
        &["run-use", "run-1", "--retry-request", "sit"],
        40,
    );
    assert_eq!(seated["seated"], true, "{seated}");
    let corrected = carry_out(
        ledger,
        LEADER_PANE,
        &[
            "task-update",
            "--run",
            "run-1",
            "--task",
            "t-1",
            "--result",
            COORDINATOR_REVIEW,
            "--attempt",
            attempt,
            "--source",
            source,
            "--retry-request",
            "review",
        ],
        41,
    );
    assert_eq!(corrected["author"], "coordinator", "{corrected}");
}

/// The same ledger, stored and read back with one row changed through the
/// projection the store persists — so the change is one its loader accepts.
fn rebuilt_with(ledger: &Ledger, change: impl FnOnce(&mut serde_json::Value)) -> Ledger {
    let mut rows = serde_json::to_value(ledger.export()).expect("the projection serializes");
    change(&mut rows);
    ledger_with(rows)
}

/// What a read that believed nothing it should not has to show. The task is
/// still listed — `coordinator_report` is the reports section's fixed kind and
/// the summary counts its rows — but none of its review facts is on, and no
/// test receipt came out of it.
fn assert_no_review_facts(read: &Read) {
    let report = &read.evidence.reports.data[0];
    assert_eq!(report.kind, "coordinator_report");
    assert!(
        !report.verified
            && !report.merged
            && !report.deployed
            && !report.written
            && report.merge_head.is_none(),
        "a review nobody may stand behind was read as the coordinator's facts: {}",
        read.json
    );
    assert_eq!(read.evidence.verification.data.len(), 0);
    assert_eq!(read.evidence.verification.state, SourceState::Missing);
    assert_eq!(read.evidence.summary.receipts_current, 0);
    assert_eq!(read.evidence.summary.receipts_current_passing, 0);
}

/* ---- a store with rows in it ------------------------------------------- */

fn assignment(id: &str, repository_id: &str, worktree_id: &str, base: &str) -> NewAssignment {
    NewAssignment {
        workflow_id: id.to_string(),
        assignee_id: "worker-alice".to_string(),
        repository_id: repository_id.to_string(),
        worktree_id: worktree_id.to_string(),
        publisher_scope: "origin:test/repo".to_string(),
        base_oid: base.to_string(),
        source_ref: "refs/heads/main".to_string(),
        target_ref: "refs/zerocode/integration/main".to_string(),
        policy: zerocode_orchestrator::handoff::PublishPolicy::default(),
    }
}

/// A receipt the host-trusted verifier would have written, put where it writes
/// it. The digests are shaped as the table's own CHECKs demand; what this test
/// exercises is the READ, and the writer has its own battery.
fn record_trusted_receipt(
    store: &WorkflowStore,
    manifest_id: &str,
    snapshot_digest: &str,
    source_oid: &str,
    exit_code: i32,
) {
    let connection = store
        .fault_connection_for_tests()
        .expect("a connection to seed with");
    let digest = |seed: &str| {
        use sha2::{Digest as _, Sha256};
        format!("{:x}", Sha256::digest(seed.as_bytes()))
    };
    connection
        .execute(
            "INSERT INTO trusted_test_evidence (
                manifest_id, test_name, command_id, snapshot_digest, source_oid,
                started_at_ms, ended_at_ms, exit_code, succeeded, command_digest,
                stdout_digest, stderr_digest, evidence_digest, output_artifact_id,
                output_truncated
             ) VALUES (?1, 'gate', 'just-verify', ?2, ?3, 100, 200, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
            params![
                manifest_id,
                snapshot_digest,
                source_oid,
                i64::from(exit_code),
                i64::from(exit_code == 0),
                digest("command"),
                digest("stdout"),
                digest("stderr"),
                digest(manifest_id),
                format!("test-evidence-{}", digest(manifest_id)),
            ],
        )
        .expect("seed one trusted receipt");
}

fn submitted(
    store: &WorkflowStore,
    orchestrator: &Orchestrator,
    checkout: &Path,
    id: &str,
    tests: Vec<TestReceipt>,
) -> (WorkflowRecord, HandoffManifestV1) {
    let snapshot = orchestrator
        .handoff_snapshot(checkout, 1)
        .expect("a snapshot to bind to");
    let base = git(checkout, &["rev-parse", "HEAD"]);
    let assigned = store
        .assign(&assignment(
            id,
            &snapshot.repository_id,
            &snapshot.worktree_id,
            &base,
        ))
        .expect("assign");
    let manifest = HandoffManifestV1::new(
        assigned.generation,
        1,
        HandoffLineage {
            run_id: "run-1".to_string(),
            task_id: id.to_string(),
            dispatch_id: "d-1".to_string(),
            worker_id: "worker-alice".to_string(),
            parent_manifest_id: None,
        },
        snapshot,
        tests,
    )
    .expect("manifest");
    let record = store
        .submit(
            id,
            assigned.generation,
            &manifest,
            &zerocode_orchestrator::handoff::PublishPolicy::default(),
        )
        .expect("submit");
    (record, manifest)
}

/* ---- the rules --------------------------------------------------------- */

/// A receipt is current only while the digest it was taken against is the one
/// observed NOW. The head alone is not enough: the checkout below never moves
/// its commit, and changing one uncommitted byte is still a different tree.
#[test]
fn a_receipt_taken_against_other_bytes_is_stale_even_on_the_same_head() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    let store = WorkflowStore::open(&store_path).expect("store");
    let orchestrator = Orchestrator::open(root.path()).expect("repository");

    let before = orchestrator
        .handoff_snapshot(root.path(), 1)
        .expect("first observation");
    let head = before.head_oid.clone();
    let (_, manifest) = submitted(&store, &orchestrator, root.path(), "wf-1", Vec::new());
    record_trusted_receipt(
        &store,
        &manifest.manifest_id,
        &before.content_digest,
        &head,
        0,
    );

    let current = observe(root.path(), None, Some(&store_path), 2);
    let receipt = current
        .evidence
        .verification
        .data
        .iter()
        .find(|row| row.source == ReceiptSource::HostTrusted)
        .expect("the trusted receipt is read");
    assert_eq!(receipt.currency, Currency::Current, "{}", current.json);
    assert_eq!(receipt.currency_reason, "digest_matches");
    assert_eq!(current.evidence.summary.receipts_current_passing, 1);

    // One uncommitted byte. HEAD has not moved and the tree has.
    std::fs::write(root.path().join("tracked.txt"), "two\n").expect("edit");
    let moved = observe(root.path(), None, Some(&store_path), 3);
    assert_eq!(
        moved
            .evidence
            .snapshot
            .data
            .as_ref()
            .expect("git answered")
            .head_oid,
        head,
        "the head must not have moved for this case to mean anything"
    );
    let receipt = moved
        .evidence
        .verification
        .data
        .iter()
        .find(|row| row.source == ReceiptSource::HostTrusted)
        .expect("the receipt is still stored");
    assert_eq!(receipt.currency, Currency::Stale, "{}", moved.json);
    assert_eq!(receipt.currency_reason, "content_changed");
    assert_eq!(moved.evidence.summary.receipts_current_passing, 0);
    assert_eq!(moved.evidence.summary.receipts_stale, 1);
}

/// An observation that did not cover everything cannot promote anything to
/// current — the bytes it did not hash are exactly the ones a person is most
/// likely to be working in.
#[test]
fn an_observation_with_gaps_answers_unknown_and_never_current() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    let store = WorkflowStore::open(&store_path).expect("store");
    let orchestrator = Orchestrator::open(root.path()).expect("repository");
    let before = orchestrator
        .handoff_snapshot(root.path(), 1)
        .expect("observation");
    let head = before.head_oid.clone();
    let (_, manifest) = submitted(&store, &orchestrator, root.path(), "wf-1", Vec::new());
    record_trusted_receipt(
        &store,
        &manifest.manifest_id,
        &before.content_digest,
        &head,
        0,
    );

    // An untracked file the digest does not cover. The digest itself is
    // unchanged, so only the coverage gap can save this answer.
    std::fs::write(root.path().join("scratch.tmp"), "draft\n").expect("untracked file");
    let read = observe(root.path(), None, Some(&store_path), 2);
    let facts = read.evidence.snapshot.data.as_ref().expect("git answered");
    assert!(!facts.complete, "{}", read.json);
    assert!(!facts.coverage_gaps.is_empty());
    let receipt = &read.evidence.verification.data[0];
    assert_eq!(receipt.currency, Currency::Unknown, "{}", read.json);
    assert_eq!(receipt.currency_reason, "incomplete_observation");
    assert_eq!(read.evidence.summary.receipts_current_passing, 0);
    assert_eq!(read.evidence.summary.receipts_unknown, 1);
}

/// `verified: true` on a task result is a COORDINATOR saying it looked — the
/// run's live seat, through `task-update`, naming the attempt and the source
/// it reviewed. It is carried, labelled, and never counted among the test
/// receipts; and it survives the store and the rebuild as the same fact.
#[test]
fn a_coordinator_report_is_labelled_and_never_counted_as_a_test() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let mut ledger = handed_in(&checkout);
    reviewed_by_the_coordinator(&mut ledger, "d-1", HANDED_IN);
    // The author is the one the ledger wrote down from the seat, bound to
    // what the review named — not a word in anybody's body.
    assert!(
        matches!(
            &ledger.runs()[0].tasks[0].result_author,
            Some(ResultAuthor::Coordinator {
                generation: Some(_),
                attempt: Some(attempt),
                source: Some(source),
                ..
            }) if attempt == "d-1" && source == HANDED_IN
        ),
        "{:?}",
        ledger.runs()[0].tasks[0].result_author
    );
    let rebuilt = rebuilt_with(&ledger, |_| {});

    for ledger in [&ledger, &rebuilt] {
        let read = observe(root.path(), Some(ledger), None, 5);

        let report = &read.evidence.reports.data[0];
        assert_eq!(report.kind, "coordinator_report");
        assert!(
            report.verified && report.merged && report.written,
            "{}",
            read.json
        );
        assert_eq!(read.evidence.summary.coordinator_reports, 1);

        // And nothing reached the receipts.
        assert_eq!(read.evidence.verification.data.len(), 0);
        assert_eq!(read.evidence.summary.receipts_current_passing, 0);
        assert_eq!(read.evidence.summary.receipts_current, 0);
        // The store was never offered, so its sections say so rather than
        // "none".
        assert_eq!(read.evidence.verification.state, SourceState::Missing);
        assert_eq!(read.evidence.verification.freshness, Freshness::Unobserved);
    }
}

/// A row written before authorship was recorded is a claim by nobody known,
/// whatever its body says about who reviewed it (t-6815).
#[test]
fn a_result_nobody_is_known_to_have_written_is_not_a_coordinator_report() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let ledger = ledger_with(one_run(&checkout, &a_workers_review_keys()));
    assert_eq!(ledger.runs()[0].tasks[0].result_author, None);

    let read = observe(root.path(), Some(&ledger), None, 5);
    assert_no_review_facts(&read);
}

/// The worker's own `worker_done` — `reviewedBy: "coordinator"`, a forged
/// author and all — is its claim. The one word of it this read carries is
/// the attempt's own success, where it has always been: on the execution.
#[test]
fn a_workers_own_review_keys_are_not_a_coordinator_report() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let ledger = handed_in(&checkout);
    assert_eq!(
        ledger.runs()[0].tasks[0].result_author,
        Some(ResultAuthor::Worker {
            worker: "w-1".to_string(),
            dispatch: Some("d-1".to_string()),
        }),
        "the host wrote down the pane that reported, not the body's author"
    );

    let read = observe(root.path(), Some(&ledger), None, 5);
    assert_no_review_facts(&read);
    let execution = &read.evidence.executions.data[0];
    assert_eq!(execution.dispatch_id, "d-1");
    assert_eq!(execution.worker_reported_success, Some(true));
}

/// A coordinator's review is of the attempt it named. A retry of the same
/// task — handing in the very same commit — inherits none of it; the reader
/// keeps who wrote it and names the attempt that moved past it.
#[test]
fn a_review_of_an_earlier_attempt_is_not_a_report_on_the_next() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let mut reviewed = handed_in(&checkout);
    reviewed_by_the_coordinator(&mut reviewed, "d-1", HANDED_IN);
    let ledger = rebuilt_with(&reviewed, |rows| {
        let dispatches = rows["dispatches"].as_array_mut().expect("the attempts");
        let mut retry = dispatches[0].clone();
        retry["id"] = "d-2".into();
        retry["retry_of"] = "d-1".into();
        retry["started_ms"] = 50.into();
        retry["ended_ms"] = 60.into();
        dispatches.push(retry);
    });

    let read = observe(root.path(), Some(&ledger), None, 5);
    assert_no_review_facts(&read);

    let run = &ledger.runs()[0];
    let facts = run.review_of(&run.tasks[0]);
    assert_eq!(facts.author, ReviewAuthor::Coordinator, "{facts:?}");
    assert_eq!(facts.attempt.as_deref(), Some("d-1"), "{facts:?}");
    assert_eq!(facts.superseded_by.as_deref(), Some("d-2"), "{facts:?}");
}

/// A coordinator's review is of the source it named. The same attempt
/// standing on another hand-in than the one reviewed is not reported on;
/// the reader keeps the source it named beside the one handed in now.
#[test]
fn a_review_of_another_hand_in_is_not_a_report_on_this_one() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let mut reviewed = handed_in(&checkout);
    reviewed_by_the_coordinator(&mut reviewed, "d-1", HANDED_IN);
    let other = "def5678";
    let ledger = rebuilt_with(&reviewed, |rows| {
        rows["dispatches"][0]["source"] = other.into();
    });

    let read = observe(root.path(), Some(&ledger), None, 5);
    assert_no_review_facts(&read);

    let run = &ledger.runs()[0];
    let facts = run.review_of(&run.tasks[0]);
    assert_eq!(facts.author, ReviewAuthor::Coordinator, "{facts:?}");
    assert_eq!(facts.source.as_deref(), Some(HANDED_IN), "{facts:?}");
    assert_eq!(facts.source_now.as_deref(), Some(other), "{facts:?}");
    assert_eq!(facts.superseded_by, None, "{facts:?}");
}

/// Two checkouts of one repository are two checkouts. A dispatch that ran next
/// door is not this tree's evidence, however alike the two look.
#[test]
fn only_the_dispatches_seated_in_this_checkout_are_listed() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the second checkout");
    let neighbour = beside.path().join("neighbour");
    git(
        root.path(),
        &[
            "worktree",
            "add",
            "-b",
            "side",
            neighbour.to_str().expect("utf-8 path"),
        ],
    );
    let ledger = ledger_with(one_run(&neighbour.to_string_lossy(), "{}"));

    let here = observe(root.path(), Some(&ledger), None, 5);
    assert_eq!(here.evidence.executions.data.len(), 0, "{}", here.json);
    assert_eq!(here.evidence.executions.state, SourceState::Empty);
    assert_eq!(
        here.evidence.executions.freshness,
        Freshness::Observed,
        "the ledger WAS read; it simply holds nothing for this tree"
    );

    let there = observe(&neighbour, Some(&ledger), None, 5);
    let execution = &there.evidence.executions.data[0];
    assert_eq!(execution.dispatch_id, "d-1");
    assert_eq!(execution.agent, "codex");
    assert_eq!(
        execution.retry_of.as_deref(),
        Some("d-0"),
        "the retry lineage survives the read"
    );
    assert_eq!(execution.worker_reported_success, Some(true));
    assert!(!execution.open);
}

/// Four absences, four answers. A window with no ledger has not found an empty
/// ledger, and a checkout on another host has not been found clean.
#[test]
fn absence_is_told_apart_from_emptiness_and_from_refusal() {
    let root = repository();
    let nothing = observe(root.path(), None, None, 5);
    assert_eq!(nothing.evidence.executions.state, SourceState::Missing);
    assert_eq!(nothing.evidence.decisions.state, SourceState::Missing);
    assert_eq!(nothing.evidence.ci.state, SourceState::Unsupported);

    let empty = ledger_with(serde_json::json!({
        "schema": zerocode_core::orchestration::PROJECTION_SCHEMA,
        "next_id": 1, "runs": [], "tasks": [], "dispatches": [], "workers": [],
        "messages": [], "inboxes": [], "bound": [], "served": [], "acked": [],
        "gates": [], "attachments": [],
    }));
    let read = observe(root.path(), Some(&empty), None, 5);
    assert_eq!(read.evidence.executions.state, SourceState::Empty);
    assert_eq!(read.evidence.executions.freshness, Freshness::Observed);

    // A directory that is not a checkout at all: git has nothing to say, and
    // the section says it could not be read rather than that it was clean.
    let bare = tempfile::tempdir().expect("a plain directory");
    let read = observe(bare.path(), Some(&empty), None, 5);
    assert_eq!(read.evidence.snapshot.state, SourceState::Missing);
    assert!(read.evidence.snapshot.data.is_none());
    assert_eq!(read.evidence.summary.dirty, None);
}

/// A read never creates the authority store, and never migrates one.
#[test]
fn the_read_makes_no_store_and_refuses_one_it_cannot_read() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the store");
    let absent = vault(beside.path());
    let read = observe(root.path(), None, Some(&absent), 5);
    assert!(!absent.exists(), "a read must not make an authority store");
    assert_eq!(read.evidence.verification.state, SourceState::Missing);
    assert_eq!(read.evidence.decisions.state, SourceState::Missing);

    // A file at the store's place written by something else stays refused
    // rather than being upgraded into a store.
    let foreign = beside.path().join("authority").join("foreign.sqlite");
    let connection = rusqlite::Connection::open(&foreign).expect("a foreign database");
    connection
        .pragma_update(None, "user_version", 4_i64)
        .expect("an older version");
    drop(connection);
    assert!(matches!(
        ReadOnlyWorkflows::open(&foreign),
        Err(WorkflowStoreError::UnsupportedSchema { found: 4 })
    ));
    let read = observe(root.path(), None, Some(&foreign), 5);
    assert_eq!(read.evidence.verification.state, SourceState::Error);
    assert_eq!(
        read.evidence
            .verification
            .error
            .as_ref()
            .expect("a reason")
            .code,
        "store_schema_unsupported"
    );
    assert_eq!(read.evidence.decisions.state, SourceState::Error);
}

/// The answer names what nothing records, so its absence cannot be read as a
/// negative, and it never carries a local path.
#[test]
fn the_answer_names_what_is_unrecorded_and_carries_no_local_path() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    let store = WorkflowStore::open(&store_path).expect("store");
    let orchestrator = Orchestrator::open(root.path()).expect("repository");
    submitted(&store, &orchestrator, root.path(), "wf-1", Vec::new());
    let ledger = ledger_with(one_run(&checkout, r#"{"verified":true}"#));

    let read = observe(root.path(), Some(&ledger), Some(&store_path), 5);
    assert_eq!(
        read.evidence.summary.unrecorded,
        vec!["tool_approvals", "ci_checks"]
    );
    assert_eq!(read.evidence.ci.state, SourceState::Unsupported);

    for private in [
        checkout.as_str(),
        store_path.to_string_lossy().as_ref(),
        // The task's spec is the coordinator's instruction. The row carries
        // the title beside it; the instruction itself stays out.
        "the whole instruction",
    ] {
        assert!(
            !read.json.contains(private),
            "the answer leaked `{private}`:\n{}",
            read.json
        );
    }
    // What it DOES carry is opaque and machine-local.
    assert!(read.evidence.workspace_id.starts_with("workspace-"));
    assert!(read.evidence.repository_id.is_some());
}

/// A bound that took rows says so. Silence about truncation is the one thing a
/// bounded answer must never do.
#[test]
fn a_bound_that_took_rows_reports_the_count_it_did_not_carry() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let over = MAX_EXECUTIONS + 3;
    let mut dispatches = Vec::new();
    for at in 0..over {
        let carried = at == over - 1;
        dispatches.push(serde_json::json!({
            "run": "run-1", "id": format!("d-{at}"), "task": "t-1", "worker": "w-1",
            "started_ms": 12 + at as i64,
            "ended_ms": if carried { serde_json::Value::Null } else { (30 + at as i64).into() },
            "succeeded": if carried { serde_json::Value::Null } else { true.into() },
        }));
    }
    let ledger = ledger_with(serde_json::json!({
        "schema": zerocode_core::orchestration::PROJECTION_SCHEMA,
        "next_id": 999,
        "runs": [{ "id": "run-1", "name": "nightly", "created_ms": 10, "auto": null }],
        "tasks": [{
            "run": "run-1", "id": "t-1", "spec": "work", "title": "work", "deps": [],
            "parent": null, "status": "dispatched", "result": "", "failures": 0,
            "created_ms": 11,
        }],
        "dispatches": dispatches,
        "workers": [{
            "run": "run-1", "id": "w-1", "team": "team-1", "agent": "codex", "pane": "%1",
            "state": "active", "started_ms": 12, "dispatch": format!("d-{}", over - 1),
            "checkout": checkout, "quiet_at": null, "archive": null,
        }],
        "messages": [], "inboxes": [], "bound": [], "served": [], "acked": [],
        "gates": [], "attachments": [],
    }));

    let read = observe(root.path(), Some(&ledger), None, 5);
    assert_eq!(read.evidence.executions.coverage.counted, over);
    assert_eq!(read.evidence.executions.coverage.returned, MAX_EXECUTIONS);
    assert!(read.evidence.executions.coverage.truncated);
    assert_eq!(read.evidence.executions.data.len(), MAX_EXECUTIONS);
    // The summary counts what EXISTS, not what fitted.
    assert_eq!(read.evidence.summary.executions, over);
}

/// The manifest's own receipts are a different source kind from the ones the
/// host-trusted runner filed, and both are judged against this observation.
#[test]
fn manifest_receipts_and_host_trusted_receipts_stay_apart() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    let store = WorkflowStore::open(&store_path).expect("store");
    let orchestrator = Orchestrator::open(root.path()).expect("repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1)
        .expect("observation");
    let carried = TestReceipt::new(
        "unit",
        "just-test",
        snapshot.content_digest.clone(),
        10,
        20,
        0,
    );
    let (_, manifest) = submitted(&store, &orchestrator, root.path(), "wf-1", vec![carried]);
    record_trusted_receipt(
        &store,
        &manifest.manifest_id,
        &snapshot.content_digest,
        &snapshot.head_oid,
        1,
    );

    let read = observe(root.path(), None, Some(&store_path), 2);
    let kinds: Vec<_> = read
        .evidence
        .verification
        .data
        .iter()
        .map(|row| (row.source, row.name.as_str(), row.passed, row.currency))
        .collect();
    assert!(
        kinds.contains(&(ReceiptSource::Manifest, "unit", true, Currency::Current)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(ReceiptSource::HostTrusted, "gate", false, Currency::Current)),
        "{kinds:?}"
    );
    // A failing receipt is current and still not a pass.
    assert_eq!(read.evidence.summary.receipts_current, 2);
    assert_eq!(read.evidence.summary.receipts_current_passing, 1);
}

/// A tree Git cannot identify is never looked up in the store, and a tree on
/// another host is not looked for on this disk at all: every section that
/// depends on either says `unsupported`, never `empty`.
#[test]
fn an_unidentifiable_or_remote_tree_is_unsupported_and_never_empty() {
    let root = repository();
    let checkout = root.path().to_string_lossy().into_owned();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    WorkflowStore::open(&store_path).expect("store");
    let ledger = ledger_with(one_run(&checkout, r#"{"verified":true}"#));

    // A plain folder: no repository, so no identity to key the store by.
    let folder = tempfile::tempdir().expect("a plain folder");
    let plain = observe(folder.path(), Some(&ledger), Some(&store_path), 5);
    assert_eq!(plain.evidence.snapshot.state, SourceState::Missing);
    assert_eq!(plain.evidence.verification.state, SourceState::Unsupported);
    assert_eq!(plain.evidence.decisions.state, SourceState::Unsupported);
    // The ledger WAS read for it, and holds nothing seated there.
    assert_eq!(plain.evidence.executions.state, SourceState::Empty);

    // Another host's tree, even one whose path spells this checkout's.
    let remote = read(
        &EvidenceTarget::remote(&format!("edge-ssh:{checkout}")),
        &GitSnapshotSource::new(Orchestrator::open(root.path()).ok().as_ref()),
        &LedgerSource::new(Some(&ledger)),
        &WorkflowSource::new(Some(&store_path)),
        5,
    );
    for (name, state) in [
        ("snapshot", remote.snapshot.state),
        ("executions", remote.executions.state),
        ("reports", remote.reports.state),
        ("verification", remote.verification.state),
        ("decisions", remote.decisions.state),
        ("ci", remote.ci.state),
    ] {
        assert_eq!(state, SourceState::Unsupported, "{name}");
    }
    assert!(remote.repository_id.is_none());
    assert_eq!(remote.summary.executions, 0);
}

/// A receipt table that cannot be read after the reviews were is news about
/// the receipts only: rows already in hand travel with the refusal beside
/// them, an empty hand is an error rather than "no test ever ran", and the
/// decisions section is not told about it at all.
#[test]
fn a_receipt_read_that_fails_after_the_reviews_is_news_for_receipts_only() {
    let root = repository();
    let beside = tempfile::tempdir().expect("a place for the store");
    let store_path = vault(beside.path());
    let store = WorkflowStore::open(&store_path).expect("store");
    let orchestrator = Orchestrator::open(root.path()).expect("repository");
    let snapshot = orchestrator
        .handoff_snapshot(root.path(), 1)
        .expect("observation");
    let carried = TestReceipt::new(
        "unit",
        "just-test",
        snapshot.content_digest.clone(),
        10,
        20,
        0,
    );
    submitted(&store, &orchestrator, root.path(), "wf-1", vec![carried]);
    // The trusted receipts' table goes missing under a store whose workflows
    // are still readable.
    store
        .fault_connection_for_tests()
        .expect("a connection to break the table with")
        .execute_batch("ALTER TABLE trusted_test_evidence RENAME TO trusted_test_evidence_gone;")
        .expect("rename the receipts table");

    let with_rows = observe(root.path(), None, Some(&store_path), 2);
    let verification = &with_rows.evidence.verification;
    assert_eq!(verification.state, SourceState::Ok, "{}", with_rows.json);
    assert_eq!(
        verification.data.len(),
        1,
        "the manifest receipt still travels"
    );
    assert_eq!(verification.data[0].source, ReceiptSource::Manifest);
    assert_eq!(
        verification.error.as_ref().map(|refusal| refusal.code),
        Some("store_unavailable")
    );
    assert!(
        with_rows.evidence.decisions.error.is_none(),
        "a receipt refusal is not news for decisions: {}",
        with_rows.json
    );
    assert_eq!(with_rows.evidence.decisions.state, SourceState::Empty);

    // The same refusal with nothing in hand is an error, never an empty.
    let other = repository();
    let other_orchestrator = Orchestrator::open(other.path()).expect("second repository");
    submitted(
        &store,
        &other_orchestrator,
        other.path(),
        "wf-2",
        Vec::new(),
    );
    let empty_hand = observe(other.path(), None, Some(&store_path), 3);
    assert_eq!(
        empty_hand.evidence.verification.state,
        SourceState::Error,
        "{}",
        empty_hand.json
    );
    assert_eq!(empty_hand.evidence.summary.receipts_current_passing, 0);
}
