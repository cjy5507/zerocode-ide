//! Bash command validation — what this runtime's permission modes make of a
//! shell command.
//!
//! The rules themselves are the core crate's (`zerocode_core::shell_rule`,
//! t-10916): what a read-only session may run, what is destructive, which
//! paths escape, a command's intent. They moved there so the window reads a
//! command by the same rules zo does; what stays here is the half that speaks
//! of a [`PermissionMode`]:
//! - `readOnlyValidation` — in read-only mode, run only what the read-only
//!   list proves
//! - `modeValidation` — enforce permission mode constraints on commands
//! - the pipeline that runs the mode's rule and the every-mode checks
//!   ([`validate_command`])

use std::path::Path;

use crate::permissions::PermissionMode;

pub mod inspection;
pub use inspection::literal_argv;

pub use zerocode_core::shell_rule::{
    check_destructive, classify_command, git_worktree_escape_reason, path_within_root,
    reaches_outside_workspace, split_command_segments, validate_paths, CommandIntent,
    ValidationResult,
};
use zerocode_core::shell_rule::parse::strip_command_wrappers;
#[cfg(test)]
use zerocode_core::shell_rule::parse::extract_first_command;
use zerocode_core::shell_rule::{
    command_targets_outside_workspace, proven_read_only, read_only,
    FIND_MUTATING_PRIMARIES, MAX_ANALYZED_COMMAND_LEN,
};

// ---------------------------------------------------------------------------
// readOnlyValidation
// ---------------------------------------------------------------------------

/// Validate that a command is allowed under read-only mode: every other mode
/// allows it here, and read-only asks the core rule
/// ([`zerocode_core::shell_rule::read_only`]).
///
/// Corresponds to upstream `tools/BashTool/readOnlyValidation.ts`.
#[must_use]
pub fn validate_read_only(command: &str, mode: PermissionMode) -> ValidationResult {
    if mode != PermissionMode::ReadOnly {
        return ValidationResult::Allow;
    }
    read_only(command)
}

/// Minimal permission mode a bash command actually needs, derived from the
/// same rule that gates execution: a command whose every program, verb and
/// option is on the read-only list (e.g. `git log`, `grep`) only needs
/// [`PermissionMode::ReadOnly`], so read-only sessions can run it without
/// escalating. Anything unprovable keeps the bash tool's static
/// `DangerFullAccess` requirement — writes are deliberately not downgraded
/// to `WorkspaceWrite`, because shell path containment cannot be proven
/// statically ([`zerocode_core::shell_rule::proven_read_only`]).
#[must_use]
pub fn required_mode_for_command(command: &str) -> PermissionMode {
    if proven_read_only(command) {
        PermissionMode::ReadOnly
    } else {
        PermissionMode::DangerFullAccess
    }
}

// ---------------------------------------------------------------------------
// modeValidation
// ---------------------------------------------------------------------------

/// Validate that a command is consistent with the given permission mode.
///
/// Corresponds to upstream `tools/BashTool/modeValidation.ts`.
#[must_use]
pub fn validate_mode(command: &str, mode: PermissionMode) -> ValidationResult {
    match mode {
        PermissionMode::ReadOnly => validate_read_only(command, mode),
        PermissionMode::WorkspaceWrite => {
            // In workspace-write mode, check for system-level destructive
            // operations that go beyond workspace scope.
            if command_targets_outside_workspace(command) {
                return ValidationResult::Warn {
                    message:
                        "Command appears to target files outside the workspace — requires elevated permission"
                            .to_string(),
                };
            }
            ValidationResult::Allow
        }
        PermissionMode::DangerFullAccess | PermissionMode::Allow | PermissionMode::Prompt => {
            ValidationResult::Allow
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline: run all validations
// ---------------------------------------------------------------------------

/// Run the full validation pipeline on a bash command.
///
/// Returns the first non-Allow result, or Allow if all validations pass.
///
/// Shell-aware: a read-only session reads the whole line against the
/// read-only list, every simple command of it as the shell will run it
/// ([`read_only`]) — wrappers are not stripped first, because `env
/// GIT_EXTERNAL_DIFF=… git diff` is not `git diff`. Other modes check
/// **each** segment of a compound command (`a && b`, `a | b`, `a; b`, …)
/// after wrapper-stripping, so a safe leading command can never smuggle in
/// a blocked trailing one. The substring-based destructive and path checks
/// run once over the whole line (they already match anywhere).
#[must_use]
pub fn validate_command(command: &str, mode: PermissionMode, workspace: &Path) -> ValidationResult {
    // 1. The mode's own rule.
    let result = if mode == PermissionMode::ReadOnly {
        read_only(command)
    } else {
        split_command_segments(command)
            .into_iter()
            .map(|segment| validate_mode(strip_command_wrappers(segment), mode))
            .find(|result| *result != ValidationResult::Allow)
            .unwrap_or(ValidationResult::Allow)
    };
    if result != ValidationResult::Allow {
        return result;
    }

    // 2. Destructive command warnings (substring match over whole line).
    let result = check_destructive(command);
    if result != ValidationResult::Allow {
        return result;
    }

    // 3. Path validation (substring match over whole line).
    validate_paths(command, workspace)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
