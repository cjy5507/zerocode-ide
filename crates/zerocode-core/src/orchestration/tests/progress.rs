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
    let run_id = bench.ledger.runs()[0].id.clone();

    let working = task_of(&mut bench, "working");
    let (_, _, open) = attempt_on(&mut bench, &working);
    let (row, whole) = read(&bench, &working, &open);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::Working, WorkStage::Working)
    );
    assert_eq!(row.next, None);

    let (reported, worker, dispatch) = handed_in(&mut bench, "reported", HEAD_A, "");
    let (row, whole) = read(&bench, &reported, &dispatch);
    assert_eq!(
        (row.stage, whole.stage),
        (WorkStage::ReportOnly, WorkStage::ReportOnly)
    );
    assert_eq!(row.source.as_deref(), Some(HEAD_A));
    assert_eq!(
        row.next.as_deref(),
        Some(format!("zerocode-orc worktree-evidence --run {run_id} --worker {worker}").as_str())
    );

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

    let failed = task_of(&mut bench, "failed");
    let (worker, pane, dispatch) = attempt_on(&mut bench, &failed);
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":false}");
    let (row, _) = read(&bench, &failed, &dispatch);
    assert_eq!(row.stage, WorkStage::Failed);
    assert_eq!(
        row.next.as_deref(),
        Some(format!("zerocode-orc worker-transcript --run {run_id} --worker {worker}").as_str())
    );

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

    let by_hand = task_of(&mut bench, "by-hand");
    bench.json(&format!("task-update --task {by_hand} --status completed"));
    let run = &bench.ledger.runs()[0];
    let whole = run.progress_of(run.task(&by_hand).expect("the task"));
    assert_eq!((whole.stage, whole.next), (WorkStage::Unknown, None));

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
                "zerocode-orc land-check --run {run_id} --task {verified} --head {HEAD_A} --retry-request land-check-{verified}-{}",
                &HEAD_A[..12]
            )
            .as_str()
        )
    );

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
    let (both, _, dispatch) = handed_in(&mut bench, "both", HEAD_A, "");
    review(
        &mut bench,
        &both,
        &format!("{{\"verified\":true,\"mergeHead\":\"{HEAD_A}\",\"nothingToLand\":true}}"),
        &dispatch,
        HEAD_A,
    );
    assert_eq!(read(&bench, &both, &dispatch).1.stage, WorkStage::Merged);

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

    let (stale, _, first) = handed_in(&mut bench, "stale", HEAD_A, "");
    review(&mut bench, &stale, "{\"verified\":true}", &first, HEAD_A);
    bench.json(&format!("task-update --task {stale} --status ready"));
    let (_, _, second) = attempt_on(&mut bench, &stale);
    let (old, whole) = read(&bench, &stale, &first);
    assert_eq!(old.stage, WorkStage::Superseded);
    assert_eq!(old.source.as_deref(), Some(HEAD_A));
    assert_eq!(old.next, None);
    assert_eq!(whole.stage, WorkStage::Working);
    assert_eq!(read(&bench, &stale, &second).0.stage, WorkStage::Working);

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
        Some(
            format!(
                "zerocode-orc worktree-evidence --run {} --worker {worker}",
                run.id
            )
            .as_str()
        )
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

/// A shown command names the run its work is in, so a caller bound to another run reads that work
/// and not its own run's.
#[test]
fn a_shown_command_names_its_run_so_a_caller_bound_elsewhere_reads_the_right_work() {
    let mut bench = Bench::new();
    bench.json("run-create --name first");
    let first = bench.ledger.runs()[0].id.clone();
    let task = task_of(&mut bench, "evidence");
    let (_, pane, _) = attempt_on(&mut bench, &task);
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{HEAD_A}\"}}"),
    );
    assert!(
        bench
            .ledger
            .worker_seated(("team-1", &pane), "/checkouts/first")
    );
    let next = {
        let run = &bench.ledger.runs()[0];
        run.progress_of(run.task(&task).expect("the task"))
            .next
            .expect("a command to read the work with")
    };
    bench.json("run-create --name second");
    assert!(next.contains(&format!("--run {first}")), "{next}");
    let read = bench.run(next.strip_prefix("zerocode-orc ").expect("the program"));
    assert_eq!(read.reply.exit_code, 0, "{}", read.reply.stderr);
    assert_eq!(
        read.effect,
        Effect::WorktreeEvidence {
            checkout: Some("/checkouts/first".to_string())
        }
    );
}

