//! Tool-name normalization and alias resolution.
//!
//! The table itself lives in `core_types::tool_names` (t-96186), where the
//! conversation loop, the permission rules, the hooks and the session's
//! history reconciler read it too: one table of every spelling a tool answers
//! to, so a call, a rule and a rendered row can never disagree about which
//! tool a name means. This module re-exports it under the names the
//! dispatcher always used.

use runtime::PermissionMode;

use crate::error::ToolError;

pub use core_types::tool_names::canonical_tool_name;
pub(crate) use core_types::tool_names::{normalize_tool_name, TOOL_NAME_ALIASES};

/// Translate the plugin-declared permission string into the runtime enum.
pub(crate) fn permission_mode_from_plugin(value: &str) -> Result<PermissionMode, ToolError> {
    match value {
        "read-only" => Ok(PermissionMode::ReadOnly),
        "workspace-write" => Ok(PermissionMode::WorkspaceWrite),
        "danger-full-access" => Ok(PermissionMode::DangerFullAccess),
        other => Err(ToolError::InvalidInput(format!(
            "unsupported plugin permission: {other}"
        ))),
    }
}
