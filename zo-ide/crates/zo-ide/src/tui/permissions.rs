//! Codex-style `/permissions` picker data and zo mode mapping.
//!
//! 문안은 Codex 0.150.0 의 preset 설명을 그대로 쓰되 **주어만 `zo`** 다. 이
//! 화면은 zo 패인에서 zo 가 무엇을 해도 되는지 묻는 자리이고, 우리는 이미
//! 어시스턴트 라벨을 `codex` 에서 `zo` 로 옮겼다(4라운드). 남의 제품 이름으로
//! 우리 권한을 설명하면 바이트는 같아져도 문장이 거짓이 된다.
//!
//! Codex presents approval presets rather than the runtime enum directly. Zo
//! keeps that presentation vocabulary in one table and maps each selectable
//! preset to the closest existing [`runtime::PermissionMode`]. The plain
//! `/permissions <mode>` parser remains the canonical path for the explicit
//! `read-only` label, including on platforms where Codex hides that preset
//! from its interactive menu.

use runtime::PermissionMode;

/// The picker title emitted by Codex 0.150.0.
pub const TITLE: &str = "Update Model Permissions";

/// One Codex approval preset as exposed by the zo picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub mode: PermissionMode,
    pub label: String,
    pub description: &'static str,
}

/// Return the visible non-Windows Codex presets, marking the closest current
/// zo mode.
///
/// `Approve for me` shares zo's workspace-write sandbox because zo's runtime
/// has no separate automatic-review mode — and its description **says so**.
/// It used to carry Codex's own sentence ("Only ask for actions detected as
/// potentially unsafe"), which describes a policy zo does not have: picking it
/// changed nothing at all, while the screen promised narrower prompting. Copy
/// parity is worth a lot on this picker, but not on the one surface where a
/// person decides how much the agent may do without asking.
#[must_use]
pub fn choices(current: PermissionMode) -> Vec<Choice> {
    vec![
        Choice {
            mode: PermissionMode::WorkspaceWrite,
            label: (if current == PermissionMode::WorkspaceWrite {
                "Ask for approval (current)"
            } else {
                "Ask for approval"
            })
            .to_string(),
            description: "zo can read and edit files in the current workspace, and run commands. Approval is required to access the internet or edit other files.",
        },
        Choice {
            mode: PermissionMode::WorkspaceWrite,
            label: "Approve for me".to_string(),
            description: "Same workspace sandbox as above — zo has no separate automatic-review mode yet.",
        },
        Choice {
            mode: PermissionMode::DangerFullAccess,
            label: (if current == PermissionMode::DangerFullAccess {
                "Full Access (current)"
            } else {
                "Full Access"
            })
            .to_string(),
            description: "zo can edit files outside this workspace and access the internet without asking for approval. Exercise caution when using.",
        },
    ]
}

/// User-facing label for the three zo permission modes.
#[must_use]
pub const fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "Read Only",
        PermissionMode::WorkspaceWrite | PermissionMode::Prompt => "Ask for approval",
        PermissionMode::Allow | PermissionMode::DangerFullAccess => "Full Access",
    }
}

/// How much a mode allows, for comparing two of them.
///
/// Only the ordering matters, and only so the screen can tell widening from
/// narrowing — the two directions are not symmetric mid-turn and must not be
/// reported with the same sentence.
pub(super) const fn permission_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::ReadOnly => 0,
        PermissionMode::DangerFullAccess => 2,
        _ => 1,
    }
}

/// The name used when a mode is **active**, shared with the footer.
///
/// The permissions picker keeps Codex's preset label (`Read Only`) because it
/// describes a selectable sandbox. Once active, the footer calls that same
/// state `Plan mode`; transition notes must source that wording from the
/// footer constant instead of inventing a second name for the same state.
pub(super) const fn active_permission_label(mode: PermissionMode) -> &'static str {
    if matches!(mode, PermissionMode::ReadOnly) {
        crate::status_format::PLAN_MODE_FOOTER_TEXT
    } else {
        mode_label(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::{choices, mode_label};
    use runtime::PermissionMode;

    /// Two rows that land the same mode must not describe different policies.
    ///
    /// This picker is where a person decides how much the agent may do without
    /// asking, so a row that promises narrower prompting and delivers the row
    /// above it is the one lie this screen cannot carry. Codex has a separate
    /// automatic-review mode; zo does not, and until it does the row says so.
    #[test]
    fn a_row_that_shares_a_mode_admits_it() {
        let rows = choices(PermissionMode::WorkspaceWrite);
        for (index, row) in rows.iter().enumerate() {
            let shares = rows
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.mode == row.mode);
            if !shares {
                continue;
            }
            let admits = row.description.to_lowercase().contains("same")
                || row.label.to_lowercase().contains("current");
            assert!(
                admits,
                "`{}` lands the same mode as another row but its description \
                 claims a policy of its own: {:?}",
                row.label, row.description
            );
        }
    }

    /// Specifically: Codex's aspirational sentence must not come back while the
    /// mode behind it is still workspace-write.
    #[test]
    fn the_unsafe_only_promise_is_not_made_without_a_mode_behind_it() {
        let rows = choices(PermissionMode::WorkspaceWrite);
        for row in &rows {
            if row.description.contains("detected as potentially unsafe") {
                assert_ne!(
                    row.mode,
                    PermissionMode::WorkspaceWrite,
                    "the row promising unsafe-only prompting still maps to the plain \
                     workspace sandbox — implement the mode or drop the promise"
                );
            }
        }
    }

    #[test]
    fn choices_keep_codex_order_and_map_to_existing_modes() {
        let rows = choices(PermissionMode::WorkspaceWrite);
        assert_eq!(
            rows.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            vec!["Ask for approval (current)", "Approve for me", "Full Access"]
        );
        assert_eq!(
            rows.iter().map(|row| row.mode).collect::<Vec<_>>(),
            vec![
                PermissionMode::WorkspaceWrite,
                PermissionMode::WorkspaceWrite,
                PermissionMode::DangerFullAccess,
            ]
        );
    }

    #[test]
    fn full_access_marks_the_current_row() {
        assert_eq!(
            choices(PermissionMode::DangerFullAccess)[2].label,
            "Full Access (current)"
        );
    }

    #[test]
    fn labels_fold_runtime_aliases_to_the_three_cli_modes() {
        assert_eq!(mode_label(PermissionMode::ReadOnly), "Read Only");
        assert_eq!(mode_label(PermissionMode::WorkspaceWrite), "Ask for approval");
        assert_eq!(mode_label(PermissionMode::DangerFullAccess), "Full Access");
    }
}