/// An older attempt never wears the review of a newer one: once a retry handed in and was verified,
/// the first attempt's own row says it was superseded, with its own attempt and commit and nothing to
/// run — whether the retry handed in another commit or the same one — and a first attempt that failed
/// still says it failed.
#[test]
fn an_older_attempt_never_wears_the_review_of_a_newer_one() {
    let mut bench = Bench::new();
    bench.json("run-create --name attempts");
    let retried = |bench: &mut Bench, spec: &str, first_body: &str, second_head: &str| {
        let task = task_of(bench, spec);
        let (_, pane, first) = attempt_on(bench, &task);
        bench.json_at(
            &pane,
            &format!("send --type worker_done --body {first_body}"),
        );
        if bench.ledger.runs()[0].task(&task).map(|held| held.status) != Some(TaskStatus::Ready) {
            bench.json(&format!("task-update --task {task} --status ready"));
        }
        let (_, pane, second) = attempt_on(bench, &task);
        bench.json_at(
            &pane,
            &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{second_head}\"}}"),
        );
        review(bench, &task, "{\"verified\":true}", &second, second_head);
        (task, first, second)
    };
    let cases = [
        (
            "another-head",
            format!("{{\"ok\":true,\"head\":\"{HEAD_A}\"}}"),
            HEAD_B,
            "superseded",
        ),
        (
            "same-head",
            format!("{{\"ok\":true,\"head\":\"{HEAD_A}\"}}"),
            HEAD_A,
            "superseded",
        ),
        (
            "failed-first",
            "{\"ok\":false}".to_string(),
            HEAD_B,
            "failed",
        ),
    ];
    for (spec, first_body, second_head, said) in cases {
        let (task, first, second) = retried(&mut bench, spec, &first_body, second_head);
        let (old, whole) = read(&bench, &task, &first);
        let (new, _) = read(&bench, &task, &second);
        assert_eq!(old.stage.word(), said, "{spec}: {old:?}");
        assert_eq!(
            old.attempt.as_deref(),
            Some(first.as_str()),
            "{spec}: {old:?}"
        );
        assert_eq!(old.next, None, "{spec}: {old:?}");
        assert_eq!(old.merge_head, None, "{spec}: {old:?}");
        if said == "superseded" {
            assert_eq!(old.source.as_deref(), Some(HEAD_A), "{spec}: {old:?}");
        }
        assert_eq!(new.stage.word(), "verified", "{spec}: {new:?}");
        assert_eq!(whole.stage.word(), "verified", "{spec}: {whole:?}");
        assert_eq!(new.source.as_deref(), Some(second_head), "{spec}: {new:?}");
    }
}

/// A coordinator's result that carries no review — a note — is not a review of an older commit: the
/// report it sits beside still waits for one.
#[test]
fn a_coordinators_note_that_is_no_review_leaves_the_report_waiting() {
    let mut bench = Bench::new();
    bench.json("run-create --name note");
    let (task, _, dispatch) = handed_in(&mut bench, "noted", HEAD_A, "");
    bench.json(&format!(
        "task-update --task {task} --result {{\"note\":\"later\"}}"
    ));
    let (row, whole) = read(&bench, &task, &dispatch);
    assert_eq!(row.stage.word(), "report_only", "{row:?}");
    assert_eq!(whole.stage.word(), "report_only", "{whole:?}");
}

/// A coordinator's record that carries review keys but names no source it read is unknown once the
/// attempt has handed something in — whether the record named the same attempt or an earlier one. It
/// is never read as a review of an older commit.
#[test]
fn a_record_that_names_no_source_reads_unknown_whatever_attempt_it_names() {
    let mut bench = Bench::new();
    bench.json("run-create --name legacy");
    let run_id = bench.ledger.runs()[0].id.clone();
    let unbound = |attempt: &str| ResultAuthor::Coordinator {
        seat: "team-1/%0".to_string(),
        generation: Some(1),
        attempt: Some(attempt.to_string()),
        source: None,
        completed_ms: None,
        verified_ms: None,
    };
    let (same, _, attempt) = handed_in(&mut bench, "same-attempt", HEAD_A, "");
    bench
        .ledger
        .update_task(
            &run_id,
            &same,
            None,
            Some("{\"verified\":true}".to_string()),
            unbound(&attempt),
            bench.clock,
        )
        .expect("an old record on the same attempt");
    let earlier = task_of(&mut bench, "earlier-attempt");
    let (worker, _, first) = attempt_on(&mut bench, &earlier);
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the first attempt ends");
    if bench.ledger.runs()[0]
        .task(&earlier)
        .map(|held| held.status)
        != Some(TaskStatus::Ready)
    {
        bench.json(&format!("task-update --task {earlier} --status ready"));
    }
    let (_, pane, second) = attempt_on(&mut bench, &earlier);
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{HEAD_B}\"}}"),
    );
    bench
        .ledger
        .update_task(
            &run_id,
            &earlier,
            None,
            Some("{\"verified\":true}".to_string()),
            unbound(&first),
            bench.clock,
        )
        .expect("an old record on the earlier attempt");
    for (task, dispatch) in [(&same, &attempt), (&earlier, &second)] {
        let (row, whole) = read(&bench, task, dispatch);
        assert_eq!(row.stage.word(), "unknown", "{task}: {row:?}");
        assert_eq!(whole.stage.word(), "unknown", "{task}: {whole:?}");
    }
}

/// A task a sweep compacted away leaves its attempt: a failed one still reads failed, a reported one
/// still reads reported, and nothing is offered to run without the task to name.
#[test]
fn an_attempt_whose_task_was_compacted_away_reads_by_its_own_end() {
    let mut bench = Bench::new();
    bench.json("run-create --name compacted");
    let failed = task_of(&mut bench, "failed");
    let (_, pane, lost) = attempt_on(&mut bench, &failed);
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":false}");
    let (_, _, handed) = handed_in(&mut bench, "reported", HEAD_A, "");
    let run = &bench.ledger.runs()[0];
    let none = ReviewFacts::default();
    let lost = run.progress_from(None, &none, run.dispatch(&lost));
    let handed = run.progress_from(None, &none, run.dispatch(&handed));
    assert_eq!((lost.stage, lost.next), (WorkStage::Failed, None));
    assert_eq!((handed.stage, handed.next), (WorkStage::ReportOnly, None));
    assert_eq!(handed.source.as_deref(), Some(HEAD_A));
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
