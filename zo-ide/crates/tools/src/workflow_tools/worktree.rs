//! Per-agent worktree isolation for the workflow engine (roadmap step 8a).
//!
//! `isolation:"worktree"` runs each workflow agent in its own git worktree so
//! fan-out items cannot clobber one another's files. The engine asks a
//! [`WorktreeProvider`] for a fresh directory per agent and injects it as that
//! agent's `cwd` (see [`crate::context::ToolContext::with_cwd`]); the returned
//! [`WorktreeGuard`] removes the worktree when dropped (RAII), so cleanup
//! survives early returns and panics.
//!
//! The trait is injected (`Option<&dyn WorktreeProvider>` on
//! [`super::engine::RunOptions`]) so the engine's isolation logic is testable
//! without spawning real `git` — unit tests use a temp-dir mock.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const HARD_MIN_DISK_BYTES: u64 = 128 * 1024 * 1024;

fn worktree_disk_refusal(available: u64, parent: &Path) -> Option<String> {
    (available < HARD_MIN_DISK_BYTES).then(|| {
        format!(
            "refusing to run: only {}MB left on the filesystem holding {} — free disk space first (reclaim Rust target/ build dirs, temp scratch, or orphaned worktrees)",
            available / (1024 * 1024),
            parent.display(),
        )
    })
}

fn worktree_disk_preflight(parent: &Path) -> Result<Option<String>, String> {
    if let Some(error) = runtime::available_disk_bytes(parent)
        .and_then(|available| worktree_disk_refusal(available, parent))
    {
        return Err(error);
    }
    Ok(runtime::low_disk_warning(parent))
}

/// Source of isolated working directories for agents.
///
/// `&self` (state lives in the returned guard) so it threads as a shared
/// `Option<&dyn WorktreeProvider>` through the engine alongside `&RunOptions`,
/// with no `&mut` plumbing.
///
pub(crate) trait WorktreeProvider {
    /// Create a fresh isolated directory, `label` only feeding the dir name for
    /// diagnostics. `Err` means the caller should run this one agent without
    /// isolation (never panic) — the engine records an honest fallback note.
    fn create(&self, label: &str) -> Result<Box<dyn WorktreeGuard>, String>;

    /// Merge one collected patch (from [`WorktreeGuard::collect_patch`]) back
    /// into the main working tree with a 3-way apply. `Err` means it did not
    /// apply cleanly (conflict / unsupported / corrupt patch); the engine
    /// records that as an honest note rather than aborting the run. The default
    /// refuses, so a provider with no merge-back support never silently drops an
    /// agent's changes.
    fn apply_patch(&self, _patch: &str) -> Result<(), String> {
        Err("this worktree provider does not support merge-back".to_string())
    }
}

/// Handle to a live isolated directory; dropping it tears the worktree down.
///
/// `Send` so a still-live fan-out worker's guard can be moved to a background
/// cleanup owner and dropped only after the worker physically exits.
pub(crate) trait WorktreeGuard: Send {
    /// Absolute path of the isolated directory, injected as the agent's `cwd`.
    fn path(&self) -> &Path;

    /// Advisory captured immediately before creation, surfaced in the workflow
    /// report without changing whether isolation succeeds.
    fn creation_warning(&self) -> Option<&str> {
        None
    }

