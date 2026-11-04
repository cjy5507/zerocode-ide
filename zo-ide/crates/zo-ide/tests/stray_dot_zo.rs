//! No crate folder holds a `.zo/` a test left there (t-78045).
//!
//! `cargo test` runs a crate's tests with the crate's folder as the working
//! folder, and zo writes its bookkeeping — `.zo/turns`, `.zo/dream`,
//! `.zo/artifacts`, `.zo/registries` — in the working folder, so a test that
//! lets zo write without giving it a folder of its own leaves them in
//! `crates/<crate>/`. `.gitignore` hides `.zo/` everywhere, so nothing shows in
//! `git status`: on this machine 10 MB sat in `crates/tools/.zo`, a few hundred
//! KB in two other crates, and in every worktree a worker was cut for, until a
//! person went looking.
//!
//! This is the `stray-zo` recipe of the justfile, which `verify` runs after
//! `test` and `e2e`: only there have the other test binaries run. Run alone,
//! on a checkout whose tests have not run, it finds nothing and says so. It
//! fails with the folders it found and what to do about them; a fresh
//! checkout, or a worktree cut for a task, has none.

use std::path::{Path, PathBuf};

/// Folders that never hold a test's leftover, or are too big to walk: the
/// build output and dependencies.
const SKIPPED: [&str; 3] = ["target", "node_modules", ".git"];

/// Every `.zo` directory under `root` (not entered once found: what it holds
/// is its own), by path.
fn stray_dot_zo(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `file_type` does not follow links: a link to a folder is not walked.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            if name == ".zo" {
                found.push(path);
            } else if !SKIPPED.iter().any(|skipped| name == *skipped) {
                pending.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The `crates` folder of the workspace this test belongs to.
fn crates_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/zo-ide has a parent")
        .to_path_buf()
}

#[test]
fn no_test_leaves_a_dot_zo_in_a_crate_folder() {
    let crates = crates_folder();
    let stray = stray_dot_zo(&crates);
    assert!(
        stray.is_empty(),
        "a test left zo's bookkeeping in {} crate folder(s): {stray:#?}\n\
         `.gitignore` hides them. Find the test by the file names inside (a session id, an \
         artifact's sha) and give it a folder of its own: a temp folder as the working folder or \
         root, or `ZO_ARTIFACT_STORE` / `ZO_TRACE_ROOT` pointed at one. Delete the folders \
         afterwards; nothing of value is in them.",
        stray.len()
    );
}

/// The walk itself: a `.zo` at any depth is found, and not entered; `target`
/// and a link are not walked.
#[test]
fn the_walk_finds_a_dot_zo_at_any_depth_and_skips_the_build_output() {
    let root = tempfile::tempdir().expect("a crates folder");
    let crates = root.path();
    for dir in [
        "runtime/.zo/turns",
        "tools/tests/fixtures/.zo",
        "zo-ide/target/debug/.zo",
        "api/src",
    ] {
        std::fs::create_dir_all(crates.join(dir)).expect("the tree");
    }
    assert_eq!(
        stray_dot_zo(crates),
        vec![crates.join("runtime/.zo"), crates.join("tools/tests/fixtures/.zo")],
        "a .zo is found where it stands, once, and the build output is not walked"
    );
    assert!(stray_dot_zo(&crates.join("api")).is_empty());
}
