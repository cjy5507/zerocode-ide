//! Git worktree management tools: `EnterWorktree` and `ExitWorktree`.
//!
//! `EnterWorktree` verifies or creates a git worktree at the given path,
//! then switches the process working directory so all subsequent tool
//! calls operate inside it.  `ExitWorktree` restores the original cwd
//! and removes the worktree only when `EnterWorktree` created it — and
//! then without `--force`, so git refuses a tree with uncommitted work.
//! A directory that already existed (the person's own worktree or
//! checkout) is never removed.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use api::sync_bridge::lock_recovered;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{to_pretty_json, ToolError, ToolSpec};
use runtime::PermissionMode;

/// One `EnterWorktree` still open: the cwd it left, and the worktree it
/// created there — `None` when the directory already existed, which
/// `ExitWorktree` then never removes.
#[derive(Debug, Clone)]
struct SavedCwd {
    prev: PathBuf,
    created: Option<PathBuf>,
}

/// Stack of saved working directories. The last entry is the cwd
/// before the most recent `EnterWorktree` call.
///
/// Poison policy: recover (`lock_recovered`) — push/pop of complete
/// entries keeps the stack consistent at every panic point.
static SAVED_CWD: Mutex<Vec<SavedCwd>> = Mutex::new(Vec::new());

/// The directories saved by every `EnterWorktree` still on the stack, oldest
/// first. Read by the session agent registry when it adopts legacy stores —
/// each entry is a cwd a pre-registry session may have spawned children from.
pub(crate) fn saved_cwd_stack() -> Vec<PathBuf> {
    lock_recovered(&SAVED_CWD).iter().map(|saved| saved.prev.clone()).collect()
}

/// Run `f` with `path` pushed on the `EnterWorktree` stack, as if the session
/// had entered a worktree from there, then pop it again.
#[cfg(test)]
pub(crate) fn with_saved_cwd_for_tests<T>(path: &std::path::Path, f: impl FnOnce() -> T) -> T {
    lock_recovered(&SAVED_CWD).push(SavedCwd { prev: path.to_path_buf(), created: None });
    let out = f();
    let mut stack = lock_recovered(&SAVED_CWD);
    if let Some(at) = stack.iter().rposition(|saved| saved.prev == path) {
        stack.remove(at);
    }
    out
}

#[derive(Debug, Deserialize)]
pub(crate) struct EnterWorktreeInput {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExitWorktreeInput {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorktreeOutput {
    status: &'static str,
    message: String,
}

#[must_use]
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "EnterWorktree",
            description: "Enter (activate) a git worktree for subsequent tool calls.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "ExitWorktree",
            description: "Exit a previously entered git worktree and return to the directory you entered it from. A worktree EnterWorktree created is removed when it holds no uncommitted work; a directory that already existed is left in place.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
    ]
}

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

/// What [`ensure_worktree`] found or made at a path.
struct EnsuredWorktree {
    /// Whether `git worktree add` made it now; a directory that was already
    /// there is somebody's, and leaving it never removes it.
    created: bool,
    warning: Option<String>,
}

/// Ensure a git worktree exists at `path`. If the path is already a
/// directory (worktree, checkout, or plain dir) it is accepted as-is.
/// Otherwise a new git worktree is created.
fn ensure_worktree(path: &Path, branch: Option<&str>) -> Result<EnsuredWorktree, String> {
    if path.is_dir() {
        // Already exists — accept it (could be a worktree, checkout, or
        // a plain directory the caller wants to work in).
        return Ok(EnsuredWorktree { created: false, warning: None });
    }

    let parent = path
        .parent()
        .ok_or_else(|| "worktree destination has no parent directory".to_string())?;
    let warning = worktree_disk_preflight(parent)?;

    let mut cmd = Command::new("git");
    cmd.arg("worktree").arg("add");
    if let Some(b) = branch {
        cmd.arg("-b").arg(b);
    }
    cmd.arg(path);

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run git worktree add: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {stderr}"));
    }
    Ok(EnsuredWorktree { created: true, warning })
}

