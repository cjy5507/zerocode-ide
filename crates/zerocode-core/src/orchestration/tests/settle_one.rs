//! t-119569: work nothing was ever handed in on — a measurement, a design, work
//! done by hand — settled by the coordinator in one call, with why and the
//! report it rests on, as 완료 — 착지할 것 없음.
//!
//! Before it, such a task could take no review at all: `task-update` refuses a
//! review of an attempt that handed nothing in, `hand-in` takes a commit, and
//! the coordinator closed finished work `outdated` (t-112868, 11-20), where
//! nothing reads it as done. The settle takes the road a backfill takes — the
//! seat's own attempt, a hand-in on it, and the review of that hand-in by the
//! road every review takes — so the review is bound like any other.

use super::*;
use crate::orchestration::coordinator_take;

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// Attempts that all ended handing nothing in, then the coordinator wrote the
/// task down as done by hand — the shape of the 158 the person's board held
/// (t-119569) and of the measurement closed `outdated` on 11-20.
fn done_by_hand(bench: &mut Bench, spec: &str) -> String {
    done_by_hand_saying(bench, spec, "{\"outcome\":\"measured\"}")
}

/// [`done_by_hand`], the coordinator writing `result` as the task's.
fn done_by_hand_saying(bench: &mut Bench, spec: &str, result: &str) -> String {
    let id = task_of(bench, spec);
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
    if bench.ledger.runs[0].task(&id).map(|task| task.status) != Some(TaskStatus::Ready) {
        bench.json(&format!("task-update --task {id} --status ready"));
    }
    bench.json(&format!(
        "task-update --task {id} --status completed --result {result}"
    ));
    id
}

fn task<'a>(bench: &'a Bench, id: &str) -> &'a Task {
    bench.ledger.runs()[0].task(id).expect("the task")
}

fn stage(bench: &Bench, id: &str) -> WorkStage {
    let run = &bench.ledger.runs()[0];
    run.progress_of(run.task(id).expect("the task")).stage
}

/// The one call: why and the report, from the coordinator seat.
fn settle(bench: &mut Bench, id: &str) -> serde_json::Value {
    bench.json(&format!(
        "task-settle --task {id} --why measured-on-the-installed-build --report /reports/r.md"
    ))
}

/// A task done by hand after an attempt that handed nothing in takes no review
/// by `task-update` — and one `task-settle --task` writes it done with nothing
/// to land, on the seat's own attempt and hand-in, keeping what it said before.
#[test]
fn work_nothing_was_handed_in_on_is_settled_in_one_call_as_done_with_nothing_to_land() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle-one");
    let id = done_by_hand(&mut bench, "measure");
    assert_eq!(stage(&bench, &id), WorkStage::Unreviewable);
    let attempt = bench.ledger.runs()[0]
        .newest_attempt(&id)
        .expect("the empty attempt")
        .id
        .clone();
    let refused = bench.run(&format!(
        "task-update --task {id} --attempt {attempt} --source {attempt} --result {{\"verified\":true}}"
    ));
    assert_ne!(
        refused.reply.exit_code, 0,
        "a review of an attempt that handed nothing in was taken"
    );

    let said = settle(&mut bench, &id);
    assert_eq!(said["taskId"], id.as_str(), "{said}");
    assert_eq!(said["stage"], "nothing_to_land", "{said}");
    let dispatch = said["dispatchId"].as_str().expect("the seat's attempt");
    let handed = said["handIn"].as_str().expect("the hand-in");

    let run = &bench.ledger.runs()[0];
    let held = run.task(&id).expect("the task");
    assert_eq!(held.status, TaskStatus::Completed);
    assert_eq!(stage(&bench, &id), WorkStage::NothingToLand);
    let review = run.review_of(held);
    assert!(review.verified && review.nothing_to_land, "{review:?}");
    assert_eq!(review.author, ReviewAuthor::Coordinator);
    assert_eq!(review.attempt.as_deref(), Some(dispatch));
    assert_eq!(review.source.as_deref(), Some(handed));
    assert!(
        !review.unreviewable,
        "a settled task still reads unreviewable"
    );
    let attempt = run.newest_attempt(&id).expect("the seat's attempt");
    assert_eq!(attempt.id, dispatch);
    assert!(
        attempt
            .worker
            .starts_with(coordinator_take::COORDINATOR_CARRIER)
    );
    assert_eq!(attempt.source.as_deref(), Some(handed));
    let result: serde_json::Value =
        serde_json::from_str(held.result.as_str()).expect("the result is JSON");
    assert_eq!(result["outcome"], "measured", "the result it had is kept");
    assert_eq!(result["settled"]["why"], "measured-on-the-installed-build");
    assert_eq!(result["settled"]["report"], "/reports/r.md");
    assert_eq!(result["report"], "/reports/r.md");
}

