//! t-26587: the window's half of the evidence gate. The receipt files a
//! `worker_done` names are read here, before the ledger judges the report, and
//! the board's row for a task shows the condition states and the notes the
//! ledger keeps for it.

use std::path::Path;

use serde_json::json;
use zerocode_core::orchestration::evidence::{ConditionState, RECEIPT_BYTES_MAX};
use zerocode_core::orchestration::{Draft, Ledger, MessageKind, Priority, Text, worker_address};

use super::cost_book::CostBook;
use super::desk::desk_snapshot;
use super::{read_commits, read_receipts};

/// A receipt path that is absolute on this platform (the product names only
/// absolute evidence paths; the Unix spelling has no drive on Windows).
#[cfg(not(windows))]
const TESTS_RECEIPT: &str = "/Users/dev/receipts/tests.rc";
#[cfg(windows)]
const TESTS_RECEIPT: &str = r"C:\Users\dev\receipts\tests.rc";

const TEAM: &str = "team-evidence";

/// The argv a worker's `send --type worker_done` arrives as, with `payload`.
fn worker_done_with(payload: &str) -> Vec<String> {
    vec![
        "send".into(),
        "--type".into(),
        "worker_done".into(),
        "--body".into(),
        r#"{"ok":true}"#.into(),
        "--payload".into(),
        payload.into(),
    ]
}

/// A payload that names these paths as its evidence.
fn naming(paths: &[&Path]) -> String {
    let named: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    json!({ "evidencePaths": named }).to_string()
}

#[test]
fn the_window_reads_each_named_receipt_as_text_and_leaves_out_the_rest() {
    let folder = tempfile::tempdir().expect("a temp folder");
    let plain = folder.path().join("tests.rc");
    std::fs::write(&plain, "0\n").expect("a receipt");
    let long = folder.path().join("long.rc");
    std::fs::write(&long, format!("0{}", "x".repeat(200))).expect("a long receipt");
    let kept_folder = folder.path().join("folder.rc");
    std::fs::create_dir(&kept_folder).expect("a folder named like a receipt");
    let missing = folder.path().join("missing.rc");

    let files = read_receipts(&worker_done_with(&naming(&[
        &plain,
        &long,
        &kept_folder,
        &missing,
    ])));

    assert_eq!(files.get(&plain.display().to_string()), Some("0\n"));
    assert_eq!(
        files.get(&long.display().to_string()).map(str::len),
        Some(RECEIPT_BYTES_MAX as usize),
        "a receipt longer than the limit is cut to it"
    );
    assert_eq!(files.get(&kept_folder.display().to_string()), None);
    assert_eq!(files.get(&missing.display().to_string()), None);
}

#[cfg(unix)]
#[test]
fn a_receipt_that_is_a_link_is_not_followed() {
    let folder = tempfile::tempdir().expect("a temp folder");
    let target = folder.path().join("real.txt");
    std::fs::write(&target, "0\n").expect("a file");
    let link = folder.path().join("link.rc");
    std::os::unix::fs::symlink(&target, &link).expect("a link");

    let files = read_receipts(&worker_done_with(&naming(&[&link])));

    assert_eq!(files.get(&link.display().to_string()), None);
}

#[test]
fn a_verb_that_is_not_a_worker_done_reads_no_receipt() {
    let folder = tempfile::tempdir().expect("a temp folder");
    let plain = folder.path().join("tests.rc");
    std::fs::write(&plain, "0\n").expect("a receipt");
    let argv: Vec<String> = vec![
        "send".into(),
        "--type".into(),
        "status".into(),
        "--payload".into(),
        naming(&[&plain]),
    ];

    let files = read_receipts(&argv);

    assert_eq!(files.get(&plain.display().to_string()), None);
}

