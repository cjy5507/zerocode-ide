//! t-78890 (O8, VERDICT F1): the landing of a stack written in one call. A coordinator that put a
//! stack on main writes `merged` and `mergeHead` — and, once released, `deployed` and the release —
//! on every member with one `task-update --members`, and the ledger binds each member's record to
//! the attempt and the source its standing review named, as a review written one by one would be.

use super::*;

/// The commit the stack landed as.
const MAIN: &str = "e5f60718293a4b5c6d7e8f901234567890123456";
/// What each worker handed in.
const FIRST: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const SECOND: &str = "b2c3d4e5f60718293a4b5c6d7e8f901234567890";

/// A task a worker handed `head` in on: the task and its attempt.
fn handed(bench: &mut Bench, head: &str) -> (String, String) {
    if bench.ledger.runs().is_empty() {
        bench.json("run-create --name stack");
    }
    let task = bench.json("task-create --spec stack-member")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let dispatch = bench.ledger.runs()[0]
        .dispatches
        .last()
        .expect("an attempt")
        .id
        .clone();
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"}}"),
    );
    (task, dispatch)
}

fn verified(bench: &mut Bench, task: &str, dispatch: &str, head: &str) {
    bench.json(&format!(
        "task-update --task {task} --result {{\"verified\":true,\"testedHead\":\"{head}\"}} \
         --attempt {dispatch} --source {head}"
    ));
}

fn facts(bench: &Bench, task: &str) -> ReviewFacts {
    let run = &bench.ledger.runs()[0];
    run.review_of(run.task(task).expect("the task"))
}

fn result_of(bench: &Bench, task: &str) -> serde_json::Value {
    let run = &bench.ledger.runs()[0];
    serde_json::from_str(run.task(task).expect("the task").result.as_str()).expect("a JSON result")
}

