//! Whether a landed task's commit is on the remote (t-73910 W4-2).
//!
//! The ledger has no column for it: a coordinator writes `mergeHead` when the
//! work lands on the base, and pushing the base is a second step nobody
//! records. This asks git the question the reclaimer asks of a checkout —
//! [`Orchestrator::commits_beyond`] — of the remote's default branch as this
//! repository last heard it ([`ORIGIN_HEAD_REF`]): no commit of the merge
//! beyond it is `confirmed`, some is `not_yet`, and a ref or a commit that
//! cannot be read is `unknown`. Nothing here fetches, so the answer is as
//! fresh as the last fetch or push: `not_yet` can be behind the remote, and a
//! `confirmed` is never ahead of it.
//!
//! It rides the beat once a minute and puts the question to git for two
//! merges at most, those never asked first and then the longest unasked; a
//! confirmed one is not asked again. The desk is dressed
//! every beat from what was answered, with no git.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use tauri::{AppHandle, Manager};
use zerocode_orchestrator::Orchestrator;

use crate::orchestration::desk::DeskSnapshot;
use crate::worktree_runtime::ORIGIN_HEAD_REF;
use crate::{AppState, ShellStateExt};

/// How often the beat looks. A push is minutes behind a landing at the
/// soonest, so a minute is prompt.
const LOOK_EVERY_MS: i64 = 60_000;

/// How long an answer that is not `confirmed` stands before git is asked
/// again.
const ASK_AGAIN_AFTER_MS: i64 = 5 * 60_000;

/// How many merges one look puts to git: each is a few short processes on
/// the beat's own thread.
const ASKS_PER_LOOK: usize = 2;

/// Where a task's `mergeHead` stands against the remote's default branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OnRemote {
    Confirmed,
    NotYet,
    Unknown,
}

/// What git says of `merge_head` against [`ORIGIN_HEAD_REF`] in the
/// orchestrator's repository.
pub(crate) fn on_remote(orchestrator: &Orchestrator, merge_head: &str) -> OnRemote {
    match orchestrator.commits_beyond(orchestrator.repo_root(), ORIGIN_HEAD_REF, merge_head) {
        Ok(Some(0)) => OnRemote::Confirmed,
        Ok(Some(_)) => OnRemote::NotYet,
        Ok(None) | Err(_) => OnRemote::Unknown,
    }
}

/// The answers so far, for one repository: each merge's, and when it was
/// given.
#[derive(Debug, Default)]
struct Book {
    repo: Option<PathBuf>,
    answers: HashMap<String, (OnRemote, i64)>,
}

impl Book {
    /// The merges among `heads` to put to git now: never asked, or not
    /// confirmed and asked long enough ago — those never asked first, then the
    /// longest unasked, in `heads`' order between equals, so that however many
    /// merges stand unconfirmed each is reached in turn. Forgets every merge
    /// no longer in `heads`, and all of them when the repository changed.
    fn due(&mut self, repo: &Path, heads: &BTreeSet<String>, now_ms: i64) -> Vec<String> {
        if self.repo.as_deref() != Some(repo) {
            self.answers.clear();
            self.repo = Some(repo.to_path_buf());
        }
        self.answers.retain(|head, _| heads.contains(head));
        let mut due: Vec<(&String, Option<i64>)> = heads
            .iter()
            .filter_map(|head| match self.answers.get(head) {
                None => Some((head, None)),
                Some((OnRemote::Confirmed, _)) => None,
                Some((_, at_ms)) => (now_ms.saturating_sub(*at_ms) >= ASK_AGAIN_AFTER_MS)
                    .then_some((head, Some(*at_ms))),
            })
            .collect();
        // Stable: `None` sorts before any time, and equals keep their order.
        due.sort_by_key(|(_, asked_at)| *asked_at);
        due.into_iter()
            .take(ASKS_PER_LOOK)
            .map(|(head, _)| head.clone())
            .collect()
    }

    fn answer(&self, head: &str) -> Option<OnRemote> {
        self.answers.get(head).map(|(said, _)| *said)
    }
}

fn book() -> &'static Mutex<Book> {
    static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
    BOOK.get_or_init(Mutex::default)
}

/// The merges the desk's tasks were written as landing in.
fn merge_heads(desk: &DeskSnapshot) -> BTreeSet<String> {
    desk.tasks
        .iter()
        .filter_map(|task| task.progress.as_ref()?.merge_head.clone())
        .collect()
}

/// Called from the beat, after the board's reading is published. Git is
/// asked off the book's lock, so the desk's dressing never waits on it.
pub(crate) fn sweep(app: &AppHandle, now_ms: i64) {
    static LAST_LOOK_MS: AtomicI64 = AtomicI64::new(i64::MIN);
    let last = LAST_LOOK_MS.load(Ordering::SeqCst);
    if now_ms.saturating_sub(last) < LOOK_EVERY_MS
        || LAST_LOOK_MS
            .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }
    let heads = merge_heads(&crate::orchestration::board_ledger_snapshot().desk);
    let Some(orchestrator) = app.state::<AppState>().orchestrator() else {
        return;
    };
    look(&orchestrator, &heads, now_ms);
}

/// One look of [`sweep`] at `heads` in the orchestrator's repository.
fn look(orchestrator: &Orchestrator, heads: &BTreeSet<String>, now_ms: i64) {
    let due = book().lock().unwrap_or_else(PoisonError::into_inner).due(
        orchestrator.repo_root(),
        heads,
        now_ms,
    );
    for head in due {
        let said = on_remote(orchestrator, &head);
        book()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .answers
            .insert(head, (said, now_ms));
    }
}

