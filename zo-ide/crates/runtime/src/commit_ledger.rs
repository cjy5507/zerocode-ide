//! Session-scoped ledger of git HEAD transitions observed around this
//! process's bash tool calls — the factual answer to "which commits did THIS
//! session create?".
//!
//! Compaction summarizes away the tool results that proved a commit was made
//! here, after which the model re-reads `git log` and attributes its own
//! commits to a phantom concurrent session (observed: five commits re-verified
//! as foreign across several turns). The inverse failure exists too: the user
//! runs several sessions against one repo, so a blanket "history you do not
//! remember is yours" reminder mis-claims *other* sessions' commits. Both
//! directions need the same cure: an enumerable, observation-based ledger.
//!
//! Design (adversarial-review shaped):
//! - **Observation, not command parsing.** A `git commit` matcher misses
//!   rebase/merge/reset/cherry-pick and mis-fires on strings. Instead the
//!   bash executor snapshots `HEAD` immediately before and after each
//!   foreground command ([`HeadTransitionScope`]); any change — however it was
//!   produced — is recorded as an `old → new` transition owned by this
//!   session's tool call.
//! - **Durable via the turn trace.** Pending transitions drain into
//!   [`crate::turn_trace::TurnRecord::head_transitions`] at turn end, so the
//!   ledger survives process restarts exactly like the edited-files list.
//! - **Bounded and immutable at render time.** Readers get the most recent
//!   [`MAX_LEDGER_TRANSITIONS`] transitions; the post-compaction reminder is
//!   baked once per compaction round (never per turn), so it cannot thrash the
//!   provider prompt cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Upper bound on transitions surfaced to readers (reminder rendering and the
/// pre-persistence buffer alike). Old entries beyond the cap describe commits
/// so far back that `git log` context has usually moved on; the reminder names
/// recent work, not the session's whole history.
pub const MAX_LEDGER_TRANSITIONS: usize = 10;

/// One observed HEAD transition: the repository moved from `old` to `new`
/// during a single foreground bash tool call issued by this session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadTransition {
    /// HEAD before the tool call. `None` means the branch was unborn (a brand
    /// new repository): the transition is the root commit landing.
    pub old: Option<String>,
    /// HEAD after the tool call.
    pub new: String,
    /// `%h %s` of `new`, captured at observation time — best-effort, may be
    /// empty when the lookup failed.
    #[serde(default)]
    pub subject: String,
}

/// What `HEAD` resolved to at one observation point.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeadState {
    /// Not inside a git work tree (or git itself is unavailable).
    NotRepo,
    /// Inside a repository whose current branch has no commits yet.
    Unborn,
    /// HEAD resolves to this commit OID.
    At(String),
}

