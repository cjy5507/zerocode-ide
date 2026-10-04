//! t-34501 stage 2 (t-22105): the ledger holds a task's own record against git's and tells the
//! coordinator what is late, and tells a live worker that its branch ran away.
//!
//! The clock is an argument of the one function that decides, so every time below is a number the test
//! chose; and the git facts arrive as witnesses, so no repository is needed to say what git said.

use super::*;
use crate::orchestration::landing_watch::{
    CLEANABLE_AFTER_MS, ConflictCell, DRIFT_PER_DISPATCH_MAX, GitSays, HORIZON_MS, LandingWitness,
    MERGE_OVERDUE_MS, MERGED_UNRECORDED_GRACE_MS, RENOTIFY_MS, VERIFY_OVERDUE_MS,
};

const CHECKOUT: &str = "/r/checkout";
const HEAD: &str = "abc1234def5678abc1234def5678abc1234def56";

/// A task a seated worker handed in: the ids, and when the report landed.
struct Handed {
    task: String,
    worker: String,
    dispatch: String,
    pane: String,
    done_at: i64,
}

fn handed_in(bench: &mut Bench, checkout: &str, head: &str) -> Handed {
    if bench.ledger.runs().is_empty() {
        bench.json("run-create --name watch");
    }
    let task = bench.json("task-create --spec watch-task")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(
        bench.ledger.worker_seated(("team-1", &pane), checkout),
        "the worker was seated in its checkout"
    );
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
    Handed {
        task,
        worker,
        dispatch,
        pane,
        done_at: bench.clock,
    }
}

/// The coordinator's review, bound to the attempt and the commit it looked at. Answers the time it
/// was written.
fn review(bench: &mut Bench, handed: &Handed, keys: &str) -> i64 {
    bench.json(&format!(
        "task-update --task {} --result {{{keys}}} --attempt {} --source {HEAD}",
        handed.task, handed.dispatch
    ));
    bench.clock
}

fn witness(checkout: &str, head: &str, git: GitSays, occupied: bool) -> LandingWitness {
    LandingWitness {
        checkout: checkout.to_string(),
        branch: Some("wt/watch".to_string()),
        head: head.to_string(),
        compare_ref: "origin/main".to_string(),
        compare_oid: "c0ffee0000000000000000000000000000000001".to_string(),
        git,
        occupied,
    }
}

fn silent() -> Vec<LandingWitness> {
    vec![witness(CHECKOUT, HEAD, GitSays::Silent, false)]
}

fn news(bench: &Bench, kind: MessageKind) -> Vec<Message> {
    bench.ledger.runs()[0]
        .messages()
        .iter()
        .filter(|held| held.kind == kind)
        .cloned()
        .collect()
}

fn body(message: &Message) -> serde_json::Value {
    serde_json::from_str(message.body.as_str()).expect("a notice is JSON")
}

#[test]
fn a_report_nobody_verified_is_told_after_two_hours_once_and_again_after_six() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let seen = silent();
    let at = |after: i64| handed.done_at + after;
    assert_eq!(
        bench.ledger.landing_watch(&seen, at(VERIFY_OVERDUE_MS - 1)),
        0,
        "one millisecond short of two hours"
    );
    assert_eq!(bench.ledger.landing_watch(&seen, at(VERIFY_OVERDUE_MS)), 1);
    let told = news(&bench, MessageKind::LandingStalled);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].from, LEDGER_ITSELF);
    assert_eq!(told[0].to, bench.ledger.runs()[0].address());
    assert_eq!(told[0].task.as_deref(), Some(handed.task.as_str()));
    let said = body(&told[0]);
    assert_eq!(said["reason"], "no_review");
    assert_eq!(said["taskId"], handed.task.as_str());
    assert_eq!(said["workerId"], handed.worker.as_str());
    assert_eq!(said["dispatchId"], handed.dispatch.as_str());
    assert_eq!(said["checkout"], CHECKOUT);
    assert_eq!(said["branch"], "wt/watch");
    assert_eq!(said["head"], HEAD);
    assert_eq!(said["sinceMs"], handed.done_at);
    assert!(said["next"].as_str().is_some_and(|line| !line.is_empty()));

    // The same state is not told twice, and is told again six hours after the last telling.
    let told_at = at(VERIFY_OVERDUE_MS);
    assert_eq!(bench.ledger.landing_watch(&seen, told_at + 1), 0);
    assert_eq!(
        bench.ledger.landing_watch(&seen, told_at + RENOTIFY_MS - 1),
        0
    );
    assert_eq!(bench.ledger.landing_watch(&seen, told_at + RENOTIFY_MS), 1);
    assert_eq!(news(&bench, MessageKind::LandingStalled).len(), 2);
}

