//! t-34501 stage 3 (t-42447): the window's landing-check evidence reaches the ledger through the
//! actor, and the same check id writes its letter once.

use super::*;
use zerocode_core::orchestration::land_check::LandCheckReceipt;

/// Plan one verb that creates something, and read back the id its answer names.
fn created(actor: &RuntimeActor, argv: &[&str], now_ms: i64, key: &str) -> String {
    let (decided, _) = actor
        .plan(a_command(argv, now_ms))
        .expect("the verb is planned");
    assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
    let said: serde_json::Value =
        serde_json::from_str(&decided.reply.stdout).expect("the answer is JSON");
    said[key]
        .as_str()
        .unwrap_or_else(|| panic!("no `{key}` in {}", decided.reply.stdout))
        .to_string()
}

fn evidence_of(run: &str, task: &str, check: &str) -> LandCheckReceipt {
    LandCheckReceipt {
        run: run.to_string(),
        task: task.to_string(),
        check: check.to_string(),
        evidence: r#"{"state":"passed","rc":0}"#.to_string(),
    }
}

/// The first evidence for a check id moves the ledger; the same id again writes nothing and moves
/// nothing — the ledger's revision stands still.
#[test]
fn a_check_letter_moves_the_ledger_once_per_check_id() {
    let fixture = Fixture::new();
    let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
    let run = created(
        &actor,
        &["run-create", "--name", "nightly", "--retry-request", "r-1"],
        20,
        "runId",
    );
    let task = created(
        &actor,
        &[
            "task-create",
            "--spec",
            "check-me",
            "--retry-request",
            "r-2",
        ],
        30,
        "taskId",
    );
    let before = actor.view().expect("an image").revision();
    let receipt = evidence_of(&run, &task, "lc-1700000000000-1");
    let (moved, after) = actor
        .land_checked(receipt.clone(), 40)
        .expect("the letter is written");
    assert!(moved, "the first evidence for a check id moves the ledger");
    assert!(after > before, "a letter is a durable verb");
    let (again, revision) = actor
        .land_checked(receipt, 41)
        .expect("the same check id is answered, not refused");
    assert!(!again, "the same check id writes no second letter");
    assert_eq!(revision, after, "a repeat moves no revision");
}

/// Evidence for a run the ledger does not hold is refused, not written to some other run.
#[test]
fn a_check_letter_for_a_run_the_ledger_does_not_hold_is_refused() {
    let fixture = Fixture::new();
    let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
    let run = created(
        &actor,
        &["run-create", "--name", "nightly", "--retry-request", "r-1"],
        20,
        "runId",
    );
    let task = created(
        &actor,
        &[
            "task-create",
            "--spec",
            "check-me",
            "--retry-request",
            "r-2",
        ],
        30,
        "taskId",
    );
    let stranger = format!("{run}-not-held");
    assert!(
        actor
            .land_checked(evidence_of(&stranger, &task, "lc-1700000000000-2"), 40)
            .is_err()
    );
}
