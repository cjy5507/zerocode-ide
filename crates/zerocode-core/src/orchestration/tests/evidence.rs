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
    let briefing = worker_briefing("t-1", "parser", TWO_CONDITIONS);
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
    let plain = worker_briefing("t-1", "rename", NO_CONDITIONS);
    assert!(
        !plain.contains("\"evidence\""),
        "a task with no conditions was briefed about evidence: {plain}"
    );
}

/// The briefing also says what a named job must do (review of t-26587): a job
/// named as a condition's evidence ends with a non-zero exit code when that
/// condition fails — a job that records its stages and then `exit 0` would
/// leave a receipt of 0 and a false "checked" on the board.
#[test]
fn a_briefing_says_an_evidence_job_must_end_non_zero_when_its_condition_fails() {
    let briefing = worker_briefing("t-1", "parser", TWO_CONDITIONS);
    assert!(
        briefing.contains("ends with a non-zero exit code when that condition fails"),
        "the briefing does not say the job must fail with its condition: {briefing}"
    );
    assert!(
        briefing.contains("never a closing `exit 0`"),
        "the briefing does not warn against a closing exit 0: {briefing}"
    );
}

/// A spec line is a condition when it begins with the mark, after its spaces
/// and an optional list marker, and the mark ends at a space, a colon or the
/// end of the line. `통과 전체` is not one.
#[test]
fn a_spec_line_is_a_condition_when_it_begins_with_the_mark_after_a_list_marker() {
    let spec = "Plan first.\n  통과 전 build is green  \n2. 통과 전: clippy is clean\n- 통과 전\n통과 전체 테스트는 아님\nnot 통과 전 here\n10) 통과 전 last";
    assert_eq!(
        crate::orchestration::evidence::conditions(spec),
        vec!["build is green", "clippy is clean", "", "last"]
    );
}

