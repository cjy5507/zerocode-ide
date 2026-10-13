//! t-47835: where an attempt's time went, read off the ledger's own records
//! (`attempt_time.rs`). Each run is written out whole, so every moment in it
//! is chosen; every closed attempt's pieces must add up to its own wall clock.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::orchestration::attempt_time::{
    AttemptTime, Hold, HoldEnd, Piece, TaskTime, attempt_time, task_time,
};

const MINUTE: i64 = 60_000;
const HEAD: &str = "abc1234def5678";

fn attempt(id: &str, started: i64, ended: Option<i64>, succeeded: Option<bool>) -> Value {
    json!({
        "id": id, "task": "t-1", "worker": format!("w-{id}"), "started_ms": started,
        "ended_ms": ended, "succeeded": succeeded,
    })
}

fn message(id: &str, kind: &str, dispatch: Option<&str>, at: i64) -> Value {
    json!({
        "id": id, "from": "worker:w-dp-1", "to": "run:run-time", "kind": kind, "body": "{}",
        "thread": null, "task": "t-1", "dispatch": dispatch, "created_ms": at,
    })
}

fn question(id: &str, dispatch: &str, at: i64) -> Value {
    json!({
        "id": id, "from": format!("worker:w-{dispatch}"), "to": "run:run-time", "kind": "question",
        "body": "which?", "thread": null, "task": "t-1", "dispatch": dispatch, "created_ms": at,
    })
}

fn answer(id: &str, to_question: &str, at: i64) -> Value {
    json!({
        "id": id, "from": "run:run-time", "to": "worker:w-dp-1", "kind": "question",
        "body": "this", "thread": to_question, "task": null, "dispatch": null, "created_ms": at,
    })
}

fn done(id: &str, dispatch: &str, head: &str, at: i64) -> Value {
    json!({
        "id": id, "from": format!("worker:w-{dispatch}"), "to": "run:run-time", "kind": "worker_done",
        "body": json!({"ok": true, "head": head}).to_string(), "thread": null, "task": "t-1",
        "dispatch": dispatch, "created_ms": at,
    })
}

fn gate(id: &str, created: i64, resolved: Option<i64>) -> Value {
    let status = if resolved.is_some() {
        "resolved"
    } else {
        "pending"
    };
    json!({
        "id": id, "task": "t-1", "question": "go?", "options": [], "status": status,
        "resolution": "", "created_ms": created, "resolved_ms": resolved,
    })
}

fn reviewed(attempt: &str, source: &str, verified_ms: i64) -> Value {
    json!({
        "result": json!({"verified": true}).to_string(),
        "result_author": {
            "kind": "coordinator", "seat": "team/%1", "attempt": attempt, "source": source,
            "verified_ms": verified_ms,
        },
    })
}

fn run_of(dispatches: Value, messages: Value, gates: Value, review: Value) -> Run {
    let mut task = json!({
        "id": "t-1", "spec": "time", "title": "", "deps": [], "parent": null,
        "status": "completed", "result": "", "failures": 0, "created_ms": 0,
    });
    if let Value::Object(fields) = review {
        task.as_object_mut().expect("a task").extend(fields);
    }
    serde_json::from_value(json!({
        "id": "run-time", "name": "time", "created_ms": 0, "tasks": [task],
        "dispatches": dispatches, "workers": [], "messages": messages, "gates": gates,
        "inboxes": [],
    }))
    .expect("a run")
}

fn timed(run: &Run, id: &str) -> AttemptTime {
    attempt_time(
        run,
        run.dispatches
            .iter()
            .find(|one| one.id == id)
            .expect("the attempt"),
    )
}

fn task_of(run: &Run) -> TaskTime {
    task_time(run, run.task("t-1").expect("the task"))
}

fn piece(from: i64, to: i64, holds: &[Hold]) -> Piece {
    Piece {
        from_ms: from * MINUTE,
        to_ms: to * MINUTE,
        holds: holds.to_vec(),
    }
}

/// The pieces cover the attempt end to end, and add up to its wall clock.
fn adds_up(time: &AttemptTime) {
    let wall = time.wall_ms.expect("a closed attempt");
    assert_eq!(
        time.pieces
            .iter()
            .map(|one| one.to_ms - one.from_ms)
            .sum::<i64>(),
        wall
    );
    assert_eq!(
        time.pieces.first().map(|one| one.from_ms),
        Some(time.started_ms)
    );
    assert_eq!(time.pieces.last().map(|one| one.to_ms), time.ended_ms);
    assert!(
        time.pieces
            .windows(2)
            .all(|pair| pair[0].to_ms == pair[1].from_ms)
    );
}

