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

/// What the window has counted, for the settings card (t-26583): the launch
/// ledger's counters and what the day has cost as far as the window watched it.
/// What a person set is the settings document's, and comes with its snapshot.
#[derive(serde::Serialize)]
pub(crate) struct HarnessStatus {
    counters: zerocode_core::launch_budget::Counters,
    day_spent_usd: f64,
}

#[tauri::command(async)]
pub(crate) fn harness_status() -> HarnessStatus {
    HarnessStatus {
        counters: crate::launch_budget_door::counters(),
        day_spent_usd: crate::orchestration::gate_book::day_spent_now(),
    }
}

/// The gate's mode and budgets and the launch ceilings a person just chose
/// (t-26583). A ceiling that is not a whole number of launches refuses the save;
/// a budget that is not a budget is none ([`crate::harness_settings`]). Answers
/// with the document's snapshot like every setting does; the beat and the launch
/// ledger take the new numbers at once, and the card's counts are
/// [`harness_status`]'s.
#[tauri::command(async)]
pub(crate) fn set_harness_settings(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    harness: serde_json::Value,
) -> Result<SettingsSnapshot, String> {
    let harness = crate::harness_settings::HarnessSettings::parse(&harness)?;
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::HARNESS],
        move |settings| {
            settings.harness = harness;
            Ok(())
        },
    )?;
    crate::orchestration::gate_book::set_settings(harness.gate);
    crate::landing_watch_sweep::set_enabled(harness.alerts.landing);
    crate::launch_budget_door::set_limits(harness.launches);
    Ok(snapshot)
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
/// 셋으로 나누면 창이 선택의 3분의 2만 흡수한 채 서 있을 수 있다. 팀 보기와
/// 브랜치 보기(`layout`, t-56141)도 같은 값에 산다. 그 낱말을 싣지 않은 호출은
/// 저장된 보기를 그대로 둔다.
#[tauri::command(async)]
pub(crate) fn set_sidebar_view(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    group_by: String,
    sort_by: String,
    project_order: String,
    layout: Option<String>,
) -> Result<SettingsSnapshot, String> {
    let view = SidebarView::parsed(&group_by, &sort_by, &project_order)?;
    let layout = layout.as_deref().map(SidebarLayout::parsed).transpose()?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SIDEBAR_VIEW],
        move |settings| {
            let view = SidebarView {
                layout: layout.unwrap_or(settings.sidebar_view.layout),
                ..view
            };
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

/// Whether a first publish opens beside the pane that made it (t-11958).
/// A view preference like the diff's shape: asked once, and every later
/// publication follows it. The window's hint line under the header band
/// turns it off in place, through this same door.
#[tauri::command(async)]
pub(crate) fn set_artifacts_auto_open_beside(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::ARTIFACTS_AUTO_OPEN_BESIDE],
        move |settings| {
            settings.artifacts_auto_open_beside = on;
            Ok(())
        },
    )
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

/// Whether a conversation lays every step out on one timeline rather than
/// folding the tool work between two answers into one pill (t-81525 B1).
///
/// A view preference and not a page one, for `set_diff_side_by_side`'s reason:
/// a person asks for the whole timeline once, and every conversation they
/// open afterwards — a helper's page, a pane's view, a wire's session — is the
/// one they asked for. The toggle stands on the conversation's own head rather
/// than in the settings window: it is reached while reading, not while
/// configuring.
#[tauri::command(async)]
pub(crate) fn set_conversation_timeline_view(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CONVERSATION_TIMELINE_VIEW],
        move |settings| {
            settings.conversation_timeline_view = on;
            Ok(())
        },
    )
}