/// The board reads each condition from the last report that named the task:
/// passing evidence whose receipt the ledger read is `checked`, passing
/// evidence without one is `claimed`, and a condition with none is `missing`.
/// The notes the report left come along.
#[test]
fn the_board_reads_each_condition_from_the_last_report_that_named_the_task() {
    use crate::orchestration::evidence::{ConditionState, hand_in_of};
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let task = written_task(&mut bench, TWO_CONDITIONS);
    let pane = worker_on(&mut bench, &task);
    let body = serde_json::json!({"ok": true, "decisions": "keep the parser", "blocked": " ", "next": "review", "evidence": [
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
        "done-board",
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let run = &bench.ledger.runs()[0];
    let view = hand_in_of(run, run.task(&task).expect("the task"));
    let states: Vec<ConditionState> = view.conditions.iter().map(|row| row.state).collect();
    assert_eq!(
        states,
        vec![ConditionState::Checked, ConditionState::Claimed],
        "{view:?}"
    );
    assert_eq!(view.decisions.as_deref(), Some("keep the parser"));
    assert_eq!(view.blocked, None, "a blank note is no note");
    assert_eq!(view.next.as_deref(), Some("review"));
}

/// A task that writes no conditions has nothing on the board for its hand-in,
/// and a report with no evidence leaves every condition `missing`.
#[test]
fn a_task_without_conditions_has_no_hand_in_view_and_a_bare_report_leaves_every_condition_missing()
{
    use crate::orchestration::evidence::{ConditionState, hand_in_of};
    let mut bench = Bench::new();
    bench.json("run-create --name evidence");
    let plain = written_task(&mut bench, NO_CONDITIONS);
    let pane = worker_on(&mut bench, &plain);
    let planned = report(
        &mut bench,
        &pane,
        &serde_json::json!({"ok": true, "summary": "renamed"}),
        None,
        &[],
        "done-plain",
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let run = &bench.ledger.runs()[0];
    let view = hand_in_of(run, run.task(&plain).expect("the task"));
    assert!(view.is_empty(), "{view:?}");

    let tested = written_task(&mut bench, TWO_CONDITIONS);
    let carried = worker_on(&mut bench, &tested);
    let failed = report(
        &mut bench,
        &carried,
        &serde_json::json!({"ok": false, "summary": "not yet"}),
        None,
        &[],
        "done-open",
    );
    assert_eq!(failed.reply.exit_code, 0, "{}", failed.reply.stderr);
    let run = &bench.ledger.runs()[0];
    let view = hand_in_of(run, run.task(&tested).expect("the task"));
    let states: Vec<ConditionState> = view.conditions.iter().map(|row| row.state).collect();
    assert_eq!(
        states,
        vec![ConditionState::Missing, ConditionState::Missing]
    );
}

/// Numbers for the report (t-26587), printed as one line beginning `NUMBERS`:
/// how many success reports for tasks that write conditions the ledger takes
/// with no evidence, and how long each refusal takes, measured around the
/// report call in this test binary's build. Uses only the helpers the red set
/// already had, so the same test measures the ledger before the gate and after
/// it. Run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement: prints a NUMBERS line; run with --ignored --nocapture"]
fn the_numbers_of_the_gate_for_the_report() {
    const TASKS: usize = 12;
    let mut bench = Bench::new();
    bench.json("run-create --name numbers");
    let mut with_conditions = 0usize;
    let mut taken_bare = 0usize;
    let mut refusal_ms: Vec<f64> = Vec::new();
    for index in 0..TASKS {
        let writes_conditions = index % 2 == 0;
        let spec = if writes_conditions {
            TWO_CONDITIONS
        } else {
            NO_CONDITIONS
        };
        let task = written_task(&mut bench, spec);
        let pane = worker_on(&mut bench, &task);
        let body = serde_json::json!({"ok": true, "summary": "done"});
        let started = std::time::Instant::now();
        let planned = report(&mut bench, &pane, &body, None, &[], &format!("num-{index}"));
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if writes_conditions {
            with_conditions += 1;
            match planned.reply.exit_code {
                0 => taken_bare += 1,
                _ => refusal_ms.push(elapsed_ms),
            }
        }
    }
    let refused = refusal_ms.len();
    let mean_ms = refusal_ms.iter().sum::<f64>() / refused.max(1) as f64;
    let max_ms = refusal_ms.iter().copied().fold(0.0_f64, f64::max);
    eprintln!(
        "NUMBERS tasks={TASKS} with_conditions={with_conditions} taken_without_evidence={taken_bare} refused={refused} refusal_mean_ms={mean_ms:.3} refusal_max_ms={max_ms:.3}"
    );
}

/// The board's share of the numbers (t-26587), printed as one line beginning
/// `BOARD`: how many tasks show conditions in their hand-in view after a mix of
/// reports, and how many condition rows stand in each state. Run with
/// `--ignored --nocapture`.
#[test]
#[ignore = "measurement: prints a BOARD line; run with --ignored --nocapture"]
fn the_numbers_of_the_board_for_the_report() {
    use crate::orchestration::evidence::{ConditionState, hand_in_of};
    const TASKS: usize = 12;
    let mut bench = Bench::new();
    bench.json("run-create --name board-numbers");
    let mut tasks = Vec::new();
    for index in 0..TASKS {
        let writes_conditions = index % 2 == 0;
        let spec = if writes_conditions {
            TWO_CONDITIONS
        } else {
            NO_CONDITIONS
        };
        let task = written_task(&mut bench, spec);
        let pane = worker_on(&mut bench, &task);
        let (body, payload) = if writes_conditions {
            (
                serde_json::json!({"ok": true, "evidence": [
                    evidence_of(1, "tests", 0),
                    evidence_of(2, "clippy", 0),
                ]}),
                Some(serde_json::json!({"evidencePaths": [TESTS_RECEIPT]})),
            )
        } else {
            (serde_json::json!({"ok": true, "summary": "renamed"}), None)
        };
        let planned = report(
            &mut bench,
            &pane,
            &body,
            payload.as_ref(),
            &[(TESTS_RECEIPT, "0\n")],
            &format!("board-{index}"),
        );
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        tasks.push(task);
    }
    let run = &bench.ledger.runs()[0];
    let (mut rows, mut checked, mut claimed, mut missing) = (0usize, 0usize, 0usize, 0usize);
    for task in &tasks {
        let view = hand_in_of(run, run.task(task).expect("the task"));
        if view.conditions.is_empty() {
            continue;
        }
        rows += 1;
        for row in &view.conditions {
            match row.state {
                ConditionState::Checked => checked += 1,
                ConditionState::Claimed => claimed += 1,
                ConditionState::Missing => missing += 1,
            }
        }
    }
    eprintln!(
        "BOARD tasks={TASKS} rows_with_conditions={rows} checked={checked} claimed={claimed} missing={missing}"
    );
}
