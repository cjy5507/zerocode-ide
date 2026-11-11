//! Where zo keeps one project's own state: the name it gives a workspace's
//! folder under its projects root (moved here from zo's runtime by t-11349),
//! so the window finds — and writes — a project's zo ledgers where zo itself
//! keeps them, from the same folder by the same name.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

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

/// How the workspace record beside a zo transcript is named: zo writes
/// `<session>.cwd` next to `<session>.jsonl` when it creates the session
/// (`runtime::session_control::SESSION_CWD_EXTENSION` in zo-ide, held to it
/// by the window's `jev_scope` test; spelled once more here because this
/// crate cannot depend on zo).
pub const SESSION_CWD_EXTENSION: &str = "cwd";

/// The workspace one `.cwd` sidecar names (`{"cwd": …}`), when it names one.
///
/// The one reader of that file: the transcript itself says no folder, so the
/// session vault's card, the seat dashboard and a reopened pane all learn
/// where a zo conversation ran from here.
#[must_use]
pub fn recorded_workspace(sidecar: &Path) -> Option<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Recorded {
        cwd: String,
    }
    let recorded: Recorded = serde_json::from_str(&std::fs::read_to_string(sidecar).ok()?).ok()?;
    let cwd = recorded.cwd.trim();
    (!cwd.is_empty()).then(|| PathBuf::from(cwd))
}

/// The machine's temporary roots, as spelled and resolved — zo's scoreboard's
/// own (`temp_roots`): the system temporary directory and `/tmp`. A folder
/// under one is a bench round's `mkdtemp`, a test's `tempfile`, a scratchpad:
/// not one a person keeps work in. The seat dashboard leaves such projects
/// out and the session vault keeps such conversations apart, by this list.
#[must_use]
pub fn temporary_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        if let Ok(resolved) = root.canonicalize() {
            roots.push(resolved);
        }
        roots.push(root);
    }
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_names_the_workspace_it_recorded() {
        let dir = tempfile::tempdir().expect("temp");
        let sidecar = dir.path().join("session-1.cwd");
        std::fs::write(&sidecar, r#"{"cwd":"/Users/dev/acme"}"#).expect("write");
        assert_eq!(
            recorded_workspace(&sidecar),
            Some(PathBuf::from("/Users/dev/acme"))
        );
    }

    #[test]
    fn a_sidecar_that_names_nothing_is_no_workspace() {
        let dir = tempfile::tempdir().expect("temp");
        let blank = dir.path().join("blank.cwd");
        std::fs::write(&blank, r#"{"cwd":"  "}"#).expect("write");
        let torn = dir.path().join("torn.cwd");
        std::fs::write(&torn, "{").expect("write");
        assert_eq!(
            [
                recorded_workspace(&blank),
                recorded_workspace(&torn),
                recorded_workspace(&dir.path().join("absent.cwd")),
            ],
            [None, None, None]
        );
    }
}
