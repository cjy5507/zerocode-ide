//! t-34501 stage 3 (t-42447): the window's landing check, run against real temporary repositories.
//!
//! Every road below runs `git` for real in a temporary repository and an app-data folder of its
//! own. The evidence is read from the channel the window's sink feeds, so no ledger is needed.

use super::*;
use std::sync::mpsc;
use std::time::Instant;

/// How long a test waits for a letter the window writes in the background.
const LETTER_WAIT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> String {
    let output = crate::proc::quiet_command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit_file(repo: &Path, name: &str, text: &str, message: &str) {
    std::fs::write(repo.join(name), text).expect("a file");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", message]);
}

fn policy(command: Option<&str>) -> Policy {
    Policy {
        command: command.map(str::to_string),
        timeout: Duration::from_secs(30),
        pinned_base: None,
        shell: PathBuf::from("/bin/bash"),
    }
}

/// A repository whose `feature` branch adds `b.txt` and whose `main` moved on with `c.txt`, so the
/// merge is a real one and not a fast-forward.
struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    data: PathBuf,
    feature: String,
    main: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("a temporary folder");
        // Canonical from the start: the folders the window makes are compared by their real names.
        let base = std::fs::canonicalize(temp.path()).expect("a canonical folder");
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("the repository folder");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "ZeroCode Test"]);
        git(&repo, &["config", "user.email", "test@zerocode"]);
        commit_file(&repo, "a.txt", "one\n", "first");
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        commit_file(&repo, "b.txt", "feature\n", "feature");
        let feature = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "main"]);
        commit_file(&repo, "c.txt", "main\n", "main moves");
        let main = git(&repo, &["rev-parse", "HEAD"]);
        Self {
            _temp: temp,
            repo,
            data: base.join("app-data"),
            feature,
            main,
        }
    }

    fn store(&self) -> Store {
        Store::at(&self.data).expect("the store")
    }

    fn merge(&self, head: &str, prepare: bool) -> LandCheckAsk {
        LandCheckAsk::Merge {
            run: "run-1".to_string(),
            task: "t-1".to_string(),
            head: head.to_string(),
            checkout: self.repo.display().to_string(),
            prepare,
        }
    }

    /// Nothing the window made is left: one worktree (the repository's own), and no folder under
    /// `work/`.
    fn assert_left_nothing(&self, store: &Store) {
        let listed = git(&self.repo, &["worktree", "list", "--porcelain"]);
        assert_eq!(listed.matches("\nworktree ").count() + 1, 1, "{listed}");
        let work = store.root().join("work");
        let left: Vec<String> = std::fs::read_dir(&work)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(left.is_empty(), "left under work/: {left:?}");
    }
}

/// The window's sink in a test: every receipt lands in the channel, in the order it was written.
fn sink() -> (Sink, mpsc::Receiver<LandCheckReceipt>) {
    let (tx, rx) = mpsc::channel();
    (
        Arc::new(move |receipt| {
            let _ = tx.send(receipt);
        }),
        rx,
    )
}

/// The next letter, with its evidence read as JSON.
fn letter(rx: &mpsc::Receiver<LandCheckReceipt>) -> (LandCheckReceipt, serde_json::Value) {
    let receipt = rx
        .recv_timeout(LETTER_WAIT)
        .expect("the evidence is written");
    let evidence = serde_json::from_str(&receipt.evidence).expect("the evidence is JSON");
    (receipt, evidence)
}

/// The answer the verb printed, read as JSON, or a panic that says what was refused.
fn said(answer: Result<String, String>) -> serde_json::Value {
    let text = answer.unwrap_or_else(|why| panic!("the check was refused: {why}"));
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("the answer is not JSON: {text}"))
}

/// A row as the window writes it for a check that was running or prepared.
fn write_row(store: &Store, check: &str, row: &serde_json::Value) {
    let rows = store.root().join("rows");
    std::fs::create_dir_all(&rows).expect("the rows folder");
    std::fs::write(rows.join(format!("{check}.json")), row.to_string()).expect("a row");
}

/// Prepare the feature head and answer its folder, row and log.
fn prepared(fixture: &Fixture, store: &Store, sink: &Sink) -> serde_json::Value {
    said(run(
        store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, true),
        1_000,
        sink,
    ))
}

