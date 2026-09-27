//! Bash command validation — what this runtime's permission modes make of a
//! shell command.
//!
//! The rules themselves are the core crate's (`zerocode_core::shell_rule`,
//! t-10916): what a read-only session may not run, what is destructive, which
//! paths escape, a command's intent. They moved there so the window reads a
//! command by the same rules zo does; what stays here is the half that speaks
//! of a [`PermissionMode`]:
//! - `readOnlyValidation` — block write-like commands in read-only mode
//! - `modeValidation` — enforce permission mode constraints on commands
//! - `sedValidation` — validate sed expressions before execution
//! - the pipeline that runs them all ([`validate_command`])

use std::path::Path;

use crate::permissions::PermissionMode;

pub mod inspection;

pub use zerocode_core::shell_rule::{
    check_destructive, classify_command, git_worktree_escape_reason, path_within_root,
    reaches_outside_workspace, split_command_segments, validate_paths, CommandIntent,
    ValidationResult,
};
use zerocode_core::shell_rule::parse::strip_command_wrappers;
#[cfg(test)]
use zerocode_core::shell_rule::parse::extract_first_command;
use zerocode_core::shell_rule::{
    command_targets_outside_workspace, proven_read_only, read_only, read_only_sed,
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
/// same classifier that gates execution: a command every segment of which
/// passes read-only validation (e.g. `git log`, `grep`) only needs
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
// sedValidation
// ---------------------------------------------------------------------------

/// Validate sed expressions for safety: in read-only mode an in-place `sed`
/// is blocked ([`zerocode_core::shell_rule::read_only_sed`]); every other mode
/// allows it here.
///
/// Corresponds to upstream `tools/BashTool/sedValidation.ts`.
#[must_use]
pub fn validate_sed(command: &str, mode: PermissionMode) -> ValidationResult {
    if mode != PermissionMode::ReadOnly {
        return ValidationResult::Allow;
    }
    read_only_sed(command)
}

// ---------------------------------------------------------------------------
// Pipeline: run all validations
// ---------------------------------------------------------------------------

/// Run the full validation pipeline on a bash command.
///
/// Returns the first non-Allow result, or Allow if all validations pass.
///
/// Shell-aware: the per-command checks (mode/read-only and sed) run on
/// **each** segment of a compound command (`a && b`, `a | b`, `a; b`, …)
/// after wrapper-stripping, so a safe leading command can never smuggle
/// in a blocked trailing one. The substring-based destructive and path
/// checks run once over the whole line (they already match anywhere).
#[must_use]
pub fn validate_command(command: &str, mode: PermissionMode, workspace: &Path) -> ValidationResult {
    // 1 + 2. Per-segment mode and sed validation.
    for segment in split_command_segments(command) {
        let inner = strip_command_wrappers(segment);

        let result = validate_mode(inner, mode);
        if result != ValidationResult::Allow {
            return result;
        }

        let result = validate_sed(inner, mode);
        if result != ValidationResult::Allow {
            return result;
        }
    }

    // 3. Destructive command warnings (substring match over whole line).
    let result = check_destructive(command);
    if result != ValidationResult::Allow {
        return result;
    }

    // 4. Path validation (substring match over whole line).
    validate_paths(command, workspace)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
