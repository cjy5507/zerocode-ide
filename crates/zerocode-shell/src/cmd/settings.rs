//! Settings commands.

use crate::*;

/// Remember which shortcut rows were put away.
///
/// A name this window does not draw is refused rather than stored: the only
/// way back is the settings toggle that names the same row, so a row nobody
/// can name is a row nobody could restore.
#[tauri::command(async)]
pub(crate) fn set_hidden_shortcuts(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    rows: Vec<String>,
) -> Result<SettingsSnapshot, String> {
    validate_shortcut_rows(&rows)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDDEN_SHORTCUTS],
        move |settings| {
            settings.hidden_shortcuts = rows;
            Ok(())
        },
    )
}

/// Change one shortcut row without replacing a stale window's whole list.
#[tauri::command(async)]
pub(crate) fn set_shortcut_visibility(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    name: String,
    visible: bool,
) -> Result<SettingsSnapshot, String> {
    if !SHORTCUTS.contains(&name.as_str()) {
        return Err(format!("{name}는 이 창에 있는 줄이 아닙니다"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDDEN_SHORTCUTS],
        move |settings| {
            settings.hidden_shortcuts.retain(|row| row != &name);
            if !visible {
                settings.hidden_shortcuts.push(name);
            }
            Ok(())
        },
    )
}

/// Put a task source away, or bring it back.
///
/// Whole list rather than one name, the same shape `set_hidden_shortcuts` has:
/// the window owns which sources are away and says so, instead of the two
/// sides keeping separate tallies that can disagree.
#[tauri::command(async)]
pub(crate) fn set_hidden_task_sources(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    rows: Vec<String>,
) -> Result<SettingsSnapshot, String> {
    let known: Vec<String> = rows
        .into_iter()
        .filter(|name| TASK_SOURCES.contains(&name.as_str()))
        .collect();
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDDEN_TASK_SOURCES],
        move |settings| {
            settings.hidden_task_sources = known;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_task_source_visibility(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    source: String,
    visible: bool,
) -> Result<SettingsSnapshot, String> {
    if !TASK_SOURCES.contains(&source.as_str()) {
        return Err(format!("{source}는 지원하는 작업 소스가 아닙니다"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDDEN_TASK_SOURCES],
        move |settings| {
            settings.hidden_task_sources.retain(|row| row != &source);
            if !visible {
                let would_hide_every_source = TASK_SOURCES.iter().all(|candidate| {
                    *candidate == source
                        || settings
                            .hidden_task_sources
                            .iter()
                            .any(|row| row == candidate)
                });
                if would_hide_every_source {
                    return Err("작업 소스를 하나 이상 표시해야 합니다".to_string());
                }
                settings.hidden_task_sources.push(source);
            }
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_usage_percentage_display(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    display: UsagePercentageDisplay,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::USAGE_PERCENTAGE_DISPLAY],
        move |settings| {
            settings.usage_percentage_display = display;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_status_bar_usage_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: StatusBarUsageMode,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::STATUS_BAR_USAGE_MODE],
        move |settings| {
            settings.status_bar_usage_mode = mode;
            Ok(())
        },
    )
}

/// The rows one Source Control section shows in tree mode.
///
/// The window sends the paths it is ALREADY showing, in the order it is
/// showing them, and gets back the folders that hold them. Two consequences
/// are the point: a filtered list yields a filtered tree without this side
/// knowing what a filter is, and the file order stays the one the flat list
/// decided — the two views can never disagree about which file comes first.
#[tauri::command]
pub(crate) fn scm_tree_rows(
    area: String,
    paths: Vec<String>,
    folded: Vec<String>,
) -> Vec<zerocode_core::scm_tree::Row> {
    let _crumb = crate::crumbs::Command::enter("scm_tree_rows");
    zerocode_core::scm_tree::rows(&area, &paths, &folded.into_iter().collect())
}

/// Turns one provider's token ledger on or off.
///
/// The original keeps this per provider rather than as one master switch
/// (`ClaudeUsagePane`/`CodexUsagePane` each own a `Switch`), because the two
/// ledgers are two different reads of two different disks.
#[tauri::command(async)]
pub(crate) fn set_usage_analytics_enabled(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    provider: String,
    enabled: bool,
) -> Result<SettingsSnapshot, String> {
    if !LEDGER_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("{provider}는 토큰 원장을 가진 제공자가 아닙니다"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::USAGE_ANALYTICS_OFF],
        move |settings| {
            settings.usage_analytics_off.retain(|row| row != &provider);
            if !enabled {
                settings.usage_analytics_off.push(provider);
            }
            Ok(())
        },
    )
}

/// Which way the changed files stand. Orca keeps this per person, not per
/// window, and the toggle lives in the overflow menu rather than on the bar —
/// it is chosen rarely and looked past afterwards.
#[tauri::command(async)]
pub(crate) fn set_source_control_view_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: SourceControlViewMode,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SOURCE_CONTROL_VIEW_MODE],
        move |settings| {
            settings.source_control_view_mode = mode;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_default_task_source(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    source: String,
) -> Result<SettingsSnapshot, String> {
    if !TASK_SOURCES.contains(&source.as_str()) {
        return Err(format!("{source}는 지원하는 작업 소스가 아닙니다"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::DEFAULT_TASK_SOURCE],
        move |settings| {
            if settings
                .hidden_task_sources
                .iter()
                .any(|row| row == &source)
            {
                return Err("숨긴 소스는 기본 소스로 고를 수 없습니다".to_string());
            }
            settings.default_task_source = source;
            Ok(())
        },
    )
}

/// Hide the workspaces automations cut, or bring them back.
///
/// Stored the same way every other sidebar display option is, so it survives
/// the restart that a `NewPerRun` job's overnight worktrees will otherwise be
/// waiting on the other side of.
/// Let an orchestrating agent open panes for the agents it starts — or stop it.
///
/// Three answers rather than a switch, because the middle one is a real state:
/// `in-process` is Claude's own way of running a team without any terminal at
/// all, and it is what a machine where the shim cannot be put on `PATH` is
/// honestly capable of. Saying so beats a switch that appears to be on and
/// silently makes no panes.
#[tauri::command(async)]
pub(crate) fn set_agent_teams_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: TeamsMode,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::AGENT_TEAMS_MODE],
        move |settings| {
            settings.agent_teams_mode = mode;
            Ok(())
        },
    )
}

/// What that setting is now — read by the settings pane, which cannot see the
/// file.
#[tauri::command(async)]
pub(crate) fn agent_teams_mode(state: State<'_, AppState>) -> Result<TeamsMode, String> {
    Ok(load_settings(state.settings())?.document.agent_teams_mode)
}

/// Whether the window may switch the Claude account by itself (t-7538).
/// Refused for a word the table does not hold, so the file never carries
/// one the beat cannot read.
#[tauri::command(async)]
pub(crate) fn set_claude_autoswitch_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: String,
) -> Result<SettingsSnapshot, String> {
    let mode = zerocode_core::account_autoswitch::AutoSwitchMode::of(&mode)
        .ok_or_else(|| format!("{mode}는 이 창이 아는 자동 전환 낱말이 아닙니다 (off|ask|auto)"))?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CLAUDE_AUTOSWITCH_MODE],
        move |settings| {
            settings.claude_autoswitch_mode = mode;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn claude_autoswitch_mode(
    state: State<'_, AppState>,
) -> Result<zerocode_core::account_autoswitch::AutoSwitchMode, String> {
    Ok(load_settings(state.settings())?
        .document
        .claude_autoswitch_mode)
}

#[tauri::command(async)]
pub(crate) fn set_hide_automation_workspaces(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    hidden: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDE_AUTOMATION_WORKSPACES],
        move |settings| {
            settings.hide_automation_workspaces = hidden;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_hide_default_branch_workspaces(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    hidden: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDE_DEFAULT_BRANCH_WORKSPACES],
        move |settings| {
            settings.hide_default_branch_workspaces = hidden;
            Ok(())
        },
    )
}

