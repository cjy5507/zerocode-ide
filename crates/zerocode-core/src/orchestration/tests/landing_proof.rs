//! t-119569 (ruling §4): a landing written without the person's hand — `task-backfill` proves
//! the merge and the release of verified work from git alone, and writes nothing it cannot prove.
//!
//! Merged only when every commit of the range the coordinator's review read is in the base as the
//! same change — landed as it is, or picked onto the base as copies — with the commit the last of
//! them landed as. Deployed only when a published release tag holds that landing. The ledger never
//! writes the verification: the review that stands is the coordinator's, and the landing is bound
//! to it as `task-update --members` binds one.

use super::*;
use crate::orchestration::coordinator_take::{self, BaseSeen, CommitsSeen, RangeSeen};
use crate::orchestration::evidence::ReceiptFiles;

const MAIN: &str = "e5f60718293a4b5c6d7e8f901234567890123456";
const REPO: &str = "/Users/dev/repo";
/// The reviewed head and its range's two commits.
const HEAD: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const FIRST: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
/// The newest copy of the range on `main`: what the landing is written as.
const LANDED_AS: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// A task a worker handed `head` in on, and the coordinator's verified review of it.
fn verified(bench: &mut Bench, spec: &str, head: &str) -> String {
    let id = task_of(bench, spec);
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"}}"),
    );
    let attempt = bench.ledger.runs()[0]
        .newest_attempt(&id)
        .expect("the attempt")
        .id
        .clone();
    bench.json(&format!(
        "task-update --task {id} --attempt {attempt} --source {head} --result {{\"verified\":true}}"
    ));
    id
}

/// What git says in the bench's repository about the range `HEAD` reviewed, against `main`.
fn git_says(range: RangeSeen) -> CommitsSeen {
    let mut seen = CommitsSeen {
        repo: REPO.to_string(),
        base: Some(BaseSeen {
            name: "main".to_string(),
            head: Some(MAIN.to_string()),
        }),
        tags: Some("v*".to_string()),
        ..CommitsSeen::default()
    };
    seen.insert_range(HEAD, range);
    seen
}

/// Picked onto `main` as copies, every one of the range's commits, the last of them in a release.
fn landed_and_released() -> RangeSeen {
    RangeSeen {
        commits: vec![(FIRST.to_string(), true), (HEAD.to_string(), true)],
        landed_as: Some(LANDED_AS.to_string()),
        tag: Some("v1.2.0".to_string()),
    }
}

fn backfill(bench: &mut Bench, line: &str, seen: CommitsSeen) -> serde_json::Value {
    let receipts = ReceiptFiles::default().with_commits(Some(seen));
    let planned = bench.at_argv_with(agent_teams::LEADER_PANE, words(line), &receipts);
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    serde_json::from_str(&planned.reply.stdout).expect("JSON")
}

fn landing_row(listed: &serde_json::Value, task: &str) -> serde_json::Value {
    listed["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["taskId"] == task && row["landing"] == true)
        .unwrap_or_else(|| panic!("{task} has no landing row: {listed}"))
        .clone()
}

fn review(bench: &Bench, task: &str) -> ReviewFacts {
    let run = &bench.ledger.runs()[0];
    run.review_of(run.task(task).expect("the task"))
}

/// A range picked onto `main` as copies is landed: the backfill lists the merge, as the newest
/// copy, and the release that holds it, then writes both bound to the coordinator's own review —
/// which stays the coordinator's, verified on the same attempt and source.
#[test]
fn a_range_picked_onto_main_lands_and_ships_on_the_review_that_stands() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing");
    let task = verified(&mut bench, "picked", HEAD);
    let before = review(&bench, &task);
    assert!(before.verified && !before.merged, "{before:?}");

    let listed = backfill(&mut bench, "task-backfill", git_says(landed_and_released()));
    let row = landing_row(&listed, &task);
    assert_eq!(row["stage"], "verified", "{row}");
    assert_eq!(row["write"]["merged"], true, "{row}");
    assert_eq!(row["write"]["mergeHead"], LANDED_AS, "{row}");
    assert_eq!(row["write"]["deployed"], true, "{row}");
    assert_eq!(row["write"]["release"], "v1.2.0", "{row}");
    assert_eq!(
        row["write"].get("verified"),
        None,
        "the ledger writes no verification: {row}"
    );
    assert_eq!(row["why"], serde_json::Value::Null, "{row}");
    assert_eq!(listed["landings"], 1, "{listed}");
    assert!(!review(&bench, &task).merged, "a list wrote something");

    let applied = backfill(
        &mut bench,
        "task-backfill --apply",
        git_says(landed_and_released()),
    );
    assert!(
        applied["applied"]
            .as_array()
            .is_some_and(|done| done.iter().any(|one| one == task.as_str())),
        "{applied}"
    );
    let after = review(&bench, &task);
    assert!(
        after.verified && after.merged && after.deployed,
        "{after:?}"
    );
    assert_eq!(after.merge_head.as_deref(), Some(LANDED_AS));
    assert_eq!(after.author, ReviewAuthor::Coordinator);
    assert_eq!(
        after.attempt, before.attempt,
        "bound to the attempt the review read"
    );
    assert_eq!(
        after.source, before.source,
        "bound to the source the review read"
    );
    let run = &bench.ledger.runs()[0];
    let result: serde_json::Value =
        serde_json::from_str(run.task(&task).expect("the task").result.as_str()).expect("JSON");
    assert_eq!(result["release"], "v1.2.0", "{result}");
    assert_eq!(result["backfill"]["by"], "task-backfill", "{result}");
    assert_eq!(
        result["backfill"]["landing"]["facts"]["mergeHead"], LANDED_AS,
        "{result}"
    );
}

