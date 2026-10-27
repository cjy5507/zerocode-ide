//! t-78075: a coordinator's own attempt on a task — `task-take`, `hand-in`, and the review that
//! follows through `task-update` unchanged — and `task-backfill`, which walks the three steps for
//! finished work whose result already names a landed commit.
//!
//! The ledger runs no git: what git said arrives as [`CommitsSeen`], the window's reading before
//! the plan, and these tests hand it in the way the window does. The git half is tested beside the
//! window's reader, against a throwaway repository.

use super::*;
use crate::orchestration::coordinator_take::{
    BaseSeen, CommitSeen, CommitsSeen, carrier_generation, coordinator_carrier,
};
use crate::orchestration::evidence::ReceiptFiles;
use crate::orchestration::settle::{Left, left_behind};

/// A commit that is on `main`, in release `v1.0.1`.
const LANDED: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
/// A commit git knows that is not on `main`.
const ASIDE: &str = "b2c3d4e5f60718293a4b5c6d7e8f901234567890";
/// A commit on `main` that no release holds yet.
const UNRELEASED: &str = "c3d4e5f60718293a4b5c6d7e8f90123456789012";
/// A commit git does not know.
const UNKNOWN: &str = "d4e5f60718293a4b5c6d7e8f9012345678901234";
/// Where `main` stands.
const MAIN: &str = "e5f60718293a4b5c6d7e8f901234567890123456";
const REPO: &str = "/Users/dev/repo";

/// What git would say in the bench's repository about every fixture commit, measured against
/// `main` and the `v*` tags.
fn git_says() -> CommitsSeen {
    let mut seen = CommitsSeen {
        repo: REPO.to_string(),
        base: Some(BaseSeen {
            name: "main".to_string(),
            head: Some(MAIN.to_string()),
        }),
        tags: Some("v*".to_string()),
        commits: Default::default(),
    };
    for (named, full, in_base, tag) in [
        (LANDED, Some(LANDED), Some(true), Some("v1.0.1")),
        (&LANDED[..9], Some(LANDED), Some(true), Some("v1.0.1")),
        (ASIDE, Some(ASIDE), Some(false), None),
        (UNRELEASED, Some(UNRELEASED), Some(true), None),
        (UNKNOWN, None, None, None),
    ] {
        seen.insert(
            named,
            CommitSeen {
                full: full.map(str::to_string),
                in_base,
                tag: tag.map(str::to_string),
            },
        );
    }
    seen
}

/// The same, asked with no base: what a hand-in without `--base` reads.
fn git_says_without_a_base() -> CommitsSeen {
    let mut seen = git_says();
    seen.base = None;
    seen.tags = None;
    for one in seen.commits.values_mut() {
        one.in_base = None;
        one.tag = None;
    }
    seen
}

fn seeing(bench: &mut Bench, pane: &str, line: &str, seen: Option<CommitsSeen>) -> Decided {
    let receipts = ReceiptFiles::default().with_commits(seen);
    bench.at_argv_with(pane, words(line), &receipts)
}

fn json_seeing(bench: &mut Bench, line: &str, seen: CommitsSeen) -> serde_json::Value {
    let planned = seeing(bench, agent_teams::LEADER_PANE, line, Some(seen));
    assert_eq!(
        planned.reply.exit_code, 0,
        "`{line}` was refused: {}",
        planned.reply.stderr
    );
    serde_json::from_str(&planned.reply.stdout).expect("the verb answers JSON")
}

fn refusal_seeing(bench: &mut Bench, pane: &str, line: &str, seen: Option<CommitsSeen>) -> String {
    let planned = seeing(bench, pane, line, seen);
    assert_ne!(
        planned.reply.exit_code, 0,
        "`{line}` was taken: {}",
        planned.reply.stdout
    );
    planned.reply.stderr
}

fn sound(bench: &Bench, after: &str) {
    if let Err(wrong) = bench.ledger.validate_loaded() {
        panic!("after {after}, this window would refuse its own ledger: {wrong}");
    }
    if let Err(wrong) = Ledger::rebuild(bench.ledger.export()) {
        panic!("after {after}, the durable projection would not load again: {wrong:?}");
    }
}

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

