//! Tool-name normalization and alias resolution.
//!
//! The Anthropic API ships tool calls in `PascalCase` while several internal
//! handlers still use `snake_case` for historical reasons. The dispatcher
//! resolves either spelling through [`canonical_tool_name`] backed by the
//! single [`TOOL_NAME_ALIASES`] table. Keep new aliases here so the renderer
//! and runtime stay in lockstep.

use runtime::PermissionMode;

use crate::error::ToolError;

// The one normalization dispatch and permission rules share: lower-case,
// `-` as `_`. It lives in `runtime`, where rules are judged and which this
// crate depends on.
pub(crate) use runtime::normalize_tool_name;

/// Canonical-name alias table for tool dispatch.
///
/// Keys are matched after [`normalize_tool_name`] (lowercased, `-`→`_`), so a
/// single entry handles e.g. `Read`, `read`, and `READ`.
///
pub(crate) const TOOL_NAME_ALIASES: &[(&str, &str)] = &[
    // File tools — PascalCase and short forms route to their handlers.
    ("read", crate::file_tools::READ_FILE_TOOL_NAME),
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

/// Alias keys that are zo's own shorthand rather than another agent's name for
/// the same tool: Claude Code's `Task` starts a sub-agent (zo's `Agent`),
/// while zo reads `task` as `TaskCreate`.
const ZO_SHORTHAND_ALIASES: &[&str] = &["task"];

/// zo's tool for `name` when the alias table maps it to one and the name means
/// that tool elsewhere too (`Edit` is `edit_file`): a hint at the tool a
/// permission rule written for another agent meant (t-81312). `None` for a
/// name the table does not map and for zo's own shorthand.
#[must_use]
pub fn same_tool_alias(name: &str) -> Option<&'static str> {
    let normalized = normalize_tool_name(name);
    TOOL_NAME_ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalized && !ZO_SHORTHAND_ALIASES.contains(alias))
        .map(|(_, canonical)| *canonical)
}

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