    /// Everything this agent changed in its worktree as a `git apply`-able
    /// patch, or `Ok(None)` when it left the tree clean. Called once after the
    /// batch barrier and before teardown (so an agent's edits survive the RAII
    /// drop only if they were collected first). The default reports a clean
    /// tree, so a non-git guard never fabricates a change-set.
    fn collect_patch(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// Production provider: real `git worktree add --detach` off the current repo
/// HEAD, under a scratch root in the OS temp dir (never inside the repo, so a
/// run leaves no tracked-tree pollution).
pub(crate) struct GitWorktreeProvider {
    root: PathBuf,
    /// Main work tree top level — where merge-back patches are applied so a
    /// `cwd` deeper in the tree never changes which tree receives the changes.
    repo_root: PathBuf,
    counter: AtomicUsize,
}

impl GitWorktreeProvider {
    /// Construct a provider, verifying we are inside a git work tree first.
    /// `Err` (not a repo / git missing) lets the caller fall back honestly
    /// instead of failing the whole workflow.
    pub(crate) fn new() -> Result<Self, String> {
        let probe = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .map_err(|e| format!("git not available: {e}"))?;
        if !probe.status.success() {
            return Err(format!(
                "not a git work tree: {}",
                String::from_utf8_lossy(&probe.stderr).trim()
            ));
        }
        // The main work tree top level, resolved once. Merge-back runs `git apply`
        // here regardless of the process cwd within the repo.
        let top = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|e| format!("git rev-parse failed: {e}"))?;
        if !top.status.success() {
            return Err(format!(
                "cannot resolve repo top level: {}",
                String::from_utf8_lossy(&top.stderr).trim()
            ));
        }
        let repo_root = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
        // Reap admin entries for worktrees whose dirs are already gone (e.g. an
        // OS-temp sweep, or a previous run's clean teardown) — best-effort, never
        // fatal. Pairs with the non-force [`GitWorktreeGuard::drop`], which leaves
        // a *dirty* worktree on disk rather than destroying unsaved agent work.
        let _ = Command::new("git").args(["worktree", "prune"]).output();
        let root = std::env::temp_dir().join("zo-workflow-worktrees");
        std::fs::create_dir_all(&root)
            .map_err(|e| format!("cannot create worktree scratch root: {e}"))?;
        Ok(Self {
            root,
            repo_root,
            counter: AtomicUsize::new(0),
        })
    }

    fn unique_path(&self, label: &str) -> PathBuf {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let slug: String = label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(40)
            .collect();
        self.root
            .join(format!("{}-{}-{}", std::process::id(), n, slug))
    }
}

impl WorktreeProvider for GitWorktreeProvider {
    fn create(&self, label: &str) -> Result<Box<dyn WorktreeGuard>, String> {
        let path = self.unique_path(label);
        let warning = worktree_disk_preflight(&self.root)?;
        // `--detach` checks out HEAD without claiming a branch, so parallel
        // worktrees never collide on a branch name.
        let output = Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .output()
            .map_err(|e| format!("git worktree add failed to start: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        // Pin the base commit now so `collect_patch` diffs against the exact
        // tree the agent started from, even if the agent commits (moving HEAD).
        let base = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map_or_else(
                || "HEAD".to_string(),
                |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
            );
        Ok(Box::new(GitWorktreeGuard {
            path,
            base,
            warning,
        }))
    }

    fn apply_patch(&self, patch: &str) -> Result<(), String> {
        // 3-way apply so a patch built off the agent's base still merges into a
        // main tree that has moved on; conflicts are reported (non-zero exit),
        // not silently dropped. `--whitespace=nowarn` keeps stderr signal clean.
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["apply", "--3way", "--whitespace=nowarn"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("git apply failed to start: {e}"))?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| "git apply stdin unavailable".to_string())?;
            stdin
                .write_all(patch.as_bytes())
                .map_err(|e| format!("writing patch to git apply failed: {e}"))?;
        } // stdin dropped here → pipe closed so git apply can finish reading.
        let out = child
            .wait_with_output()
            .map_err(|e| format!("git apply wait failed: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git apply --3way did not apply cleanly: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// RAII guard: removes the worktree on drop, best-effort (a failed teardown is
/// a leaked scratch dir, never a workflow failure).
struct GitWorktreeGuard {
    path: PathBuf,
    /// Commit the worktree was checked out at — the diff base for merge-back.
    base: String,
    warning: Option<String>,
}

impl WorktreeGuard for GitWorktreeGuard {
    fn path(&self) -> &Path {
        &self.path
    }

    fn creation_warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    fn collect_patch(&self) -> Result<Option<String>, String> {
        // Stage all changes (modified + new + deleted) so untracked files appear
        // in the diff; best-effort — a failure here just yields a tracked-only
        // patch rather than aborting merge-back.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["add", "-A"])
            .output();
        // Full base→staged patch. No `--binary`: a binary change would otherwise
        // produce bytes that lossy UTF-8 decoding corrupts; without it, a binary
        // file shows as "Binary files differ" and `git apply` reports an honest
        // failure (recorded as a note) instead of silently mangling content.
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["diff", "--cached", &self.base])
            .output()
            .map_err(|e| format!("git diff failed to start: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        if out.stdout.is_empty() {
            Ok(None)
        } else {
            Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
        }
    }
}

impl Drop for GitWorktreeGuard {
    fn drop(&mut self) {
        // Deliberately **no** `--force`: non-force `git worktree remove` cleanly
        // removes an idle, clean worktree and *refuses* one with modified/untracked
        // files (exit 128). That protects the real hazard — a straggler that
        // overran the phase timeout while actively writing leaves a dirty tree, so
        // forcing here would `rm -rf` its uncommitted work (the P1-1 data-loss
        // case). NOTE: git has no process/open-handle "in use" guard — a *clean*
        // but still-live straggler (read-only agent, pre-first-write, or
        // post-commit) is still removed, but by definition it has no unsaved tree
        // edits to lose. A refused removal just leaks a scratch dir under the OS
        // temp root, which the next provider's `git worktree prune` reaps once the
        // dir is gone.
        let _ = Command::new("git")
            .args(["worktree", "remove"])
            .arg(&self.path)
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_disk_refusal_matches_shared_bash_wording() {
        let parent = Path::new("/tmp/workflow-worktree-parent");
        assert_eq!(
            worktree_disk_refusal(12 * 1024 * 1024, parent).as_deref(),
            Some(
                "refusing to run: only 12MB left on the filesystem holding /tmp/workflow-worktree-parent — free disk space first (reclaim Rust target/ build dirs, temp scratch, or orphaned worktrees)"
            )
        );
    }
}