#[test]
fn a_stack_is_recorded_merged_on_every_member_in_one_call_bound_to_each_members_own_review() {
    let mut bench = Bench::new();
    let (one, one_attempt) = handed(&mut bench, FIRST);
    let (two, two_attempt) = handed(&mut bench, SECOND);
    verified(&mut bench, &one, &one_attempt, FIRST);
    verified(&mut bench, &two, &two_attempt, SECOND);

    let said = bench.json(&format!(
        "task-update --members {one},{two} --result {{\"merged\":true,\"mergeHead\":\"{MAIN}\"}}"
    ));
    let rows = said["recorded"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{said}");
    for ((task, attempt, head), row) in [(&one, &one_attempt, FIRST), (&two, &two_attempt, SECOND)]
        .into_iter()
        .zip(rows)
    {
        assert_eq!(row["taskId"], task.as_str());
        assert_eq!(row["attempt"], attempt.as_str());
        assert_eq!(row["source"], head);
        let now = facts(&bench, task);
        assert!(now.verified && now.merged, "{now:?}");
        assert_eq!(now.merge_head.as_deref(), Some(MAIN));
        assert_eq!(now.attempt.as_deref(), Some(attempt.as_str()));
        assert_eq!(now.source.as_deref(), Some(head));
        // The review's own words stay; the landing is added beside them.
        assert_eq!(result_of(&bench, task)["testedHead"], head);
        let run = &bench.ledger.runs()[0];
        assert!(
            run.completion_ms(run.task(task).expect("the task"))
                .is_some(),
            "the hand-in to main time the board measures"
        );
    }

    // The release, the same way: the landing stays, the shipping is added.
    let said = bench.json(&format!(
        "task-update --members {one},{two} --result {{\"deployed\":true,\"release\":\"v1.1.101\"}}"
    ));
    assert_eq!(said["recorded"].as_array().map(Vec::len), Some(2));
    for task in [&one, &two] {
        let now = facts(&bench, task);
        assert!(now.verified && now.merged && now.deployed, "{now:?}");
        assert_eq!(now.merge_head.as_deref(), Some(MAIN));
        assert_eq!(result_of(&bench, task)["release"], "v1.1.101");
    }
}

/// The hand-in to main time the landing stamps is the one `dispatch-show` says (t-78890 r2, review
/// P2-1): `afterReport.mergedMs` is the moment `--members` first wrote `merged`, kept when the
/// release is added, and a task nobody recorded as merged says none.
#[test]
fn dispatch_show_says_when_the_landing_was_recorded_and_nothing_for_a_task_not_recorded() {
    let mut bench = Bench::new();
    let (one, one_attempt) = handed(&mut bench, FIRST);
    let (two, two_attempt) = handed(&mut bench, SECOND);
    verified(&mut bench, &one, &one_attempt, FIRST);
    verified(&mut bench, &two, &two_attempt, SECOND);
    let merged_ms = |bench: &mut Bench, task: &str| {
        bench.json(&format!("dispatch-show --task {task}"))["time"]["task"]["afterReport"]
            ["mergedMs"]
            .clone()
    };
    assert_eq!(merged_ms(&mut bench, &one), serde_json::Value::Null);

    bench.json(&format!(
        "task-update --members {one} --result {{\"merged\":true,\"mergeHead\":\"{MAIN}\"}}"
    ));
    let landed_at = bench.clock;
    let stamped = {
        let run = &bench.ledger.runs()[0];
        run.completion_ms(run.task(&one).expect("the task"))
    };
    assert_eq!(stamped, Some(landed_at), "the write that said merged");
    assert_eq!(merged_ms(&mut bench, &one), landed_at);
    assert_eq!(
        merged_ms(&mut bench, &two),
        serde_json::Value::Null,
        "verified and never recorded as merged"
    );

    bench.json(&format!(
        "task-update --members {one} --result {{\"deployed\":true,\"release\":\"v1.1.101\"}}"
    ));
    assert!(bench.clock > landed_at);
    assert_eq!(
        merged_ms(&mut bench, &one),
        landed_at,
        "the release keeps the landing moment"
    );
    let shown = bench.json(&format!("dispatch-show --task {one}"));
    assert_eq!(
        shown["time"]["task"]["afterReport"]["deployedMs"],
        serde_json::Value::Null,
        "the ledger stamps no release moment"
    );
}

/// One member that cannot take the record refuses the whole call, each reason on its own line,
/// and nothing is written: a stack is recorded whole or not at all.
#[test]
fn a_member_with_nothing_handed_in_or_no_verified_review_refuses_the_call_and_writes_nothing() {
    let mut bench = Bench::new();
    let (one, one_attempt) = handed(&mut bench, FIRST);
    verified(&mut bench, &one, &one_attempt, FIRST);
    let (unreviewed, _) = handed(&mut bench, SECOND);
    let carrying = bench.json("task-create --spec still-carried")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    bench.seat(&format!("worker-start --agent claude --task {carrying}"));
    let before = result_of(&bench, &one);

    let refused = bench.run(&format!(
        "task-update --members {one},{unreviewed},{carrying},t-999999 --result \
         {{\"merged\":true,\"mergeHead\":\"{MAIN}\"}}"
    ));
    assert_ne!(refused.reply.exit_code, 0);
    let why = refused.reply.stderr.as_str();
    let lines: Vec<&str> = why.lines().filter(|line| line.starts_with("t-")).collect();
    assert_eq!(
        lines.len(),
        3,
        "one line for each member that cannot take it: {why}"
    );
    assert!(why.contains(&format!("{unreviewed}: ")), "{why}");
    assert!(why.contains("no verified review"), "{why}");
    assert!(why.contains(&format!("{carrying}: ")), "{why}");
    assert!(why.contains("handed nothing in"), "{why}");
    assert!(why.contains("t-999999: unknown task"), "{why}");
    assert_eq!(result_of(&bench, &one), before, "nothing was written");
    assert!(!facts(&bench, &one).merged);
}

/// `--members` writes a landing and nothing else: a review is of one hand-in and is written one by
/// one, so a verification, a status or a named attempt is refused here.
#[test]
fn members_take_landing_keys_only() {
    let mut bench = Bench::new();
    let (one, one_attempt) = handed(&mut bench, FIRST);
    verified(&mut bench, &one, &one_attempt, FIRST);
    for line in [
        format!("task-update --members {one} --result {{\"verified\":true,\"merged\":true}}"),
        format!("task-update --members {one} --result {{\"release\":\"v1\"}}"),
        format!("task-update --members {one} --result {{\"note\":\"x\"}}"),
        format!("task-update --members {one} --status completed"),
        format!("task-update --members {one} --attempt {one_attempt} --result {{\"merged\":true}}"),
        format!("task-update --members {one} --task {one} --result {{\"merged\":true}}"),
    ] {
        let refused = bench.run(&line);
        assert_ne!(refused.reply.exit_code, 0, "`{line}` was taken");
    }
    assert!(!facts(&bench, &one).merged);
}
