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
    current_workspace: bool,
}

impl GitWorktreeProvider {
    /// Construct a provider, verifying we are inside a git work tree first.
    /// `Err` (not a repo / git missing) lets the caller fall back honestly
    /// instead of failing the whole workflow.
    pub(crate) fn new() -> Result<Self, String> {
        let probe = Command::new("git")
            .args(["--no-optional-locks", "rev-parse", "--is-inside-work-tree"])
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
            .args(["--no-optional-locks", "rev-parse", "--show-toplevel"])
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
        let _ = git_at(&repo_root).args(["worktree", "prune"]).output();
        let root = std::env::temp_dir().join("zo-workflow-worktrees");
        std::fs::create_dir_all(&root)
            .map_err(|e| format!("cannot create worktree scratch root: {e}"))?;
        Ok(Self {
            root,
            repo_root,
            counter: AtomicUsize::new(0),
            current_workspace: false,
        })
    }

    /// Graph visits start from the preceding visit's applied working tree,
    /// including unstaged and untracked files, without moving the real index.
    #[must_use]
    pub(crate) fn with_current_workspace(mut self) -> Self {
        self.current_workspace = true;
        self
    }

    fn working_index(&self) -> Result<ScratchIndex, String> {
        let index = ScratchIndex::new(&self.root)?;
        index.run(&self.repo_root, &["read-tree", "HEAD"])?;
        index.run(&self.repo_root, &["add", "-A"])?;
        Ok(index)
    }

    fn snapshot_commit(&self) -> Result<String, String> {
        let index = self.working_index()?;
        let tree = index.run(&self.repo_root, &["write-tree"])?;
        let parent = index.run(&self.repo_root, &["rev-parse", "HEAD"])?;
        let mut command = index.command(&self.repo_root);
        command.args(["-c", "commit.gpgSign=false", "commit-tree", &tree, "-p", &parent,
            "-m", "Workflow workspace snapshot"])
            .env("GIT_AUTHOR_NAME", "Workflow")
            .env("GIT_AUTHOR_EMAIL", "workflow@example.invalid")
            .env("GIT_COMMITTER_NAME", "Workflow")
            .env("GIT_COMMITTER_EMAIL", "workflow@example.invalid");
        git_output(command)
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

/// A fresh alternate index; its private directory is never reused or followed.
struct ScratchIndex(PathBuf);

impl ScratchIndex {
    fn new(root: &Path) -> Result<Self, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        for _ in 0..128 {
            let path = root.join(format!("index-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = std::fs::DirBuilder::new();
                builder.mode(0o700);
                builder
            };
            #[cfg(not(unix))]
            let builder = std::fs::DirBuilder::new();
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("cannot create workflow snapshot index: {error}")),
            }
        }
        Err("cannot reserve a fresh workflow snapshot index".to_string())
    }

    fn command(&self, repo: &Path) -> Command {
        let mut command = git_at(repo);
        command.env("GIT_INDEX_FILE", self.0.join("index"));
        command
    }

