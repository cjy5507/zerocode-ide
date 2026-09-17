//! Tool-name normalization and alias resolution.
//!
//! The Anthropic API ships tool calls in `PascalCase` while several internal
//! handlers still use `snake_case` for historical reasons. The dispatcher
//! resolves either spelling through [`canonical_tool_name`] backed by the
//! single [`TOOL_NAME_ALIASES`] table. Keep new aliases here so the renderer
//! and runtime stay in lockstep.

use runtime::PermissionMode;

use crate::error::ToolError;

/// Lower-case a tool name and replace `-` with `_` so matches are insensitive
/// to `PascalCase` / `snake_case` / kebab-case spelling.
pub(crate) fn normalize_tool_name(value: &str) -> String {
    value.trim().replace('-', "_").to_ascii_lowercase()
}

/// Canonical-name alias table for tool dispatch.
///
/// Keys are matched after [`normalize_tool_name`] (lowercased, `-`→`_`), so a
/// single entry handles e.g. `Read`, `read`, and `READ`.
///
pub(crate) const TOOL_NAME_ALIASES: &[(&str, &str)] = &[
    // File tools — PascalCase and short forms route to their handlers.
    ("read", "read_file"),
    ("write", "write_file"),
    ("edit", "edit_file"),
    ("multi_edit", "MultiEdit"),
    ("glob", "glob_search"),
    ("grep", "grep_search"),
    // Bash handler is registered under lowercase `bash`; force PascalCase
    // `Bash` (and `BASH`) to normalize to it.
    ("bash", "bash"),
    // Task/todo/web/misc canonical names are already PascalCase; the entries
    // below are defensive so snake_case spellings also resolve.
    ("todo_write", "TodoWrite"),
    ("notebook_edit", "NotebookEdit"),
    ("web_fetch", "WebFetch"),
    ("web_search", "WebSearch"),
    ("task", "TaskCreate"),
];

/// Map any accepted tool alias to the canonical handler name used by the
/// dispatcher. Returns the input unchanged when no alias matches, so
/// already-canonical names (including `PascalCase` handlers like `TodoWrite`)
/// remain a no-op.
#[must_use]
pub fn canonical_tool_name(name: &str) -> String {
    let normalized = normalize_tool_name(name);
    for (alias, canonical) in TOOL_NAME_ALIASES {
        if normalized == *alias {
            return (*canonical).to_string();
        }
    }
    name.to_string()
}

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