#[test]
fn a_verification_nobody_merged_is_told_two_hours_after_the_verification_not_the_report() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    // The coordinator took three hours to look; the clock for a merge starts when it did.
    let verified_at = review(&mut bench, &handed, "\"verified\":true");
    let seen = silent();
    // Report to verification is the first alert's business, and it has been answered.
    assert_eq!(
        bench
            .ledger
            .landing_watch(&seen, verified_at + MERGE_OVERDUE_MS - 1),
        0
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&seen, verified_at + MERGE_OVERDUE_MS),
        1
    );
    let told = news(&bench, MessageKind::LandingStalled);
    let said = body(&told[0]);
    assert_eq!(said["reason"], "no_merge");
    assert_eq!(said["sinceMs"], verified_at);
}

#[test]
fn a_verification_written_before_its_time_was_kept_stays_silent_rather_than_guess() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let verified_at = review(&mut bench, &handed, "\"verified\":true");
    // The row as an older window wrote it: a coordinator's review with no time on it.
    let run_id = bench.ledger.runs()[0].id.clone();
    let task = bench
        .ledger
        .run_mut(&run_id)
        .expect("the run")
        .tasks
        .iter_mut()
        .find(|held| held.id == handed.task)
        .expect("the task");
    if let Some(ResultAuthor::Coordinator { verified_ms, .. }) = task.result_author.as_mut() {
        *verified_ms = None;
    }
    let seen = silent();
    assert_eq!(
        bench
            .ledger
            .landing_watch(&seen, verified_at + 10 * MERGE_OVERDUE_MS),
        0,
        "no time to count from"
    );
}

#[test]
fn work_git_has_in_main_that_the_ledger_never_recorded_is_told_after_thirty_minutes() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let took = handed.done_at + 1_000;
    let seen = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Landed { at_ms: Some(took) },
        false,
    )];
    assert_eq!(
        bench
            .ledger
            .landing_watch(&seen, took + MERGED_UNRECORDED_GRACE_MS - 1),
        0
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&seen, took + MERGED_UNRECORDED_GRACE_MS),
        1
    );
    let said = body(&news(&bench, MessageKind::LandingStalled)[0]);
    assert_eq!(said["reason"], "merged_unrecorded");
    assert_eq!(said["sinceMs"], took);

    // A checkout that moved on since the report says nothing about the commit that was handed in.
    let mut other = Bench::new();
    let handed = handed_in(&mut other, CHECKOUT, HEAD);
    let elsewhere = vec![witness(
        CHECKOUT,
        "ffffffffffffffffffffffffffffffffffffffff",
        GitSays::Landed {
            at_ms: Some(handed.done_at),
        },
        false,
    )];
    assert_eq!(
        other
            .ledger
            .landing_watch(&elsewhere, handed.done_at + 3 * MERGED_UNRECORDED_GRACE_MS),
        0
    );
}