#[test]
fn a_clean_merge_without_a_command_is_merged_and_names_its_tree() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let answer = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    assert_eq!(answer["state"], "merged", "{answer}");
    let tree = answer["tree"].as_str().expect("the merged tree");
    assert_eq!(tree.len(), 40, "{tree}");
    // The same merge, asked of git directly, names the same tree.
    let probe = git(
        &fixture.repo,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            &fixture.main,
            &fixture.feature,
        ],
    );
    assert_eq!(probe.lines().next(), Some(tree));
    let (receipt, evidence) = letter(&rx);
    assert_eq!(Some(receipt.check.as_str()), answer["check"].as_str());
    assert_eq!(receipt.task, "t-1");
    assert_eq!(evidence["state"], "merged", "{evidence}");
    fixture.assert_left_nothing(&store);
}

#[test]
fn a_clash_names_its_files_and_is_a_conflict_with_nothing_left_behind() {
    let fixture = Fixture::new();
    let store = fixture.store();
    // Both sides change a.txt, so the merge must clash on it.
    commit_file(&fixture.repo, "a.txt", "main side\n", "main edits a");
    git(&fixture.repo, &["checkout", "-q", "feature"]);
    commit_file(&fixture.repo, "a.txt", "feature side\n", "feature edits a");
    let clashing = git(&fixture.repo, &["rev-parse", "HEAD"]);
    git(&fixture.repo, &["checkout", "-q", "main"]);
    let (sink, rx) = sink();
    let answer = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&clashing, false),
        1_000,
        &sink,
    ));
    assert_eq!(answer["state"], "conflict", "{answer}");
    assert_eq!(answer["conflicts"]["total"], 1, "{answer}");
    assert_eq!(answer["conflicts"]["files"], serde_json::json!(["a.txt"]));
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "conflict", "{evidence}");
    assert_eq!(evidence["conflicts"]["files"], serde_json::json!(["a.txt"]));
    fixture.assert_left_nothing(&store);
}

#[test]
fn a_passing_command_is_passed_with_rc_zero_and_its_log() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let answer = said(run(
        &store,
        &|_| policy(Some("echo check-ran-here\nexit 0")),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    assert_eq!(answer["state"], "started", "{answer}");
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "passed", "{evidence}");
    assert_eq!(evidence["rc"], 0, "{evidence}");
    assert!(evidence["tookMs"].as_i64().is_some(), "{evidence}");
    let check = answer["check"].as_str().expect("the check id");
    let log = PathBuf::from(evidence["log"].as_str().expect("a log path"));
    assert_eq!(log, store.root().join("logs").join(format!("{check}.log")));
    let said_in_log = std::fs::read_to_string(&log).expect("the log");
    assert!(said_in_log.contains("check-ran-here"), "{said_in_log}");
    fixture.assert_left_nothing(&store);
}

#[test]
fn a_failing_command_is_failed_with_its_exit_code() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(Some("exit 3")),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "failed", "{evidence}");
    assert_eq!(evidence["rc"], 3, "{evidence}");
    fixture.assert_left_nothing(&store);
}

#[cfg(unix)]
#[test]
fn a_command_past_its_limit_is_timed_out_and_its_processes_go_with_it() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let pid_file = fixture.data.join("sleeper.pid");
    std::fs::create_dir_all(&fixture.data).expect("the data folder");
    let command = format!("sleep 60 & echo $! > '{}'\nwait\n", pid_file.display());
    let limited = Policy {
        timeout: Duration::from_secs(1),
        ..policy(Some(&command))
    };
    let _ = said(run(
        &store,
        &|_| limited.clone(),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "timed_out", "{evidence}");
    let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
        .expect("the sleeper's pid")
        .trim()
        .parse()
        .expect("a pid");
    // SAFETY: signal 0 only asks whether the process exists; nothing is sent.
    let alive = || unsafe { libc::kill(pid, 0) } == 0;
    let waited = Instant::now();
    while alive() && waited.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(), "the sleeper outlived the check's limit");
}

#[cfg(unix)]
#[test]
fn a_command_that_cannot_start_is_unstartable_and_its_folder_is_still_removed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let missing = Policy {
        shell: PathBuf::from("/nonexistent/zerocode-check-shell"),
        ..policy(Some("exit 0"))
    };
    let _ = said(run(
        &store,
        &|_| missing.clone(),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "unstartable", "{evidence}");
    fixture.assert_left_nothing(&store);
}