fn held(time: &AttemptTime, hold: Hold) -> i64 {
    time.held_ms
        .as_ref()
        .expect("totals")
        .get(&hold)
        .copied()
        .unwrap_or(0)
}

#[test]
fn two_attempts_and_the_gap_between_them_each_add_up_to_their_own_wall() {
    let run = run_of(
        json!([
            attempt("dp-1", 0, Some(10 * MINUTE), Some(false)),
            attempt("dp-2", 15 * MINUTE, Some(30 * MINUTE), Some(true)),
        ]),
        json!([]),
        json!([]),
        json!({}),
    );
    let first = timed(&run, "dp-1");
    adds_up(&first);
    assert_eq!(first.succeeded, Some(false), "a failed attempt is kept");
    assert_eq!(
        (first.wall_ms, first.unknown_ms),
        (Some(10 * MINUTE), Some(10 * MINUTE))
    );
    adds_up(&timed(&run, "dp-2"));
    let task = task_of(&run);
    assert_eq!(
        task.wall_ms,
        task_cost::wall_ms(&task_cost::attempts(&run, "t-1"))
    );
    assert_eq!(
        (
            task.wall_ms,
            task.covered_ms,
            task.gap_ms,
            task.attempt_overlap_ms
        ),
        (
            Some(30 * MINUTE),
            Some(25 * MINUTE),
            Some(5 * MINUTE),
            Some(0)
        )
    );
}

#[test]
fn attempts_open_at_once_are_an_overlap_and_are_covered_once() {
    let run = run_of(
        json!([
            attempt("dp-1", 0, Some(20 * MINUTE), Some(true)),
            attempt("dp-2", 10 * MINUTE, Some(30 * MINUTE), Some(true)),
        ]),
        json!([]),
        json!([]),
        json!({}),
    );
    let task = task_of(&run);
    assert_eq!(
        (
            task.wall_ms,
            task.covered_ms,
            task.gap_ms,
            task.attempt_overlap_ms
        ),
        (
            Some(30 * MINUTE),
            Some(30 * MINUTE),
            Some(0),
            Some(10 * MINUTE)
        )
    );
}

#[test]
fn an_answered_question_is_its_own_time_and_the_rest_is_unknown() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            question("m-q", "dp-1", 2 * MINUTE),
            answer("m-a", "m-q", 5 * MINUTE)
        ]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(
        time.pieces,
        vec![
            piece(0, 2, &[]),
            piece(2, 5, &[Hold::Question]),
            piece(5, 10, &[])
        ]
    );
    assert_eq!(held(&time, Hold::Question), 3 * MINUTE);
    assert_eq!(
        (time.unknown_ms, time.overlap_ms),
        (Some(7 * MINUTE), Some(0))
    );
    assert_eq!(time.holds[0].ends, HoldEnd::Answered);
}

#[test]
fn a_question_left_unanswered_stands_until_its_attempt_ends() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(false))]),
        json!([question("m-q", "dp-1", 6 * MINUTE)]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(
        (time.holds[0].to_ms, time.holds[0].ends),
        (Some(10 * MINUTE), HoldEnd::AttemptEnded)
    );
    assert_eq!(held(&time, Hold::Question), 4 * MINUTE);
}

#[test]
fn an_answer_after_the_attempt_ended_counts_only_inside_it() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(false))]),
        json!([
            question("m-q", "dp-1", 8 * MINUTE),
            answer("m-a", "m-q", 14 * MINUTE)
        ]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(
        time.holds[0].to_ms,
        Some(14 * MINUTE),
        "the late answer is kept"
    );
    assert_eq!(held(&time, Hold::Question), 2 * MINUTE);
}

#[test]
fn a_second_answer_does_not_move_the_end() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            question("m-q", "dp-1", 2 * MINUTE),
            answer("m-a", "m-q", 4 * MINUTE),
            answer("m-b", "m-q", 7 * MINUTE),
        ]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    assert_eq!(time.holds[0].to_ms, Some(4 * MINUTE));
    assert_eq!(held(&time, Hold::Question), 2 * MINUTE);
}

