//! t-24545: the git this app starts in the background takes no optional lock.
//!
//! A plain `git status` refreshes the stat data in `.git/index` and writes it
//! back under `index.lock`; a `commit`, `add` or `merge` that a person or a
//! worker starts in that checkout meanwhile is refused. The cure is one
//! environment variable, `GIT_OPTIONAL_LOCKS=0`, set by one function —
//! `zerocode_core::host::lock_free_git` — that every git this app starts goes
//! through. A new git started by hand, beside it, is the one that was missed:
//! this contract reads every `crates/*/src` and fails on it.
//!
//! What it does not read, and why:
//! - the tests: from a `#[cfg(test)] mod` to the end of the file, and the
//!   files that are wholly test code (`main_unit_tests.rs`, `orchestration/tests.rs`,
//!   `orchestration/gate_snapshot/tests.rs`);
//! - `build.rs`, which is not under `src` and runs at build time;
//! - `crates/zo-ide`, the zo runtime, which has its own background git
//!   (`--no-optional-locks` in `commit_ledger.rs` and `deep_gate.rs`).

use std::path::{Path, PathBuf};

use crate::support::strip_rust_comments;

const EXEMPT_CRATES: &[(&str, &str)] = &[(
    "zo-ide",
    "the zo runtime has its own background git, outside this task",
)];

const EXEMPT_FILES: &[(&str, &str)] = &[
    ("zerocode-shell/src/main_unit_tests.rs", "test code"),
    ("zerocode-shell/src/orchestration/tests.rs", "test code"),
    (
        "zerocode-shell/src/orchestration/gate_snapshot/tests.rs",
        "test code",
    ),
];

/// The three ways a child is started in this repository.
const SPAWNERS: &[&str] = &["Command::new(", "quiet_command(", "quiet_tokio_command("];

fn sources() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates directory")
        .to_path_buf();
    let mut found = Vec::new();
    for member in std::fs::read_dir(&crates).expect("crates") {
        let member = member.expect("a crate").path();
        let name = member
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        if EXEMPT_CRATES.iter().any(|(it, _)| *it == name) || !member.join("src").is_dir() {
            continue;
        }
        let mut pending = vec![member.join("src")];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("source directory") {
                let path: PathBuf = entry.expect("source entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().and_then(|it| it.to_str()) == Some("rs") {
                    let rel = path
                        .strip_prefix(&crates)
                        .expect("under crates")
                        .to_string_lossy()
                        .replace('\\', "/");
                    if EXEMPT_FILES.iter().any(|(it, _)| *it == rel) {
                        continue;
                    }
                    found.push((rel, std::fs::read_to_string(&path).expect("source")));
                }
            }
        }
    }
    found.sort();
    assert!(
        found.len() > 200,
        "the walk found only {} files",
        found.len()
    );
    found
}

/// The shipped part of a file: comments gone, the trailing `#[cfg(test)] mod`
/// cut away, and every space removed so that a line `rustfmt` broke does not
/// hide a call from the reading.
fn shipped_compact(source: &str) -> String {
    let code = strip_rust_comments(source);
    let lines: Vec<&str> = code.lines().collect();
    let cut = lines
        .windows(2)
        .position(|pair| opens_a_test_module(pair[0], pair[1]))
        .unwrap_or(lines.len());
    lines[..cut]
        .concat()
        .chars()
        .filter(|glyph| !glyph.is_whitespace())
        .collect()
}

/// `#[cfg(test)]`, `#[cfg(all(test, unix))]`… on the line before a `mod`.
fn opens_a_test_module(attribute: &str, next: &str) -> bool {
    let attribute = attribute.trim();
    let next = next.trim_start();
    attribute.starts_with("#[cfg(")
        && attribute.contains("test")
        && !attribute.contains("not(test")
        && ["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "]
            .iter()
            .any(|opens| next.starts_with(opens))
}

/// Where the compact text starts a child that is git, and no `lock_free_git(`
/// stands just before it.
fn bare_git_children(compact: &str) -> Vec<String> {
    let mut bare = Vec::new();
    for spawner in SPAWNERS {
        for (at, _) in compact.match_indices(spawner) {
            let argument: String = compact[at + spawner.len()..].chars().take(48).collect();
            let is_git = argument.starts_with("\"git\"")
                || argument.starts_with("\"git.exe\"")
                || argument.starts_with("git)")
                || argument.starts_with("&git)")
                || argument.contains("GIT_EXECUTABLE");
            if !is_git {
                continue;
            }
            let before: String = compact[..at]
                .chars()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if !before.contains("lock_free_git(") {
                bare.push(format!("{spawner}{argument}"));
            }
        }
    }
    bare
}

#[test]
fn every_git_the_app_starts_goes_through_lock_free_git() {
    let mut offenders = Vec::new();
    for (name, source) in sources() {
        for bare in bare_git_children(&shipped_compact(&source)) {
            offenders.push(format!("{name}: {bare}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "git started without `zerocode_core::host::lock_free_git(` — a background `status` \
         there rewrites .git/index under index.lock and refuses a commit or add that a \
         person or a worker starts at the same time:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_contract_sees_a_bare_git_and_a_wrapped_one() {
    assert_eq!(
        bare_git_children(&shipped_compact(
            "fn f() { Command::new(\"git\").args([]); }"
        ))
        .len(),
        1
    );
    assert_eq!(
        bare_git_children(&shipped_compact(
            "fn f() { crate::proc::quiet_command(zerocode_orchestrator::GIT_EXECUTABLE) }"
        ))
        .len(),
        1
    );
    assert!(
        bare_git_children(&shipped_compact(
            "fn f() { zerocode_core::host::lock_free_git(\n    crate::proc::quiet_command(\n        \"git\",\n    ))\n }"
        ))
        .is_empty()
    );
    assert!(bare_git_children(&shipped_compact("fn f() { Command::new(\"gh\") }")).is_empty());
    for opens in [
        "#[cfg(all(test, unix))]\nmod tests",
        "#[cfg(test)]\npub(crate) mod tests",
    ] {
        let source = format!("fn f() {{}}\n{opens} {{ fn g() {{ Command::new(\"git\") }} }}");
        assert!(
            bare_git_children(&shipped_compact(&source)).is_empty(),
            "{opens}"
        );
    }
    let shipped_after = "#[cfg(not(test))]\nmod real { fn g() { Command::new(\"git\") } }";
    assert_eq!(bare_git_children(&shipped_compact(shipped_after)).len(), 1);
    assert!(
        bare_git_children(&shipped_compact(
            "fn f() {}\n#[cfg(test)]\nmod tests { fn g() { Command::new(\"git\") } }"
        ))
        .is_empty()
    );
}
