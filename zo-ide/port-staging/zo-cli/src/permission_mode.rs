//! Permission-mode resolution helpers.
//!
//! One cohesive concern lifted out of `main.rs`: turning the various
//! permission-mode representations (CLI label, config-resolved enum, env
//! override) into the runtime's [`PermissionMode`]. The crate root re-exports
//! the `pub(crate)` entry points so existing `crate::…` call sites are
//! unchanged; the resolution-detail helpers stay private to this module.

use std::path::Path;

use runtime::{ConfigLoader, PermissionMode, ResolvedPermissionMode};

use crate::current_cli_cwd;

/// Map a CLI/label string to a [`PermissionMode`], warning and defaulting to
/// `read-only` on an unrecognised label.
pub(crate) fn permission_mode_from_label(mode: &str) -> PermissionMode {
    match mode {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => {
            eprintln!(
                "warning: unsupported permission mode '{other}', falling back to read-only"
            );
            PermissionMode::ReadOnly
        }
    }
}

fn permission_mode_from_resolved(mode: ResolvedPermissionMode) -> PermissionMode {
    match mode {
        ResolvedPermissionMode::ReadOnly => PermissionMode::ReadOnly,
        ResolvedPermissionMode::WorkspaceWrite => PermissionMode::WorkspaceWrite,
        ResolvedPermissionMode::DangerFullAccess => PermissionMode::DangerFullAccess,
    }
}

/// The effective default permission mode: env override first, then the config
/// for the current directory, then a workspace-risk-aware fallback.
pub(crate) fn default_permission_mode() -> PermissionMode {
    resolve_permission_mode(fallback_permission_mode_for_current_dir)
}

/// Like [`default_permission_mode`] but routes the cwd fallback through the
/// interactive workspace-trust gate (the Claude-Code-style "trust this folder?"
/// prompt). Used only by the TUI repl entry point, so the prompt never fires on
/// a headless `-p`/serve path — those keep going through
/// [`default_permission_mode`].
pub(crate) fn interactive_default_permission_mode(inline: bool) -> PermissionMode {
    resolve_permission_mode(|| interactive_fallback_permission_mode_for_current_dir(inline))
}

/// Shared config → `fallback` resolution. The environment override is
/// intentionally NOT consulted: the permission mode comes only from the actual
/// configured value for the current directory (the live `/permissions`
/// selection persisted to settings), falling back to the workspace-risk-aware
/// default when nothing is configured. Only the cwd fallback differs between
/// the headless and interactive entry points.
fn resolve_permission_mode(fallback: impl FnOnce() -> PermissionMode) -> PermissionMode {
    config_permission_mode_for_current_dir().unwrap_or_else(fallback)
}

fn interactive_fallback_permission_mode_for_current_dir(inline: bool) -> PermissionMode {
    current_cli_cwd().ok().as_deref().map_or(
        PermissionMode::DangerFullAccess,
        |cwd| {
            crate::workspace_trust::resolve_trust_for_cwd(
                cwd,
                fallback_permission_mode_for_cwd,
                inline,
            )
        },
    )
}

/// Whether the session will present inline, resolving CLI flags against
/// `settings.tui.inlineMode` exactly like the session bootstrap does.
///
/// The first-visit workspace-trust prompt runs before the TUI exists, so it has
/// to reach the same verdict on its own — a mismatch would open the prompt on
/// the alternate screen and then drop the session into inline (or the reverse).
/// Config load errors fail open to the default because the normal runtime
/// bootstrap reports them with full context moments later.
pub(crate) fn resolved_tui_mode_is_inline(inline_flag: bool, fullscreen_flag: bool) -> bool {
    let configured = current_cli_cwd()
        .ok()
        .and_then(|cwd| ConfigLoader::default_for(&cwd).load().ok())
        .and_then(|config| config.tui_inline_mode());
    zo_cli::tui::TerminalMode::resolve(inline_flag, fullscreen_flag, configured).is_inline()
}

fn config_permission_mode_for_current_dir() -> Option<PermissionMode> {
    let cwd = current_cli_cwd().ok()?;
    let loader = ConfigLoader::default_for(&cwd);
    loader
        .load()
        .ok()?
        .permission_mode()
        .map(permission_mode_from_resolved)
}

fn fallback_permission_mode_for_current_dir() -> PermissionMode {
    current_cli_cwd().ok().as_deref().map_or(
        PermissionMode::DangerFullAccess,
        fallback_permission_mode_for_cwd,
    )
}

fn fallback_permission_mode_for_cwd(cwd: &Path) -> PermissionMode {
    if crate::workspace_trust::classify_cwd(cwd).requires_safe_default() {
        PermissionMode::ReadOnly
    } else {
        PermissionMode::DangerFullAccess
    }
}

/// Canonicalise a permission-mode label, including Claude Code-compatible
/// aliases, or return `None` when the label is unsupported.
pub(crate) fn normalize_permission_mode(mode: &str) -> Option<&'static str> {
    match mode.trim() {
        "read-only" | "plan" => Some("read-only"),
        "workspace-write" | "default" | "acceptEdits" => Some("workspace-write"),
        "danger-full-access" | "bypassPermissions" => Some("danger-full-access"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_permission_mode, permission_mode_from_label};
    use runtime::PermissionMode;

    #[test]
    fn normalize_permission_mode_accepts_supported_modes() {
        assert_eq!(normalize_permission_mode("read-only"), Some("read-only"));
        assert_eq!(
            normalize_permission_mode("workspace-write"),
            Some("workspace-write")
        );
        assert_eq!(
            normalize_permission_mode("danger-full-access"),
            Some("danger-full-access")
        );
        assert_eq!(
            normalize_permission_mode("  read-only  "),
            Some("read-only")
        );
        assert_eq!(normalize_permission_mode("unknown"), None);
    }

    #[test]
    fn normalize_permission_mode_accepts_claude_code_aliases() {
        assert_eq!(normalize_permission_mode("default"), Some("workspace-write"));
        assert_eq!(normalize_permission_mode("acceptEdits"), Some("workspace-write"));
        assert_eq!(normalize_permission_mode("plan"), Some("read-only"));
        assert_eq!(
            normalize_permission_mode("bypassPermissions"),
            Some("danger-full-access")
        );
        assert_eq!(normalize_permission_mode("acceptedits"), None);
    }

    #[test]
    fn permission_mode_from_unknown_label_fails_safe() {
        assert_eq!(permission_mode_from_label("future-mode"), PermissionMode::ReadOnly);
    }
}