fn row<'a>(bench: &'a Bench, task: &str) -> &'a Task {
    bench.ledger.runs()[0].task(task).expect("the task")
}

fn newest<'a>(bench: &'a Bench, task: &str) -> &'a Dispatch {
    bench.ledger.runs()[0]
        .newest_attempt(task)
        .expect("an attempt")
}

/// A task a worker carried and was stopped on: its one attempt ended handing nothing in.
fn stopped_attempt(bench: &mut Bench, spec: &str) -> (String, String) {
    let task = task_of(bench, spec);
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    bench.json(&format!("worker-stop --worker {worker}"));
    let attempt = newest(bench, &task).id.clone();
    assert!(newest(bench, &task).source.is_none());
    (task, attempt)
}

/// A finished task whose attempt handed nothing in, holding `result` as an old row holds it: the
/// worker's own words, review keys and all, written before a review was bound to an attempt.
fn finished_by_hand(bench: &mut Bench, spec: &str, result: &str) -> String {
    let (task, _) = stopped_attempt(bench, spec);
    bench.json(&format!("task-update --task {task} --status completed"));
    let held = bench.ledger.runs[0]
        .tasks
        .iter_mut()
        .find(|one| one.id == task)
        .expect("the task");
    held.result = result.into();
    held.result_author = Some(ResultAuthor::Worker {
        worker: "w-0".to_string(),
        dispatch: None,
    });
    task
}

fn the_generation(bench: &Bench) -> u32 {
    bench.ledger.runs()[0]
        .coordinator_live()
        .expect("a live seat")
        .generation
}

/// ① The road the gap closes: a worker's attempt ended handing nothing in, the coordinator takes
/// the task, hands its commit in, and the review `task-update` already writes goes through
/// unchanged — bound to the coordinator's attempt and to the commit git answered for.
#[test]
fn a_coordinator_takes_hands_in_and_reviews_work_a_worker_left_without_a_hand_in() {
    let mut bench = Bench::new();
    bench.json("run-create --name take");
    let (task, stopped) = stopped_attempt(&mut bench, "landed-by-hand");

    let before = bench.run(&format!(
        "task-update --task {task} --attempt {stopped} --source {LANDED} --result \
         {{\"verified\":true}}"
    ));
    assert!(
        before
            .reply
            .stderr
            .contains("the attempt has not handed in a source yet"),
        "the gap this closes is not the gap there was: {}",
        before.reply.stderr
    );

    let taken = bench.json(&format!(
        "task-take --task {task} --from {stopped} --why worker-left-it"
    ));
    let mine = taken["dispatchId"]
        .as_str()
        .expect("an attempt")
        .to_string();
    assert_ne!(mine, stopped);
    assert_eq!(taken["retryOf"], stopped.as_str());
    assert_eq!(taken["status"], "dispatched");
    let generation = the_generation(&bench);
    assert_eq!(
        newest(&bench, &task).worker,
        coordinator_carrier(generation),
        "the attempt is the seat's, under its generation"
    );
    assert_eq!(
        carrier_generation(&newest(&bench, &task).worker),
        Some(generation)
    );
    assert!(newest(&bench, &task).is_open());
    sound(&bench, "task-take");

    let handed = json_seeing(
        &mut bench,
        &format!(
            "hand-in --task {task} --head {} --base main --body \
             {{\"ok\":true,\"summary\":\"landed\"}}",
            &LANDED[..9]
        ),
        git_says(),
    );
    assert_eq!(handed["dispatchId"], mine.as_str());
    assert_eq!(
        handed["source"], LANDED,
        "the source is the commit git resolved, not the abbreviation"
    );
    assert_eq!(handed["status"], "completed");
    let attempt = newest(&bench, &task);
    assert_eq!(attempt.source.as_deref(), Some(LANDED));
    assert!(!attempt.is_open());
    assert_eq!(attempt.succeeded, Some(true));
    sound(&bench, "hand-in");

    // The letter it leaves has the shape a worker's report has, read by the same lifecycle list.
    let shown = bench.json(&format!("dispatch-show --task {task}"));
    let lifecycle = shown["lifecycle"].as_array().expect("a lifecycle");
    let letter = lifecycle
        .iter()
        .find(|one| one["type"] == "worker_done")
        .expect("the hand-in's letter");
    let run_address = bench.ledger.runs()[0].address();
    assert_eq!(letter["from"], run_address.as_str());
    let body: serde_json::Value =
        serde_json::from_str(letter["body"].as_str().expect("a body")).expect("a JSON body");
    assert_eq!(body["ok"], true);
    assert_eq!(body["summary"], "landed");
    assert_eq!(body["head"], LANDED);

    // And the review is the one every other review is.
    bench.json(&format!(
        "task-update --task {task} --attempt {mine} --source {LANDED} --result \
         {{\"verified\":true,\"merged\":true,\"mergeHead\":\"{LANDED}\"}}"
    ));
    let review = bench.ledger.runs()[0].review_of(row(&bench, &task));
    assert!(review.verified && review.merged, "{review:?}");
    assert_eq!(review.author, ReviewAuthor::Coordinator);
    assert_eq!(review.attempt.as_deref(), Some(mine.as_str()));
    assert_eq!(review.source.as_deref(), Some(LANDED));
    sound(&bench, "the review");
}