#[test]
fn a_question_and_a_gate_at_once_are_an_overlap_not_a_share() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            question("m-q", "dp-1", 2 * MINUTE),
            answer("m-a", "m-q", 6 * MINUTE)
        ]),
        json!([gate("g-1", 4 * MINUTE, Some(8 * MINUTE))]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(
        time.pieces,
        vec![
            piece(0, 2, &[]),
            piece(2, 4, &[Hold::Question]),
            piece(4, 6, &[Hold::Question, Hold::Gate]),
            piece(6, 8, &[Hold::Gate]),
            piece(8, 10, &[]),
        ]
    );
    assert_eq!(
        (held(&time, Hold::Question), held(&time, Hold::Gate)),
        (4 * MINUTE, 4 * MINUTE)
    );
    assert_eq!(
        (time.overlap_ms, time.unknown_ms),
        (Some(2 * MINUTE), Some(4 * MINUTE))
    );
    assert_eq!(task_of(&run).gates.len(), 1);
}

#[test]
fn two_questions_at_once_are_one_stretch_of_question_time() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            question("m-q", "dp-1", 2 * MINUTE),
            question("m-r", "dp-1", 4 * MINUTE),
            answer("m-a", "m-q", 6 * MINUTE),
            answer("m-b", "m-r", 8 * MINUTE),
        ]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(held(&time, Hold::Question), 6 * MINUTE);
    assert_eq!(time.overlap_ms, Some(0));
}

#[test]
fn an_open_attempt_has_no_totals_and_its_end_is_not_now() {
    let run = run_of(
        json!([attempt("dp-1", 0, None, None)]),
        json!([question("m-q", "dp-1", 2 * MINUTE)]),
        json!([gate("g-1", MINUTE, None)]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    assert_eq!(
        (
            time.wall_ms,
            time.held_ms.clone(),
            time.overlap_ms,
            time.unknown_ms
        ),
        (None, None, None, None)
    );
    assert!(time.pieces.is_empty());
    assert_eq!(
        (time.holds[0].to_ms, time.holds[0].ends),
        (None, HoldEnd::Standing)
    );
    let task = task_of(&run);
    assert_eq!(
        (task.wall_ms, task.covered_ms, task.gap_ms),
        (None, None, None)
    );
}

#[test]
fn a_quota_wall_and_a_continuation_are_events_with_no_length() {
    let mut walled = message("m-w", "quota_walled", Some("dp-1"), 3 * MINUTE);
    walled["from"] = json!("ledger");
    walled["body"] =
        json!(json!({"observedAtMs": 3 * MINUTE, "resetsAtMs": 60 * MINUTE}).to_string());
    let mut resumed = message("m-r", "resumed", Some("dp-1"), 5 * MINUTE);
    resumed["from"] = json!("ledger");
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            walled,
            resumed,
            message("m-s", "status", Some("dp-1"), 4 * MINUTE)
        ]),
        json!([]),
        json!({}),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(time.held_ms, Some(BTreeMap::new()));
    assert_eq!(time.unknown_ms, Some(10 * MINUTE));
    assert_eq!(
        time.events.keys().copied().collect::<Vec<_>>(),
        vec!["quota_walled", "resumed"],
        "a worker's status is not an event of the ledger's"
    );
    assert_eq!(time.events["quota_walled"].last, "m-w");
}

#[test]
fn a_clock_that_runs_backwards_has_no_totals() {
    let run = run_of(
        json!([
            attempt("dp-1", 10 * MINUTE, Some(5 * MINUTE), Some(true)),
            attempt("dp-2", -MINUTE, Some(MINUTE), Some(true)),
        ]),
        json!([]),
        json!([]),
        json!({}),
    );
    for id in ["dp-1", "dp-2"] {
        let time = timed(&run, id);
        assert_eq!((time.wall_ms, time.unknown_ms), (None, None), "{id}");
        assert!(time.pieces.is_empty());
    }
    assert_eq!(task_of(&run).wall_ms, None);
}

#[test]
fn a_review_wait_runs_from_the_reviewed_report_to_the_verification() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([
            done("m-d", "dp-1", HEAD, 10 * MINUTE),
            done("m-late", "dp-1", HEAD, 40 * MINUTE),
        ]),
        json!([]),
        reviewed("dp-1", HEAD, 25 * MINUTE),
    );
    let task = task_of(&run);
    let after = &task.after_report;
    assert_eq!(
        (after.attempt.as_deref(), after.source.as_deref()),
        (Some("dp-1"), Some(HEAD))
    );
    assert_eq!(
        (after.reported_ms, after.verified_ms, after.review_wait_ms),
        (Some(10 * MINUTE), Some(25 * MINUTE), Some(15 * MINUTE)),
        "a later duplicate report does not move the start"
    );
    assert_eq!((after.merged_ms, after.deployed_ms), (None, None));
    assert_eq!(
        task.wall_ms,
        Some(10 * MINUTE),
        "the wait is not added to the wall"
    );
    assert_eq!(timed(&run, "dp-1").events["worker_done"].count, 2);
}

