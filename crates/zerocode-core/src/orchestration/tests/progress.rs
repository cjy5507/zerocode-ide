//! t-47538: one reading of where a task's work stands (`progress.rs`).
//!
//! Every case is a ledger built through the verbs a coordinator and a worker
//! use, and read the two ways the window asks: about one worker's own attempt,
//! and about the task as a whole (the desk). These pin the projection itself;
//! the faults it closes are pinned where a person saw them — the desk's stages
//! (`desk.rs`) and the window's words (`ui/tests`).

use super::*;

const HEAD_A: &str = "aaaaaaa1111111";
const HEAD_B: &str = "bbbbbbb2222222";

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// One worker on `task`: its id, its pane and its attempt.
fn attempt_on(bench: &mut Bench, task: &str) -> (String, String, String) {
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let dispatch = bench.ledger.runs()[0]
        .newest_attempt(task)
        .expect("an attempt")
        .id
        .clone();
    (worker, pane, dispatch)
}

/// A worker that handed `head` in with `extra` keys beside `ok:true`: the
/// task, the worker and the attempt.
fn handed_in(bench: &mut Bench, spec: &str, head: &str, extra: &str) -> (String, String, String) {
    let task = task_of(bench, spec);
    let (worker, pane, dispatch) = attempt_on(bench, &task);
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"{extra}}}"),
    );
    (task, worker, dispatch)
}

fn review(bench: &mut Bench, task: &str, result: &str, dispatch: &str, source: &str) {
    bench.json(&format!(
        "task-update --task {task} --result {result} --attempt {dispatch} --source {source}"
    ));
}

/// The two readings: the attempt's own, and the task's.
fn read(bench: &Bench, task: &str, dispatch: &str) -> (WorkProgress, WorkProgress) {
    let run = &bench.ledger.runs()[0];
    let held = run.task(task).expect("the task");
    (
        run.progress_from(Some(held), &run.review_of(held), run.dispatch(dispatch)),
        run.progress_of(held),
    )
}