#[test]
fn a_head_that_is_no_commit_is_refused_before_anything_is_made() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let refused = run(
        &store,
        &|_| policy(None),
        &fixture.merge("1234567", false),
        1_000,
        &sink,
    );
    assert!(refused.is_err(), "{refused:?}");
    assert!(rx.try_recv().is_err(), "a refusal writes no letter");
    fixture.assert_left_nothing(&store);
}

/// The original checkout's `HEAD` and index are the same bytes after a check as before it: the
/// merge happens in a folder of the window's own.
#[test]
fn the_original_checkout_keeps_its_head_and_index_bytes() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let head_file = fixture.repo.join(".git").join("HEAD");
    let index_file = fixture.repo.join(".git").join("index");
    let before = (
        std::fs::read(&head_file).expect("HEAD"),
        std::fs::read(&index_file).expect("index"),
    );
    let (sink, rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "merged", "{evidence}");
    let after = (
        std::fs::read(&head_file).expect("HEAD"),
        std::fs::read(&index_file).expect("index"),
    );
    assert!(before.0 == after.0, "the checkout's HEAD changed");
    assert!(before.1 == after.1, "the checkout's index changed");
}

/// A folder and worktree entry a crashed run left behind are taken down before the next check, and
/// the abandoned run's task hears about it by letter.
#[cfg(unix)]
#[test]
fn an_expired_leftover_is_taken_down_with_an_abandoned_letter_before_a_new_check() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let old = "lc-1-1";
    let folder = store.root().join("work").join(old);
    std::fs::create_dir_all(store.root().join("work")).expect("the work folder");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            &folder.display().to_string(),
            &fixture.feature,
        ],
    );
    write_row(
        &store,
        old,
        &serde_json::json!({
            "check": old,
            "run": "run-1",
            "task": "t-old",
            "head": fixture.feature,
            "repo": fixture.repo.display().to_string(),
            "path": folder.display().to_string(),
            "state": "running",
            "startedMs": 1,
            "leaseUntilMs": 2,
        }),
    );
    let (sink, rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000_000,
        &sink,
    ));
    let mut letters = Vec::new();
    while let Ok(receipt) = rx.recv_timeout(Duration::from_secs(5)) {
        letters.push(receipt);
    }
    let abandoned = letters
        .iter()
        .find(|receipt| receipt.check == old)
        .expect("the abandoned run is told");
    assert_eq!(abandoned.task, "t-old");
    let evidence: serde_json::Value =
        serde_json::from_str(&abandoned.evidence).expect("the evidence is JSON");
    assert_eq!(evidence["state"], "abandoned", "{evidence}");
    assert!(!folder.exists(), "the stale folder is gone");
    let listed = git(&fixture.repo, &["worktree", "list", "--porcelain"]);
    assert!(!listed.contains(old), "{listed}");
    assert!(
        !store
            .root()
            .join("rows")
            .join(format!("{old}.json"))
            .exists()
    );
}

/// A row whose lease has not run out is somebody's live check, and nothing sweeps it.
#[test]
fn a_live_lease_is_never_swept() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let live = "lc-1-2";
    let folder = store.root().join("work").join(live);
    std::fs::create_dir_all(&folder).expect("a live folder");
    std::fs::write(folder.join("keep.txt"), "live\n").expect("a file");
    write_row(
        &store,
        live,
        &serde_json::json!({
            "check": live,
            "run": "run-1",
            "task": "t-live",
            "head": fixture.feature,
            "repo": fixture.repo.display().to_string(),
            "path": folder.display().to_string(),
            "state": "running",
            "startedMs": 1_000_000,
            "leaseUntilMs": 4_600_000,
        }),
    );
    let (sink, _rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000_000,
        &sink,
    ));
    assert!(
        folder.join("keep.txt").exists(),
        "a live check's folder was swept"
    );
    assert!(
        store
            .root()
            .join("rows")
            .join(format!("{live}.json"))
            .exists()
    );
}

/// A folder under `work/` with no row is nobody's to keep: it is a leftover of a run that never
/// wrote its row, and it is taken down.
#[test]
fn a_folder_with_no_row_is_swept_as_a_leftover() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let stray = store.root().join("work").join("lc-1-3");
    std::fs::create_dir_all(&stray).expect("a stray folder");
    let (sink, _rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    assert!(!stray.exists(), "the stray folder is still there");
}