/// 카드가 그리는 속성 하나를 켜거나 끈다.
///
/// 형제의 상태를 실어 보내지 않는 항목 패치다 — 두 창이 각각 다른 속성을 끄는
/// 동안 나중에 도착한 쓰기가 앞의 것을 되돌리지 않는다.
#[tauri::command(async)]
pub(crate) fn set_worktree_card_property(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    property: String,
    shown: bool,
) -> Result<SettingsSnapshot, String> {
    if !WORKTREE_CARD_PROPERTIES.contains(&property.as_str()) {
        return Err(format!("unknown worktree card property: {property}"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKTREE_CARD_PROPERTIES],
        move |settings| {
            if shown {
                if !settings.worktree_card_properties.contains(&property) {
                    settings.worktree_card_properties.push(property);
                }
            } else {
                settings
                    .worktree_card_properties
                    .retain(|held| held != &property);
            }
            settings.worktree_card_properties =
                normalize_worktree_card_properties(&settings.worktree_card_properties);
            Ok(())
        },
    )
}

/// Rename, restyle, move, add, or remove one workspace-board lane.
///
/// This is an item patch rather than a replacement list: two windows editing
/// different statuses cannot overwrite each other's entire lane vocabulary.
#[tauri::command(async)]
pub(crate) fn patch_workspace_board_status(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: WorkspaceBoardStatusPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKSPACE_BOARD],
        move |settings| settings.workspace_board.patch_status(patch),
    )
}