/// What an agent's pane opens on: its conversation or its terminal (t-82038).
///
/// A typed enum, so a word the window misspells is refused by deserialization
/// instead of becoming a silent default. The window reads the answer once for
/// each pane, when it is first seen running an agent; a pane the person turned
/// with its head's switch stays where they turned it.
#[tauri::command(async)]
pub(crate) fn set_agent_pane_first_view(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    view: AgentPaneFirstView,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::AGENT_PANE_FIRST_VIEW],
        move |settings| {
            settings.agent_pane_first_view = view;
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
///
/// Less every finished worker's conversation (t-57717): the window opens a
/// workspace with what this answers, and nobody chose a conversation there.
/// On 2026-10-15 and 16 three finished workers' conversations came back off
/// the file as agent panes nobody had called, each on the first visit to its
/// checkout after a restart; a workspace whose set held nothing else now
/// opens as one that stored nothing — the person's own choice for it.
#[tauri::command(async)]
pub(crate) fn pane_layouts(
    state: State<'_, AppState>,
    worktree: String,
) -> Vec<pane_layout::TabLayout> {
    let left = opened_set(&pane_layouts_file(state.config_root()), &worktree);
    note_struck(&state, "read", &worktree, &left.struck);
    left.kept()
}

/// The stored set the window opens `worktree` with, off the file at `file`
/// (t-57717): what [`pane_layouts`] answers, and the finished workers whose
/// conversations it left out. A ledger this window cannot read leaves out
/// nothing.
pub(crate) fn opened_set(file: &std::path::Path, worktree: &str) -> pane_layout::Left {
    let stored = pane_layouts_read(file, worktree);
    let finished = crate::orchestration::finished_conversations();
    pane_layout::without_finished(stored, |wake| {
        finished
            .as_ref()
            .and_then(|finished| finished(&wake.agent, &wake.key, &wake.id))
    })
}

/// The workspaces the boot puts back behind the front, off the file at
/// `file` (t-14036): those standing for a conversation that is not a
/// finished worker's (t-57717) — what [`standing_pane_worktrees`] answers.
pub(crate) fn standing_set(file: &std::path::Path) -> Vec<String> {
    let mut layouts = pane_layout::read(file);
    if let Some(finished) = crate::orchestration::finished_conversations() {
        for tabs in layouts.values_mut() {
            *tabs = pane_layout::without_finished(std::mem::take(tabs), |wake| {
                finished(&wake.agent, &wake.key, &wake.id)
            })
            .kept();
        }
    }
    pane_layout::standing_worktrees(&layouts)
}

/// A save's set less every worker's conversation (t-57717): what
/// [`save_pane_layouts`] writes, each tab in the order given. A ledger this
/// window cannot read strikes nothing.
pub(crate) fn saved_set(layouts: Vec<pane_layout::TabLayout>) -> pane_layout::Left {
    let workers = crate::orchestration::worker_conversations();
    pane_layout::without_finished(layouts, |wake| {
        workers
            .as_ref()
            .and_then(|workers| workers(&wake.agent, &wake.key, &wake.id))
    })
}

/// One line in the window's log for the workers' conversations a pane-layout
/// road left out (t-57717) — nothing when it left out none.
fn note_struck(state: &AppState, road: &str, worktree: &str, struck: &[String]) {
    if struck.is_empty() {
        return;
    }
    note_window_event(
        state.local_data_root(),
        &format!(
            "pane layouts of {worktree}: the {road} left out {} worker conversation(s) — {}",
            struck.len(),
            struck.join(", ")
        ),
    );
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
    // Every conversation the set names as sent, for the owed rule below.
    let named: Vec<(String, String)> = layouts
        .iter()
        .flat_map(|layout| layout.agents.values())
        .map(|wake| (wake.agent.clone(), wake.id.clone()))
        .collect();
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
            let displaced = layout.agents.insert(
                ordinal,
                pane_layout::WakeAgent {
                    agent: AgentKind::Zo.slug().to_string(),
                    key: "session_id".to_string(),
                    id: session.id.clone(),
                    transcript_path: session.transcript_path.clone(),
                    interrupted,
                },
            );
            // Another conversation the leaf stood for is owed, never dropped
            // (t-76489, t-12063): a record that named a claude coordinator
            // here and now names the pane's zo must not forget the claude —
            // unless another leaf of the set names it, where it stands.
            if let Some(displaced) = displaced
                && (displaced.agent != AgentKind::Zo.slug() || displaced.id != session.id)
                && named
                    .iter()
                    .filter(|(agent, id)| *agent == displaced.agent && *id == displaced.id)
                    .count()
                    < 2
            {
                layout.owed.push(displaced);
            }
        }
    }
}