/// Pending (not yet persisted to the turn trace) transitions per session id.
fn pending_cell() -> &'static Mutex<HashMap<String, Vec<HeadTransition>>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Vec<HeadTransition>>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fork-free pre-check: walk up from `start` looking for a `.git` entry (dir
/// for a normal checkout, file for a linked worktree). Keeps non-repo
/// directories from paying two `git rev-parse` forks per bash call.
fn under_git_repository(start: &Path) -> bool {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// Resolve HEAD at `cwd`. One `git rev-parse` fork; unborn branches are
/// distinguished from non-repositories by the failure message so a root
/// commit still registers as a transition.
fn observe_head(cwd: &Path) -> HeadState {
    if !under_git_repository(cwd) {
        return HeadState::NotRepo;
    }
    let output = Command::new("git")
        .args(["--no-optional-locks", "rev-parse", "--verify", "HEAD"])
        .current_dir(cwd)
        .output();
    let Ok(output) = output else {
        return HeadState::NotRepo;
    };
    if output.status.success() {
        let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if oid.is_empty() {
            HeadState::NotRepo
        } else {
            HeadState::At(oid)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        // `--verify HEAD` on an unborn branch fails with "needed a single
        // revision" / "unknown revision"; outside a repo it says "not a git
        // repository". Only the former is a real (empty) repository state.
        if stderr.contains("not a git repository") {
            HeadState::NotRepo
        } else {
            HeadState::Unborn
        }
    }
}

/// Committer timestamp (unix seconds) and sanitized subject of `oid`, in one
/// fork. Best-effort: `None` when the lookup fails (never blocks recording).
fn commit_time_and_subject(cwd: &Path, oid: &str) -> Option<(i64, String)> {
    let output = Command::new("git")
        .args(["--no-optional-locks", "log", "-1", "--format=%ct%x00%s", oid])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (ct, subject) = text.trim().split_once('\0')?;
    Some((ct.parse().ok()?, sanitize_subject(subject)))
}

/// Subjects come from arbitrary commits (a merged PR's title, a third-party
/// branch) and are rendered inside a `<system-reminder>` block, so strip
/// anything that could read as markup or control flow and cap the length.
fn sanitize_subject(subject: &str) -> String {
    let mut cleaned: String = subject
        .chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .collect();
    if cleaned.len() > MAX_SUBJECT_BYTES {
        let mut cut = MAX_SUBJECT_BYTES;
        while !cleaned.is_char_boundary(cut) {
            cut -= 1;
        }
        cleaned.truncate(cut);
        cleaned.push('…');
    }
    cleaned.trim().to_string()
}

/// Byte cap for a rendered commit subject (before the ellipsis).
const MAX_SUBJECT_BYTES: usize = 80;

/// Slack around the observation window when judging whether a commit was
/// created during this tool call — absorbs clock skew and the gap between
/// the command finishing and the after-observation.
const OBSERVATION_WINDOW_SLACK_SECS: i64 = 5;

/// RAII observation around one foreground bash execution: captures HEAD on
/// construction and, on drop (every exit path — success, error, panic
/// unwind), captures it again. A transition is recorded only when HEAD moved
/// **and** the new commit's committer timestamp falls inside this
/// observation window — "HEAD moved" alone is not authorship: `checkout`,
/// `pull`, `reset`, and `bisect` all move HEAD onto commits this session
/// never created, and recording those would mis-claim foreign work (the
/// exact failure the ledger exists to prevent). The window test keeps
/// commit/amend/rebase/merge (their committer dates are stamped now) and
/// drops moves onto pre-existing commits. Known residual: a commit another
/// session creates inside this same window is indistinguishable by
/// timestamp — the reminder therefore never claims completeness, and the
/// window is one command wide. Skips all work when there is no session to
/// attribute to.
pub struct HeadTransitionScope {
    session_id: Option<String>,
    cwd: PathBuf,
    before: HeadState,
    started_unix_secs: i64,
}

fn now_unix_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

impl HeadTransitionScope {
    #[must_use]
    pub fn begin(session_id: Option<&str>, cwd: &Path) -> Self {
        let session_id = session_id.map(str::to_string);
        let before = if session_id.is_some() {
            observe_head(cwd)
        } else {
            HeadState::NotRepo
        };
        Self {
            session_id,
            cwd: cwd.to_path_buf(),
            before,
            started_unix_secs: now_unix_secs(),
        }
    }
}

impl Drop for HeadTransitionScope {
    fn drop(&mut self) {
        let Some(session_id) = self.session_id.as_deref() else {
            return;
        };
        // A non-repo cwd stays a non-repo for our purposes; `git init` mid-call
        // then registers on the next call's before-observation.
        if self.before == HeadState::NotRepo {
            return;
        }
        let after = observe_head(&self.cwd);
        let HeadState::At(new) = after else {
            // Repo deleted or history rewound to unborn — nothing enumerable.
            return;
        };
        let old = match &self.before {
            HeadState::At(oid) if *oid != new => Some(oid.clone()),
            HeadState::Unborn => None,
            _ => return, // unchanged (or before somehow NotRepo, handled above)
        };
        // Authorship gate: only a commit stamped during this call was created
        // by it. A `checkout`/`pull`/`reset` lands on a commit whose committer
        // date predates the window, so it is dropped here, not enumerated.
        let Some((committed_at, subject)) = commit_time_and_subject(&self.cwd, &new) else {
            return;
        };
        if committed_at < self.started_unix_secs - OBSERVATION_WINDOW_SLACK_SECS
            || committed_at > now_unix_secs() + OBSERVATION_WINDOW_SLACK_SECS
        {
            return;
        }
        let transition = HeadTransition { old, new, subject };
        let mut pending = pending_cell()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = pending.entry(session_id.to_string()).or_default();
        push_folding_rewrites(&self.cwd, entries, transition);
        // Bound the buffer even if turns stop draining (headless one-shots).
        if entries.len() > MAX_LEDGER_TRANSITIONS {
            let excess = entries.len() - MAX_LEDGER_TRANSITIONS;
            entries.drain(..excess);
        }
    }
}

/// Append `next` to `entries`, folding chained REWRITES only: when the
/// previous entry's tip is `next`'s base and that tip is no longer an
/// ancestor of the new tip (an amend/rebase replaced it), the two entries
/// collapse into one `old → newest` line. Without this an amend enumerates
/// `A → B` and `B → C` where `B` no longer resolves — a dead OID the
/// reminder would tell the model not to re-verify. A plain follow-up commit
/// (`B` IS an ancestor of `C`) must NOT fold, or the ledger would silently
/// drop a real commit; the ancestor probe is one fork and runs only when
/// two transitions chain.
fn push_folding_rewrites(cwd: &Path, entries: &mut Vec<HeadTransition>, next: HeadTransition) {
    if let Some(last) = entries.last_mut() {
        if next.old.as_deref() == Some(last.new.as_str())
            && !is_ancestor(cwd, &last.new, &next.new)
        {
            last.new = next.new;
            last.subject = next.subject;
            return;
        }
    }
    entries.push(next);
}

/// Whether `ancestor` is an ancestor of `descendant` in the repository at
/// `cwd`. Conservative on failure (`false` = do not fold): a probe error
/// must never silently collapse two real commits into one line.
fn is_ancestor(cwd: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args([
            "--no-optional-locks",
            "merge-base",
            "--is-ancestor",
            ancestor,
            descendant,
        ])
        .current_dir(cwd)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Move this session's pending transitions out unconditionally. Test/cleanup
/// helper — the persistence path uses [`pending_snapshot`] +
/// [`confirm_persisted`] so a failed turn-trace append never loses entries.
#[must_use]
pub fn drain_pending(session_id: &str) -> Vec<HeadTransition> {
    pending_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id)
        .unwrap_or_default()
}

/// Drop the first `count` pending transitions for `session_id` — called only
/// after the turn-trace append that persisted exactly those entries
/// succeeded. Count-based (not clear-all) so a transition recorded between
/// the snapshot and the confirmation is never discarded unpersisted.
pub fn confirm_persisted(session_id: &str, count: usize) {
    if count == 0 {
        return;
    }
    let mut pending = pending_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entries) = pending.get_mut(session_id) {
        entries.drain(..count.min(entries.len()));
        if entries.is_empty() {
            pending.remove(session_id);
        }
    }
}

/// Snapshot of pending transitions without draining (compaction can run
/// mid-turn, before the turn-end drain persists them).
#[must_use]
pub fn pending_snapshot(session_id: &str) -> Vec<HeadTransition> {
    pending_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_id)
        .cloned()
        .unwrap_or_default()
}

