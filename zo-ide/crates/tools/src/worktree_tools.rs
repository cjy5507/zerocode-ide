//! Git worktree management tools: `EnterWorktree` and `ExitWorktree`.
//!
//! `EnterWorktree` verifies or creates a git worktree at the given path,
//! then switches the process working directory so all subsequent tool
//! calls operate inside it.  `ExitWorktree` restores the original cwd
//! and optionally removes the worktree.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use api::sync_bridge::lock_recovered;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{to_pretty_json, ToolError, ToolSpec};
use runtime::PermissionMode;

/// Stack of saved working directories. The last entry is the cwd
/// before the most recent `EnterWorktree` call.
///
/// Poison policy: recover (`lock_recovered`) — push/pop of complete
/// `PathBuf` values keeps the stack consistent at every panic point.
static SAVED_CWD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The directories saved by every `EnterWorktree` still on the stack, oldest
/// first. Read by the session agent registry when it adopts legacy stores —
/// each entry is a cwd a pre-registry session may have spawned children from.
pub(crate) fn saved_cwd_stack() -> Vec<PathBuf> {
    lock_recovered(&SAVED_CWD).clone()
}

/// Run `f` with `path` pushed on the `EnterWorktree` stack, as if the session
/// had entered a worktree from there, then pop it again.
#[cfg(test)]
pub(crate) fn with_saved_cwd_for_tests<T>(path: &std::path::Path, f: impl FnOnce() -> T) -> T {
    lock_recovered(&SAVED_CWD).push(path.to_path_buf());
    let out = f();
    let mut stack = lock_recovered(&SAVED_CWD);
    if let Some(at) = stack.iter().rposition(|saved| saved == path) {
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
            description: "Exit a previously entered git worktree.",
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

/// Ensure a git worktree exists at `path`. If the path is already a
/// directory (worktree, checkout, or plain dir) it is accepted as-is.
/// Otherwise a new git worktree is created.
fn ensure_worktree(path: &Path, branch: Option<&str>) -> Result<Option<String>, String> {
    if path.is_dir() {
        // Already exists — accept it (could be a worktree, checkout, or
        // a plain directory the caller wants to work in).
        return Ok(None);
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
    Ok(warning)
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
    let warning = ensure_worktree(&target, input.branch.as_deref())
        .map_err(|e| ToolError::Execution(format!("worktree setup failed: {e}")))?;

    // Save current cwd before switching.
    let prev =
        env::current_dir().map_err(|e| ToolError::Execution(format!("cannot read cwd: {e}")))?;

    env::set_current_dir(&target)
        .map_err(|e| ToolError::Execution(format!("cannot chdir to {}: {e}", target.display())))?;

    lock_recovered(&SAVED_CWD).push(prev);

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

pub(crate) fn run_exit_worktree(input: ExitWorktreeInput) -> Result<String, ToolError> {
    let prev = lock_recovered(&SAVED_CWD).pop();

    let Some(restore_to) = prev else {
        return to_pretty_json(WorktreeOutput {
            status: "noop",
            message: "Not inside a worktree entered via EnterWorktree.".to_string(),
        });
    };

    // Optionally remove the worktree we're leaving.
    let worktree_path = input
        .path
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok());

    env::set_current_dir(&restore_to).map_err(|e| {
        ToolError::Execution(format!(
            "cannot restore cwd to {}: {e}",
            restore_to.display()
        ))
    })?;

    // Best-effort cleanup: `git worktree remove` the path we just left.
    if let Some(wt) = worktree_path {
        let _ = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&wt)
            .output();
    }

    to_pretty_json(WorktreeOutput {
        status: "ok",
        message: format!("Restored cwd to {}", restore_to.display()),
    })
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

        let out = run_exit_worktree(ExitWorktreeInput { path: None }).expect("should return noop");
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

        // Exit back — pass the worktree path explicitly so `git worktree
        // remove` targets the tmp dir instead of whatever cwd happens to be.
        let out = run_exit_worktree(ExitWorktreeInput {
            path: Some(tmp.to_string_lossy().to_string()),
        })
        .expect("should exit");
        assert!(out.contains("ok"));
        assert_eq!(
            env::current_dir().expect("cwd").canonicalize().ok(),
            original.canonicalize().ok()
        );
    }
}