/// Whether the shell `term` holds the conversation `wake` names now
/// (t-76489): the agent this window knows the pane as and the session it
/// reported — the two records [`crate::conversation_wake::panes_naming`]
/// reads — and `in_front`, that the pane's agent still holds its terminal
/// ([`crate::agent_tools_runtime::agent_in_front_within`]: the program in
/// front of its shell is that agent, not the shell and not a command run
/// after it). Both records outlive an agent quit back to its shell, so a
/// pane that ended the conversation and the pane that reopened it since
/// would both answer by the records alone, and the first by order would
/// take the second's place. `None` when the records name the conversation
/// and `in_front` cannot tell (t-76489 r5).
pub(crate) fn pane_holds(
    sessions: &HashMap<TermId, zerocode_core::ProviderSession>,
    agents: &HashMap<TermId, &'static str>,
    in_front: &dyn Fn(TermId) -> Option<bool>,
    wake: &pane_layout::WakeAgent,
    term: TermId,
) -> Option<bool> {
    let named = agents.get(&term).copied() == Some(wake.agent.as_str())
        && sessions
            .get(&term)
            .is_some_and(|session| session.id == wake.id);
    if named { in_front(term) } else { Some(false) }
}

/// One line in the window's log for the leaves a pane-layout road took a
/// conversation off because another leaf names it too (t-76489) — nothing
/// when it took none.
fn note_doubled(state: &AppState, road: &str, struck: &[String]) {
    if struck.is_empty() {
        return;
    }
    note_window_event(
        state.local_data_root(),
        &format!(
            "pane layouts: the {road} kept one leaf per conversation and took {} off {} other leaf(s)",
            struck.join(", "),
            struck.len()
        ),
    );
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
) -> Result<Vec<Option<u64>>, String> {
    let file = pane_layouts_file(state.config_root());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let sessions = state.pane_sessions().clone();
    let agents = state.agent_terms().clone();
    carry_zo_pane_sessions(&mut layouts, &sessions, &agents);
    // A worker's conversation is the ledger's to bring back, never this
    // file's (t-57717): a worker pane the window seated in a tab without
    // marking it the ledger's — a detached worker row pressed, a worker
    // listed and opened — was saved here, and came back off the file as an
    // agent pane after its worker had finished. The person's own panes and a
    // run's coordinator stay; a ledger this window cannot read strikes
    // nothing.
    let left = saved_set(layouts);
    note_struck(&state, "save", &worktree, &left.struck);
    let kept: Vec<bool> = left.tabs.iter().map(Option::is_some).collect();
    // The one writer, and not once the window is leaving (t-7812 D): the
    // file is then the next window's list, and a save of the window dying
    // would take its tabs off it.
    // The names the tabs were stored under go back to the window (t-14036):
    // a tab born in this session is nameless until the file names it, and the
    // stage can only say where a named tab stands. One name per tab sent, in
    // the order sent — none for a tab the save struck whole.
    // One leaf per conversation across the file (t-76489): the leaf whose
    // pane holds it keeps it, so a dead pane's copy — a split that died at
    // the writer lease of a conversation a tab beside it held — never wakes
    // in the living one's place.
    let (named, doubled) = pane_layout::save_window_set(
        &file,
        worktree,
        left.kept(),
        &crate::exit_runtime::leaving,
        &|wake, term| {
            let in_front = |term| {
                let shell_pane = { state.shell_panes().contains(&term) };
                crate::agent_tools_runtime::agent_in_front_within(
                    state.terminals(),
                    shell_pane,
                    term,
                    wake.agent.as_str(),
                    None,
                )
            };
            pane_holds(&sessions, &agents, &in_front, wake, term)
        },
    )
    .map(Option::unwrap_or_default)
    .map_err(|error| error.to_string())?;
    note_doubled(&state, "save", &doubled);
    let mut named = named.into_iter();
    Ok(kept
        .into_iter()
        .map(|kept| if kept { named.next().flatten() } else { None })
        .collect())
}