    fn run(&self, repo: &Path, args: &[&str]) -> Result<String, String> {
        let mut command = self.command(repo);
        command.args(args);
        git_output(command)
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git_at(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(repo).arg("--no-optional-locks");
    command
}

fn git_output(mut command: Command) -> Result<String, String> {
    let output = command.output().map_err(|error| format!("workflow git failed to start: {error}"))?;
    if !output.status.success() {
        return Err(format!("workflow git failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    String::from_utf8(output.stdout).map(|text| text.trim().to_string())
        .map_err(|_| "workflow git returned invalid UTF-8".to_string())
}

impl WorktreeProvider for GitWorktreeProvider {
    fn create(&self, label: &str) -> Result<Box<dyn WorktreeGuard>, String> {
        let path = self.unique_path(label);
        let warning = worktree_disk_preflight(&self.root)?;
        let snapshot = self.current_workspace.then(|| self.snapshot_commit()).transpose()?;
        // `--detach` checks out HEAD without claiming a branch, so parallel
        // worktrees never collide on a branch name.
        let mut command = git_at(&self.repo_root);
        command.args(["worktree", "add", "--detach"]).arg(&path);
        if let Some(snapshot) = &snapshot {
            command.arg(snapshot);
        }
        let output = command
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
        let base = snapshot.unwrap_or_else(|| git_at(&path)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map_or_else(
                || "HEAD".to_string(),
                |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ));
        Ok(Box::new(GitWorktreeGuard {
            path,
            base,
            warning,
            repo_root: self.repo_root.clone(),
            graph_snapshot: self.current_workspace,
        }))
    }

    fn apply_patch(&self, patch: &str) -> Result<(), String> {
        // 3-way apply so a patch built off the agent's base still merges into a
        // main tree that has moved on; conflicts are reported (non-zero exit),
        // not silently dropped. `--whitespace=nowarn` keeps stderr signal clean.
        // `--3way` implies --index. Graph snapshots include the dirty working
        // tree, so give merge-back the same private-index view rather than
        // requiring or staging over the person's real index.
        let index = self.current_workspace.then(|| self.working_index()).transpose()?;
        let mut command = index.as_ref().map_or_else(|| git_at(&self.repo_root), |index| index.command(&self.repo_root));
        let mut child = command.args(["apply", "--3way", "--whitespace=nowarn"])
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
    repo_root: PathBuf,
    graph_snapshot: bool,
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
        let staged = git_at(&self.path)
            .args(["add", "-A"])
            .output();
        if self.graph_snapshot && !staged.as_ref().is_ok_and(|output| output.status.success()) {
            return Err("could not stage the graph worker changes for merge-back".to_string());
        }
        // The legacy path keeps its old patch format. Graph visits include
        // ASCII-encoded binary patches and reject non-UTF-8 textual diffs.
        let mut command = git_at(&self.path);
        command.args(["-c", "diff.autoRefreshIndex=false", "diff", "--cached", "--no-ext-diff", "--no-textconv"]);
        if self.graph_snapshot {
            // Git's binary patches are ASCII encoded, not raw file bytes.
            command.args(["--binary", "--full-index"]);
        }
        let out = command.arg(&self.base)
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
        } else if self.graph_snapshot {
            String::from_utf8(out.stdout).map(Some)
                .map_err(|_| "graph patch contains non-UTF-8 text; merge-back refused".to_string())
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
        let _ = git_at(&self.repo_root)
            .args(["worktree", "remove"])
            .arg(&self.path)
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let mut command = git_at(repo);
        command.args(args);
        git_output(command).expect("fixture git command")
    }

    fn graph_provider(root: &Path) -> GitWorktreeProvider {
        let repo = root.join("repo");
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.name", "Workflow"]);
        git(&repo, &["config", "user.email", "workflow@example.invalid"]);
        git(&repo, &["config", "commit.gpgSign", "false"]);
        std::fs::write(repo.join("tracked.txt"), "baseline\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.tmp\n").unwrap();
        std::fs::write(repo.join("binary.bin"), [0, 255, 1]).unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "fixture"]);
        GitWorktreeProvider {
            root: scratch,
            repo_root: repo,
            counter: AtomicUsize::new(0),
            current_workspace: false,
        }.with_current_workspace()
    }

    #[test]
    fn graph_worktrees_inherit_dirty_parent_and_each_preceding_applied_visit() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().unwrap();
        let provider = graph_provider(root.path());
        let repo = &provider.repo_root;
        std::fs::write(repo.join("tracked.txt"), "staged version\n").unwrap();
        git(repo, &["add", "tracked.txt"]);
        std::fs::write(repo.join("tracked.txt"), "dirty version\n").unwrap();
        std::fs::write(repo.join("untracked.txt"), "new parent file\n").unwrap();
        std::fs::write(repo.join("ignored.tmp"), "do not copy\n").unwrap();
        let index_before = std::fs::read(repo.join(".git/index")).unwrap();
        let refs_before = git(repo, &["show-ref"]);
        let head_before = git(repo, &["rev-parse", "HEAD"]);

        let first = provider.create("first").unwrap();
        assert_eq!(std::fs::read_to_string(first.path().join("tracked.txt")).unwrap(), "dirty version\n");
        assert_eq!(std::fs::read_to_string(first.path().join("untracked.txt")).unwrap(), "new parent file\n");
        assert!(!first.path().join("ignored.tmp").exists());
        assert_eq!(first.collect_patch().unwrap(), None, "the parent's dirt is the baseline, not this node's patch");
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before);

        std::fs::write(first.path().join("tracked.txt"), "first node fix\n").unwrap();
        std::fs::write(first.path().join("untracked.txt"), "first node update\n").unwrap();
        std::fs::write(first.path().join("node-output.txt"), "first node output\n").unwrap();
        let patch = first.collect_patch().unwrap().expect("first node changes");
        provider.apply_patch(&patch).unwrap();
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before, "graph merge-back preserves staging");

        let second = provider.create("second").unwrap();
        assert_eq!(std::fs::read_to_string(second.path().join("tracked.txt")).unwrap(), "first node fix\n");
        assert_eq!(std::fs::read_to_string(second.path().join("untracked.txt")).unwrap(), "first node update\n");
        assert_eq!(std::fs::read_to_string(second.path().join("node-output.txt")).unwrap(), "first node output\n");
        assert_eq!(second.collect_patch().unwrap(), None);
        std::fs::write(second.path().join("tracked.txt"), "second node repair\n").unwrap();
        provider.apply_patch(&second.collect_patch().unwrap().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("tracked.txt")).unwrap(), "second node repair\n");
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before);
        assert_eq!(git(repo, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(git(repo, &["show-ref"]), refs_before);
        assert_eq!(git(repo, &["rev-list", "--count", "HEAD"]), "1");
        assert!(std::fs::read_dir(&provider.root).unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().starts_with("index-")
        }));
    }

    #[test]
    fn graph_binary_patch_preserves_bytes_and_the_parent_index() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().unwrap();
        let provider = graph_provider(root.path());
        let index = std::fs::read(provider.repo_root.join(".git/index")).unwrap();
        let worker = provider.create("binary").unwrap();
        let bytes = [0, 254, 2, 255, 0, 3];
        std::fs::write(worker.path().join("binary.bin"), bytes).unwrap();
        let patch = worker.collect_patch().unwrap().unwrap();
        assert!(patch.contains("GIT binary patch"));
        provider.apply_patch(&patch).unwrap();
        assert_eq!(std::fs::read(provider.repo_root.join("binary.bin")).unwrap(), bytes);
        assert_eq!(std::fs::read(provider.repo_root.join(".git/index")).unwrap(), index);
    }

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
