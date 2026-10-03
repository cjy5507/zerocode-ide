//! t-19159 (t-15558 slice 1): a closed state that is not called failure —
//! a task folded into another, handed over to the other run, or made
//! outdated is `closed`, says why, and is neither completed nor failed; and
//! one settle pass lists (dry run) and closes the old tasks whose attempts
//! all ended handing nothing in.

use super::*;

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// A task with one attempt that ended without handing anything in.
fn task_with_an_empty_attempt(bench: &mut Bench, spec: &str) -> String {
    let id = task_of(bench, spec);
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
    id
}

fn refused(bench: &mut Bench, line: &str) -> String {
    let planned = bench.run(line);
    assert_eq!(planned.reply.exit_code, 1, "`{line}` was accepted");
    planned.reply.stderr
}

#[test]
fn a_closing_is_refused_without_its_reason_and_its_target() {
    let mut bench = Bench::new();
    bench.json("run-create --name closing");
    let one = task_of(&mut bench, "one");
    let two = task_of(&mut bench, "two");

    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed"),
    );
    assert!(said.contains("--closed-as"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as folded"),
    );
    assert!(said.contains("--into"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as handed-over"),
    );
    assert!(said.contains("--to"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as outdated"),
    );
    assert!(said.contains("--why"), "{said}");
    // A target the run does not hold, itself, and a name that is no run.
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as folded --into t-nobody"),
    );
    assert!(said.contains("t-nobody"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as folded --into {one}"),
    );
    assert!(said.contains("itself"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as handed-over --to {two}"),
    );
    assert!(said.contains("run-"), "{said}");
    // A reason without the closing it explains, and a word nobody wrote.
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status ready --closed-as outdated --why x"),
    );
    assert!(said.contains("--status closed"), "{said}");
    let said = refused(
        &mut bench,
        &format!("task-update --task {one} --status closed --closed-as gone"),
    );
    assert!(said.contains("gone"), "{said}");
    // Nothing above moved the record.
    assert_eq!(
        bench.ledger.runs[0].task(&one).expect("the task").status,
        TaskStatus::Ready
    );
}

#[test]
fn a_closed_task_says_why_and_is_not_failed_or_open() {
    let mut bench = Bench::new();
    bench.json("run-create --name closing");
    let folded = task_of(&mut bench, "folded");
    let into = task_of(&mut bench, "bigger");
    let handed = task_of(&mut bench, "handed");
    let old = task_of(&mut bench, "old");
    bench.json(&format!(
        "task-update --task {folded} --status closed --closed-as folded --into {into}"
    ));
    bench.json(&format!(
        "task-update --task {handed} --status closed --closed-as handed-over --to run-9"
    ));
    let said = bench.json(&format!(
        "task-update --task {old} --status closed --closed-as outdated --why overtaken"
    ));
    assert_eq!(said["status"], "closed");

    let listed = bench.json("task-list --status closed");
    let rows = listed["tasks"].as_array().expect("tasks");
    assert_eq!(rows.len(), 3);
    let row = |id: &str| rows.iter().find(|r| r["taskId"] == id).expect("a row");
    assert_eq!(row(&folded)["closed"]["kind"], "folded");
    assert_eq!(row(&folded)["closed"]["into"], into.as_str());
    assert_eq!(row(&handed)["closed"]["to"], "run-9");
    assert_eq!(row(&old)["closed"]["why"], "overtaken");

    // The open list leaves them out; the one still-open task stays.
    let open = bench.json("task-list --open");
    let ids: Vec<_> = open["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|r| r["taskId"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids, vec![into.clone()]);

    // A count of closed is its own count, and failed stays zero.
    let shown = bench.json("run-show");
    assert_eq!(shown["tasks"]["closed"], 3);
    assert_eq!(shown["tasks"]["failed"], 0);

    // Putting one back to work drops its reason with its closing.
    bench.json(&format!("task-update --task {old} --status ready"));
    let back = bench.json("task-list --open");
    assert!(
        back["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .any(|r| r["taskId"] == old.as_str() && r["closed"].is_null())
    );
}

#[test]
fn a_dependency_on_a_folded_task_follows_it_and_the_others_say_they_cannot() {
    let mut bench = Bench::new();
    bench.json("run-create --name closing");
    let folded = task_of(&mut bench, "folded");
    let into = task_of(&mut bench, "bigger");
    let outdated = task_of(&mut bench, "outdated");
    let follower = bench.json(&format!("task-create --spec follower --deps {folded}"));
    let follower = follower["taskId"].as_str().expect("id").to_string();
    let stuck = bench.json(&format!("task-create --spec stuck --deps {outdated}"));
    let stuck = stuck["taskId"].as_str().expect("id").to_string();
    bench.json(&format!(
        "task-update --task {folded} --status closed --closed-as folded --into {into}"
    ));
    bench.json(&format!(
        "task-update --task {outdated} --status closed --closed-as outdated --why gone"
    ));

    let row = |bench: &mut Bench, id: &str| {
        let all = bench.json("task-list");
        all["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .find(|r| r["taskId"] == id)
            .expect("a row")
            .clone()
    };
    // Waiting on the task it was folded into, which is not done yet.
    assert_eq!(row(&mut bench, &follower)["depsMet"], false);
    assert_eq!(row(&mut bench, &follower)["status"], "pending");
    // Done there, it is done here.
    bench.json(&format!("task-update --task {into} --status completed"));
    assert_eq!(row(&mut bench, &follower)["depsMet"], true);
    assert_eq!(row(&mut bench, &follower)["status"], "ready");
    // An outdated dependency cannot be followed and the row says so.
    let stuck_row = row(&mut bench, &stuck);
    assert_eq!(stuck_row["depsMet"], false);
    assert_eq!(stuck_row["blockedBy"], serde_json::json!([outdated]));
}

#[test]
fn folds_that_would_circle_are_refused() {
    let mut bench = Bench::new();
    bench.json("run-create --name closing");
    let one = task_of(&mut bench, "one");
    let two = task_of(&mut bench, "two");
    bench.json(&format!(
        "task-update --task {one} --status closed --closed-as folded --into {two}"
    ));
    let said = refused(
        &mut bench,
        &format!("task-update --task {two} --status closed --closed-as folded --into {one}"),
    );
    assert!(said.contains("circle"), "{said}");
}

#[test]
fn a_task_carried_by_a_worker_is_not_closed_under_it() {
    let mut bench = Bench::new();
    bench.json("run-create --name closing");
    let id = task_of(&mut bench, "carried");
    let _seat = bench.seat(&format!("worker-start --agent codex --task {id}"));
    let said = refused(
        &mut bench,
        &format!("task-update --task {id} --status closed --closed-as outdated --why x"),
    );
    assert!(said.contains("open attempts"), "{said}");
}

#[test]
fn a_ledger_written_before_closings_loads_unchanged() {
    let mut bench = Bench::new();
    bench.json("run-create --name old");
    let id = task_of(&mut bench, "old");
    let written = serde_json::to_value(bench.ledger.runs[0].task(&id).expect("task"))
        .expect("a task serializes");
    // An open task writes no `closed` key at all, so an older window reads
    // what a newer one wrote, and a row without the key reads as before.
    assert!(written.get("closed").is_none(), "{written}");
    let read: Task = serde_json::from_value(written).expect("a row without the field loads");
    assert_eq!(read.closed, None);
    assert_eq!(read.status, TaskStatus::Ready);

    // A closed one survives the round trip through the projection.
    bench.json(&format!(
        "task-update --task {id} --status closed --closed-as outdated --why gone"
    ));
    let carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    let task = carried.runs[0].task(&id).expect("the task");
    assert_eq!(task.status, TaskStatus::Closed);
    assert_eq!(task.closed, Some(Closure::Outdated { why: "gone".into() }));
}

#[test]
fn the_settle_pass_lists_first_and_closes_only_when_applied() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle");
    let empty = task_with_an_empty_attempt(&mut bench, "empty");
    let untouched = task_of(&mut bench, "no attempt at all");
    let failed_empty = task_with_an_empty_attempt(&mut bench, "also empty");
    // Done is done: a completed task with nothing handed in is listed apart
    // and never closed.
    let done_unhanded = task_with_an_empty_attempt(&mut bench, "done by hand");
    bench.json(&format!(
        "task-update --task {done_unhanded} --status completed"
    ));
    // One a coordinator already judged: old news it is not.
    let judged = task_with_an_empty_attempt(&mut bench, "judged");
    bench.ledger.runs[0]
        .tasks
        .iter_mut()
        .find(|task| task.id == judged)
        .expect("the task")
        .result = r#"{"merged":true}"#.into();
    // Nobody has touched any of it for longer than the quiet time.
    bench.clock += SETTLE_QUIET_DAYS * 86_400_000 + 1;
    // One handed something in, however long ago: it can take a review.
    let handed = task_of(&mut bench, "handed in");
    let (worker, pane) = bench.seat(&format!("worker-start --agent codex --task {handed}"));
    let _ = worker;
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    // One too fresh to call old.
    let fresh = task_with_an_empty_attempt(&mut bench, "fresh");

    let listed = bench.json("task-settle");
    assert_eq!(listed["apply"], false);
    assert_eq!(listed["days"], SETTLE_QUIET_DAYS);
    assert_eq!(listed["count"], 2, "{listed}");
    let ids: Vec<_> = listed["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|r| r["taskId"].as_str().expect("id").to_string())
        .collect();
    assert!(
        ids.contains(&empty) && ids.contains(&failed_empty),
        "{ids:?}"
    );
    assert!(
        !ids.contains(&untouched)
            && !ids.contains(&handed)
            && !ids.contains(&fresh)
            && !ids.contains(&judged),
        "{ids:?}"
    );
    assert_eq!(listed["unreviewableCount"], 1, "{listed}");
    assert_eq!(
        listed["unreviewableCompleted"][0]["taskId"],
        done_unhanded.as_str()
    );
    // Listing wrote nothing.
    assert!(
        bench.ledger.runs[0]
            .tasks
            .iter()
            .all(|task| task.status != TaskStatus::Closed)
    );

    // A worker cannot apply; the coordinator can, and it closes outdated.
    let said = bench.at(&pane, "task-settle --apply");
    assert_eq!(said.reply.exit_code, 1, "{:?}", said.reply);
    let done = bench.json("task-settle --apply");
    assert_eq!(done["closed"].as_array().expect("closed").len(), 2);
    let task = bench.ledger.runs[0].task(&empty).expect("task");
    assert_eq!(task.status, TaskStatus::Closed);
    assert!(matches!(task.closed, Some(Closure::Outdated { .. })));
    assert_eq!(
        bench.ledger.runs[0]
            .task(&done_unhanded)
            .expect("task")
            .status,
        TaskStatus::Completed,
        "settle changed a completed task"
    );
    // A second pass finds nothing left to do.
    assert_eq!(bench.json("task-settle")["count"], 0);
}

#[test]
fn a_closed_task_is_never_graded_as_a_failed_attempt() {
    use crate::summon_difficulty::outcomes::observe;
    let mut bench = Bench::new();
    bench.json("run-create --name graded");
    let id = task_with_an_empty_attempt(&mut bench, "stopped");
    let dispatch = bench.ledger.runs()[0].dispatches[0].id.clone();
    let grade = |bench: &Bench| {
        let run = &bench.ledger.runs()[0];
        let total = task_cost::task_cost(
            run,
            &id,
            &task_cost::SessionBook::default(),
            task_cost::JevTally::default(),
            &task_cost::SessionAttribution::new(bench.ledger.runs()),
        );
        observe(
            run,
            run.dispatch(&dispatch).expect("the attempt"),
            &task_cost::GenerationCost::default(),
            &total,
        )
    };
    // While the task is open, its stopped attempt is graded; once the work
    // has moved elsewhere it is no evidence about the model at all.
    assert!(grade(&bench).is_some(), "the open task was not graded");
    bench.json(&format!(
        "task-update --task {id} --status closed --closed-as outdated --why moved"
    ));
    assert_eq!(grade(&bench), None);
}