#[test]
fn every_stage_the_ledger_can_say_is_told_apart_by_one_reading() {
    let mut bench = Bench::new();
    bench.json("run-create --name progress");

    // Working: the attempt is open.
    let working = task_of(&mut bench, "working");
    let (_, _, open) = attempt_on(&mut bench, &working);
    let (row, whole) = read(&bench, &working, &open);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Working, WorkStage::Working)
    );
    assert_eq!(row.next, None);

    // Report only: handed in, nobody reviewed it. The next look is what the checkout proves.
    let (reported, worker, dispatch) = handed_in(&mut bench, "reported", HEAD_A, "");
    let (row, whole) = read(&bench, &reported, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::ReportOnly, WorkStage::ReportOnly)
    );
    assert_eq!(row.source.as_deref(), Some(HEAD_A));
    assert_eq!(
        row.next.as_deref(),
        Some(format!("zerocode-orc worktree-evidence --worker {worker}").as_str())
    );

    // A worker's own word is a claim beside the report, never the stage — and its word that there
    // is nothing to land leaves the work waiting for a review.
    let (claimed, _, dispatch) = handed_in(&mut bench, "claimed", HEAD_A, ",\"merged\":true");
    let (row, _) = read(&bench, &claimed, &dispatch);
    assert_eq!(
        (row.stage, row.claimed),
        (WorkStage::ReportOnly, Some(WorkStage::Merged))
    );
    let (unlanded, _, dispatch) =
        handed_in(&mut bench, "unlanded", HEAD_A, ",\"nothingToLand\":true");
    let (row, _) = read(&bench, &unlanded, &dispatch);
    assert_eq!((row.stage, row.claimed), (WorkStage::ReportOnly, None));

    // Failed: the attempt ended without a successful report. The next look is its own words.
    let failed = task_of(&mut bench, "failed");
    let (worker, pane, dispatch) = attempt_on(&mut bench, &failed);
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":false}");
    let (row, _) = read(&bench, &failed, &dispatch);
    assert_eq!(row.stage, WorkStage::Failed);
    assert_eq!(
        row.next.as_deref(),
        Some(format!("zerocode-orc worker-transcript --worker {worker}").as_str())
    );

    // Unreviewable: done by hand, and no attempt ever handed anything in. Over, and nothing to read.
    let unreviewable = task_of(&mut bench, "unreviewable");
    let (worker, _, dispatch) = attempt_on(&mut bench, &unreviewable);
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
    bench.json(&format!(
        "task-update --task {unreviewable} --status completed"
    ));
    let (row, whole) = read(&bench, &unreviewable, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Unreviewable, WorkStage::Unreviewable)
    );
    assert_eq!(whole.next, None);

    // Unknown: completed by hand with no attempt at all — not unreviewable, and not a report.
    let by_hand = task_of(&mut bench, "by-hand");
    bench.json(&format!("task-update --task {by_hand} --status completed"));
    let run = &bench.ledger.runs()[0];
    let whole = run.progress_of(run.task(&by_hand).expect("the task"));
    assert_eq!((whole.stage, whole.next), (WorkStage::Unknown, None));

    // Verified, not merged: the next check is the landing check of the reviewed head, named so that
    // pasting it twice is one request.
    let (verified, _, dispatch) = handed_in(&mut bench, "verified", HEAD_A, "");
    review(
        &mut bench,
        &verified,
        "{\"verified\":true}",
        &dispatch,
        HEAD_A,
    );
    let (row, whole) = read(&bench, &verified, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Verified, WorkStage::Verified)
    );
    assert_eq!(
        whole.next.as_deref(),
        Some(
            format!(
                "zerocode-orc land-check --task {verified} --head {HEAD_A} --retry-request land-check-{verified}-{}",
                &HEAD_A[..12]
            )
            .as_str()
        )
    );

    // Verified with no code to land.
    let (nothing, _, dispatch) = handed_in(&mut bench, "nothing", HEAD_A, "");
    review(
        &mut bench,
        &nothing,
        "{\"verified\":true,\"nothingToLand\":true}",
        &dispatch,
        HEAD_A,
    );
    assert_eq!(
        read(&bench, &nothing, &dispatch).1.stage,
        WorkStage::NothingToLand
    );
    // A merge written beside the mark wins: the work is in main.
    let (both, _, dispatch) = handed_in(&mut bench, "both", HEAD_A, "");
    review(
        &mut bench,
        &both,
        &format!("{{\"verified\":true,\"mergeHead\":\"{HEAD_A}\",\"nothingToLand\":true}}"),
        &dispatch,
        HEAD_A,
    );
    assert_eq!(read(&bench, &both, &dispatch).1.stage, WorkStage::Merged);

    // Merged, not deployed: the commit it landed as rides along.
    let (merged, _, dispatch) = handed_in(&mut bench, "merged", HEAD_A, "");
    review(
        &mut bench,
        &merged,
        &format!("{{\"verified\":true,\"mergeHead\":\"{HEAD_B}\"}}"),
        &dispatch,
        HEAD_A,
    );
    let (_, whole) = read(&bench, &merged, &dispatch);
    assert_eq!(whole.stage, WorkStage::Merged);
    assert_eq!(whole.merge_head.as_deref(), Some(HEAD_B));
    assert_eq!(whole.next, None);

    // Deployed, with the attempt and the source the review named.
    let (deployed, _, dispatch) = handed_in(&mut bench, "deployed", HEAD_A, "");
    review(
        &mut bench,
        &deployed,
        "{\"verified\":true,\"merged\":true,\"deployed\":true}",
        &dispatch,
        HEAD_A,
    );
    let (row, whole) = read(&bench, &deployed, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Deployed, WorkStage::Deployed)
    );
    assert_eq!(whole.source.as_deref(), Some(HEAD_A));
    assert_eq!(whole.attempt.as_deref(), Some(dispatch.as_str()));

    // Stale: a review of one attempt, and a retry started after it. The old attempt's row no longer
    // stands verified, and offers nothing to read for an attempt the task moved past; the retry is
    // at work.
    let (stale, _, first) = handed_in(&mut bench, "stale", HEAD_A, "");
    review(&mut bench, &stale, "{\"verified\":true}", &first, HEAD_A);
    bench.json(&format!("task-update --task {stale} --status ready"));
    let (_, _, second) = attempt_on(&mut bench, &stale);
    let (old, whole) = read(&bench, &stale, &first);
    assert_eq!(old.stage, WorkStage::Stale);
    assert_eq!(old.reviewed.as_deref(), Some(HEAD_A));
    assert_eq!(old.next, None);
    assert_eq!(whole.stage, WorkStage::Working);
    assert_eq!(read(&bench, &stale, &second).0.stage, WorkStage::Working);

    // Closed outranks every review.
    let (closed, _, dispatch) = handed_in(&mut bench, "closed", HEAD_A, "");
    bench.json(&format!(
        "task-update --task {closed} --status closed --closed-as outdated --why replaced"
    ));
    let (row, whole) = read(&bench, &closed, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Closed, WorkStage::Closed)
    );
}