#[test]
fn a_report_handing_in_another_source_is_not_the_report_reviewed() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(10 * MINUTE), Some(true))]),
        json!([done("m-d", "dp-1", "fff0000aaa1111", 10 * MINUTE)]),
        json!([]),
        reviewed("dp-1", HEAD, 25 * MINUTE),
    );
    let after = task_of(&run).after_report;
    assert_eq!(
        (after.reported_ms, after.verified_ms, after.review_wait_ms),
        (None, Some(25 * MINUTE), None)
    );
}

#[test]
fn a_review_of_an_attempt_the_task_moved_past_measures_no_wait() {
    let mut retried = attempt("dp-2", 20 * MINUTE, Some(30 * MINUTE), Some(true));
    retried["source"] = json!("fff0000aaa1111");
    let mut first = attempt("dp-1", 0, Some(10 * MINUTE), Some(true));
    first["source"] = json!(HEAD);
    let run = run_of(
        json!([first, retried]),
        json!([
            done("m-d", "dp-1", HEAD, 10 * MINUTE),
            done("m-e", "dp-2", "fff0000aaa1111", 30 * MINUTE),
        ]),
        json!([]),
        reviewed("dp-1", HEAD, 15 * MINUTE),
    );
    let after = task_of(&run).after_report;
    assert_eq!(after.attempt.as_deref(), Some("dp-1"));
    assert_eq!(
        (after.reported_ms, after.verified_ms, after.review_wait_ms),
        (Some(10 * MINUTE), None, None)
    );
}

#[test]
fn before_any_review_the_newest_attempts_own_report_is_named() {
    let mut first = attempt("dp-1", 0, Some(10 * MINUTE), Some(true));
    first["source"] = json!(HEAD);
    let run = run_of(
        json!([first]),
        json!([done("m-d", "dp-1", HEAD, 10 * MINUTE)]),
        json!([]),
        json!({}),
    );
    let after = task_of(&run).after_report;
    assert_eq!(
        (
            after.attempt.as_deref(),
            after.reported_ms,
            after.review_wait_ms
        ),
        (Some("dp-1"), Some(10 * MINUTE), None)
    );
}

#[test]
fn moments_at_the_ends_of_the_clock_never_overflow() {
    let run = run_of(
        json!([attempt("dp-1", 0, Some(i64::MAX), Some(true))]),
        json!([
            question("m-q", "dp-1", i64::MIN),
            answer("m-a", "m-q", i64::MAX),
            done("m-d", "dp-1", HEAD, -5),
        ]),
        json!([gate("g-1", i64::MIN, Some(i64::MIN))]),
        reviewed("dp-1", HEAD, i64::MAX),
    );
    let time = timed(&run, "dp-1");
    adds_up(&time);
    assert_eq!(held(&time, Hold::Question), i64::MAX);
    assert_eq!(held(&time, Hold::Gate), 0);
    let task = task_of(&run);
    assert_eq!((task.covered_ms, task.gap_ms), (Some(i64::MAX), Some(0)));
    assert_eq!(
        (
            task.after_report.reported_ms,
            task.after_report.review_wait_ms
        ),
        (None, None),
        "a report before the epoch is no start"
    );
}

#[test]
fn dispatch_show_carries_the_attempts_time_and_the_tasks() {
    let mut bench = Bench::new();
    bench.json("run-create --name time");
    let task = bench.json("task-create --spec timed")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let never = bench.json(&format!("dispatch-show --task {task}"));
    assert_eq!(never["time"]["attempt"], Value::Null);
    assert_eq!(never["time"]["task"]["attempts"], 0);
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    let open = bench.json(&format!("dispatch-show --task {task}"));
    assert_eq!(open["time"]["attempt"]["dispatchId"], open["dispatchId"]);
    assert_eq!(open["time"]["attempt"]["wallMs"], Value::Null);
    bench.clock += MINUTE;
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
    let ended = bench.json(&format!("dispatch-show --task {task}"));
    let wall = ended["time"]["attempt"]["wallMs"]
        .as_i64()
        .expect("a closed attempt's wall");
    assert_eq!(ended["time"]["attempt"]["unknownMs"].as_i64(), Some(wall));
    assert_eq!(ended["time"]["task"]["wallMs"].as_i64(), Some(wall));
}
