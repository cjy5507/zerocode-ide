//! t-26587: a `worker_done` that says `ok: true` for a task whose spec writes
//! pass conditions (lines that begin `통과 전`) must show the evidence of each
//! one before the ledger takes it. Until now the verdict alone closed the task,
//! so a worker that reported before its build line had run was believed.
//!
//! The evidence of one condition is a build-line job's name, its exit code and
//! the numbers it produced. A receipt the report names beside the job
//! (`<job>.rc`) is read by the window before the ledger judges; a receipt that
//! says a different exit code refuses the report, because a receipt that says
//! otherwise is not a receipt.

use super::*;

/// A task with two pass conditions, written the way a coordinator writes them.
const TWO_CONDITIONS: &str = "Fix the parser.\n통과 전 cargo test -p zerocode-core is green\n통과 전 cargo clippy --all-targets -- -D warnings is clean";

/// A task that writes no condition lines: every task written before this change.
const NO_CONDITIONS: &str = "Rename the helper.";

/// The receipt a build line leaves beside its log, named the way a worker names it.
const TESTS_RECEIPT: &str = "/Users/dev/receipts/tests.rc";

/// One condition's evidence, as a worker writes it in the `evidence` list.
fn evidence_of(condition: u64, job: &str, rc: i64) -> serde_json::Value {
    serde_json::json!({
        "condition": condition,
        "job": job,
        "rc": rc,
        "numbers": "412 passed, 0 failed",
    })
}

/// The task the coordinator wrote, by its id.
fn written_task(bench: &mut Bench, spec: &str) -> String {
    let planned = bench.at_argv(
        agent_teams::LEADER_PANE,
        vec!["task-create".into(), "--spec".into(), spec.into()],
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let said: serde_json::Value =
        serde_json::from_str(&planned.reply.stdout).expect("task-create answers JSON");
    said["taskId"].as_str().expect("a task id").to_string()
}

/// One `worker_done` from a pane, with the argv built whole the way the shim
/// delivers it, so a body with spaces arrives intact.
fn report(
    bench: &mut Bench,
    pane: &str,
    body: &serde_json::Value,
    payload: Option<&serde_json::Value>,
    readings: &[(&str, &str)],
    retry: &str,
) -> Decided {
    let mut argv: Vec<String> = vec![
        "send".into(),
        "--type".into(),
        "worker_done".into(),
        "--body".into(),
        body.to_string(),
        "--retry-request".into(),
        retry.into(),
    ];
    if let Some(payload) = payload {
        argv.push("--payload".into());
        argv.push(payload.to_string());
    }
    bench.at_argv_reading(pane, argv, readings)
}

/// The pane of a worker summoned onto the task.
fn worker_on(bench: &mut Bench, task: &str) -> String {
    bench
        .seat(&format!("worker-start --agent claude --task {task}"))
        .1
}

/// No `worker_done` reached the coordinator's mail.
fn no_completion_was_mailed(bench: &Bench) -> bool {
    bench.ledger.runs()[0]
        .messages
        .iter()
        .all(|one| one.kind != MessageKind::WorkerDone)
}

/// A success report that shows no evidence at all is refused, and nothing
/// moves: the attempt stays open and the coordinator is not told it is done.
#[test]
fn a_success_report_with_no_evidence_is_refused_while_conditions_stand() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let planned = report(
        &mut bench,
        &pane,
        &serde_json::json!({"ok": true, "summary": "done"}),
        None,
        &[],
        "done-a",
    );
    assert_eq!(planned.reply.exit_code, 1, "{}", planned.reply.stdout);
    assert!(planned.receipt.is_none(), "a refused report left a receipt");
    let listed = bench.json("task-list");
    assert_eq!(
        listed["tasks"][0]["status"], "dispatched",
        "a refused report ended the attempt: {listed}"
    );
    assert!(
        no_completion_was_mailed(&bench),
        "a refused report reached the coordinator's mail"
    );
}

/// Evidence for one of two conditions is not enough, and the refusal says
/// which condition still has none.
#[test]
fn a_success_report_that_shows_one_condition_of_two_is_refused_and_names_the_other() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "evidence": [evidence_of(1, "tests", 0)]});
    let planned = report(&mut bench, &pane, &body, None, &[], "done-b");
    assert_eq!(planned.reply.exit_code, 1, "{}", planned.reply.stderr);
    assert!(
        planned.reply.stderr.contains("condition 2"),
        "the refusal does not name the condition with no evidence: {}",
        planned.reply.stderr
    );
    assert!(
        no_completion_was_mailed(&bench),
        "a refused report reached the coordinator's mail"
    );
}

/// A failure report is the worker saying the work is not done, so it needs no
/// evidence and is taken as it always was.
#[test]
fn a_failure_report_needs_no_evidence_for_its_conditions() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": false, "summary": "clippy still warns"});
    let planned = report(&mut bench, &pane, &body, None, &[], "done-c");
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    assert!(
        !no_completion_was_mailed(&bench),
        "a failure report was not filed"
    );
}