/// A task still open — done by hand before anybody was sent — is settled the
/// same way, and what waited on it is freed.
#[test]
fn an_open_task_done_by_hand_is_settled_and_frees_what_waits_on_it() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle-open");
    let first = task_of(&mut bench, "first");
    let waiting = bench.json(&format!("task-create --spec waiting --deps {first}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    assert_eq!(task(&bench, &waiting).status, TaskStatus::Pending);

    settle(&mut bench, &first);
    assert_eq!(task(&bench, &first).status, TaskStatus::Completed);
    assert_eq!(stage(&bench, &first), WorkStage::NothingToLand);
    assert_eq!(task(&bench, &waiting).status, TaskStatus::Ready);
}

/// Refused, with nothing written, wherever a review has something to name, a
/// review already stands, the work is still somebody's or over another way —
/// and from anywhere but the live coordinator seat.
#[test]
fn the_one_call_settle_is_refused_where_a_review_has_something_to_name_or_nobody_sits() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle-refused");
    let handed = task_of(&mut bench, "handed");
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {handed}"));
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    let carried = task_of(&mut bench, "carried");
    let (_carrier, carrier_pane) =
        bench.seat(&format!("worker-start --agent codex --task {carried}"));
    let closed = task_of(&mut bench, "closed");
    bench.json(&format!(
        "task-update --task {closed} --status closed --closed-as outdated --why gone"
    ));
    let gated = task_of(&mut bench, "gated");
    bench.json(&format!(
        "gate-create --task {gated} --question which --options [\"a\",\"b\"]"
    ));
    let first = task_of(&mut bench, "first");
    let waiting = bench.json(&format!("task-create --spec waiting --deps {first}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let plain = task_of(&mut bench, "plain");
    let open = task_of(&mut bench, "open");

    // What a refusal must leave as it found it: the work's own tables. A refused verb is still
    // tallied for the day, so the whole projection is not the measure.
    let work = |bench: &Bench| {
        let held = bench.ledger.export();
        (
            held.runs,
            held.tasks,
            held.dispatches,
            held.messages,
            held.gates,
        )
    };
    let refused = |bench: &mut Bench, pane: &str, line: &str, says: &str| {
        let before = work(bench);
        let planned = bench.at(pane, line);
        assert_ne!(planned.reply.exit_code, 0, "`{line}` was taken");
        assert!(
            planned.reply.stderr.contains(says),
            "`{line}` was refused for another reason: {}",
            planned.reply.stderr
        );
        assert!(work(bench) == before, "`{line}` wrote something");
    };
    let leader = agent_teams::LEADER_PANE;
    let line = |id: &str| format!("task-settle --task {id} --why done --report /reports/r.md");
    refused(&mut bench, leader, &line(&handed), "review that hand-in");
    refused(&mut bench, leader, &line(&carried), "carried by an attempt");
    refused(&mut bench, leader, &line(&closed), "is closed");
    refused(&mut bench, leader, &line(&gated), "blocked by gate");
    refused(
        &mut bench,
        leader,
        &line(&waiting),
        "waiting on its dependencies",
    );
    refused(
        &mut bench,
        leader,
        &format!("task-settle --task {plain} --report /reports/r.md"),
        "--why",
    );
    refused(
        &mut bench,
        leader,
        &format!("task-settle --task {plain} --why done"),
        "--report",
    );
    refused(
        &mut bench,
        leader,
        &format!("task-settle --task {plain} --why done --report /r.md --apply"),
        "does not support --apply",
    );
    refused(
        &mut bench,
        &carrier_pane,
        &line(&plain),
        "coordinator's verb",
    );

    // A review already stands: settled once, it is not settled again.
    settle(&mut bench, &plain);
    refused(&mut bench, leader, &line(&plain), "review already stands");

    // Nobody sits: the seat's verb is nobody's.
    let at = bench.clock;
    bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .expect("the run has a seat")
        .vacated_ms = Some(at);
    refused(&mut bench, leader, &line(&open), "nobody sitting");
}

/// A result that names a commit is code that may have landed (t-119569, the other provider's check
/// B · G): settled as nothing to land, it would read as done with no code. Refused with nothing
/// written, naming the commit and the two roads that read it — the review, and the backfill that
/// proves a landing from git. Both shapes: the coordinator's own words with the commit one object
/// down, and an old row in a worker's words with the commit on top.
#[test]
fn a_task_whose_result_names_a_commit_is_not_settled_as_nothing_to_land() {
    const NESTED: &str = "89abcdef0123";
    const ON_TOP: &str = "0123456789abcdef0123456789abcdef01234567";
    let mut bench = Bench::new();
    bench.json("run-create --name settle-commit");
    let nested = done_by_hand_saying(
        &mut bench,
        "nested",
        &format!("{{\"landed\":{{\"mergeHead\":\"{NESTED}\"}}}}"),
    );
    let old = done_by_hand(&mut bench, "old");
    let held = bench.ledger.runs[0]
        .tasks
        .iter_mut()
        .find(|one| one.id == old)
        .expect("the task");
    held.result = format!("{{\"ok\":true,\"commit\":\"{ON_TOP}\"}}").into();
    held.result_author = Some(ResultAuthor::Worker {
        worker: "w-0".to_string(),
        dispatch: None,
    });

    let work = |bench: &Bench| {
        let held = bench.ledger.export();
        (
            held.runs,
            held.tasks,
            held.dispatches,
            held.messages,
            held.gates,
        )
    };
    for (id, commit, under) in [
        (&nested, NESTED, "landed.mergeHead"),
        (&old, ON_TOP, "commit"),
    ] {
        let before = work(&bench);
        let refused = bench.run(&format!(
            "task-settle --task {id} --why done --report /reports/r.md"
        ));
        assert_ne!(
            refused.reply.exit_code, 0,
            "{id}, naming {commit}, was settled as nothing to land"
        );
        let says = &refused.reply.stderr;
        assert!(
            says.contains(commit) && says.contains(under) && says.contains("task-backfill"),
            "{id} was refused for another reason: {says}"
        );
        assert!(work(&bench) == before, "{id}'s refusal wrote something");
        assert_ne!(stage(&bench, id), WorkStage::NothingToLand, "{id}");
    }
}

/// The settle's review is a review like any other: a later attempt moves it,
/// so a coordinator's take reads the task as unreviewed again.
#[test]
fn a_settled_task_takes_what_comes_after_it_as_any_review_does() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle-after");
    let id = done_by_hand(&mut bench, "after");
    settle(&mut bench, &id);
    assert_eq!(stage(&bench, &id), WorkStage::NothingToLand);
    bench.json(&format!("task-take --task {id} --why again"));
    let run = &bench.ledger.runs()[0];
    let review = run.review_of(run.task(&id).expect("the task"));
    assert!(
        !review.verified && review.superseded_by.is_some(),
        "the settle's review dressed the attempt after it: {review:?}"
    );
}

/// The `--task` form changes the ledger and asks for a retry name; the bare
/// listing does not.
#[test]
fn settling_one_task_asks_for_a_retry_name_and_the_listing_does_not() {
    let argv = |line: &str| words(line);
    assert!(needs_a_retry_name(&argv(
        "task-settle --task t-1 --why done --report /r.md"
    )));
    assert!(!needs_a_retry_name(&argv("task-settle")));
    assert!(needs_a_retry_name(&argv("task-settle --apply")));
}