/// A link inside `work/` is never followed: what it points at is not taken down with it.
#[cfg(unix)]
#[test]
fn a_link_in_the_work_folder_never_takes_its_target_with_it() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let outside = fixture.data.join("outside");
    std::fs::create_dir_all(&outside).expect("an outside folder");
    std::fs::write(outside.join("keep.txt"), "not yours\n").expect("a file");
    std::fs::create_dir_all(store.root().join("work")).expect("the work folder");
    std::os::unix::fs::symlink(&outside, store.root().join("work").join("lc-1-4")).expect("a link");
    let (sink, _rx) = sink();
    let _ = said(run(
        &store,
        &|_| policy(None),
        &fixture.merge(&fixture.feature, false),
        1_000,
        &sink,
    ));
    assert!(
        outside.join("keep.txt").exists(),
        "the link's target was taken down"
    );
}

/// `--prepare` keeps the merged folder and its row and answers where they are; `--record` on the
/// same check, with the log the answer named, ends it: the letter says passed and both are gone.
#[test]
fn prepare_keeps_the_folder_and_its_row_until_a_record_ends_it() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let answer = prepared(&fixture, &store, &sink);
    assert_eq!(answer["state"], "prepared", "{answer}");
    let check = answer["check"].as_str().expect("the check id").to_string();
    let path = PathBuf::from(answer["path"].as_str().expect("the folder"));
    assert!(path.starts_with(store.root().join("work")), "{path:?}");
    assert!(path.join("b.txt").exists(), "the merged folder is kept");
    assert_eq!(answer["tree"].as_str().map(str::len), Some(40), "{answer}");
    assert_eq!(answer["head"], fixture.feature.as_str());
    assert!(
        store
            .root()
            .join("rows")
            .join(format!("{check}.json"))
            .exists(),
        "the prepared row is kept"
    );
    assert!(
        rx.try_recv().is_err(),
        "a prepared check writes no letter yet"
    );

    // The check ran elsewhere and wrote its log where the answer said.
    let log = PathBuf::from(answer["log"].as_str().expect("the log path"));
    std::fs::create_dir_all(log.parent().expect("a log folder")).expect("the log folder");
    std::fs::write(&log, "ran elsewhere\n").expect("the log");
    let ended = said(run(
        &store,
        &|_| policy(None),
        &LandCheckAsk::Record {
            run: "run-1".to_string(),
            task: "t-1".to_string(),
            head: fixture.feature.clone(),
            check: check.clone(),
            rc: 0,
            log: log.display().to_string(),
            took_ms: Some(5),
        },
        2_000,
        &sink,
    ));
    assert_eq!(ended["state"], "passed", "{ended}");
    let (_, evidence) = letter(&rx);
    assert_eq!(evidence["state"], "passed", "{evidence}");
    assert_eq!(evidence["rc"], 0, "{evidence}");
    assert!(
        !path.exists(),
        "the prepared folder is gone after the record"
    );
    assert!(
        !store
            .root()
            .join("rows")
            .join(format!("{check}.json"))
            .exists()
    );
    fixture.assert_left_nothing(&store);
}

/// A record must match what was prepared: another head, another task, or a check id nobody
/// prepared is refused, and the prepared check stands for the right record.
#[test]
fn a_record_for_another_head_task_or_check_is_refused_and_the_prepared_check_stands() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, rx) = sink();
    let answer = prepared(&fixture, &store, &sink);
    let check = answer["check"].as_str().expect("the check id").to_string();
    let log = PathBuf::from(answer["log"].as_str().expect("the log path"));
    std::fs::create_dir_all(log.parent().expect("a log folder")).expect("the log folder");
    std::fs::write(&log, "ran elsewhere\n").expect("the log");
    let record = |head: &str, task: &str, check: &str| LandCheckAsk::Record {
        run: "run-1".to_string(),
        task: task.to_string(),
        head: head.to_string(),
        check: check.to_string(),
        rc: 0,
        log: log.display().to_string(),
        took_ms: None,
    };
    let refused = [
        (fixture.main.as_str(), "t-1", check.as_str(), "head"),
        (fixture.feature.as_str(), "t-2", check.as_str(), "task"),
        (fixture.feature.as_str(), "t-1", "lc-9-9", "check"),
    ];
    for (head, task, named, word) in refused {
        let why = run(
            &store,
            &|_| policy(None),
            &record(head, task, named),
            2_000,
            &sink,
        )
        .expect_err("a record that does not match is refused");
        assert!(why.contains(word), "{why}");
    }
    assert!(rx.try_recv().is_err(), "a refused record writes no letter");
    let ended = said(run(
        &store,
        &|_| policy(None),
        &record(&fixture.feature, "t-1", &check),
        2_000,
        &sink,
    ));
    assert_eq!(ended["state"], "passed", "{ended}");
}

