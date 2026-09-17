//! These drive a **real `git`** against real repositories in a tempdir.
//!
//! A faked git would only prove we can format arguments. The behaviour worth
//! pinning is git's: that a worktree really appears on disk, that a leftover
//! branch really blocks a name, and that a removal really refuses to throw away
//! a file the user never committed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use zerocode_core::WorktreeTask;
use zerocode_orchestrator::{NewWorktree, Orchestrator, OrchestratorError, Removal, Worktree};

// ------------------------------------------------------------------ fixtures

/// Run git and insist it worked, returning stdout.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git_succeeds(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("run git")
        .status
        .success()
}

/// A repository with one commit, and an orchestrator whose worktrees land in a
/// sibling directory of the tempdir rather than in the user's home.
fn fixture() -> (TempDir, PathBuf, Orchestrator) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir_all(&root).expect("create project dir");

    git(&root, &["init", "-b", "main"]);
    // Local config only: the machine running these tests may sign commits, or
    // have no identity at all.
    git(&root, &["config", "user.name", "ZeroCode Test"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    fs::write(root.join("README.md"), "hello\n").expect("write file");
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-m", "first"]);

    let orchestrator = Orchestrator::open(&root)
        .expect("open repository")
        .with_worktree_root(dir.path().join("worktrees"));
    (dir, root, orchestrator)
}

fn task(title: &str) -> WorktreeTask {
    WorktreeTask::from_spec(title, None, None)
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    git_succeeds(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

fn listed(orchestrator: &Orchestrator, path: &Path) -> Option<Worktree> {
    orchestrator
        .list()
        .expect("list")
        .into_iter()
        .find(|worktree| {
            worktree.path.canonicalize().ok() == path.canonicalize().ok()
                && path.canonicalize().is_ok()
        })
}

// -------------------------------------------------------------------- create

/// An automation's nightly checkout is cut from the branch the job names, not
/// from whatever HEAD happens to be — and a base that stopped existing since
/// the job was written refuses out loud instead of running the job somewhere
/// it did not mean.
#[test]
fn a_worktree_can_be_cut_from_a_named_base() {
    let (_keep, root, orchestrator) = fixture();

    // A second branch, one commit ahead, so the two start points are
    // distinguishable by content.
    git(&root, &["checkout", "-b", "release"]);
    fs::write(root.join("release.txt"), "released\n").expect("write");
    git(&root, &["add", "release.txt"]);
    git(&root, &["commit", "-m", "release work"]);
    git(&root, &["checkout", "main"]);

    let made = orchestrator
        .create_from(&task("nightly report"), Some("release"))
        .expect("create from release");
    assert!(
        made.path.join("release.txt").exists(),
        "the checkout was not cut from the named base"
    );

    // No base is HEAD — the same behaviour create() always had.
    let plain = orchestrator
        .create_from(&task("plain checkout"), None)
        .expect("create from HEAD");
    assert!(
        !plain.path.join("release.txt").exists(),
        "HEAD grew the other branch's file"
    );

    // git is the validator: a base that does not exist is a refusal with
    // git's own words, not a checkout from somewhere surprising.
    let refused = orchestrator.create_from(&task("doomed"), Some("no-such-branch"));
    assert!(refused.is_err(), "a bogus base was accepted");
}

#[test]
fn a_task_gets_a_worktree_on_a_branch_named_after_it() {
    let (_dir, root, orchestrator) = fixture();

    let worktree = orchestrator
        .create(&task("Refactor the drain gate"))
        .expect("create worktree");

    assert_eq!(
        worktree.branch.as_deref(),
        Some("wt/refactor-the-drain-gate")
    );
    assert!(!worktree.is_main);
    // A real checkout, not just a registration: the committed file is there.
    assert_eq!(
        fs::read_to_string(worktree.path.join("README.md")).expect("read"),
        "hello\n"
    );

    let worktrees = orchestrator.list().expect("list");
    assert_eq!(worktrees.len(), 2, "{worktrees:?}");
    assert!(worktrees[0].is_main, "git lists the repository first");
    assert!(branch_exists(&root, "wt/refactor-the-drain-gate"));
}

/// The three answers the create dialog can give that `HEAD` cannot: a branch
/// somebody typed, an existing branch to check out, and no prefix at all.
///
/// All three go through the SAME loop as an ordinary create — that is the
/// reason `create_with` exists rather than three creates — so what is pinned
/// here is that each one still lands on a real checkout and that a typed name
/// is never quietly suffixed into a different branch.
#[test]
fn a_typed_branch_is_used_verbatim_and_refused_by_name_when_it_is_taken() {
    let (_dir, root, orchestrator) = fixture();

    let made = orchestrator
        .create_with(
            &task("login redirect"),
            &NewWorktree {
                branch: Some("feature/login"),
                ..NewWorktree::default()
            },
        )
        .expect("create with a typed branch");
    assert_eq!(made.branch.as_deref(), Some("feature/login"));
    // The DIRECTORY still comes from the name, under the worktree root.
    assert!(made.path.ends_with("login-redirect"), "{:?}", made.path);
    assert!(branch_exists(&root, "feature/login"));

    // Asking for it a second time is refused by name rather than answered with
    // `feature/login-2`, which would be a different branch than the one typed.
    let refused = orchestrator.create_with(
        &task("login redirect"),
        &NewWorktree {
            branch: Some("feature/login"),
            ..NewWorktree::default()
        },
    );
    assert!(
        matches!(
            refused,
            Err(OrchestratorError::BranchTaken { ref branch }) if branch == "feature/login"
        ),
        "{refused:?}"
    );
}

/// "Reuse branch": check the branch out instead of cutting it, and never cut a
/// second one behind somebody's back when it is not there.
#[test]
fn an_existing_branch_can_be_checked_out_rather_than_cut() {
    let (_dir, root, orchestrator) = fixture();
    git(&root, &["branch", "already-here"]);

    let made = orchestrator
        .create_with(
            &task("pick it up"),
            &NewWorktree {
                branch: Some("already-here"),
                reuse_branch: true,
                ..NewWorktree::default()
            },
        )
        .expect("check out the existing branch");
    assert_eq!(made.branch.as_deref(), Some("already-here"));

    let refused = orchestrator.create_with(
        &task("nothing to pick up"),
        &NewWorktree {
            branch: Some("not-a-branch"),
            reuse_branch: true,
            ..NewWorktree::default()
        },
    );
    assert!(
        matches!(refused, Err(OrchestratorError::NoSuchBranch { .. })),
        "{refused:?}"
    );
    assert!(
        !branch_exists(&root, "not-a-branch"),
        "a reuse that found nothing cut a branch anyway"
    );
}

/// The `none` prefix mode. The branch is the name, with nothing in front of
/// it, and the one place that spells it is `prefixed_branch`.
#[test]
fn a_repository_can_put_its_task_branches_at_the_top_level() {
    let (dir, root, orchestrator) = fixture();
    let bare = orchestrator.without_branch_prefix();
    assert_eq!(bare.prefixed_branch("drain-gate"), "drain-gate");
    assert_eq!(
        Orchestrator::open(&root)
            .expect("open")
            .with_worktree_root(dir.path().join("worktrees"))
            .with_branch_prefix("joe")
            .prefixed_branch("drain-gate"),
        "joe/drain-gate"
    );

    let made = bare.create(&task("drain gate")).expect("create");
    assert_eq!(made.branch.as_deref(), Some("drain-gate"));
    assert!(branch_exists(&root, "drain-gate"));
    assert!(bare.has_branch("drain-gate").expect("ask git"));
    assert!(!bare.has_branch("never-existed").expect("ask git"));
}

/// Two tasks can honestly have the same title. Neither may be refused, and
/// neither may land on the other's worktree.
#[test]
fn a_repeated_title_takes_the_next_suffix_rather_than_failing() {
    let (_dir, root, orchestrator) = fixture();

    let first = orchestrator.create(&task("fix the tests")).expect("first");
    let second = orchestrator.create(&task("fix the tests")).expect("second");

    assert_eq!(first.branch.as_deref(), Some("wt/fix-the-tests"));
    assert_eq!(second.branch.as_deref(), Some("wt/fix-the-tests-2"));
    assert_ne!(first.path, second.path);
    assert!(first.path.is_dir() && second.path.is_dir());
    assert!(branch_exists(&root, "wt/fix-the-tests"));
    assert!(branch_exists(&root, "wt/fix-the-tests-2"));
}

/// A finished task leaves its branch behind after its directory is gone, so the
/// path being free is not enough to conclude the name is.
#[test]
fn a_leftover_branch_with_no_worktree_still_advances_the_name() {
    let (_dir, root, orchestrator) = fixture();
    git(&root, &["branch", "wt/drain-gate"]);

    let worktree = orchestrator.create(&task("drain gate")).expect("create");

    assert_eq!(worktree.branch.as_deref(), Some("wt/drain-gate-2"));
    assert!(
        worktree.path.ends_with("drain-gate-2"),
        "the directory must follow the branch: {:?}",
        worktree.path
    );
}

/// And the mirror case: a directory git knows nothing about.
#[test]
fn a_directory_git_has_never_heard_of_still_advances_the_name() {
    let (dir, _root, orchestrator) = fixture();
    let squatted = dir.path().join("worktrees").join("drain-gate");
    fs::create_dir_all(&squatted).expect("create squatting directory");
    fs::write(squatted.join("mine.txt"), "not git's").expect("write");

    let worktree = orchestrator.create(&task("drain gate")).expect("create");

    assert_eq!(worktree.branch.as_deref(), Some("wt/drain-gate-2"));
    // The squatter is untouched — we stepped around it, we did not clean it up.
    assert_eq!(
        fs::read_to_string(squatted.join("mine.txt")).expect("read"),
        "not git's"
    );
}

/// The classification that replaced stderr matching, checked from the side that
/// matters: a repository with no commits cannot host a worktree at all, and that
/// failure must be **reported**, not mistaken for a taken name and retried under
/// nineteen more of them until it surfaces as a naming error.
#[test]
fn a_genuine_git_failure_is_reported_instead_of_retried_under_another_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("unborn");
    fs::create_dir_all(&root).expect("create");
    git(&root, &["init", "-b", "main"]);

    let orchestrator = Orchestrator::open(&root)
        .expect("an empty repository is still a repository")
        .with_worktree_root(dir.path().join("worktrees"));

    let error = orchestrator
        .create(&task("first task"))
        .expect_err("a repository with no commits has no HEAD to branch from");

    match error {
        OrchestratorError::Git { stderr, .. } => {
            assert!(!stderr.is_empty(), "git's own reason must reach the user");
        }
        other => panic!("the real cause must not be hidden behind a name error: {other}"),
    }
}

/// Task titles in this product are often Korean. Only their ASCII words may
/// survive into the directory on disk and the ref in the repository.
#[test]
fn a_korean_title_produces_an_ascii_branch_and_directory() {
    let (_dir, root, orchestrator) = fixture();

    let worktree = orchestrator
        .create(&task("t-1400 복원 워커는 제 탭으로"))
        .expect("create");

    assert_eq!(worktree.branch.as_deref(), Some("wt/t-1400"));
    assert!(worktree.path.is_dir(), "{:?}", worktree.path);
    assert!(branch_exists(&root, "wt/t-1400"));

    // The ASCII-only path must still round-trip through git's worktree list.
    assert!(listed(&orchestrator, &worktree.path).is_some());
    orchestrator
        .remove(&worktree.path, Removal::ConfirmedIfClean)
        .expect("an ASCII-named worktree must be removable");
    assert!(!worktree.path.exists());
}

/// A pasted title can be pure punctuation. That must still produce a worktree,
/// not an invalid ref and not an empty path component.
#[test]
fn a_title_with_no_usable_characters_still_produces_a_worktree() {
    let (_dir, root, orchestrator) = fixture();

    let worktree = orchestrator.create(&task("!!! ??? ***")).expect("create");

    assert_eq!(worktree.branch.as_deref(), Some("wt/task"));
    assert!(worktree.path.is_dir());
    assert!(branch_exists(&root, "wt/task"));
}

// -------------------------------------------------------------------- remove

/// The data-loss case. An untracked file exists in no commit, so removing the
/// directory is the only place it can go — and the user has to be told what
/// they are about to lose, not just that something is in the way.
#[test]
fn a_dirty_worktree_is_refused_and_the_error_names_what_would_be_lost() {
    let (_dir, _root, orchestrator) = fixture();
    let worktree = orchestrator.create(&task("drain gate")).expect("create");
    fs::write(worktree.path.join("notes.md"), "an hour of thinking").expect("write");

    let error = orchestrator
        .remove(&worktree.path, Removal::ConfirmedIfClean)
        .expect_err("a dirty worktree must not be removed");

    match error {
        OrchestratorError::UncommittedChanges { changes, .. } => {
            assert!(
                changes.iter().any(|change| change.contains("notes.md")),
                "the user must be shown the file: {changes:?}"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
    assert!(
        worktree.path.join("notes.md").exists(),
        "the refusal must not have deleted anything"
    );
    assert!(listed(&orchestrator, &worktree.path).is_some());
}

/// The same worktree, after the user was shown the changes and said yes.
#[test]
fn a_confirmed_discard_removes_the_worktree_and_its_files() {
    let (_dir, _root, orchestrator) = fixture();
    let worktree = orchestrator.create(&task("drain gate")).expect("create");
    fs::write(worktree.path.join("notes.md"), "disposable").expect("write");

    let loss = orchestrator.pending_loss(&worktree.path).expect("status");
    assert_eq!(loss.uncommitted.len(), 1, "{loss:?}");

    orchestrator
        .remove(&worktree.path, Removal::ConfirmedDiscardingChanges)
        .expect("remove");

    assert!(!worktree.path.exists());
    assert_eq!(orchestrator.list().expect("list").len(), 1);
}

/// A lock is a person's explicit request to keep a checkout, including when
/// the caller claims to have confirmed discarding its contents. The guard must
/// run before `--force`, so a cleanup path cannot turn a portable or offline
/// worktree into a deletion.
#[test]
fn a_locked_worktree_is_refused_before_any_removal_mode() {
    let (_dir, root, orchestrator) = fixture();
    let worktree = orchestrator.create(&task("keep this")).expect("create");
    let path = worktree.path.to_string_lossy().into_owned();
    git(
        &root,
        &["worktree", "lock", "--reason", "portable checkout", &path],
    );

    for removal in [
        Removal::ConfirmedIfClean,
        Removal::ConfirmedDiscardingChanges,
    ] {
        let error = orchestrator
            .remove(&worktree.path, removal)
            .expect_err("a locked worktree must never be removed");
        assert!(
            matches!(error, OrchestratorError::LockedWorktree { .. }),
            "unexpected error: {error}"
        );
        assert!(worktree.path.is_dir(), "the locked checkout was deleted");
    }
}

/// `git worktree prune` has the same safety promise as removal: a missing
/// checkout can still be intentionally locked, so pruning must leave its
/// administrative record for the person who locked it.
#[test]
fn pruning_keeps_a_locked_missing_worktree_record() {
    let (_dir, root, orchestrator) = fixture();
    let worktree = orchestrator
        .create(&task("offline checkout"))
        .expect("create");
    let path = worktree.path.to_string_lossy().into_owned();
    git(&root, &["worktree", "lock", "--reason", "offline", &path]);
    fs::remove_dir_all(&worktree.path).expect("remove missing checkout");

    orchestrator.prune().expect("prune");

    let retained = orchestrator
        .list()
        .expect("list")
        .into_iter()
        .find(|candidate| candidate.path == worktree.path)
        .expect("locked worktree metadata was pruned");
    assert!(retained.locked);
}

/// The quiet data-loss path, and the reason `PendingLoss` has two lists.
///
/// `.env` is ignored, so `git status` says nothing about it and
/// `git worktree remove` deletes it **without `--force` and without a word** —
/// verified against real git, not assumed. A confirmation dialog that showed
/// only `git status` would therefore promise "nothing will be lost" and then
/// destroy the one file in the worktree nobody can regenerate.
#[test]
fn ignored_files_are_reported_even_though_git_status_hides_them() {
    let (_dir, root, orchestrator) = fixture();
    fs::write(root.join(".gitignore"), ".env\ntarget/\n").expect("write gitignore");
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "-m", "ignore local config"]);

    let worktree = orchestrator.create(&task("drain gate")).expect("create");
    fs::write(worktree.path.join(".env"), "SECRET=xyz").expect("write .env");
    fs::create_dir_all(worktree.path.join("target")).expect("create target");
    fs::write(worktree.path.join("target").join("big.bin"), "artifact").expect("write artifact");

    let loss = orchestrator.pending_loss(&worktree.path).expect("status");
    assert!(
        loss.uncommitted.is_empty(),
        "git considers this worktree clean: {loss:?}"
    );
    assert!(
        loss.ignored.iter().any(|path| path.contains(".env")),
        "the irreplaceable file must be reported: {loss:?}"
    );
    assert!(
        loss.ignored.iter().any(|path| path.contains("target")),
        "{loss:?}"
    );
    // An ignored directory collapses to one entry, so a built worktree does not
    // report forty thousand files.
    assert_eq!(loss.ignored.len(), 2, "{loss:?}");
    assert!(!loss.is_empty());

    // And the documented consequence: a "clean" removal still deletes them, so
    // the caller has to have shown that list.
    orchestrator
        .remove(&worktree.path, Removal::ConfirmedIfClean)
        .expect("no uncommitted changes, so this is allowed");
    assert!(!worktree.path.exists());
}

/// Cleanup must not double as branch deletion: a lane can finish with commits
/// that exist nowhere else, and the branch is what keeps them reachable.
#[test]
fn removing_a_clean_worktree_leaves_its_branch_behind() {
    let (_dir, root, orchestrator) = fixture();
    let worktree = orchestrator.create(&task("drain gate")).expect("create");

    fs::write(worktree.path.join("work.txt"), "committed work").expect("write");
    git(&worktree.path, &["add", "work.txt"]);
    git(&worktree.path, &["commit", "-m", "lane work"]);
    let head = git(&worktree.path, &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    orchestrator
        .remove(&worktree.path, Removal::ConfirmedIfClean)
        .expect("a committed worktree is clean");

    assert!(!worktree.path.exists());
    assert!(branch_exists(&root, "wt/drain-gate"));
    // The commit is still reachable, which is the point of keeping the branch.
    assert_eq!(
        git(&root, &["rev-parse", "wt/drain-gate"]).trim(),
        head,
        "the lane's commit must survive the cleanup"
    );
}

/// The other end of the same promise. A detached worktree has no branch, so
/// "the branch survives the directory" does not hold for it: `git status` is
/// empty, git removes it without complaint, and the commit becomes reachable
/// from nothing. `ConfirmedIfClean` must therefore refuse it, and say which
/// commit the user would have to rescue.
#[test]
fn a_detached_worktree_is_refused_because_no_branch_would_survive_it() {
    let (dir, root, orchestrator) = fixture();
    let detached = dir.path().join("detached");
    git(
        &root,
        &["worktree", "add", "--detach", detached.to_str().unwrap()],
    );

    fs::write(
        detached.join("only-copy.txt"),
        "work that exists nowhere else",
    )
    .expect("write");
    git(&detached, &["add", "only-copy.txt"]);
    git(&detached, &["commit", "-m", "unreachable once removed"]);
    let head = git(&detached, &["rev-parse", "HEAD"]).trim().to_string();

    let error = orchestrator
        .remove(&detached, Removal::ConfirmedIfClean)
        .expect_err("a detached worktree must not be removed as if it were clean");

    match error {
        OrchestratorError::DetachedHead { head: reported, .. } => {
            assert_eq!(reported, head, "the user needs the commit to rescue it");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert!(detached.join("only-copy.txt").exists());

    // Discarding is still allowed, because that variant *is* the confirmation.
    orchestrator
        .remove(&detached, Removal::ConfirmedDiscardingChanges)
        .expect("an explicit discard may remove it");
    assert!(!detached.exists());
}

#[test]
fn the_repository_itself_is_never_removed() {
    let (_dir, root, orchestrator) = fixture();

    let error = orchestrator
        .remove(&root, Removal::ConfirmedDiscardingChanges)
        .expect_err("the main worktree must be refused");

    assert!(
        matches!(error, OrchestratorError::RefusesToRemoveMainWorktree { .. }),
        "unexpected error: {error}"
    );
    assert!(root.join("README.md").exists());
}

#[test]
fn a_path_that_is_not_a_worktree_is_reported_rather_than_deleted() {
    let (dir, _root, orchestrator) = fixture();
    let stranger = dir.path().join("not-a-worktree");
    fs::create_dir_all(&stranger).expect("create");
    fs::write(stranger.join("keep.txt"), "keep me").expect("write");

    let error = orchestrator
        .remove(&stranger, Removal::ConfirmedDiscardingChanges)
        .expect_err("an unknown path must be refused");

    assert!(
        matches!(error, OrchestratorError::UnknownWorktree { .. }),
        "unexpected error: {error}"
    );
    assert!(stranger.join("keep.txt").exists());
}

/// A user who deletes a worktree in Finder leaves git's records behind. Pruning
/// clears them, and the name becomes available again.
#[test]
fn pruning_clears_records_for_a_directory_deleted_outside_git() {
    let (_dir, _root, orchestrator) = fixture();
    let worktree = orchestrator.create(&task("drain gate")).expect("create");
    fs::remove_dir_all(&worktree.path).expect("delete by hand");

    assert_eq!(
        orchestrator.list().expect("list").len(),
        2,
        "git still has a record until it is pruned"
    );

    orchestrator.prune().expect("prune");
    assert_eq!(orchestrator.list().expect("list").len(), 1);
}

// ----------------------------------------------------------------- ref rules

/// The slug rules are asserted against **git itself** rather than against our
/// reading of the ref format. `check-ref-format` is the same code that would
/// reject the branch at creation time, so this cannot drift from what git
/// actually accepts.
#[test]
fn no_hostile_title_can_produce_a_name_git_would_reject() {
    let (_dir, root, orchestrator) = fixture();

    let hostile = [
        "..",
        "-leading dash",
        "trailing.lock",
        "at@{brace}",
        "tilde~caret^colon:question?star*bracket[",
        "back\\slash",
        "control\u{7}chars\u{1b}here",
        "HEAD",
        "@",
        "123456",
        "   ",
        "!!!",
        "CON",
        "combining e\u{301}",
        "zero\u{200d}width",
        "rtl\u{202e}override",
        "türkçe İstanbul",
        "emoji 🚀 title",
        "slash/inside",
        &"long ".repeat(200),
        "가나다",
        ".hidden",
        "dot.dot.dot",
    ];

    for title in hostile {
        let slug = zerocode_orchestrator::slugify(title);
        assert!(!slug.is_empty(), "empty slug from {title:?}");
        let branch = format!("{}/{}", orchestrator.branch_prefix(), slug);
        assert!(
            git_succeeds(&root, &["check-ref-format", "--branch", &branch]),
            "git rejects {branch:?}, derived from {title:?}"
        );
    }
}

/// And the names really are creatable, not merely well-formed: a title that is
/// only a device name, only digits, or only punctuation still yields a worktree.
#[test]
fn awkward_titles_still_produce_distinct_creatable_worktrees() {
    let (_dir, _root, orchestrator) = fixture();

    let first = orchestrator.create(&task("CON")).expect("device name");
    let second = orchestrator.create(&task("!!!")).expect("punctuation only");
    let third = orchestrator.create(&task("123456")).expect("digits only");

    // The first two both fall back to the same base, so the suffix must part them.
    assert_eq!(first.branch.as_deref(), Some("wt/task"));
    assert_eq!(second.branch.as_deref(), Some("wt/task-2"));
    assert_eq!(third.branch.as_deref(), Some("wt/123456"));
    for worktree in [&first, &second, &third] {
        assert!(worktree.path.is_dir(), "{:?}", worktree.path);
    }
}

// ---------------------------------------------------------------------- open

/// The override has to start at `open` itself — opening is a git invocation,
/// so an override that only applied to later commands would run the default
/// git once anyway, which is exactly the defect this pins. The shim logs every
/// call it receives and delegates to the real git, so the assertion is that
/// *both* the resolution and the work went through it.
#[cfg(unix)]
#[test]
fn a_custom_git_binary_carries_from_open_through_every_command() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, _root, orchestrator) = fixture();
    let marker = dir.path().join("git-calls.log");
    let shim = dir.path().join("custom-git");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec git \"$@\"\n",
            marker.display()
        ),
    )
    .expect("write shim");
    let mut perms = fs::metadata(&shim).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&shim, perms).expect("chmod");

    let custom = Orchestrator::open_with_git(orchestrator.repo_root(), &shim)
        .expect("open through the shim")
        .with_worktree_root(dir.path().join("worktrees"));
    let worktree = custom.create(&task("drain gate")).expect("create");
    assert!(worktree.path.is_dir());

    let calls = fs::read_to_string(&marker).expect("the shim must have been invoked");
    assert!(
        calls.lines().next().is_some_and(|first| {
            first.contains("rev-parse --path-format=absolute --show-toplevel --git-common-dir")
        }),
        "opening itself must go through the custom binary: {calls:?}"
    );
    assert!(
        calls.lines().any(|line| line.contains("worktree add")),
        "later commands must keep using it: {calls:?}"
    );
}

/// The catalog's repository snapshot starts git twice: one combined
/// `rev-parse` for the checkout and common directories, then one worktree
/// listing. Before the cache/direct-config path, the same facts took four
/// starts (`open`, `list`, `shared_root`, and `git config`). The shim counts
/// actual child processes, so this pins the before/after boundary rather than
/// only counting Rust method calls.
#[cfg(unix)]
#[test]
fn catalog_repository_facts_take_two_git_starts() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, root, _orchestrator) = fixture();
    git(&root, &["config", "branch.catalog.base", "main"]);
    let marker = dir.path().join("git-calls.log");
    let shim = dir.path().join("counting-git");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec git \"$@\"\n",
            marker.display()
        ),
    )
    .expect("write counting git shim");
    let mut perms = fs::metadata(&shim)
        .expect("counting git metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&shim, perms).expect("chmod counting git shim");

    let custom = Orchestrator::open_with_git(&root, &shim).expect("open through counting git");
    let listed = custom.list().expect("list through counting git");
    assert_eq!(listed.len(), 1);
    let shared = custom.shared_root().expect("cached shared root");
    assert_eq!(
        shared,
        root.canonicalize().expect("canonical repository root")
    );
    assert_eq!(
        custom.creation_bases().get("catalog").map(String::as_str),
        Some("main")
    );

    let calls = fs::read_to_string(&marker).expect("counting git must have been invoked");
    assert_eq!(
        calls.lines().count(),
        2,
        "catalog facts should use one combined open and one list: {calls:?}"
    );
    assert!(
        calls.lines().any(|line| line
            .contains("rev-parse --path-format=absolute --show-toplevel --git-common-dir")),
        "open did not combine the two repository roots: {calls:?}"
    );
    assert!(
        calls
            .lines()
            .any(|line| line.contains("worktree list --porcelain")),
        "catalog did not list worktrees: {calls:?}"
    );
}

