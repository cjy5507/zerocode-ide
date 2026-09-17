//! Directories a repository asks every workspace to SHARE with the primary
//! checkout, rather than get its own copy of.
//!
//! `node_modules` is the reason the feature exists: one install serves every
//! workspace, and a workspace that had to run its own would take minutes to
//! become useful. Orca links these and says why it never clone-copies them —
//! *"a clone would give each worktree its own node_modules, and the point of a
//! shared directory is that one install serves every worktree"*
//! (`worktree-symlinks.ts:335-346`).
//!
//! The list itself is made safe in [`zerocode_core::project::shared_directories`]
//! — a repository file is somebody else's input, and what it names gets linked
//! into a checkout. What lives here is the filesystem: creating the links after
//! a workspace is made, and taking them away before one is removed.
//!
//! ## Why removal matters as much as creation
//!
//! `git worktree remove` refuses a worktree holding untracked files, and a
//! symlink pointing at the primary's `node_modules` looks untracked to git. So
//! without the unlink pass, configuring this feature would make **every**
//! ordinary deletion fail with "use force" — Orca hit that and wrote the
//! reason down (`worktree-symlinks.ts:389-400`). Only actual symlinks are
//! removed: a real file or directory that happens to share a name is somebody's
//! work and is left exactly where it is.

use std::path::{Path, PathBuf};

/// What one entry's linking did, for the report a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Linked {
    /// The link now stands.
    Made,
    /// The primary has no such directory — nothing to share yet. Not a
    /// failure: a repository can name `node_modules` before anybody has
    /// installed anything.
    SourceMissing,
    /// Something is already at that path in the workspace. Left alone.
    Occupied,
    /// The link could not be made, with the reason.
    Failed(String),
}

/// Where an entry points from and to.
fn ends(primary: &Path, workspace: &Path, entry: &str) -> Option<(PathBuf, PathBuf)> {
    // The entry arrives already normalised, but this is the last gate before a
    // path is acted on and it costs one comparison.
    if entry.is_empty() || entry.starts_with('/') || entry.split('/').any(|part| part == "..") {
        return None;
    }
    Some((
        entry
            .split('/')
            .fold(primary.to_path_buf(), |at, part| at.join(part)),
        entry
            .split('/')
            .fold(workspace.to_path_buf(), |at, part| at.join(part)),
    ))
}

/// Point each shared directory of `workspace` at the one in `primary`.
///
/// Failures are per entry and never stop the others: a stale name in a
/// repository file must not be able to fail a workspace that git already
/// created (`worktree-symlinks.ts:348-352`).
pub(crate) fn link_shared(primary: &Path, workspace: &Path, entries: &[String]) -> Vec<Linked> {
    entries
        .iter()
        .map(|entry| {
            let Some((from, to)) = ends(primary, workspace, entry) else {
                return Linked::Failed("경로가 아닙니다".to_string());
            };
            // `symlink_metadata` rather than `exists`: a broken link left by an
            // earlier run is something at that path, and replacing it blindly
            // would be this code overwriting its own mistake without saying so.
            if to.symlink_metadata().is_ok() {
                return Linked::Occupied;
            }
            if !from.exists() {
                return Linked::SourceMissing;
            }
            if let Some(parent) = to.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return Linked::Failed(error.to_string());
            }
            match make_link(&from, &to) {
                Ok(()) => Linked::Made,
                Err(error) => Linked::Failed(error.to_string()),
            }
        })
        .collect()
}

#[cfg(unix)]
fn make_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(windows)]
fn make_link(from: &Path, to: &Path) -> std::io::Result<()> {
    // A directory symlink on Windows needs its own call, and without
    // Developer Mode it needs a privilege most people do not have — the error
    // travels to the report rather than being swallowed.
    if from.is_dir() {
        std::os::windows::fs::symlink_dir(from, to)
    } else {
        std::os::windows::fs::symlink_file(from, to)
    }
}

