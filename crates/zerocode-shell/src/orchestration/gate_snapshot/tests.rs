use super::*;

/// One git command in `repo`, its output; a refusal fails the test with git's words.
fn run(repo: &Path, args: &[&str]) -> String {
    let output = crate::proc::quiet_command("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A checkout with one commit, a file in it and an ignore rule.
fn checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a checkout");
    run(dir.path(), &["init", "-q"]);
    run(dir.path(), &["config", "user.name", "Fixture"]);
    run(
        dir.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    std::fs::write(dir.path().join("a.txt"), "one\n").expect("a file");
    std::fs::write(dir.path().join(".gitignore"), "ignored.log\n").expect("a rule");
    run(dir.path(), &["add", "-A"]);
    run(dir.path(), &["commit", "-q", "-m", "first"]);
    dir
}

#[test]
fn a_changed_and_a_new_file_are_saved_and_the_workers_own_tree_is_untouched() {
    let dir = checkout();
    let repo = dir.path();
    std::fs::write(repo.join("a.txt"), "two\n").expect("a change");
    std::fs::write(repo.join("b.txt"), "new\n").expect("a new file");
    std::fs::write(repo.join("ignored.log"), "noise\n").expect("an ignored file");
    let status_before = run(repo, &["status", "--porcelain"]);
    let head_before = run(repo, &["rev-parse", "HEAD"]);
    let index_before = run(repo, &["diff", "--cached", "--name-only"]);

    let saved = save(repo, "w-1", 1).expect("a snapshot");
    let reference = saved.expect("there was something to save");
    assert_eq!(reference, "refs/zerocode/checkpoints/w-1/1");

    assert_eq!(run(repo, &["status", "--porcelain"]), status_before);
    assert_eq!(run(repo, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        run(repo, &["diff", "--cached", "--name-only"]),
        index_before
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).expect("still there"),
        "two\n"
    );

    assert_eq!(run(repo, &["show", &format!("{reference}:a.txt")]), "two\n");
    assert_eq!(run(repo, &["show", &format!("{reference}:b.txt")]), "new\n");
    let saved_files = run(repo, &["ls-tree", "-r", "--name-only", &reference]);
    assert!(
        !saved_files.contains("ignored.log"),
        "the ignore rules are the worker's own: {saved_files}"
    );
    assert_eq!(
        run(repo, &["rev-parse", &format!("{reference}^")]),
        head_before,
        "the restore point stands on the commit the worker was on"
    );
}

#[test]
fn a_clean_tree_has_nothing_to_save() {
    let dir = checkout();
    assert_eq!(save(dir.path(), "w-1", 1), Ok(None));
    assert_eq!(
        run(dir.path(), &["for-each-ref", "refs/zerocode/"]),
        "",
        "and no ref was made"
    );
}

#[test]
fn only_the_newest_checkpoints_of_a_worker_are_kept_and_another_workers_are_not_touched() {
    let dir = checkout();
    let repo = dir.path();
    std::fs::write(repo.join("a.txt"), "other\n").expect("a change");
    save(repo, "w-2", 1).expect("another worker's");
    for number in 1..=5_u32 {
        std::fs::write(repo.join("a.txt"), format!("edit {number}\n")).expect("a change");
        save(repo, "w-1", number).expect("a snapshot");
    }
    let kept = run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/zerocode/checkpoints/w-1/",
        ],
    );
    let mut kept: Vec<&str> = kept.lines().collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        [
            "refs/zerocode/checkpoints/w-1/3",
            "refs/zerocode/checkpoints/w-1/4",
            "refs/zerocode/checkpoints/w-1/5",
        ]
    );
    assert!(
        run(repo, &["for-each-ref", "refs/zerocode/checkpoints/w-2/"]).contains("w-2/1"),
        "another worker's checkpoint is its own"
    );
}

#[test]
fn what_git_cannot_do_is_said_and_nothing_is_left_behind() {
    let empty = tempfile::tempdir().expect("an empty repository");
    run(empty.path(), &["init", "-q"]);
    let said = save(empty.path(), "w-1", 1).expect_err("no commit to stand on");
    assert!(said.contains("failed"), "{said}");

    let gone = std::path::Path::new("/nonexistent/zerocode-checkpoint-fixture");
    assert!(
        save(gone, "w-1", 1)
            .expect_err("no such checkout")
            .contains("gone")
    );

    let dir = checkout();
    std::fs::write(dir.path().join("a.txt"), "two\n").expect("a change");
    save(dir.path(), "w-1", 1).expect("a snapshot");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".git"))
        .expect("the git dir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("zerocode-checkpoint")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_worker_id_a_ref_cannot_carry_is_refused_before_git_is_asked() {
    let dir = checkout();
    for worker in ["", "w 1", "../w-1", ".hidden", "w-1/../x", "w-1:x"] {
        assert!(
            save(dir.path(), worker, 1).is_err(),
            "{worker:?} named a ref"
        );
    }
    assert_eq!(
        run(dir.path(), &["for-each-ref"]).lines().count(),
        1,
        "only the branch"
    );
}