/// A task nobody was ever dispatched on takes a coordinator's attempt as well — work a
/// coordinator does by hand from the start.
#[test]
fn a_task_with_no_attempt_takes_a_coordinators_attempt() {
    let mut bench = Bench::new();
    bench.json("run-create --name take");
    let task = task_of(&mut bench, "by-hand");
    let taken = bench.json(&format!("task-take --task {task}"));
    assert_eq!(taken["retryOf"], serde_json::Value::Null);
    assert_eq!(taken["status"], "dispatched");
    sound(&bench, "a first attempt");
    let handed = json_seeing(
        &mut bench,
        &format!("hand-in --task {task} --head {LANDED} --body {{\"ok\":true}}"),
        git_says_without_a_base(),
    );
    assert_eq!(handed["source"], LANDED);
    assert_eq!(row(&bench, &task).status, TaskStatus::Completed);
    sound(&bench, "its hand-in");

    // Handed in with no base named, it waits for the review `task-update` writes, and a backfill
    // says so instead of proving the commit over again.
    let listed = json_seeing(&mut bench, "task-backfill", git_says());
    let rows = listed["rows"].as_array().expect("rows");
    let mine = rows
        .iter()
        .find(|one| one["taskId"] == task.as_str())
        .unwrap_or_else(|| panic!("{task} is not listed: {listed}"));
    assert_eq!(mine["write"], serde_json::Value::Null, "{mine}");
    assert!(
        mine["why"]
            .as_str()
            .is_some_and(|why| why.contains("handed in") && why.contains("task-update")),
        "{mine}"
    );
    assert_eq!(listed["ready"], 0, "{listed}");
}

/// The seat's open attempt carries its task as a live worker's attempt does: the settle's reading
/// — the desk, the flow and the person's press all draw it — does not stand the task up for a
/// look, however quiet it has been. An attempt of a seat that sat before carries nothing, and
/// stays for a look until the next take ends it.
#[test]
fn the_seats_open_attempt_carries_its_task_and_an_earlier_sittings_does_not() {
    let mut bench = Bench::new();
    bench.json("run-create --name carried");
    let task = task_of(&mut bench, "by-hand");
    bench.json(&format!("task-take --task {task}"));
    let read = left_behind(&bench.ledger.runs()[0], bench.clock);
    assert_eq!(read.get(task.as_str()), None, "{read:?}");

    bench.clock += SETTLE_QUIET_DAYS * 86_400_000 + 1;
    let read = left_behind(&bench.ledger.runs()[0], bench.clock);
    assert_eq!(
        read.get(task.as_str()),
        None,
        "a quiet week does not end an attempt the seat still carries: {read:?}"
    );

    let at = bench.clock;
    bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .expect("a seat")
        .vacated_ms = Some(at);
    let run = bench.ledger.runs()[0].id.clone();
    bench.json(&format!("run-use {run}"));
    let read = left_behind(&bench.ledger.runs()[0], bench.clock);
    assert_eq!(
        read.get(task.as_str()),
        Some(&Left::Look),
        "an earlier sitting's attempt is nobody's now: {read:?}"
    );
}