/// A task that writes no condition lines takes a success report exactly as it
/// did before this change.
#[test]
fn a_task_that_writes_no_conditions_takes_a_success_report_as_before() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, NO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "summary": "renamed"});
    let planned = report(&mut bench, &pane, &body, None, &[], "done-d");
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let listed = bench.json("task-list");
    assert_eq!(listed["tasks"][0]["status"], "completed", "{listed}");
}

/// A receipt the window read that agrees with the claimed exit code lets the
/// report through; the condition whose receipt was not named is claimed and
/// still accepted.
#[test]
fn a_report_whose_named_receipt_agrees_with_its_claim_is_taken() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "evidence": [
        evidence_of(1, "tests", 0),
        evidence_of(2, "clippy", 0),
    ]});
    let payload = serde_json::json!({"evidencePaths": [TESTS_RECEIPT]});
    let planned = report(
        &mut bench,
        &pane,
        &body,
        Some(&payload),
        &[(TESTS_RECEIPT, "0\n")],
        "done-f",
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let listed = bench.json("task-list");
    assert_eq!(listed["tasks"][0]["status"], "completed", "{listed}");
}

/// A receipt whose file says a different exit code refuses the report, and
/// the refusal names the job whose receipt disagrees.
#[test]
fn a_receipt_file_that_disagrees_with_the_claimed_exit_code_refuses_the_report() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "evidence": [
        evidence_of(1, "tests", 0),
        evidence_of(2, "clippy", 0),
    ]});
    let payload = serde_json::json!({"evidencePaths": [TESTS_RECEIPT]});
    let planned = report(
        &mut bench,
        &pane,
        &body,
        Some(&payload),
        &[(TESTS_RECEIPT, "1\n")],
        "done-e",
    );
    assert_eq!(planned.reply.exit_code, 1, "{}", planned.reply.stdout);
    assert!(
        planned.reply.stderr.contains("tests"),
        "the refusal does not name the job whose receipt disagrees: {}",
        planned.reply.stderr
    );
    assert!(
        no_completion_was_mailed(&bench),
        "a refused report reached the coordinator's mail"
    );
}

/// A receipt the report names that the window could not read is not a receipt
/// the ledger can agree with, so the report is refused.
#[test]
fn a_named_receipt_that_could_not_be_read_refuses_the_report() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "evidence": [
        evidence_of(1, "tests", 0),
        evidence_of(2, "clippy", 0),
    ]});
    let payload = serde_json::json!({"evidencePaths": [TESTS_RECEIPT]});
    let planned = report(&mut bench, &pane, &body, Some(&payload), &[], "done-g");
    assert_eq!(planned.reply.exit_code, 1, "{}", planned.reply.stdout);
    assert!(
        planned.reply.stderr.contains("tests.rc"),
        "the refusal does not name the receipt it could not read: {}",
        planned.reply.stderr
    );
}

/// A refused report left no receipt, so the worker can fix the evidence and
/// send the same `--retry-request` again, and the second send is taken.
#[test]
fn a_refused_report_can_be_sent_again_under_the_same_retry_name() {
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let partial = serde_json::json!({"ok": true, "evidence": [evidence_of(1, "tests", 0)]});
    let first = report(&mut bench, &pane, &partial, None, &[], "done-h");
    assert_eq!(first.reply.exit_code, 1, "{}", first.reply.stderr);
    assert!(first.receipt.is_none(), "a refused report left a receipt");
    let full = serde_json::json!({"ok": true, "evidence": [
        evidence_of(1, "tests", 0),
        evidence_of(2, "clippy", 0),
    ]});
    let second = report(&mut bench, &pane, &full, None, &[], "done-h");
    assert_eq!(second.reply.exit_code, 0, "{}", second.reply.stderr);
    let listed = bench.json("task-list");
    assert_eq!(listed["tasks"][0]["status"], "completed", "{listed}");
}

/// The briefing hands a worker the conditions its task must show, and the
/// shape the evidence takes; a task with no conditions gets neither.
#[test]
fn a_worker_briefing_for_a_task_with_conditions_names_them_and_the_evidence_shape() {
    let briefing = worker_briefing(TWO_CONDITIONS, "parser");
    for words in [
        "cargo test -p zerocode-core is green",
        "cargo clippy --all-targets -- -D warnings is clean",
        "\"evidence\"",
        "\"condition\"",
        "\"rc\"",
    ] {
        assert!(
            briefing.contains(words),
            "the briefing does not carry `{words}`: {briefing}"
        );
    }
    let plain = worker_briefing(NO_CONDITIONS, "rename");
    assert!(
        !plain.contains("\"evidence\""),
        "a task with no conditions was briefed about evidence: {plain}"
    );
}