/// Move one or several catalogued workspaces to a lane and/or change their
/// pinned state. Both fields ride one mutation so a drop on the pin target can
/// preserve status without racing a simultaneous status drop.
#[tauri::command(async)]
pub(crate) fn patch_workspace_board_items(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    paths: Vec<String>,
    status: Option<String>,
    pinned: Option<bool>,
    automatic: Option<bool>,
) -> Result<SettingsSnapshot, String> {
    if automatic == Some(true) && status.is_some() {
        return Err("자동 상태와 수동 단계를 동시에 지정할 수 없습니다".to_string());
    }
    if status.is_none() && pinned.is_none() && automatic != Some(true) {
        return Err("바꿀 워크스페이스 보드 값이 없습니다".to_string());
    }
    let paths = known_workspace_board_paths(state.config_root(), paths)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKSPACE_BOARD],
        move |settings| {
            if status
                .as_deref()
                .is_some_and(|id| !settings.workspace_board.knows_status(id))
            {
                return Err("이 보드에 없는 상태입니다".to_string());
            }
            for path in paths {
                let card = settings
                    .workspace_board
                    .cards
                    .entry(path.clone())
                    .or_default();
                if automatic == Some(true) {
                    card.status = None;
                } else if let Some(status) = &status {
                    card.status = Some(status.clone());
                }
                if let Some(pinned) = pinned {
                    card.pinned = pinned;
                }
                if card.status.is_none() && !card.pinned {
                    settings.workspace_board.cards.remove(&path);
                }
            }
            Ok(())
        },
    )
}

/// Persist the width shared by every board lane. The backend clamps it too:
/// pointer arithmetic belongs to the renderer, but the durable value must be
/// valid even when a settings file or a stale window sends something else.
#[tauri::command(async)]
pub(crate) fn set_workspace_board_column_width(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    width: u32,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKSPACE_BOARD],
        move |settings| {
            settings.workspace_board.column_width = width.clamp(
                WORKSPACE_BOARD_COLUMN_WIDTH_MIN,
                WORKSPACE_BOARD_COLUMN_WIDTH_MAX,
            );
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_hide_sleeping_workspaces(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    hidden: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDE_SLEEPING_WORKSPACES],
        move |settings| {
            settings.hide_sleeping_workspaces = hidden;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_keep_default_branch_awake(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    keep: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::KEEP_DEFAULT_BRANCH_AWAKE],
        move |settings| {
            settings.keep_default_branch_awake = keep;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_hide_agent_scratch_workspaces(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    hidden: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDE_AGENT_SCRATCH_WORKSPACES],
        move |settings| {
            settings.hide_agent_scratch_workspaces = hidden;
            Ok(())
        },
    )
}

/// 사이드바의 세 시선을 한 번에 적는다.
///
/// 하나의 패치인 이유는 이것이 하나의 메뉴에서 오는 하나의 선택이기 때문이다 —
/// 셋으로 나누면 창이 선택의 3분의 2만 흡수한 채 서 있을 수 있다.
#[tauri::command(async)]
pub(crate) fn set_sidebar_view(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    group_by: String,
    sort_by: String,
    project_order: String,
) -> Result<SettingsSnapshot, String> {
    let view = SidebarView::parsed(&group_by, &sort_by, &project_order)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SIDEBAR_VIEW],
        move |settings| {
            settings.sidebar_view = view;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_hide_detached_head_workspaces(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    hidden: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HIDE_DETACHED_HEAD_WORKSPACES],
        move |settings| {
            settings.hide_detached_head_workspaces = hidden;
            Ok(())
        },
    )
}