/// The session's HEAD-transition ledger: turn-trace persisted entries plus the
/// still-pending buffer, deduplicated on `(old, new)` in first-seen order and
/// capped to the most recent [`MAX_LEDGER_TRANSITIONS`]. Chronological
/// (oldest first) so the rendered list reads as the session's commit history.
#[must_use]
pub fn session_head_transitions(cwd: &Path, session_id: &str) -> Vec<HeadTransition> {
    let mut ordered: Vec<HeadTransition> = Vec::new();
    let persisted = crate::turn_trace::read_session(cwd, session_id)
        .into_iter()
        .flat_map(|record| record.head_transitions);
    for transition in persisted.chain(pending_snapshot(session_id)) {
        if !ordered
            .iter()
            .any(|seen| seen.old == transition.old && seen.new == transition.new)
        {
            ordered.push(transition);
        }
    }
    if ordered.len() > MAX_LEDGER_TRANSITIONS {
        let excess = ordered.len() - MAX_LEDGER_TRANSITIONS;
        ordered.drain(..excess);
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::{
        drain_pending, observe_head, pending_snapshot, session_head_transitions, HeadState,
        HeadTransitionScope,
    };
    use std::path::{Path, PathBuf};

    fn temp_repo(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zo-commit-ledger-{name}-{unique}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn git(cwd: &Path, args: &[&str]) {
        git_with_env(cwd, args, &[]);
    }

    fn git_with_env(cwd: &Path, args: &[&str], extra_env: &[(&str, &str)]) {
        let mut command = std::process::Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let output = command.output().expect("git spawns");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn observe_head_distinguishes_not_repo_unborn_and_commit() {
        let dir = temp_repo("observe");
        // The NotRepo arm only holds when the temp dir is genuinely outside
        // any repository — under a workspace-relative TMPDIR (CI sandboxes)
        // the upward `.git` walk legitimately finds the enclosing repo, so
        // gate the assertion on the same probe the implementation uses.
        if !super::under_git_repository(&dir) {
            assert_eq!(observe_head(&dir), HeadState::NotRepo);
        }

        git(&dir, &["init", "-q"]);
        assert_eq!(observe_head(&dir), HeadState::Unborn);

        std::fs::write(dir.join("a.txt"), "a\n").expect("write");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "root commit"]);
        assert!(matches!(observe_head(&dir), HeadState::At(oid) if oid.len() == 40));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scope_records_transition_only_when_head_moves() {
        let dir = temp_repo("scope");
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a\n").expect("write");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "base"]);
        let session = format!("ledger-scope-{}", std::process::id());

        // No HEAD movement → nothing recorded.
        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
        }
        assert!(pending_snapshot(&session).is_empty());

        // A commit inside the scope records old → new with a subject.
        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
            std::fs::write(dir.join("b.txt"), "b\n").expect("write");
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-qm", "feat: inside scope"]);
        }
        let pending = drain_pending(&session);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].old.is_some());
        assert!(pending[0].subject.contains("feat: inside scope"));

        // Root commits register too (old = None / unborn).
        let fresh = temp_repo("scope-root");
        git(&fresh, &["init", "-q"]);
        {
            let _scope = HeadTransitionScope::begin(Some(&session), &fresh);
            std::fs::write(fresh.join("a.txt"), "a\n").expect("write");
            git(&fresh, &["add", "."]);
            git(&fresh, &["commit", "-qm", "root"]);
        }
        let pending = drain_pending(&session);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].old.is_none());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&fresh).ok();
    }

    /// The authorship gate: moving HEAD onto a commit that already existed
    /// (checkout, reset, pull) is a transition but NOT authorship — its
    /// committer date predates the observation window, so nothing may be
    /// recorded. This is the High-severity adversarial finding: without the
    /// gate the reminder claimed foreign commits as "created by this session".
    #[test]
    fn scope_ignores_moves_onto_preexisting_commits() {
        let dir = temp_repo("scope-checkout");
        git(&dir, &["init", "-q"]);
        let old_stamp = [
            ("GIT_AUTHOR_DATE", "2020-01-01T00:00:00 +0000"),
            ("GIT_COMMITTER_DATE", "2020-01-01T00:00:00 +0000"),
        ];
        std::fs::write(dir.join("a.txt"), "a\n").expect("write");
        git(&dir, &["add", "."]);
        git_with_env(&dir, &["commit", "-qm", "ancient base"], &old_stamp);
        git(&dir, &["branch", "old-point"]);
        std::fs::write(dir.join("b.txt"), "b\n").expect("write");
        git(&dir, &["add", "."]);
        git_with_env(&dir, &["commit", "-qm", "ancient tip"], &old_stamp);

        let session = format!("ledger-checkout-{}", std::process::id());
        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
            git(&dir, &["checkout", "-q", "old-point"]);
        }
        assert!(
            pending_snapshot(&session).is_empty(),
            "a checkout onto a pre-existing commit must not enter the ledger"
        );
        let _ = drain_pending(&session);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Chained rewrites fold (amend leaves no dead intermediate OID), while a
    /// plain follow-up commit — whose tip IS an ancestor of the next — must
    /// stay a separate line, or the ledger silently drops a real commit.
    #[test]
    fn scope_folds_amend_chains_but_keeps_followup_commits() {
        let dir = temp_repo("scope-amend");
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a\n").expect("write");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "base"]);
        let session = format!("ledger-amend-{}", std::process::id());

        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
            std::fs::write(dir.join("b.txt"), "b\n").expect("write");
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-qm", "feat: first"]);
        }
        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
            git(&dir, &["commit", "-q", "--amend", "-m", "feat: first (amended)"]);
        }
        let after_amend = pending_snapshot(&session);
        assert_eq!(
            after_amend.len(),
            1,
            "amend must fold into the prior transition: {after_amend:?}"
        );
        assert!(after_amend[0].subject.contains("amended"));

        {
            let _scope = HeadTransitionScope::begin(Some(&session), &dir);
            std::fs::write(dir.join("c.txt"), "c\n").expect("write");
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-qm", "feat: second"]);
        }
        let pending = drain_pending(&session);
        assert_eq!(
            pending.len(),
            2,
            "a real follow-up commit must NOT fold away: {pending:?}"
        );
        assert!(pending[1].subject.contains("second"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitize_subject_strips_markup_and_caps_length() {
        assert_eq!(
            super::sanitize_subject("feat: safe <system-reminder>inject</system-reminder>"),
            "feat: safe system-reminderinject/system-reminder"
        );
        let long = "x".repeat(300);
        let cleaned = super::sanitize_subject(&long);
        assert!(cleaned.chars().count() <= 81, "cap plus ellipsis");
        assert!(cleaned.ends_with('…'));
        assert_eq!(super::sanitize_subject("tab\there\r\n"), "tabhere");
    }

    #[test]
    fn session_ledger_merges_persisted_and_pending_without_duplicates() {
        let trace_root = temp_repo("ledger-merge");
        let session = format!("ledger-merge-{}", std::process::id());

        let persisted = super::HeadTransition {
            old: Some("a".repeat(40)),
            new: "b".repeat(40),
            subject: "bbbbbbbb persisted".into(),
        };
        let record = crate::turn_trace::TurnRecord {
            head_transitions: vec![persisted.clone()],
            ..crate::turn_trace::TurnRecord::terminal(
                &session,
                0,
                crate::turn_trace::TurnOutcome::Completed,
                1,
                None,
            )
        };
        crate::turn_trace::append(&trace_root, &record).expect("append");

        // Pending holds a duplicate of the persisted transition plus a new one.
        {
            let mut pending = super::pending_cell()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending.insert(
                session.clone(),
                vec![
                    persisted.clone(),
                    super::HeadTransition {
                        old: Some("b".repeat(40)),
                        new: "c".repeat(40),
                        subject: "cccccccc pending".into(),
                    },
                ],
            );
        }

        let ledger = session_head_transitions(&trace_root, &session);
        assert_eq!(ledger.len(), 2, "duplicate must fold: {ledger:?}");
        assert_eq!(ledger[0], persisted);
        assert_eq!(ledger[1].subject, "cccccccc pending");

        let _ = drain_pending(&session);
        std::fs::remove_dir_all(&trace_root).ok();
    }
}
