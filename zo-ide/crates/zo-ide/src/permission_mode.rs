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
#[must_use]
pub fn permission_mode_from_label(mode: &str) -> PermissionMode {
    if let Some(mode) = mode_from_label(mode) {
        mode
    } else {
        {
            let other = mode;
            eprintln!(
                "warning: unsupported permission mode '{other}', falling back to read-only"
            );
            PermissionMode::ReadOnly
        }
    }
}

/// 케밥 라벨 → 모드. 저장소·플래그·화면이 쓰는 **한 벌의 어휘**다.
///
/// 이 표가 네 벌 있었고(여기·`workspace_trust` 두 개·`plain_session`) 이미
/// 어긋나 있었다: 신뢰 저장소는 `prompt`/`allow` 도 쓰는데 그 둘을 모르는
/// 판이 경고를 찍고 read-only 로 떨어뜨렸다.
#[must_use]
pub fn mode_from_label(label: &str) -> Option<PermissionMode> {
    match label {
        "read-only" => Some(PermissionMode::ReadOnly),
        "workspace-write" => Some(PermissionMode::WorkspaceWrite),
        "danger-full-access" => Some(PermissionMode::DangerFullAccess),
        "prompt" => Some(PermissionMode::Prompt),
        "allow" => Some(PermissionMode::Allow),
        _ => None,
    }
}

/// 모드 → 케밥 라벨. [`mode_from_label`] 의 역이다.
#[must_use]
pub const fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read-only",
        PermissionMode::WorkspaceWrite => "workspace-write",
        PermissionMode::DangerFullAccess => "danger-full-access",
        PermissionMode::Prompt => "prompt",
        PermissionMode::Allow => "allow",
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
#[must_use]
pub fn default_permission_mode() -> PermissionMode {
    resolve_permission_mode(fallback_permission_mode_for_current_dir)
}

/// Like [`default_permission_mode`] but routes the cwd fallback through the
/// interactive workspace-trust gate (the Claude-Code-style "trust this folder?"
/// prompt). Used only by the TUI repl entry point, so the prompt never fires on
/// a headless `-p`/serve path — those keep going through
/// [`default_permission_mode`].
#[must_use]
pub fn interactive_default_permission_mode(inline: bool) -> PermissionMode {
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
#[must_use]
pub fn normalize_permission_mode(mode: &str) -> Option<&'static str> {
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