/// Ask before a pinned tab closes, or stop asking.
///
/// Orca's own switch behind `confirmClosePinnedTab`. Stored the way every
/// other one-bit display setting is, and it survives the restart for the
/// reason those do: a person who turned the question off and met it again on
/// the next launch has been told the switch does not work.
///
/// What it turns off is the QUESTION, never the guard. The bulk closes filter
/// pinned tabs out of their own lists (`closeOtherTabs`/`closeTabsToRight`,
/// I18nProvider:58085) and are not on this wire at all — with this off, a
/// pinned tab still cannot be taken by "close the others"; it can only be
/// closed by aiming at it.
#[tauri::command(async)]
pub(crate) fn set_confirm_close_pinned(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CONFIRM_CLOSE_PINNED],
        move |settings| {
            settings.confirm_close_pinned = on;
            Ok(())
        },
    )
}

/// Remember Orca's "don't ask again for running terminals" answer.
///
/// The stored bit is the skip instruction Orca persists, not a renderer-local
/// inverse. That keeps every window and the next boot on one answer.
#[tauri::command(async)]
pub(crate) fn set_skip_close_terminal_with_running_process_confirm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    skip: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SKIP_CLOSE_TERMINAL_WITH_RUNNING_PROCESS_CONFIRM],
        move |settings| {
            settings.skip_close_terminal_with_running_process_confirm = skip;
            Ok(())
        },
    )
}

/// Choose whether Control+Tab follows visit history or the visible strip.
#[tauri::command(async)]
pub(crate) fn set_ctrl_tab_order_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: CtrlTabOrderMode,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CTRL_TAB_ORDER_MODE],
        move |settings| {
            settings.ctrl_tab_order_mode = mode;
            Ok(())
        },
    )
}

/// Change one workspace-location answer against the latest canonical
/// document. A field patch is essential here: two open windows may edit the
/// directory and nesting switch independently, and neither may replace the
/// sibling value it did not change.
#[tauri::command(async)]
pub(crate) fn patch_workspace_creation_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: WorkspaceCreationPrefsPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WORKSPACE_CREATION_PREFS],
        move |settings| patch.apply(&mut settings.workspace_creation_prefs),
    )
}

/// Change one floating-workspace answer against the latest canonical
/// document — a field patch for the same reason the workspace one is: the
/// enable switch, the directory and the trigger location are edited
/// independently, and none may replace a sibling it did not change.
#[tauri::command(async)]
pub(crate) fn patch_floating_workspace(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: FloatingWorkspacePatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::FLOATING_WORKSPACE],
        move |settings| patch.apply(&mut settings.floating_workspace),
    )
}

/// Where the floating shell would start right now — the settings page's
/// directory field asks, so a stored directory that has since disappeared
/// reads as the home it will actually fall back to, never as a path the
/// shell will not sit in. Orca's pane makes the same read-time resolution
/// (`getFloatingTerminalCwd`, FloatingWorkspacePane.tsx:44-61).
#[tauri::command(async)]
pub(crate) fn floating_workspace_seat(state: State<'_, AppState>) -> Result<String, String> {
    let prefs = load_settings_for_boot(state.settings())
        .document
        .floating_workspace;
    Ok(resolved_floating_workspace_seat(&prefs)?
        .display()
        .to_string())
}

/// Add, edit, or remove one workspace launcher against the latest document.
///
/// Item patches preserve disjoint edits from two open windows: adding Cursor
/// in one window cannot replace a custom application added in another.
#[tauri::command(async)]
pub(crate) fn patch_open_in_applications(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: OpenInApplicationsPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::OPEN_IN_APPLICATIONS],
        move |settings| patch.apply(&mut settings.open_in_applications),
    )
}

/// Choose whether a context-menu workspace deletion asks before the first
/// safe removal attempt. Skipping the question never implies force: a refused
/// safe removal returns to the existing loss review and explicit force path.
#[tauri::command(async)]
pub(crate) fn set_skip_delete_worktree_confirm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    skip: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SKIP_DELETE_WORKTREE_CONFIRM],
        move |settings| {
            settings.skip_delete_worktree_confirm = skip;
            Ok(())
        },
    )
}

/// Choose whether deleting an automation asks before removing both the
/// schedule and its run history. Generated worktrees are never part of this
/// operation.
#[tauri::command(async)]
pub(crate) fn set_skip_delete_automation_confirm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    skip: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SKIP_DELETE_AUTOMATION_CONFIRM],
        move |settings| {
            settings.skip_delete_automation_confirm = skip;
            Ok(())
        },
    )
}

