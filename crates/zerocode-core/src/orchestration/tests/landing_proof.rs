//! t-119569 (ruling §4): a landing written without the person's hand — `task-backfill` proves
//! the merge and the release of verified work from git alone, and writes nothing it cannot prove.
//!
//! Merged only when every commit of the range the coordinator's review read is in the base as the
//! same change — landed as it is, or picked onto the base as copies — with the commit the last of
//! them landed as; never where the range holds a merge commit, changes nothing, or the base
//! reverted the landing. Deployed only when a release tag origin holds holds that landing. The
//! ledger never writes the verification: the review that stands is the coordinator's, and the
//! landing is bound to it as `task-update --members` binds one.

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
/// A merge commit inside the range, a commit of it that changes nothing, and a commit of `main`
/// that reverts the landing.
const MERGE: &str = "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4";
const EMPTY: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
const REVERT: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";

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
        ..RangeSeen::default()
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
        ..RangeSeen::default()
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
            ..RangeSeen::default()
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

/// The row of `task` for a backfill that applies `range`, and the review the task holds after it.
fn applied(bench: &mut Bench, task: &str, range: RangeSeen) -> (serde_json::Value, ReviewFacts) {
    let listed = backfill(bench, "task-backfill --apply", git_says(range));
    (landing_row(&listed, task), review(bench, task))
}

/// A merge commit inside the range (t-119569, the other provider's check B · E) proves no landing
/// by copies: what it changes itself — a conflict resolved, a line added in it — matches no commit
/// of `main`. Nothing is written, and the row names the merge.
#[test]
fn a_range_holding_a_merge_commit_writes_nothing_and_says_which() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-merge");
    let task = verified(&mut bench, "merging", HEAD);
    let (row, after) = applied(
        &mut bench,
        &task,
        RangeSeen {
            merges: vec![MERGE.to_string()],
            ..landed_and_released()
        },
    );
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    let why = row["why"].as_str().expect("why it cannot");
    assert!(why.contains(MERGE) && why.contains("merge"), "{why}");
    assert!(!after.merged && !after.deployed, "{after:?}");
}

/// Commits that change nothing (check B · D) are nothing to land: left out of the range, a range
/// of nothing else proves no landing and says so — and beside commits that landed, they do not
/// hold the landing back.
#[test]
fn commits_that_change_nothing_are_nothing_to_land() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-empty");
    let only = verified(&mut bench, "only-empty", HEAD);
    let (row, after) = applied(
        &mut bench,
        &only,
        RangeSeen {
            empty: vec![EMPTY.to_string()],
            ..RangeSeen::default()
        },
    );
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    let why = row["why"].as_str().expect("why it cannot");
    assert!(
        why.contains(EMPTY) && why.contains("change nothing"),
        "{why}"
    );
    assert!(!after.merged, "{after:?}");

    let mut beside = Bench::new();
    beside.json("run-create --name landing-empty-beside");
    let task = verified(&mut beside, "beside", HEAD);
    let (row, after) = applied(
        &mut beside,
        &task,
        RangeSeen {
            empty: vec![EMPTY.to_string()],
            ..landed_and_released()
        },
    );
    assert_eq!(row["write"]["mergeHead"], LANDED_AS, "{row}");
    assert!(after.merged && after.deployed, "{after:?}");
}

/// A landing `main` reverted (check B · F) is no landing to write: neither merged nor deployed,
/// for work verified and for work already written merged — and the row names the revert.
#[test]
fn a_landing_main_reverted_writes_neither_merged_nor_deployed() {
    let mut bench = Bench::new();
    bench.json("run-create --name landing-reverted");
    let task = verified(&mut bench, "reverted", HEAD);
    let reverted = || RangeSeen {
        reverted: vec![(REVERT.to_string(), LANDED_AS.to_string())],
        ..landed_and_released()
    };
    let (row, after) = applied(&mut bench, &task, reverted());
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    let why = row["why"].as_str().expect("why it cannot");
    assert!(why.contains(REVERT) && why.contains("revert"), "{why}");
    assert!(!after.merged && !after.deployed, "{after:?}");

    // Written merged before the revert came: the release is not written either.
    let landed = RangeSeen {
        tag: None,
        ..landed_and_released()
    };
    let (_, merged) = applied(&mut bench, &task, landed);
    assert!(merged.merged && !merged.deployed, "{merged:?}");
    let mut seen = git_says(RangeSeen::default());
    seen.insert_range(
        LANDED_AS,
        RangeSeen {
            commits: vec![(LANDED_AS.to_string(), true)],
            landed_as: Some(LANDED_AS.to_string()),
            ..reverted()
        },
    );
    let listed = backfill(&mut bench, "task-backfill --apply", seen);
    let row = landing_row(&listed, &task);
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    assert!(!review(&bench, &task).deployed);
}

/// A release tag origin does not hold (check B · A) ships nothing: the lane pushes its tag only
/// once it ends green, so a tag only local is a release still running or one that failed — and the
/// row says why in git's own terms.
#[test]
fn a_release_tag_origin_does_not_hold_ships_nothing_and_says_why() {
    const UNPUBLISHED: &str = "v1.2.0 holds the landing, but origin has no such tag";
    let mut bench = Bench::new();
    bench.json("run-create --name landing-unpublished");
    let task = verified(&mut bench, "unpublished", HEAD);
    let (row, after) = applied(
        &mut bench,
        &task,
        RangeSeen {
            tag: None,
            unpublished: Some(UNPUBLISHED.to_string()),
            ..landed_and_released()
        },
    );
    assert_eq!(row["write"]["merged"], true, "{row}");
    assert_eq!(row["write"].get("deployed"), None, "{row}");
    assert!(after.merged && !after.deployed, "{after:?}");

    let mut seen = git_says(RangeSeen::default());
    seen.insert_range(
        LANDED_AS,
        RangeSeen {
            commits: vec![(LANDED_AS.to_string(), true)],
            landed_as: Some(LANDED_AS.to_string()),
            unpublished: Some(UNPUBLISHED.to_string()),
            ..RangeSeen::default()
        },
    );
    let listed = backfill(&mut bench, "task-backfill --apply", seen);
    let row = landing_row(&listed, &task);
    assert_eq!(row["stage"], "merged", "{row}");
    assert_eq!(row["write"], serde_json::Value::Null, "{row}");
    assert!(
        row["why"]
            .as_str()
            .is_some_and(|why| why.contains(UNPUBLISHED)),
        "{row}"
    );
    assert!(!review(&bench, &task).deployed);
}
