//! t-19328 (t-15558 slice 2): the board stops counting old completed work that
//! can take no review record as 검증 대기.
//!
//! A completed task whose every attempt ended handing nothing in has no source
//! a coordinator could name, so `task-update` refuses any review of it for ever
//! — and the board still said 검증 대기 of it, 347 times on one machine. The
//! fact is the ledger's, written once in core ([`Run::handed_nothing_in`], the
//! slice-1 settle pass's own reading) and shipped on the review the board
//! already reads, never guessed by the window.
//!
//! These tests read it the way the window and `task-list` do, as JSON, so they
//! say what the screen would be told.

use super::*;

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// One attempt on the task that ended without handing anything in — the
/// terminal was stopped.
fn an_attempt_that_ends_empty(bench: &mut Bench, id: &str) {
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
}

/// The shape of the old pile: attempts that all ended empty, then the
/// coordinator wrote the task down as done by hand.
fn done_by_hand_after(bench: &mut Bench, spec: &str, attempts: usize) -> String {
    let id = task_of(bench, spec);
    for _ in 0..attempts {
        an_attempt_that_ends_empty(bench, &id);
        if bench.ledger.runs[0].task(&id).map(|task| task.status) != Some(TaskStatus::Ready) {
            bench.json(&format!("task-update --task {id} --status ready"));
        }
    }
    bench.json(&format!("task-update --task {id} --status completed"));
    id
}

/// A task a worker handed a report in on: the report is the source a review
/// can name.
fn done_and_handed_in(bench: &mut Bench, spec: &str) -> String {
    let id = task_of(bench, spec);
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    id
}

