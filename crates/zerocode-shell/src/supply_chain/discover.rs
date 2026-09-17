//! Where a workspace's lockfiles are.

use std::path::Path;

use ignore::WalkBuilder;
use zerocode_core::supply_chain::Ecosystem;

/// How deep below the workspace root a lockfile is looked for, the root's own
/// entries being depth 1 (§5.1). Monorepos keep a package's lockfile two
/// directories down (`packages/web/package-lock.json`, depth 3); a lockfile
/// deeper than that belongs to a fixture or a vendored copy — this
/// repository's own `fixtures/quality-baseline/repo/Cargo.lock` sits at 4.
pub(crate) const LOCKFILE_SEARCH_DEPTH: usize = 3;

/// Directories never walked: installed npm packages (their lockfiles are
/// theirs, not the workspace's), Cargo's build output, and git's own store.
pub(crate) const LOCKFILE_SKIPPED_DIRECTORIES: [&str; 3] = ["node_modules", "target", ".git"];

/// One lockfile the walk found.
#[derive(Debug)]
pub(crate) struct FoundLockfile {
    /// Workspace-relative, `/`-separated.
    pub(crate) relative: String,
    pub(crate) ecosystem: Ecosystem,
    /// Its text, or why it could not be read.
    pub(crate) text: Result<String, String>,
}

/// Every lockfile this layer reads under `root`, sorted by path. Links are
/// not followed: a workspace's lockfiles are the files in it.
pub(crate) fn lockfiles(root: &Path) -> Vec<FoundLockfile> {
    let mut walk = WalkBuilder::new(root);
    walk.standard_filters(false)
        .follow_links(false)
        .max_depth(Some(LOCKFILE_SEARCH_DEPTH))
        .filter_entry(|entry| {
            let directory = entry.file_type().is_some_and(|kind| kind.is_dir());
            !(directory
                && entry.depth() > 0
                && LOCKFILE_SKIPPED_DIRECTORIES
                    .iter()
                    .any(|skipped| entry.file_name() == *skipped))
        });
    let mut found: Vec<FoundLockfile> = walk
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let ecosystem = Ecosystem::of_lockfile(entry.file_name().to_str()?)?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(entry.path()).map_err(|error| error.to_string());
            Some(FoundLockfile {
                relative,
                ecosystem,
                text,
            })
        })
        .collect();
    found.sort_by(|left, right| left.relative.cmp(&right.relative));
    found
}