/// How long the artifact store keeps what agents made. Clamped to the
/// artifact table's cap; `0` follows the ledger. The store learns the new
/// days here, on the same commit — settings only call into the runtime.
#[tauri::command(async)]
pub(crate) fn set_artifacts_retention_days(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    days: u32,
) -> Result<SettingsSnapshot, String> {
    let days = days.min(artifact_runtime::retention_days_max());
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::ARTIFACTS_RETENTION_DAYS],
        move |settings| {
            settings.artifacts_retention_days = days;
            Ok(())
        },
    )?;
    artifact_runtime::note_retention_days(days);
    Ok(snapshot)
}

/// Which shape a diff opens in. Orca's `renderSideBySide` toggle, stored.
///
/// A view preference and not a document one: it is asked once per person, not
/// once per file, which is why it is here beside the other one-bit display
/// settings rather than on the tab.
#[tauri::command(async)]
pub(crate) fn set_diff_side_by_side(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::DIFF_SIDE_BY_SIDE],
        move |settings| {
            settings.diff_side_by_side = on;
            Ok(())
        },
    )
}

/// Whether a conversation folds each turn's tool work behind one summary row
/// — the extension's 「Focus view」 (2.1.221).
///
/// A view preference and not a page one, for `set_diff_side_by_side`'s reason:
/// a person asks for the quieter transcript once, and every conversation they
/// open afterwards — a helper's page, a pane's view, a wire's session — is the
/// one they asked for. The toggle stands on the conversation's own head rather
/// than in the settings window, the way the extension keeps it in its command
/// menu: it is reached while reading, not while configuring.
#[tauri::command(async)]
pub(crate) fn set_conversation_focus_view(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CONVERSATION_FOCUS_VIEW],
        move |settings| {
            settings.conversation_focus_view = on;
            Ok(())
        },
    )
}

/// Change one editor/diff wrapping preference against the latest document.
/// The tagged patch lets stale windows edit different controls without either
/// replacing the sibling value it has not re-read yet.
#[tauri::command(async)]
pub(crate) fn patch_editing_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: EditingPrefsPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::EDITING_PREFS],
        move |settings| {
            patch.apply(&mut settings.editing_prefs);
            Ok(())
        },
    )
}

/// Read Linux's PRIMARY selection. The webview calls this only on Linux and
/// falls back to its private buffer when a compositor does not offer it.
#[tauri::command(async)]
pub(crate) fn read_primary_selection(state: State<'_, AppState>) -> Result<String, String> {
    state.primary_selection().read_text()
}

/// Replace Linux's PRIMARY selection with a bounded plain-text value.
#[tauri::command(async)]
pub(crate) fn write_primary_selection(
    state: State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    state.primary_selection().write_text(text)
}

/// Installed font families for the IDE and editor autocompletes.
///
/// Discovery is lazy, cached, bounded, and best-effort in `system_fonts`; a
/// failure returns its short platform fallback rather than breaking Settings.
#[tauri::command]
pub(crate) async fn list_system_fonts() -> Vec<String> {
    system_fonts::list().await
}

/// Move a chord, put it back, or take it away — one call, three meanings.
///
/// Measured verbatim: Orca has a single `keybindings.setAction({ actionId,
/// bindings })` and the value of `bindings` says which of the three it is —
/// a list overrides, `null` resets to the default, `[]` disables the action
/// outright (`store-BgJxB0hr.js:33343-33372`, where `setKeybindingOverride`,
/// `resetKeybindingOverride` and `disableKeybindingAction` all funnel into
/// that one call). `Option<Vec<String>>` carries exactly that tri-state over
/// the wire: `null` arrives as `None` and `[]` as `Some(vec![])`.
///
/// It answers with the whole override map rather than an acknowledgement,
/// because that is what Orca's own store does with the result — it replaces
/// its state from the returned snapshot instead of patching what it sent
/// (`applySnapshot`). The window never has to guess what the file now says.
#[tauri::command(async)]
pub(crate) fn set_keybinding(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    action_id: String,
    bindings: Option<Vec<String>>,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::KEYBINDINGS],
        move |settings| apply_keybinding(&mut settings.keybindings, action_id, bindings),
    )
}