/// A note an earlier backfill left — what it replaced, the original it kept — survives the
/// landing written beside it, and the landing time stays the backfill's, out of the task's cost.
#[test]
fn a_landing_keeps_what_an_earlier_backfill_kept() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-kept");
    let task = task_of(&mut bench, "kept");
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{HEAD}\"}}"),
    );
    let attempt = bench.ledger.runs()[0]
        .newest_attempt(&task)
        .expect("the attempt")
        .id
        .clone();
    bench.json(&format!(
        "task-update --task {task} --attempt {attempt} --source {HEAD} --result \
         {{\"verified\":true,\"backfill\":{{\"by\":\"task-backfill\",\"original\":\"was\"}}}}"
    ));
    backfill(
        &mut bench,
        "task-backfill --apply",
        git_says(landed_and_released()),
    );
    let run = &bench.ledger.runs()[0];
    let result: serde_json::Value =
        serde_json::from_str(run.task(&task).expect("the task").result.as_str()).expect("JSON");
    assert_eq!(result["backfill"]["original"], "was", "{result}");
    assert_eq!(
        result["backfill"]["landing"]["facts"]["release"], "v1.2.0",
        "{result}"
    );
    assert!(coordinator_take::backfilled(
        run.task(&task).expect("the task")
    ));
}

/// One commit of the range missing from `main` is no landing: the row says which, and nothing is
/// written — however many of the others landed.
#[test]
fn a_range_with_one_commit_missing_from_main_writes_nothing_and_says_which() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-partial");
    let task = verified(&mut bench, "partial", HEAD);
    let partial = RangeSeen {
        commits: vec![(FIRST.to_string(), true), (HEAD.to_string(), false)],
        landed_as: None,
        tag: None,
    };
    let listed = backfill(&mut bench, "task-backfill --apply", git_says(partial));
    let row = landing_row(&listed, &task);
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    let why = row["why"].as_str().expect("why it cannot");
    assert!(
        why.contains("1 of 2 commits") && why.contains(HEAD) && !why.contains(FIRST),
        "{why}"
    );
    let after = review(&bench, &task);
    assert!(
        after.verified && !after.merged && !after.deployed,
        "{after:?}"
    );
}

/// A release tag cut before the merge holds none of it: the merge is written, the release is not.
#[test]
fn a_tag_cut_before_the_merge_ships_nothing() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-tag");
    let task = verified(&mut bench, "untagged", HEAD);
    let landed = RangeSeen {
        tag: None,
        ..landed_and_released()
    };
    let listed = backfill(&mut bench, "task-backfill --apply", git_says(landed));
    let row = landing_row(&listed, &task);
    assert_eq!(row["write"]["merged"], true, "{row}");
    assert_eq!(row["write"].get("deployed"), None, "{row}");
    let after = review(&bench, &task);
    assert!(after.merged && !after.deployed, "{after:?}");

    // Once a published release holds the landing, the landed work is listed again for it alone —
    // asked by the commit its merge names, which `main` holds as it is.
    let mut released = git_says(RangeSeen::default());
    released.insert_range(
        LANDED_AS,
        RangeSeen {
            commits: vec![(LANDED_AS.to_string(), true)],
            landed_as: Some(LANDED_AS.to_string()),
            tag: Some("v1.2.0".to_string()),
        },
    );
    let shipped = backfill(&mut bench, "task-backfill", released);
    let row = landing_row(&shipped, &task);
    assert_eq!(row["stage"], "merged", "{row}");
    assert_eq!(row["write"]["deployed"], true, "{row}");
    assert_eq!(row["write"].get("merged"), None, "{row}");
}

/// Without git there is no landing to prove, and work no coordinator verified is never listed:
/// the ledger does not write a verification, and a landing stands only on one.
#[test]
fn nothing_is_proven_without_git_and_unverified_work_is_never_listed() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-blind");
    let task = verified(&mut bench, "blind", HEAD);
    let unverified = task_of(&mut bench, "reported");
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {unverified}"));
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{FIRST}\"}}"),
    );
    let receipts = ReceiptFiles::default().with_commits(None);
    let planned = bench.at_argv_with(
        agent_teams::LEADER_PANE,
        words("task-backfill --apply"),
        &receipts,
    );
    let listed: serde_json::Value = serde_json::from_str(&planned.reply.stdout).expect("JSON");
    let row = landing_row(&listed, &task);
    assert!(
        row["why"]
            .as_str()
            .is_some_and(|why| why.contains("git was not asked")),
        "{row}"
    );
    assert!(
        listed["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|row| !(row["taskId"] == unverified.as_str() && row["landing"] == true)),
        "unverified work was listed for a landing: {listed}"
    );
    assert!(!review(&bench, &task).merged);
}
