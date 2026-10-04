//! The git this crate and the shell's `LocalVcs` start in the background must
//! not rewrite `.git/index`.
//!
//! A plain `git status` refreshes the stat data kept in the index and writes it
//! back under `index.lock` (git-status, BACKGROUND REFRESH). A `commit`, `add` or
//! `merge` a person or a worker starts in that checkout meanwhile is refused with
//! "Unable to create '.git/index.lock': File exists". The proof is the index
//! file itself: a tracked file whose mtime alone changed makes a plain `status`
//! write new bytes, and a door that takes no optional lock leaves them alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use zerocode_core::host::{LocalVcs, Vcs};
use zerocode_orchestrator::Orchestrator;

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env_remove("GIT_OPTIONAL_LOCKS")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository whose index holds a fresh stat entry for `tracked.txt`, and
/// then a mtime that no longer matches it: the content is the same, the size is
/// the same, only the clock moved.
fn stale_stat_repository() -> (tempfile::TempDir, PathBuf) {
    assert!(
        std::env::var_os("GIT_OPTIONAL_LOCKS").is_none(),
        "GIT_OPTIONAL_LOCKS is already set in this environment, so a plain `git status` \
         would not rewrite the index either and this test would prove nothing — unset it"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir_all(&root).expect("create project");
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    fs::write(root.join("tracked.txt"), "one\n").expect("write");
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-q", "-m", "first"]);
    // Settles the stat entries, so the later change is the only one.
    git(&root, &["status", "--porcelain"]);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(root.join("tracked.txt"))
        .expect("open");
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .expect("move the mtime");
    drop(file);
    (dir, root)
}

fn index_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join(".git").join("index")).expect("read the index")
}

#[test]
fn a_plain_git_status_rewrites_the_index_of_a_changed_mtime() {
    let (_dir, root) = stale_stat_repository();
    let before = index_bytes(&root);
    git(&root, &["status", "--porcelain"]);
    assert_ne!(
        before,
        index_bytes(&root),
        "the control: without the setting a plain status refreshes the index, \
         so the tests below prove something"
    );
}

#[test]
fn pending_loss_does_not_rewrite_the_index() {
    let (_dir, root) = stale_stat_repository();
    let orchestrator = Orchestrator::open(&root).expect("open");
    let before = index_bytes(&root);
    orchestrator.pending_loss(&root).expect("pending loss");
    assert_eq!(before, index_bytes(&root), "pending_loss wrote the index");
}

#[test]
fn the_local_vcs_status_does_not_rewrite_the_index() {
    let (_dir, root) = stale_stat_repository();
    let before = index_bytes(&root);
    LocalVcs
        .text(&root, &["status", "--porcelain=v2", "--branch"])
        .expect("status");
    assert_eq!(
        before,
        index_bytes(&root),
        "LocalVcs status wrote the index"
    );
}

#[test]
fn the_local_vcs_diff_numstat_does_not_rewrite_the_index() {
    let (_dir, root) = stale_stat_repository();
    let before = index_bytes(&root);
    LocalVcs.text(&root, &["diff", "--numstat"]).expect("diff");
    assert_eq!(before, index_bytes(&root), "LocalVcs diff wrote the index");
}

#[test]
fn the_panels_numstat_and_diff_against_head_do_not_rewrite_the_index() {
    let (_dir, root) = stale_stat_repository();
    let orchestrator = Orchestrator::open(&root).expect("open");
    let before = index_bytes(&root);
    orchestrator.numstat(&root).expect("numstat");
    orchestrator
        .diff(&root, &[Path::new("tracked.txt")])
        .expect("diff");
    assert_eq!(
        before,
        index_bytes(&root),
        "numstat or diff against HEAD wrote the index"
    );
}