#[test]
fn a_merged_checkout_nobody_is_in_is_told_as_cleanable_after_an_hour() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let merged_at = review(
        &mut bench,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    let landed = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Landed {
            at_ms: Some(merged_at),
        },
        false,
    )];
    assert_eq!(
        bench
            .ledger
            .landing_watch(&landed, merged_at + CLEANABLE_AFTER_MS - 1),
        0
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&landed, merged_at + CLEANABLE_AFTER_MS),
        1
    );
    let said = body(&news(&bench, MessageKind::LandingStalled)[0]);
    assert_eq!(said["reason"], "cleanable");
    assert_eq!(said["checkout"], CHECKOUT);

    // A session in the folder, or a worker still in its pane, is not an orphan.
    let mut busy = Bench::new();
    let handed = handed_in(&mut busy, CHECKOUT, HEAD);
    let merged_at = review(
        &mut busy,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    let occupied = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Landed {
            at_ms: Some(merged_at),
        },
        true,
    )];
    assert_eq!(
        busy.ledger
            .landing_watch(&occupied, merged_at + 10 * CLEANABLE_AFTER_MS),
        0
    );
    let _ = handed.pane;
}

#[test]
fn a_ledger_that_says_merged_where_git_has_no_such_head_is_told_at_once() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let merged_at = review(
        &mut bench,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    let missing = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Unlanded {
            behind: Some(3),
            far_behind: false,
            conflict: Some(ConflictCell {
                total: 0,
                files: Vec::new(),
            }),
        },
        false,
    )];
    assert_eq!(
        bench.ledger.landing_watch(&missing, merged_at + 1),
        1,
        "no waiting"
    );
    let said = body(&news(&bench, MessageKind::LandingStalled)[0]);
    assert_eq!(said["reason"], "ledger_merged_git_not");
    // And when git does have it, the same ledger is not a disagreement at all.
    let mut agreeing = Bench::new();
    let handed = handed_in(&mut agreeing, CHECKOUT, HEAD);
    let merged_at = review(
        &mut agreeing,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    let landed = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Landed {
            at_ms: Some(merged_at),
        },
        true,
    )];
    assert_eq!(agreeing.ledger.landing_watch(&landed, merged_at + 1), 0);
}

#[test]
fn closed_unreviewable_failed_and_nothing_to_land_work_is_never_told() {
    // Closed: the coordinator ended the task after the report.
    let mut closed = Bench::new();
    let handed = handed_in(&mut closed, CHECKOUT, HEAD);
    closed.json(&format!(
        "task-update --task {} --status closed --closed-as outdated --why overtaken",
        handed.task
    ));
    assert_eq!(
        closed
            .ledger
            .landing_watch(&silent(), handed.done_at + 10 * VERIFY_OVERDUE_MS),
        0,
        "a closed task"
    );

    // A report that said ok:false is not work waiting for a review.
    let mut failed = Bench::new();
    failed.json("run-create --name watch");
    let task = failed.json("task-create --spec failing-task")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = failed.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(failed.ledger.worker_seated(("team-1", &pane), CHECKOUT));
    failed.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":false,\"head\":\"{HEAD}\"}}"),
    );
    assert_eq!(
        failed
            .ledger
            .landing_watch(&silent(), failed.clock + 10 * VERIFY_OVERDUE_MS),
        0,
        "a failed attempt"
    );

    // Verified, and the coordinator wrote that there is no code to land: research, a review, a design.
    let mut nothing = Bench::new();
    let handed = handed_in(&mut nothing, CHECKOUT, HEAD);
    let verified_at = review(
        &mut nothing,
        &handed,
        "\"verified\":true,\"nothingToLand\":true",
    );
    assert_eq!(
        nothing
            .ledger
            .landing_watch(&silent(), verified_at + 10 * MERGE_OVERDUE_MS),
        0,
        "nothing to land"
    );
}

#[test]
fn a_task_older_than_the_horizon_is_not_told_so_old_records_do_not_become_a_flood() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), handed.done_at + HORIZON_MS + VERIFY_OVERDUE_MS),
        0
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), handed.done_at + VERIFY_OVERDUE_MS),
        1,
        "inside the horizon it is told"
    );
}