/// A newer hand-in on the SAME attempt leaves the review of the older commit behind: the task reads
/// stale, names both commits, and its next look is what the checkout now proves.
#[test]
fn a_review_of_an_older_commit_on_the_same_attempt_reads_stale_and_names_both() {
    let mut bench = Bench::new();
    bench.json("run-create --name stale-source");
    let (task, worker, attempt) = handed_in(&mut bench, "moved", HEAD_A, "");
    review(&mut bench, &task, "{\"verified\":true}", &attempt, HEAD_A);
    let mut moved = bench.ledger.export();
    moved
        .dispatches
        .iter_mut()
        .find(|row| row.id == attempt)
        .expect("attempt")
        .source = Some(HEAD_B.to_string());
    let moved = Ledger::rebuild(moved).expect("a readable changed hand-in");
    let run = &moved.runs()[0];
    let whole = run.progress_of(run.task(&task).expect("task"));
    assert_eq!(whole.stage, WorkStage::Stale);
    assert_eq!(whole.reviewed.as_deref(), Some(HEAD_A));
    assert_eq!(whole.source.as_deref(), Some(HEAD_B));
    assert_eq!(
        whole.next.as_deref(),
        Some(format!("zerocode-orc worktree-evidence --worker {worker}").as_str())
    );
}

/// A coordinator's record that names no source it reviewed is not believed as success, whatever it
/// says: the stage is unknown, and nothing is offered to run.
#[test]
fn a_coordinators_record_that_names_no_source_is_unknown_never_a_success_stage() {
    let mut bench = Bench::new();
    bench.json("run-create --name unbound");
    let (task, _, _) = handed_in(&mut bench, "unbound", HEAD_A, "");
    let run_id = bench.ledger.runs()[0].id.clone();
    bench
        .ledger
        .update_task(
            &run_id,
            &task,
            None,
            Some("{\"verified\":true,\"merged\":true,\"deployed\":true}".to_string()),
            ResultAuthor::Coordinator {
                seat: "team/%0".to_string(),
                generation: Some(1),
                attempt: None,
                source: None,
                completed_ms: None,
                verified_ms: None,
            },
            bench.clock,
        )
        .expect("an old record");
    let run = &bench.ledger.runs()[0];
    let whole = run.progress_of(run.task(&task).expect("task"));
    assert_eq!((whole.stage, whole.next), (WorkStage::Unknown, None));
}

/// A review of a report names no head to merge: the work reads verified, and no landing check of a
/// report's id is offered.
#[test]
fn a_review_of_a_report_offers_no_landing_check() {
    let mut bench = Bench::new();
    bench.json("run-create --name report");
    let task = task_of(&mut bench, "research");
    let (_, pane, dispatch) = attempt_on(&mut bench, &task);
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    let report = bench.ledger.runs()[0]
        .dispatch(&dispatch)
        .and_then(|one| one.source.clone())
        .expect("the report is what was handed in");
    review(&mut bench, &task, "{\"verified\":true}", &dispatch, &report);
    let run = &bench.ledger.runs()[0];
    let whole = run.progress_of(run.task(&task).expect("task"));
    assert_eq!((whole.stage, whole.next), (WorkStage::Verified, None));
}

/// The wire names are the window's keys: one per stage, none shared.
#[test]
fn every_stage_has_its_own_wire_name() {
    let words: std::collections::BTreeSet<_> =
        WorkStage::ALL.iter().map(|stage| stage.word()).collect();
    assert_eq!(words.len(), WorkStage::ALL.len());
    for stage in WorkStage::ALL {
        assert_eq!(
            serde_json::to_value(stage).expect("serializes"),
            serde_json::Value::String(stage.word().to_string())
        );
    }
}