/// The workspaces whose conversations were running when the window last went,
/// for the boot to put back behind the one in front (t-14036) — less the
/// finished workers' conversations, which no road brings back on its own
/// (t-57717, [`pane_layouts`]).
#[tauri::command(async)]
pub(crate) fn standing_pane_worktrees(state: State<'_, AppState>) -> Vec<String> {
    standing_set(&pane_layouts_file(state.config_root()))
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

/// Only the person may require or relax app sharing (t-74582).
#[tauri::command(async)]
pub(crate) fn set_computer_share_required(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPUTER_SHARE_REQUIRED],
        |settings| {
            settings.computer_share_required = on;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_computer_input_policy(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    policy: zerocode_core::computer_use_protocol::execution::InputPolicy,
) -> Result<SettingsSnapshot, String> {
    let mut revoked = false;
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPUTER_INPUT_POLICY],
        |settings| {
            revoked = settings.computer_input_policy
                == zerocode_core::computer_use_protocol::execution::InputPolicy::SharedDesktop
                && policy
                    == zerocode_core::computer_use_protocol::execution::InputPolicy::Background;
            settings.computer_input_policy = policy;
            Ok(())
        },
    )?;
    if revoked {
        computer_use::guard::stop(zerocode_core::computer_use::STOP_REASON_WINDOW);
        let _ = app.emit(
            "computer:activity",
            computer_use::guard::activity_report(None),
        );
    }
    Ok(snapshot)
}

/// Whether a live reflex run may start (`computer_live_reflex`, t-10221):
/// the settings page offers it on only when the platform, both permissions
/// and a check with this helper stand; the reflex door reads it again at
/// every start, so nothing else keeps a copy of it.
#[tauri::command(async)]
pub(crate) fn set_computer_live_reflex(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPUTER_LIVE_REFLEX],
        |settings| {
            settings.computer_live_reflex = on;
            Ok(())
        },
    )
}

/// Which road Computer Use's generator takes (`computer_generator_road`,
/// t-10372): the Claude Code login, a person's own API key, or nobody. A
/// writer reads it when it is made, so the next walk or autopilot takes the
/// road chosen here, with no restart.
#[tauri::command(async)]
pub(crate) fn set_computer_generator_road(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    road: zerocode_core::type_value::GeneratorRoad,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPUTER_GENERATOR_ROAD],
        move |settings| {
            settings.computer_generator_road = road;
            Ok(())
        },
    )
}

/// The person's answer to the operator's question (§1.5).
#[tauri::command]
pub(crate) fn computer_confirm_answer(id: String, allow: bool) -> bool {
    let _crumb = crate::crumbs::Command::enter("computer_confirm_answer");
    computer_use::confirm::answer(&id, allow)
}

/// The person takes control of the window shared with an agent (t-73906):
/// the request in flight stops at its next step and every act after it is
/// refused until they hand it back.
#[tauri::command(async)]
pub(crate) fn computer_take_control() {
    let _crumb = crate::crumbs::Command::enter("computer_take_control");
    computer_use::share::take_control();
}

/// The person hands the shared window back to the agent (t-73906).
#[tauri::command(async)]
pub(crate) fn computer_give_back() {
    let _crumb = crate::crumbs::Command::enter("computer_give_back");
    computer_use::share::give_back();
}

/// End the displayed share and forget that app's answer. The generation
/// prevents a late press from ending a share handed over since.
#[tauri::command(async)]
pub(crate) fn computer_share_end(app: String, generation: u64) -> serde_json::Value {
    let _crumb = crate::crumbs::Command::enter("computer_share_end");
    computer_use::share::end(&app, generation)
}

/// The person hands over, from the band, the app an agent waits for under
/// required sharing (t-74826): that app only, and only while it waits.
#[tauri::command]
pub(crate) fn computer_share_hand_over(app: String) -> bool {
    let _crumb = crate::crumbs::Command::enter("computer_share_hand_over");
    computer_use::share::hand_over(&app)
}

