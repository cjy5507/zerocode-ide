//! t-34501 stage 2 (t-22105): the ledger holds a task's own record against git's and tells the
//! coordinator what is late, and tells a live worker that its branch ran away.
//!
//! The clock is an argument of the one function that decides, so every time below is a number the test
//! chose; and the git facts arrive as witnesses, so no repository is needed to say what git said.

use super::*;
use crate::orchestration::landing_watch::{
    CLEANABLE_AFTER_MS, ConflictCell, DRIFT_PER_DISPATCH_MAX, GitSays, HORIZON_MS, LandingWitness,
    MERGE_OVERDUE_MS, MERGED_UNRECORDED_GRACE_MS, RENOTIFY_MS, REVIEW_BY_KIND, REVIEW_BY_TITLE,
    REVIEWED_HEAD_KEY, VERIFY_OVERDUE_MS,
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
        beside: Vec::new(),
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

/// A coordinator who looked and wrote `verified: false` with a reason has reviewed the hand-in:
/// "nobody verified it" is not true of it, and it is not told every six hours until the horizon
/// (t-44451, t-54222).
#[test]
fn a_review_that_said_no_is_not_told_as_no_review() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    review(
        &mut bench,
        &handed,
        "\"verified\":false,\"reason\":\"fails\"",
    );
    let facts = {
        let run = &bench.ledger.runs()[0];
        run.review_of(run.task(&handed.task).expect("the task"))
    };
    assert!(facts.written && !facts.verified, "{facts:?}");
    let seen = silent();
    for after in [VERIFY_OVERDUE_MS, VERIFY_OVERDUE_MS + RENOTIFY_MS] {
        assert_eq!(
            bench.ledger.landing_watch(&seen, handed.done_at + after),
            0,
            "a review that said no is a review"
        );
    }
    assert!(news(&bench, MessageKind::LandingStalled).is_empty());
}

/// The review is of one hand-in. A new attempt, or a new commit on the same attempt, is a hand-in
/// nobody has reviewed, and it is told again (t-44451).
#[test]
fn a_new_hand_in_after_a_review_that_said_no_is_told_as_no_review() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    review(
        &mut bench,
        &handed,
        "\"verified\":false,\"reason\":\"fails\"",
    );
    bench.json(&format!(
        "task-update --task {} --status ready",
        handed.task
    ));
    let (_, pane) = bench.seat(&format!(
        "worker-start --agent claude --task {}",
        handed.task
    ));
    let second = bench.ledger.runs()[0]
        .dispatches
        .last()
        .expect("a second attempt")
        .id
        .clone();
    assert_ne!(second, handed.dispatch);
    bench.json_at(
        &pane,
        "send --type worker_done --body {\"ok\":true,\"head\":\"def5678def5678def5678def5678def5678def5\"}",
    );
    let second_done = bench.clock;
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), second_done + VERIFY_OVERDUE_MS),
        1
    );
    let told = news(&bench, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "no_review");
    assert_eq!(body(&told[0])["dispatchId"], second.as_str());

    // The same attempt hands in another commit: the review named the first one.
    let mut same = Bench::new();
    let handed = handed_in(&mut same, CHECKOUT, HEAD);
    review(
        &mut same,
        &handed,
        "\"verified\":false,\"reason\":\"fails\"",
    );
    let mut moved = same.ledger.export();
    moved
        .dispatches
        .iter_mut()
        .find(|row| row.id == handed.dispatch)
        .expect("the attempt")
        .source = Some("123abcd123abcd123abcd123abcd123abcd123ab".to_string());
    same.ledger = Ledger::rebuild(moved).expect("a readable changed hand-in");
    assert_eq!(
        same.ledger
            .landing_watch(&silent(), handed.done_at + VERIFY_OVERDUE_MS),
        1
    );
    let told = news(&same, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "no_review");
    assert_eq!(
        body(&told[0])["head"],
        "123abcd123abcd123abcd123abcd123abcd123ab"
    );
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