/// ② A worker still carrying the task keeps it: the coordinator stops it first.
#[test]
fn a_live_workers_attempt_is_not_taken_from_under_it() {
    let mut bench = Bench::new();
    bench.json("run-create --name take");
    let task = task_of(&mut bench, "carried");
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    let attempts = bench.ledger.runs()[0].dispatches.len();
    let refused = refusal_seeing(
        &mut bench,
        agent_teams::LEADER_PANE,
        &format!("task-take --task {task}"),
        None,
    );
    assert!(
        refused.contains(&worker) && refused.contains("worker-stop"),
        "{refused}"
    );
    assert_eq!(bench.ledger.runs()[0].dispatches.len(), attempts);
    assert!(newest(&bench, &task).is_open());

    // A closed task takes no attempt either.
    let closed = task_of(&mut bench, "closed");
    bench.json(&format!(
        "task-update --task {closed} --status closed --closed-as outdated --why over"
    ));
    let refused = refusal_seeing(
        &mut bench,
        agent_teams::LEADER_PANE,
        &format!("task-take --task {closed}"),
        None,
    );
    assert!(refused.contains("closed"), "{refused}");
}

/// ③ A hand-in names a commit git answered for, on the seat's own open attempt, once.
#[test]
fn a_hand_in_is_refused_unless_git_answered_for_its_commit_on_the_seats_own_attempt() {
    let mut bench = Bench::new();
    bench.json("run-create --name take");
    let task = task_of(&mut bench, "hand-in");
    let leader = agent_teams::LEADER_PANE;
    let body = "--body {\"ok\":true}";

    // Before any take there is nothing of the seat's to hand in on.
    let refused = refusal_seeing(
        &mut bench,
        leader,
        &format!("hand-in --task {task} --head {LANDED} {body}"),
        Some(git_says()),
    );
    assert!(
        refused.contains(super::super::coordinator_take::TAKE_VERB),
        "{refused}"
    );

    bench.json(&format!("task-take --task {task}"));
    for (line, seen, says) in [
        (
            format!("hand-in --task {task} --head {UNKNOWN} {body}"),
            Some(git_says()),
            "not a commit",
        ),
        (
            format!("hand-in --task {task} --head {ASIDE} --base main {body}"),
            Some(git_says()),
            "not in main",
        ),
        (
            format!("hand-in --task {task} --head {LANDED} {body}"),
            None,
            "git",
        ),
        (
            format!("hand-in --task {task} --head not-a-commit {body}"),
            Some(git_says()),
            "--head",
        ),
        (
            format!(
                "hand-in --task {task} --head {LANDED} --body {{\"ok\":true,\"head\":\"{ASIDE}\"}}"
            ),
            Some(git_says()),
            "body's head",
        ),
        (
            format!("hand-in --task {task} --head {LANDED} --body {{\"summary\":\"no-verdict\"}}"),
            Some(git_says()),
            "verdict",
        ),
    ] {
        let refused = refusal_seeing(&mut bench, leader, &line, seen);
        assert!(
            refused.contains(says),
            "`{line}` refused without {says:?}: {refused}"
        );
        let attempt = newest(&bench, &task);
        assert!(
            attempt.is_open() && attempt.source.is_none(),
            "a refused hand-in moved the attempt: {line}"
        );
    }
    sound(&bench, "the refusals");

    // A seat sitting again under a new generation does not inherit the old one's attempt…
    let at = bench.clock;
    bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .expect("a seat")
        .vacated_ms = Some(at);
    let run = bench.ledger.runs()[0].id.clone();
    bench.json(&format!("run-use {run}"));
    let refused = refusal_seeing(
        &mut bench,
        leader,
        &format!("hand-in --task {task} --head {LANDED} {body}"),
        Some(git_says()),
    );
    assert!(refused.contains("generation"), "{refused}");
    // …it takes it over, and the old attempt ends the way a stopped one does.
    let old = newest(&bench, &task).id.clone();
    let taken = bench.json(&format!("task-take --task {task} --from {old}"));
    assert_eq!(taken["closed"], old.as_str());
    let old_row = bench.ledger.runs()[0]
        .dispatch(&old)
        .expect("the old attempt");
    assert!(!old_row.is_open() && old_row.succeeded.is_none());
    sound(&bench, "a take-over between generations");

    json_seeing(
        &mut bench,
        &format!("hand-in --task {task} --head {LANDED} {body}"),
        git_says(),
    );
    // And once handed in, an attempt is not handed in again.
    let refused = refusal_seeing(
        &mut bench,
        leader,
        &format!("hand-in --task {task} --head {UNRELEASED} {body}"),
        Some(git_says()),
    );
    assert!(refused.contains("already handed in"), "{refused}");
    assert_eq!(newest(&bench, &task).source.as_deref(), Some(LANDED));
}