/// What is shared with an agent now, for the band's first paint (t-73906).
#[tauri::command(async)]
pub(crate) fn computer_share_state(panel_label: Option<String>) -> serde_json::Value {
    let _crumb = crate::crumbs::Command::enter("computer_share_state");
    computer_use::share::panel_label(panel_label);
    computer_use::share::state()
}

/// The page's answer to the window's request about its own controls
/// (t-73906): a look, one control's facts, or a press it made. Called from
/// the window's page and from nowhere else.
#[tauri::command]
pub(crate) fn computer_self_answer(id: u64, answer: serde_json::Value) -> bool {
    let _crumb = crate::crumbs::Command::enter("computer_self_answer");
    computer_use::self_surface::answered(id, answer)
}

/// The person's code for a person's-turn card that has a line for one
/// (t-40807): what they typed, judged by core's one rule and handed once to
/// the handoff that waits, for the window to type into the field the agent
/// named. Called from the window's page and from nowhere else. The answer is
/// a word the page reads — `delivered`, `invalid`, `gone` — never the code.
#[tauri::command]
pub(crate) fn computer_handoff_code(
    id: String,
    code: String,
) -> computer_use::confirm::CodeVerdict {
    let _crumb = crate::crumbs::Command::enter("computer_handoff_code");
    computer_use::confirm::answer_code(&id, &code)
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

/// The window's resume button: the person lifts the stop whose revision the
/// band showed (`guard::resume`) — from the main window's band alone
/// (t-62872): the board pop-out's band offers the stop, safe from anywhere,
/// and not the resume, whose work browsers only the main window holds.
#[tauri::command(async)]
pub(crate) fn computer_resume(
    app: AppHandle,
    webview: tauri::Webview,
    reset: bool,
    revision: Option<u64>,
) -> Result<serde_json::Value, String> {
    from_the_main_webview(&webview).map_err(|_| {
        "computer_resume_main_window_only: 정지는 메인 창의 띠에서만 다시 시작합니다".to_string()
    })?;
    // The person's resume (t-66753 A): the one place that names them, and
    // the helper is told it is theirs.
    let lifter = computer_use::guard::Lifter::Person { revision };
    let resumed = computer_use::guard::resume(
        lifter,
        || computer_use::guard::helper_resume(reset, lifter),
        computer_use::guard::stop_helper,
    );
    let _ = app.emit(
        "computer:activity",
        computer_use::guard::activity_report(None),
    );
    Ok(resumed.answer())
}

/// The work screen panel's look (t-59372): the newest look the pane `term`
/// took under the assignment `scope` it still holds — its number, when it was
/// taken and its picture when one is kept. Only the window's own page asks,
/// and only about a pane the window holds; a pane whose assignment the window
/// cannot tell is refused, never shown.
#[tauri::command(async)]
pub(crate) fn computer_progress_frame(
    app: AppHandle,
    webview: tauri::Webview,
    term: TermId,
    scope: String,
    seq: Option<u64>,
) -> Result<Option<serde_json::Value>, String> {
    let _crumb = crate::crumbs::Command::enter("computer_progress_frame");
    from_the_main_webview(&webview)?;
    let held = crate::artifact_runtime::held_pane(&app, &crate::hooks::pane_key_of(term))
        .ok_or("computer_progress_pane_unknown: 이 창에 없는 판입니다")?;
    let holds = crate::agent_tools_runtime::computer_frame_scope(&app, &held).ok_or(
        "computer_progress_scope_unknown: 이 판의 지금 일과 대화를 가릴 수 없어 화면을 보이지 않습니다",
    )?;
    if holds != scope {
        return Ok(None);
    }
    let Some(mut look) = computer_use::observe::owned_look(&held.key, &scope) else {
        return Ok(None);
    };
    if seq.is_some_and(|wanted| look["seq"].as_u64().is_none_or(|newest| newest < wanted)) {
        return Ok(None);
    }
    look["term"] = term.into();
    look["scope"] = scope.into();
    Ok(Some(look))
}

/// Where the operator stands, for the page.
#[tauri::command]
pub(crate) fn computer_guard_status() -> serde_json::Value {
    let _crumb = crate::crumbs::Command::enter("computer_guard_status");
    computer_use::guard::activity_report(None)
}