/// Lays each task's answer on its desk row, from the book alone. A merge
/// git was not asked of yet carries none.
pub(crate) fn dress_desk(desk: &mut DeskSnapshot) {
    let book = book().lock().unwrap_or_else(PoisonError::into_inner);
    for task in &mut desk.tasks {
        task.on_remote = task
            .progress
            .as_ref()
            .and_then(|progress| progress.merge_head.as_deref())
            .and_then(|head| book.answer(head));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo)
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

    /// A bare repository standing in for the remote, and a clone of it with
    /// one commit pushed — no network, two folders.
    struct Bench {
        _temp: tempfile::TempDir,
        clone: PathBuf,
    }

    impl Bench {
        fn open() -> Self {
            let temp = tempfile::tempdir().expect("a remote and a clone");
            let real = dunce::canonicalize(temp.path()).expect("the real temporary path");
            let remote = real.join("remote.git");
            let clone = real.join("clone");
            std::fs::create_dir_all(&remote).expect("remote");
            git(&remote, &["init", "-q", "--bare", "-b", "main"]);
            git(
                &real,
                &["clone", "-q", remote.to_str().expect("utf-8"), "clone"],
            );
            git(&clone, &["config", "user.name", "ZeroCode Test"]);
            git(&clone, &["config", "user.email", "test@zerocode"]);
            git(&clone, &["symbolic-ref", "HEAD", "refs/heads/main"]);
            git(&clone, &["commit", "--allow-empty", "-q", "-m", "first"]);
            git(&clone, &["push", "-q", "-u", "origin", "main"]);
            git(&clone, &["remote", "set-head", "origin", "main"]);
            Self { _temp: temp, clone }
        }

        fn commit(&self, message: &str) -> String {
            git(
                &self.clone,
                &["commit", "--allow-empty", "-q", "-m", message],
            );
            git(&self.clone, &["rev-parse", "HEAD"])
        }

        fn orchestrator(&self) -> Orchestrator {
            Orchestrator::open(&self.clone).expect("open the clone")
        }
    }

    #[test]
    fn a_merge_head_on_origin_is_remote_confirmed_and_one_missing_is_not() {
        let bench = Bench::open();
        let pushed = bench.commit("landed and pushed");
        git(&bench.clone, &["push", "-q", "origin", "main"]);
        let local = bench.commit("landed, not pushed");
        let open = bench.orchestrator();
        assert_eq!(on_remote(&open, &pushed), OnRemote::Confirmed);
        assert_eq!(on_remote(&open, &local), OnRemote::NotYet);
        assert_eq!(
            on_remote(&open, "0123456789abcdef0123456789abcdef01234567"),
            OnRemote::Unknown,
            "a commit this repository does not hold"
        );
        git(&bench.clone, &["remote", "set-head", "origin", "--delete"]);
        assert_eq!(
            on_remote(&open, &pushed),
            OnRemote::Unknown,
            "with no default branch of the remote's, nothing is guessed"
        );
    }

    /// More unconfirmed merges than the looks of one asking-again span can
    /// reach must not leave the last of them for ever unasked: each look takes
    /// the longest unasked first (the review R5, W4-2).
    #[test]
    fn unconfirmed_merges_beyond_the_looks_of_a_span_are_still_asked_in_turn() {
        let mut book = Book::default();
        let repo = Path::new("/repo");
        let heads: BTreeSet<String> = (0..12).map(|n| format!("{n:02}")).collect();
        let mut asked: HashMap<String, Vec<i64>> = HashMap::new();
        for beat in 0..60 {
            let now_ms = beat * LOOK_EVERY_MS;
            for head in book.due(repo, &heads, now_ms) {
                asked.entry(head.clone()).or_default().push(now_ms);
                book.answers.insert(head, (OnRemote::NotYet, now_ms));
            }
        }
        let never: Vec<&String> = heads
            .iter()
            .filter(|head| !asked.contains_key(*head))
            .collect();
        assert!(never.is_empty(), "never asked in an hour: {never:?}");
        let longest_gap = asked
            .values()
            .flat_map(|times| times.windows(2).map(|pair| pair[1] - pair[0]))
            .max()
            .expect("every merge was asked more than once in an hour");
        assert!(
            longest_gap
                <= ASK_AGAIN_AFTER_MS
                    .max(heads.len().div_ceil(ASKS_PER_LOOK) as i64 * LOOK_EVERY_MS),
            "a merge waited {longest_gap} ms between two asks"
        );
    }

    #[test]
    fn a_look_asks_two_merges_at_most_and_never_again_one_confirmed() {
        let mut book = Book::default();
        let repo = Path::new("/repo");
        let heads: BTreeSet<String> = ["a", "b", "c"].map(String::from).into();
        let first = book.due(repo, &heads, 0);
        assert_eq!(first, ["a", "b"]);
        book.answers.insert("a".into(), (OnRemote::Confirmed, 0));
        book.answers.insert("b".into(), (OnRemote::NotYet, 0));
        assert_eq!(book.due(repo, &heads, 1_000), ["c"]);
        book.answers.insert("c".into(), (OnRemote::Unknown, 1_000));
        assert!(book.due(repo, &heads, 2_000).is_empty());
        assert_eq!(
            book.due(repo, &heads, ASK_AGAIN_AFTER_MS + 1_000),
            ["b", "c"],
            "what is not confirmed is asked again; what is, never"
        );
        let fewer: BTreeSet<String> = ["b".to_string()].into();
        book.due(repo, &fewer, ASK_AGAIN_AFTER_MS + 2_000);
        assert_eq!(book.answer("a"), None, "a merge off the desk is forgotten");
        assert_eq!(
            book.due(Path::new("/other"), &heads, ASK_AGAIN_AFTER_MS + 3_000),
            ["a", "b"],
            "another repository's answers are not this one's"
        );
        assert!(book.answers.is_empty());
    }
}