/// No witness is no word from git: the window gives none for a removed folder, or for an answer that
/// stood on a compare oid it no longer holds as today's (t-44451). Read as nothing, it never makes a
/// landing, a disagreement or a folder to clean up.
#[test]
fn no_witness_is_never_read_as_landed_unlanded_or_cleanable() {
    let reasons = |bench: &Bench| {
        news(bench, MessageKind::LandingStalled)
            .iter()
            .map(|told| body(told)["reason"].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
    };
    let mut merged = Bench::new();
    let handed = handed_in(&mut merged, CHECKOUT, HEAD);
    let merged_at = review(
        &mut merged,
        &handed,
        &format!("\"verified\":true,\"mergeHead\":\"{HEAD}\""),
    );
    assert_eq!(
        merged
            .ledger
            .landing_watch(&[], merged_at + 10 * CLEANABLE_AFTER_MS),
        0
    );
    assert!(reasons(&merged).is_empty(), "{:?}", reasons(&merged));

    let mut verified = Bench::new();
    let handed = handed_in(&mut verified, CHECKOUT, HEAD);
    let verified_at = review(&mut verified, &handed, "\"verified\":true");
    verified
        .ledger
        .landing_watch(&[], verified_at + 10 * MERGED_UNRECORDED_GRACE_MS);
    assert_eq!(reasons(&verified), ["no_merge"]);
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

/// A worker that handed in with other checkouts of its task still standing beside its own is told
/// to the coordinator once, at the hand-in, with their paths (t-45336) — the rule is one checkout per
/// task — and a hand-in with nothing beside it hears nothing of the kind.
#[test]
fn checkouts_left_beside_a_hand_in_are_told_once_with_their_paths() {
    let mut bench = Bench::new();
    let handed = handed_in(&mut bench, CHECKOUT, HEAD);
    let mut seen = witness(CHECKOUT, HEAD, GitSays::Silent, false);
    seen.beside = vec![
        "/r/checkout-red".to_string(),
        "/r/checkout-verify".to_string(),
    ];
    let left = |bench: &Bench| {
        news(bench, MessageKind::LandingStalled)
            .iter()
            .map(body)
            .filter(|said| said["reason"] == "checkouts_left")
            .collect::<Vec<_>>()
    };
    assert_eq!(
        bench
            .ledger
            .landing_watch(std::slice::from_ref(&seen), handed.done_at + 1),
        1
    );
    let told = left(&bench);
    assert_eq!(told.len(), 1, "the left checkouts are told at the hand-in");
    assert_eq!(
        told[0]["beside"],
        serde_json::json!(["/r/checkout-red", "/r/checkout-verify"])
    );
    assert_eq!(told[0]["taskId"], handed.task.as_str());
    assert!(
        told[0]["next"]
            .as_str()
            .is_some_and(|line| line.contains("one checkout per task")),
        "the notice names the rule: {}",
        told[0]["next"]
    );
    // Once: not a minute later, and not six hours later either, while the other reasons keep their
    // own pace.
    bench
        .ledger
        .landing_watch(std::slice::from_ref(&seen), handed.done_at + 60_000);
    bench.ledger.landing_watch(
        std::slice::from_ref(&seen),
        handed.done_at + RENOTIFY_MS + VERIFY_OVERDUE_MS + 1,
    );
    assert_eq!(left(&bench).len(), 1, "a left checkout is told once");

    let mut other = Bench::new();
    let handed = handed_in(&mut other, CHECKOUT, HEAD);
    assert_eq!(
        other.ledger.landing_watch(&silent(), handed.done_at + 1),
        0,
        "nothing beside the checkout, nothing told"
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

/// A task handed in as a review: what it handed in, as the ledger named it (the report's message
/// id), and the ids. `title` and `payload` are what the coordinator and the worker wrote.
fn review_handed_in(bench: &mut Bench, title: &str, payload: &str, head: Option<&str>) -> Handed {
    if bench.ledger.runs().is_empty() {
        bench.json("run-create --name watch");
    }
    let task = bench.json(&format!("task-create --spec review-task --title {title}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), CHECKOUT));
    let dispatch = bench.ledger.runs()[0]
        .dispatches
        .last()
        .expect("an attempt")
        .id
        .clone();
    let said = match head {
        Some(head) => format!("{{\"ok\":true,\"head\":\"{head}\"}}"),
        None => "{\"ok\":true,\"summary\":\"two-findings\"}".to_string(),
    };
    let payload = match payload {
        "" => String::new(),
        given => format!(" --payload {given}"),
    };
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {said}{payload}"),
    );
    Handed {
        task,
        worker,
        dispatch,
        pane,
        done_at: bench.clock,
    }
}

/// What the attempt handed in, as the ledger named it.
fn handed_source(bench: &Bench, handed: &Handed) -> String {
    bench.ledger.runs()[0]
        .dispatch(&handed.dispatch)
        .and_then(|held| held.source.clone())
        .expect("a hand-in")
}

/// The coordinator's `verified: true` of a review hand-in, against what it handed in. Answers the
/// verb's answer and the time it was written.
fn verify_review(bench: &mut Bench, handed: &Handed) -> (serde_json::Value, i64) {
    let source = handed_source(bench, handed);
    let said = bench.json(&format!(
        "task-update --task {} --result {{\"verified\":true}} --attempt {} --source {source}",
        handed.task, handed.dispatch
    ));
    (said, bench.clock)
}

/// O8 (t-78890, VERDICT F1): two of the six landing notices at 09:30 on 10-28 were review tasks —
/// read-only work whose hand-in is a report, not a commit — told `no_merge` two hours after the
/// coordinator verified them. A hand-in whose worker said `reportKind: review` has nothing to land:
/// it is never told as unlanded, and the coordinator's verification of it carries `nothingToLand`
/// without the coordinator having to remember the key.
#[test]
fn a_review_hand_in_is_never_told_as_unlanded_and_its_verification_says_nothing_to_land() {
    let mut bench = Bench::new();
    let handed = review_handed_in(
        &mut bench,
        "t-1-review",
        "{\"reportKind\":\"review\"}",
        None,
    );
    let (said, verified_at) = verify_review(&mut bench, &handed);
    assert_eq!(said["nothingToLandBecause"], "reportKind review", "{said}");
    let facts = {
        let run = &bench.ledger.runs()[0];
        run.review_of(run.task(&handed.task).expect("the task"))
    };
    assert!(
        facts.verified && facts.nothing_to_land && !facts.merged,
        "{facts:?}"
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), verified_at + 10 * MERGE_OVERDUE_MS),
        0,
        "a review hand-in told as unlanded: {:?}",
        news(&bench, MessageKind::LandingStalled)
    );
}

/// The watch reads the hand-in itself, not only the key the verification wrote: a review hand-in
/// whose verification says nothing of landing — one an older window wrote, before the key was added
/// — is still not told as unlanded.
#[test]
fn a_review_hand_in_verified_without_a_word_on_landing_is_not_told_as_unlanded_either() {
    let mut bench = Bench::new();
    let handed = review_handed_in(
        &mut bench,
        "t-1-review",
        "{\"reportKind\":\"review\"}",
        None,
    );
    let (_, verified_at) = verify_review(&mut bench, &handed);
    let run_id = bench.ledger.runs()[0].id.clone();
    let task = bench
        .ledger
        .run_mut(&run_id)
        .expect("the run")
        .tasks
        .iter_mut()
        .find(|held| held.id == handed.task)
        .expect("the task");
    task.result = Text::from("{\"verified\":true}");
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), verified_at + MERGE_OVERDUE_MS),
        0
    );
}