/// What the board reads of one task — its review, through `task-list`.
fn row_of(bench: &mut Bench, id: &str) -> serde_json::Value {
    let listed = bench.json("task-list");
    listed["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|row| row["taskId"] == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
        .clone()
}

/// The fact, as the row carries it. A row that carries none is a failure of
/// the test, not a `false`: the window must never have to guess.
fn unreviewable(bench: &mut Bench, id: &str) -> bool {
    row_of(bench, id)["review"]["unreviewable"]
        .as_bool()
        .unwrap_or_else(|| panic!("the review of {id} carries no `unreviewable`"))
}

/// The ids whose review says unreviewable, out of one `task-list`.
fn unreviewable_ids(bench: &mut Bench) -> std::collections::BTreeSet<String> {
    let listed = bench.json("task-list");
    listed["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .filter(|row| row["review"]["unreviewable"] == true)
        .map(|row| row["taskId"].as_str().expect("an id").to_string())
        .collect()
}

#[test]
fn a_task_done_by_hand_after_attempts_that_handed_nothing_in_can_take_no_review() {
    let mut bench = Bench::new();
    bench.json("run-create --name board");
    let once = done_by_hand_after(&mut bench, "one empty attempt", 1);
    // Every attempt counts: two that both ended empty are still nothing handed in.
    let twice = done_by_hand_after(&mut bench, "two empty attempts", 2);
    assert!(unreviewable(&mut bench, &once));
    assert!(unreviewable(&mut bench, &twice));
    // The refusal the board now tells the truth about: no review can name it,
    // because the attempt has not handed in a source.
    let attempt = bench.ledger.runs[0]
        .newest_attempt(&once)
        .expect("an attempt")
        .id
        .clone();
    let said = bench.run(&format!(
        "task-update --task {once} --result {{\"verified\":true}} --attempt {attempt} --source abc1234"
    ));
    assert_eq!(said.reply.exit_code, 1, "a review of it was accepted");
    assert!(
        said.reply.stderr.contains("has not handed in a source"),
        "{}",
        said.reply.stderr
    );
}

#[test]
fn work_a_coordinator_can_still_review_is_never_unreviewable() {
    let mut bench = Bench::new();
    bench.json("run-create --name still-reviewable");

    // A worker handed a report in: the report is a source a review can name.
    let handed = done_and_handed_in(&mut bench, "handed in");
    // Written down as done with nobody ever dispatched: a review may still
    // name `--attempt none`.
    let nobody = task_of(&mut bench, "never dispatched");
    bench.json(&format!("task-update --task {nobody} --status completed"));
    // Completed by hand while an attempt still carries it: the attempt has
    // not ended, so nothing is known of what it will hand in.
    let carried = task_of(&mut bench, "still carried");
    let _seat = bench.seat(&format!("worker-start --agent codex --task {carried}"));
    bench.json(&format!("task-update --task {carried} --status completed"));
    // An early attempt ended empty, a later one handed something in.
    let retried = done_by_hand_after(&mut bench, "retried", 1);
    bench.json(&format!("task-update --task {retried} --status ready"));
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {retried}"));
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    // A coordinator already wrote its review: that is a record, not its absence.
    let judged = done_by_hand_after(&mut bench, "judged", 1);
    bench.ledger.runs[0]
        .tasks
        .iter_mut()
        .find(|task| task.id == judged)
        .expect("the task")
        .result = r#"{"merged":true}"#.into();
    // Not completed at all: an open task whose attempt ended empty is the
    // settle pass's pile, not a completed one.
    let open = task_of(&mut bench, "open");
    an_attempt_that_ends_empty(&mut bench, &open);

    for (id, why) in [
        (&handed, "a report was handed in"),
        (&nobody, "no attempt was made"),
        (&carried, "an attempt is still open"),
        (&retried, "a later attempt handed something in"),
        (&judged, "a review is written"),
        (&open, "the task is not completed"),
    ] {
        assert!(!unreviewable(&mut bench, id), "{id}: {why}");
    }
}

/// One definition, not two: the settle pass lists the completed tasks that can
/// never take a review, and the board says the same of the same tasks — the
/// only difference is the settle pass's own quiet time, which the board has no
/// reason to wait for (a task done by hand a minute ago is as unreviewable as
/// one done a year ago).
#[test]
fn the_settle_listing_and_the_board_name_the_same_tasks_apart_from_age() {
    let mut bench = Bench::new();
    bench.json("run-create --name one-definition");
    let old_one = done_by_hand_after(&mut bench, "old one", 1);
    let old_two = done_by_hand_after(&mut bench, "old two", 2);
    let reviewable = done_and_handed_in(&mut bench, "reviewable");
    bench.clock += SETTLE_QUIET_DAYS * 86_400_000 + 1;
    let fresh = done_by_hand_after(&mut bench, "fresh", 1);

    let board = unreviewable_ids(&mut bench);
    let listed: std::collections::BTreeSet<String> =
        bench.json("task-settle")["unreviewableCompleted"]
            .as_array()
            .expect("the settle listing")
            .iter()
            .map(|row| row["taskId"].as_str().expect("an id").to_string())
            .collect();

    assert_eq!(
        board,
        [old_one.clone(), old_two.clone(), fresh.clone()]
            .into_iter()
            .collect(),
        "the board's fact"
    );
    assert_eq!(
        listed,
        [old_one, old_two].into_iter().collect(),
        "the settle pass's listing is the same tasks, old enough"
    );
    assert!(!board.contains(&reviewable));
    // The pile the board stops counting as 검증 대기: before, every completed
    // task with no merge; after, only what a coordinator can still review.
    let run = &bench.ledger.runs()[0];
    let before = run
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Completed && !run.review_of(task).merged)
        .count();
    let after = before - board.len();
    assert_eq!((before, after), (4, 1), "검증 대기 before and after");
}

/// The fact is a reading of the attempts, never a thing stored: a later
/// attempt that hands something in takes it back, and no row of the ledger
/// gains a key for it.
#[test]
fn the_fact_follows_the_attempts_and_is_never_stored() {
    let mut bench = Bench::new();
    bench.json("run-create --name derived");
    let id = done_by_hand_after(&mut bench, "reopened", 1);
    assert!(unreviewable(&mut bench, &id));

    // Nothing new is written: neither the task nor its attempts gain a key.
    let run = &bench.ledger.runs[0];
    let task = serde_json::to_value(run.task(&id).expect("the task")).expect("a task serializes");
    let attempts = serde_json::to_value(&run.dispatches).expect("attempts serialize");
    assert!(!task.to_string().contains("unreviewable"), "{task}");
    assert!(!attempts.to_string().contains("unreviewable"), "{attempts}");

    // The coordinator puts it back to work and a new attempt hands something in.
    bench.json(&format!("task-update --task {id} --status ready"));
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    assert_eq!(
        bench.ledger.runs[0].task(&id).expect("the task").status,
        TaskStatus::Completed
    );
    assert!(!unreviewable(&mut bench, &id));
}

/// Old ledgers read unchanged: a review as an older window's row carried it
/// has no such key and reads as "no", and a ledger read back from its own rows
/// — attempts written before hand-ins were recorded have no source at all —
/// gives the same answer as the live one.
#[test]
fn a_ledger_written_before_the_fact_reads_unchanged() {
    let older: ReviewFacts = serde_json::from_value(serde_json::json!({
        "verified": false, "merged": false, "deployed": false, "written": false
    }))
    .expect("an older review reads");
    assert_eq!(
        serde_json::to_value(older).expect("a review serializes")["unreviewable"],
        false,
        "a review with no such key reads as not unreviewable"
    );

    let mut bench = Bench::new();
    bench.json("run-create --name older");
    let id = done_by_hand_after(&mut bench, "old", 1);
    let carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    let run = &carried.runs()[0];
    let read = run.task(&id).expect("the task");
    assert_eq!(
        serde_json::to_value(run.review_of(read)).expect("a review serializes")["unreviewable"],
        true,
        "a ledger read back from its rows says the same of the same task"
    );
}