/// Remember how wide the person dragged the two columns.
///
/// Clamped rather than refused: a drag cannot produce a width outside the
/// bounds anyway, so a value that is outside them came from a file somebody
/// edited — and the useful answer to that is the nearest width this window
/// can actually lay out, not an error on a panel drag.
#[tauri::command(async)]
pub(crate) fn set_panel_widths(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    sidebar: u32,
    aside: u32,
) -> Result<SettingsSnapshot, String> {
    let kept = PanelWidths { sidebar, aside }.clamped();
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::PANEL_WIDTHS],
        move |settings| {
            settings.panel_widths = kept;
            Ok(())
        },
    )
}

/// Remember only the column the person moved. The repository mutation reads
/// the latest pair under its lock, so a stale second window cannot overwrite
/// a disjoint drag from the first one.
#[tauri::command(async)]
pub(crate) fn set_panel_width(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    side: PanelSide,
    width: u32,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::PANEL_WIDTHS],
        move |settings| {
            settings.panel_widths.apply(side, width);
            Ok(())
        },
    )
}

/// The terminal tabs a worktree had when it was last looked at, for the
/// window to rebuild on the way back in.
///
/// Already normalised — the ratio rules and every positional check ran when
/// the layouts were stored, so what the window receives is what the file
/// keeps and there is no second reading of the rules to drift.
#[tauri::command(async)]
pub(crate) fn pane_layouts(
    state: State<'_, AppState>,
    worktree: String,
) -> Vec<pane_layout::TabLayout> {
    let file = pane_layouts_file(state.config_root());
    pane_layouts_read(&file, &worktree)
}

/// Carry the session identity a pane-owned Zo reported into its durable leaf.
///
/// Unlike hook-reporting agents, Zo can tell the backend its session before
/// the renderer has any pane-session event to persist. The `terms` ordinal map
/// joins those two ledgers here, at the one disk-writing door.
pub(crate) fn carry_zo_pane_sessions(
    layouts: &mut [pane_layout::TabLayout],
    sessions: &HashMap<TermId, zerocode_core::ProviderSession>,
    agents: &HashMap<TermId, &'static str>,
) {
    for layout in layouts {
        for (&ordinal, &term) in &layout.terms {
            if agents.get(&term).copied() != Some(AgentKind::Zo.slug()) {
                continue;
            }
            let Some(session) = sessions.get(&term) else {
                continue;
            };
            if session.key != zerocode_core::SessionKey::SessionId {
                continue;
            }
            // The backend learned this id from the pane's own session.info,
            // while the renderer may still know only that the program is Zo.
            // Preserve its one extra observation (mid-turn) but let the live
            // process's identity replace a stale offered id.
            let interrupted = layout
                .agents
                .get(&ordinal)
                .is_some_and(|wake| wake.interrupted);
            layout
                .running
                .insert(ordinal, AgentKind::Zo.slug().to_string());
            layout.agents.insert(
                ordinal,
                pane_layout::WakeAgent {
                    agent: AgentKind::Zo.slug().to_string(),
                    key: "session_id".to_string(),
                    id: session.id.clone(),
                    transcript_path: session.transcript_path.clone(),
                    interrupted,
                },
            );
        }
    }
}