/// The coordinator's word on landing stands over the worker's `reportKind` (t-79642, review t-79586
/// P3-1) — as it does when the verification is written, where `nothingToLand` is added only if the
/// result says nothing of landing. A review verified with `nothingToLand: false` is code the
/// coordinator means to land, and is told `no_merge`; one it wrote `merged` on that git does not
/// hold is told `ledger_merged_git_not`, as any merged claim git denies is.
#[test]
fn a_coordinators_word_on_a_review_hand_ins_landing_stands_over_the_workers_report_kind() {
    let mut said_false = Bench::new();
    let handed = review_handed_in(
        &mut said_false,
        "t-1-review",
        "{\"reportKind\":\"review\"}",
        None,
    );
    let source = handed_source(&said_false, &handed);
    said_false.json(&format!(
        "task-update --task {} --result {{\"verified\":true,\"nothingToLand\":false}} --attempt {} \
         --source {source}",
        handed.task, handed.dispatch
    ));
    let verified_at = said_false.clock;
    assert_eq!(
        said_false
            .ledger
            .landing_watch(&silent(), verified_at + MERGE_OVERDUE_MS),
        1
    );
    let told = news(&said_false, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "no_merge");

    let mut said_merged = Bench::new();
    let handed = review_handed_in(
        &mut said_merged,
        "stack-pass",
        "{\"reportKind\":\"review\"}",
        Some(HEAD),
    );
    said_merged.json(&format!(
        "task-update --task {} --result {{\"verified\":true,\"mergeHead\":\"{HEAD}\"}} \
         --attempt {} --source {HEAD}",
        handed.task, handed.dispatch
    ));
    let merged_at = said_merged.clock;
    let missing = vec![witness(
        CHECKOUT,
        HEAD,
        GitSays::Unlanded {
            behind: None,
            far_behind: false,
            conflict: None,
        },
        false,
    )];
    assert_eq!(said_merged.ledger.landing_watch(&missing, merged_at + 1), 1);
    let told = news(&said_merged, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "ledger_merged_git_not");
}

/// The title the coordinator gave the task — 「검수」, 「읽기 전용」 — says the same thing of a hand-in
/// that names no commit (both 09:30 notices handed in a report id, `m-78620`). A task so titled that
/// handed in a commit is code waiting to land and is told as before: the title alone never hides
/// a commit.
#[test]
fn a_read_only_title_marks_a_report_hand_in_as_a_review_but_never_a_commit() {
    let mut report = Bench::new();
    let handed = review_handed_in(
        &mut report,
        "t-78356-검수-(Fable-high,-읽기-전용)",
        "",
        None,
    );
    let (said, verified_at) = verify_review(&mut report, &handed);
    assert_eq!(said["nothingToLandBecause"], "read-only title", "{said}");
    assert_eq!(
        report
            .ledger
            .landing_watch(&silent(), verified_at + MERGE_OVERDUE_MS),
        0
    );

    let mut commit = Bench::new();
    let handed = review_handed_in(&mut commit, "검수-P2-고침", "", Some(HEAD));
    let (said, verified_at) = verify_review(&mut commit, &handed);
    assert!(said["nothingToLandBecause"].is_null(), "{said}");
    assert_eq!(
        commit
            .ledger
            .landing_watch(&silent(), verified_at + MERGE_OVERDUE_MS),
        1
    );
    let told = news(&commit, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "no_merge");
}