/// Prepare a CLI startup worktree through the same disk check and git creation path.
pub fn prepare_start_worktree(path: &Path) -> Result<std::path::PathBuf, String> {
    if let Some(warning) = ensure_worktree(path, None)?.warning { eprintln!("{warning}"); }
    let output = Command::new("git").arg("-C").arg(path).args(["rev-parse", "--show-toplevel"]).output().map_err(|e| e.to_string())?;
    if !output.status.success() { return Err("--worktree requires a git working tree".into()); }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if root.canonicalize().map_err(|e| e.to_string())? != canonical { return Err("--worktree must name the root of a git working tree".into()); }
    Ok(canonical)
}

pub(crate) fn run_enter_worktree(input: &EnterWorktreeInput) -> Result<String, ToolError> {
    let target = PathBuf::from(&input.path);
    let target = if target.is_absolute() {
        target
    } else {
        env::current_dir()
            .map_err(|e| ToolError::InvalidInput(format!("cannot resolve cwd: {e}")))?
            .join(&target)
    };

    // Ensure the worktree exists (creates if necessary).
    let EnsuredWorktree { created, warning } = ensure_worktree(&target, input.branch.as_deref())
        .map_err(|e| ToolError::Execution(format!("worktree setup failed: {e}")))?;

    // Save current cwd before switching.
    let prev =
        env::current_dir().map_err(|e| ToolError::Execution(format!("cannot read cwd: {e}")))?;

    env::set_current_dir(&target)
        .map_err(|e| ToolError::Execution(format!("cannot chdir to {}: {e}", target.display())))?;

    lock_recovered(&SAVED_CWD).push(SavedCwd { prev, created: created.then_some(target.clone()) });

    to_pretty_json(WorktreeOutput {
        status: "ok",
        message: format!(
            "Entered worktree at {}{}{}",
            target.display(),
            input
                .branch
                .as_deref()
                .map_or(String::new(), |b| format!(" (branch: {b})")),
            warning.map_or(String::new(), |warning| format!("\n{warning}")),
        ),
    })
}

pub(crate) fn run_exit_worktree(input: &ExitWorktreeInput) -> Result<String, ToolError> {
    let saved = lock_recovered(&SAVED_CWD).pop();

    let Some(SavedCwd { prev: restore_to, created }) = saved else {
        return to_pretty_json(WorktreeOutput {
            status: "noop",
            message: "Not inside a worktree entered via EnterWorktree.".to_string(),
        });
    };

    env::set_current_dir(&restore_to).map_err(|e| {
        ToolError::Execution(format!(
            "cannot restore cwd to {}: {e}",
            restore_to.display()
        ))
    })?;

    let removal = match created {
        // A `path` names the worktree to leave; one that is not the worktree
        // this EnterWorktree created is never removed.
        Some(created) if input.path.as_deref().is_none_or(|named| same_dir(Path::new(named), &created)) => {
            remove_created_worktree(&restore_to, &created)
        }
        Some(created) => format!(
            "; left {} in place: {} is not the worktree EnterWorktree created",
            created.display(),
            input.path.as_deref().unwrap_or_default(),
        ),
        None => "; the directory already existed before EnterWorktree, so it was left in place".to_string(),
    };

    to_pretty_json(WorktreeOutput {
        status: "ok",
        message: format!("Restored cwd to {}{removal}", restore_to.display()),
    })
}

