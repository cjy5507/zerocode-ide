//! Where zo keeps one project's own state: the name it gives a workspace's
//! folder under its projects root (moved here from zo's runtime by t-11349),
//! so the window finds — and writes — a project's zo ledgers where zo itself
//! keeps them, from the same folder by the same name.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// A short, stable, filesystem-safe key derived from the workspace path so
/// concurrent runs in different repos never share scratch state.
#[must_use]
pub fn workspace_scratch_key(cwd: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The folder name zo gives `cwd`'s state: the path's last eighty characters
/// with every one outside `[A-Za-z0-9._-]` made a `-` and the ends trimmed of
/// them, then [`workspace_scratch_key`].
#[must_use]
pub fn project_slug(cwd: &Path) -> String {
    let sanitized: String = cwd
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let tail_start = sanitized.len().saturating_sub(80);
    let stem = sanitized[tail_start..].trim_matches('-');
    let hash = workspace_scratch_key(cwd);
    if stem.is_empty() {
        hash
    } else {
        format!("{stem}-{hash}")
    }
}