/// A review nobody has read is still told: `no_review` is about the coordinator's reading, not
/// about landing, and a review waiting two hours for its reader is the stall F4 counted.
#[test]
fn a_review_hand_in_nobody_read_is_still_told_as_no_review() {
    let mut bench = Bench::new();
    let handed = review_handed_in(
        &mut bench,
        "t-1-review",
        "{\"reportKind\":\"review\"}",
        None,
    );
    assert_eq!(
        bench
            .ledger
            .landing_watch(&silent(), handed.done_at + VERIFY_OVERDUE_MS),
        1
    );
    let told = news(&bench, MessageKind::LandingStalled);
    assert_eq!(body(&told[0])["reason"], "no_review");
}

/// A title says review by a word, never by a piece of one (t-78890 r2, review P2-2): 「preview」 and
/// 「reviewer」 hold the letters of 「review」 and are not it, so a report handed in on such a task is
/// work waiting to land and is told as before. A mark is a word wherever the title breaks it —
/// 「읽기-전용」, 「read only」 — and a Korean mark takes the particle written after it.
#[test]
fn a_title_says_review_by_a_whole_word_and_never_by_a_piece_of_one() {
    for title in [
        "artifact-preview-구현",
        "reviewer-picker",
        "previewed-audits",
        "재검수표-정리",
    ] {
        let mut bench = Bench::new();
        let handed = review_handed_in(&mut bench, title, "", None);
        let (said, verified_at) = verify_review(&mut bench, &handed);
        assert!(said["nothingToLandBecause"].is_null(), "{title}: {said}");
        assert_eq!(
            bench
                .ledger
                .landing_watch(&silent(), verified_at + MERGE_OVERDUE_MS),
            1,
            "{title}"
        );
        let told = news(&bench, MessageKind::LandingStalled);
        assert_eq!(body(&told[0])["reason"], "no_merge", "{title}");
    }
    for title in [
        "T-1-Review",
        "audit:-the-stack",
        "검수를-마친-보고",
        "읽기_전용으로-본-것",
        "a-READ-ONLY-pass",
    ] {
        let mut bench = Bench::new();
        let handed = review_handed_in(&mut bench, title, "", None);
        let (said, _) = verify_review(&mut bench, &handed);
        assert_eq!(
            said["nothingToLandBecause"], REVIEW_BY_TITLE,
            "{title}: {said}"
        );
    }
}

/// A review worker hands in the head it reviewed — someone else's commit (t-78392, t-78487,
/// t-78512 and t-78521 on 10-28). With `reportKind: review` that hand-in is still a review, whatever
/// the title: it is never told as unlanded, and its verification writes the commit as the one
/// reviewed, beside `nothingToLand`, rather than as a head to land.
#[test]
fn a_review_that_hands_in_the_head_it_reviewed_is_a_review_and_names_that_head_as_reviewed() {
    let mut bench = Bench::new();
    let handed = review_handed_in(
        &mut bench,
        "stack-pass",
        "{\"reportKind\":\"review\"}",
        Some(HEAD),
    );
    let (said, verified_at) = verify_review(&mut bench, &handed);
    assert_eq!(said["nothingToLandBecause"], REVIEW_BY_KIND, "{said}");
    let (facts, result) = {
        let run = &bench.ledger.runs()[0];
        let task = run.task(&handed.task).expect("the task");
        (
            run.review_of(task),
            serde_json::from_str::<serde_json::Value>(task.result.as_str()).expect("a JSON result"),
        )
    };
    assert!(
        facts.verified && facts.nothing_to_land && !facts.merged && facts.merge_head.is_none(),
        "{facts:?}"
    );
    assert_eq!(result[REVIEWED_HEAD_KEY], HEAD, "{result}");
    assert_eq!(
        bench.ledger.landing_watch(
            &[witness(
                CHECKOUT,
                HEAD,
                GitSays::Unlanded {
                    behind: None,
                    far_behind: false,
                    conflict: None,
                },
                false,
            )],
            verified_at + 10 * MERGE_OVERDUE_MS,
        ),
        0,
        "a reviewed head told as unlanded: {:?}",
        news(&bench, MessageKind::LandingStalled)
    );
}