/// A record's log is the file the prepared check named, under the store's log folder, and a regular
/// file; anything else is refused.
#[cfg(unix)]
#[test]
fn a_record_whose_log_is_not_its_own_log_path_is_refused() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (sink, _rx) = sink();
    let answer = prepared(&fixture, &store, &sink);
    let check = answer["check"].as_str().expect("the check id").to_string();
    let record = |log: &str| LandCheckAsk::Record {
        run: "run-1".to_string(),
        task: "t-1".to_string(),
        head: fixture.feature.clone(),
        check: check.clone(),
        rc: 0,
        log: log.to_string(),
        took_ms: None,
    };
    let refused = run(
        &store,
        &|_| policy(None),
        &record("/etc/hosts"),
        2_000,
        &sink,
    );
    assert!(refused.is_err(), "{refused:?}");
    let own = PathBuf::from(answer["log"].as_str().expect("the log path"));
    std::fs::create_dir_all(own.parent().expect("a log folder")).expect("the log folder");
    let decoy = fixture.data.join("decoy.log");
    std::fs::write(&decoy, "elsewhere\n").expect("a decoy");
    std::os::unix::fs::symlink(&decoy, &own).expect("a link named as the log");
    let through_link = run(
        &store,
        &|_| policy(None),
        &record(&own.display().to_string()),
        2_000,
        &sink,
    );
    assert!(
        through_link.is_err(),
        "a link named as the log was accepted"
    );
    // The prepared check stands: its folder and row are still there for the right record.
    let row = store.root().join("rows").join(format!("{check}.json"));
    assert!(row.exists(), "the refusals took the prepared row away");
}

/// The stored words of a project become one policy: a blank command is none, a missing or zero
/// limit is the default, and a blank pin is none.
#[test]
fn the_stored_words_name_the_command_its_limit_and_its_pin() {
    let none = Policy::from_settings(None, None, None);
    assert_eq!(none.command, None);
    assert_eq!(none.timeout, DEFAULT_TIMEOUT);
    assert_eq!(none.pinned_base, None);

    let blank = Policy::from_settings(Some("   "), Some(0), Some(" "));
    assert_eq!(blank.command, None);
    assert_eq!(blank.timeout, DEFAULT_TIMEOUT);
    assert_eq!(blank.pinned_base, None);

    let named = Policy::from_settings(Some(" make check "), Some(90), Some("origin/dev"));
    assert_eq!(named.command.as_deref(), Some("make check"));
    assert_eq!(named.timeout, Duration::from_secs(90));
    assert_eq!(named.pinned_base.as_deref(), Some("origin/dev"));
}

/// Measurement, not a judgment: the time one full check takes on the fake window (a clean merge
/// and a passing command, one repository), over twenty runs. Asked for by name with `--ignored`.
#[test]
#[ignore = "measurement: run by name with --ignored --nocapture"]
fn measure_one_check_on_the_fake_window() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut took: Vec<u128> = Vec::new();
    for round in 0..20 {
        let (sink, rx) = sink();
        let started = Instant::now();
        let _ = said(run(
            &store,
            &|_| policy(Some("exit 0")),
            &fixture.merge(&fixture.feature, false),
            1_000 + round,
            &sink,
        ));
        let _ = letter(&rx);
        took.push(started.elapsed().as_millis());
    }
    took.sort_unstable();
    let pick = |share: usize| took[(took.len() * share / 100).min(took.len() - 1)];
    println!(
        "land-check fake window: n={} p50={} ms p95={} ms",
        took.len(),
        pick(50),
        pick(95)
    );
}