/// Remember one worktree's terminal tabs — the whole set, replacing what was
/// there, exactly like the watch list: the set is derived state, and a full
/// replacement cannot drift from what is actually open.
///
/// The window sends its ratios raw; the file's two rules (even splits store
/// nothing, uneven ones store three decimals) are [`pane_layout`]'s, applied
/// here so they live in one place.
#[tauri::command(async)]
pub(crate) fn save_pane_layouts(
    state: State<'_, AppState>,
    worktree: String,
    mut layouts: Vec<pane_layout::TabLayout>,
) -> Result<(), String> {
    let file = pane_layouts_file(state.config_root());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let sessions = state.pane_sessions().clone();
    let agents = state.agent_terms().clone();
    carry_zo_pane_sessions(&mut layouts, &sessions, &agents);
    // The one writer, and not once the window is leaving (t-7812 D): the
    // file is then the next window's list, and a save of the window dying
    // would take its tabs off it.
    pane_layout::save_window_set(&file, worktree, layouts, &crate::exit_runtime::leaving)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// One worktree's stored stage — the document tabs, their groups, the split
/// tree — already normalised: the tabs that cannot come back are gone and
/// the tree has folded around them, so the window rebuilds without knowing
/// the rules exist (`hydrateUnifiedFormat`'s job, done on this side).
#[tauri::command(async)]
pub(crate) fn stage_layouts(
    state: State<'_, AppState>,
    worktree: String,
) -> Option<stage_layout::StageLayout> {
    let file = stage_layouts_file(state.config_root());
    stage_layout::read(&file)
        .remove(&worktree)
        .and_then(stage_layout::StageLayout::normalized)
}

/// Remember one worktree's stage — the whole thing, replacing what was
/// there, `None` when the last document tab closed. Same manners as the
/// pane layouts: the rules live in [`stage_layout`], applied on the way in.
#[tauri::command(async)]
pub(crate) fn save_stage_layouts(
    state: State<'_, AppState>,
    worktree: String,
    layout: Option<stage_layout::StageLayout>,
) -> Result<(), String> {
    static LAYOUTS_WRITE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _writing = LAYOUTS_WRITE
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|held| held.into_inner());

    let file = stage_layouts_file(state.config_root());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut held = stage_layout::read(&file);
    stage_layout::store(&mut held, worktree, layout);
    let text = serde_json::to_string(&held).map_err(|error| error.to_string())?;
    zerocode_shell_state::durable_file::replace_bytes(&file, text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Which closed window's screen a restored leaf gets back: its tab's name in
/// the pane layouts file, and the leaf's position inside that tab.
///
/// The tab is named rather than counted. A restored tab may sit on the strip
/// for an hour before anybody opens it, and the tabs around it are closed and
/// pinned to the front in the meantime — by its position, this would hand it
/// the last screen of whichever tab has since taken that seat.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StoredScreen {
    worktree: String,
    id: u64,
    ordinal: usize,
}

/// Put a closed window's screen back into the terminal a restored leaf just
/// spawned — called by the spawn road itself, BEFORE the terminal is held.
///
/// The stored buffer feeds the terminal's own parser — never the child's
/// stdin: this is what the screen SHOWED, not what somebody typed. The
/// payload around it is Orca's replay, whole (`restoreScrollbackBuffers`,
/// I18nProvider-4EBrmTGg.js:46152): a buffer that ends inside the alternate
/// screen is cut at the entry, and the mode reset after it hands the new
/// program a terminal in its right mind. A leaf with nothing stored replays
/// nothing, silently — most leaves, forever.
///
/// **Before the hold, because the reset is only right on a fresh terminal.**
/// Nothing parses a child's output until its terminal is held — the pump
/// drains it — so a screen fed here comes ahead of the program's first byte,
/// whatever the program is and however long its spawn took. The replay used to
/// be a second command the window sent once the spawn had answered, and by
/// then a program that sets its modes once had set them: `resume_session`
/// answers only after zo has published its channel, so zo's `ESC[?2004h` was
/// always parsed first and the reset's `ESC[?2004l` switched bracketed paste
/// off for the pane's whole life. Every multi-line paste into a zo pane the
/// window had restored then arrived as typed keys, and its first line break
/// submitted the prompt (2026-09-17). Kitty keyboard flags, mouse tracking,
/// focus reporting and a hidden cursor went the same way, and the old screen
/// was written over the program's first frame.
pub(crate) fn replay_stored_screen(
    config_root: &Path,
    screen: &StoredScreen,
    terminal: &mut zerocode_pty::Terminal,
) {
    let file = pane_layouts_file(config_root);
    let stored = pane_layout::read(&file)
        .remove(&screen.worktree)
        .unwrap_or_default()
        .into_iter()
        .find(|layout| layout.id == Some(screen.id))
        .and_then(|mut layout| layout.buffers.remove(&screen.ordinal))
        .unwrap_or_default();
    if stored.is_empty() || stored.len() > zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT {
        return;
    }
    terminal.feed(&zerocode_pty::replay_payload(&stored));
}

/// What a new terminal runs, as one editable line.
///
/// Quoted here rather than in the window: the field is read back, edited and
/// saved again, so whatever writes the line and whatever splits it are two
/// halves of one round trip. Keeping both in one place is what lets
/// `a_command_comes_back_as_the_words_it_was` check the property instead of
/// two languages agreeing by inspection — they did not, and a word ending in
/// a backslash lost its closing quote.
#[tauri::command(async)]
pub(crate) fn terminal_command(state: State<'_, AppState>) -> String {
    load_settings_for_boot(state.settings())
        .document
        .terminal_command
}

/// Set what a new terminal runs. An empty line restores the shell.
///
/// One line in, argv stored: [`split_command`] is the only thing that decides
/// where a word ends, so what the person reads in the field and what gets
/// spawned cannot disagree.
///
/// A program that cannot be found is refused here rather than at spawn time.
/// The alternative is what this used to do — accept it, then hand out a shell
/// every time a terminal opens, with the settings field still showing the
/// command as though it were in force.
#[tauri::command(async)]
pub(crate) fn set_terminal_command(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    command: String,
) -> Result<SettingsSnapshot, String> {
    let kept = split_command(&command);
    if let Some(program) = kept.first()
        && !program_exists(program)
    {
        return Err(format!(
            "{program}을(를) 찾지 못했습니다 — 경로를 확인하거나, 띄어쓰기가 있으면 따옴표로 묶으세요"
        ));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::TERMINAL_COMMAND],
        move |settings| {
            settings.terminal_command = quote_command(&kept);
            Ok(())
        },
    )
}