/// ④ Both verbs are the coordinator seat's: a worker's pane is refused before anything moves.
#[test]
fn a_workers_pane_can_neither_take_nor_hand_in() {
    let mut bench = Bench::new();
    bench.json("run-create --name take");
    let mine = task_of(&mut bench, "mine");
    let other = task_of(&mut bench, "other");
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {mine}"));
    let attempts = bench.ledger.runs()[0].dispatches.len();
    for line in [
        format!("task-take --task {other}"),
        format!("hand-in --task {mine} --head {LANDED} --body {{\"ok\":true}}"),
        "task-backfill --apply".to_string(),
    ] {
        let refused = refusal_seeing(&mut bench, &pane, &line, Some(git_says()));
        assert!(refused.contains("coordinator"), "`{line}`: {refused}");
    }
    assert_eq!(bench.ledger.runs()[0].dispatches.len(), attempts);
    assert!(newest(&bench, &mine).is_open() && newest(&bench, &mine).source.is_none());

    // Nor can another team's leader, naming the run: it is neither a worker here nor the seat.
    let run = bench.ledger.runs()[0].id.clone();
    let mut stranger = Team::new("team-2", "stranger", 70);
    std::mem::swap(&mut bench.team, &mut stranger);
    for line in [
        format!("task-take --run {run} --task {other}"),
        format!("hand-in --run {run} --task {mine} --head {LANDED} --body {{\"ok\":true}}"),
        format!("task-backfill --run {run} --apply"),
    ] {
        let refused = refusal_seeing(
            &mut bench,
            agent_teams::LEADER_PANE,
            &line,
            Some(git_says()),
        );
        assert!(refused.contains("not that seat"), "`{line}`: {refused}");
    }
    std::mem::swap(&mut bench.team, &mut stranger);
    assert_eq!(bench.ledger.runs()[0].dispatches.len(), attempts);
    assert!(newest(&bench, &mine).is_open() && newest(&bench, &mine).source.is_none());
}