#[test]
fn a_task_with_conditions_shows_their_states_and_its_last_reports_notes_on_the_desk() {
    let mut ledger = Ledger::new();
    let run = ledger.create_run("board", 1_000);
    let spec = "Fix the parser.\n통과 전 cargo test is green\n통과 전 clippy is clean";
    let task = ledger
        .create_task(
            &run,
            spec.to_string(),
            "parser".to_string(),
            vec![],
            None,
            1_001,
        )
        .expect("a task");
    // A second task keeps a worker summoned, so the run is still in play.
    let moving = ledger
        .create_task(
            &run,
            "move".to_string(),
            "move".to_string(),
            vec![],
            None,
            1_002,
        )
        .expect("a task");
    let finisher = ledger
        .start_worker(&run, "claude", (TEAM, "%2"), Some(&task), 1_003)
        .expect("a worker");
    ledger
        .start_worker(&run, "claude", (TEAM, "%3"), Some(&moving), 1_004)
        .expect("a worker");
    let body = json!({"ok": true, "decisions": "keep the parser", "next": "review", "evidence": [
        {"condition": 1, "job": "tests", "rc": 0, "numbers": "412 passed"},
        {"condition": 2, "job": "clippy", "rc": 0, "numbers": "0 warnings"},
    ]});
    let payload = naming(&[Path::new(TESTS_RECEIPT)]);
    ledger
        .post(
            &run,
            Draft {
                from: worker_address(&finisher.worker),
                to: format!("run:{run}"),
                kind: MessageKind::WorkerDone,
                body: Text::from(body.to_string()),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::from(payload),
                thread: None,
                task: Some(task.clone()),
                dispatch: finisher.dispatch.clone(),
            },
            1_005,
        )
        .expect("the report");

    let mut book = CostBook::default();
    let desk = desk_snapshot(&ledger, |_| None, |run, task| book.cost(run, task));
    let row = desk
        .tasks
        .iter()
        .find(|row| row.id == task)
        .expect("the task's desk row");

    let states: Vec<ConditionState> = row.hand_in.conditions.iter().map(|one| one.state).collect();
    assert_eq!(
        states,
        vec![ConditionState::Checked, ConditionState::Claimed]
    );
    assert_eq!(row.hand_in.decisions.as_deref(), Some("keep the parser"));
    assert_eq!(row.hand_in.next.as_deref(), Some("review"));
    assert_eq!(row.hand_in.blocked, None);
}

/// t-78075: what a coordinator's `hand-in` or `task-backfill` would hand git as it was typed is
/// refused here, before git is asked and before the ledger plans — a value git could read as an
/// option, a `--head` that names no commit, a `--checkout` found from the window's own folder.
#[test]
fn the_window_refuses_what_git_would_read_as_an_option_before_git_is_asked() {
    let folder = tempfile::tempdir().expect("a temp folder");
    let repo = folder.path().display().to_string();
    let repo = repo.as_str();
    let head = "0123456789abcdef0123456789abcdef01234567";
    let argv = |words: &[&str], checkout: &str| -> Vec<String> {
        words
            .iter()
            .chain(&["--checkout", checkout])
            .map(|word| (*word).to_string())
            .collect()
    };
    let refused_words: [(&[&str], &str, &str); 5] = [
        (&["hand-in", "--head", "--all"], repo, "--head"),
        (
            &["hand-in", "--head", head, "--base", "--output=x"],
            repo,
            "--base",
        ),
        (
            &["task-backfill", "--tags", "--format=%(refname)"],
            repo,
            "--tags",
        ),
        (
            &["hand-in", "--head", "not-a-commit"],
            repo,
            "seven hex digits",
        ),
        (&["hand-in", "--head", head], "repo", "absolute path"),
    ];
    for (words, checkout, says) in refused_words {
        let Err(refused) = read_commits(&argv(words, checkout), None, (TEAM, "%1"), None) else {
            panic!("{words:?} in {checkout} reached git");
        };
        assert!(
            refused.contains(says),
            "{words:?} refused without {says:?}: {refused}"
        );
    }

    // What is well formed is asked, in the checkout named.
    let asked = read_commits(
        &argv(&["hand-in", "--head", head], repo),
        None,
        (TEAM, "%1"),
        None,
    )
    .expect("a well-formed hand-in is asked")
    .expect("git was asked");
    assert_eq!(asked.repo, repo);
    assert_eq!(
        read_commits(&["task-list".to_string()], Some(repo), (TEAM, "%1"), None),
        Ok(None),
        "another verb asks git nothing"
    );
}