fn running_away(
    behind: u32,
    conflict: Option<ConflictCell>,
    oid: &str,
    head: &str,
) -> LandingWitness {
    let mut seen = witness(
        CHECKOUT,
        head,
        GitSays::Unlanded {
            behind: Some(behind),
            far_behind: behind >= 20,
            conflict,
        },
        false,
    );
    seen.compare_oid = oid.to_string();
    seen
}

#[test]
fn a_live_worker_whose_branch_ran_away_is_woken_once_per_state_and_at_most_ten_times() {
    let mut bench = Bench::new();
    bench.json("run-create --name drift");
    let task = bench.json("task-create --spec drifting")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), CHECKOUT));
    let now = bench.clock + 1_000;
    let clash = Some(ConflictCell {
        total: 2,
        files: vec!["src/a.rs".to_string(), "ui/b.js".to_string()],
    });
    let seen = [running_away(70, clash.clone(), "oid-1", HEAD)];
    assert_eq!(bench.ledger.landing_watch(&seen, now), 1);
    let woken = news(&bench, MessageKind::BranchDrifted);
    assert_eq!(woken.len(), 1);
    assert_eq!(woken[0].from, LEDGER_ITSELF);
    assert_eq!(woken[0].to, worker_address(&worker));
    let said = body(&woken[0]);
    assert_eq!(said["behind"], 70);
    assert_eq!(said["compareRef"], "origin/main");
    assert_eq!(said["conflict"]["total"], 2);
    assert_eq!(said["conflict"]["files"][0], "src/a.rs");
    assert!(
        said["next"]
            .as_str()
            .is_some_and(|line| line.contains("fetch") && line.contains("gate")),
        "{said}"
    );

    // The same (head, compare ref) state is not told again.
    assert_eq!(bench.ledger.landing_watch(&seen, now + 1), 0);
    // A compare ref that moved is a new state; ten of them are the most one attempt hears.
    for index in 2..=DRIFT_PER_DISPATCH_MAX {
        let moved = [running_away(
            70 + index as u32,
            clash.clone(),
            &format!("oid-{index}"),
            HEAD,
        )];
        assert_eq!(
            bench.ledger.landing_watch(&moved, now + index as i64),
            1,
            "state {index}"
        );
    }
    let eleventh = [running_away(99, clash.clone(), "oid-11", HEAD)];
    assert_eq!(
        bench.ledger.landing_watch(&eleventh, now + 100),
        0,
        "the eleventh is not told"
    );
    assert_eq!(
        news(&bench, MessageKind::BranchDrifted).len(),
        DRIFT_PER_DISPATCH_MAX
    );

    // The worker moves its branch: a new head is a new state and the count starts again.
    let merged_main = [running_away(
        80,
        clash,
        "oid-12",
        "1111111111111111111111111111111111111111",
    )];
    assert_eq!(bench.ledger.landing_watch(&merged_main, now + 200), 1);
}

#[test]
fn a_branch_that_is_neither_far_behind_nor_clashing_wakes_nobody_and_nor_does_a_finished_worker() {
    let mut bench = Bench::new();
    bench.json("run-create --name drift");
    let task = bench.json("task-create --spec drifting")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), CHECKOUT));
    let now = bench.clock + 1_000;
    let clean = Some(ConflictCell {
        total: 0,
        files: Vec::new(),
    });
    // Behind by a few, merges clean: nothing to say. Not looked at: nothing to say either.
    assert_eq!(
        bench
            .ledger
            .landing_watch(&[running_away(5, clean.clone(), "oid-1", HEAD)], now),
        0
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&[running_away(5, None, "oid-2", HEAD)], now),
        0
    );
    // Far behind but clean: told. A clash alone, close behind: told.
    assert_eq!(
        bench
            .ledger
            .landing_watch(&[running_away(25, clean, "oid-3", HEAD)], now),
        1
    );
    let clash = Some(ConflictCell {
        total: 1,
        files: vec!["x.rs".to_string()],
    });
    assert_eq!(
        bench
            .ledger
            .landing_watch(&[running_away(2, clash.clone(), "oid-4", HEAD)], now),
        1
    );
    // The worker reports; it is no longer a branch that is being worked on.
    bench.json_at(&pane, "send --type worker_done --body {\"ok\":true}");
    assert_eq!(
        bench.ledger.landing_watch(
            &[running_away(90, clash, "oid-5", HEAD)],
            bench.clock + 1_000
        ),
        0
    );
}