/// And the failure half of the same contract: a binary that does not exist is
/// an error, never a silent fallback to whatever `PATH` holds.
#[test]
fn a_missing_custom_git_binary_is_reported_not_silently_replaced() {
    let (dir, root, _orchestrator) = fixture();
    let missing = dir.path().join("no-such-git");

    let error = Orchestrator::open_with_git(&root, &missing)
        .expect_err("a nonexistent binary must not open anything");

    assert!(
        matches!(error, OrchestratorError::GitUnavailable(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn a_directory_outside_any_repository_is_reported_as_such() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = dir.path().join("plain");
    fs::create_dir_all(&outside).expect("create");

    // The premise, put to git rather than assumed. A temporary directory that
    // happened to sit inside a repository would make this test pass without
    // testing anything, so that shows up as a failure instead of a false green.
    assert!(
        !git_succeeds(&outside, &["rev-parse", "--show-toplevel"]),
        "the temporary directory is itself inside a repository, so this test \
         cannot say anything about a path outside one"
    );

    let error = Orchestrator::open(&outside).expect_err("a plain directory has no repository");
    assert!(
        matches!(error, OrchestratorError::NotAGitRepository { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn opening_a_subdirectory_finds_the_repository_root() {
    let (_dir, root, _orchestrator) = fixture();
    let nested = root.join("crates").join("deep");
    fs::create_dir_all(&nested).expect("create nested");

    let orchestrator = Orchestrator::open(&nested).expect("open from a subdirectory");
    assert_eq!(
        orchestrator.repo_root().canonicalize().expect("canonical"),
        root.canonicalize().expect("canonical")
    );
}