/// Whether two paths name one directory: by their resolved form when both
/// resolve, else as written.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `git worktree remove` without `--force`, run from the repository the
/// worktree was added from: git removes a clean worktree and refuses one
/// holding modified or untracked files, which then stays for the person.
/// Returns the tail of the exit message. A refusal is reported by its reason,
/// not by git's own text, which points at `--force`.
fn remove_created_worktree(repo: &Path, worktree: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "remove"])
        .arg(worktree)
        .output();
    match output {
        Ok(out) if out.status.success() => format!("; removed worktree {}", worktree.display()),
        Ok(_) => format!(
            "; kept worktree {}: git did not find it clean (uncommitted, untracked or locked work), so it stays as it is for the person",
            worktree.display(),
        ),
        Err(error) => format!("; kept worktree {}: git did not run: {error}", worktree.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_disk_refusal_matches_shared_bash_wording() {
        let parent = Path::new("/tmp/worktree-parent");
        assert_eq!(
            worktree_disk_refusal(12 * 1024 * 1024, parent).as_deref(),
            Some(
                "refusing to run: only 12MB left on the filesystem holding /tmp/worktree-parent — free disk space first (reclaim Rust target/ build dirs, temp scratch, or orphaned worktrees)"
            )
        );
    }

    #[test]
    fn enter_worktree_spec_present() {
        let specs = tool_specs();
        assert!(specs.iter().any(|s| s.name == "EnterWorktree"));
        assert!(specs.iter().any(|s| s.name == "ExitWorktree"));
    }

    #[test]
    fn exit_worktree_noop_without_enter() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Drain any leftovers from previous tests.
        SAVED_CWD.lock().expect("cwd lock").clear();

        let out = run_exit_worktree(&ExitWorktreeInput { path: None }).expect("should return noop");
        assert!(out.contains("noop"));
    }

    #[test]
    fn enter_and_exit_roundtrip() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Drain any leftovers from previous tests.
        SAVED_CWD.lock().expect("cwd lock").clear();

        let original = env::current_dir().expect("cwd should exist");
        let tmp = std::env::temp_dir();

        // Enter the temp dir as a "worktree" (it already exists).
        let out = run_enter_worktree(&EnterWorktreeInput {
            path: tmp.to_string_lossy().to_string(),
            branch: None,
        })
        .expect("should enter");
        assert!(out.contains("ok"));
        assert_eq!(
            env::current_dir().expect("cwd").canonicalize().ok(),
            tmp.canonicalize().ok()
        );

        // Exit back. The tmp dir existed before EnterWorktree, so exit only
        // restores the cwd and leaves it in place.
        let out = run_exit_worktree(&ExitWorktreeInput {
            path: Some(tmp.to_string_lossy().to_string()),
        })
        .expect("should exit");
        assert!(out.contains("ok"));
        assert_eq!(
            env::current_dir().expect("cwd").canonicalize().ok(),
            original.canonicalize().ok()
        );
    }

    /// A scratch repository with one commit, and the process cwd put back
    /// when the test ends — these tests move it, and the next test must not
    /// start inside a deleted directory.
    struct ScratchRepo {
        dir: tempfile::TempDir,
        original_cwd: PathBuf,
    }

    impl ScratchRepo {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let original_cwd = env::current_dir().expect("cwd");
            let repo = Self { dir, original_cwd };
            Self::git(&["init", "-q", "-b", "main", "repo"], &repo.root());
            Self::git(
                &["-c", "user.name=fixture", "-c", "user.email=fixture@example.com", "-c", "commit.gpgsign=false",
                  "commit", "-q", "--allow-empty", "-m", "init"],
                &repo.repo(),
            );
            repo
        }

        fn root(&self) -> PathBuf {
            self.dir.path().canonicalize().expect("canonical tempdir")
        }

        fn repo(&self) -> PathBuf {
            self.root().join("repo")
        }

        fn git(args: &[&str], at: &Path) {
            let out = Command::new("git").arg("-C").arg(at).args(args).output().expect("git runs");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }

        /// A worktree the person made before the session entered it, with
        /// one file nobody committed.
        fn persons_worktree_with_unsaved_work(&self, name: &str) -> PathBuf {
            let path = self.root().join(name);
            Self::git(&["worktree", "add", "-q", "-b", name, path.to_str().expect("utf8")], &self.repo());
            std::fs::write(path.join("unsaved.txt"), "work in progress\n").expect("write unsaved");
            path
        }
    }

    /// The env lock, with every git these tests start — theirs and the
    /// tool's — kept off the person's global and system git settings (hooks
    /// among them).
    fn git_without_the_persons_config() -> crate::tests::EnvGuard {
        let no_file = if cfg!(windows) { "NUL" } else { "/dev/null" };
        crate::tests::EnvGuard::set("GIT_CONFIG_GLOBAL", no_file).set_also("GIT_CONFIG_NOSYSTEM", "1")
    }

    impl Drop for ScratchRepo {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.original_cwd);
        }
    }

    fn enter(path: &Path) -> String {
        run_enter_worktree(&EnterWorktreeInput { path: path.to_string_lossy().to_string(), branch: None })
            .expect("should enter")
    }

    fn exit(path: Option<&Path>) -> String {
        run_exit_worktree(&ExitWorktreeInput { path: path.map(|path| path.to_string_lossy().to_string()) })
            .expect("should exit")
    }

    #[test]
    fn exit_leaves_a_worktree_that_existed_before_enter() {
        let _guard = git_without_the_persons_config();
        SAVED_CWD.lock().expect("cwd lock").clear();
        let scratch = ScratchRepo::new();
        let persons = scratch.persons_worktree_with_unsaved_work("persons");
        env::set_current_dir(scratch.repo()).expect("cd repo");

        enter(&persons);
        let out = exit(None);

        assert!(persons.join("unsaved.txt").is_file(), "the person's uncommitted file survives: {out}");
    }

    #[test]
    fn exit_leaves_a_clean_worktree_that_existed_before_enter() {
        let _guard = git_without_the_persons_config();
        SAVED_CWD.lock().expect("cwd lock").clear();
        let scratch = ScratchRepo::new();
        let persons = scratch.root().join("persons");
        ScratchRepo::git(&["worktree", "add", "-q", "-b", "persons", persons.to_str().expect("utf8")], &scratch.repo());
        env::set_current_dir(scratch.repo()).expect("cd repo");

        enter(&persons);
        let out = exit(None);

        assert!(persons.is_dir(), "only a worktree EnterWorktree made is removed, clean or not: {out}");
    }

    #[test]
    fn exit_keeps_a_worktree_it_created_while_it_holds_uncommitted_work() {
        let _guard = git_without_the_persons_config();
        SAVED_CWD.lock().expect("cwd lock").clear();
        let scratch = ScratchRepo::new();
        env::set_current_dir(scratch.repo()).expect("cd repo");
        let created = scratch.root().join("created");

        enter(&created);
        std::fs::write(created.join("unsaved.txt"), "work in progress\n").expect("write unsaved");
        let out = exit(None);

        assert!(created.join("unsaved.txt").is_file(), "uncommitted work is never forced away: {out}");
        assert!(out.contains("kept worktree"), "{out}");
        assert!(!out.contains("--force"), "the result never points the model at a forced removal: {out}");
    }

    #[test]
    fn exit_removes_a_clean_worktree_it_created() {
        let _guard = git_without_the_persons_config();
        SAVED_CWD.lock().expect("cwd lock").clear();
        let scratch = ScratchRepo::new();
        env::set_current_dir(scratch.repo()).expect("cd repo");
        let created = scratch.root().join("created");

        enter(&created);
        let out = exit(None);

        assert!(!created.exists(), "a clean worktree EnterWorktree made is cleaned up: {out}");
        assert_eq!(env::current_dir().expect("cwd").canonicalize().ok(), Some(scratch.repo()));
    }

    #[test]
    fn exit_never_removes_a_named_worktree_it_did_not_create() {
        let _guard = git_without_the_persons_config();
        SAVED_CWD.lock().expect("cwd lock").clear();
        let scratch = ScratchRepo::new();
        let persons = scratch.persons_worktree_with_unsaved_work("persons");
        env::set_current_dir(scratch.repo()).expect("cd repo");
        let created = scratch.root().join("created");

        enter(&created);
        let out = exit(Some(&persons));

        assert!(persons.join("unsaved.txt").is_file(), "a worktree it never entered is not its to remove: {out}");
    }
}