#[test]
fn the_two_new_kinds_are_the_ledgers_own_voice_and_can_be_asked_for() {
    let mut bench = Bench::new();
    bench.json("run-create --name voice");
    for kind in ["landing_stalled", "branch_drifted"] {
        let refused = bench.run(&format!("send --type {kind} --body x"));
        assert_eq!(refused.reply.exit_code, 1, "a caller typed {kind}");
        assert!(
            refused.reply.stderr.contains("ledger's own"),
            "{}",
            refused.reply.stderr
        );
        let asked = bench.run(&format!("check --peek --types {kind}"));
        assert_eq!(asked.reply.exit_code, 0, "{}", asked.reply.stderr);
        assert_eq!(
            kind.parse::<MessageKind>().map(MessageKind::as_str),
            Ok(kind)
        );
    }
}

#[test]
fn nothing_to_land_is_the_coordinators_fact_and_a_workers_word_for_it_is_a_claim() {
    // A worker's result carrying the key: kept apart as a claim, and not a fact.
    let mut claim = Bench::new();
    claim.json("run-create --name watch");
    let task = claim.json("task-create --spec claimed-task")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = claim.seat(&format!("worker-start --agent claude --task {task}"));
    claim.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{HEAD}\",\"nothingToLand\":true}}"),
    );
    let run = &claim.ledger.runs()[0];
    let claimed = run.review_of(run.task(&task).expect("the task"));
    assert!(
        !claimed.nothing_to_land && claimed.claimed_nothing_to_land,
        "{claimed:?}"
    );

    // The coordinator's, bound to the attempt and the source it looked at, under either spelling.
    let facts = |bench: &Bench, task: &str| {
        let run = &bench.ledger.runs()[0];
        run.review_of(run.task(task).expect("the task"))
    };
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    review(
        &mut bench,
        &handed,
        "\"verified\":true,\"nothingToLand\":true",
    );
    let believed = facts(&bench, &handed.task);
    assert!(
        believed.verified && believed.nothing_to_land && !believed.merged,
        "{believed:?}"
    );
    let mut other = Bench::new();
    let handed_other = handed_in(&mut other, CHECKOUT, HEAD);
    review(
        &mut other,
        &handed_other,
        "\"verified\":true,\"noCodeChange\":true",
    );
    assert!(facts(&other, &handed_other.task).nothing_to_land);

    // It survives a rebuild from the stored rows.
    let carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    let run = &carried.runs()[0];
    assert!(
        run.review_of(run.task(&handed.task).expect("the task"))
            .nothing_to_land
    );
}

#[test]
fn the_time_of_a_verification_is_stamped_once_and_kept_by_a_rewrite_of_the_same_review() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let first = review(&mut bench, &handed, "\"verified\":true");
    let read = |bench: &Bench| {
        let run = &bench.ledger.runs()[0];
        run.review_of(run.task(&handed.task).expect("the task"))
            .verified_ms
    };
    assert_eq!(read(&bench), Some(first));
    // The same review written again with the merge added: still the first time it said verified.
    review(
        &mut bench,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    assert_eq!(
        read(&bench),
        Some(first),
        "a rewrite of the same review kept the time"
    );
    // And it is stored: a rebuild reads it back.
    let carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    let run = &carried.runs()[0];
    assert_eq!(
        run.review_of(run.task(&handed.task).expect("the task"))
            .verified_ms,
        Some(first)
    );
}