/// ⑤ The backfill lists first and writes only what git proves: merged where the commit is in
/// `main`, deployed where a release tag holds it (named), verified only where the result names a
/// review task whose own review passed — and it keeps the original result, adding beside it.
#[test]
fn a_backfill_lists_then_writes_only_what_git_proves() {
    let mut bench = Bench::new();
    bench.json("run-create --name backfill");

    // A review task whose own review passed: the verdict the backfill may cite.
    let check = task_of(&mut bench, "the-review");
    let (_worker, pane) = bench.seat(&format!("worker-start --agent codex --task {check}"));
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{UNRELEASED}\"}}"),
    );
    let checked = newest(&bench, &check).id.clone();
    bench.json(&format!(
        "task-update --task {check} --attempt {checked} --source {UNRELEASED} --result \
         {{\"verified\":true,\"nothingToLand\":true}}"
    ));

    let landed = finished_by_hand(
        &mut bench,
        "landed",
        &format!(
            "{{\"mergeHead\":\"{LANDED}\",\"reviews\":[\"{check}\"],\"note\":\"by-hand\",\
             \"verified\":true}}"
        ),
    );
    let unreleased = finished_by_hand(
        &mut bench,
        "unreleased",
        &format!("{{\"merged\":\"{UNRELEASED}\",\"verified\":true}}"),
    );
    // A list of commits, read from its end: the last one named is the one believed first.
    let listed_commits = finished_by_hand(
        &mut bench,
        "listed-commits",
        &format!("{{\"commits\":[\"{ASIDE}\",\"{UNRELEASED}\"]}}"),
    );
    let nothing = finished_by_hand(&mut bench, "nothing", "{\"note\":\"no-commit-here\"}");
    let aside = finished_by_hand(
        &mut bench,
        "aside",
        &format!("{{\"mergeHead\":\"{ASIDE}\"}}"),
    );
    let unknown = finished_by_hand(
        &mut bench,
        "unknown",
        &format!("{{\"mergeHead\":\"{UNKNOWN}\"}}"),
    );
    // Closed with its facts nested, the shape t-77628 was left in.
    let (closed, _) = stopped_attempt(&mut bench, "closed-landed");
    bench.json(&format!(
        "task-update --task {closed} --result {{\"landed\":{{\"mergeHead\":\"{LANDED}\"}}}}"
    ));
    bench.json(&format!(
        "task-update --task {closed} --status closed --closed-as outdated --why landed"
    ));
    sound(&bench, "the fixture");

    let attempts = bench.ledger.runs()[0].dispatches.len();
    let results: Vec<String> = [&landed, &unreleased, &closed, &listed_commits]
        .iter()
        .map(|task| row(&bench, task).result.to_string())
        .collect();

    // Without git, nothing can be proven and nothing is said to be.
    let blind = seeing(&mut bench, agent_teams::LEADER_PANE, "task-backfill", None);
    let blind: serde_json::Value = serde_json::from_str(&blind.reply.stdout).expect("JSON");
    assert_eq!(blind["ready"], 0, "{blind}");

    let listed = json_seeing(&mut bench, "task-backfill", git_says());
    assert_eq!(listed["apply"], false);
    assert_eq!(listed["base"], "main");
    let rows = listed["rows"].as_array().expect("rows");
    let of = |task: &str| {
        rows.iter()
            .find(|one| one["taskId"] == task)
            .unwrap_or_else(|| panic!("{task} is not listed: {listed}"))
            .clone()
    };
    assert!(
        rows.iter().all(|one| one["taskId"] != check.as_str()),
        "a reviewed task is listed: {listed}"
    );
    let first = of(&landed);
    assert_eq!(first["commit"], LANDED);
    assert_eq!(first["tag"], "v1.0.1");
    assert_eq!(first["write"]["merged"], true);
    assert_eq!(first["write"]["mergeHead"], LANDED);
    assert_eq!(first["write"]["deployed"], true);
    assert_eq!(first["write"]["verified"], true);
    assert_eq!(first["why"], serde_json::Value::Null);
    let second = of(&unreleased);
    assert_eq!(second["write"]["merged"], true);
    assert_eq!(second["write"].get("deployed"), None, "{second}");
    assert_eq!(second["write"].get("verified"), None, "{second}");
    assert_eq!(second["verified"], "unknown");
    let from_a_list = of(&listed_commits);
    assert_eq!(from_a_list["commit"], UNRELEASED, "{from_a_list}");
    assert_eq!(from_a_list["foundUnder"], "commits", "{from_a_list}");
    assert_eq!(from_a_list["write"]["merged"], true, "{from_a_list}");
    assert!(
        of(&nothing)["why"]
            .as_str()
            .is_some_and(|why| why.contains("names no commit"))
    );
    assert!(
        of(&aside)["why"]
            .as_str()
            .is_some_and(|why| why.contains("not in main"))
    );
    assert!(
        of(&unknown)["why"]
            .as_str()
            .is_some_and(|why| why.contains("not a commit"))
    );
    assert_eq!(of(&closed)["write"]["merged"], true);
    assert_eq!(listed["ready"], 4);
    assert_eq!(listed["repo"], REPO);
    assert_eq!(
        bench.ledger.runs()[0].dispatches.len(),
        attempts,
        "a list wrote something"
    );

    let applied = json_seeing(&mut bench, "task-backfill --apply", git_says());
    assert_eq!(applied["apply"], true);
    let mut done: Vec<&str> = applied["applied"]
        .as_array()
        .expect("applied")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    done.sort_unstable();
    let mut expected = vec![
        landed.as_str(),
        unreleased.as_str(),
        closed.as_str(),
        listed_commits.as_str(),
    ];
    expected.sort_unstable();
    assert_eq!(done, expected);
    sound(&bench, "the backfill");

    let run = &bench.ledger.runs()[0];
    let generation = the_generation(&bench);
    for (task, original) in [&landed, &unreleased, &closed, &listed_commits]
        .into_iter()
        .zip(&results)
    {
        let attempt = run.newest_attempt(task).expect("the backfill's attempt");
        assert_eq!(attempt.worker, coordinator_carrier(generation));
        assert!(!attempt.is_open());
        let review = run.review_of(run.task(task).expect("the task"));
        assert!(review.merged, "{task}: {review:?}");
        assert_eq!(review.author, ReviewAuthor::Coordinator);
        assert_eq!(review.attempt.as_deref(), Some(attempt.id.as_str()));
        // The original is kept: every key it had reads as it did — a review key under
        // `backfill.replaced`, where it is no longer read as the seat's review.
        let now: serde_json::Value =
            serde_json::from_str(run.task(task).expect("the task").result.as_str())
                .expect("a JSON result");
        let was: serde_json::Value = serde_json::from_str(original).expect("JSON");
        for (key, value) in was.as_object().expect("an object") {
            let kept = now["backfill"]["replaced"]
                .get(key)
                .or_else(|| now.get(key));
            assert_eq!(kept, Some(value), "{task} lost {key}: {now}");
        }
        assert_eq!(now["backfill"]["by"], "task-backfill", "{now}");
        assert_eq!(
            now["backfill"]["repo"], REPO,
            "the note names the checkout git was asked in: {now}"
        );
    }
    let first = run.review_of(run.task(&landed).expect("the task"));
    assert!(first.verified && first.deployed, "{first:?}");
    let second = run.review_of(run.task(&unreleased).expect("the task"));
    assert!(!second.verified && !second.deployed, "{second:?}");
    // The worker's own `"verified":true` was never a review: with no review task named, it is
    // kept under `backfill.replaced` and no longer stands at the top, where the seat's review is
    // read.
    let now: serde_json::Value =
        serde_json::from_str(run.task(&unreleased).expect("the task").result.as_str())
            .expect("a JSON result");
    assert_eq!(now.get("verified"), None, "{now}");
    assert_eq!(now["backfill"]["replaced"]["verified"], true, "{now}");
    assert_eq!(now["backfill"]["verified"], "unknown", "{now}");
    assert_eq!(
        run.task(&closed).expect("the task").status,
        TaskStatus::Closed
    );
    for untouched in [&nothing, &aside, &unknown] {
        assert!(
            run.newest_attempt(untouched)
                .is_some_and(|attempt| attempt.source.is_none()),
            "{untouched} was written though git proved nothing"
        );
    }

    // Applied once, it is not listed again.
    let again = json_seeing(&mut bench, "task-backfill", git_says());
    assert_eq!(again["ready"], 0, "{again}");
}