/// Persist the view limit; the retained walk always keeps the absolute cap.
#[tauri::command(async)]
pub(crate) fn set_vault_session_limit(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    limit: usize,
) -> Result<SettingsSnapshot, String> {
    let limit = if limit == 0 {
        0
    } else {
        zerocode_core::vault::Limits::DEFAULT.session_limit(Some(limit))
    };
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::VAULT_SESSION_LIMIT],
        move |settings| {
            settings.vault_session_limit = limit;
            Ok(())
        },
    )
}

/// Which last steps the operator holds for the person (docs/design/
/// computer-use-full-operator.md §1.5). Applied to the live policy after the
/// write, so a refused commit changes nothing.
#[tauri::command(async)]
pub(crate) fn set_computer_confirm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    kind: String,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    use zerocode_core::computer_use::ConfirmKind;
    let kind = ConfirmKind::parse(&kind).ok_or_else(|| format!("unknown confirm kind '{kind}'"))?;
    let key = match kind {
        ConfirmKind::Payment => setting_key::COMPUTER_CONFIRM_PAYMENT,
        ConfirmKind::Transfer => setting_key::COMPUTER_CONFIRM_TRANSFER,
        ConfirmKind::Delete => setting_key::COMPUTER_CONFIRM_DELETE,
    };
    let mut applied = None;
    let snapshot = commit_setting(&app, &webview, &state, &[key], |settings| {
        match kind {
            ConfirmKind::Payment => settings.computer_confirm_payment = on,
            ConfirmKind::Transfer => settings.computer_confirm_transfer = on,
            ConfirmKind::Delete => settings.computer_confirm_delete = on,
        }
        applied = Some(settings.computer_confirm_policy());
        Ok(())
    })?;
    // After the write, so a refused commit changes nothing on the machine.
    if let Some(policy) = applied {
        computer_use::confirm::set_policy(policy);
    }
    Ok(snapshot)
}

/// The person's answer to the operator's question (§1.5).
#[tauri::command]
pub(crate) fn computer_confirm_answer(id: String, allow: bool) -> bool {
    let _crumb = crate::crumbs::Command::enter("computer_confirm_answer");
    computer_use::confirm::answer(&id, allow)
}

/// The window's stop button (§1.3).
#[tauri::command]
pub(crate) fn computer_stop(app: AppHandle) -> serde_json::Value {
    let _crumb = crate::crumbs::Command::enter("computer_stop");
    let answer = computer_use::guard::stop(zerocode_core::computer_use::STOP_REASON_WINDOW);
    let _ = app.emit(
        "computer:activity",
        computer_use::guard::activity_report(None),
    );
    answer
}

/// The window's resume button.
#[tauri::command(async)]
pub(crate) fn computer_resume(app: AppHandle, reset: bool) -> serde_json::Value {
    computer_use::guard::lift();
    let helper = if computer_use::session_stands() {
        computer_use::call("resume", serde_json::json!({ "resetBudget": reset }))
            .unwrap_or_else(|error| serde_json::json!({ "error": error.code }))
    } else {
        serde_json::Value::String("idle".into())
    };
    let _ = app.emit(
        "computer:activity",
        computer_use::guard::activity_report(None),
    );
    serde_json::json!({ "resumed": true, "helper": helper })
}

/// Where the operator stands, for the page.
#[tauri::command]
pub(crate) fn computer_guard_status() -> serde_json::Value {
    let _crumb = crate::crumbs::Command::enter("computer_guard_status");
    computer_use::guard::activity_report(None)
}
