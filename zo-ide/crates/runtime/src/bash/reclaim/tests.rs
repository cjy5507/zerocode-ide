//! What the door promises: a session on a full disk can still see and still
//! free, and can still do nothing else.

use super::*;

/// The exact command a full disk refused on this machine, and the ones a
/// session reaches for next.
#[test]
fn the_commands_that_diagnose_a_full_disk_still_run() {
    for command in [
        "df -h /Users/dev",
        "df -g /",
        "du -sg /Users/dev/2026/zerocode/target",
        "ls -la /tmp",
        "find /Users/dev/.zo -name '*.jsonl'",
        "stat -f %z big.log",
        "lsof +L1",
        "pwd",
    ] {
        assert!(frees_or_looks(command), "a look was refused: {command}");
    }
}

/// And the ones that end it.
#[test]
fn the_commands_that_free_space_still_run() {
    for command in [
        "rm -rf target/debug/incremental",
        "rmdir /tmp/empty",
        "truncate -s 0 huge.log",
        "cargo clean",
        "git worktree remove --force /Users/dev/wt/gone",
        "git gc --prune=now",
        "/bin/rm -rf /tmp/scratch",
    ] {
        assert!(frees_or_looks(command), "a reclaim was refused: {command}");
    }
}

/// A program that can free is not a licence for the program.
///
/// `cargo clean` frees; `cargo build` is the long build the floor exists to
/// stop. The first argument settles it, and a program whose row demands one
/// never runs without it.
#[test]
fn a_program_admitted_for_one_verb_is_not_admitted_for_its_others() {
    assert!(frees_or_looks("cargo clean"));
    for command in [
        "cargo build",
        "cargo test",
        "cargo",
        "git commit -m x",
        "git clone https://example.invalid/r",
        "git",
    ] {
        assert!(!frees_or_looks(command), "admitted: {command}");
    }
}

/// One simple command, or nothing.
///
/// This is the rule that matters: `rm -rf big && cargo build` frees a little
/// and then does exactly what the floor forbids. No chain, no expansion, no
/// redirect, no second line.
#[test]
fn nothing_that_could_become_another_command_is_admitted() {
    for command in [
        "rm -rf big && cargo build",
        "rm -rf big; cargo build",
        "rm -rf big || cargo build",
        "du -sg * | sort -rn",
        "rm -rf $(cat targets.txt)",
        "rm -rf `cat targets.txt`",
        "df -h > /tmp/out",
        "df -h < /dev/null",
        "rm -rf {a,b}",
        "df -h\ncargo build",
        "df -h & cargo build",
    ] {
        assert!(!frees_or_looks(command), "a chain was admitted: {command}");
    }
}

/// Everything else stays refused, including the ordinary and the harmless.
///
/// The floor is not asking whether a command is dangerous. It is asking
/// whether it can only look or only free, and "cat a file" is neither.
#[test]
fn an_ordinary_command_is_still_refused() {
    for command in [
        "cat Cargo.toml",
        "npm install",
        "just verify",
        "python3 script.py",
        "mkdir -p /tmp/new",
        "cp a b",
        "mv a b",
        "",
        "   ",
    ] {
        assert!(!frees_or_looks(command), "admitted: {command}");
    }
}

/// A relative path to the program is refused, because what it names depends
/// on a working directory this door cannot see.
#[test]
fn only_an_absolute_path_may_name_the_program() {
    assert!(frees_or_looks("/usr/bin/du -sg ."));
    assert!(!frees_or_looks("./rm -rf everything"));
    assert!(!frees_or_looks("../bin/rm -rf everything"));
    assert!(!frees_or_looks("bin/rm -rf everything"));
}