/// Take the links away before `git worktree remove` looks at the checkout.
///
/// Only symlinks. Anything else at the same path is somebody's own work, and
/// the whole point of checking is that this code must never be the reason a
/// person's file disappeared.
pub(crate) fn unlink_shared(workspace: &Path, entries: &[String]) -> usize {
    entries
        .iter()
        .filter(|entry| {
            let Some((_, at)) = ends(Path::new(""), workspace, entry) else {
                return false;
            };
            let Ok(held) = at.symlink_metadata() else {
                return false;
            };
            if !held.file_type().is_symlink() {
                return false;
            }
            // On Windows a directory symlink is removed with `remove_dir`; on
            // Unix `remove_file` takes both.
            let gone = if cfg!(windows) && at.is_dir() {
                std::fs::remove_dir(&at)
            } else {
                std::fs::remove_file(&at)
            };
            gone.is_ok()
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let primary = root.path().join("repo");
        let workspace = root.path().join("wt");
        std::fs::create_dir_all(&primary).expect("primary");
        std::fs::create_dir_all(&workspace).expect("workspace");
        (root, primary, workspace)
    }

    /// One install serves every workspace: the link points at the primary's
    /// directory rather than copying it.
    #[test]
    fn a_shared_directory_points_at_the_primary_rather_than_copying_it() {
        let (_root, primary, workspace) = sandbox();
        std::fs::create_dir_all(primary.join("node_modules")).expect("modules");
        std::fs::write(primary.join("node_modules/marker"), b"one install").expect("marker");
        // A nested entry: the parent is made on the way.
        std::fs::create_dir_all(primary.join("apps/web")).expect("apps");
        std::fs::write(primary.join("apps/web/.env"), b"KEY=1").expect("env");

        let entries = vec!["node_modules".to_string(), "apps/web/.env".to_string()];
        let done = link_shared(&primary, &workspace, &entries);
        assert_eq!(done, vec![Linked::Made, Linked::Made]);

        // The file is reachable through the workspace, and it is the SAME
        // file — writing through the link is seen by the primary.
        assert_eq!(
            std::fs::read_to_string(workspace.join("node_modules/marker")).expect("read"),
            "one install"
        );
        std::fs::write(workspace.join("apps/web/.env"), b"KEY=2").expect("write through");
        assert_eq!(
            std::fs::read_to_string(primary.join("apps/web/.env")).expect("read primary"),
            "KEY=2",
            "the workspace got its own copy instead of sharing"
        );
        assert!(
            workspace
                .join("node_modules")
                .symlink_metadata()
                .expect("link")
                .file_type()
                .is_symlink()
        );
    }

    /// Re-sharing a workspace is idempotent. In particular, a symlink already
    /// occupying the target must not be followed and linked again, which could
    /// turn a source/target pair into a cycle.
    #[cfg(unix)]
    #[test]
    fn an_existing_shared_symlink_is_not_relinked() {
        let (_root, primary, workspace) = sandbox();
        std::fs::create_dir_all(primary.join("node_modules")).expect("modules");
        let entries = vec!["node_modules".to_string()];

        assert_eq!(
            link_shared(&primary, &workspace, &entries),
            vec![Linked::Made]
        );
        let target = workspace.join("node_modules");
        let before = std::fs::read_link(&target).expect("shared link");

        assert_eq!(
            link_shared(&primary, &workspace, &entries),
            vec![Linked::Occupied]
        );
        assert_eq!(
            std::fs::read_link(&target).expect("shared link remains"),
            before,
            "the second share replaced an existing symlink"
        );
    }

    /// A repository can name a directory nobody has created yet, and a
    /// workspace can already hold one of its own. Neither is a failure, and
    /// neither is overwritten.
    #[test]
    fn a_missing_source_and_an_occupied_target_are_both_left_alone() {
        let (_root, primary, workspace) = sandbox();
        // Nothing installed yet.
        assert_eq!(
            link_shared(&primary, &workspace, &["node_modules".to_string()]),
            vec![Linked::SourceMissing]
        );

        // Something real already sits where the link would go.
        std::fs::create_dir_all(primary.join("vendor")).expect("vendor");
        std::fs::create_dir_all(workspace.join("vendor")).expect("their vendor");
        std::fs::write(workspace.join("vendor/theirs"), b"mine").expect("theirs");
        assert_eq!(
            link_shared(&primary, &workspace, &["vendor".to_string()]),
            vec![Linked::Occupied]
        );
        assert!(
            workspace.join("vendor/theirs").exists(),
            "somebody's own directory was replaced by a link"
        );
    }

    /// The unlink pass is what keeps ordinary deletion working — and it only
    /// ever removes links.
    #[test]
    fn only_the_links_are_taken_away_before_a_workspace_is_removed() {
        let (_root, primary, workspace) = sandbox();
        std::fs::create_dir_all(primary.join("node_modules")).expect("modules");
        std::fs::create_dir_all(primary.join("vendor")).expect("vendor");
        let entries = vec![
            "node_modules".to_string(),
            "vendor".to_string(),
            "never-linked".to_string(),
        ];
        link_shared(&primary, &workspace, &entries[..2]);

        // A real directory the person made, sharing a name with an entry.
        std::fs::create_dir_all(workspace.join("never-linked")).expect("theirs");
        std::fs::write(workspace.join("never-linked/work.txt"), b"hours").expect("work");

        assert_eq!(unlink_shared(&workspace, &entries), 2);
        assert!(workspace.join("node_modules").symlink_metadata().is_err());
        assert!(workspace.join("vendor").symlink_metadata().is_err());
        assert_eq!(
            std::fs::read_to_string(workspace.join("never-linked/work.txt")).expect("kept"),
            "hours",
            "a real directory was deleted because it shared a name with an entry"
        );

        // And the primary is untouched throughout — sharing is not moving.
        assert!(primary.join("node_modules").is_dir());
    }

    /// The last gate: nothing that climbs or starts at the root is acted on,
    /// even though the normaliser should already have dropped it.
    #[test]
    fn a_path_that_climbs_is_refused_at_the_filesystem_too() {
        let (_root, primary, workspace) = sandbox();
        for bad in ["../outside", "/etc/passwd", "", "a/../../b"] {
            assert!(
                ends(&primary, &workspace, bad).is_none(),
                "`{bad}` reached the filesystem"
            );
        }
        assert_eq!(
            link_shared(&primary, &workspace, &["../outside".to_string()]),
            vec![Linked::Failed("경로가 아닙니다".to_string())]
        );
        assert_eq!(unlink_shared(&workspace, &["../outside".to_string()]), 0);
    }
}