/// A new branch fast-forwarded to main has its head in main without a single commit of its own, and a
/// task that lands stage by stage has the ledger's `merged` only at the end: neither is a late
/// landing, because the commit git has is not the one the attempt handed in.
#[test]
fn only_the_commit_the_attempt_handed_in_counts_as_what_git_has() {
    // No report yet: the worker's checkout is a branch main fast-forwarded past, and git calls it landed.
    let mut fresh = Bench::new();
    fresh.json("run-create --name watch");
    let task = fresh.json("task-create --spec fresh-task")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = fresh.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(fresh.ledger.worker_seated(("team-1", &pane), CHECKOUT));
    let landed = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Landed { at_ms: Some(1) },
        false,
    )];
    assert_eq!(
        fresh
            .ledger
            .landing_watch(&landed, fresh.clock + 10 * MERGED_UNRECORDED_GRACE_MS),
        0,
        "an attempt that handed nothing in has no commit git could have"
    );

    // A stage of the work landed earlier: git has an older commit of the same branch.
    let mut staged = Bench::new();
    let handed = handed_in(&mut staged, CHECKOUT, HEAD);
    let earlier = vec![witness(
        CHECKOUT,
        "1111111111111111111111111111111111111111",
        GitSays::Landed {
            at_ms: Some(handed.done_at),
        },
        false,
    )];
    assert_eq!(
        staged
            .ledger
            .landing_watch(&earlier, handed.done_at + 3 * MERGED_UNRECORDED_GRACE_MS),
        0,
        "the stage that landed is not the commit that was handed in"
    );
    let ledger_says_merged = {
        let mut bench = Bench::new();
        let handed = handed_in(&mut bench, CHECKOUT, HEAD);
        let merged_at = review(
            &mut bench,
            &handed,
            &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
        );
        let unlanded_stage = vec![witness(
            CHECKOUT,
            "1111111111111111111111111111111111111111",
            GitSays::Unlanded {
                behind: Some(1),
                far_behind: false,
                conflict: None,
            },
            false,
        )];
        bench.ledger.landing_watch(&unlanded_stage, merged_at + 1)
    };
    assert_eq!(
        ledger_says_merged, 0,
        "git's word about another commit is not a disagreement"
    );
}

/// The alert's ledger is the rows themselves: the same moment twice, and a ledger read in again, tell
/// nothing a second time.
#[test]
fn a_notice_already_told_is_not_told_again_at_the_same_moment_or_after_a_rebuild() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let seen = silent();
    let at = handed.done_at + VERIFY_OVERDUE_MS;
    assert_eq!(bench.ledger.landing_watch(&seen, at), 1);
    assert_eq!(
        bench.ledger.landing_watch(&seen, at),
        0,
        "the same moment twice"
    );
    let mut carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    assert_eq!(
        carried.landing_watch(&seen, at + 1),
        0,
        "a ledger read in again"
    );
    assert_eq!(
        carried.landing_watch(&seen, at + RENOTIFY_MS),
        1,
        "and six hours later"
    );
}

/// A review an older window wrote has no time on it, and the row it left reads and writes back.
#[test]
fn a_coordinators_row_from_before_the_verification_time_reads_and_writes_back_without_it() {
    let old = r#"{"kind":"coordinator","seat":"team-1/%1","generation":1,"attempt":"dp-1","source":"abc1234","completed_ms":5}"#;
    let read: ResultAuthor = serde_json::from_str(old).expect("an old row");
    match &read {
        ResultAuthor::Coordinator {
            completed_ms,
            verified_ms,
            ..
        } => assert_eq!((*completed_ms, *verified_ms), (Some(5), None)),
        other => panic!("not a coordinator's row: {other:?}"),
    }
    let written = serde_json::to_value(&read).expect("written");
    assert!(written.get("verified_ms").is_none(), "{written}");
    assert_eq!(
        serde_json::from_value::<ResultAuthor>(written).expect("read back"),
        read
    );
}
